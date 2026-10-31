//! Initialize-based HTTP compatibility using the same protected endpoint and handler.

use super::*;

fn initialization(version: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":version,"capabilities":{},
        "clientInfo":{"name":"synthetic-mcp-client","version":"1"}
    }})
}

fn inline_meta() -> Value {
    json!({"io.modelcontextprotocol/protocolVersion":LEGACY_VERSION,
        "io.modelcontextprotocol/clientCapabilities":{}})
}

async fn sse_message(response: reqwest::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    let text = tokio::time::timeout(Duration::from_secs(2), response.text())
        .await
        .unwrap()
        .unwrap();
    let messages: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        // rmcp initializes SSE with an empty data event carrying ID/retry hints.
        .filter(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert_eq!(messages.len(), 1, "{text}");
    messages.into_iter().next().unwrap()
}

impl Http {
    fn legacy_post(&self, body: Value, session: Option<&str>) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(format!("http://{}/mcp", self.address))
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&body);
        if let Some(session) = session {
            request = request
                .header("mcp-session-id", session)
                .header("mcp-protocol-version", LEGACY_VERSION);
        }
        request
    }

    fn session_operation(&self, method: http::Method, session: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("http://{}/mcp", self.address))
            .header(header::ACCEPT, "text/event-stream")
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", LEGACY_VERSION)
    }

    async fn initialize(&self, version: &str) -> (String, Value) {
        let response = self
            .legacy_post(initialization(version), None)
            .send()
            .await
            .unwrap();
        let session = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let result = sse_message(response).await;
        assert_eq!(result["id"], 1);
        assert_eq!(result["result"]["protocolVersion"], LEGACY_VERSION);
        assert_eq!(
            status(self.legacy_post(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(&session),
            ))
            .await,
            StatusCode::ACCEPTED
        );
        (session, result["result"].clone())
    }

    async fn legacy_rpc(&self, session: &str, method: &str, params: Value) -> Value {
        let id = self
            .next_legacy_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let response = self
            .legacy_post(
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                Some(session),
            )
            .send()
            .await
            .unwrap();
        let result = sse_message(response).await;
        assert_eq!(result["id"], id);
        result
    }
}

#[tokio::test]
async fn initialization_negotiates_supported_handshake_and_fallback_without_registering_agents() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 8;
    let mut http = fixture.serve(fixture.state.clone(), None).await;
    let mut sessions = Vec::new();
    for requested in [LEGACY_VERSION, VERSION, "2024-11-05", "2099-01-01"] {
        let (session, result) = http.initialize(requested).await;
        assert_eq!(result["capabilities"], json!({"tools":{},"resources":{}}));
        assert!(!sessions.contains(&session));
        let listed = http.legacy_rpc(&session, "tools/list", json!({})).await;
        assert_eq!(
            listed["result"]["tools"],
            http.rpc("tools/list", json!({})).await["result"]["tools"]
        );
        assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 7);
        sessions.push(session);
    }
    // Initialization is allowed to supply its matching version header too.
    let response = http
        .legacy_post(initialization(LEGACY_VERSION), None)
        .header("mcp-protocol-version", LEGACY_VERSION)
        .send()
        .await
        .unwrap();
    assert_eq!(
        sse_message(response).await["result"]["protocolVersion"],
        LEGACY_VERSION
    );
    let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    assert_eq!(
        sql.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let discovery: Value = http.discover().send().await.unwrap().json().await.unwrap();
    assert_eq!(
        discovery["result"]["supportedVersions"],
        json!([LEGACY_VERSION, VERSION])
    );
    assert_eq!(
        discovery["result"]["capabilities"]["resources"]["subscribe"],
        true
    );
    http.finish().await;
}

