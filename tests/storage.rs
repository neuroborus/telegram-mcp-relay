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
            "INSERT INTO sessions(session_key, external_session_id, title, created_at, last_activity_at, expires_at) VALUES(?1, ?2, 'Same title', 100, 100, 200)",
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
    assert_eq!(database.schema_version().unwrap(), 10);
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
    for version in [1, 2, 3, 4, 5, 6, 7, 8] {
        let connection = fixture.connect();
        connection
            .execute_batch("DROP TABLE download_records")
            .unwrap();
        connection
            .execute_batch(
                "DROP INDEX receipts_message;
            ALTER TABLE acknowledgment_receipts DROP COLUMN telegram_message_id;
            ALTER TABLE acknowledgment_receipts DROP COLUMN telegram_chat_id;
            ALTER TABLE telegram_cursor DROP COLUMN telegram_message_id;
            ALTER TABLE telegram_cursor DROP COLUMN telegram_chat_id;",
            )
            .unwrap();
        if version < 8 {
            connection.execute_batch("DROP TABLE attachment_records; DROP TABLE journal_records;
                DROP TABLE session_displays; DROP TABLE history_usage; DROP TABLE history_coverage;").unwrap();
        } else {
            connection
                .execute_batch(
                    "ALTER TABLE journal_records DROP COLUMN telegram_update_id;
                    ALTER TABLE journal_records DROP COLUMN reply_to_message_id",
                )
                .unwrap();
        }
        if version < 7 {
            connection
                .execute_batch("ALTER TABLE sessions DROP COLUMN directory_label")
                .unwrap();
        }
        if version < 6 {
            connection
                .execute_batch(
                    "DROP INDEX sessions_expiry;
                 CREATE INDEX sessions_expiry ON sessions(expires_at);",
                )
                .unwrap();
        }
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
            // Restore the older ID constraint before exercising its ignored conflict;
            // v9 intentionally permits distinct sources with the same update ID.
            connection
                .execute_batch(
                    "CREATE UNIQUE INDEX synthetic_legacy_updates
                ON update_dispositions(telegram_update_id)",
                )
                .unwrap();
            connection.execute(
                "INSERT OR IGNORE INTO update_dispositions VALUES(12, 'unroutable', -101, 9, 140, 240)", [],
            ).unwrap();
        }
        connection
            .pragma_update(None, "user_version", version)
            .unwrap();
        let reopened = Database::open(&settings, &Limits::default()).unwrap();
        assert_eq!(reopened.schema_version().unwrap(), 10);
        assert_eq!(reopened.session("a").unwrap().directory_label, None);
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
    // Later sequences may reuse an update ID, but never a composite source message.
    add_reply(&connection, "r3", 11, 10).unwrap();
    assert!(add_reply(&connection, "r4", 13, 8).is_err());
    assert_constraint(&connection, "UPDATE replies SET session_key = 'missing'");
    assert_constraint(
        &connection,
        "UPDATE replies SET attachment_metadata_json = 'invalid'",
    );
    assert_constraint(&connection, "UPDATE replies SET expires_at = 0");
    assert_constraint(
        &connection,
        "INSERT INTO telegram_cursor(singleton_id, last_durably_processed_update_id) VALUES(2, 12)",
    );
    // Removing a route is independent of an already accepted reply's retention.
    connection
        .execute("DELETE FROM message_routes WHERE session_key = 'a'", [])
        .unwrap();
    assert_constraint(&connection, "DELETE FROM sessions WHERE session_key = 'a'");
    let replies: i64 = connection
        .query_row("SELECT count(*) FROM replies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(replies, 3);
    connection
        .execute(
            "INSERT INTO acknowledgment_receipts(reply_id, session_key, claim_token, telegram_update_id, created_at, expires_at) VALUES('r1', 'a', 'token-a', 11, 120, 220)",
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

// Construct the actual accepted schema, including its former global update-ID keys.
fn v8_fixture() -> Fixture {
    let fixture = Fixture::new();
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(fixture.settings().path)
        .unwrap();
    let sql = fixture.connect();
    sql.execute_batch("PRAGMA auto_vacuum = INCREMENTAL; BEGIN IMMEDIATE;")
        .unwrap();
    for ddl in [
        include_str!("../src/repositories/schema.sql"),
        include_str!("../src/repositories/ingress.sql"),
        include_str!("../src/repositories/inbox.sql"),
        include_str!("../src/repositories/processing.sql"),
        include_str!("../src/repositories/notices.sql"),
        include_str!("../src/repositories/retention.sql"),
        include_str!("../src/repositories/presentation.sql"),
        include_str!("../src/repositories/history.sql"),
    ] {
        sql.execute_batch(ddl).unwrap();
    }
    sql.execute_batch("PRAGMA user_version = 8; PRAGMA application_id = 0x544d4352; COMMIT;")
        .unwrap();
    add_session(&sql, "a", None);
    add_reply(&sql, "processing", 11, 8).unwrap();
    sql.execute_batch(
        "UPDATE replies SET state = 'processing', claimed_at = 120, claim_token = 'current',
        processing_started_at = 130, reconciliation_request_id = 'saved-request',
        reconciliation_from_token = 'prior';
        INSERT INTO acknowledgment_receipts VALUES('completed', 'a', 'synthetic-token', 12, 110, 100000);
        INSERT INTO notice_attempts VALUES(11, -101, 8, 'unconfirmed', 99, 150, 250);
        INSERT INTO notice_attempts VALUES(14, -101, 8, 'unconfirmed', 100, 150, 250);
        INSERT INTO update_dispositions VALUES(13, 'unroutable', -101, 10, 140, 240, 1);
        INSERT INTO attachment_records(attachment_handle, chat_id, message_id, sender_user_id,
            reply_id, file_id, file_unique_id, expires_at, metadata_bytes, kind)
            VALUES(printf('%064d', 1), -101, 8, 7, 'processing', 'private-file', 'unique', 210, 600, 'document');
        UPDATE telegram_cursor SET last_durably_processed_update_id = 13;"
    ).unwrap();
    fixture
}

#[test]
fn v8_migration_preserves_completion_reconciliation_and_attachment_grants() {
    use telegram_mcp_relay::{
        config::{Config, Delivery},
        repositories::{IngestOutcome, RouteKind},
        services::{inbox::InboxService, ingress::IngressService},
    };
    let fixture = v8_fixture();
    let mut database = Database::open(&fixture.settings(), &Limits::default()).unwrap();
    let inbox = InboxService::new(&Delivery::default()).unwrap();
    inbox
        .acknowledge(&mut database, "a", "completed", "synthetic-token", 150)
        .unwrap();
    let recovered = inbox
        .reconcile_processing(
            &mut database,
            "a",
            "processing",
            "prior",
            "saved-request",
            150,
        )
        .unwrap();
    assert_eq!(recovered.claim_token, "current");
    assert_eq!(recovered.status.processing_started_at, Some(130));
    assert_eq!(
        inbox.acknowledge(&mut database, "a", "processing", "prior", 150),
        Err(AppError::InvalidClaim)
    );
    let sql = fixture.connect();
    let source: (Option<i64>, Option<i64>) = sql.query_row(
        "SELECT telegram_chat_id, telegram_message_id FROM acknowledgment_receipts WHERE reply_id = 'completed'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(source, (None, None)); // No invented legacy provenance.
    let attachment: String = sql
        .query_row("SELECT reply_id FROM attachment_records", [], |r| r.get(0))
        .unwrap();
    assert_eq!(attachment, "processing");
    assert_eq!(database.update_cursor().unwrap(), Some(13));
    assert_eq!(database.schema_version().unwrap(), 10);
    assert!(
        !sql.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    // Rebuilding parent tables must not consume an uncertain notice or change its identity.
    let notice: (i64, i64) = sql
        .query_row(
            "SELECT trigger_message_id, notice_message_id FROM notice_attempts WHERE telegram_update_id = 11",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(notice, (8, 99));
    assert_eq!(sql.query_row::<i64, _, _>("SELECT count(*) FROM notice_attempts WHERE trigger_message_id = 8 AND notice_message_id IN (99, 100)", [], |r| r.get(0)).unwrap(), 2);
    // Unknown legacy provenance cannot veto another authorized source using the same ID.
    let reservation = database.reserve_route("a").unwrap();
    database
        .persist_route(reservation, -101, 7, RouteKind::Text, 100, 100)
        .unwrap();
    let mut config = Config::default();
    config.telegram.allowed_chat_ids = vec![-101];
    config.telegram.allowed_user_ids = vec![7];
    let update = serde_json::from_value(serde_json::json!({"update_id":12, "message":{
        "message_id":20, "date":1, "chat":{"id":-101}, "from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":7}, "text":"New source"}}))
    .unwrap();
    assert!(matches!(
        IngressService::new(&config)
            .unwrap()
            .ingest(&mut database, &update, 150)
            .unwrap(),
        IngestOutcome::ReplyStored { .. }
    ));
    inbox
        .acknowledge(&mut database, "a", "completed", "synthetic-token", 150)
        .unwrap();
    inbox
        .acknowledge(&mut database, "a", "processing", "current", 150)
        .unwrap();
    assert_eq!(
        sql.query_row::<Option<String>, _, _>("SELECT reply_id FROM attachment_records", [], |r| r
            .get(0))
            .unwrap(),
        None
    );
}

#[test]
fn failed_source_key_migration_rolls_back_rebuilds_and_private_grants() {
    let fixture = v8_fixture();
    let sql = fixture.connect();
    // Fail at the final DDL, after the parent and receipt rebuilds would have run.
    sql.execute_batch("ALTER TABLE telegram_cursor ADD COLUMN telegram_chat_id INTEGER")
        .unwrap();
    assert!(Database::open(&fixture.settings(), &Limits::default()).is_err());
    assert_eq!(
        sql.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        8
    );
    let saved: (String, String, String) = sql
        .query_row(
            "SELECT claim_token, reconciliation_request_id, reconciliation_from_token FROM replies",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        saved,
        ("current".into(), "saved-request".into(), "prior".into())
    );
    assert_eq!(
        sql.query_row::<String, _, _>("SELECT reply_id FROM attachment_records", [], |r| r.get(0))
            .unwrap(),
        "processing"
    );
    assert_eq!(
        sql.query_row::<String, _, _>("SELECT claim_token FROM acknowledgment_receipts", [], |r| r
            .get(0))
            .unwrap(),
        "synthetic-token"
    );
    assert_eq!(
        sql.query_row::<i64, _, _>(
            "SELECT notice_message_id FROM notice_attempts WHERE telegram_update_id = 11",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        99
    );
    assert!(
        !sql.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    // The original global uniqueness constraint is restored too.
    assert!(add_reply(&sql, "collision", 11, 20).is_err());
}

#[test]
fn v9_download_migration_preserves_recovery_and_rolls_back_late_ddl_failure() {
    for conflict in [false, true] {
        let fixture = v8_fixture();
        let database = Database::open(&fixture.settings(), &Limits::default()).unwrap();
        drop(database);
        let sql = fixture.connect();
        sql.execute_batch("DROP TABLE download_records; PRAGMA user_version = 9;")
            .unwrap();
        if conflict {
            sql.execute_batch("CREATE INDEX download_expiry ON sessions(expires_at)")
                .unwrap();
            assert!(Database::open(&fixture.settings(), &Limits::default()).is_err());
            assert_eq!(
                sql.pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
                    .unwrap(),
                9
            );
            let table: bool = sql.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'download_records')", [], |row| row.get(0)).unwrap();
            assert!(!table);
            sql.execute_batch("DROP INDEX download_expiry").unwrap();
        }
        let database = Database::open(&fixture.settings(), &Limits::default()).unwrap();
        assert_eq!(database.schema_version().unwrap(), 10);
        let saved: (String, String, String) = sql.query_row(
            "SELECT claim_token, reconciliation_request_id, reconciliation_from_token FROM replies", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert_eq!(
            saved,
            ("current".into(), "saved-request".into(), "prior".into())
        );
        let count: i64 = sql
            .query_row("SELECT count(*) FROM download_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(database.update_cursor().unwrap(), Some(13));
    }
}
