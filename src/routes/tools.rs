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
                                        content_type: if parts.method == http::Method::GET {
                                            parts.headers.get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap().to_owned()).unwrap_or_default()
                                        } else {
                                            parts.headers[header::CONTENT_TYPE].to_str().unwrap().to_owned()
                                        },
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

pub(crate) fn assert_tool_catalog(catalog: &Value) {
    use crate::dto::{
        AckRequest, AckResult, AskResult, GetAttachmentRequest, GetAttachmentResult,
        GuidanceRequest, GuidanceResult, HistoryRequest, HistoryResult, InboxRequest, InboxResult,
        ProcessingRequest, ProcessingResult, RegisterRequest, RegisterResult, SendFileRequest,
        SendRequest, SendResult, WaitRequest, WaitResult,
    };
    use rmcp::{model::Tool, schemars::JsonSchema};

    fn schema<I: JsonSchema + 'static, O: JsonSchema + 'static>(name: &'static str) -> Tool {
        Tool::new(name, "", serde_json::Map::new())
            .with_input_schema::<I>()
            .with_output_schema::<O>()
    }

    // Keep expected names and DTO bindings separate from handler registration so missing
    // tools, swapped schemas and weakened required fields fail together.
    let expected = [
        (
            schema::<RegisterRequest, RegisterResult>("telegram_register"),
            json!(["title"]),
        ),
        (
            schema::<SendRequest, SendResult>("telegram_send"),
            json!(["session_key", "text"]),
        ),
        (
            schema::<SendFileRequest, SendResult>("telegram_send_file"),
            json!(["session_key", "path"]),
        ),
        (
            schema::<InboxRequest, InboxResult>("telegram_inbox"),
            json!(["session_key"]),
        ),
        (
            schema::<ProcessingRequest, ProcessingResult>("telegram_processing"),
            json!(["session_key", "reply_id", "claim_token"]),
        ),
        (
            schema::<AckRequest, AckResult>("telegram_ack"),
            json!(["session_key", "reply_id", "claim_token"]),
        ),
        (
            schema::<SendRequest, AskResult>("telegram_ask"),
            json!(["session_key", "text"]),
        ),
        (
            schema::<WaitRequest, WaitResult>("telegram_wait"),
            json!(["session_key", "outbound_message_id"]),
        ),
        (
            schema::<HistoryRequest, HistoryResult>("telegram_history"),
            json!(["session_key"]),
        ),
        (
            schema::<GetAttachmentRequest, GetAttachmentResult>("telegram_get_attachment"),
            json!(["session_key", "attachment_handle"]),
        ),
        (
            schema::<GuidanceRequest, GuidanceResult>("telegram_guidance"),
            Value::Null,
        ),
    ];
    let tools = catalog.as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|(tool, _)| tool.name.as_ref())
            .collect::<Vec<_>>()
    );
    for (tool, (expected, required)) in tools.iter().zip(expected) {
        assert!(
            tool["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["inputSchema"]["required"], required);
        assert_eq!(tool["outputSchema"]["type"], "object");
        assert_eq!(
            tool["inputSchema"],
            serde_json::to_value(expected.input_schema).unwrap()
        );
        assert_eq!(
            tool["outputSchema"],
            serde_json::to_value(expected.output_schema).unwrap()
        );
    }
}

#[tokio::test]
async fn tools_have_typed_schemas_and_registration_survives_separate_requests_and_renames() {
    let mut fixture = Fixture::new();
    let state = fixture.state.clone();
    let mut http = fixture.serve(state, None).await;
    let listing = http.rpc("tools/list", json!({})).await;
    assert_tool_catalog(&listing["result"]["tools"]);
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

#[tokio::test]
async fn shared_history_requires_bearer_and_is_read_only_through_both_protocols() {
    let mut fixture = Fixture::new();
    let auth = fixture.auth();
    fixture.config.history.enabled = true;
    let mut database = Database::open_with_history(
        &fixture.config.storage,
        &fixture.config.limits,
        &fixture.config.history,
        true,
    )
    .unwrap();
    let now = unix_millis().unwrap();
    let observer = database
        .register_session(None, None, "Observer", None, now)
        .unwrap();
    let owner = database
        .register_session(None, None, "Owner", None, now)
        .unwrap();
    database
        .record_message(
            &crate::repositories::JournalInput {
                source: crate::repositories::MessageSource {
                    chat_id: -101,
                    message_id: 7,
                    sender_user_id: Some(7),
                },
                direction: crate::repositories::MessageDirection::Incoming,
                body: Some("Untrusted *literal*"),
                chat_label: None,
                sender_label: None,
                session: Some(&owner),
                message_date: now,
                reply_to_message_id: None,
            },
            now,
        )
        .unwrap();
    let database = Arc::new(Mutex::new(database));
    let client =
        TelegramClient::for_test(&fixture.config, "http://127.0.0.1:1/botSynthetic/".into())
            .unwrap();
    let handler = Arc::new(McpHandler::new(database.clone(), client, &fixture.config).unwrap());
    let mut http = fixture.serve(handler, Some(&auth)).await;
    let arguments = json!({"session_key":observer.session_key});
    let mut call = message("tools/call");
    call["params"]["name"] = "telegram_history".into();
    call["params"]["arguments"] = arguments.clone();
    assert_eq!(
        status(http.post(call.clone())).await,
        StatusCode::UNAUTHORIZED
    );
    let modern: Value = http
        .post(call)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(modern["result"]["isError"], false);
    assert_eq!(
        modern["result"]["structuredContent"]["messages"][0]["text_or_caption"],
        "Untrusted *literal*"
    );
    assert_eq!(modern["result"]["content"].as_array().unwrap().len(), 1);
    assert_eq!(modern["result"]["content"][0]["type"], "text");
    assert_eq!(
        serde_json::from_str::<Value>(modern["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        modern["result"]["structuredContent"]
    );
    assert!(!modern.to_string().contains(&owner.session_key));
    let response = http
        .legacy_post(legacy::initialization(LEGACY_VERSION), None)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    let transport = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    legacy::sse_message(response).await;
    assert_eq!(
        status(
            http.legacy_post(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(&transport)
            )
            .bearer_auth(TOKEN)
        )
        .await,
        StatusCode::ACCEPTED
    );
    let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"telegram_history","arguments":arguments}});
    assert_eq!(
        status(http.legacy_post(call.clone(), Some(&transport))).await,
        StatusCode::UNAUTHORIZED
    );
    let legacy = http
        .legacy_post(call, Some(&transport))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    let legacy = legacy::sse_message(legacy).await;
    assert_eq!(modern["result"]["resultType"], "complete");
    assert!(legacy["result"].get("resultType").is_none());
    let expected = json!({
        "structuredContent": modern["result"]["structuredContent"],
        "content": modern["result"]["content"],
        "isError": false,
    });
    assert_eq!(legacy, json!({"jsonrpc":"2.0", "id":7, "result":expected}));
    let mut expected = expected;
    expected["resultType"] = json!("complete");
    assert_eq!(modern, json!({"jsonrpc":"2.0", "id":7, "result":expected}));
    assert_eq!(
        database
            .lock()
            .unwrap()
            .session(&observer.session_key)
            .unwrap(),
        observer
    );
    http.finish().await;
}
