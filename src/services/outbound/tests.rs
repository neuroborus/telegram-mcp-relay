use super::*;
use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf};

use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{config::Storage, repositories::Session};

enum Response {
    Json(u16, Value),
    Held(oneshot::Receiver<()>, Value),
    Disconnect,
}

fn sent(message_id: i64) -> Value {
    json!({"ok":true,"result":{"message_id":message_id,"date":100,"chat":{"id":-101}}})
}

struct Fixture {
    directory: PathBuf,
    config: Config,
}

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("outbound-test-{:016x}", u64::from_ne_bytes(random)));
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
        config.telegram.chat_id = -101;
        config.telegram.allowed_chat_ids = vec![-101];
        config.limits.background_tasks = 1;
        config.limits.routes = 2;
        Self { directory, config }
    }

    fn open(&self) -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(
            Database::open(&self.config.storage, &self.config.limits).unwrap(),
        ))
    }

    fn sql(&self) -> Connection {
        Connection::open(&self.config.storage.path).unwrap()
    }

    fn register(&self, database: &Arc<Mutex<Database>>) -> Session {
        database
            .lock()
            .unwrap()
            .register_session(None, None, " 界\n ", 50_000)
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

struct Mock {
    client: TelegramClient,
    requests: Arc<Mutex<Vec<Value>>>,
    seen: mpsc::Receiver<()>,
    task: JoinHandle<()>,
}

impl Mock {
    async fn new(config: &Config, responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TelegramClient::for_test(
            config,
            format!("http://{}/botSynthetic/", listener.local_addr().unwrap()),
        )
        .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let (seen, received) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            let mut responses = responses.into_iter();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let payload = loop {
                    let mut chunk = [0; 1_024];
                    let read = socket.read(&mut chunk).await.unwrap();
                    assert!(read > 0 && request.len() + read <= 16_384);
                    request.extend_from_slice(&chunk[..read]);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let header = std::str::from_utf8(&request[..end]).unwrap();
                        assert!(header.starts_with("POST /botSynthetic/sendMessage HTTP/1.1\r\n"));
                        let length: usize = header
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        if request.len() == end + 4 + length {
                            break serde_json::from_slice(&request[end + 4..]).unwrap();
                        }
                    }
                };
                {
                    let mut requests = recorded.lock().unwrap();
                    assert!(requests.len() < 16);
                    requests.push(payload);
                }
                seen.send(()).await.unwrap();
                let (status, body) = match responses.next().unwrap_or(Response::Disconnect) {
                    Response::Json(status, body) => (status, body),
                    Response::Held(release, body) => {
                        release.await.unwrap();
                        (200, body)
                    }
                    Response::Disconnect => continue,
                };
                let body = body.to_string();
                socket.write_all(format!("HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        Self {
            client,
            requests,
            seen: received,
            task,
        }
    }

    fn service(&self, fixture: &Fixture, database: &Arc<Mutex<Database>>) -> OutboundService {
        let mut service =
            OutboundService::new(database.clone(), self.client.clone(), &fixture.config).unwrap();
        service.now = || Ok(200_000);
        service
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn validates_before_effects_and_persists_exact_formatted_boundaries_across_restart() {
    let fixture = Fixture::new();
    let database = fixture.open();
    let session = fixture.register(&database);
    let mock = Mock::new(
        &fixture.config,
        vec![Response::Json(200, sent(7)), Response::Json(200, sent(8))],
    )
    .await;
    let service = mock.service(&fixture, &database);
    assert_eq!(
        service
            .send_text("unknown", "private body")
            .await
            .unwrap_err(),
        AppError::SessionNotFound
    );
    for body in ["a".repeat(997), "😀".repeat(997)] {
        let error = service
            .send_text(&session.session_key, &body)
            .await
            .unwrap_err();
        let AppError::TextTooLong(details) = &error else {
            panic!("{error}")
        };
        assert_eq!((details.limit, details.actual_characters), (1_000, 1_001));
        assert!(details.guidance.contains("telegram_send_file"));
        assert!(!format!("{error:?} {error}").contains(&body));
    }
    assert_eq!(mock.count(), 0);
    assert_eq!(
        database
            .lock()
            .unwrap()
            .session(&session.session_key)
            .unwrap(),
        session
    );
    assert!(
        database
            .lock()
            .unwrap()
            .route(-101, 7, 200_000)
            .unwrap()
            .is_none()
    );
    // The normalized header consumes four scalars; astral text is legal below native bounds.
    for (id, body) in [(7, "a".repeat(996)), (8, "😀".repeat(996))] {
        let result = service
            .send_text(&session.session_key, &body)
            .await
            .unwrap();
        assert_eq!(result.message_id, id);
        let requests = mock.requests.lock().unwrap();
        assert_eq!(
            requests.last().unwrap(),
            &json!({"chat_id":-101,"text":format!("[界]\n{body}")})
        );
    }
    assert_eq!(mock.count(), 2);
    drop(service);
    drop(database);
    let database = fixture.open();
    for id in [7, 8] {
        let route = database
            .lock()
            .unwrap()
            .route(-101, id, 200_000)
            .unwrap()
            .unwrap();
        assert_eq!(
            (route.session_key, route.kind, route.created_at),
            (session.session_key.clone(), RouteKind::Text, 100_000)
        );
        assert!(
            database
                .lock()
                .unwrap()
                .route(-102, id, 200_000)
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(
        database
            .lock()
            .unwrap()
            .session(&session.session_key)
            .unwrap()
            .last_activity_at,
        200_000
    );
    // Outbound bodies have no storage owner; only the two route records were added.
    assert_eq!(
        fixture
            .sql()
            .query_row("SELECT count(*) FROM message_routes", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn rejects_unapproved_destination_and_exhausted_storage_before_http() {
    for metadata_limited in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.limits.routes = 1;
        if metadata_limited {
            fixture.config.limits.metadata_bytes = 128 + 64 + "界".len() as u32;
        }
        let database = fixture.open();
        let session = fixture.register(&database);
        if !metadata_limited {
            let mut database = database.lock().unwrap();
            let reservation = database.reserve_route(&session.session_key).unwrap();
            database
                .persist_route(reservation, -101, 6, RouteKind::Text, 50_000, 50_000)
                .unwrap();
        }
        let mock = Mock::new(&fixture.config, vec![]).await;
        let service = mock.service(&fixture, &database);
        for chat_id in [0, -102, i64::MAX] {
            fixture.config.telegram.chat_id = chat_id;
            assert!(matches!(
                OutboundService::new(database.clone(), mock.client.clone(), &fixture.config),
                Err(AppError::InvalidConfig("telegram.chat_id"))
            ));
        }
        assert_eq!(
            service
                .send_text(&session.session_key, "private body")
                .await
                .unwrap_err(),
            AppError::StorageCapacity
        );
        assert_eq!(mock.count(), 0);
        assert_eq!(service.admission.available_permits(), 1);
        assert_eq!(
            database
                .lock()
                .unwrap()
                .session(&session.session_key)
                .unwrap(),
            session
        );
        assert!(
            database
                .lock()
                .unwrap()
                .route(-101, 7, 200_000)
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn definitive_rejection_releases_capacity_without_route_or_activity() {
    let mut fixture = Fixture::new();
    fixture.config.limits.routes = 1;
    let database = fixture.open();
    let session = fixture.register(&database);
    let mock = Mock::new(
        &fixture.config,
        vec![
            Response::Json(
                403,
                json!({"ok":false,"error_code":403,"description":"private body and token"}),
            ),
            Response::Json(200, sent(7)),
        ],
    )
    .await;
    let service = mock.service(&fixture, &database);
    let error = service
        .send_text(&session.session_key, "private body")
        .await
        .unwrap_err();
    assert_eq!(
        error,
        AppError::Telegram(TelegramError::Rejected {
            code: 403,
            retry_after_seconds: None
        })
    );
    assert!(!format!("{error} {error:?}").contains("private"));
    assert_eq!(mock.count(), 1);
    assert_eq!(
        database
            .lock()
            .unwrap()
            .session(&session.session_key)
            .unwrap(),
        session
    );
    assert!(
        database
            .lock()
            .unwrap()
            .route(-101, 7, 200_000)
            .unwrap()
            .is_none()
    );
    service
        .send_text(&session.session_key, "next intentional send")
        .await
        .unwrap();
    assert_eq!(mock.count(), 2);
}

#[tokio::test]
async fn lost_response_is_uncertain_without_inventing_a_route_or_resending() {
    let fixture = Fixture::new();
    let database = fixture.open();
    let session = fixture.register(&database);
    let mock = Mock::new(&fixture.config, vec![Response::Disconnect]).await;
    let service = mock.service(&fixture, &database);
    let error = service
        .send_text(&session.session_key, "private body")
        .await
        .unwrap_err();
    assert_eq!(error, AppError::Telegram(TelegramError::DeliveryUncertain));
    assert!(error.to_string().contains("do not resend automatically"));
    assert_eq!(mock.count(), 1);
    assert_eq!(
        database
            .lock()
            .unwrap()
            .session(&session.session_key)
            .unwrap(),
        session
    );
    assert!(
        database
            .lock()
            .unwrap()
            .route(-101, 7, 200_000)
            .unwrap()
            .is_none()
    );
    assert_eq!(service.admission.available_permits(), 1);
    // The failed send also released its metadata reservation.
    let mut database = database.lock().unwrap();
    let _first = database.reserve_route(&session.session_key).unwrap();
    let _second = database.reserve_route(&session.session_key).unwrap();
}

#[tokio::test]
async fn accepted_send_with_failed_transaction_is_distinct_and_never_replayed() {
    let mut fixture = Fixture::new();
    fixture.config.limits.routes = 1;
    let database = fixture.open();
    let session = fixture.register(&database);
    fixture.sql().execute_batch("CREATE TRIGGER fail_activity BEFORE UPDATE ON sessions BEGIN SELECT RAISE(ABORT, 'private failure'); END").unwrap();
    let mock = Mock::new(
        &fixture.config,
        vec![Response::Json(200, sent(7)), Response::Json(200, sent(8))],
    )
    .await;
    let service = mock.service(&fixture, &database);
    let error = service
        .send_text(&session.session_key, "private body")
        .await
        .unwrap_err();
    assert_eq!(error, AppError::SendBookkeepingFailed);
    assert_eq!(error.code(), "SEND_BOOKKEEPING_FAILED");
    assert!(error.to_string().contains("Telegram accepted"));
    assert!(error.to_string().contains("do not resend automatically"));
    assert!(!format!("{error} {error:?}").contains("private"));
    assert_eq!(mock.count(), 1);
    assert_eq!(
        database
            .lock()
            .unwrap()
            .session(&session.session_key)
            .unwrap(),
        session
    );
    assert!(
        database
            .lock()
            .unwrap()
            .route(-101, 7, 200_000)
            .unwrap()
            .is_none()
    );
    fixture
        .sql()
        .execute_batch("DROP TRIGGER fail_activity")
        .unwrap();
    service
        .send_text(&session.session_key, "next intentional send")
        .await
        .unwrap();
    assert_eq!(mock.count(), 2);
    assert!(
        database
            .lock()
            .unwrap()
            .route(-101, 8, 200_000)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn cancellation_keeps_admitted_send_bounded_and_finishes_restart_visible_bookkeeping() {
    let mut fixture = Fixture::new();
    fixture.config.limits.routes = 1;
    let database = fixture.open();
    let session = fixture.register(&database);
    let (release, blocked) = oneshot::channel();
    let mut mock = Mock::new(&fixture.config, vec![Response::Held(blocked, sent(7))]).await;
    let service = mock.service(&fixture, &database);
    let caller_service = service.clone();
    let key = session.session_key.clone();
    let caller = tokio::spawn(async move { caller_service.send_text(&key, "private body").await });
    mock.seen.recv().await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(
        service
            .send_text(&session.session_key, "second")
            .await
            .unwrap_err(),
        AppError::SendCapacity
    );
    assert!(matches!(
        database.lock().unwrap().reserve_route(&session.session_key),
        Err(AppError::StorageCapacity)
    ));
    assert_eq!(
        database
            .lock()
            .unwrap()
            .session(&session.session_key)
            .unwrap(),
        session
    );
    release.send(()).unwrap();
    // Waiting on admission observes completion of the detached task without polling/sleeps.
    let idle = service.admission.acquire().await.unwrap();
    assert_eq!(mock.count(), 1);
    assert!(
        database
            .lock()
            .unwrap()
            .route(-101, 7, 200_000)
            .unwrap()
            .is_some()
    );
    drop(idle);
    drop(service);
    drop(database);
    let database = fixture.open();
    let database = database.lock().unwrap();
    assert_eq!(
        database
            .route(-101, 7, 200_000)
            .unwrap()
            .unwrap()
            .session_key,
        session.session_key
    );
    assert_eq!(
        database
            .session(&session.session_key)
            .unwrap()
            .last_activity_at,
        200_000
    );
}
