use super::*;
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
};

use http::{Request, Response};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use crate::{
    repositories::RouteKind,
    services::{ingress::IngressService, unix_millis},
};

mod conversations;
mod extended;
mod probe;
mod recovery;

struct Fixture {
    directory: PathBuf,
    config: Config,
    listener: Option<TcpListener>,
}

impl Fixture {
    async fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("daemon-{:016x}", u64::from_ne_bytes(random)));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = Config::default();
        config.server.listen = listener.local_addr().unwrap();
        config.storage.path = directory.join("relay.sqlite");
        config.telegram.chat_id = -101;
        config.telegram.allowed_chat_ids = vec![-101];
        config.telegram.allowed_user_ids = vec![7];
        config.files.allowed_roots = vec![directory.clone()];
        Self {
            directory,
            config,
            listener: Some(listener),
        }
    }

    fn database(&self) -> Database {
        Database::open(&self.config.storage, &self.config.limits).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        rusqlite::Connection::open(&self.config.storage.path)
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn start(&mut self, client: TelegramClient) -> Running {
        self.start_authenticated(client, None)
    }

    fn start_authenticated(&mut self, client: TelegramClient, auth: Option<&Secret>) -> Running {
        let app = Application::new(&self.config, client, auth).unwrap();
        let retired = app.stop.clone();
        let listener = self.listener.take().unwrap();
        let stop = CancellationToken::new();
        let running = tokio::spawn(app.serve(listener, stop.clone().cancelled_owned()));
        Running {
            stop,
            retired,
            task: running,
        }
    }
}

struct Running {
    stop: CancellationToken,
    retired: CancellationToken,
    task: JoinHandle<Result<(), AppError>>,
}

