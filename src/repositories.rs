//! Synchronous, short SQLite operations. Never retain a database guard across an await.

use std::{
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    sync::{Arc, Weak},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use rustix::{
    fs::{Mode, OFlags, open, openat},
    process::geteuid,
};
use tokio::sync::Semaphore;

use crate::{
    config::{Limits, Storage},
    error::AppError,
};

mod registry;
pub(crate) use registry::PendingRoutes;
pub use registry::{MessageRoute, RouteKind, RouteReservation, Session};
mod ingress;
pub(crate) use ingress::IncomingReply;
pub use ingress::IngestOutcome;

const SCHEMA_VERSION: i64 = 2;
const APPLICATION_ID: i64 = 0x544d4352; // TMCR

/// One private connection; services will expose narrowly scoped transactions, not raw SQL.
pub struct Database {
    connection: Connection,
    registry: registry::RegistryPolicy,
    reserved_routes: Arc<registry::ReservedRoutes>,
    pending_routes: Vec<Weak<()>>,
    pub(crate) poll_admission: Arc<Semaphore>,
}

impl Database {
    /// Open in an existing private owned directory, initializing only an empty database.
    /// Ancestor directories must be trusted; existing permissions are never repaired.
    pub fn open(settings: &Storage, limits: &Limits) -> Result<Self, AppError> {
        let registry = registry::RegistryPolicy::new(settings, limits)?;
        if !(1_048_576..=1_073_741_824).contains(&settings.max_database_bytes) {
            return Err(AppError::InvalidConfig("storage.max_database_bytes"));
        }
        let file = protected_file(&settings.path)?;
        if file
            .metadata()
            .map_err(|_| AppError::StorageUnavailable)?
            .len()
            > u64::from(settings.max_database_bytes)
        {
            return Err(AppError::StorageCapacity);
        }
        let mut connection = Connection::open_with_flags(
            &settings.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::from_millis(100))?;
        connection.set_prepared_statement_cache_capacity(8);
        check_schema(&connection)?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA trusted_schema = OFF;
             PRAGMA synchronous = FULL;
             PRAGMA cache_size = -512;
             PRAGMA mmap_size = 0;
             PRAGMA temp_store = MEMORY;
             PRAGMA journal_size_limit = 0;",
        )?;
        let mode: String =
            connection.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
        if mode != "delete" {
            return Err(AppError::StorageFailure);
        }
        let page_size: u32 = connection.pragma_query_value(None, "page_size", |r| r.get(0))?;
        let pages = settings.max_database_bytes / page_size;
        let actual: u32 =
            connection.query_row(&format!("PRAGMA max_page_count = {pages}"), [], |r| {
                r.get(0)
            })?;
        if actual > pages {
            return Err(AppError::StorageCapacity);
        }
        initialize(&mut connection)?;
        Ok(Self {
            connection,
            registry,
            reserved_routes: Arc::default(),
            pending_routes: Vec::new(),
            poll_admission: Arc::new(Semaphore::new(1)),
        })
    }

    /// Read the committed schema version without exposing a connection to callers.
    pub fn schema_version(&self) -> Result<i64, AppError> {
        Ok(self
            .connection
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }
}

fn check_schema(connection: &Connection) -> Result<i64, AppError> {
    let (version, application, empty): (i64, i64, bool) = connection.query_row(
        "SELECT user_version, application_id,
         NOT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*')
         FROM pragma_user_version, pragma_application_id",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    match (version, application, empty) {
        (0, 0, true) | (1..=SCHEMA_VERSION, APPLICATION_ID, false) => Ok(version),
        _ => Err(AppError::UnsupportedSchema(version)),
    }
}

fn initialize(connection: &mut Connection) -> Result<(), AppError> {
    // SQLite ignores this initialization setting inside a transaction. Set it before DDL.
    if check_schema(connection)? == 0 {
        connection.pragma_update(None, "auto_vacuum", 2)?;
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Recheck under the write lock: another opener may have initialized the file meanwhile.
    let version = check_schema(&transaction)?;
    if version == 0 {
        transaction.execute_batch(include_str!("repositories/schema.sql"))?;
        transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
    }
    if version < 2 {
        transaction.execute_batch(include_str!("repositories/ingress.sql"))?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    let vacuum: i64 = transaction.pragma_query_value(None, "auto_vacuum", |r| r.get(0))?;
    if vacuum != 2 {
        return Err(AppError::StorageFailure);
    }
    transaction.commit()?;
    Ok(())
}

fn protected_file(path: &Path) -> Result<File, AppError> {
    let unavailable = || AppError::StorageUnavailable;
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(unavailable());
    }
    let parent = File::from(
        open(
            path.parent().ok_or_else(unavailable)?,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| unavailable())?,
    );
    let metadata = parent.metadata().map_err(|_| unavailable())?;
    if metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(unavailable());
    }
    let name = path.file_name().ok_or_else(unavailable)?;
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = match openat(
        &parent,
        name,
        flags | OFlags::CREATE | OFlags::EXCL,
        Mode::RUSR | Mode::WUSR,
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::EXIST) => {
            openat(&parent, name, flags, Mode::empty()).map_err(|_| unavailable())?
        }
        Err(_) => return Err(unavailable()),
    };
    let file = File::from(fd);
    check_file(&file)?;
    // A pre-existing hot journal must be protected before SQLite can recover it.
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = name.to_os_string();
        sidecar.push(suffix);
        match openat(&parent, &sidecar, flags, Mode::empty()) {
            Ok(fd) => check_file(&File::from(fd))?,
            Err(rustix::io::Errno::NOENT) => (),
            Err(_) => return Err(unavailable()),
        }
    }
    Ok(file)
}

