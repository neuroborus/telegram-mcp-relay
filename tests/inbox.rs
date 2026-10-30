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
    claims()
        .processing(
            &mut db,
            &key,
            &original[1].reply.reply_id,
            &original[1].claim_token,
            true,
            2_001,
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

fn assert_invalid_transitions(db: &mut Database, key: &str, id: &str, token: &str, now: i64) {
    for active in [true, false] {
        assert_eq!(
            claims().processing(db, key, id, token, active, now),
            Err(AppError::InvalidClaim)
        );
    }
    assert_eq!(
        claims().reconcile_processing(db, key, id, token, "different-resume", now),
        Err(AppError::InvalidClaim)
    );
    assert_eq!(
        claims().acknowledge(db, key, id, token, now),
        Err(AppError::InvalidClaim)
    );
}

#[test]
fn first_start_is_idempotent_and_inactivity_never_releases_processing_ownership() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let claimed = claims()
        .claim(&mut db, &key, None, 1, 2_000)
        .unwrap()
        .remove(0);
    let token = &claimed.claim_token;
    let inactive = claims()
        .processing(&mut db, &key, &id, token, false, 2_001)
        .unwrap();
    assert_eq!(inactive.processing_started_at, None);
    assert_eq!(
        db.inbox_snapshot(&key, None, 1, 2_001).unwrap().replies[0],
        claimed.reply
    );
    assert_eq!(
        claims().reconcile_processing(&mut db, &key, &id, token, "resume", 2_001),
        Err(AppError::InvalidClaim)
    );
    let started = claims()
        .processing(&mut db, &key, &id, token, true, 2_002)
        .unwrap();
    assert_eq!(started.telegram_chat_id, -101);
    assert_eq!(started.processing_started_at, Some(2_002));
    for request_id in [String::new(), "x".repeat(65), "invalid\0id".to_owned()] {
        assert_eq!(
            claims().reconcile_processing(&mut db, &key, &id, token, &request_id, 2_002),
            Err(AppError::InvalidRequest("reconciliation request ID"))
        );
    }
    for (active, now) in [(true, 2_003), (false, 700_000), (true, 800_000)] {
        assert_eq!(
            claims()
                .processing(&mut db, &key, &id, token, active, now)
                .unwrap(),
            started
        );
    }
    assert!(
        claims()
            .claim(&mut db, &key, None, 1, 800_000)
            .unwrap()
            .is_empty()
    );
    let reply = db
        .inbox_snapshot(&key, None, 1, 800_000)
        .unwrap()
        .replies
        .remove(0);
    assert_eq!(reply.reply_id, id);
    assert_eq!(reply.state, ReplyState::Processing);
    assert_eq!(reply.claim_expires_at, None);
    assert_eq!(reply.expires_at, claimed.reply.expires_at);
}

#[test]
fn transitions_fence_wrong_sessions_replies_tokens_and_expired_preprocessing_claims() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let other = fixture.session(&mut db, -102);
    let id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    assert_invalid_transitions(&mut db, &key, &id, "not-a-claim", 1_001);
    let claimed = claims()
        .claim(&mut db, &key, None, 1, 2_000)
        .unwrap()
        .remove(0);
    assert_invalid_transitions(&mut db, &other, &id, &claimed.claim_token, 2_001);
    assert_invalid_transitions(&mut db, &key, "unknown-reply", &claimed.claim_token, 2_001);
    assert_invalid_transitions(&mut db, &key, &id, "wrong-token", 2_001);
    assert_invalid_transitions(&mut db, &key, &id, &claimed.claim_token, 62_000);
    let renewed = claims()
        .claim(&mut db, &key, None, 1, 62_000)
        .unwrap()
        .remove(0);
    assert_invalid_transitions(&mut db, &key, &id, &claimed.claim_token, 62_001);
    assert_eq!(
        claims().processing(
            &mut db,
            "unknown-session",
            &id,
            &renewed.claim_token,
            true,
            62_001
        ),
        Err(AppError::SessionNotFound)
    );
    assert_eq!(
        claims().acknowledge(&mut db, &key, &id, &renewed.claim_token, -1),
        Err(AppError::InvalidRequest("timestamp"))
    );
    let error = claims()
        .acknowledge(&mut db, &key, &id, "private-token", 62_001)
        .unwrap_err();
    assert_eq!(error.code(), "CLAIM_INVALID");
    assert!(!format!("{error:?} {error}").contains("private-token"));
    // A valid current claim can complete without a separate processing-start call.
    claims()
        .acknowledge(&mut db, &key, &id, &renewed.claim_token, 62_001)
        .unwrap();
}