#[tokio::test]
async fn shutdown_and_service_failure_drain_disconnected_sends_and_preserve_ingress() {
    for service_failure in [false, true] {
        let mut fixture = Fixture::new().await;
        fixture.config.limits.background_tasks = 1;
        let now = unix_millis().unwrap();
        let key = {
            let mut db = fixture.database();
            let key = db
                .register_session(None, None, "Drain", None, now)
                .unwrap()
                .session_key;
            let reservation = db.reserve_route(&key).unwrap();
            db.persist_route(reservation, -101, 5, RouteKind::Text, now, now)
                .unwrap();
            key
        };
        let mut mock = Mock::new(&fixture.config).await;
        let address = fixture.config.server.listen;
        let mut running = fixture.start(mock.client.clone());
        mock.next("getWebhookInfo")
            .await
            .respond(json!({"ok":true,"result":{"url":"","pending_update_count":0}}));
        mock.next("getUpdates")
            .await
            .respond(json!({"ok":true,"result":[{
            "update_id":10,"message":{"message_id":15,"date":1,"chat":{"id":-101},
            "from":{"id":7,"is_bot":false},"reply_to_message":{"message_id":5},
            "text":"Recover after shutdown"}}]}));
        let poll = mock.next("getUpdates").await;
        assert_eq!(poll.payload["offset"], 11); // Accepted ingress committed before shutdown.
        let send_key = key.clone();
        let caller = tokio::spawn(async move {
            tool(
                address,
                "telegram_send",
                json!({"session_key":send_key,"text":"Result"}),
            )
            .await
        });
        let send = mock.next("sendMessage").await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        if service_failure {
            poll.respond(json!({"ok":false,"error_code":401}));
        } else {
            running.stop.cancel();
            drop(poll);
        }
        running.retired.cancelled().await; // Supervisor has fenced admission and stopped peers.
        assert!(!running.task.is_finished());
        send.respond(json!({"ok":true,"result":{
            "message_id":6,"date":now/1000,"chat":{"id":-101}}}));
        let result = (&mut running.task).await.unwrap();
        if service_failure {
            assert_eq!(
                result,
                Err(AppError::Telegram(crate::error::TelegramError::Rejected {
                    code: 401,
                    retry_after_seconds: None
                }))
            );
        } else {
            result.unwrap();
        }
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
        let db = fixture.database();
        assert_eq!(db.update_cursor().unwrap(), Some(10));
        assert_eq!(db.route(-101, 6, now).unwrap().unwrap().session_key, key);
        let inbox = db.inbox_snapshot(&key, None, 10, now).unwrap();
        assert_eq!(inbox.replies.len(), 1);
        assert_eq!(fixture.count("acknowledgment_receipts"), 0);
        assert_eq!(fixture.count("notice_attempts"), 0);
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

struct TelegramRequest {
    method: String,
    content_type: String,
    body: Vec<u8>,
    payload: Value,
    response: oneshot::Sender<Value>,
}

impl TelegramRequest {
    fn part(&self, name: &str) -> &[u8] {
        let boundary = self
            .content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let marker = format!("name=\"{name}\"");
        let start = self
            .body
            .windows(marker.len())
            .position(|bytes| bytes == marker.as_bytes())
            .unwrap();
        let part = &self.body[start..];
        let header_end = part.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
        let body = &part[header_end + 4..];
        let delimiter = format!("\r\n--{boundary}");
        let end = body
            .windows(delimiter.len())
            .position(|bytes| bytes == delimiter.as_bytes())
            .unwrap();
        &body[..end]
    }

    fn respond(self, value: Value) {
        self.response.send(value).unwrap();
    }
}

struct Mock {
    address: SocketAddr,
    client: TelegramClient,
    requests: mpsc::Receiver<TelegramRequest>,
    task: JoinHandle<()>,
}

impl Mock {
    async fn new(config: &Config) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client =
            TelegramClient::for_test(config, format!("http://{address}/botSynthetic/")).unwrap();
        let (send, requests) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept(), if connections.len() < 8 => {
                        let (socket, _) = accepted.unwrap();
                        let send = send.clone();
                        connections.spawn(async move {
                            let service = service_fn(move |request: Request<Incoming>| {
                                let send = send.clone();
                                async move {
                                    let method = request.uri().path().rsplit('/').next().unwrap().to_owned();
                                    let download = request.method() == http::Method::GET;
                                    let content_type = if download {
                                        request.headers().get(http::header::CONTENT_TYPE)
                                            .map(|value| value.to_str().unwrap().to_owned()).unwrap_or_default()
                                    } else {
                                        request.headers()[http::header::CONTENT_TYPE].to_str().unwrap().to_owned()
                                    };
                                    let body = Limited::new(request.into_body(), 8_192)
                                        .collect().await.unwrap().to_bytes();
                                    let payload = if download || method == "sendDocument" {
                                        Value::Null
                                    } else {
                                        serde_json::from_slice(&body).unwrap()
                                    };
                                    let (response, receive) = oneshot::channel();
                                    send.send(TelegramRequest { method, content_type,
                                        body: body.to_vec(), payload, response }).await.unwrap();
                                    let value = receive.await.unwrap_or(Value::Null);
                                    Ok::<_, std::convert::Infallible>(Response::new(
                                        Full::new(Bytes::from(value.to_string()))))
                                }
                            });
                            let _ = http1::Builder::new().serve_connection(TokioIo::new(socket), service).await;
                        });
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            address,
            client,
            requests,
            task,
        }
    }

    async fn next(&mut self, method: &str) -> TelegramRequest {
        let request = self.requests.recv().await.unwrap();
        assert_eq!(request.method, method);
        request
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn tool(address: SocketAddr, name: &str, arguments: Value) -> Value {
    let result = rpc(
        address,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
    .await;
    assert_ne!(result["isError"], true, "{result}");
    result["structuredContent"].clone()
}

async fn rpc(address: SocketAddr, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientCapabilities":{}});
    let name = match method {
        "tools/call" => params["name"].as_str().unwrap(),
        "resources/read" => params["uri"].as_str().unwrap(),
        _ => unreachable!("unsupported fixture method"),
    };
    let response: Value = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{address}/mcp"))
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", method)
        .header("mcp-name", name)
        .json(&json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["id"], 1);
    response["result"].clone()
}

#[tokio::test]
async fn composition_cleans_before_polling_and_shares_send_ingress_and_typing() {
    let mut fixture = Fixture::new().await;
    {
        let mut database = fixture.database();
        for _ in 0..105 {
            database
                .register_session(None, None, "Expired", None, 0)
                .unwrap();
        }
    }
    let mut mock = Mock::new(&fixture.config).await;
    let address = fixture.config.server.listen;
    let mut running = fixture.start(mock.client.clone());
    let webhook = mock.next("getWebhookInfo").await;
    assert_eq!(fixture.count("sessions"), 0); // Entire multi-batch startup sweep finished.
    webhook.respond(json!({"ok":true,"result":{"url":"", "pending_update_count":0}}));
    let poll = mock.next("getUpdates").await;
    let registration = tool(address, "telegram_register", json!({"title":"Synthetic"})).await;
    let key = registration["session_key"].as_str().unwrap();
    let send = tool(
        address,
        "telegram_send",
        json!({"session_key":key,"text":"Question"}),
    );
    tokio::pin!(send);
    let request = tokio::select! {
        request = mock.next("sendMessage") => request,
        _ = &mut send => panic!("send completed before Telegram response"),
    };
    assert_eq!(request.payload["text"], "[Synthetic]\n\nQuestion");
    request.respond(
        json!({"ok":true,"result":{"message_id":5,"chat":{"id":-101},
        "date":unix_millis().unwrap()/1000}}),
    );
    assert_eq!(send.await["outbound_message_id"], 5);
    poll.respond(json!({"ok":true,"result":[{"update_id":10,"message":{
        "message_id":15,"date":1,"chat":{"id":-101},"from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":5},"text":"Synthetic reply"}}]}));
    let held_poll = mock.next("getUpdates").await;
    assert_eq!(held_poll.payload["offset"], 11); // Cursor committed before the next poll.
    let inbox = tool(address, "telegram_inbox", json!({"session_key":key})).await;
    let reply = &inbox["replies"][0];
    assert_eq!(reply["text_or_caption"], "Synthetic reply");
    let handle =
        json!({"session_key":key,"reply_id":reply["reply_id"],"claim_token":reply["claim_token"]});
    let mut processing = handle.clone();
    processing["active"] = true.into();
    tool(address, "telegram_processing", processing).await;
    mock.next("sendChatAction")
        .await
        .respond(json!({"ok":true,"result":true}));
    tool(address, "telegram_ack", handle).await;
    running.stop.cancel();
    (&mut running.task).await.unwrap().unwrap();
    drop(held_poll);
    let database = fixture.database();
    assert_eq!(database.update_cursor().unwrap(), Some(10));
    assert_eq!(
        database
            .route(-101, 5, unix_millis().unwrap())
            .unwrap()
            .unwrap()
            .session_key,
        key
    );
    assert!(
        database
            .inbox_snapshot(key, None, 1, unix_millis().unwrap())
            .unwrap()
            .replies
            .is_empty()
    );
    assert_eq!(fixture.count("acknowledgment_receipts"), 1);
}

#[tokio::test]
async fn startup_failure_accepts_no_mcp_and_starts_no_telegram_work() {
    let mut fixture = Fixture::new().await;
    let mut mock = Mock::new(&fixture.config).await;
    let app = Application::new(&fixture.config, mock.client.clone(), None).unwrap();
    let lock = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let listener = fixture.listener.take().unwrap();
    assert_eq!(
        app.serve(listener, std::future::pending()).await,
        Err(AppError::StorageFailure)
    );
    assert!(mock.requests.try_recv().is_err());
    assert!(
        tokio::net::TcpStream::connect(fixture.config.server.listen)
            .await
            .is_err()
    );
    lock.execute_batch("ROLLBACK").unwrap();
}

#[tokio::test]
async fn recovered_notice_dispatch_and_terminal_poll_failure_are_supervised() {
    let mut fixture = Fixture::new().await;
    let now = unix_millis().unwrap();
    let key = {
        let mut db = fixture.database();
        let key = db
            .register_session(None, None, "Recovered", None, now)
            .unwrap()
            .session_key;
        let reservation = db.reserve_route(&key).unwrap();
        db.persist_route(reservation, -101, 5, RouteKind::Text, now, now)
            .unwrap();
        IngressService::new(&fixture.config)
            .unwrap()
            .ingest(
                &mut db,
                &serde_json::from_value(json!({"update_id":1,"message":{"message_id":10,"date":1,
                "chat":{"id":-101},"from":{"id":7,"is_bot":false},
                "reply_to_message":{"message_id":5},"text":"Recovered reply"}}))
                .unwrap(),
                now - 61_000,
            )
            .unwrap();
        key
    };
    let mut mock = Mock::new(&fixture.config).await;
    let mut running = fixture.start(mock.client.clone());
    let first = mock.requests.recv().await.unwrap();
    let second = mock.requests.recv().await.unwrap();
    let (webhook, notice) = if first.method == "getWebhookInfo" {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(webhook.method, "getWebhookInfo");
    assert_eq!(notice.method, "sendMessage");
    notice
        .respond(json!({"ok":true,"result":{"message_id":20,"date":now/1000,"chat":{"id":-101}}}));
    // Keep polling unresolved until the once-only notice attempt has been persisted.
    assert_eq!(fixture.count("notice_attempts"), 1);
    webhook.respond(json!({"ok":true,"result":{"url":"https://synthetic.invalid/webhook", "pending_update_count":0}}));
    assert_eq!(
        (&mut running.task).await.unwrap(),
        Err(AppError::Telegram(crate::error::TelegramError::Rejected {
            code: 409,
            retry_after_seconds: None
        }))
    );
    assert!(
        tokio::net::TcpStream::connect(fixture.config.server.listen)
            .await
            .is_err()
    );
    let db = fixture.database();
    assert_eq!(db.update_cursor().unwrap(), Some(1));
    assert_eq!(
        db.inbox_snapshot(&key, None, 1, now).unwrap().replies.len(),
        1
    );
}

#[tokio::test]
async fn production_entry_validates_config_secrets_and_binding_before_service_effects() {
    let fixture = Fixture::new().await;
    let path = fixture.directory.join("config.toml");
    fs::write(&path, "[telegram]\nchat_id = 0").unwrap();
    assert!(matches!(
        run_until(&path, std::future::pending()).await,
        Err(AppError::InvalidConfig(_))
    ));
    let token = fixture.directory.join("bot.token");
    let input = format!(
        r#"
[server]
listen = "{}"
[telegram]
bot_token_file = {:?}
chat_id = -101
allowed_chat_ids = [-101]
allowed_user_ids = [7]
[storage]
path = {:?}
[files]
allowed_roots = [{:?}]
"#,
        fixture.config.server.listen, token, fixture.config.storage.path, fixture.directory
    );
    fs::write(&path, input).unwrap();
    assert_eq!(
        run_until(&path, std::future::pending()).await,
        Err(AppError::SecretUnavailable("telegram.bot_token"))
    );
    assert!(!fixture.config.storage.path.exists());
    use std::io::Write;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(token)
        .unwrap()
        .write_all(b"123:synthetic")
        .unwrap();
    // The fixture holds this exact port, so production cannot bind or contact Telegram.
    assert_eq!(
        run_until(&path, std::future::pending()).await,
        Err(AppError::McpUnavailable)
    );
    assert!(fixture.database().schema_version().is_ok());
}