#[tokio::test]
async fn session_calls_and_read_only_resources_share_explicit_application_identity() {
    let mut fixture = Fixture::new();
    let mut http = fixture.serve(fixture.state.clone(), None).await;
    let (transport, _) = http.initialize(LEGACY_VERSION).await;
    let registered = http
        .legacy_rpc(
            &transport,
            "tools/call",
            json!({"name":"telegram_register",
            "arguments":{"external_session_id":"synthetic-thread","title":"Shared"}}),
        )
        .await;
    assert_eq!(registered["result"]["isError"], false);
    let application = &registered["result"]["structuredContent"];
    assert_ne!(application["session_key"], transport);
    assert_eq!(
        http.register(json!({"external_session_id":"synthetic-thread","title":"Shared"}))
            .await,
        *application
    );
    for (method, params) in [
        ("resources/list", json!({})),
        ("resources/templates/list", json!({})),
        ("resources/read", json!({"uri":application["inbox_uri"]})),
    ] {
        let legacy = http.legacy_rpc(&transport, method, params.clone()).await;
        let modern = http.rpc(method, params).await;
        assert!(legacy.get("error").is_none(), "{legacy}");
        // Read responses have a legacy wire shape without the modern resultType tag.
        let mut modern_result = modern["result"].clone();
        modern_result.as_object_mut().unwrap().remove("resultType");
        assert_eq!(legacy["result"], modern_result);
    }
    for method in ["resources/subscribe", "resources/unsubscribe"] {
        let result = http
            .legacy_rpc(&transport, method, json!({"uri":application["inbox_uri"]}))
            .await;
        assert_eq!(result["error"]["code"], -32601);
    }
    assert_eq!(
        status(http.session_operation(http::Method::DELETE, &transport)).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        status(http.legacy_post(
            json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}),
            Some(&transport)
        ))
        .await,
        StatusCode::NOT_FOUND
    );
    // Transport termination does not remove durable application registration.
    assert_eq!(
        http.register(json!({"session_key":application["session_key"],"title":"Shared"}))
            .await,
        *application
    );
    http.finish().await;
}