#[test]
fn restart_resume_reconciliation_has_one_cas_winner_and_preserves_first_start() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let claimed = claims()
        .claim(&mut db, &key, None, 1, 2_000)
        .unwrap()
        .remove(0);
    claims()
        .processing(&mut db, &key, &id, &claimed.claim_token, true, 2_001)
        .unwrap();
    drop(db);
    let mut db = fixture.open();
    db.register_session(None, Some(&key), "Resumed", DAY)
        .unwrap();
    assert!(
        claims()
            .claim(&mut db, &key, None, 1, DAY)
            .unwrap()
            .is_empty()
    );
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = [db, fixture.open()]
        .into_iter()
        .enumerate()
        .map(|(request, mut db)| {
            let (barrier, key, id, token) = (
                barrier.clone(),
                key.clone(),
                id.clone(),
                claimed.claim_token.clone(),
            );
            std::thread::spawn(move || {
                barrier.wait();
                claims().reconcile_processing(
                    &mut db,
                    &key,
                    &id,
                    &token,
                    &format!("resume-{request}"),
                    DAY,
                )
            })
        })
        .collect();
    let mut winners = Vec::new();
    for worker in workers {
        match worker.join().unwrap() {
            Ok(winner) => winners.push(winner),
            Err(error) => assert_eq!(error, AppError::InvalidClaim),
        }
    }
    assert_eq!(winners.len(), 1);
    let winner = winners.remove(0);
    assert_eq!(winner.status.processing_started_at, Some(2_001));
    assert_ne!(winner.claim_token, claimed.claim_token);
    assert_eq!(winner.claim_token.len(), 64);
    assert!(!format!("{winner:?}").contains(&winner.claim_token));
    let mut db = fixture.open();
    assert_invalid_transitions(&mut db, &key, &id, &claimed.claim_token, DAY);
    let snapshot = db
        .inbox_snapshot(&key, None, 1, DAY)
        .unwrap()
        .replies
        .remove(0);
    assert_eq!(snapshot.reply_id, id);
    assert_eq!(snapshot.processing_started_at, Some(2_001));
    assert_eq!(snapshot.claimed_at, Some(DAY));
    assert_eq!(snapshot.expires_at, claimed.reply.expires_at);
    assert_eq!(
        claims()
            .processing(&mut db, &key, &id, &winner.claim_token, true, DAY)
            .unwrap(),
        winner.status
    );
    claims()
        .acknowledge(&mut db, &key, &id, &winner.claim_token, DAY)
        .unwrap();
    drop(db); // Simulate losing the acknowledgment response after commit.
    let mut db = fixture.open();
    claims()
        .acknowledge(&mut db, &key, &id, &winner.claim_token, DAY + 1)
        .unwrap();
    assert_invalid_transitions(&mut db, &key, &id, &claimed.claim_token, DAY + 1);
}