fn check_file(file: &File) -> Result<(), AppError> {
    let m = file.metadata().map_err(|_| AppError::StorageUnavailable)?;
    if !m.is_file()
        || m.uid() != geteuid().as_raw()
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
        || m.mode() & 0o600 != 0o600
    {
        return Err(AppError::StorageUnavailable);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Directory(std::path::PathBuf);

    impl Directory {
        fn new() -> Self {
            use std::os::unix::fs::DirBuilderExt;

            let mut random = [0; 8];
            getrandom::fill(&mut random).unwrap();
            let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "migration-test-{:016x}",
                u64::from_ne_bytes(random)
            ));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            Self(path)
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn migration_failure_rolls_back_tables_cursor_and_version() {
        let directory = Directory::new();
        let mut connection = Connection::open(directory.0.join("test.sqlite")).unwrap();
        // Exhaust pages partway through DDL, rather than fail before the transaction starts.
        connection
            .pragma_update(None, "max_page_count", 20)
            .unwrap();
        assert_eq!(initialize(&mut connection), Err(AppError::StorageCapacity));
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
        assert_eq!(check_schema(&connection).unwrap(), 0);
        connection
            .pragma_update(None, "max_page_count", 1024)
            .unwrap();
        initialize(&mut connection).unwrap();
        assert_eq!(check_schema(&connection).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn configured_page_limit_rejects_growth_without_losing_committed_rows() {
        let directory = Directory::new();
        let settings = Storage {
            path: directory.0.join("test.sqlite"),
            max_database_bytes: 1_048_576,
            ..Storage::default()
        };
        let database = Database::open(&settings, &Limits::default()).unwrap();
        database
            .connection
            .execute(
                "INSERT INTO sessions VALUES('a', NULL, 'Synthetic', 100, 100, 200)",
                [],
            )
            .unwrap();
        let result = database
            .connection
            .execute("UPDATE sessions SET title = hex(zeroblob(600000))", []);
        assert_eq!(
            result.map_err(AppError::from),
            Err(AppError::StorageCapacity)
        );
        drop(database);
        let database = Database::open(&settings, &Limits::default()).unwrap();
        let title: String = database
            .connection
            .query_row(
                "SELECT title FROM sessions WHERE session_key = 'a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Synthetic");
    }
}
