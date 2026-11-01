use std::{fs, os::unix::fs::DirBuilderExt};

use rusqlite::Connection;
use serde_json::json;
use telegram_mcp_relay::{
    config::Config,
    error::AppError,
    repositories::{
        AttachmentInput, AttachmentKind, CleanupSweep, Database, IngestOutcome, JournalInput,
        MessageDirection, MessageSource, RouteKind,
    },
    services::{inbox::InboxService, ingress::IngressService},
};

const DAY: i64 = 86_400_000;
#[path = "history/read.rs"]
mod reads;
const SOURCE: MessageSource = MessageSource {
    chat_id: -101,
    message_id: 7,
    sender_user_id: Some(9),
};

struct Fixture(Config);
impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("history-test-{:016x}", u64::from_ne_bytes(random)));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let mut config = Config::default();
        config.storage.path = directory.join("relay.sqlite");
        config.history.enabled = true;
        config.history.retention_days = 1;
        config.telegram.allowed_chat_ids = vec![-101];
        config.telegram.allowed_user_ids = vec![9];
        Self(config)
    }
    fn open(&self) -> Database {
        Database::open_with_history(&self.0.storage, &self.0.limits, &self.0.history, true).unwrap()
    }
    fn sql(&self) -> Connection {
        Connection::open(&self.0.storage.path).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.storage.path.parent().unwrap()).unwrap();
    }
}

fn input() -> JournalInput<'static> {
    JournalInput {
        source: SOURCE,
        direction: MessageDirection::Incoming,
        body: Some("Private synthetic body"),
        chat_label: Some("Synthetic chat"),
        sender_label: Some("Synthetic sender"),
        session: None,
        message_date: 1,
        reply_to_message_id: None,
    }
}

fn clean(db: &mut Database, now: i64) {
    let mut sweep = CleanupSweep::new(now).unwrap();
    for _ in 0..4 {
        if db.cleanup_batch(&mut sweep).unwrap().complete {
            return;
        }
    }
    panic!("bounded fixture cleanup did not finish");
}

