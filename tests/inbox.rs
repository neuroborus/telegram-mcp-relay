use std::{fs, os::unix::fs::DirBuilderExt, sync::Arc};

use rusqlite::Connection;
use serde_json::{Value, json};
use telegram_mcp_relay::{
    config::{Config, Delivery, Limits, Storage},
    error::AppError,
    repositories::{Database, InboxReply, IngestOutcome, ReplyState, ReplyTarget, RouteKind},
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
            .join(format!("inbox-test-{:016x}", u64::from_ne_bytes(random)));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let mut config = Config {
            limits,
            storage: Storage {
                path: directory.join("relay.sqlite"),
                session_ttl_days: 1,
                reply_ttl_days: 2,
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

    fn session(&self, db: &mut Database, chat: i64) -> String {
        let key = db
            .register_session(None, None, "Same", 100)
            .unwrap()
            .session_key;
        self.route(db, &key, chat, 5);
        key
    }

    fn route(&self, db: &mut Database, key: &str, chat: i64, message: i64) {
        let reservation = db.reserve_route(key).unwrap();
        db.persist_route(reservation, chat, message, RouteKind::Text, 100, 100)
            .unwrap();
    }

    fn receive(&self, db: &mut Database, input: Value, now: i64) -> String {
        let update = serde_json::from_value(input).unwrap();
        match IngressService::new(&self.0)
            .unwrap()
            .ingest(db, &update, now)
            .unwrap()
        {
            IngestOutcome::ReplyStored { reply_id, .. } => reply_id,
            other => panic!("expected reply, got {other:?}"),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.storage.path.parent().unwrap()).unwrap();
    }
}

fn input(id: i64, chat: i64, parent: i64) -> Value {
    json!({"update_id":id, "message":{"message_id":id + 100, "date":1,
        "chat":{"id":chat}, "from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":parent}, "text":"reply"}})
}

fn claims() -> InboxService {
    InboxService::new(&Delivery::default()).unwrap()
}

fn update_ids(replies: &[InboxReply]) -> Vec<i64> {
    replies.iter().map(|r| r.telegram_update_id).collect()
}

#[test]
fn snapshots_paginate_in_receipt_order_without_claiming_and_keep_sessions_separate() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let a = fixture.session(&mut db, -101);
    let b = fixture.session(&mut db, -102);
    let first_id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let mut document = input(12, -101, 5);
    document["message"].as_object_mut().unwrap().remove("text");
    document["message"]["document"] = json!({"file_id":"synthetic", "file_unique_id":"unique"});
    fixture.receive(&mut db, document, 1_000);
    fixture.receive(&mut db, input(13, -101, 5), 1_000);
    fixture.receive(&mut db, input(14, -102, 5), 1_000);
    fixture.receive(&mut db, input(1, -101, 5), 1_001); // A later update-ID sequence.
    let activity = db.session(&a).unwrap();
    let first = db.inbox_snapshot(&a, None, 2, 1_001).unwrap();
    assert_eq!(update_ids(&first.replies), [11, 12]);
    assert_eq!(first.replies[0].reply_id, first_id);
    assert_eq!(first.replies[0].text_or_caption.as_deref(), Some("reply"));
    assert_eq!(first.replies[1].text_or_caption, None);
    assert!(
        first.replies[1]
            .attachment_metadata_json
            .as_ref()
            .unwrap()
            .contains("synthetic")
    );
    assert!(
        first
            .replies
            .iter()
            .all(|r| r.state == ReplyState::Pending && r.claimed_at.is_none())
    );
    assert_eq!(db.inbox_snapshot(&a, None, 2, 1_001).unwrap(), first);
    let last = db
        .inbox_snapshot(&a, first.next.as_ref(), 2, 1_001)
        .unwrap();
    assert_eq!(update_ids(&last.replies), [13, 1]);
    assert_eq!(last.next, None);
    assert_eq!(
        db.inbox_snapshot(&b, None, 2, 1_001).unwrap().replies[0].telegram_update_id,
        14
    );
    assert_eq!(db.session(&a).unwrap(), activity);
    assert!(!format!("{first:?}").contains(&first_id));
    assert_eq!(
        claims().claim(&mut db, &a, None, 1, 1_001).unwrap()[0]
            .reply
            .reply_id,
        first_id
    );
    assert_eq!(
        db.inbox_snapshot(&a, None, 1, 1_001).unwrap().replies[0].state,
        ReplyState::Claimed
    );
}

#[test]
fn simultaneous_consumers_have_only_one_committed_owner() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let other = fixture.open();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = [db, other]
        .into_iter()
        .map(|mut db| {
            let barrier = barrier.clone();
            let key = key.clone();
            std::thread::spawn(move || {
                barrier.wait();
                claims().claim(&mut db, &key, None, 1, 2_000).unwrap()
            })
        })
        .collect();
    let winners: Vec<_> = workers
        .into_iter()
        .flat_map(|w| w.join().unwrap())
        .collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(winners[0].reply.reply_id, id);
    let token = &winners[0].claim_token;
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    let persisted: String = fixture
        .sql()
        .query_row("SELECT claim_token FROM replies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(&persisted, token);
    assert!(!format!("{:?}", winners[0]).contains(token));
}

