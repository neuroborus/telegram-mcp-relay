//! Actual MCP HTTP calls through the production handler and a separate fake Bot API.

use super::*;
use crate::{clients::telegram::TelegramClient, repositories::RouteKind, services::unix_millis};
use std::{collections::VecDeque, sync::Mutex};

struct Recorded {
    method: String,
    content_type: String,
    body: Vec<u8>,
}

impl Recorded {
    fn part(&self, name: &str) -> &[u8] {
        let boundary = self
            .content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let marker = format!("name=\"{name}\"");
        let start = self
            .body
            .windows(marker.len())
            .position(|b| b == marker.as_bytes())
            .unwrap();
        let part = &self.body[start..];
        let header_end = part.windows(4).position(|b| b == b"\r\n\r\n").unwrap();
        let body = &part[header_end + 4..];
        let delimiter = format!("\r\n--{boundary}");
        let end = body
            .windows(delimiter.len())
            .position(|b| b == delimiter.as_bytes())
            .unwrap();
        &body[..end]
    }
}

pub(super) struct Telegram {
    client: TelegramClient,
    requests: Arc<Mutex<Vec<Recorded>>>,
    task: JoinHandle<()>,
}

impl Telegram {
    pub(super) fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub(super) async fn new(config: &Config, responses: Vec<(u16, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TelegramClient::for_test(
            config,
            format!("http://{}/botSynthetic/", listener.local_addr().unwrap()),
        )
        .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept(), if connections.len() < 4 => {
                        let (socket, _) = accepted.unwrap();
                        let recorded = recorded.clone();
                        let responses = responses.clone();
                        connections.spawn(async move {
                            let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                                let recorded = recorded.clone();
                                let responses = responses.clone();
                                async move {
                                    let (parts, body) = request.into_parts();
                                    let body = Limited::new(body, 32_768).collect().await.unwrap().to_bytes();
                                    let mut requests = recorded.lock().unwrap();
                                    assert!(requests.len() < 16);
                                    requests.push(Recorded {
                                        method: parts.uri.path().to_owned(),
                                        content_type: parts.headers[header::CONTENT_TYPE].to_str().unwrap().to_owned(),
                                        body: body.to_vec(),
                                    });
                                    let (status, body) = responses.lock().unwrap().pop_front().unwrap();
                                    Ok::<_, Infallible>(Response::builder().status(status)
                                        .body(Full::new(Bytes::from(body.to_string()))).unwrap())
                                }
                            });
                            http1::Builder::new().serve_connection(TokioIo::new(socket), service).await.unwrap();
                        });
                    }
                    result = connections.join_next(), if !connections.is_empty() => { result.unwrap().unwrap(); }
                }
            }
        });
        Self {
            client,
            requests,
            task,
        }
    }

    pub(super) fn handler(&self, fixture: &Fixture) -> Arc<McpHandler> {
        Arc::new(
            McpHandler::new(
                Arc::new(Mutex::new(
                    Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
                )),
                self.client.clone(),
                &fixture.config,
            )
            .unwrap(),
        )
    }
}

impl Drop for Telegram {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Http {
    pub(super) async fn rpc(&self, method: &str, params: Value) -> Value {
        let mut body = message(method);
        body["params"]
            .as_object_mut()
            .unwrap()
            .extend(params.as_object().unwrap().clone());
        let response = self.post(body).send().await.unwrap();
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["id"], 7);
        body
    }

    pub(super) async fn tool(&self, name: &str, arguments: Value) -> Value {
        let body = self
            .rpc("tools/call", json!({"name":name,"arguments":arguments}))
            .await;
        assert!(body.get("error").is_none(), "{body}");
        body["result"].clone()
    }

    pub(super) async fn register(&self, arguments: Value) -> Value {
        let result = self.tool("telegram_register", arguments).await;
        assert_eq!(result["isError"], false);
        result["structuredContent"].clone()
    }
}

pub(super) fn sent(id: i64) -> (u16, Value) {
    (
        200,
        json!({"ok":true,"result":{"chat":{"id":-101},"message_id":id,"date":unix_millis().unwrap()/1000}}),
    )
}

pub(super) fn failure(result: &Value, code: &str) {
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["code"], code);
    let rendered: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(rendered, result["structuredContent"]);
}

