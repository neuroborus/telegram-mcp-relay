//! Confirmed processing drives the real Bot API adapter; replies remain durable.

use super::*;
use crate::{
    repositories::{ClaimedReply, CleanupSweep, IngestOutcome},
    services::{cleanup::CleanupService, inbox::InboxService, ingress::IngressService},
};
use std::time::Duration;

fn inbox(service: &OutboundService, fixture: &Fixture) -> InboxService {
    InboxService::new(&fixture.config.delivery)
        .unwrap()
        .with_typing(service.typing())
}

fn claim(fixture: &Fixture, database: &Arc<Mutex<Database>>, key: &str, id: i64) -> ClaimedReply {
    claim_at(fixture, database, key, id, 50_000)
}

fn claim_at(
    fixture: &Fixture,
    database: &Arc<Mutex<Database>>,
    key: &str,
    id: i64,
    now: i64,
) -> ClaimedReply {
    let mut database = database.lock().unwrap();
    let reservation = database.reserve_route(key).unwrap();
    database
        .persist_route(reservation, -101, id, RouteKind::Text, now, now)
        .unwrap();
    let update = serde_json::from_value(json!({"update_id":id,"message":{
        "message_id":id+100,"date":50,"chat":{"id":-101},"from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":id},"text":"synthetic reply"}}))
    .unwrap();
    assert!(matches!(
        IngressService::new(&fixture.config)
            .unwrap()
            .ingest(&mut database, &update, now)
            .unwrap(),
        IngestOutcome::ReplyStored { .. }
    ));
    InboxService::new(&fixture.config.delivery)
        .unwrap()
        .claim(&mut database, key, None, 1, now)
        .unwrap()
        .pop()
        .unwrap()
}

fn fixture() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.config.telegram.allowed_user_ids = vec![7];
    fixture.config.limits.routes = 10;
    fixture
}

#[tokio::test]
async fn fenced_commit_precedes_typing_and_inactivity_cancels_a_stalled_action() {
    let fixture = fixture();
    let database = fixture.open();
    let session = fixture.register(&database);
    let key = &session.session_key;
    let (_release, blocked) = oneshot::channel();
    let mut mock = Mock::new(
        &fixture.config,
        vec![Response::Held(blocked, json!({"ok":true,"result":true}))],
    )
    .await;
    let service = mock.service(&fixture, &database);
    let inbox = inbox(&service, &fixture);
    let reply = claim(&fixture, &database, key, 1);
    assert_eq!(mock.count(), 0); // Receipt/claim alone never starts activity.
    fixture.sql().execute_batch("CREATE TRIGGER fail_start BEFORE UPDATE ON replies BEGIN SELECT RAISE(ABORT, 'private'); END").unwrap();
    assert!(
        inbox
            .processing(
                &mut database.lock().unwrap(),
                key,
                &reply.reply.reply_id,
                &reply.claim_token,
                true,
                50_000
            )
            .is_err()
    );
    assert_eq!(mock.count(), 0);
    fixture
        .sql()
        .execute_batch("DROP TRIGGER fail_start")
        .unwrap();
    let started = inbox
        .processing(
            &mut database.lock().unwrap(),
            key,
            &reply.reply.reply_id,
            &reply.claim_token,
            true,
            50_000,
        )
        .unwrap();
    mock.seen.recv().await.unwrap();
    {
        let requests = mock.requests.lock().unwrap();
        assert_eq!(requests[0].method, "/botSynthetic/sendChatAction");
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[0].body).unwrap(),
            json!({"chat_id":-101,"action":"typing"})
        );
    }
    assert_eq!(
        inbox
            .processing(
                &mut database.lock().unwrap(),
                key,
                &reply.reply.reply_id,
                "stale",
                false,
                50_001
            )
            .unwrap_err(),
        AppError::InvalidClaim
    );
    assert_eq!(
        inbox
            .processing(
                &mut database.lock().unwrap(),
                key,
                &reply.reply.reply_id,
                &reply.claim_token,
                false,
                50_001
            )
            .unwrap(),
        started
    );
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3_600)).await;
    tokio::task::yield_now().await;
    assert_eq!(mock.count(), 1);
    // No completion or notice is emitted by stopping/expiry; restart retains first start.
    let mut reopened = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    let page = reopened.inbox_snapshot(key, None, 1, 3_650_000).unwrap();
    assert_eq!(page.replies[0].processing_started_at, Some(50_000));
    assert!(
        inbox
            .claim(&mut reopened, key, None, 1, 3_650_000)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn accepted_session_text_and_document_sends_stop_only_their_own_activity() {
    for document in [false, true] {
        let fixture = fixture();
        let database = fixture.open();
        let a = fixture.register(&database);
        let b = fixture.register(&database);
        let mut mock = Mock::new(
            &fixture.config,
            vec![
                Response::Json(200, json!({"ok":true,"result":true})),
                Response::Json(200, sent(7)),
                Response::Json(200, json!({"ok":true,"result":true})),
            ],
        )
        .await;
        let service = mock.service(&fixture, &database);
        let inbox = inbox(&service, &fixture);
        let first = claim(&fixture, &database, &a.session_key, 1);
        let second = claim(&fixture, &database, &b.session_key, 2);
        for (session, reply) in [(&a, &first), (&b, &second)] {
            inbox
                .processing(
                    &mut database.lock().unwrap(),
                    &session.session_key,
                    &reply.reply.reply_id,
                    &reply.claim_token,
                    true,
                    50_000,
                )
                .unwrap();
        }
        mock.seen.recv().await.unwrap(); // Both same-title sessions merge into one action.
        if document {
            service
                .send_file(&a.session_key, fixture.file(b"synthetic"), "result")
                .await
                .unwrap();
        } else {
            service.send_text(&a.session_key, "result").await.unwrap();
        }
        mock.seen.recv().await.unwrap(); // Accepted send.
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::time::resume();
        mock.seen.recv().await.unwrap(); // The other session still refreshes.
        inbox
            .acknowledge(
                &mut database.lock().unwrap(),
                &b.session_key,
                &second.reply.reply_id,
                &second.claim_token,
                54_000,
            )
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(600)).await;
        tokio::task::yield_now().await;
        assert_eq!(mock.count(), 3); // No activity from the session that sent its result.
        let page = database
            .lock()
            .unwrap()
            .inbox_snapshot(&a.session_key, None, 1, 650_000)
            .unwrap();
        assert_eq!(page.replies[0].processing_started_at, Some(50_000));
        tokio::time::resume();
    }
}