#[test]
fn restart_recovers_claims_at_lease_boundary_without_reclaiming_processing_work() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    fixture.receive(&mut db, input(11, -101, 5), 1_000);
    fixture.receive(&mut db, input(12, -101, 5), 1_000);
    let original = claims().claim(&mut db, &key, None, 2, 2_000).unwrap();
    fixture
        .sql()
        .execute(
            "UPDATE replies SET state = 'processing', processing_started_at = 2001,
        claim_expires_at = NULL WHERE telegram_update_id = 12",
            [],
        )
        .unwrap();
    drop(db);
    let mut db = fixture.open();
    assert!(
        claims()
            .claim(&mut db, &key, None, 2, 61_999)
            .unwrap()
            .is_empty()
    );
    let renewed = claims().claim(&mut db, &key, None, 2, 62_000).unwrap();
    assert_eq!(renewed.len(), 1);
    assert_eq!(renewed[0].reply.reply_id, original[0].reply.reply_id);
    assert_ne!(renewed[0].claim_token, original[0].claim_token);
    assert_eq!(renewed[0].reply.claimed_at, Some(62_000));
    assert_eq!(renewed[0].reply.claim_expires_at, Some(122_000));
    assert_eq!(renewed[0].reply.expires_at, original[0].reply.expires_at);
    // Neither a new registration nor a time beyond the typing cap steals processing work.
    db.register_session(None, Some(&key), "Renamed", 700_000)
        .unwrap();
    let later = claims().claim(&mut db, &key, None, 2, 700_000).unwrap();
    assert_eq!(later.len(), 1);
    assert_eq!(later[0].reply.reply_id, original[0].reply.reply_id);
    let snapshot = db.inbox_snapshot(&key, None, 2, 700_000).unwrap();
    assert_eq!(snapshot.replies[1].state, ReplyState::Processing);
    assert_eq!(snapshot.replies[1].processing_started_at, Some(2_001));
    let processing_token: String = fixture
        .sql()
        .query_row(
            "SELECT claim_token FROM replies WHERE telegram_update_id = 12",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(processing_token, original[1].claim_token);
}

#[test]
fn question_claims_match_composite_targets_and_session_then_use_normal_inbox_arbitration() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let a = fixture.session(&mut db, -101);
    let b = fixture.session(&mut db, -102);
    fixture.route(&mut db, &a, -101, 6);
    fixture.route(&mut db, &a, -102, 6);
    fixture.receive(&mut db, input(11, -101, 5), 1_000);
    fixture.receive(&mut db, input(12, -102, 6), 1_000);
    let id = fixture.receive(&mut db, input(13, -101, 6), 1_000);
    fixture.receive(&mut db, input(14, -102, 5), 1_000);
    let target = Some(ReplyTarget {
        chat_id: -101,
        message_id: 6,
    });
    assert!(
        claims()
            .claim(&mut db, &b, target, 2, 2_000)
            .unwrap()
            .is_empty()
    );
    let matched = claims().claim(&mut db, &a, target, 2, 2_000).unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].reply.reply_id, id);
    let remaining = claims().claim(&mut db, &a, None, 2, 2_000).unwrap();
    assert_eq!(remaining[0].reply.telegram_update_id, 11);
    assert_eq!(remaining[1].reply.telegram_update_id, 12);
    assert_eq!(
        claims().claim(&mut db, &b, None, 2, 2_000).unwrap()[0]
            .reply
            .telegram_update_id,
        14
    );
}

#[test]
fn expired_replies_are_excluded_and_claim_leases_never_extend_retention() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let expiry = 1_000 + 2 * DAY;
    // A retained identity remains usable beyond the independent session TTL.
    let last = claims().claim(&mut db, &key, None, 1, expiry - 1).unwrap();
    assert_eq!(last[0].reply.claim_expires_at, Some(expiry));
    assert!(
        claims()
            .claim(&mut db, &key, None, 1, expiry)
            .unwrap()
            .is_empty()
    );
    assert!(
        db.inbox_snapshot(&key, None, 1, expiry)
            .unwrap()
            .replies
            .is_empty()
    );
    let retained: i64 = fixture
        .sql()
        .query_row("SELECT count(*) FROM replies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(retained, 1); // Reads/claims never delete even an expired payload.
}

