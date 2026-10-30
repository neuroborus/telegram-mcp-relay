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
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("storage-test-{:016x}", u64::from_ne_bytes(random)));
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
    assert_eq!(database.schema_version().unwrap(), 6);
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
            "UPDATE replies SET state = 'processing', claimed_at = 120,
        claim_token = 'token-a', processing_started_at = 130",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE telegram_cursor SET last_durably_processed_update_id = 11",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO notice_attempts VALUES(12, -101, 9, 'unroutable', NULL, 150, 250)",
            [],
        )
        .unwrap();
    drop(connection);
    drop(database);
    // Recreate each accepted older shape, preserving its records through migration.
    for version in [1, 2, 3, 4, 5] {
        let connection = fixture.connect();
        connection
            .execute_batch(
                "DROP INDEX sessions_expiry;
                 CREATE INDEX sessions_expiry ON sessions(expires_at);",
            )
            .unwrap();
        if version < 5 {
            connection
                .execute_batch(
                    "DROP INDEX dispositions_notice;
                DROP INDEX dispositions_message;
                DROP INDEX notices_trigger;
                ALTER TABLE update_dispositions DROP COLUMN notice_attempted;
                DROP INDEX replies_notice;
                CREATE INDEX replies_notice ON replies(created_at)
                    WHERE processing_started_at IS NULL AND notice_attempted = 0;",
                )
                .unwrap();
        }
        if version < 4 {
            connection
                .execute_batch(
                    "ALTER TABLE replies DROP COLUMN reconciliation_from_token;
                    ALTER TABLE replies DROP COLUMN reconciliation_request_id;",
                )
                .unwrap();
        }
        if version == 1 {
            connection
                .execute_batch("DROP TABLE update_dispositions")
                .unwrap();
        }
        if version < 3 {
            connection
                .execute_batch("DROP INDEX replies_ordered")
                .unwrap();
        }
        if (2..5).contains(&version) {
            connection.execute(
                "INSERT OR IGNORE INTO update_dispositions VALUES(12, 'unroutable', -101, 9, 140, 240)", [],
            ).unwrap();
        }
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        let reopened = Database::open(&settings, &Limits::default()).unwrap();
        assert_eq!(reopened.schema_version().unwrap(), 6);
        assert_eq!(
            reopened.inbox_snapshot("a", None, 1, 150).unwrap().replies[0].processing_started_at,
            Some(130)
        );
        let saved: (String, i64) = connection.query_row(
        "SELECT reply_id, last_durably_processed_update_id FROM replies CROSS JOIN telegram_cursor",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
        assert_eq!(saved, ("reply-a".into(), 11));
        let notice: (Option<i64>, i64, i64) = connection
            .query_row(
                "SELECT notice_message_id, created_at, expires_at FROM notice_attempts",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(notice, (None, 150, 250));
        if version > 1 {
            let disposition: (i64, i64, i64) = connection
                .query_row(
                    "SELECT notice_attempted, created_at, expires_at FROM update_dispositions",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(disposition, (1, 140, 240));
        }
    }
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