#[test]
fn opt_in_deduplication_and_display_snapshots_survive_restart_without_secret_disclosure() {
    let fixture = Fixture::new();
    assert!(matches!(
        Database::open_with_history(
            &fixture.0.storage,
            &fixture.0.limits,
            &fixture.0.history,
            false
        ),
        Err(AppError::InvalidConfig(_))
    ));
    assert!(!fixture.0.storage.path.exists());
    let mut db = fixture.open();
    let session = db
        .register_session(
            Some("private-external"),
            None,
            "Synthetic",
            Some("module"),
            0,
        )
        .unwrap();
    let mut message = input();
    message.session = Some(&session);
    let saved = db.record_message(&message, 10).unwrap();
    let duplicate = db.record_message(&message, 20).unwrap();
    assert_eq!(saved.message_handle, duplicate.message_handle);
    assert_eq!(saved.expires_at, duplicate.expires_at);
    let display: String = fixture
        .sql()
        .query_row("SELECT session_display_id FROM journal_records", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(display.len(), 64);
    assert_ne!(display, session.session_key);
    assert!(!format!("{message:?} {saved:?}").contains("Private synthetic body"));
    assert!(!format!("{message:?}").contains(&session.session_key));
    let before = db.history_usage().unwrap();
    assert_eq!(before.records, 1);
    assert_eq!(db.history_coverage().unwrap().observed_from, Some(10));
    drop(db);
    let mut db = fixture.open();
    assert!(db.history_coverage().unwrap().has_gaps);
    assert_eq!(db.history_usage().unwrap(), before);
    fixture
        .sql()
        .execute("UPDATE sessions SET title = 'Renamed'", [])
        .unwrap();
    assert_eq!(
        db.record_message(&message, 30).unwrap().message_handle,
        saved.message_handle
    );
    let title: String = fixture
        .sql()
        .query_row("SELECT session_title FROM journal_records", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(title, "Synthetic");
    // Disable capture; retained data is neither erased nor extended and still expires.
    drop(db);
    let mut db = Database::open(&fixture.0.storage, &fixture.0.limits).unwrap();
    assert!(matches!(
        db.record_message(&message, 40),
        Err(AppError::HistoryDisabled)
    ));
    clean(&mut db, saved.expires_at);
    assert_eq!(db.history_usage().unwrap().records, 0);
    assert!(db.session(&session.session_key).is_ok());
}

#[test]
fn capacity_failure_preserves_inbox_admission_for_an_observation_not_yet_ingested() {
    let mut fixture = Fixture::new();
    fixture.0.history.max_records = 1;
    let mut db = fixture.open();
    db.record_message(&input(), 10).unwrap();
    let before = db.history_usage().unwrap();
    let coverage = db.history_coverage().unwrap();
    let update_id: Option<i64> = fixture.sql().query_row(
        "SELECT telegram_update_id FROM journal_records WHERE chat_id = -101 AND message_id = 7",
        [], |r| r.get(0)).unwrap();
    assert_eq!(update_id, None);
    let session = db.register_session(None, None, "Another", None, 0).unwrap();
    let mut message = input();
    message.source.message_id = 8;
    message.session = Some(&session);
    assert!(matches!(
        db.record_message(&message, 20),
        Err(AppError::StorageCapacity)
    ));
    assert_eq!(db.history_usage().unwrap(), before);
    assert_eq!(db.history_coverage().unwrap(), coverage);
    let maps: i64 = fixture
        .sql()
        .query_row("SELECT count(*) FROM session_displays", [], |r| r.get(0))
        .unwrap();
    assert_eq!(maps, 0);
    assert_eq!(db.update_cursor().unwrap(), None);
    let reserve = db.reserve_route(&session.session_key).unwrap();
    db.persist_route(reserve, -101, 5, RouteKind::Text, 0, 0)
        .unwrap();
    let update = serde_json::from_value(json!({"update_id":11,"message":{
        "message_id":7,"date":1,"chat":{"id":-101},"from":{"id":9,"is_bot":false},
        "reply_to_message":{"message_id":5},"text":"Still admitted"}}))
    .unwrap();
    assert!(matches!(
        IngressService::new(&fixture.0)
            .unwrap()
            .ingest(&mut db, &update, 30)
            .unwrap(),
        IngestOutcome::ReplyStored { .. }
    ));
    assert_eq!(
        db.inbox_snapshot(&session.session_key, None, 1, 30)
            .unwrap()
            .replies
            .len(),
        1
    );
    // Retained content alone was not an ingress disposition. The first routed Reply
    // reuses that bounded record, then its committed update ID fences redelivery.
    assert_eq!(db.history_usage().unwrap(), before);
    let update_id: Option<i64> = fixture.sql().query_row(
        "SELECT telegram_update_id FROM journal_records WHERE chat_id = -101 AND message_id = 7",
        [], |r| r.get(0)).unwrap();
    assert_eq!(update_id, Some(11));
    assert_eq!(
        IngressService::new(&fixture.0)
            .unwrap()
            .ingest(&mut db, &update, 31)
            .unwrap(),
        IngestOutcome::Duplicate
    );
    assert_eq!(db.update_cursor().unwrap(), Some(11));
    assert_eq!(
        db.inbox_snapshot(&session.session_key, None, 2, 31)
            .unwrap()
            .replies
            .len(),
        1
    );
}

#[test]
fn own_reply_provenance_survives_history_expiry_and_disablement_then_retires_after_ack() {
    let mut fixture = Fixture::new();
    fixture.0.history.max_attachments = 1;
    let mut db = fixture.open();
    let session = db
        .register_session(None, None, "Synthetic", None, 0)
        .unwrap();
    let reserve = db.reserve_route(&session.session_key).unwrap();
    db.persist_route(reserve, -101, 5, RouteKind::Text, 0, 0)
        .unwrap();
    let update = serde_json::from_value(json!({"update_id":11,"message":{
        "message_id":7,"date":1,"chat":{"id":-101},"from":{"id":9,"is_bot":false},
        "reply_to_message":{"message_id":5},"document":{"file_id":"private-file","file_unique_id":"unique"}}})).unwrap();
    let IngestOutcome::ReplyStored { reply_id, .. } = IngressService::new(&fixture.0)
        .unwrap()
        .ingest(&mut db, &update, 10)
        .unwrap()
    else {
        panic!("missing reply")
    };
    let journal = db.record_message(&input(), 10).unwrap();
    let mut attachment = AttachmentInput {
        source: SOURCE,
        reply_id: Some(&reply_id),
        message_handle: Some(&journal.message_handle),
        file_id: "private-file",
        file_unique_id: "unique",
        filename: Some("untrusted.txt"),
        mime_type: None,
        file_size: None,
        kind: AttachmentKind::Document,
        width: None,
        height: None,
    };
    let handle = db.record_attachment(&attachment, 10).unwrap();
    assert_eq!(db.record_attachment(&attachment, 20).unwrap(), handle);
    attachment.file_unique_id = "second";
    assert!(matches!(
        db.record_attachment(&attachment, 20),
        Err(AppError::StorageCapacity)
    ));
    assert_eq!(db.history_usage().unwrap().attachments, 1);
    attachment.file_unique_id = "unique";
    assert!(!format!("{attachment:?}").contains("private-file"));
    attachment.source.message_id = 99;
    assert!(db.record_attachment(&attachment, 20).is_err());
    attachment.source = SOURCE;
    drop(db);
    let mut db = Database::open(&fixture.0.storage, &fixture.0.limits).unwrap();
    clean(&mut db, DAY + 10);
    assert_eq!(db.history_usage().unwrap().records, 0);
    assert_eq!(db.history_usage().unwrap().attachments, 1);
    attachment.message_handle = None;
    assert_eq!(db.record_attachment(&attachment, DAY + 11).unwrap(), handle);
    attachment.file_unique_id = "second";
    db.record_attachment(&attachment, DAY + 11).unwrap();
    assert_eq!(db.history_usage().unwrap().attachments, 2);
    let inbox = InboxService::new(&fixture.0.delivery).unwrap();
    let claim = inbox
        .claim(&mut db, &session.session_key, None, 1, DAY + 11)
        .unwrap()
        .pop()
        .unwrap();
    inbox
        .acknowledge(
            &mut db,
            &session.session_key,
            &reply_id,
            &claim.claim_token,
            DAY + 11,
        )
        .unwrap();
    clean(&mut db, DAY + 11);
    assert_eq!(db.history_usage().unwrap().attachments, 0);
}

#[test]
fn failed_v7_migration_preserves_old_schema_and_records_transactionally() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    let session = db
        .register_session(None, None, "Preserve", None, 0)
        .unwrap();
    drop(db);
    let sql = fixture.sql();
    sql.execute_batch(
        "DROP INDEX receipts_message; ALTER TABLE acknowledgment_receipts DROP COLUMN telegram_message_id;
        ALTER TABLE acknowledgment_receipts DROP COLUMN telegram_chat_id;
        ALTER TABLE telegram_cursor DROP COLUMN telegram_message_id;
        ALTER TABLE telegram_cursor DROP COLUMN telegram_chat_id;
        DROP TABLE attachment_records; DROP TABLE journal_records; DROP TABLE session_displays;
        DROP TABLE history_usage; DROP TABLE history_coverage; DROP TABLE download_records; PRAGMA user_version = 7;
        CREATE TABLE history_coverage(conflict TEXT);",
    )
    .unwrap();
    assert!(Database::open(&fixture.0.storage, &fixture.0.limits).is_err());
    assert_eq!(
        sql.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        7
    );
    let exists: bool = sql
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'history_usage')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!exists);
    sql.execute_batch("DROP TABLE history_coverage").unwrap();
    let db = fixture.open();
    assert_eq!(db.schema_version().unwrap(), 10);
    assert_eq!(
        db.session(&session.session_key).unwrap().title.as_str(),
        "Preserve"
    );
}

