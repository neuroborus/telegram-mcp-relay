use std::{fs, os::unix::fs::DirBuilderExt};

use rusqlite::Connection;
use serde_json::json;
use telegram_mcp_relay::{
    config::{Config, Delivery, Notifications, Storage},
    error::AppError,
    repositories::{CleanupSweep, Database, IngestOutcome, NoticePolicy, ReplyState, RouteKind},
    services::{inbox::InboxService, ingress::IngressService},
};

const DAY: i64 = 86_400_000;

struct Fixture(Config);

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "retention-test-{:016x}",
            u64::from_ne_bytes(random)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let mut config = Config {
            storage: Storage {
                path: directory.join("relay.sqlite"),
                session_ttl_days: 1,
                route_ttl_days: 2,
                reply_ttl_days: 3,
                ..Storage::default()
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

    fn sql(&self) -> Connection {
        Connection::open(&self.0.storage.path).unwrap()
    }

    fn session(&self, db: &mut Database, message: i64) -> String {
        let key = db
            .register_session(None, None, "Same", None, 0)
            .unwrap()
            .session_key;
        let reservation = db.reserve_route(&key).unwrap();
        db.persist_route(reservation, -101, message, RouteKind::Text, 0, 0)
            .unwrap();
        key
    }

    fn receive(
        &self,
        db: &mut Database,
        id: i64,
        parent: i64,
        now: i64,
    ) -> Result<IngestOutcome, AppError> {
        let update = serde_json::from_value(json!({"update_id":id, "message":{
            "message_id":id + 100, "date":1, "chat":{"id":-101},
            "from":{"id":7,"is_bot":false}, "reply_to_message":{"message_id":parent},
            "text":"synthetic", "document":{"file_id":"file", "file_unique_id":"unique"}}}))
        .unwrap();
        IngressService::new(&self.0)
            .unwrap()
            .ingest(db, &update, now)
    }

    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn assert_integrity(&self) {
        assert!(
            !self
                .sql()
                .prepare("PRAGMA foreign_key_check")
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

fn clean(db: &mut Database, now: i64) -> usize {
    let mut sweep = CleanupSweep::new(now).unwrap();
    let mut removed = 0;
    for _ in 0..100 {
        let batch = db.cleanup_batch(&mut sweep).unwrap();
        removed += batch.removed;
        if batch.complete {
            assert_eq!(db.cleanup_batch(&mut sweep).unwrap().removed, 0);
            return removed;
        }
    }
    panic!("bounded fixture sweep did not finish");
}

#[test]
fn independent_expiry_preserves_processing_recovery_after_route_and_session_ttl() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    let key = fixture.session(&mut db, 5);
    fixture.receive(&mut db, 11, 5, DAY).unwrap();
    let inbox = InboxService::new(&Delivery::default()).unwrap();
    let claim = inbox
        .claim(&mut db, &key, None, 1, DAY)
        .unwrap()
        .pop()
        .unwrap();
    inbox
        .processing(
            &mut db,
            &key,
            &claim.reply.reply_id,
            &claim.claim_token,
            true,
            DAY,
        )
        .unwrap();
    // Both parent TTLs end at day 2. The reply's independent receipt TTL ends at day 4.
    assert!(db.route(-101, 5, 2 * DAY - 1).unwrap().is_some());
    assert_eq!(db.route(-101, 5, 2 * DAY).unwrap(), None);
    assert_eq!(clean(&mut db, 2 * DAY), 1);
    assert_eq!(fixture.count("message_routes"), 0);
    assert_eq!(fixture.count("sessions"), 1);
    drop(db);
    let mut db = fixture.open();
    let resumed = inbox
        .reconcile_processing(
            &mut db,
            &key,
            &claim.reply.reply_id,
            &claim.claim_token,
            "resume",
            3 * DAY,
        )
        .unwrap();
    assert_eq!(resumed.status.processing_started_at, Some(DAY));
    let reply = db
        .inbox_snapshot(&key, None, 1, 4 * DAY - 1)
        .unwrap()
        .replies
        .pop()
        .unwrap();
    assert_eq!(
        (reply.state, reply.expires_at),
        (ReplyState::Processing, 4 * DAY)
    );
    assert_eq!(clean(&mut db, 4 * DAY - 1), 0);
    assert!(
        db.inbox_snapshot(&key, None, 1, 4 * DAY)
            .unwrap()
            .replies
            .is_empty()
    );
    assert_eq!(
        inbox.processing(
            &mut db,
            &key,
            &reply.reply_id,
            &resumed.claim_token,
            true,
            4 * DAY
        ),
        Err(AppError::InvalidClaim)
    );
    assert_eq!(clean(&mut db, 4 * DAY), 2);
    assert_eq!(db.session(&key), Err(AppError::SessionNotFound));
    assert_eq!(db.update_cursor().unwrap(), Some(11));
    fixture.assert_integrity();
}

#[test]
fn valid_routes_and_receipts_keep_expired_session_identity_without_extending_child_ttl() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    let route_only = fixture.session(&mut db, 5);
    let completed = fixture.session(&mut db, 6);
    fixture.receive(&mut db, 11, 6, 1).unwrap();
    let inbox = InboxService::new(&Delivery::default()).unwrap();
    let claim = inbox
        .claim(&mut db, &completed, None, 1, DAY)
        .unwrap()
        .pop()
        .unwrap();
    let policy = NoticePolicy::new(&Delivery::default(), &Notifications::default()).unwrap();
    assert_eq!(db.take_notice_attempts(&policy, 1, DAY).unwrap().len(), 1);
    inbox
        .acknowledge(
            &mut db,
            &completed,
            &claim.reply.reply_id,
            &claim.claim_token,
            DAY,
        )
        .unwrap();
    assert_eq!(clean(&mut db, DAY + 1), 0);
    assert!(db.session(&route_only).is_ok());
    clean(&mut db, 2 * DAY);
    assert_eq!(db.session(&route_only), Err(AppError::SessionNotFound));
    assert_eq!(fixture.count("notice_attempts"), 1);
    // Completing the source reply neither removes its attempt nor extends its TTL.
    assert_eq!(clean(&mut db, 3 * DAY), 0);
    assert_eq!(clean(&mut db, 3 * DAY + 1), 1);
    drop(db);
    let mut db = fixture.open();
    inbox
        .acknowledge(
            &mut db,
            &completed,
            &claim.reply.reply_id,
            &claim.claim_token,
            4 * DAY - 1,
        )
        .unwrap();
    assert_eq!(clean(&mut db, 4 * DAY - 1), 0);
    assert_eq!(
        inbox.acknowledge(
            &mut db,
            &completed,
            &claim.reply.reply_id,
            &claim.claim_token,
            4 * DAY
        ),
        Err(AppError::InvalidClaim)
    );
    assert_eq!(clean(&mut db, 4 * DAY), 2);
    assert_eq!(fixture.count("acknowledgment_receipts"), 0);
    fixture.assert_integrity();
}

#[test]
fn admitted_sends_pin_only_their_session_until_success_or_release() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    let pinned = fixture.session(&mut db, 5);
    let released = fixture.session(&mut db, 6);
    let unrelated = db
        .register_session(None, None, "Other", None, 0)
        .unwrap()
        .session_key;
    let success = db.reserve_route(&pinned).unwrap();
    let failed = db.reserve_route(&released).unwrap();
    assert_eq!(clean(&mut db, 2 * DAY), 3); // Expired routes/orphan go; admitted identities stay.
    assert_eq!(fixture.count("sessions"), 2);
    assert_eq!(db.session(&unrelated), Err(AppError::SessionNotFound));
    drop(failed);
    db.persist_route(success, -101, 7, RouteKind::Document, 2 * DAY, 2 * DAY)
        .unwrap();
    clean(&mut db, 2 * DAY);
    assert_eq!(db.session(&released), Err(AppError::SessionNotFound));
    assert_eq!(db.session(&pinned).unwrap().expires_at, 3 * DAY);
    assert!(db.route(-101, 7, 3 * DAY).unwrap().is_some());
    fixture.assert_integrity();
}

#[test]
fn protected_sessions_do_not_starve_later_candidates_and_interrupted_sweeps_restart() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    for id in 1..=205 {
        fixture.session(&mut db, id);
    }
    let orphan = db
        .register_session(None, None, "Orphan", None, DAY)
        .unwrap()
        .session_key;
    let mut sweep = CleanupSweep::new(2 * DAY).unwrap();
    let batch = db.cleanup_batch(&mut sweep).unwrap();
    assert_eq!(batch.removed, 100);
    assert!(!batch.complete);
    assert_eq!(fixture.count("message_routes"), 105);
    // Restart after a committed partial batch. Move the surviving routes' independent
    // expiry forward; their 105 expired parents must not block scanning later sessions.
    fixture
        .sql()
        .execute("UPDATE message_routes SET expires_at = ?1", [4 * DAY])
        .unwrap();
    drop(db);
    let mut db = fixture.open();
    assert_eq!(clean(&mut db, 2 * DAY), 101); // 100 now-childless identities plus the orphan.
    assert_eq!(db.session(&orphan), Err(AppError::SessionNotFound));
    assert_eq!(fixture.count("sessions"), 105);
    assert_eq!(fixture.count("message_routes"), 105);
    fixture.assert_integrity();
}

