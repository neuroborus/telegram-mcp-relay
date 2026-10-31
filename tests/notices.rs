use std::{fs, os::unix::fs::DirBuilderExt, sync::Arc};

use rusqlite::Connection;
use serde_json::json;
use telegram_mcp_relay::{
    clients::telegram::Update,
    config::{Config, Delivery, Limits, Notifications, Storage},
    error::AppError,
    repositories::{Database, IngestOutcome, NoticeKind, NoticePolicy, RouteKind},
    services::{inbox::InboxService, ingress::IngressService},
};

const DAY: i64 = 86_400_000;

struct Fixture(Config);

impl Fixture {
    fn new(limits: Limits) -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("notices-test-{:016x}", u64::from_ne_bytes(random)));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let mut config = Config {
            limits,
            storage: Storage {
                path: directory.join("relay.sqlite"),
                reply_ttl_days: 1,
                ..Storage::default()
            },
            ..Config::default()
        };
        config.telegram.allowed_chat_ids = vec![-101, -102];
        config.telegram.allowed_user_ids = vec![7];
        Self(config)
    }

    fn open(&self) -> Database {
        Database::open(&self.0.storage, &self.0.limits).unwrap()
    }

    fn sql(&self) -> Connection {
        Connection::open(&self.0.storage.path).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn seed(&self, db: &mut Database) -> String {
        let key = db
            .register_session(None, None, "Same", None, 100)
            .unwrap()
            .session_key;
        let reservation = db.reserve_route(&key).unwrap();
        db.persist_route(reservation, -101, 5, RouteKind::Text, 100, 100)
            .unwrap();
        key
    }

    fn ingest(
        &self,
        db: &mut Database,
        id: i64,
        chat: i64,
        parent: i64,
        now: i64,
    ) -> Result<IngestOutcome, AppError> {
        let update = serde_json::from_value(json!({"update_id":id,
            "message":{"message_id":id + 100, "date":0, "chat":{"id":chat},
            "from":{"id":7,"is_bot":false}, "reply_to_message":{"message_id":parent},
            "text":"Synthetic reply"}}))
        .unwrap();
        IngressService::new(&self.0)
            .unwrap()
            .ingest(db, &update, now)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.storage.path.parent().unwrap()).unwrap();
    }
}

fn policy() -> NoticePolicy {
    NoticePolicy::new(&Delivery::default(), &Notifications::default()).unwrap()
}

fn inbox() -> InboxService {
    InboxService::new(&Delivery::default()).unwrap()
}

#[test]
fn receipt_deadline_claims_first_start_and_expiry_control_eligibility() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.seed(&mut db);
    for id in 11..=14 {
        fixture.ingest(&mut db, id, -101, 5, 1_000).unwrap();
    }
    let claims = inbox().claim(&mut db, &key, None, 3, 1_001).unwrap();
    inbox()
        .processing(
            &mut db,
            &key,
            &claims[0].reply.reply_id,
            &claims[0].claim_token,
            true,
            1_002,
        )
        .unwrap();
    // Inactivity before first start does not confirm processing.
    inbox()
        .processing(
            &mut db,
            &key,
            &claims[1].reply.reply_id,
            &claims[1].claim_token,
            false,
            1_002,
        )
        .unwrap();
    inbox()
        .acknowledge(
            &mut db,
            &key,
            &claims[2].reply.reply_id,
            &claims[2].claim_token,
            1_002,
        )
        .unwrap();
    assert_eq!(
        fixture.ingest(&mut db, 12, -101, 5, 60_000),
        Ok(IngestOutcome::Duplicate)
    );
    assert!(
        db.take_notice_attempts(&policy(), 100, 60_999)
            .unwrap()
            .is_empty()
    );
    let first = db.take_notice_attempts(&policy(), 1, 61_000).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].telegram_update_id, 12);
    assert_eq!(first[0].trigger_message_id, 112);
    assert_eq!(
        first[0].kind,
        NoticeKind::Unconfirmed {
            session_key: key.clone()
        }
    );
    assert!(!format!("{first:?}").contains(&key));
    assert!(!format!("{:?}", first[0].kind).contains(&key));
    assert_eq!(
        db.take_notice_attempts(&policy(), 100, 61_000).unwrap()[0].telegram_update_id,
        14
    );
    drop(db); // No dispatch success is recorded; both attempts remain consumed.
    let mut db = fixture.open();
    assert!(
        db.take_notice_attempts(&policy(), 100, 61_000)
            .unwrap()
            .is_empty()
    );
    // Delayed final completion and inactivity never reset first start.
    inbox()
        .processing(
            &mut db,
            &key,
            &claims[0].reply.reply_id,
            &claims[0].claim_token,
            false,
            700_000,
        )
        .unwrap();
    assert!(
        db.take_notice_attempts(&policy(), 100, 700_000)
            .unwrap()
            .is_empty()
    );
    fixture.ingest(&mut db, 15, -101, 5, 2_000).unwrap();
    assert!(
        db.take_notice_attempts(&policy(), 100, DAY + 2_000)
            .unwrap()
            .is_empty()
    );
    let expiry: i64 = fixture
        .sql()
        .query_row(
            "SELECT expires_at FROM notice_attempts WHERE telegram_update_id = 12",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(expiry, DAY + 1_000);
    assert_eq!(fixture.count("replies"), 4);
}

