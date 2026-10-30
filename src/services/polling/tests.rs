use super::*;
use std::{fs, os::unix::fs::DirBuilderExt, path::PathBuf};

use futures_util::poll;
use rusqlite::Connection;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};

use crate::{config::Storage, repositories::RouteKind, services::outbound::OutboundService};

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
            .join(format!("poll-test-{:016x}", u64::from_ne_bytes(random)));
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
        config.telegram.allowed_user_ids = vec![7];
        Self { directory, config }
    }

    fn open(&self) -> Arc<Mutex<Database>> {
        Arc::new(Mutex::new(
            Database::open(&self.config.storage, &self.config.limits).unwrap(),
        ))
    }

    fn seed(&self, database: &Arc<Mutex<Database>>) -> String {
        let mut db = database.lock().unwrap();
        let now = unix_millis().unwrap();
        let key = db
            .register_session(None, None, "Synthetic", now)
            .unwrap()
            .session_key;
        let reservation = db.reserve_route(&key).unwrap();
        db.persist_route(reservation, -101, 5, RouteKind::Text, now, now)
            .unwrap();
        key
    }

    fn sql(&self) -> Connection {
        Connection::open(&self.config.storage.path).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

struct Request {
    method: String,
    payload: Value,
    response: oneshot::Sender<(u16, Value)>,
    closed: oneshot::Receiver<()>,
}

impl Request {
    fn respond(self, value: Value) {
        self.response.send((200, value)).unwrap();
    }
}

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
        let (send, requests) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            let mut children = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let send = send.clone();
                        assert!(children.len() < 8);
                        children.spawn(serve(socket, send));
                    }
                    Some(result) = children.join_next(), if !children.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            client,
            requests,
            task,
        }
    }

    async fn next(&mut self, method: &str) -> Request {
        let request = self.requests.recv().await.unwrap();
        assert_eq!(request.method, method);
        request
    }

    async fn webhook(&mut self) {
        self.next("getWebhookInfo").await.respond(json!({"ok":true,
            "result":{"url":"", "pending_update_count":0}}));
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(mut socket: TcpStream, send: mpsc::Sender<Request>) {
    let mut bytes = Vec::new();
    let (method, payload) = loop {
        let mut chunk = [0; 1_024];
        let read = socket.read(&mut chunk).await.unwrap();
        if read == 0 {
            return;
        }
        assert!(bytes.len() + read <= 8_192);
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end]).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            if bytes.len() == end + 4 + length {
                break (
                    headers
                        .lines()
                        .next()
                        .unwrap()
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .strip_prefix("/botSynthetic/")
                        .unwrap()
                        .to_owned(),
                    serde_json::from_slice(&bytes[end + 4..]).unwrap(),
                );
            }
        }
    };
    let (response, receive) = oneshot::channel();
    let (closed, disconnected) = oneshot::channel();
    if send
        .send(Request {
            method,
            payload,
            response,
            closed: disconnected,
        })
        .await
        .is_err()
    {
        return;
    }
    let mut extra = [0; 1];
    tokio::select! {
        response = receive => if let Ok((status, body)) = response {
            let body = body.to_string();
            let reply = format!("HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = socket.write_all(reply.as_bytes()).await;
        },
        _ = socket.read(&mut extra) => (),
    }
    let _ = closed.send(());
}

fn reply(update: i64, message: i64, target: i64) -> Value {
    json!({"update_id":update,"message":{"message_id":message,"date":1,
        "chat":{"id":-101},"from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":target},"text":"Synthetic reply"}})
}

fn start(service: Arc<PollingService>) -> (watch::Sender<bool>, JoinHandle<Result<(), AppError>>) {
    let (shutdown, receive) = watch::channel(false);
    (
        shutdown,
        tokio::spawn(async move { service.run(receive).await }),
    )
}