#[tokio::test]
async fn reconciliation_retry_preserves_resumed_typing_and_the_original_deadline() {
    let mut fixture = fixture();
    fixture.config.activity.typing_max_duration_seconds = 12;
    let database = fixture.open();
    let session = fixture.register(&database);
    let key = &session.session_key;
    let mut mock = Mock::new(
        &fixture.config,
        vec![
            Response::Json(200, json!({"ok":true,"result":true})),
            Response::Json(403, json!({"ok":false,"error_code":403})),
            Response::Json(200, json!({"ok":true,"result":true})),
            Response::Json(200, json!({"ok":true,"result":true})),
        ],
    )
    .await;
    let service = mock.service(&fixture, &database);
    let inbox = inbox(&service, &fixture);
    let reply = claim(&fixture, &database, key, 1);
    let started = inbox
        .processing(
            &mut database.lock().unwrap(),
            key,
            &reply.reply.reply_id,
            &reply.claim_token,
            true,
            50_000,
        )
        .unwrap();
    mock.seen.recv().await.unwrap();
    assert!(service.send_text(key, "result").await.is_err());
    mock.seen.recv().await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(4)).await;
    tokio::time::resume();
    mock.seen.recv().await.unwrap();
    let resumed = inbox
        .reconcile_processing(
            &mut database.lock().unwrap(),
            key,
            &reply.reply.reply_id,
            &reply.claim_token,
            "resume",
            54_000,
        )
        .unwrap();
    assert_eq!(resumed.status, started);
    inbox
        .processing(
            &mut database.lock().unwrap(),
            key,
            &reply.reply.reply_id,
            &resumed.claim_token,
            true,
            54_000,
        )
        .unwrap();
    let recovered = inbox
        .reconcile_processing(
            &mut database.lock().unwrap(),
            key,
            &reply.reply.reply_id,
            &reply.claim_token,
            "resume",
            54_000,
        )
        .unwrap();
    assert_eq!(recovered, resumed); // Replaying the original proof does not stop resumed work.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(4)).await;
    tokio::time::resume();
    // Bound a regression failure: the old retry behavior stopped this action indefinitely.
    tokio::time::timeout(Duration::from_secs(4), mock.seen.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(4)).await;
    tokio::task::yield_now().await;
    assert_eq!(mock.count(), 4);
    assert_eq!(
        inbox
            .processing(
                &mut database.lock().unwrap(),
                key,
                &reply.reply.reply_id,
                &resumed.claim_token,
                true,
                62_000
            )
            .unwrap(),
        started
    );
    assert!(
        inbox
            .claim(&mut database.lock().unwrap(), key, None, 1, 62_000)
            .unwrap()
            .is_empty()
    );
    inbox
        .acknowledge(
            &mut database.lock().unwrap(),
            key,
            &reply.reply.reply_id,
            &resumed.claim_token,
            62_000,
        )
        .unwrap();
}