#[test]
fn first_start_and_attempt_have_one_transactional_order_across_connections() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.seed(&mut db);
    fixture.ingest(&mut db, 11, -101, 5, 1_000).unwrap();
    let claim = inbox()
        .claim(&mut db, &key, None, 1, 60_999)
        .unwrap()
        .remove(0);
    let mut other = fixture.open();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let gate = Arc::clone(&barrier);
    let starter = std::thread::spawn(move || {
        gate.wait();
        inbox()
            .processing(
                &mut other,
                &key,
                &claim.reply.reply_id,
                &claim.claim_token,
                true,
                61_000,
            )
            .unwrap()
    });
    barrier.wait();
    let attempts = db.take_notice_attempts(&policy(), 100, 61_000).unwrap();
    assert_eq!(starter.join().unwrap().processing_started_at, Some(61_000));
    assert!(attempts.len() <= 1);
    assert_eq!(fixture.count("notice_attempts"), attempts.len() as i64);
    drop(db);
    let mut db = fixture.open();
    assert!(
        db.take_notice_attempts(&policy(), 100, 700_000)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unroutable_decisions_are_authorized_deduplicated_and_restart_safe() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    // Unauthorized input creates no notice obligation.
    assert_eq!(
        fixture.ingest(&mut db, 10, -103, 5, 900),
        Ok(IngestOutcome::Ignored)
    );
    assert_eq!(
        fixture.ingest(&mut db, 11, -101, 6, 1_000),
        Ok(IngestOutcome::Unroutable)
    );
    assert_eq!(
        fixture.ingest(&mut db, 11, -101, 6, 1_001),
        Ok(IngestOutcome::Duplicate)
    );
    // The update ID can be reused: the other authorized chat is a distinct source.
    assert_eq!(
        fixture.ingest(&mut db, 11, -102, 6, 1_001),
        Ok(IngestOutcome::Unroutable)
    );
    // A distinct update ID for the same composite message is still one Reply.
    let mut duplicate: Update = serde_json::from_value(json!({"update_id":12,
        "message":{"message_id":111,"date":1,"chat":{"id":-101},
        "from":{"id":7,"is_bot":false},"reply_to_message":{"message_id":6},"text":"retry"}}))
    .unwrap();
    assert_eq!(
        IngressService::new(&fixture.0)
            .unwrap()
            .ingest(&mut db, &duplicate, 1_002),
        Ok(IngestOutcome::Duplicate)
    );
    assert_eq!(db.update_cursor().unwrap(), Some(12));
    // Reidentifying that second source must not create another notice obligation.
    duplicate.update_id = 13;
    duplicate.message.as_mut().unwrap().chat.id = -102;
    assert_eq!(
        IngressService::new(&fixture.0)
            .unwrap()
            .ingest(&mut db, &duplicate, 1_003),
        Ok(IngestOutcome::Duplicate)
    );
    let attempts = db.take_notice_attempts(&policy(), 100, 1_003).unwrap();
    assert_eq!(
        attempts
            .iter()
            .map(|a| (
                a.telegram_update_id,
                a.telegram_chat_id,
                a.trigger_message_id
            ))
            .collect::<Vec<_>>(),
        [(11, -101, 111), (11, -102, 111)]
    );
    assert!(attempts.iter().all(|a| a.kind == NoticeKind::Unroutable));
    // No success message ID is needed: lost responses/crash-before-dispatch cannot retry.
    drop(attempts);
    drop(db);
    let mut db = fixture.open();
    assert!(
        db.take_notice_attempts(&policy(), 100, 61_000)
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.count("notice_attempts"), 2);
    assert_eq!(fixture.count("message_routes"), 0);
    // Even after disposition cleanup, the retained attempt deduplicates its trigger.
    fixture
        .sql()
        .execute("DELETE FROM update_dispositions", [])
        .unwrap();
    assert_eq!(
        fixture.ingest(&mut db, 11, -101, 6, 61_000),
        Ok(IngestOutcome::Duplicate)
    );
    assert_eq!(db.update_cursor().unwrap(), Some(13));
    assert_eq!(
        fixture.ingest(&mut db, 11, -102, 6, 61_000),
        Ok(IngestOutcome::Duplicate)
    );
    assert_eq!(db.update_cursor().unwrap(), Some(13));
    duplicate.update_id = 12;
    duplicate.message.as_mut().unwrap().chat.id = -101;
    assert_eq!(
        IngressService::new(&fixture.0)
            .unwrap()
            .ingest(&mut db, &duplicate, 61_000),
        Ok(IngestOutcome::Duplicate)
    );
    assert_eq!(fixture.count("update_dispositions"), 0);
}

