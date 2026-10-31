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
mod inbox;
pub use inbox::{ClaimedReply, InboxCursor, InboxPage, InboxReply, ReplyState, ReplyTarget};
mod processing;
pub use processing::{ProcessingStatus, ReconciledProcessing};
mod notices;
pub use notices::{NoticeAttempt, NoticeKind, NoticePolicy};
mod notifications;
pub(crate) use notifications::{InboxNotifications, InboxSubscription};
mod retention;
pub use retention::{CleanupBatch, CleanupSweep};
mod maintenance;
pub use maintenance::DiskUsage;
pub(crate) use maintenance::RECLAIM_PAGES;

const SCHEMA_VERSION: i64 = 7;
const APPLICATION_ID: i64 = 0x544d4352; // TMCR

/// One private connection; services will expose narrowly scoped transactions, not raw SQL.
pub struct Database {
    connection: Connection,
    files: maintenance::DatabaseFiles,
    main_file_limit_bytes: u64,
    registry: registry::RegistryPolicy,
    reserved_routes: Arc<registry::ReservedRoutes>,
    pending_routes: Vec<Weak<String>>,
    pub(crate) poll_admission: Arc<Semaphore>,
    pub(crate) notice_admission: Arc<Semaphore>,
    pub(crate) cleanup_admission: Arc<Semaphore>,
    pub(crate) inbox_notifications: InboxNotifications,
}

impl Database {
    /// Open in an existing private owned directory, initializing only an empty database.
    /// Ancestor directories must be trusted; existing permissions are never repaired.
    pub fn open(settings: &Storage, limits: &Limits) -> Result<Self, AppError> {
        let registry = registry::RegistryPolicy::new(settings, limits)?;
        let inbox_notifications = InboxNotifications::new(limits)?;
        if !(1_048_576..=1_073_741_824).contains(&settings.max_database_bytes) {
            return Err(AppError::InvalidConfig("storage.max_database_bytes"));
        }
        let files = protected_file(&settings.path)?;
        if files
            .database
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
            files,
            main_file_limit_bytes: u64::from(pages) * u64::from(page_size),
            registry,
            reserved_routes: Arc::default(),
            pending_routes: Vec::new(),
            poll_admission: Arc::new(Semaphore::new(1)),
            notice_admission: Arc::new(Semaphore::new(1)),
            cleanup_admission: Arc::new(Semaphore::new(1)),
            inbox_notifications,
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
    if version < 3 {
        transaction.execute_batch(include_str!("repositories/inbox.sql"))?;
    }
    if version < 4 {
        transaction.execute_batch(include_str!("repositories/processing.sql"))?;
    }
    if version < 5 {
        transaction.execute_batch(include_str!("repositories/notices.sql"))?;
    }
    if version < 6 {
        transaction.execute_batch(include_str!("repositories/retention.sql"))?;
    }
    if version < 7 {
        transaction.execute_batch(include_str!("repositories/presentation.sql"))?;
    }
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    let vacuum: i64 = transaction.pragma_query_value(None, "auto_vacuum", |r| r.get(0))?;
    if vacuum != 2 {
        return Err(AppError::StorageFailure);
    }
    transaction.commit()?;
    Ok(())
}

fn protected_file(path: &Path) -> Result<maintenance::DatabaseFiles, AppError> {
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
    let mut journal_name = name.to_os_string();
    journal_name.push("-journal");
    Ok(maintenance::DatabaseFiles {
        database: file,
        directory: parent,
        journal_name,
    })
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
                "INSERT INTO sessions VALUES('a', NULL, 'Synthetic', 100, 100, 200, NULL)",
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

    #[test]
    fn disk_accounting_includes_live_rollback_journal_and_preserves_durability_settings() {
        let directory = Directory::new();
        let settings = Storage {
            path: directory.0.join("test.sqlite"),
            ..Storage::default()
        };
        let mut database = Database::open(&settings, &Limits::default()).unwrap();
        let key = database
            .register_session(None, None, "Synthetic", None, 0)
            .unwrap()
            .session_key;
        let before = database.disk_usage().unwrap();
        assert_eq!((before.journal_bytes, before.temporary_disk_bytes), (0, 0));
        database
            .connection
            .execute_batch("BEGIN IMMEDIATE; UPDATE sessions SET title = 'Changed'")
            .unwrap();
        let during = database.disk_usage().unwrap();
        let metadata = std::fs::metadata(directory.0.join("test.sqlite-journal")).unwrap();
        assert!(during.journal_bytes > 0);
        assert_eq!(during.journal_bytes, metadata.len());
        assert_eq!(during.journal_allocated_bytes, metadata.blocks() * 512);
        let metadata = std::fs::metadata(&settings.path).unwrap();
        assert_eq!(during.database_allocated_bytes, metadata.blocks() * 512);
        assert_eq!(during.database_bytes, metadata.len());
        database.connection.execute_batch("ROLLBACK").unwrap();
        assert_eq!(database.disk_usage().unwrap(), before);
        assert_eq!(database.session(&key).unwrap().title.as_str(), "Synthetic");
        database.reclaim_batch().unwrap();
        for (pragma, expected) in [
            ("synchronous", 2),
            ("auto_vacuum", 2),
            ("temp_store", 2),
            ("foreign_keys", 1),
        ] {
            assert_eq!(
                database
                    .connection
                    .pragma_query_value(None, pragma, |r| r.get::<_, i64>(0))
                    .unwrap(),
                expected
            );
        }
        let mode: String = database
            .connection
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "delete");
    }

