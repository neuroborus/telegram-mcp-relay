use super::*;
use std::{fs, os::unix::fs::DirBuilderExt};

use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};

use crate::{
    config::Storage,
    repositories::{IngestOutcome, RouteKind},
    services::{inbox::InboxService, ingress::IngressService},
};

struct Fixture(Config);

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "notice-delivery-{:016x}",
            u64::from_ne_bytes(random)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let mut config = Config {
            storage: Storage {
                path: directory.join("relay.sqlite"),
                ..Storage::default()
            },
            ..Config::default()
        };
        config.telegram.allowed_chat_ids = vec![-101, -102];
        config.telegram.allowed_user_ids = vec![7];
        Self(config)
    }

    fn open(&self) -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(
            Database::open(&self.0.storage, &self.0.limits).unwrap(),
        ))
    }

    fn sql(&self) -> Connection {
        Connection::open(&self.0.storage.path).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    fn seed(&self, db: &Arc<Mutex<Database>>, title: &str) -> String {
        let mut db = db.lock().unwrap();
        let key = db
            .register_session(None, None, title, 100)
            .unwrap()
            .session_key;
        let route = db.reserve_route(&key).unwrap();
        db.persist_route(route, -101, 5, RouteKind::Text, 100, 100)
            .unwrap();
        key
    }

    fn ingest(&self, db: &Arc<Mutex<Database>>, id: i64, target: Value) -> IngestOutcome {
        self.ingest_message(
            db,
            id,
            json!({"message_id":id + 100,"date":1,
            "chat":{"id":-101},"from":{"id":7,"is_bot":false},
            "reply_to_message":target,"text":"Synthetic reply"}),
        )
    }

    fn ingest_message(&self, db: &Arc<Mutex<Database>>, id: i64, message: Value) -> IngestOutcome {
        IngressService::new(&self.0)
            .unwrap()
            .ingest(
                &mut db.lock().unwrap(),
                &serde_json::from_value(json!({"update_id":id,"message":message})).unwrap(),
                1_000,
            )
            .unwrap()
    }

    fn service(&self, db: Arc<Mutex<Database>>, client: TelegramClient) -> Arc<NoticeService> {
        let mut service = NoticeService::new(db, client, &self.0).unwrap();
        service.now = || Ok(61_000);
        Arc::new(service)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.storage.path.parent().unwrap()).unwrap();
    }
}

type Request = (Value, oneshot::Sender<(u16, String)>);

struct Mock {
    client: TelegramClient,
    requests: mpsc::Receiver<Request>,
    task: JoinHandle<()>,
}