#[tokio::test]
async fn ordered_batches_resume_from_committed_offsets_and_deduplicate_after_restart() {
    let fixture = Fixture::new();
    let database = fixture.open();
    fixture.seed(&database);
    let mut mock = Mock::new(&fixture.config).await;
    for round in 0..2 {
        let db = if round == 0 {
            database.clone()
        } else {
            fixture.open()
        };
        let service = Arc::new(
            PollingService::new(db.clone(), mock.client.clone(), &fixture.config).unwrap(),
        );
        let mut changes = service.inbox_changes();
        let (stop, running) = start(service);
        mock.webhook().await;
        let request = mock.next("getUpdates").await;
        assert_eq!(request.payload["timeout"], 45);
        assert_eq!(request.payload["limit"], 100);
        assert_eq!(request.payload["allowed_updates"], json!(["message"]));
        assert_eq!(
            request.payload.get("offset").and_then(Value::as_i64),
            if round == 0 { None } else { Some(13) }
        );
        let updates = if round == 0 {
            json!([reply(11, 20, 5), {"update_id":12}])
        } else {
            json!([reply(11, 20, 5), reply(13, 21, 5)])
        };
        request.respond(json!({"ok":true,"result":updates}));
        let pending = mock.next("getUpdates").await;
        assert_eq!(pending.payload["offset"], 13 + round);
        assert_eq!(
            db.lock().unwrap().update_cursor().unwrap(),
            Some(12 + round)
        );
        assert_eq!(fixture.count("replies"), 1 + round);
        changes.changed().await.unwrap();
        assert_eq!(*changes.borrow_and_update(), 1); // Duplicates/ignored input never signal.
        stop.send(true).unwrap();
        running.await.unwrap().unwrap();
        pending.closed.await.unwrap();
    }
}

#[tokio::test]
async fn shared_lease_prevents_competing_consumers_and_cancellation_releases_it() {
    let fixture = Fixture::new();
    let database = fixture.open();
    let mut mock = Mock::new(&fixture.config).await;
    let client = mock.client.clone();
    let make = || {
        Arc::new(PollingService::new(database.clone(), client.clone(), &fixture.config).unwrap())
    };
    let (_stop, running) = start(make());
    mock.webhook().await;
    let pending = mock.next("getUpdates").await;
    let (_unused, receive) = watch::channel(false);
    assert_eq!(make().run(receive).await, Err(AppError::PollerActive));
    assert!(mock.requests.try_recv().is_err());
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    pending.closed.await.unwrap();
    let (stop, running) = start(Arc::new(
        PollingService::new(database, mock.client.clone(), &fixture.config).unwrap(),
    ));
    mock.webhook().await;
    let pending = mock.next("getUpdates").await;
    drop(stop); // Loss of the supervisor also retires the poller.
    running.await.unwrap().unwrap();
    pending.closed.await.unwrap();
}

#[tokio::test]
async fn capacity_pressure_retains_the_batch_without_confirming_or_skipping_input() {
    for recover in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.limits.inbox_items = 1;
        let database = fixture.open();
        fixture.seed(&database);
        let mut mock = Mock::new(&fixture.config).await;
        let service = Arc::new(
            PollingService::new(database.clone(), mock.client.clone(), &fixture.config).unwrap(),
        );
        let mut changes = service.inbox_changes();
        let (stop, running) = start(service);
        mock.webhook().await;
        mock.next("getUpdates").await.respond(json!({"ok":true,
        "result":[reply(11, 20, 5), reply(12, 21, 5), {"update_id":13}]}));
        changes.changed().await.unwrap(); // The task then reaches its local pressure backoff.
        assert_eq!(database.lock().unwrap().update_cursor().unwrap(), Some(11));
        assert_eq!(fixture.count("replies"), 1);
        assert!(mock.requests.try_recv().is_err());
        let pending = if recover {
            // Simulate later completion releasing capacity; this step has no ack service yet.
            fixture
                .sql()
                .execute("DELETE FROM replies WHERE telegram_update_id = 11", [])
                .unwrap();
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::time::resume();
            let pending = mock.next("getUpdates").await;
            assert_eq!(pending.payload["offset"], 14);
            assert_eq!(
                fixture
                    .sql()
                    .query_row("SELECT telegram_update_id FROM replies", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                12
            );
            Some(pending)
        } else {
            None
        };
        stop.send(true).unwrap();
        running.await.unwrap().unwrap();
        if let Some(pending) = pending {
            pending.closed.await.unwrap();
        }
        // A restarted poll must confirm only the resolved prefix, not the held suffix.
        let (stop, running) = start(Arc::new(
            PollingService::new(fixture.open(), mock.client.clone(), &fixture.config).unwrap(),
        ));
        mock.webhook().await;
        let pending = mock.next("getUpdates").await;
        assert_eq!(pending.payload["offset"], if recover { 14 } else { 12 });
        stop.send(true).unwrap();
        running.await.unwrap().unwrap();
        pending.closed.await.unwrap();
    }
}