#[test]
fn notice_batch_and_ingress_failures_roll_back_all_decisions() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.seed(&mut db);
    fixture.ingest(&mut db, 11, -101, 5, 1_000).unwrap();
    fixture.ingest(&mut db, 12, -101, 6, 1_000).unwrap();
    fixture
        .sql()
        .execute_batch(
            "CREATE TRIGGER fail_notice BEFORE INSERT ON notice_attempts
         WHEN NEW.telegram_update_id = 12 BEGIN SELECT RAISE(ABORT, 'private detail'); END;",
        )
        .unwrap();
    let error = db.take_notice_attempts(&policy(), 100, 61_000).unwrap_err();
    assert_eq!(error, AppError::StorageFailure);
    assert!(!format!("{error} {error:?}").contains("private detail"));
    assert_eq!(fixture.count("notice_attempts"), 0);
    let marked: i64 = fixture
        .sql()
        .query_row(
            "SELECT (SELECT sum(notice_attempted) FROM replies)
         + (SELECT sum(notice_attempted) FROM update_dispositions)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(marked, 0);
    assert_eq!(
        db.inbox_snapshot(&key, None, 1, 61_000)
            .unwrap()
            .replies
            .len(),
        1
    );
    fixture
        .sql()
        .execute_batch(
            "DROP TRIGGER fail_notice;
         CREATE TRIGGER fail_cursor BEFORE UPDATE ON telegram_cursor
         BEGIN SELECT RAISE(ABORT, 'private detail'); END;",
        )
        .unwrap();
    assert_eq!(
        fixture.ingest(&mut db, 13, -101, 6, 2_000),
        Err(AppError::StorageFailure)
    );
    assert_eq!(fixture.count("update_dispositions"), 1);
    assert_eq!(db.update_cursor().unwrap(), Some(12));
    fixture
        .sql()
        .execute_batch("DROP TRIGGER fail_cursor")
        .unwrap();
    assert_eq!(
        db.take_notice_attempts(&policy(), 100, 61_000)
            .unwrap()
            .len(),
        2
    );
    // A committed notice does not prevent a later processing start or alter its identity.
    let claim = inbox()
        .claim(&mut db, &key, None, 1, 61_001)
        .unwrap()
        .remove(0);
    assert_eq!(
        inbox()
            .processing(
                &mut db,
                &key,
                &claim.reply.reply_id,
                &claim.claim_token,
                true,
                61_002
            )
            .unwrap()
            .processing_started_at,
        Some(61_002)
    );
}