#[test]
fn expiry_recovers_shared_capacity_without_early_eviction() {
    let mut fixture = Fixture::new();
    fixture.0.limits.inbox_items = 1;
    fixture.0.limits.receipts = 2;
    fixture.0.limits.sessions = 1;
    fixture.0.limits.routes = 1;
    let mut db = fixture.open();
    let key = fixture.session(&mut db, 5);
    fixture.receive(&mut db, 11, 5, 0).unwrap();
    assert_eq!(
        fixture.receive(&mut db, 12, 5, 1),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(db.update_cursor().unwrap(), Some(11));
    fixture.receive(&mut db, 12, 999, 0).unwrap(); // One authorized unroutable disposition.
    let policy = NoticePolicy::new(&Delivery::default(), &Notifications::default()).unwrap();
    assert_eq!(db.take_notice_attempts(&policy, 1, 0).unwrap().len(), 1);
    assert_eq!(
        db.take_notice_attempts(&policy, 1, 60_000),
        Err(AppError::StorageCapacity)
    );
    assert!(matches!(
        db.reserve_route(&key),
        Err(AppError::StorageCapacity)
    ));
    assert_eq!(clean(&mut db, 2 * DAY), 1); // Route only; reply/notice/disposition stay.
    assert_eq!(fixture.count("replies"), 1);
    assert_eq!(fixture.count("notice_attempts"), 1);
    assert_eq!(fixture.count("update_dispositions"), 1);
    assert_eq!(clean(&mut db, 3 * DAY - 1), 0);
    assert_eq!(clean(&mut db, 3 * DAY), 4); // Reply, attempt, disposition and session.
    assert_eq!(db.update_cursor().unwrap(), Some(12));
    let key = db
        .register_session(None, None, "Same", None, 3 * DAY)
        .unwrap()
        .session_key;
    fixture.receive(&mut db, 13, 6, 3 * DAY).unwrap(); // Unknown route; disposition admission recovers.
    // A fresh route and reply use the freed item, payload and metadata budgets.
    let reservation = db.reserve_route(&key).unwrap();
    db.persist_route(reservation, -101, 7, RouteKind::Text, 3 * DAY, 3 * DAY)
        .unwrap();
    fixture.receive(&mut db, 14, 7, 3 * DAY).unwrap();
    assert_eq!(
        db.inbox_snapshot(&key, None, 1, 3 * DAY)
            .unwrap()
            .replies
            .len(),
        1
    );
    fixture.assert_integrity();
}

#[test]
fn cleanup_failure_rolls_back_all_tables_and_cursor_progress() {
    let fixture = Fixture::new();
    let mut db = fixture.open();
    fixture.session(&mut db, 5);
    fixture.receive(&mut db, 11, 5, 0).unwrap();
    fixture
        .sql()
        .execute_batch(
            "CREATE TRIGGER fail_cleanup BEFORE DELETE ON sessions
        BEGIN SELECT RAISE(ABORT, 'synthetic private detail'); END",
        )
        .unwrap();
    let mut sweep = CleanupSweep::new(3 * DAY).unwrap();
    assert_eq!(db.cleanup_batch(&mut sweep), Err(AppError::StorageFailure));
    assert_eq!(fixture.count("replies"), 1);
    assert_eq!(fixture.count("message_routes"), 1);
    assert_eq!(db.update_cursor().unwrap(), Some(11));
    fixture
        .sql()
        .execute_batch("DROP TRIGGER fail_cleanup")
        .unwrap();
    assert_eq!(db.cleanup_batch(&mut sweep).unwrap().removed, 3);
    assert_eq!(fixture.count("sessions"), 0);
    fixture.assert_integrity();
    assert!(matches!(
        CleanupSweep::new(-1),
        Err(AppError::InvalidRequest(_))
    ));
}