#[tokio::test]
async fn reply_waits_for_detached_send_bookkeeping_or_definitive_rejection() {
    for success in [true, false] {
        let fixture = Fixture::new();
        let database = fixture.open();
        let key = fixture.seed(&database);
        let mut mock = Mock::new(&fixture.config).await;
        let outbound =
            OutboundService::new(database.clone(), mock.client.clone(), &fixture.config).unwrap();
        let caller =
            tokio::spawn(async move { outbound.send_text(&key, "Synthetic question").await });
        let send = mock.next("sendMessage").await;
        let service = Arc::new(
            PollingService::new(database.clone(), mock.client.clone(), &fixture.config).unwrap(),
        );
        let mut changes = service.inbox_changes();
        let (stop, running) = start(service);
        mock.webhook().await;
        mock.next("getUpdates").await.respond(json!({"ok":true,
            "result":[reply(11, 20, 5), reply(12, 21, 7)]}));
        changes.changed().await.unwrap();
        assert_eq!(database.lock().unwrap().update_cursor().unwrap(), Some(11));
        assert_eq!(fixture.count("update_dispositions"), 0);
        assert!(mock.requests.try_recv().is_err());
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        send.respond(if success {
            json!({"ok":true,"result":{"message_id":7,"date":unix_millis().unwrap()/1_000,"chat":{"id":-101}}})
        } else { json!({"ok":false,"error_code":400,"description":"private rejection"}) });
        let pending = mock.next("getUpdates").await;
        assert_eq!(pending.payload["offset"], 13);
        assert_eq!(fixture.count("replies"), if success { 2 } else { 1 });
        assert_eq!(
            fixture.count("update_dispositions"),
            if success { 0 } else { 1 }
        );
        if success {
            changes.changed().await.unwrap();
        }
        stop.send(true).unwrap();
        running.await.unwrap().unwrap();
        pending.closed.await.unwrap();
    }
}