#[test]
fn notice_admission_shares_receipt_metadata_and_outstanding_send_budgets() {
    let fixture = Fixture::new(Limits {
        metadata_bytes: 900,
        ..Limits::default()
    });
    let mut db = fixture.open();
    let key = fixture.seed(&mut db);
    fixture.ingest(&mut db, 11, -101, 5, 1_000).unwrap();
    let reservation = db.reserve_route(&key).unwrap();
    assert_eq!(
        db.take_notice_attempts(&policy(), 1, 61_000),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(fixture.count("notice_attempts"), 0);
    drop(reservation);
    assert_eq!(
        db.take_notice_attempts(&policy(), 1, 61_000).unwrap().len(),
        1
    );

    let fixture = Fixture::new(Limits {
        receipts: 1,
        ..Limits::default()
    });
    let mut db = fixture.open();
    let key = fixture.seed(&mut db);
    for id in [11, 12] {
        fixture.ingest(&mut db, id, -101, 5, 1_000).unwrap();
    }
    assert_eq!(
        db.take_notice_attempts(&policy(), 100, 61_000),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(fixture.count("notice_attempts"), 0); // Whole-batch rollback.
    assert_eq!(
        db.take_notice_attempts(&policy(), 1, 61_000).unwrap().len(),
        1
    );
    let claim = inbox()
        .claim(&mut db, &key, None, 1, 61_001)
        .unwrap()
        .remove(0);
    assert_eq!(
        inbox().acknowledge(
            &mut db,
            &key,
            &claim.reply.reply_id,
            &claim.claim_token,
            61_002
        ),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(
        fixture.ingest(&mut db, 13, -101, 6, 61_003),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(fixture.count("replies"), 2);
    assert_eq!(db.update_cursor().unwrap(), Some(12));
}

#[test]
fn disabled_notices_invalid_requests_and_custom_grace_do_not_consume_attempts() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.seed(&mut db);
    fixture.ingest(&mut db, 11, -101, 5, 1_000).unwrap();
    fixture.ingest(&mut db, 12, -101, 6, 1_000).unwrap();
    let delivery = Delivery {
        processing_start_grace_seconds: 2,
        ..Delivery::default()
    };
    let disabled = NoticePolicy::new(
        &delivery,
        &Notifications {
            notify_unconfirmed_processing: false,
            notify_unroutable: false,
        },
    )
    .unwrap();
    assert!(
        db.take_notice_attempts(&disabled, 100, 61_000)
            .unwrap()
            .is_empty()
    );
    for (limit, now) in [(0, 61_000), (101, 61_000), (1, -1)] {
        assert!(matches!(
            db.take_notice_attempts(&policy(), limit, now),
            Err(AppError::InvalidRequest(_))
        ));
    }
    for seconds in [0, 86_401, u64::MAX] {
        assert!(
            NoticePolicy::new(
                &Delivery {
                    processing_start_grace_seconds: seconds,
                    ..Delivery::default()
                },
                &Notifications::default()
            )
            .is_err()
        );
    }
    let custom = NoticePolicy::new(
        &delivery,
        &Notifications {
            notify_unconfirmed_processing: true,
            notify_unroutable: false,
        },
    )
    .unwrap();
    assert!(
        db.take_notice_attempts(&custom, 100, 2_999)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db.take_notice_attempts(&custom, 100, 3_000).unwrap()[0].kind,
        NoticeKind::Unconfirmed { session_key: key }
    );
    assert_eq!(
        db.take_notice_attempts(&policy(), 100, 3_000).unwrap()[0].kind,
        NoticeKind::Unroutable
    );
}
