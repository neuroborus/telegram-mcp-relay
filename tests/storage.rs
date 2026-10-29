use std::{
    fs::{self, DirBuilder, OpenOptions, Permissions},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt, symlink},
    path::PathBuf,
};

use rusqlite::Connection;
use telegram_mcp_relay::{
    config::{Limits, Storage},
    error::AppError,
    repositories::Database,
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("LOCAL_ARTIFACTS")
            .join(format!("storage-test-{:016x}", u64::from_ne_bytes(random)));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        DirBuilder::new().mode(0o700).create(&path).unwrap();
        Self(path)
    }

    fn settings(&self) -> Storage {
        Storage {
            path: self.0.join("relay.sqlite"),
            ..Storage::default()
        }
    }

    fn connect(&self) -> Connection {
        let connection = Connection::open(self.settings().path).unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA trusted_schema = OFF;")
            .unwrap();
        connection
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn add_session(connection: &Connection, key: &str, external: Option<&str>) {
    connection
        .execute(
            "INSERT INTO sessions VALUES(?1, ?2, 'Same title', 100, 100, 200)",
            (key, external),
        )
        .unwrap();
}

fn add_reply(
    connection: &Connection,
    id: &str,
    update: i64,
    message: i64,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO replies(reply_id, telegram_update_id, telegram_chat_id,
         telegram_message_id, reply_to_bot_message_id, session_key, text_or_caption,
         attachment_metadata_json, created_at, expires_at)
         VALUES(?1, ?2, -101, ?3, 7, 'a', 'Synthetic incoming reply',
         '{\"file_id\":\"synthetic-file\",\"name\":\"sample.txt\"}', 110, 210)",
        (id, update, message),
    )
}

fn assert_constraint(connection: &Connection, sql: &str) {
    assert!(matches!(
        connection.execute(sql, []),
        Err(rusqlite::Error::SqliteFailure(cause, _))
            if cause.code == rusqlite::ErrorCode::ConstraintViolation
    ));
}

