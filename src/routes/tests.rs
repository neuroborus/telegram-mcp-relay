use super::*;
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
};

use rmcp::{
    ErrorData, RoleServer,
    model::{DiscoverResult, ProtocolVersion, SubscriptionFilter},
    service::{RequestContext, SubscriptionContext},
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
};

use crate::repositories::Database;

#[path = "ask.rs"]
mod ask;
#[path = "attachments.rs"]
mod attachments;
#[path = "guidance.rs"]
mod guidance;
#[path = "inbox.rs"]
mod inbox;
#[path = "legacy.rs"]
mod legacy;
#[path = "mixed.rs"]
mod mixed;
#[path = "subscriptions.rs"]
mod subscriptions;
#[path = "tools.rs"]
mod tools;
#[path = "wait.rs"]
mod wait;

const VERSION: &str = "2026-07-28";
const LEGACY_VERSION: &str = "2025-06-18";
const TOKEN: &str = "synthetic-bearer-token-0123456789012345";

fn message(method: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":7, "method":method, "params":{"_meta":{
        "io.modelcontextprotocol/protocolVersion":VERSION,
        "io.modelcontextprotocol/clientCapabilities":{}}}})
}

struct Fixture {
    directory: PathBuf,
    config: Config,
    state: Arc<McpHandler>,
}

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("mcp-test-{:016x}", u64::from_ne_bytes(random)));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let mut config = Config::default();
        config.storage.path = directory.join("relay.sqlite");
        config.limits.concurrent_requests = 2;
        config.telegram.chat_id = -101;
        config.telegram.allowed_chat_ids = vec![-101];
        config.telegram.allowed_user_ids = vec![7];
        config.files.allowed_roots = vec![directory.clone()];
        let state = Arc::new(
            McpHandler::new(
                Arc::new(std::sync::Mutex::new(
                    Database::open(&config.storage, &config.limits).unwrap(),
                )),
                crate::clients::telegram::TelegramClient::for_test(
                    &config,
                    "http://127.0.0.1:1/botSynthetic/".into(),
                )
                .unwrap(),
                &config,
            )
            .unwrap(),
        );
        Self {
            directory,
            config,
            state,
        }
    }

    fn auth(&mut self) -> Secret {
        for (name, value) in [("bot", "123:synthetic"), ("auth", TOKEN)] {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(self.directory.join(name))
                .unwrap();
            std::io::Write::write_all(&mut file, value.as_bytes()).unwrap();
        }
        self.config.telegram.bot_token_file = Some(self.directory.join("bot"));
        self.config.server.auth_token_file = Some(self.directory.join("auth"));
        self.config.load_secrets().unwrap().auth_token.unwrap()
    }

    async fn serve<S: ServerHandler>(&mut self, handler: Arc<S>, auth: Option<&Secret>) -> Http {
        self.serve_endpoint(handler, auth).await.0
    }

    async fn serve_endpoint<S: ServerHandler>(
        &mut self,
        handler: Arc<S>,
        auth: Option<&Secret>,
    ) -> (Http, McpEndpoint<S>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.config.server.listen = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let endpoint = McpEndpoint::new(&self.config, auth, handler, stop.clone()).unwrap();
        let task = tokio::spawn(endpoint.clone().serve(listener));
        let http = Http {
            address: self.config.server.listen,
            stop,
            task,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            next_legacy_id: std::sync::atomic::AtomicU64::new(10),
        };
        (http, endpoint)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

struct Http {
    address: std::net::SocketAddr,
    stop: CancellationToken,
    task: JoinHandle<Result<(), AppError>>,
    client: reqwest::Client,
    next_legacy_id: std::sync::atomic::AtomicU64,
}

impl Http {
    fn discover(&self) -> reqwest::RequestBuilder {
        self.post(message("server/discover"))
    }

    fn post(&self, value: Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(format!("http://{}/mcp", self.address))
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("mcp-protocol-version", VERSION)
            .header("mcp-method", value["method"].as_str().unwrap());
        if value["method"] == "tools/call"
            && let Some(name) = value["params"]["name"].as_str()
        {
            request = request.header("mcp-name", name);
        }
        if value["method"] == "resources/read"
            && let Some(uri) = value["params"]["uri"].as_str()
        {
            request = request.header("mcp-name", uri);
        }
        request.json(&value)
    }

    async fn finish(&mut self) {
        self.stop.cancel();
        tokio::time::timeout(Duration::from_secs(2), &mut self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

async fn status(request: reqwest::RequestBuilder) -> StatusCode {
    request.send().await.unwrap().status()
}

impl Drop for Http {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}

#[tokio::test]
async fn discovery_is_modern_stateless_and_accepts_native_and_same_origin_clients() {
    let mut fixture = Fixture::new();
    let state = fixture.state.clone();
    let mut http = fixture.serve(state, None).await;
    for origin in [
        None,
        Some(format!("http://localhost:{}", http.address.port())),
    ] {
        let mut request = http.discover();
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("mcp-session-id").is_none());
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["id"], 7);
        assert_eq!(
            body["result"]["supportedVersions"],
            json!([LEGACY_VERSION, VERSION])
        );
        assert_eq!(body["result"]["cacheScope"], "private");
        assert_eq!(body["result"]["ttlMs"], 0);
        assert_eq!(
            body["result"]["capabilities"],
            json!({"tools":{},"resources":{"subscribe":true}})
        );
    }
    // The transport retires its connections without waiting for client keepalive.
    http.finish().await;
}

#[tokio::test]
async fn hostile_headers_authentication_and_paths_are_rejected() {
    let mut fixture = Fixture::new();
    let secret = fixture.auth();
    let state = fixture.state.clone();
    assert!(
        McpEndpoint::new(
            &fixture.config,
            None,
            state.clone(),
            CancellationToken::new()
        )
        .is_err()
    );
    let http = fixture.serve(state, Some(&secret)).await;
    let request = || http.discover();
    for (name, value) in [
        ("host", "attacker.invalid"),
        ("host", "localhost:1"),
        ("origin", "null"),
        ("origin", "https://attacker.invalid"),
        ("origin", "http://localhost:1"),
    ] {
        assert_eq!(
            status(request().bearer_auth(TOKEN).header(name, value)).await,
            StatusCode::FORBIDDEN,
            "{name}: {value}"
        );
    }
    for authorization in [None, Some("Bearer wrong"), Some(TOKEN)] {
        let mut request = request();
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        assert!(!response.text().await.unwrap().contains(TOKEN));
    }
    for name in [header::HOST, header::ORIGIN, header::AUTHORIZATION] {
        let mut request = request().bearer_auth(TOKEN).build().unwrap();
        let value = match name.as_str() {
            "host" => http.address.to_string(),
            "origin" => format!("http://{}", http.address),
            _ => format!("Bearer {TOKEN}"),
        };
        request
            .headers_mut()
            .append(name.clone(), value.parse().unwrap());
        request.headers_mut().append(name, value.parse().unwrap());
        let response = http.client.execute(request).await.unwrap();
        assert!(response.status().is_client_error());
    }
    assert_eq!(status(request().bearer_auth(TOKEN)).await, StatusCode::OK);
    for path in ["/other", "/mcp?token=private"] {
        let request = http
            .client
            .post(format!("http://{}{path}", http.address))
            .bearer_auth(TOKEN);
        assert_eq!(status(request).await, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn protocol_errors_keep_codes_and_ids_without_echoing_parser_or_header_input() {
    let mut fixture = Fixture::new();
    let state = fixture.state.clone();
    let http = fixture.serve(state, None).await;
    let mut missing = message("server/discover");
    missing["params"] = json!({});
    let mut mismatch = message("server/discover");
    mismatch["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
        "private-header-input".into();
    let mut capabilities = message("server/discover");
    capabilities["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"] = Value::Null;
    for (body, code) in [
        (missing, -32602),
        (mismatch, -32020),
        (capabilities, -32602),
        (message("ping"), -32601),
    ] {
        let response = http.post(body).send().await.unwrap();
        assert!(response.status().is_client_error());
        let bytes = response.bytes().await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private-header-input"));
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["id"], 7);
        assert_eq!(result["error"]["code"], code);
    }
    for (name, value) in [
        ("mcp-protocol-version", None),
        ("mcp-method", None),
        ("mcp-method", Some("private-header-input")),
    ] {
        let mut request = http.discover().build().unwrap();
        if let Some(value) = value {
            request
                .headers_mut()
                .insert(name, HeaderValue::from_static(value));
        } else {
            request.headers_mut().remove(name);
        }
        let response = http.client.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let text = response.text().await.unwrap();
        assert!(!text.contains("private-header-input"));
        let result: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result["id"], 7);
        assert_eq!(result["error"]["code"], -32020);
    }
    for body in [
        "private-parser-input",
        r#"{"jsonrpc":"private-parser-input","method":"server/discover","id":7}"#,
    ] {
        let response = http.discover().body(body).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let text = response.text().await.unwrap();
        assert!(!text.contains("private-parser-input"));
    }
    let mut unsupported = message("server/discover");
    unsupported["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = "2099-01-01".into();
    let mut request = http.post(unsupported).build().unwrap();
    request.headers_mut().insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2099-01-01"),
    );
    let response = http.client.execute(request).await.unwrap();
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["error"]["code"], -32022);
    assert_eq!(
        result["error"]["data"]["supported"],
        json!([LEGACY_VERSION, VERSION])
    );
}

#[tokio::test]
async fn declared_and_chunked_bodies_enforce_the_same_inclusive_byte_bound() {
    let mut fixture = Fixture::new();
    let mut body = message("server/discover").to_string();
    fixture.config.limits.request_bytes = body.len() as u32;
    let state = fixture.state.clone();
    let http = fixture.serve(state, None).await;
    assert_eq!(
        status(http.discover().body(body.clone())).await,
        StatusCode::OK
    );
    body.push(' ');
    assert_eq!(
        status(http.discover().body(body.clone())).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let mut socket = TcpStream::connect(http.address).await.unwrap();
    let mut wire = format!(
        "POST /mcp HTTP/1.1\r\nHost: {}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nMCP-Protocol-Version: {VERSION}\r\nMcp-Method: server/discover\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        http.address
    );
    for chunk in body.as_bytes().chunks(13) {
        wire.push_str(&format!(
            "{:x}\r\n{}\r\n",
            chunk.len(),
            std::str::from_utf8(chunk).unwrap()
        ));
    }
    wire.push_str("0\r\n\r\n");
    socket.write_all(wire.as_bytes()).await.unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    assert!(response.starts_with("HTTP/1.1 413"));
}

// Test-only handlers exercise HTTP lifetimes/state without adding application tools.
struct Probe {
    state: Arc<McpHandler>,
    calls: std::sync::atomic::AtomicUsize,
    seen: mpsc::Sender<(String, http::HeaderMap, Option<ProtocolVersion>)>,
    gate: Semaphore,
    closed: CancellationToken,
    listening: tokio::sync::Notify,
}

impl ServerHandler for Probe {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        self.state.get_info()
    }
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        self.state.supported_protocol_versions()
    }
    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        let parts = context.extensions.get::<http::request::Parts>().unwrap();
        let id = parts
            .extensions
            .get::<RequestCorrelation>()
            .unwrap()
            .as_str()
            .to_owned();
        self.seen
            .send((id, parts.headers.clone(), context.protocol_version()))
            .await
            .unwrap();
        self.gate.acquire().await.unwrap().forget();
        let mut result = DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        );
        result.instructions = Some(
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .to_string(),
        );
        Ok(result)
    }
    fn accepted_subscription_filter(
        &self,
        filter: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(filter.clone())
    }
    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let _cancelled = self.closed.clone().drop_guard();
        self.listening.notify_one();
        context.cancelled().await;
        Ok(())
    }
}