    #[test]
    fn disk_accounting_rejects_unsafe_journal_instead_of_reporting_zero() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = Directory::new();
        let settings = Storage {
            path: directory.0.join("test.sqlite"),
            ..Storage::default()
        };
        let database = Database::open(&settings, &Limits::default()).unwrap();
        let journal = directory.0.join("test.sqlite-journal");
        symlink(&settings.path, &journal).unwrap();
        assert_eq!(database.disk_usage(), Err(AppError::StorageUnavailable));
        std::fs::remove_file(&journal).unwrap();
        File::create(&journal)
            .unwrap()
            .set_permissions(std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert_eq!(database.disk_usage(), Err(AppError::StorageUnavailable));
        std::fs::remove_file(&journal).unwrap();
        assert_eq!(database.disk_usage().unwrap().journal_bytes, 0);
    }

    #[test]
    fn reclamation_commit_failure_rolls_back_page_moves_without_losing_valid_rows() {
        let directory = Directory::new();
        let settings = Storage {
            path: directory.0.join("test.sqlite"),
            ..Storage::default()
        };
        let mut database = Database::open(&settings, &Limits::default()).unwrap();
        let key = database
            .register_session(None, None, "Synthetic", None, 0)
            .unwrap()
            .session_key;
        // Create free overflow pages while retaining the real session row and indexes.
        database
            .connection
            .execute("UPDATE sessions SET title = hex(zeroblob(262144))", [])
            .unwrap();
        database
            .connection
            .execute("UPDATE sessions SET title = 'Synthetic'", [])
            .unwrap();
        database.connection.busy_timeout(Duration::ZERO).unwrap();
        let before = database.disk_usage().unwrap();
        assert!(before.free_pages > 32);
        let reader = Connection::open(&settings.path).unwrap();
        reader
            .execute_batch("BEGIN; SELECT count(*) FROM sessions")
            .unwrap();
        // The shared read lock permits page moves but denies the exclusive commit lock.
        // Zero busy timeout makes the failure deterministic without timing-dependent waits.
        assert_eq!(database.reclaim_batch(), Err(AppError::StorageFailure));
        assert!(database.connection.is_autocommit());
        assert_eq!(database.disk_usage().unwrap(), before);
        assert_eq!(database.session(&key).unwrap().title.as_str(), "Synthetic");
        reader.execute_batch("ROLLBACK").unwrap();
        let after = database.reclaim_batch().unwrap();
        assert_eq!(before.free_pages - after.free_pages, 32);
        assert!(after.database_bytes < before.database_bytes);
        drop(database);
        let database = Database::open(&settings, &Limits::default()).unwrap();
        assert_eq!(database.disk_usage().unwrap(), after);
        assert_eq!(database.session(&key).unwrap().title.as_str(), "Synthetic");
    }
}