#[test]
fn lost_reconciliation_response_recovers_from_saved_request_and_fences_superseded_retries() {
    let mut fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let other = fixture.session(&mut db, -102);
    let id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let saved = claims()
        .claim(&mut db, &key, None, 1, 2_000)
        .unwrap()
        .remove(0);
    claims()
        .processing(&mut db, &key, &id, &saved.claim_token, true, 2_001)
        .unwrap();
    // The client saves this request ID with its old token before dispatch, then loses
    // the entire successful response and restarts with only that saved state.
    claims()
        .reconcile_processing(&mut db, &key, &id, &saved.claim_token, "resume-a", 2_002)
        .unwrap();
    drop(db);
    fixture.0.limits.metadata_bytes = 1;
    let mut db = fixture.open();
    let recovered = claims()
        .reconcile_processing(&mut db, &key, &id, &saved.claim_token, "resume-a", 3_000)
        .unwrap();
    assert_ne!(recovered.claim_token, saved.claim_token);
    assert_eq!(recovered.status.processing_started_at, Some(2_001));
    let reply = db
        .inbox_snapshot(&key, None, 1, 3_000)
        .unwrap()
        .replies
        .remove(0);
    assert_eq!(reply.reply_id, id);
    assert_eq!(reply.claimed_at, Some(2_002));
    assert_eq!(reply.expires_at, saved.reply.expires_at);
    assert_eq!(
        claims()
            .reconcile_processing(&mut db, &key, &id, &saved.claim_token, "resume-a", 3_001)
            .unwrap(),
        recovered
    );
    assert_invalid_transitions(&mut db, &key, &id, &saved.claim_token, 3_001);
    assert_eq!(
        claims().reconcile_processing(&mut db, &other, &id, &saved.claim_token, "resume-a", 3_001),
        Err(AppError::InvalidClaim)
    );
    assert_eq!(
        claims().reconcile_processing(&mut db, &key, &id, "wrong-token", "resume-a", 3_001),
        Err(AppError::InvalidClaim)
    );
    drop(db);
    fixture.0.limits = Limits::default();
    let mut db = fixture.open();
    let next = claims()
        .reconcile_processing(
            &mut db,
            &key,
            &id,
            &recovered.claim_token,
            &"b".repeat(64),
            3_002,
        )
        .unwrap();
    assert_eq!(
        claims().reconcile_processing(&mut db, &key, &id, &saved.claim_token, "resume-a", 3_003),
        Err(AppError::InvalidClaim)
    );
    claims()
        .acknowledge(&mut db, &key, &id, &next.claim_token, 3_003)
        .unwrap();
    assert_eq!(
        claims().reconcile_processing(
            &mut db,
            &key,
            &id,
            &recovered.claim_token,
            &"b".repeat(64),
            3_004,
        ),
        Err(AppError::InvalidClaim)
    );
}

