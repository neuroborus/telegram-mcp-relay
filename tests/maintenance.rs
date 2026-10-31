use std::{fs, os::unix::fs::DirBuilderExt};

use rusqlite::Connection;
use serde_json::json;
use telegram_mcp_relay::{
    config::{Config, Delivery, Limits, Storage},
    error::AppError,
    repositories::{CleanupSweep, Database, DiskUsage, IngestOutcome, ReplyState, RouteKind},
    services::{inbox::InboxService, ingress::IngressService},
};

const DAY: i64 = 86_400_000;

struct Fixture(Config);

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "maintenance-test-{:016x}",
            u64::from_ne_bytes(random)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let mut config = Config {
            storage: Storage {
                path: directory.join("relay.sqlite"),
                max_database_bytes: 1_048_579, // Rounded down to a whole-page main-file cap.
                reply_ttl_days: 1,
                ..Storage::default()
            },
            limits: Limits {
                reply_bytes: 524_288,
                ..Limits::default()
            },
            ..Config::default()
        };
        config.telegram.allowed_chat_ids = vec![-101];
        config.telegram.allowed_user_ids = vec![7];
        Self(config)
    }

    fn open(&self) -> Database {
        Database::open(&self.0.storage, &self.0.limits).unwrap()
    }

    fn seed(&self, db: &mut Database) -> String {
        let key = db
            .register_session(Some("synthetic"), None, "Same", None, 0)
            .unwrap()
            .session_key;
        let reservation = db.reserve_route(&key).unwrap();
        db.persist_route(reservation, -101, 5, RouteKind::Text, 0, 0)
            .unwrap();
        key
    }

    fn receive(
        &self,
        db: &mut Database,
        id: i64,
        now: i64,
        bytes: usize,
    ) -> Result<IngestOutcome, AppError> {
        let update = serde_json::from_value(json!({"update_id":id, "message":{
            "message_id":id + 100, "date":1, "chat":{"id":-101},
            "from":{"id":7,"is_bot":false}, "reply_to_message":{"message_id":5},
            "text":"x".repeat(bytes)}}))
        .unwrap();
        IngressService::new(&self.0)
            .unwrap()
            .ingest(db, &update, now)
    }

    fn clean(&self, db: &mut Database, now: i64) {
        let mut sweep = CleanupSweep::new(now).unwrap();
        for _ in 0..10 {
            if db.cleanup_batch(&mut sweep).unwrap().complete {
                return;
            }
        }
        panic!("fixture cleanup did not finish");
    }

    fn integrity(&self) {
        let sql = Connection::open(&self.0.storage.path).unwrap();
        assert_eq!(
            sql.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(
            !sql.prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.storage.path.parent().unwrap()).unwrap();
    }
}

fn reclaim(db: &mut Database, mut before: DiskUsage) -> DiskUsage {
    // The initial free-list count bounds the number of calls even if work cannot progress.
    for _ in 0..before.free_pages {
        if before.free_pages == 0 {
            return before;
        }
        let after = db.reclaim_batch().unwrap();
        assert!(after.free_pages < before.free_pages);
        assert!(before.free_pages - after.free_pages <= 32);
        assert!(after.database_bytes < before.database_bytes);
        assert!(after.database_bytes <= after.main_file_limit_bytes);
        assert_eq!((after.journal_bytes, after.temporary_disk_bytes), (0, 0));
        before = after;
    }
    assert_eq!(before.free_pages, 0);
    before
}