#[tokio::test]
async fn all_legacy_session_operations_require_bearer_and_safe_authority() {
    let mut fixture = Fixture::new();
    let secret = fixture.auth();
    let mut http = fixture.serve(fixture.state.clone(), Some(&secret)).await;
    let response = http
        .legacy_post(initialization(LEGACY_VERSION), None)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    sse_message(response).await;
    let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let listing = json!({"jsonrpc":"2.0","id":7,"method":"tools/list"});
    for request in [
        http.legacy_post(initialization(LEGACY_VERSION), None),
        http.legacy_post(initialized.clone(), Some(&session)),
        http.legacy_post(listing.clone(), Some(&session)),
        http.session_operation(http::Method::GET, &session),
        http.session_operation(http::Method::DELETE, &session),
    ] {
        for auth in [None, Some("Bearer wrong")] {
            let mut request = request.try_clone().unwrap();
            if let Some(auth) = auth {
                request = request.header(header::AUTHORIZATION, auth);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        }
        for (name, value) in [("host", "attacker.invalid"), ("origin", "null")] {
            assert_eq!(
                status(
                    request
                        .try_clone()
                        .unwrap()
                        .bearer_auth(TOKEN)
                        .header(name, value)
                )
                .await,
                StatusCode::FORBIDDEN
            );
        }
    }
    assert_eq!(
        status(
            http.legacy_post(initialized, Some(&session))
                .bearer_auth(TOKEN)
        )
        .await,
        StatusCode::ACCEPTED
    );
    let response = http
        .legacy_post(listing.clone(), Some(&session))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(
        sse_message(response).await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    for request in [
        http.legacy_post(listing, Some("synthetic-unknown-session")),
        http.session_operation(http::Method::GET, "synthetic-unknown-session"),
    ] {
        assert_eq!(
            status(request.bearer_auth(TOKEN)).await,
            StatusCode::NOT_FOUND
        );
    }
    let stream = http
        .session_operation(http::Method::GET, &session)
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(stream.headers()[header::CONTENT_TYPE], "text/event-stream");
    assert_eq!(
        status(
            http.session_operation(http::Method::DELETE, &session)
                .bearer_auth(TOKEN)
        )
        .await,
        StatusCode::ACCEPTED
    );
    // Termination also closes a still-connected standalone stream.
    tokio::time::timeout(Duration::from_secs(2), stream.bytes())
        .await
        .unwrap()
        .unwrap();
    http.finish().await;
}

#[tokio::test]
async fn ambiguous_headers_and_conflicting_initialization_fail_before_session_creation() {
    let mut fixture = Fixture::new();
    let mut http = fixture.serve(fixture.state.clone(), None).await;
    for (name, first, second) in [
        ("mcp-protocol-version", LEGACY_VERSION, LEGACY_VERSION),
        ("mcp-protocol-version", LEGACY_VERSION, VERSION),
        ("mcp-session-id", "synthetic-session", "synthetic-session"),
        ("mcp-session-id", "synthetic-session", "other-session"),
        ("mcp-method", "initialize", "tools/list"),
    ] {
        for (first, second) in [(first, second), (second, first)] {
            let response = http
                .legacy_post(initialization(LEGACY_VERSION), None)
                .header(name, first)
                .header(name, second)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert!(response.headers().get("mcp-session-id").is_none());
            assert!(!response.text().await.unwrap().contains("synthetic-session"));
        }
    }
    for (name, value) in [
        ("mcp-protocol-version", VERSION),
        ("mcp-protocol-version", "2099-01-01"),
        ("mcp-protocol-version", "2025-06-18, 2026-07-28"),
        ("mcp-session-id", ""),
    ] {
        let response = http
            .legacy_post(initialization(LEGACY_VERSION), None)
            .header(name, value)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.headers().get("mcp-session-id").is_none());
    }
    // Rejections do not consume the two-session admission budget.
    let (session, _) = http.initialize(LEGACY_VERSION).await;
    let listing = json!({"jsonrpc":"2.0","id":7,"method":"tools/list"});
    for version in [None, Some("2025-03-26"), Some("2099-01-01"), Some(VERSION)] {
        let mut request = http
            .legacy_post(listing.clone(), Some(&session))
            .build()
            .unwrap();
        request.headers_mut().remove("mcp-protocol-version");
        if let Some(version) = version {
            request.headers_mut().insert(
                "mcp-protocol-version",
                HeaderValue::from_str(version).unwrap(),
            );
        }
        assert_eq!(
            http.client.execute(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut conflicting = listing.clone();
    conflicting["params"] = json!({"_meta":{"io.modelcontextprotocol/protocolVersion":VERSION,
        "io.modelcontextprotocol/clientCapabilities":{}}});
    let response = http
        .legacy_post(conflicting, Some(&session))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        -32020
    );
    assert_eq!(
        http.legacy_rpc(&session, "tools/list", json!({})).await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    http.finish().await;
}

#[tokio::test]
async fn supplied_method_header_is_only_validated_for_modern_protocol_declarations() {
    let mut fixture = Fixture::new();
    let mut http = fixture.serve(fixture.state.clone(), None).await;
    for version in [None, Some(LEGACY_VERSION), Some(VERSION)] {
        let mut request = http
            .legacy_post(initialization(version.unwrap_or(LEGACY_VERSION)), None)
            .header("mcp-method", "tools/list");
        if let Some(version) = version {
            request = request.header("mcp-protocol-version", version);
        }
        let response = request.send().await.unwrap();
        if version == Some(VERSION) {
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert!(response.headers().get("mcp-session-id").is_none());
        } else {
            // Mcp-Method has no legacy protocol meaning. The SDK ignores it.
            let session = response.headers()["mcp-session-id"]
                .to_str()
                .unwrap()
                .to_owned();
            assert_eq!(
                sse_message(response).await["result"]["protocolVersion"],
                LEGACY_VERSION
            );
            assert_eq!(
                status(http.session_operation(http::Method::DELETE, &session)).await,
                StatusCode::ACCEPTED
            );
        }
    }
    http.finish().await;
}

#[tokio::test]
async fn matching_inline_metadata_cannot_discard_live_unknown_or_terminated_sessions() {
    let mut fixture = Fixture::new();
    let mut http = fixture.serve(fixture.state.clone(), None).await;
    let (session, _) = http.initialize(LEGACY_VERSION).await;
    let call = json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
        "_meta":inline_meta(),"name":"telegram_register",
        "arguments":{"title":"private-title","external_session_id":"private-thread"}}});
    for transport in [&session, "synthetic-unknown-session"] {
        let response = http
            .legacy_post(call.clone(), Some(transport))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(!response.text().await.unwrap().contains("private"));
    }
    // Partial metadata does not select the SDK's stateless lifecycle.
    let listed = http
        .legacy_rpc(
            &session,
            "tools/list",
            json!({"_meta":{
        "io.modelcontextprotocol/protocolVersion":LEGACY_VERSION}}),
        )
        .await;
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 7);
    assert_eq!(
        status(http.session_operation(http::Method::DELETE, &session)).await,
        StatusCode::ACCEPTED
    );
    let response = http.legacy_post(call, Some(&session)).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // Ordinary session calls still return the SDK's unknown-session status.
    assert_eq!(
        status(http.legacy_post(
            json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}),
            Some(&session)
        ))
        .await,
        StatusCode::NOT_FOUND
    );
    let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    assert_eq!(
        sql.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    http.finish().await;
}