#[test]
fn initializes_private_database_and_reopens_without_resetting_state() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    let database = Database::open(&settings, &Limits::default()).unwrap();
    assert_eq!(database.schema_version().unwrap(), 1);
    assert_eq!(
        fs::metadata(&settings.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let connection = fixture.connect();
    let vacuum: i64 = connection
        .pragma_query_value(None, "auto_vacuum", |r| r.get(0))
        .unwrap();
    let mode: String = connection
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(vacuum, 2);
    assert_eq!(mode, "delete");
    let cursor: Option<i64> = connection
        .query_row(
            "SELECT last_durably_processed_update_id FROM telegram_cursor",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cursor, None);
    add_session(&connection, "a", Some("external-a"));
    add_reply(&connection, "reply-a", 11, 8).unwrap();
    connection
        .execute(
            "UPDATE telegram_cursor SET last_durably_processed_update_id = 11",
            [],
        )
        .unwrap();
    drop(connection);
    drop(database);
    let reopened = Database::open(&settings, &Limits::default()).unwrap();
    assert_eq!(reopened.schema_version().unwrap(), 1);
    let connection = fixture.connect();
    let saved: (String, i64) = connection.query_row(
        "SELECT reply_id, last_durably_processed_update_id FROM replies CROSS JOIN telegram_cursor",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(saved, ("reply-a".into(), 11));
}

#[test]
fn constraints_preserve_identity_deduplication_and_independent_reply_retention() {
    let fixture = Fixture::new();
    let _database = Database::open(&fixture.settings(), &Limits::default()).unwrap();
    let connection = fixture.connect();
    add_session(&connection, "a", Some("external-a"));
    add_session(&connection, "b", None);
    add_session(&connection, "c", None);
    assert_constraint(
        &connection,
        "UPDATE sessions SET external_session_id = 'external-a' WHERE session_key = 'b'",
    );
    connection
        .execute_batch(
            "INSERT INTO message_routes VALUES(-101, 7, 'a', 'text', 100, 200);
         INSERT INTO message_routes VALUES(-102, 7, 'b', 'document', 100, 200);",
        )
        .unwrap();
    assert_constraint(
        &connection,
        "INSERT INTO message_routes VALUES(-101, 7, 'b', 'text', 100, 200)",
    );
    add_reply(&connection, "r1", 11, 8).unwrap();
    add_reply(&connection, "r2", 12, 9).unwrap();
    assert!(add_reply(&connection, "r3", 11, 10).is_err());
    assert!(add_reply(&connection, "r3", 13, 8).is_err());
    assert_constraint(&connection, "UPDATE replies SET session_key = 'missing'");
    assert_constraint(
        &connection,
        "UPDATE replies SET attachment_metadata_json = 'invalid'",
    );
    assert_constraint(&connection, "UPDATE replies SET expires_at = 0");
    assert_constraint(&connection, "INSERT INTO telegram_cursor VALUES(2, 12)");
    // Removing a route is independent of an already accepted reply's retention.
    connection
        .execute("DELETE FROM message_routes WHERE session_key = 'a'", [])
        .unwrap();
    assert_constraint(&connection, "DELETE FROM sessions WHERE session_key = 'a'");
    let replies: i64 = connection
        .query_row("SELECT count(*) FROM replies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(replies, 2);
    connection
        .execute(
            "INSERT INTO acknowledgment_receipts VALUES('r1', 'a', 'token-a', 11, 120, 220)",
            [],
        )
        .unwrap();
    connection.execute("DELETE FROM replies", []).unwrap();
    let receipts: i64 = connection
        .query_row("SELECT count(*) FROM acknowledgment_receipts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(receipts, 1);
    assert_constraint(&connection, "DELETE FROM sessions WHERE session_key = 'a'");
}

#[test]
fn claim_shape_and_notice_deduplication_are_guarded() {
    let fixture = Fixture::new();
    let _database = Database::open(&fixture.settings(), &Limits::default()).unwrap();
    let connection = fixture.connect();
    add_session(&connection, "a", None);
    add_reply(&connection, "r1", 11, 8).unwrap();
    for sql in [
        "UPDATE replies SET state = 'claimed'",
        "UPDATE replies SET state = 'processing'",
        "UPDATE replies SET claim_token = 'token-a'",
        "UPDATE replies SET notice_attempted = 2",
    ] {
        assert_constraint(&connection, sql);
    }
    connection.execute("UPDATE replies SET state = 'claimed', claimed_at = 120, claim_token = 'token-a', claim_expires_at = 180", []).unwrap();
    connection.execute("UPDATE replies SET state = 'processing', processing_started_at = 130, claim_expires_at = NULL", []).unwrap();
    // Reconciliation can rotate a token without discarding the earlier start deadline.
    connection
        .execute(
            "UPDATE replies SET claimed_at = 190, claim_token = 'token-b'",
            [],
        )
        .unwrap();
    let first_start: i64 = connection
        .query_row("SELECT processing_started_at FROM replies", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(first_start, 130);
    connection
        .execute(
            "INSERT INTO notice_attempts VALUES(11, -101, 8, 'unconfirmed', NULL, 170, 270)",
            [],
        )
        .unwrap();
    assert_constraint(
        &connection,
        "INSERT INTO notice_attempts VALUES(11, -101, 8, 'unconfirmed', NULL, 180, 280)",
    );
}

#[test]
fn unsupported_or_foreign_schema_is_rejected_without_mutation() {
    for sql in [
        "PRAGMA user_version = 99",
        "CREATE TABLE foreign_data(value TEXT); INSERT INTO foreign_data VALUES('preserve')",
        "CREATE TABLE sqliteX_foreign(value TEXT)",
        "PRAGMA user_version = 1; PRAGMA application_id = 42; CREATE TABLE foreign_data(value TEXT)",
    ] {
        let fixture = Fixture::new();
        let settings = fixture.settings();
        // Precreate with private permissions, as an operator-owned existing file.
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&settings.path)
            .unwrap();
        let connection = fixture.connect();
        connection.execute_batch(sql).unwrap();
        drop(connection);
        let before = fs::read(&settings.path).unwrap();
        assert!(matches!(
            Database::open(&settings, &Limits::default()),
            Err(AppError::UnsupportedSchema(_))
        ));
        assert_eq!(fs::read(&settings.path).unwrap(), before);
    }
}

#[test]
fn unprotected_files_links_special_files_and_sidecars_fail_closed() {
    let fixture = Fixture::new();
    let settings = fixture.settings();
    drop(Database::open(&settings, &Limits::default()).unwrap());
    let original = fs::read(&settings.path).unwrap();
    for mode in [0o400, 0o000, 0o640, 0o606] {
        fs::set_permissions(&settings.path, Permissions::from_mode(mode)).unwrap();
        assert!(matches!(
            Database::open(&settings, &Limits::default()),
            Err(AppError::StorageUnavailable)
        ));
    }
    fs::set_permissions(&settings.path, Permissions::from_mode(0o600)).unwrap();
    let target = fixture.0.join("target");
    fs::rename(&settings.path, &target).unwrap();
    symlink(&target, &settings.path).unwrap();
    assert!(Database::open(&settings, &Limits::default()).is_err());
    fs::remove_file(&settings.path).unwrap();
    fs::hard_link(&target, &settings.path).unwrap();
    assert!(Database::open(&settings, &Limits::default()).is_err());
    fs::remove_file(&settings.path).unwrap();
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &settings.path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    assert!(Database::open(&settings, &Limits::default()).is_err());
    fs::remove_file(&settings.path).unwrap();
    fs::rename(&target, &settings.path).unwrap();
    let journal = fixture.0.join("relay.sqlite-journal");
    symlink(&settings.path, &journal).unwrap();
    assert!(Database::open(&settings, &Limits::default()).is_err());
    fs::remove_file(journal).unwrap();
    fs::set_permissions(&fixture.0, Permissions::from_mode(0o755)).unwrap();
    assert!(Database::open(&settings, &Limits::default()).is_err());
    assert_eq!(fs::read(&settings.path).unwrap(), original);
}

#[test]
fn excessive_existing_size_is_rejected_without_eviction_and_errors_are_safe() {
    let fixture = Fixture::new();
    let mut settings = fixture.settings();
    settings.max_database_bytes = 1_048_576;
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&settings.path)
        .unwrap();
    file.set_len(1_048_577).unwrap();
    assert!(matches!(
        Database::open(&settings, &Limits::default()),
        Err(AppError::StorageCapacity)
    ));
    assert_eq!(fs::metadata(&settings.path).unwrap().len(), 1_048_577);
    let private = "synthetic-private-path-and-body";
    let error = AppError::from(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some(private.into()),
    ));
    assert_eq!(error.code(), "STORAGE_FAILED");
    assert!(!format!("{error:?} {error}").contains(private));
}