#[test]
fn incremental_reclamation_shrinks_files_and_preserves_processing_identity_after_restart() {
    let mut fixture = Fixture::new();
    // Claiming changes the large row's size: SQLite allocates its replacement overflow
    // chain before freeing the original. Reserve that transient space in this recovery case;
    // the separate page-exhaustion case retains the smaller cap.
    fixture.0.storage.max_database_bytes = 2_097_155;
    let mut db = fixture.open();
    let key = fixture.seed(&mut db);
    fixture.receive(&mut db, 11, 0, 524_288).unwrap();
    fixture.receive(&mut db, 12, DAY / 2, 16).unwrap();
    let inbox = InboxService::new(&Delivery::default()).unwrap();
    // The large first reply exhausts a claim batch's combined payload allowance.
    assert_eq!(
        inbox.claim(&mut db, &key, None, 2, DAY / 2).unwrap().len(),
        1
    );
    let small = inbox
        .claim(&mut db, &key, None, 1, DAY / 2)
        .unwrap()
        .pop()
        .unwrap();
    inbox
        .processing(
            &mut db,
            &key,
            &small.reply.reply_id,
            &small.claim_token,
            true,
            DAY / 2,
        )
        .unwrap();
    let before = db.disk_usage().unwrap();
    fixture.clean(&mut db, DAY);
    let deleted = db.disk_usage().unwrap();
    assert_eq!(deleted.database_bytes, before.database_bytes);
    assert!(deleted.free_pages > 32);
    let partial = db.reclaim_batch().unwrap();
    assert_eq!(deleted.free_pages - partial.free_pages, 32);
    assert!(partial.free_pages > 0);
    // Interrupt after a committed reclamation transaction. No volatile sweep state is needed.
    drop(db);
    let mut db = fixture.open();
    assert_eq!(db.disk_usage().unwrap(), partial);
    let after = reclaim(&mut db, partial);
    assert!(after.database_bytes < before.database_bytes);
    assert!(after.database_allocated_bytes < before.database_allocated_bytes);
    let page = db.inbox_snapshot(&key, None, 10, DAY).unwrap();
    assert_eq!(page.replies.len(), 1);
    assert_eq!(page.replies[0].reply_id, small.reply.reply_id);
    assert_eq!(page.replies[0].state, ReplyState::Processing);
    assert_eq!(db.update_cursor().unwrap(), Some(12));
    assert!(db.route(-101, 5, DAY).unwrap().is_some());
    let resumed = inbox
        .reconcile_processing(
            &mut db,
            &key,
            &small.reply.reply_id,
            &small.claim_token,
            "resume",
            DAY,
        )
        .unwrap();
    assert_eq!(resumed.status.processing_started_at, Some(DAY / 2));
    inbox
        .acknowledge(
            &mut db,
            &key,
            &small.reply.reply_id,
            &resumed.claim_token,
            DAY,
        )
        .unwrap();
    let usage = db_usage(&fixture, &db);
    reclaim(&mut db, usage);
    drop(db);
    let mut db = fixture.open();
    // The valid-token receipt remains idempotent after payload deletion and reclamation.
    inbox
        .acknowledge(
            &mut db,
            &key,
            &small.reply.reply_id,
            &resumed.claim_token,
            DAY + 1,
        )
        .unwrap();
    fixture.integrity();
}

fn db_usage(fixture: &Fixture, db: &Database) -> DiskUsage {
    let usage = db.disk_usage().unwrap();
    assert_eq!(
        usage.main_file_limit_bytes,
        u64::from(fixture.0.storage.max_database_bytes / usage.page_size)
            * u64::from(usage.page_size)
    );
    assert_eq!(
        fs::metadata(&fixture.0.storage.path).unwrap().len(),
        usage.database_bytes
    );
    usage
}

#[test]
fn page_exhaustion_rolls_back_and_cleanup_reclamation_restores_admission() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    let key = fixture.seed(&mut db);
    fixture.receive(&mut db, 11, 0, 524_288).unwrap();
    let before = db_usage(&fixture, &db);
    assert_eq!(before.main_file_limit_bytes, 1_048_576);
    assert_eq!(
        fixture.receive(&mut db, 12, DAY / 2, 524_288),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(db.update_cursor().unwrap(), Some(11));
    assert_eq!(db.session(&key).unwrap().last_activity_at, 0);
    assert_eq!(db.disk_usage().unwrap(), before);
    // Reclamation never evicts a live payload to make room for a failed admission.
    db.reclaim_batch().unwrap();
    let page = db.inbox_snapshot(&key, None, 1, DAY / 2).unwrap();
    assert_eq!(
        page.replies[0].text_or_caption.as_ref().unwrap().len(),
        524_288
    );
    drop(db);
    let mut db = fixture.open();
    fixture.clean(&mut db, DAY);
    let usage = db.disk_usage().unwrap();
    assert!(usage.free_pages > 0);
    reclaim(&mut db, usage);
    fixture.receive(&mut db, 12, DAY, 524_288).unwrap();
    assert_eq!(db.update_cursor().unwrap(), Some(12));
    assert_eq!(
        db.inbox_snapshot(&key, None, 1, DAY).unwrap().replies.len(),
        1
    );
    fixture.integrity();
}