#[tokio::test]
async fn shared_factory_metadata_and_long_requests_preserve_admission_and_cancellation() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 1;
    let secret = fixture.auth();
    let (seen, mut calls) = mpsc::channel(4);
    let probe = Arc::new(Probe {
        state: fixture.state.clone(),
        calls: std::sync::atomic::AtomicUsize::new(0),
        seen,
        gate: Semaphore::new(0),
        closed: CancellationToken::new(),
        listening: tokio::sync::Notify::new(),
    });
    let mut http = fixture.serve(probe.clone(), Some(&secret)).await;
    let request = http
        .discover()
        .bearer_auth(TOKEN)
        .header(header::COOKIE, "private-cookie")
        .header("x-request-id", "private-client-id");
    let pending = tokio::spawn(request.send());
    let (id, headers, version) = calls.recv().await.unwrap();
    assert_eq!(id.len(), 32);
    assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(version, Some(ProtocolVersion::V_2026_07_28));
    for name in ["authorization", "cookie", "x-request-id"] {
        assert!(!headers.contains_key(name));
    }
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(601)).await;
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    tokio::time::resume();
    assert_eq!(
        status(http.discover().bearer_auth(TOKEN)).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    probe.gate.add_permits(1);
    let response = pending.await.unwrap().unwrap();
    assert_eq!(response.headers()["x-request-id"], id);
    assert_eq!(
        response.json::<Value>().await.unwrap()["result"]["instructions"],
        "0"
    );
    probe.gate.add_permits(1);
    let response = http.discover().bearer_auth(TOKEN).send().await.unwrap();
    let (next_id, _, _) = calls.recv().await.unwrap();
    assert_ne!(next_id, id);
    assert_eq!(
        response.json::<Value>().await.unwrap()["result"]["instructions"],
        "1"
    );

    // SDK acknowledgment opens an SSE body. Holding it holds admission through idle time.
    let mut subscription = message("subscriptions/listen");
    subscription["params"]["notifications"] = json!({});
    let mut stream = http
        .post(subscription)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    probe.listening.notified().await;
    assert_eq!(stream.headers()[header::CONTENT_TYPE], "text/event-stream");
    assert!(!stream.chunk().await.unwrap().unwrap().is_empty());
    assert_eq!(
        status(http.discover().bearer_auth(TOKEN)).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(stream);
    probe.closed.cancelled().await;
    probe.gate.add_permits(1);
    assert_eq!(
        status(http.discover().bearer_auth(TOKEN)).await,
        StatusCode::OK
    );
    http.finish().await;
}

#[tokio::test]
async fn shutdown_cancels_an_admitted_incomplete_body() {
    let fixture = Fixture::new();
    for session in [None, Some("synthetic-session")] {
        let stop = CancellationToken::new();
        let endpoint =
            McpEndpoint::new(&fixture.config, None, fixture.state.clone(), stop.clone()).unwrap();
        let reading = Arc::new(tokio::sync::Notify::new());
        let signal = reading.clone();
        let body = http_body_util::StreamBody::new(futures_util::stream::poll_fn(move |_| {
            signal.notify_one();
            Poll::<Option<Result<Frame<Bytes>, Infallible>>>::Pending
        }));
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, fixture.config.server.listen.to_string())
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json");
        request = if let Some(session) = session {
            request
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", LEGACY_VERSION)
        } else {
            request
                .header("mcp-protocol-version", VERSION)
                .header("mcp-method", "server/discover")
        };
        let request = request.body(body).unwrap();
        let pending = tokio::spawn(async move { endpoint.handle(request).await });
        reading.notified().await;
        stop.cancel();
        assert_eq!(
            pending.await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