struct SessionProbe {
    state: Arc<McpHandler>,
    started: mpsc::Sender<String>,
    ended: mpsc::Sender<String>,
}

impl ServerHandler for SessionProbe {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        self.state.get_info()
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        self.state.supported_protocol_versions()
    }

    async fn initialize(
        &self,
        request: rmcp::model::InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::InitializeResult, ErrorData> {
        self.state.initialize(request, context).await
    }

    async fn list_tools(
        &self,
        _params: Option<rmcp::model::PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        let id = context.id.to_string();
        self.started.send(id.clone()).await.unwrap();
        context.ct.cancelled().await;
        self.ended.send(id).await.unwrap();
        Err(ErrorData::internal_error("Request cancelled", None))
    }
}

#[tokio::test]
async fn inline_metadata_does_not_escape_session_cancellation_or_delete_ownership() {
    let mut fixture = Fixture::new();
    let (started, mut starts) = mpsc::channel(4);
    let (ended, mut ends) = mpsc::channel(4);
    let probe = Arc::new(SessionProbe {
        state: fixture.state.clone(),
        started,
        ended,
    });
    let mut http = fixture.serve(probe, None).await;
    let (session, _) = http.initialize(LEGACY_VERSION).await;
    for id in [101, 102] {
        let response = http
            .legacy_post(
                json!({"jsonrpc":"2.0","id":id,"method":"tools/list"}),
                Some(&session),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), starts.recv())
                .await
                .unwrap()
                .unwrap(),
            id.to_string()
        );
        let inline = json!({"jsonrpc":"2.0","id":id+100,"method":"tools/list",
            "params":{"_meta":inline_meta()}});
        for transport in [&session, "synthetic-unknown-session"] {
            assert_eq!(
                status(http.legacy_post(inline.clone(), Some(transport))).await,
                StatusCode::BAD_REQUEST
            );
        }
        assert!(starts.try_recv().is_err());
        assert!(ends.try_recv().is_err());
        if id == 101 {
            let cancelled = json!({"jsonrpc":"2.0","method":"notifications/cancelled",
                "params":{"requestId":id,"_meta":inline_meta()}});
            assert_eq!(
                status(http.legacy_post(cancelled.clone(), Some("synthetic-unknown-session")))
                    .await,
                StatusCode::NOT_FOUND
            );
            assert!(ends.try_recv().is_err());
            // Notification metadata keeps the SDK's session semantics and cancels
            // only work belonging to this actual transport session.
            assert_eq!(
                status(http.legacy_post(cancelled, Some(&session))).await,
                StatusCode::ACCEPTED
            );
        } else {
            assert_eq!(
                status(http.session_operation(http::Method::DELETE, &session)).await,
                StatusCode::ACCEPTED
            );
        }
        // DELETE preserves rmcp's five-second in-flight response drain before
        // cancelling unfinished handlers. Wait for the event beyond that grace.
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), ends.recv())
                .await
                .unwrap()
                .unwrap(),
            id.to_string()
        );
        tokio::time::timeout(Duration::from_secs(2), response.bytes())
            .await
            .unwrap()
            .unwrap();
        if id == 102 {
            assert_eq!(
                status(http.legacy_post(inline, Some(&session))).await,
                StatusCode::BAD_REQUEST
            );
            assert!(starts.try_recv().is_err());
        }
    }
    http.finish().await;
}