#[test]
fn body_and_metadata_pressure_leave_exact_prior_usage_and_coverage() {
    for metadata in [false, true] {
        let mut fixture = Fixture::new();
        fixture.0.history.max_record_bytes = 1_024;
        fixture.0.history.max_body_bytes = 1_024;
        fixture.0.history.max_metadata_bytes = 1_024;
        let mut db = fixture.open();
        let body = "x".repeat(if metadata { 1 } else { 600 });
        let mut message = input();
        message.body = Some(&body);
        message.chat_label = None;
        message.sender_label = if metadata { Some("label") } else { None };
        db.record_message(&message, 10).unwrap();
        let before = db.history_usage().unwrap();
        message.source.message_id += 1;
        assert!(matches!(
            db.record_message(&message, 20),
            Err(AppError::StorageCapacity)
        ));
        assert_eq!(db.history_usage().unwrap(), before);
        assert_eq!(db.history_coverage().unwrap().observed_through, Some(10));
    }
}

#[test]
fn expiry_cleanup_is_batched_and_reclaims_the_journal_without_inbox_eviction() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    let mut message = input();
    for id in 1..=101 {
        message.source.message_id = id;
        db.record_message(&message, 10).unwrap();
    }
    let mut sweep = CleanupSweep::new(DAY + 10).unwrap();
    let batch = db.cleanup_batch(&mut sweep).unwrap();
    assert_eq!(batch.removed, 100);
    assert!(!batch.complete);
    assert_eq!(db.history_usage().unwrap().records, 1);
    db.cleanup_batch(&mut sweep).unwrap();
    assert_eq!(db.history_usage().unwrap().body_bytes, 0);
    assert_eq!(db.history_usage().unwrap().metadata_bytes, 0);
    let before = db.disk_usage().unwrap();
    let after = db.reclaim_batch().unwrap();
    assert!(after.database_bytes <= before.database_bytes);
    assert_eq!(
        (after.journal_bytes, after.wal_bytes, after.shm_bytes),
        (0, 0, 0)
    );
}

#[test]
fn shared_photo_and_confirmed_outgoing_document_metadata_require_retained_sources() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    let mut message = input();
    let photo = db.record_message(&message, 10).unwrap();
    let mut attachment = AttachmentInput {
        source: SOURCE,
        reply_id: None,
        message_handle: Some(&photo.message_handle),
        file_id: "private-photo",
        file_unique_id: "photo",
        filename: None,
        mime_type: None,
        file_size: Some(100),
        kind: AttachmentKind::Photo,
        width: Some(10),
        height: Some(20),
    };
    db.record_attachment(&attachment, 10).unwrap();
    attachment.height = None;
    assert!(db.record_attachment(&attachment, 10).is_err());
    message.source = MessageSource {
        message_id: 8,
        sender_user_id: None,
        ..SOURCE
    };
    message.direction = MessageDirection::Outgoing;
    let document = db.record_message(&message, 10).unwrap();
    attachment.source = message.source;
    attachment.message_handle = Some(&document.message_handle);
    attachment.kind = AttachmentKind::Document;
    attachment.width = None;
    db.record_attachment(&attachment, 10).unwrap();
    assert_eq!(db.history_usage().unwrap().attachments, 2);
}