#[test]
fn completion_removes_payloads_and_receipts_deduplicate_without_refresh_on_retry() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let mut document = input(11, -101, 5);
    document["message"]["document"] =
        json!({"file_id":"synthetic-file", "file_unique_id":"unique"});
    let id = fixture.receive(&mut db, document.clone(), 1_000);
    let token = claims()
        .claim(&mut db, &key, None, 1, 2_000)
        .unwrap()
        .remove(0)
        .claim_token;
    claims()
        .acknowledge(&mut db, &key, &id, &token, 2_001)
        .unwrap();
    assert!(
        db.inbox_snapshot(&key, None, 1, 2_001)
            .unwrap()
            .replies
            .is_empty()
    );
    let receipt: (String, String, String, i64, i64, i64) = fixture
        .sql()
        .query_row("SELECT * FROM acknowledgment_receipts", [], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap();
    assert_eq!(
        receipt,
        (
            id.clone(),
            key.clone(),
            token.clone(),
            11,
            2_001,
            2_001 + 2 * DAY
        )
    );
    let count: i64 = fixture
        .sql()
        .query_row("SELECT count(*) FROM replies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
    // Advance the cursor: deduplication must use the retained receipt, not just the cursor.
    fixture.receive(&mut db, input(12, -101, 5), 3_000);
    drop(db);
    let mut db = fixture.open();
    let update = serde_json::from_value(document).unwrap();
    assert_eq!(
        IngressService::new(&fixture.0)
            .unwrap()
            .ingest(&mut db, &update, 4_000)
            .unwrap(),
        IngestOutcome::Duplicate
    );
    assert_eq!(db.update_cursor().unwrap(), Some(12));
    claims()
        .acknowledge(&mut db, &key, &id, &token, 4_000)
        .unwrap();
    let timestamps: (i64, i64) = fixture
        .sql()
        .query_row(
            "SELECT created_at, expires_at FROM acknowledgment_receipts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(timestamps, (receipt.4, receipt.5));
    assert_eq!(
        claims().acknowledge(&mut db, &key, &id, &token, receipt.5),
        Err(AppError::InvalidClaim)
    );
}

#[test]
fn receipt_pressure_preserves_processing_payload_and_valid_ack_retry_is_free() {
    let fixture = Fixture::new(Limits {
        receipts: 1,
        ..Limits::default()
    });
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    for id in [11, 12] {
        fixture.receive(&mut db, input(id, -101, 5), 1_000);
    }
    let owned = claims().claim(&mut db, &key, None, 2, 2_000).unwrap();
    claims()
        .acknowledge(
            &mut db,
            &key,
            &owned[0].reply.reply_id,
            &owned[0].claim_token,
            2_001,
        )
        .unwrap();
    let (id, token) = (&owned[1].reply.reply_id, &owned[1].claim_token);
    claims()
        .processing(&mut db, &key, id, token, true, 2_001)
        .unwrap();
    assert_eq!(
        claims().acknowledge(&mut db, &key, id, token, 2_002),
        Err(AppError::StorageCapacity)
    );
    drop(db);
    let mut db = fixture.open();
    let reply = db
        .inbox_snapshot(&key, None, 1, 2_003)
        .unwrap()
        .replies
        .remove(0);
    assert_eq!(reply.reply_id, *id);
    assert_eq!(reply.state, ReplyState::Processing);
    assert_eq!(reply.text_or_caption.as_deref(), Some("reply"));
    assert_eq!(db.update_cursor().unwrap(), Some(12));
    claims()
        .acknowledge(
            &mut db,
            &key,
            &owned[0].reply.reply_id,
            &owned[0].claim_token,
            2_003,
        )
        .unwrap();
    // Unroutable dispositions consume the same receipt budget as completions.
    fixture
        .sql()
        .execute("DELETE FROM acknowledgment_receipts", [])
        .unwrap();
    let unknown = serde_json::from_value(input(13, -101, 999)).unwrap();
    assert_eq!(
        IngressService::new(&fixture.0)
            .unwrap()
            .ingest(&mut db, &unknown, 2_003)
            .unwrap(),
        IngestOutcome::Unroutable
    );
    assert_eq!(
        claims().acknowledge(&mut db, &key, id, token, 2_004),
        Err(AppError::StorageCapacity)
    );
    fixture
        .sql()
        .execute(
            "DELETE FROM update_dispositions WHERE kind = 'unroutable'",
            [],
        )
        .unwrap();
    claims()
        .acknowledge(&mut db, &key, id, token, 2_005)
        .unwrap();
}

#[test]
fn reconciliation_and_ack_account_for_replacement_bytes_and_outstanding_sends() {
    const BASE: u32 = 196 + 192 + 256 + 64;
    const PROOF: u32 = 64 + 6; // Prior token plus the UTF-8 request ID "resume".
    let mut fixture = Fixture::new(Limits {
        metadata_bytes: BASE + PROOF + 192 - 1,
        ..Limits::default()
    });
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let token = claims()
        .claim(&mut db, &key, None, 1, 2_000)
        .unwrap()
        .remove(0)
        .claim_token;
    claims()
        .processing(&mut db, &key, &id, &token, true, 2_001)
        .unwrap();
    let reservation = db.reserve_route(&key).unwrap();
    assert_eq!(
        claims().reconcile_processing(&mut db, &key, &id, &token, "resume", 2_002),
        Err(AppError::StorageCapacity)
    );
    drop(reservation);
    let rotated = claims()
        .reconcile_processing(&mut db, &key, &id, &token, "resume", 2_002)
        .unwrap();
    drop(db);
    fixture.0.limits.metadata_bytes = BASE + PROOF;
    let mut db = fixture.open();
    // Replacing a proof of the same size consumes no additional metadata.
    let rotated = claims()
        .reconcile_processing(&mut db, &key, &id, &rotated.claim_token, "nextid", 2_003)
        .unwrap();
    assert_eq!(
        claims().reconcile_processing(&mut db, &key, &id, &rotated.claim_token, "longer-id", 2_004),
        Err(AppError::StorageCapacity)
    );
    drop(db);
    fixture.0.limits.metadata_bytes = BASE - 1;
    let mut db = fixture.open();
    assert_eq!(
        claims().acknowledge(&mut db, &key, &id, &rotated.claim_token, 2_004),
        Err(AppError::StorageCapacity)
    );
    drop(db);
    // Completion sheds the recovery proof even when retained rows exceed a lowered cap.
    fixture.0.limits.metadata_bytes = BASE;
    let mut db = fixture.open();
    claims()
        .acknowledge(&mut db, &key, &id, &rotated.claim_token, 2_005)
        .unwrap();
}

#[test]
fn transition_failures_roll_back_first_start_token_rotation_and_payload_removal() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let token = claims()
        .claim(&mut db, &key, None, 1, 2_000)
        .unwrap()
        .remove(0)
        .claim_token;
    fixture
        .sql()
        .execute_batch(
            "CREATE TRIGGER fail_update BEFORE UPDATE ON replies
        BEGIN SELECT RAISE(ABORT, 'private-storage-detail'); END;",
        )
        .unwrap();
    assert_eq!(
        claims().processing(&mut db, &key, &id, &token, true, 2_001),
        Err(AppError::StorageFailure)
    );
    assert_eq!(
        db.inbox_snapshot(&key, None, 1, 2_001).unwrap().replies[0].state,
        ReplyState::Claimed
    );
    fixture
        .sql()
        .execute_batch("DROP TRIGGER fail_update")
        .unwrap();
    claims()
        .processing(&mut db, &key, &id, &token, true, 2_002)
        .unwrap();
    fixture
        .sql()
        .execute_batch(
            "CREATE TRIGGER fail_update BEFORE UPDATE ON replies
        BEGIN SELECT RAISE(ABORT, 'private-storage-detail'); END;",
        )
        .unwrap();
    let error = claims()
        .reconcile_processing(&mut db, &key, &id, &token, "resume", 2_003)
        .unwrap_err();
    assert_eq!(error, AppError::StorageFailure);
    assert!(!format!("{error:?} {error}").contains("private-storage-detail"));
    fixture
        .sql()
        .execute_batch(
            "DROP TRIGGER fail_update;
        CREATE TRIGGER fail_delete BEFORE DELETE ON replies
        BEGIN SELECT RAISE(ABORT, 'private-storage-detail'); END;",
        )
        .unwrap();
    assert_eq!(
        claims().acknowledge(&mut db, &key, &id, &token, 2_004),
        Err(AppError::StorageFailure)
    );
    drop(db);
    let mut db = fixture.open();
    let count: i64 = fixture
        .sql()
        .query_row("SELECT count(*) FROM acknowledgment_receipts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
    let reply = db
        .inbox_snapshot(&key, None, 1, 2_005)
        .unwrap()
        .replies
        .remove(0);
    assert_eq!(reply.reply_id, id);
    assert_eq!(reply.text_or_caption.as_deref(), Some("reply"));
    assert_eq!(reply.processing_started_at, Some(2_002));
    fixture
        .sql()
        .execute_batch("DROP TRIGGER fail_delete")
        .unwrap();
    claims()
        .acknowledge(&mut db, &key, &id, &token, 2_005)
        .unwrap();
}

#[test]
fn processing_ownership_ends_only_at_reply_retention_boundary() {
    let fixture = Fixture::new(Limits::default());
    let mut db = fixture.open();
    let key = fixture.session(&mut db, -101);
    let id = fixture.receive(&mut db, input(11, -101, 5), 1_000);
    let claimed = claims()
        .claim(&mut db, &key, None, 1, 2_000)
        .unwrap()
        .remove(0);
    claims()
        .processing(&mut db, &key, &id, &claimed.claim_token, true, 2_001)
        .unwrap();
    let rotated = claims()
        .reconcile_processing(&mut db, &key, &id, &claimed.claim_token, "expiry", 2_002)
        .unwrap();
    let expiry = claimed.reply.expires_at;
    claims()
        .processing(&mut db, &key, &id, &rotated.claim_token, false, expiry - 1)
        .unwrap();
    assert_invalid_transitions(&mut db, &key, &id, &rotated.claim_token, expiry);
    assert_eq!(
        claims().reconcile_processing(&mut db, &key, &id, &claimed.claim_token, "expiry", expiry),
        Err(AppError::InvalidClaim)
    );
    assert!(
        db.inbox_snapshot(&key, None, 1, expiry)
            .unwrap()
            .replies
            .is_empty()
    );
}