#[tokio::test]
async fn session_lifecycle_preflight_preserves_declared_and_chunked_body_bounds() {
    let mut fixture = Fixture::new();
    let body = format!(
        "{}{}",
        json!({"jsonrpc":"2.0","id":7,"method":"tools/list",
        "params":{"_meta":inline_meta()}}),
        " ".repeat(128)
    );
    fixture.config.limits.request_bytes = body.len() as u32;
    let mut http = fixture.serve(fixture.state.clone(), None).await;
    let (session, _) = http.initialize(LEGACY_VERSION).await;
    for oversized in [false, true] {
        let payload = if oversized {
            format!("{body} ")
        } else {
            body.clone()
        };
        for chunked in [false, true] {
            let request_body = if chunked {
                let chunks: Vec<_> = payload
                    .as_bytes()
                    .chunks(13)
                    .map(|chunk| Ok::<_, std::io::Error>(Bytes::copy_from_slice(chunk)))
                    .collect();
                reqwest::Body::wrap_stream(futures_util::stream::iter(chunks))
            } else {
                reqwest::Body::from(payload.clone())
            };
            assert_eq!(
                status(
                    http.legacy_post(Value::Null, Some(&session))
                        .body(request_body)
                )
                .await,
                if oversized {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                }
            );
        }
    }
    assert_eq!(
        http.legacy_rpc(&session, "tools/list", json!({})).await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    http.finish().await;
}

#[tokio::test]
async fn malformed_initialization_and_inclusive_body_limits_are_enforced() {
    let mut fixture = Fixture::new();
    let body = initialization(LEGACY_VERSION).to_string();
    fixture.config.limits.request_bytes = body.len() as u32;
    let mut http = fixture.serve(fixture.state.clone(), None).await;
    // Invalid JSON fails decoding; invalid initialize parameters fall through
    // the SDK's CustomRequest variant and are rejected before session creation.
    for (malformed, expected) in [
        ("private-parser-input", StatusCode::UNSUPPORTED_MEDIA_TYPE),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":42}}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = http
            .legacy_post(Value::Null, None)
            .body(malformed)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        assert!(response.headers().get("mcp-session-id").is_none());
        assert!(
            !response
                .text()
                .await
                .unwrap()
                .contains("private-parser-input")
        );
    }
    assert_eq!(
        status(http.legacy_post(Value::Null, None).body(format!("{body} "))).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let response = http
        .legacy_post(Value::Null, None)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        sse_message(response).await["result"]["protocolVersion"],
        LEGACY_VERSION
    );
    http.finish().await;
}

#[tokio::test]
async fn modern_tools_still_require_per_request_metadata_and_headers() {
    let mut fixture = Fixture::new();
    let mut http = fixture.serve(fixture.state.clone(), None).await;
    for key in [
        "io.modelcontextprotocol/protocolVersion",
        "io.modelcontextprotocol/clientCapabilities",
    ] {
        let mut body = message("tools/list");
        body["params"]["_meta"].as_object_mut().unwrap().remove(key);
        let response = http.post(body).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"]["code"],
            -32602
        );
    }
    for name in ["mcp-method", "mcp-protocol-version"] {
        let mut request = http.post(message("tools/list")).build().unwrap();
        request.headers_mut().remove(name);
        let response = http.client.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"]["code"],
            -32020
        );
    }
    assert_eq!(
        http.rpc("tools/list", json!({})).await["result"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    http.finish().await;
}