#[tokio::test]
async fn newer_sends_cannot_extend_unknown_reply_deferral_or_block_a_following_reply() {
    let fixture = Fixture::new();
    let database = fixture.open();
    let key = fixture.seed(&database);
    let mut mock = Mock::new(&fixture.config).await;
    let outbound =
        OutboundService::new(database.clone(), mock.client.clone(), &fixture.config).unwrap();
    let earlier = outbound.clone();
    let earlier_key = key.clone();
    let earlier_caller =
        tokio::spawn(async move { earlier.send_text(&earlier_key, "Earlier send").await });
    let earlier_send = mock.next("sendMessage").await;
    let service = Arc::new(
        PollingService::new(database.clone(), mock.client.clone(), &fixture.config).unwrap(),
    );
    let mut changes = service.inbox_changes();
    let (stop, running) = start(service);
    mock.webhook().await;
    mock.next("getUpdates").await.respond(json!({"ok":true,
        "result":[reply(11, 20, 5), reply(12, 21, 777), reply(13, 22, 5)]}));
    changes.changed().await.unwrap(); // Receipt and its admission snapshot precede this commit.
    assert_eq!(database.lock().unwrap().update_cursor().unwrap(), Some(11));
    assert_eq!(fixture.count("update_dispositions"), 0);

    let newer_caller = tokio::spawn(async move { outbound.send_text(&key, "Newer send").await });
    let newer_send = mock.next("sendMessage").await; // Keep a later admission outstanding.
    earlier_send.respond(json!({"ok":false,"error_code":400}));
    assert!(matches!(
        earlier_caller.await.unwrap(),
        Err(AppError::Telegram(TelegramError::Rejected {
            code: 400,
            ..
        }))
    ));
    let pending = mock.next("getUpdates").await;
    assert_eq!(pending.payload["offset"], 14);
    assert!(!newer_caller.is_finished());
    assert!(!newer_send.response.is_closed());
    assert_eq!(fixture.count("update_dispositions"), 1);
    assert_eq!(fixture.count("replies"), 2);
    let accepted: i64 = fixture
        .sql()
        .query_row(
            "SELECT count(*) FROM replies WHERE telegram_update_id = 13",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(accepted, 1);

    newer_send.respond(json!({"ok":true,"result":{"message_id":8,
        "date":unix_millis().unwrap()/1_000,"chat":{"id":-101}}}));
    newer_caller.await.unwrap().unwrap();
    stop.send(true).unwrap();
    running.await.unwrap().unwrap();
    pending.closed.await.unwrap();
}

#[tokio::test]
async fn webhook_and_poll_conflicts_are_terminal_without_deleting_anything() {
    for webhook in [true, false] {
        let fixture = Fixture::new();
        let database = fixture.open();
        let mut mock = Mock::new(&fixture.config).await;
        let (_stop, running) = start(Arc::new(
            PollingService::new(database.clone(), mock.client.clone(), &fixture.config).unwrap(),
        ));
        if webhook {
            mock.next("getWebhookInfo").await.respond(json!({"ok":true,
                "result":{"url":"https://synthetic.invalid/private", "pending_update_count":3}}));
        } else {
            mock.webhook().await;
            mock.next("getUpdates")
                .await
                .respond(json!({"ok":false,"error_code":409,"description":"private conflict"}));
        }
        let error = running.await.unwrap().unwrap_err();
        assert_eq!(
            error,
            AppError::Telegram(TelegramError::Rejected {
                code: 409,
                retry_after_seconds: None
            })
        );
        assert!(!format!("{error:?} {error}").contains("private"));
        assert_eq!(database.lock().unwrap().update_cursor().unwrap(), None);
        assert!(mock.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn malformed_batch_order_or_identity_is_rejected_before_any_commit() {
    for updates in [
        json!([{"update_id":12},{"update_id":11}]),
        json!([{"update_id":11},{"update_id":11}]),
        json!([{"update_id":11},{"update_id":-1}]),
        json!([{"update_id":i64::MAX}]),
    ] {
        let fixture = Fixture::new();
        let database = fixture.open();
        let mut mock = Mock::new(&fixture.config).await;
        let (_stop, running) = start(Arc::new(
            PollingService::new(database.clone(), mock.client.clone(), &fixture.config).unwrap(),
        ));
        mock.webhook().await;
        mock.next("getUpdates")
            .await
            .respond(json!({"ok":true,"result":updates}));
        assert_eq!(
            running.await.unwrap(),
            Err(TelegramError::InvalidResponse.into())
        );
        assert_eq!(database.lock().unwrap().update_cursor().unwrap(), None);
        assert_eq!(fixture.count("update_dispositions"), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn backoff_is_bounded_but_never_shortens_server_delay() {
    let mut failures = 0;
    for attempt in 0..40 {
        let delay = backoff(&mut failures, None);
        let base = Duration::from_millis(1_000_u64 << attempt.min(4));
        assert!(delay >= base && delay <= (base * 2).min(Duration::from_secs(30)));
    }
    assert_eq!(
        backoff(&mut failures, Some(u64::MAX)),
        Duration::from_secs(u64::MAX)
    );
    let error = TelegramError::Rejected {
        code: 429,
        retry_after_seconds: Some(61),
    }
    .into();
    let mut retry = Box::pin(retry_network(error, &mut failures));
    assert!(poll!(retry.as_mut()).is_pending());
    for _ in 0..2 {
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(poll!(retry.as_mut()).is_pending());
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(poll!(retry.as_mut()).is_ready());
}