impl Mock {
    async fn new(config: &Config) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TelegramClient::for_test(
            config,
            format!("http://{}/botSynthetic/", listener.local_addr().unwrap()),
        )
        .unwrap();
        let (seen, requests) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let seen = seen.clone();
                        connections.spawn(async move {
                            let mut bytes = Vec::new();
                            let payload = loop {
                                let mut chunk = [0; 1_024];
                                let count = socket.read(&mut chunk).await.unwrap();
                                if count == 0 { return; }
                                bytes.extend_from_slice(&chunk[..count]);
                                assert!(bytes.len() <= 16_384);
                                if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                                    let header = std::str::from_utf8(&bytes[..end]).unwrap();
                                    assert!(header.starts_with("POST /botSynthetic/sendMessage HTTP/1.1"));
                                    let length: usize = header.lines().find_map(|line| {
                                        let (key, value) = line.split_once(':')?;
                                        key.eq_ignore_ascii_case("content-length")
                                            .then(|| value.trim().parse().unwrap())
                                    }).unwrap();
                                    if bytes.len() == end + 4 + length {
                                        break serde_json::from_slice(&bytes[end + 4..]).unwrap();
                                    }
                                }
                            };
                            let (send, receive) = oneshot::channel::<(u16, String)>();
                            seen.send((payload, send)).await.unwrap();
                            if let Ok((status, body)) = receive.await {
                                let response = format!("HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                                let _ = socket.write_all(response.as_bytes()).await;
                            }
                        });
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => {
                        result.unwrap();
                    }
                }
            }
        });
        Self {
            client,
            requests,
            task,
        }
    }

    async fn answer(&mut self, status: u16, body: String) -> Value {
        let (request, response) = self.requests.recv().await.unwrap();
        response.send((status, body)).unwrap();
        request
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn sent(id: i64) -> String {
    json!({"ok":true,"result":{"message_id":id,"date":61,"chat":{"id":-101}}}).to_string()
}

fn dispatch(service: &Arc<NoticeService>) -> JoinHandle<Result<(), AppError>> {
    let service = service.clone();
    tokio::spawn(async move { service.dispatch_one(61_000).await })
}

#[tokio::test]
async fn factual_notices_stay_outside_routes_and_known_targets_are_silent_after_restart() {
    let fixture = Fixture::new();
    let db = fixture.open();
    let key = fixture.seed(&db, " Same\n title ");
    fixture.ingest(&db, 11, json!({"message_id":5}));
    fixture.ingest(&db, 12, json!({"message_id":6}));
    let mut mock = Mock::new(&fixture.0).await;
    let service = fixture.service(db.clone(), mock.client.clone());
    for (id, expected) in [
        (201, format!("[Same title]\n{UNCONFIRMED_NOTICE}")),
        (202, UNROUTABLE_NOTICE.to_owned()),
    ] {
        let work = dispatch(&service);
        let request = mock.answer(200, sent(id)).await;
        assert_eq!(request, json!({"chat_id":-101,"text":expected}));
        assert_eq!(work.await.unwrap(), Ok(()));
    }
    assert_eq!(fixture.count("message_routes"), 1);
    assert_eq!(fixture.count("notice_attempts"), 2);
    assert_eq!(
        fixture
            .sql()
            .query_row(
                "SELECT count(*) FROM notice_attempts WHERE notice_message_id IN (201, 202)",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        db.lock().unwrap().session(&key).unwrap().last_activity_at,
        1_000
    );
    drop(service);
    drop(db);
    let db = fixture.open();
    // No quoted sender/text is required for retained successful composite IDs.
    for (id, parent) in [(13, 201), (14, 202)] {
        assert_eq!(
            fixture.ingest(&db, id, json!({"message_id":parent})),
            IngestOutcome::Ignored
        );
    }
    assert_eq!(db.lock().unwrap().update_cursor().unwrap(), Some(14));
    assert_eq!(fixture.count("replies"), 1);
    assert_eq!(fixture.count("update_dispositions"), 2); // One unroutable and one compact ignore.
    assert!(
        db.lock()
            .unwrap()
            .take_notice_attempts(&service_policy(&fixture), 100, 700_000)
            .unwrap()
            .is_empty()
    );
    // The same ID in another chat is unknown, never mistaken for a service target.
    assert_eq!(
        fixture.ingest_message(
            &db,
            15,
            json!({"message_id":115,"date":1,
        "chat":{"id":-102},"from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":201},"text":"Synthetic"})
        ),
        IngestOutcome::Unroutable
    );
}

fn service_policy(fixture: &Fixture) -> NoticePolicy {
    NoticePolicy::new(&fixture.0.delivery, &fixture.0.notifications).unwrap()
}

#[tokio::test]
async fn uncertainty_rejection_and_failed_success_bookkeeping_never_replay() {
    for failure in 0..3 {
        let fixture = Fixture::new();
        let db = fixture.open();
        fixture.ingest(&db, 11, json!({"message_id":6}));
        if failure == 1 {
            fixture
                .sql()
                .execute_batch(
                    "CREATE TRIGGER fail_notice_id BEFORE UPDATE OF notice_message_id
                ON notice_attempts BEGIN SELECT RAISE(ABORT, 'private message'); END;",
                )
                .unwrap();
        }
        let mut mock = Mock::new(&fixture.0).await;
        let service = fixture.service(db.clone(), mock.client.clone());
        let work = dispatch(&service);
        let (status, body) = match failure {
            0 => (200, "lost response".to_owned()),
            1 => (200, sent(201)),
            _ => (
                400,
                json!({"ok":false,"error_code":400,"description":"private message"}).to_string(),
            ),
        };
        let request = mock.answer(status, body).await;
        let error = work.await.unwrap().unwrap_err();
        assert_eq!(
            error.code(),
            [
                "TELEGRAM_DELIVERY_UNCERTAIN",
                "STORAGE_FAILED",
                "TELEGRAM_REJECTED"
            ][failure]
        );
        assert!(!format!("{error} {error:?}").contains("private message"));
        assert_eq!(fixture.count("message_routes"), 0);
        assert_eq!(
            fixture
                .sql()
                .query_row("SELECT notice_message_id FROM notice_attempts", [], |r| {
                    r.get::<_, Option<i64>>(0)
                })
                .unwrap(),
            None
        );
        drop(service);
        drop(db);
        let db = fixture.open();
        let service = fixture.service(db.clone(), mock.client.clone());
        assert_eq!(service.dispatch_one(61_000).await, Ok(()));
        assert!(mock.requests.try_recv().is_err());
        // Telegram's quoted bot text identifies the service target despite no saved ID.
        let target = json!({"message_id":201,"from":{"id":9,"is_bot":true},"text":request["text"]});
        assert_eq!(
            fixture.ingest(&db, 12, target.clone()),
            IngestOutcome::Ignored
        );
        assert_eq!(fixture.ingest(&db, 12, target), IngestOutcome::Duplicate);
        assert_eq!(fixture.count("notice_attempts"), 1);
        assert_eq!(service.dispatch_one(700_000).await, Ok(()));
        assert!(mock.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn complete_notice_length_is_validated_without_truncation_or_replay() {
    for length in [1_000, 1_001] {
        let fixture = Fixture::new();
        let db = fixture.open();
        let title = format!(
            "a{}",
            "\u{301}".repeat(length - UNCONFIRMED_NOTICE.chars().count() - 4)
        );
        fixture.seed(&db, &title);
        fixture.ingest(&db, 11, json!({"message_id":5}));
        let mut mock = Mock::new(&fixture.0).await;
        let service = fixture.service(db, mock.client.clone());
        if length == 1_000 {
            let work = dispatch(&service);
            let request = mock.answer(200, sent(201)).await;
            assert_eq!(request["text"].as_str().unwrap().chars().count(), length);
            assert!(
                request["text"]
                    .as_str()
                    .unwrap()
                    .starts_with(&format!("[{title}]\n"))
            );
            assert_eq!(work.await.unwrap(), Ok(()));
        } else {
            assert!(matches!(
                service.dispatch_one(61_000).await,
                Err(AppError::TextTooLong(_))
            ));
        }
        assert_eq!(service.dispatch_one(700_000).await, Ok(()));
        assert!(mock.requests.try_recv().is_err());
        assert_eq!(fixture.count("notice_attempts"), 1);
    }
}

#[tokio::test]
async fn ordinary_routes_win_and_untrusted_or_unrelated_wording_is_not_a_service_target() {
    let fixture = Fixture::new();
    let db = fixture.open();
    fixture.seed(&db, "Same");
    let target =
        |id, bot, text: &str| json!({"message_id":id,"from":{"id":9,"is_bot":bot},"text":text});
    assert!(matches!(
        fixture.ingest(&db, 11, target(5, true, UNROUTABLE_NOTICE)),
        IngestOutcome::ReplyStored { .. }
    ));
    assert_eq!(
        fixture.ingest(&db, 12, target(6, false, UNROUTABLE_NOTICE)),
        IngestOutcome::Unroutable
    );
    assert_eq!(
        fixture.ingest(&db, 13, target(7, true, "Ordinary bot message")),
        IngestOutcome::Unroutable
    );
    assert_eq!(
        fixture.ingest(
            &db,
            14,
            target(8, true, &format!("[Old title]\n{UNCONFIRMED_NOTICE}"))
        ),
        IngestOutcome::Ignored
    );
    // The wording alone must not discard a Reply before an ordinary send's route commits.
    let key = db
        .lock()
        .unwrap()
        .register_session(None, None, "Other", 100)
        .unwrap()
        .session_key;
    let pending = db.lock().unwrap().reserve_route(&key).unwrap();
    let update = serde_json::from_value(json!({"update_id":15,"message":{
        "message_id":115,"date":1,"chat":{"id":-101},"from":{"id":7,"is_bot":false},
        "reply_to_message":target(9, true, UNROUTABLE_NOTICE),"text":"Synthetic"}}))
    .unwrap();
    let ingress = IngressService::new(&fixture.0).unwrap();
    assert_eq!(
        ingress.ingest(&mut db.lock().unwrap(), &update, 1_000),
        Err(AppError::RoutePending)
    );
    assert_eq!(db.lock().unwrap().update_cursor().unwrap(), Some(14));
    db.lock()
        .unwrap()
        .persist_route(pending, -101, 9, RouteKind::Text, 100, 1_000)
        .unwrap();
    assert!(
        matches!(ingress.ingest(&mut db.lock().unwrap(), &update, 1_000),
        Ok(IngestOutcome::ReplyStored { session_key, .. }) if session_key == key)
    );
    assert_eq!(
        fixture.ingest(&db, 16, target(10, true, UNROUTABLE_NOTICE)),
        IngestOutcome::Ignored
    );
}

#[tokio::test]
async fn confirmed_long_work_completion_disabled_categories_and_unapproved_input_are_silent() {
    let mut fixture = Fixture::new();
    let db = fixture.open();
    let key = fixture.seed(&db, "Same");
    for id in 11..=13 {
        fixture.ingest(&db, id, json!({"message_id":5}));
    }
    let inbox = InboxService::new(&fixture.0.delivery).unwrap();
    {
        let mut database = db.lock().unwrap();
        let claims = inbox.claim(&mut database, &key, None, 3, 1_001).unwrap();
        for claim in &claims[..2] {
            inbox
                .processing(
                    &mut database,
                    &key,
                    &claim.reply.reply_id,
                    &claim.claim_token,
                    true,
                    1_002,
                )
                .unwrap();
        }
        inbox
            .processing(
                &mut database,
                &key,
                &claims[1].reply.reply_id,
                &claims[1].claim_token,
                false,
                700_000,
            )
            .unwrap();
        inbox
            .acknowledge(
                &mut database,
                &key,
                &claims[2].reply.reply_id,
                &claims[2].claim_token,
                1_002,
            )
            .unwrap();
    }
    for (id, chat, user, bot, parent) in [
        (14, -103, 7, false, Some(6)),
        (15, -101, 8, false, Some(6)),
        (16, -101, 7, true, Some(6)),
        (17, -101, 7, false, None),
    ] {
        assert_eq!(
            fixture.ingest_message(
                &db,
                id,
                json!({"message_id":id + 100,"date":1,
            "chat":{"id":chat},"from":{"id":user,"is_bot":bot},
            "reply_to_message":parent.map(|id| json!({"message_id":id})),"text":"/unsupported"})
            ),
            IngestOutcome::Ignored
        );
    }
    let mock = Mock::new(&fixture.0).await;
    let service = fixture.service(db.clone(), mock.client.clone());
    assert_eq!(service.dispatch_one(700_000).await, Ok(()));
    assert_eq!(fixture.count("notice_attempts"), 0);
    fixture.0.notifications.notify_unconfirmed_processing = false;
    fixture.0.notifications.notify_unroutable = false;
    fixture.ingest(&db, 18, json!({"message_id":5}));
    fixture.ingest(&db, 19, json!({"message_id":6}));
    let service = fixture.service(db, mock.client.clone());
    assert_eq!(service.dispatch_one(700_000).await, Ok(()));
    assert_eq!(fixture.count("notice_attempts"), 0);
}

#[tokio::test]
async fn revoked_chat_authorization_and_failed_attempt_admission_have_no_network_effects() {
    let mut fixture = Fixture::new();
    let db = fixture.open();
    fixture.ingest(&db, 11, json!({"message_id":6}));
    fixture
        .sql()
        .execute_batch(
            "CREATE TRIGGER fail_attempt BEFORE INSERT ON notice_attempts
        BEGIN SELECT RAISE(ABORT, 'private detail'); END;",
        )
        .unwrap();
    let mut mock = Mock::new(&fixture.0).await;
    let service = fixture.service(db.clone(), mock.client.clone());
    assert_eq!(
        service.dispatch_one(61_000).await,
        Err(AppError::StorageFailure)
    );
    assert_eq!(fixture.count("notice_attempts"), 0);
    assert_eq!(db.lock().unwrap().update_cursor().unwrap(), Some(11));
    fixture
        .sql()
        .execute_batch("DROP TRIGGER fail_attempt")
        .unwrap();
    fixture.0.telegram.allowed_chat_ids = vec![-102];
    let service = fixture.service(db, mock.client.clone());
    assert_eq!(service.dispatch_one(61_000).await, Ok(()));
    assert_eq!(fixture.count("notice_attempts"), 1);
    assert!(mock.requests.try_recv().is_err());
}

#[tokio::test]
async fn scheduler_is_exclusive_bounded_and_cancellation_leaves_no_retry_after_restart() {
    let fixture = Fixture::new();
    let db = fixture.open();
    for id in [11, 12] {
        fixture.ingest(&db, id, json!({"message_id":6}));
    }
    let mut mock = Mock::new(&fixture.0).await;
    let service = fixture.service(db.clone(), mock.client.clone());
    let (stop, shutdown) = watch::channel(false);
    let worker = {
        let service = service.clone();
        tokio::spawn(async move { service.run(shutdown).await })
    };
    let (_, response) = mock.requests.recv().await.unwrap();
    let competitor = fixture.service(db.clone(), mock.client.clone());
    assert_eq!(
        competitor.run(stop.subscribe()).await,
        Err(AppError::NoticeSchedulerActive)
    );
    // Time passing cannot admit a second notice behind one stalled HTTP operation.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(fixture.count("notice_attempts"), 1);
    assert!(mock.requests.try_recv().is_err());
    stop.send(true).unwrap();
    assert_eq!(worker.await.unwrap(), Ok(()));
    tokio::time::resume();
    drop(response);
    assert_eq!(competitor.run(stop.subscribe()).await, Ok(())); // Lease released.
    drop(service);
    drop(competitor);
    drop(db);
    let db = fixture.open();
    let service = fixture.service(db, mock.client.clone());
    let work = dispatch(&service);
    mock.answer(200, sent(202)).await;
    assert_eq!(work.await.unwrap(), Ok(()));
    assert_eq!(service.dispatch_one(700_000).await, Ok(()));
    assert_eq!(fixture.count("notice_attempts"), 2);
    assert_eq!(
        fixture
            .sql()
            .query_row(
                "SELECT notice_message_id FROM notice_attempts
        WHERE telegram_update_id = 11",
                [],
                |r| r.get::<_, Option<i64>>(0)
            )
            .unwrap(),
        None
    );
}