#[test]
fn token_admission_counts_send_reservations_and_rotation_replaces_existing_bytes() {
    let fixture = Fixture::new(Limits {
        metadata_bytes: 196 + 192 + 2 * 256 + 192 + 64,
        ..Limits::default()
    });
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    fixture.receive(&mut db, input(11, -101, 5), 1_000);
    fixture.receive(&mut db, input(12, -101, 5), 1_000);
    let reservation = db.reserve_route(&key).unwrap();
    assert_eq!(
        claims().claim(&mut db, &key, None, 2, 2_000),
        Err(AppError::StorageCapacity)
    );
    let snapshot = db.inbox_snapshot(&key, None, 2, 2_000).unwrap();
    assert!(
        snapshot
            .replies
            .iter()
            .all(|r| r.state == ReplyState::Pending)
    );
    let first = claims().claim(&mut db, &key, None, 1, 2_000).unwrap();
    assert_eq!(
        claims().claim(&mut db, &key, None, 1, 2_001),
        Err(AppError::StorageCapacity)
    );
    let rotated = claims().claim(&mut db, &key, None, 1, 62_000).unwrap();
    assert_eq!(rotated[0].reply.reply_id, first[0].reply.reply_id);
    assert_ne!(rotated[0].claim_token, first[0].claim_token);
    drop(reservation);
    assert_eq!(
        claims()
            .claim(&mut db, &key, None, 1, 62_001)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.update_cursor().unwrap(), Some(12));
}

#[test]
fn late_claim_failure_rolls_back_the_entire_batch_and_redacts_sql_details() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    for id in [11, 12] {
        fixture.receive(&mut db, input(id, -101, 5), 1_000);
    }
    fixture
        .sql()
        .execute_batch(
            "CREATE TRIGGER synthetic_failure BEFORE UPDATE ON replies
        WHEN OLD.telegram_update_id = 12 BEGIN SELECT RAISE(ABORT, 'private detail'); END;",
        )
        .unwrap();
    let result = claims().claim(&mut db, &key, None, 2, 2_000);
    assert_eq!(result, Err(AppError::StorageFailure));
    assert!(!format!("{result:?}").contains("private detail"));
    drop(db);
    let mut db = fixture.open();
    let snapshot = db.inbox_snapshot(&key, None, 2, 2_000).unwrap();
    assert!(
        snapshot
            .replies
            .iter()
            .all(|r| r.state == ReplyState::Pending
                && r.claimed_at.is_none()
                && r.claim_expires_at.is_none())
    );
    fixture
        .sql()
        .execute_batch("DROP TRIGGER synthetic_failure")
        .unwrap();
    assert_eq!(
        claims().claim(&mut db, &key, None, 2, 2_000).unwrap().len(),
        2
    );
}

#[test]
fn payload_and_item_bounds_apply_to_snapshots_and_claims_without_skipping() {
    for (limits, count, page_size) in [
        (
            Limits {
                reply_bytes: 9,
                ..Limits::default()
            },
            3,
            1,
        ),
        (Limits::default(), 102, 100),
    ] {
        let fixture = Fixture::new(limits);
        let mut db = fixture.open();
        let key = fixture.session(&mut db, -101);
        for id in 1..=count {
            fixture.receive(&mut db, input(id, -101, 5), 1_000);
        }
        let page = db.inbox_snapshot(&key, None, 100, 2_000).unwrap();
        assert_eq!(page.replies.len(), page_size);
        let next = db
            .inbox_snapshot(&key, page.next.as_ref(), 100, 2_000)
            .unwrap();
        assert_eq!(next.replies[0].telegram_update_id, page_size as i64 + 1);
        let owned = claims().claim(&mut db, &key, None, 100, 2_000).unwrap();
        assert_eq!(owned.len(), page_size);
        assert_eq!(owned[0].reply.reply_id, page.replies[0].reply_id);
        assert_eq!(
            claims().claim(&mut db, &key, None, 100, 2_000).unwrap()[0]
                .reply
                .telegram_update_id,
            page_size as i64 + 1
        );
    }
}

#[test]
fn invalid_queries_and_lease_settings_fail_without_claim_effects() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    fixture.receive(&mut db, input(11, -101, 5), 1_000);
    for (limit, now) in [(0, 2_000), (101, 2_000), (1, -1)] {
        assert!(matches!(
            db.inbox_snapshot(&key, None, limit, now),
            Err(AppError::InvalidRequest(_))
        ));
        assert!(matches!(
            claims().claim(&mut db, &key, None, limit, now),
            Err(AppError::InvalidRequest(_))
        ));
    }
    assert!(matches!(
        claims().claim(&mut db, &key, None, 1, i64::MAX),
        Err(AppError::InvalidRequest(_))
    ));
    assert_eq!(
        db.inbox_snapshot("unknown", None, 1, 2_000),
        Err(AppError::SessionNotFound)
    );
    assert_eq!(
        claims().claim(&mut db, "unknown", None, 1, 2_000),
        Err(AppError::SessionNotFound)
    );
    for seconds in [0, 601, u64::MAX] {
        assert!(matches!(
            InboxService::new(&Delivery {
                claim_lease_seconds: seconds,
                ..Delivery::default()
            }),
            Err(AppError::InvalidConfig(_))
        ));
    }
    let maximum = InboxService::new(&Delivery {
        claim_lease_seconds: 600,
        ..Delivery::default()
    })
    .unwrap();
    let owned = maximum.claim(&mut db, &key, None, 1, 2_000).unwrap();
    assert_eq!(owned[0].reply.claim_expires_at, Some(602_000));
}