#[tokio::test]
async fn tools_have_typed_schemas_and_registration_survives_separate_requests_and_renames() {
    let mut fixture = Fixture::new();
    let state = fixture.state.clone();
    let mut http = fixture.serve(state, None).await;
    let listing = http.rpc("tools/list", json!({})).await;
    let tools = listing["result"]["tools"].as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "telegram_register",
            "telegram_send",
            "telegram_send_file",
            "telegram_inbox",
            "telegram_processing",
            "telegram_ack",
            "telegram_ask",
            "telegram_wait"
        ]
    );
    for (tool, required) in tools.iter().zip([
        json!(["title"]),
        json!(["session_key", "text"]),
        json!(["session_key", "path"]),
        json!(["session_key"]),
        json!(["session_key", "reply_id", "claim_token"]),
        json!(["session_key", "reply_id", "claim_token"]),
        json!(["session_key", "text"]),
        json!(["session_key", "outbound_message_id"]),
    ]) {
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["inputSchema"]["required"], required);
        assert_eq!(tool["outputSchema"]["type"], "object");
    }
    let first = http
        .register(json!({"external_session_id":"thread-a","title":" Shared\n title "}))
        .await;
    let second = http
        .register(json!({"external_session_id":"thread-b","title":"Shared title"}))
        .await;
    assert_eq!(first["title"], "Shared title");
    assert_eq!(first["title"], second["title"]);
    assert_ne!(first["session_key"], second["session_key"]);
    assert_eq!(
        first["inbox_uri"],
        format!(
            "telegram-bridge://sessions/{}/inbox",
            first["session_key"].as_str().unwrap()
        )
    );
    let resumed = http
        .register(json!({"external_session_id":"thread-a","title":"Renamed"}))
        .await;
    assert_eq!(resumed["session_key"], first["session_key"]);
    let anonymous = http.register(json!({"title":""})).await;
    assert_eq!(anonymous["title"], "Agent Session");
    http.finish().await;
    // A fresh handler/HTTP endpoint must resume the same durable identity and saved handle.
    let telegram = Telegram::new(&fixture.config, vec![]).await;
    let handler = telegram.handler(&fixture);
    let http = fixture.serve(handler, None).await;
    let resumed = http.register(json!({"session_key":first["session_key"],"external_session_id":"thread-a","title":"Shared title"})).await;
    assert_eq!(resumed, first);
    let saved = http
        .register(json!({"session_key":anonymous["session_key"],"title":"Recovered"}))
        .await;
    assert_eq!(saved["session_key"], anonymous["session_key"]);
    assert_eq!(http.rpc("tools/list", json!({})).await, listing);
    let invalid = http
        .rpc("tools/list", json!({"cursor":"private-cursor"}))
        .await;
    assert_eq!(invalid["error"]["code"], -32602);
    assert!(!invalid.to_string().contains("private-cursor"));
}