#[tokio::test]
async fn retention_commit_restores_typing_admission_and_rollback_preserves_activity() {
    let mut fixture = fixture();
    fixture.config.limits.inbox_items = 1;
    fixture.config.storage.reply_ttl_days = 1;
    fixture.config.storage.route_ttl_days = 1;
    let database = fixture.open();
    let session = fixture.register(&database);
    let key = &session.session_key;
    let mut mock = Mock::new(
        &fixture.config,
        (0..3)
            .map(|_| Response::Json(200, json!({"ok":true,"result":true})))
            .collect(),
    )
    .await;
    let service = mock.service(&fixture, &database);
    let inbox = inbox(&service, &fixture);
    let cleanup = CleanupService::new(database.clone(), &fixture.config.storage)
        .unwrap()
        .with_typing(service.typing());
    let first = claim(&fixture, &database, key, 1);
    inbox
        .processing(
            &mut database.lock().unwrap(),
            key,
            &first.reply.reply_id,
            &first.claim_token,
            true,
            50_000,
        )
        .unwrap();
    mock.seen.recv().await.unwrap();

    // Fail after DELETE RETURNING has collected the reply, before the transaction commits.
    fixture
        .sql()
        .execute_batch(
            "CREATE TRIGGER fail_cleanup BEFORE DELETE ON message_routes
        BEGIN SELECT RAISE(ABORT, 'synthetic private detail'); END",
        )
        .unwrap();
    let expiry = 50_000 + 86_400_000;
    let mut sweep = CleanupSweep::new(expiry).unwrap();
    assert_eq!(
        cleanup.cleanup_batch(&mut sweep),
        Err(AppError::StorageFailure)
    );
    assert_eq!(
        database
            .lock()
            .unwrap()
            .inbox_snapshot(key, None, 1, 50_000)
            .unwrap()
            .replies[0]
            .reply_id,
        first.reply.reply_id
    );
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(4)).await;
    tokio::time::resume();
    // Rollback must retain the current activity as well as its remembered cap.
    tokio::time::timeout(Duration::from_secs(4), mock.seen.recv())
        .await
        .unwrap()
        .unwrap();
    fixture
        .sql()
        .execute_batch("DROP TRIGGER fail_cleanup")
        .unwrap();

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(86_400)).await;
    tokio::task::yield_now().await;
    assert_eq!(mock.count(), 2); // Typing expiry keeps the cap, without refreshing forever.
    tokio::time::resume();
    assert_eq!(cleanup.cleanup_batch(&mut sweep).unwrap().removed, 2);
    let second = claim_at(&fixture, &database, key, 2, expiry);
    inbox
        .processing(
            &mut database.lock().unwrap(),
            key,
            &second.reply.reply_id,
            &second.claim_token,
            true,
            expiry,
        )
        .unwrap();
    // At inbox_items=1 the deleted reply's cap previously prevented this immediate action.
    tokio::time::timeout(Duration::from_secs(4), mock.seen.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mock.count(), 3);
    let requests = mock.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .all(|r| r.method == "/botSynthetic/sendChatAction")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[2].body).unwrap(),
        json!({"chat_id":-101,"action":"typing"})
    );
    drop(requests);
    inbox
        .acknowledge(
            &mut database.lock().unwrap(),
            key,
            &second.reply.reply_id,
            &second.claim_token,
            expiry,
        )
        .unwrap();
}