#[tokio::test]
async fn text_and_documents_return_committed_composite_routes_for_same_title_sessions() {
    let mut fixture = Fixture::new();
    let telegram = Telegram::new(&fixture.config, vec![sent(7), sent(8), sent(9)]).await;
    let handler = telegram.handler(&fixture);
    let http = fixture.serve(handler, None).await;
    let first = http
        .register(json!({"external_session_id":"a","title":"界"}))
        .await;
    let second = http
        .register(
            json!({"external_session_id":"b","title":"界","directory_label":" \n 📁\u{202e} "}),
        )
        .await;
    assert_eq!(second["directory_label"], "📁");
    assert_eq!(first["directory_label"], Value::Null);
    let text = "a".repeat(998);
    let path = fixture.directory.join("report.bin");
    fs::write(&path, b"synthetic report\0\xff").unwrap();
    let caption = "😀".repeat(509) + "a";
    for (tool, args, id) in [
        (
            "telegram_send",
            json!({"session_key":first["session_key"],"text":text}),
            7,
        ),
        (
            "telegram_send_file",
            json!({"session_key":second["session_key"],"path":path,"caption":caption}),
            8,
        ),
        (
            "telegram_send_file",
            json!({"session_key":first["session_key"],"path":path}),
            9,
        ),
    ] {
        let result = http.tool(tool, args).await;
        assert_eq!(result["isError"], false);
        assert_eq!(
            result["structuredContent"],
            json!({"telegram_chat_id":-101,"outbound_message_id":id})
        );
        if id == 8 {
            http.register(json!({"session_key":first["session_key"],"title":"Renamed"}))
                .await;
        }
    }
    {
        let requests = telegram.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method, "/botSynthetic/sendMessage");
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(
            body,
            json!({"chat_id":-101,"text":format!("界\n{text}"),"entities":[{"type":"bold","offset":0,"length":1}]})
        );
        for (index, (request, caption)) in requests[1..]
            .iter()
            .zip([format!("界\n📁\n{caption}"), "Renamed".into()])
            .enumerate()
        {
            assert_eq!(request.method, "/botSynthetic/sendDocument");
            assert!(
                request
                    .content_type
                    .starts_with("multipart/form-data; boundary=")
            );
            assert_eq!(request.part("caption"), caption.as_bytes());
            assert_eq!(
                serde_json::from_slice::<Value>(request.part("caption_entities")).unwrap(),
                if index == 0 {
                    json!([{"type":"bold","offset":0,"length":1}, {"type":"italic","offset":2,"length":2}])
                } else {
                    json!([{"type":"bold","offset":0,"length":7}])
                }
            );
            let bytes = b"synthetic report\0\xff";
            assert!(request.body.windows(bytes.len()).any(|b| b == bytes));
        }
    }
    let database = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    for (id, session, kind) in [
        (7, &first, RouteKind::Text),
        (8, &second, RouteKind::Document),
        (9, &first, RouteKind::Document),
    ] {
        let route = database
            .route(-101, id, unix_millis().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(route.session_key, session["session_key"].as_str().unwrap());
        assert_eq!(route.kind, kind);
        assert!(
            database
                .route(-102, id, unix_millis().unwrap())
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn validation_and_protocol_failures_have_no_telegram_route_or_activity_effects() {
    let mut fixture = Fixture::new();
    let telegram = Telegram::new(&fixture.config, vec![]).await;
    let handler = telegram.handler(&fixture);
    let http = fixture.serve(handler, None).await;
    let session = http
        .register(json!({"external_session_id":"thread","title":"界"}))
        .await;
    let key = &session["session_key"];
    let path = fixture.directory.join("report.bin");
    fs::write(&path, b"private bytes").unwrap();
    let oversized = fixture.directory.join("private-large.bin");
    let size_limit = u64::from(fixture.config.files.max_outbound_mb) * 1_000_000;
    fs::File::create(&oversized)
        .unwrap()
        .set_len(size_limit + 1)
        .unwrap();
    let database = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    let before = database.session(key.as_str().unwrap()).unwrap();
    for (name, args, code) in [
        (
            "telegram_send",
            json!({"session_key":"private-key","text":"private text"}),
            "SESSION_NOT_FOUND",
        ),
        (
            "telegram_register",
            json!({"session_key":"private-key","title":"private title"}),
            "SESSION_NOT_FOUND",
        ),
        (
            "telegram_register",
            json!({"session_key":key,"external_session_id":"private-other","title":"private title"}),
            "SESSION_IDENTITY_CONFLICT",
        ),
        (
            "telegram_register",
            json!({"session_key":key,"title":"private title","directory_label":"x".repeat(101)}),
            "REQUEST_INVALID",
        ),
        (
            "telegram_send",
            json!({"session_key":key,"text":"a".repeat(999)}),
            "TEXT_TOO_LONG",
        ),
        (
            "telegram_send_file",
            json!({"session_key":key,"path":path,"caption":"a".repeat(999)}),
            "TEXT_TOO_LONG",
        ),
        (
            "telegram_send_file",
            json!({"session_key":key,"path":path,"caption":"😀".repeat(511)+"a"}),
            "TEXT_TOO_LONG",
        ),
        (
            "telegram_send_file",
            json!({"session_key":key,"path":fixture.directory.join("../private-file")}),
            "FILE_UNAVAILABLE",
        ),
        (
            "telegram_send_file",
            json!({"session_key":key,"path":fixture.directory.join("private-missing")}),
            "FILE_UNAVAILABLE",
        ),
        (
            "telegram_send_file",
            json!({"session_key":"private-key","path":path,"caption":"private caption"}),
            "SESSION_NOT_FOUND",
        ),
    ] {
        let native_caption = args["caption"]
            .as_str()
            .is_some_and(|caption| caption.starts_with('😀'));
        let result = http.tool(name, args).await;
        failure(&result, code);
        assert!(!result.to_string().contains("private"));
        if code == "TEXT_TOO_LONG" {
            let details = &result["structuredContent"]["details"];
            assert_eq!(details["limit"], 1_000);
            assert!(
                details["guidance"]
                    .as_str()
                    .unwrap()
                    .contains("telegram_send_file")
            );
            assert_eq!(
                details["actual_characters"],
                if native_caption { 514 } else { 1_001 }
            );
            assert_eq!(
                details["actual_utf16_units"],
                if native_caption { 1_025 } else { 1_001 }
            );
            assert_eq!(
                details["native_limit_utf16"],
                if name == "telegram_send" {
                    4_096
                } else {
                    1_024
                }
            );
        }
    }
    let result = http
        .tool(
            "telegram_send_file",
            json!({"session_key":key,"path":oversized}),
        )
        .await;
    failure(&result, "FILE_TOO_LARGE");
    assert_eq!(
        result["structuredContent"]["details"],
        json!({"limit_bytes":size_limit,"actual_bytes":size_limit+1})
    );
    assert!(!result.to_string().contains("private"));
    for (name, args) in [
        ("private-unknown-tool", json!({})),
        ("telegram_send", json!({"session_key":key,"text":42})),
        (
            "telegram_send",
            json!({"session_key":key,"text":"private text","private-unknown":true}),
        ),
        ("telegram_register", json!({})),
        ("telegram_send_file", json!({"session_key":key,"path":42})),
    ] {
        let result = http
            .rpc("tools/call", json!({"name":name,"arguments":args}))
            .await;
        assert_eq!(result["error"]["code"], -32602);
        assert!(result.get("result").is_none());
        assert!(!result.to_string().contains("private"));
    }
    for name in [None, Some("private-header-mismatch")] {
        let mut body = message("tools/call");
        body["params"]["name"] = "telegram_send".into();
        body["params"]["arguments"] = json!({"session_key":key,"text":"private text"});
        let mut request = http.post(body).build().unwrap();
        request.headers_mut().remove("mcp-name");
        if let Some(name) = name {
            request
                .headers_mut()
                .insert("mcp-name", HeaderValue::from_static(name));
        }
        let response = http.client.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let result: Value = response.json().await.unwrap();
        assert_eq!(result["id"], 7);
        assert_eq!(result["error"]["code"], -32020);
        assert!(!result.to_string().contains("private"));
    }
    assert_eq!(database.session(key.as_str().unwrap()).unwrap(), before);
    let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    assert_eq!(
        sql.query_row("SELECT count(*) FROM message_routes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        sql.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(telegram.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn delivery_errors_remain_distinct_safe_tool_results_without_routes_or_replays() {
    let mut fixture = Fixture::new();
    let telegram = Telegram::new(&fixture.config, vec![
        (429, json!({"ok":false,"error_code":429,"description":"private rejection","parameters":{"retry_after":3}})),
        (502, json!({"ok":false,"error_code":502,"description":"private response"})),
        sent(9),
    ]).await;
    let handler = telegram.handler(&fixture);
    let http = fixture.serve(handler, None).await;
    let session = http.register(json!({"title":"private title"})).await;
    for code in [
        "TELEGRAM_REJECTED",
        "TELEGRAM_DELIVERY_UNCERTAIN",
        "SEND_BOOKKEEPING_FAILED",
    ] {
        if code == "SEND_BOOKKEEPING_FAILED" {
            rusqlite::Connection::open(&fixture.config.storage.path).unwrap().execute_batch(
                "CREATE TRIGGER fail_activity BEFORE UPDATE ON sessions BEGIN SELECT RAISE(ABORT, 'private failure'); END"
            ).unwrap();
        }
        let result = http
            .tool(
                "telegram_send",
                json!({"session_key":session["session_key"],"text":"private text"}),
            )
            .await;
        failure(&result, code);
        assert!(!result.to_string().contains("private"));
        if code == "TELEGRAM_REJECTED" {
            assert_eq!(
                result["structuredContent"]["details"],
                json!({"telegram_code":429,"retry_after_seconds":3})
            );
        } else {
            assert!(
                result["structuredContent"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("do not resend automatically")
            );
        }
    }
    assert_eq!(telegram.requests.lock().unwrap().len(), 3);
    let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    assert_eq!(
        sql.query_row("SELECT count(*) FROM message_routes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
