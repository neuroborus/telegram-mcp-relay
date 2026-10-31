//! Mixed protocol clients share durable application state, never transport identities.

use super::*;
use crate::{
    repositories::{IngestOutcome, RouteKind},
    services::{ingress::IngressService, unix_millis},
};
use ask::Mock;
use futures_util::future::Either;
use legacy::sse_message;
use subscriptions::{ingest, route};
use tools::{Telegram, failure, sent};

#[derive(Clone, Copy)]
enum Client<'a> {
    Modern,
    Legacy(&'a str),
}

impl Client<'_> {
    async fn rpc(self, http: &Http, method: &str, params: Value) -> Value {
        match self {
            Self::Modern => http.rpc(method, params).await,
            Self::Legacy(transport) => http.legacy_rpc(transport, method, params).await,
        }
    }

    async fn tool(self, http: &Http, name: &str, arguments: Value) -> Value {
        let body = self
            .rpc(
                http,
                "tools/call",
                json!({"name":name,"arguments":arguments}),
            )
            .await;
        assert!(body.get("error").is_none(), "{body}");
        body["result"].clone()
    }

    async fn success(self, http: &Http, name: &str, arguments: Value) -> Value {
        let result = self.tool(http, name, arguments).await;
        assert_eq!(result["isError"], false, "{result}");
        result["structuredContent"].clone()
    }

    async fn snapshot(self, http: &Http, uri: &str) -> Value {
        let response = self.rpc(http, "resources/read", json!({"uri":uri})).await;
        assert!(response.get("error").is_none(), "{response}");
        let result = &response["result"];
        assert_eq!(result["cacheScope"], "private");
        assert_eq!(result["ttlMs"], 0);
        assert_eq!(result["contents"][0]["uri"], uri);
        assert_eq!(result["contents"][0]["mimeType"], "application/json");
        serde_json::from_str(result["contents"][0]["text"].as_str().unwrap()).unwrap()
    }
}

fn handle(session: &Value, claim: &Value) -> Value {
    json!({"session_key":session["session_key"],"reply_id":claim["reply_id"],
        "claim_token":claim["claim_token"]})
}

fn start_legacy_tool(
    http: &Http,
    transport: &str,
    id: i64,
    name: &str,
    args: Value,
) -> JoinHandle<Value> {
    let request = http.legacy_post(
        json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":name,"arguments":args}}),
        Some(transport),
    );
    tokio::spawn(async move {
        let body = sse_message(request.send().await.unwrap()).await;
        assert_eq!(body["id"], id);
        assert!(body.get("error").is_none(), "{body}");
        body["result"].clone()
    })
}

#[tokio::test]
async fn both_protocols_share_seven_tools_resources_and_durable_routes() {
    let mut fixture = Fixture::new();
    fixture.config.delivery.wait_fallback_enabled = false;
    let telegram = Telegram::new(&fixture.config, (7..13).map(sent).collect()).await;
    let state = telegram.handler(&fixture);
    let mut http = fixture.serve(state.clone(), None).await;
    let (transport, _) = http.initialize(LEGACY_VERSION).await;
    let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    assert_eq!(
        sql.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let clients = [Client::Legacy(&transport), Client::Modern];
    let file = fixture.directory.join("report.bin");
    fs::write(&file, b"synthetic report\0\xff").unwrap();
    let mut sessions = Vec::new();
    for (index, client) in clients.into_iter().enumerate() {
        let listed = client.rpc(&http, "tools/list", json!({})).await;
        assert_eq!(
            listed["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "telegram_register",
                "telegram_send",
                "telegram_send_file",
                "telegram_inbox",
                "telegram_processing",
                "telegram_ack",
                "telegram_ask"
            ]
        );
        let args = json!({"title":"Same", "external_session_id":format!("mixed-{index}")});
        let session = client
            .success(&http, "telegram_register", args.clone())
            .await;
        assert_ne!(session["session_key"], transport);
        assert_eq!(
            clients[1 - index]
                .success(&http, "telegram_register", args)
                .await,
            session
        );
        for (offset, name, args, kind) in [
            (0, "telegram_send", json!({"text":"Text"}), RouteKind::Text),
            (
                1,
                "telegram_send_file",
                json!({"path":file,"caption":"Report"}),
                RouteKind::Document,
            ),
            (
                2,
                "telegram_ask",
                json!({"text":"Question"}),
                RouteKind::Text,
            ),
        ] {
            let mut args = args;
            args["session_key"] = session["session_key"].clone();
            let result = client.success(&http, name, args).await;
            let id = 7 + index as i64 * 3 + offset;
            assert_eq!(result["telegram_chat_id"], -101);
            assert_eq!(result["outbound_message_id"], id);
            if name == "telegram_ask" {
                assert_eq!(result["status"], "sent");
                assert_eq!(result["reply"], Value::Null);
            }
            let mut database =
                Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
            let now = unix_millis().unwrap();
            let route = database.route(-101, id, now).unwrap().unwrap();
            assert_eq!(route.session_key, session["session_key"].as_str().unwrap());
            assert_eq!(route.kind, kind);
            let update = serde_json::from_value(json!({"update_id":100+id,"message":{
                "message_id":200+id,"date":1,"chat":{"id":-101},"from":{"id":7,"is_bot":false},
                "reply_to_message":{"message_id":id},"text":format!("reply {id}")}}))
            .unwrap();
            assert!(matches!(
                IngressService::new(&fixture.config)
                    .unwrap()
                    .ingest(&mut database, &update, now)
                    .unwrap(),
                IngestOutcome::ReplyStored { .. }
            ));
        }
        sessions.push(session);
    }
    assert_ne!(sessions[0]["session_key"], sessions[1]["session_key"]);
    for (index, session) in sessions.iter().enumerate() {
        let uri = session["inbox_uri"].as_str().unwrap();
        let database = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
        let before = database
            .session(session["session_key"].as_str().unwrap())
            .unwrap();
        for client in clients {
            assert_eq!(
                client.rpc(&http, "resources/list", json!({})).await["result"]["resources"],
                json!([])
            );
            assert_eq!(
                client
                    .rpc(&http, "resources/templates/list", json!({}))
                    .await["result"]["resourceTemplates"][0]["uriTemplate"],
                "telegram-bridge://sessions/{session_key}/inbox"
            );
            let page = client.snapshot(&http, &format!("{uri}?limit=2")).await;
            assert_eq!(page["replies"].as_array().unwrap().len(), 2);
            let last = client
                .snapshot(&http, page["next_uri"].as_str().unwrap())
                .await;
            assert_eq!(last["replies"].as_array().unwrap().len(), 1);
            assert_eq!(last["next_uri"], Value::Null);
            assert_eq!(
                last["replies"][0]["reply_to_bot_message_id"],
                9 + index as i64 * 3
            );
            assert_eq!(
                client.snapshot(&http, uri).await,
                clients[1 - index].snapshot(&http, uri).await
            );
            for reply in page["replies"].as_array().unwrap() {
                assert_eq!(reply["state"], "pending");
                assert_eq!(reply["claimed_at"], Value::Null);
                assert!(reply.get("claim_token").is_none());
            }
        }
        assert_eq!(
            database
                .session(session["session_key"].as_str().unwrap())
                .unwrap(),
            before
        );
        let claimed = clients[index]
            .success(
                &http,
                "telegram_inbox",
                json!({"session_key":session["session_key"]}),
            )
            .await;
        assert_eq!(claimed["replies"].as_array().unwrap().len(), 3);
        for claim in claimed["replies"].as_array().unwrap() {
            let args = handle(session, claim);
            for active in [true, false] {
                let mut processing = args.clone();
                processing["active"] = active.into();
                let result = clients[index]
                    .success(&http, "telegram_processing", processing)
                    .await;
                assert!(result["processing_started_at"].is_i64());
            }
            clients[index].success(&http, "telegram_ack", args).await;
        }
        assert_eq!(
            clients[1 - index].snapshot(&http, uri).await["replies"],
            json!([])
        );
    }
    assert_eq!(
        sql.query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    // Both wire paths retain semantic and file-root validation before Telegram effects.
    for client in clients {
        for (name, args, code) in [
            (
                "telegram_send",
                json!({"text":"x".repeat(1_001)}),
                "TEXT_TOO_LONG",
            ),
            (
                "telegram_send_file",
                json!({"path":fixture.directory.parent().unwrap().join("outside.bin")}),
                "FILE_UNAVAILABLE",
            ),
        ] {
            let mut args = args;
            args["session_key"] = sessions[0]["session_key"].clone();
            failure(&client.tool(&http, name, args).await, code);
        }
    }
    http.finish().await;
    state.stop();
    state.drain().await.unwrap();
}

#[tokio::test]
async fn mixed_claims_reconcile_fence_and_ack_across_restart() {
    let mut fixture = Fixture::new();
    let mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let mut http = fixture.serve(state.clone(), None).await;
    let (transport, _) = http.initialize(LEGACY_VERSION).await;
    let older = Client::Legacy(&transport);
    let modern = Client::Modern;
    let session = older
        .success(
            &http,
            "telegram_register",
            json!({"title":"Same","external_session_id":"recovery"}),
        )
        .await;
    let other = modern
        .success(&http, "telegram_register", json!({"title":"Same"}))
        .await;
    let key = session["session_key"].as_str().unwrap();
    let uri = session["inbox_uri"].as_str().unwrap();
    route(&database, key, 7);
    let ingress = IngressService::new(&fixture.config).unwrap();
    for id in [11, 12] {
        ingest(&ingress, &database, id, 7).unwrap();
    }
    let (a, b) = tokio::join!(
        older.success(
            &http,
            "telegram_inbox",
            json!({"session_key":key,"limit":1})
        ),
        modern.success(
            &http,
            "telegram_inbox",
            json!({"session_key":key,"limit":1})
        ),
    );
    let a = &a["replies"][0];
    let b = &b["replies"][0];
    assert_ne!(a["reply_id"], b["reply_id"]);
    assert_ne!(a["claim_token"], b["claim_token"]);
    let mut start = handle(&session, a);
    start["active"] = true.into();
    let started = older.success(&http, "telegram_processing", start).await;
    assert!(started["processing_started_at"].is_i64());
    let mut resume = handle(&session, a);
    resume["reconcile_request_id"] = "synthetic-mixed-reconcile".into();
    let rotated = modern
        .success(&http, "telegram_processing", resume.clone())
        .await;
    assert_ne!(rotated["claim_token"], a["claim_token"]);
    assert_eq!(
        rotated["processing_started_at"],
        started["processing_started_at"]
    );
    http.finish().await;
    state.stop();
    state.drain().await.unwrap();
    drop(state);
    drop(database);
    let (reopened, state) = mock.shared(&fixture);
    let mut http = fixture.serve(state.clone(), None).await;
    let (replacement, _) = http.initialize(LEGACY_VERSION).await;
    assert_ne!(replacement, transport);
    let older = Client::Legacy(&replacement);
    assert_eq!(
        status(http.legacy_post(
            json!({"jsonrpc":"2.0","id":8,"method":"tools/list"}),
            Some(&transport)
        ))
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        older
            .success(
                &http,
                "telegram_register",
                json!({"session_key":key,"external_session_id":"recovery","title":"Same"})
            )
            .await,
        session
    );
    assert_eq!(
        modern
            .success(&http, "telegram_processing", resume.clone())
            .await,
        rotated
    );
    assert_eq!(
        older.success(&http, "telegram_processing", resume).await,
        rotated
    );
    let page = older.snapshot(&http, uri).await;
    assert_eq!(page, modern.snapshot(&http, uri).await);
    assert_eq!(page["replies"].as_array().unwrap().len(), 2);
    assert!(
        page["replies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reply_id"] == a["reply_id"] && r["state"] == "processing")
    );
    assert!(!page.to_string().contains("claim_token"));
    let mut current = handle(&session, a);
    current["claim_token"] = rotated["claim_token"].clone();
    for client in [older, modern] {
        for name in ["telegram_processing", "telegram_ack"] {
            let mut stale = handle(&session, a);
            let mut wrong = current.clone();
            wrong["session_key"] = other["session_key"].clone();
            if name == "telegram_processing" {
                stale["active"] = true.into();
                wrong["active"] = false.into();
            }
            failure(&client.tool(&http, name, stale).await, "CLAIM_INVALID");
            failure(&client.tool(&http, name, wrong).await, "CLAIM_INVALID");
        }
    }
    let ack = older.success(&http, "telegram_ack", current.clone()).await;
    assert_eq!(ack, json!({"reply_id":a["reply_id"],"acknowledged":true}));
    assert_eq!(modern.success(&http, "telegram_ack", current).await, ack);
    modern
        .success(&http, "telegram_ack", handle(&session, b))
        .await;
    assert_eq!(older.snapshot(&http, uri).await["replies"], json!([]));
    assert_eq!(reopened.lock().unwrap().update_cursor().unwrap(), Some(12));
    http.finish().await;
    state.stop();
    state.drain().await.unwrap();
}

#[tokio::test]
async fn mixed_questions_target_their_own_replies_while_modern_events_remain_active() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 8;
    fixture.config.limits.waiters = 2;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let mut http = fixture.serve(state.clone(), None).await;
    let (transport, _) = http.initialize(LEGACY_VERSION).await;
    let session = http.register(json!({"title":"Same"})).await;
    let key = session["session_key"].as_str().unwrap();
    let uri = session["inbox_uri"].as_str().unwrap();
    let filter = json!({"resourceSubscriptions":[uri]});
    let mut events = http.listen(filter.clone(), filter).await;
    events.updated(uri).await;
    let older = start_legacy_tool(
        &http,
        &transport,
        101,
        "telegram_ask",
        json!({"session_key":key,"text":"Older question"}),
    );
    mock.accept(&database, 7).await;
    let ingress = IngressService::new(&fixture.config).unwrap();
    let result = {
        let modern = Client::Modern.success(
            &http,
            "telegram_ask",
            json!({"session_key":key,"text":"Modern question"}),
        );
        tokio::pin!(modern);
        tokio::select! {
            _ = mock.accept(&database, 8) => (),
            _ = &mut modern => panic!("ask finished before reply"),
        }
        // Reverse reply arrival on one application identity proves parent-message targeting.
        ingest(&ingress, &database, 11, 8).unwrap();
        modern.await
    };
    assert_eq!(result["status"], "reply_received");
    assert_eq!(result["reply"]["reply_to_bot_message_id"], 8);
    assert!(!older.is_finished());
    events.updated(uri).await;
    ingest(&ingress, &database, 12, 7).unwrap();
    let result = older.await.unwrap();
    assert_eq!(result["isError"], false);
    let claim = &result["structuredContent"]["reply"];
    assert_eq!(result["structuredContent"]["status"], "reply_received");
    assert_eq!(claim["reply_to_bot_message_id"], 7);
    events.updated(uri).await;
    assert_eq!(
        Client::Legacy(&transport)
            .success(&http, "telegram_inbox", json!({"session_key":key}))
            .await["replies"],
        json!([])
    );
    let page = http.snapshot(uri).await;
    assert_eq!(page["replies"].as_array().unwrap().len(), 2);
    assert!(
        page["replies"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["state"] == "claimed" && r["processing_started_at"].is_null())
    );
    drop(events);
    http.finish().await;
    state.stop();
    state.drain().await.unwrap();
}

struct Observed {
    state: Arc<McpHandler>,
    ended: mpsc::Sender<()>,
}

struct Ended(mpsc::Sender<()>);
impl Drop for Ended {
    fn drop(&mut self) {
        self.0.try_send(()).unwrap();
    }
}

impl ServerHandler for Observed {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        self.state.get_info()
    }
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        self.state.supported_protocol_versions()
    }
    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.state.get_tool(name)
    }
    async fn initialize(
        &self,
        request: rmcp::model::InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::InitializeResult, ErrorData> {
        self.state.initialize(request, context).await
    }
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        let _ended = Ended(self.ended.clone());
        self.state.call_tool(request, context).await
    }
}

#[tokio::test]
async fn legacy_ask_cancellation_and_abandonment_preserve_sends_and_recover_capacity() {
    for cancel in [true, false] {
        let mut fixture = Fixture::new();
        fixture.config.limits.concurrent_requests = 1;
        fixture.config.limits.waiters = 1;
        let mut mock = Mock::new(&fixture.config).await;
        let (database, state) = mock.shared(&fixture);
        let session = database
            .lock()
            .unwrap()
            .register_session(
                Some("reconnect"),
                None,
                "Same",
                None,
                unix_millis().unwrap(),
            )
            .unwrap();
        let (ended, mut endings) = mpsc::channel(4);
        let (mut http, endpoint) = fixture
            .serve_endpoint(
                Arc::new(Observed {
                    state: state.clone(),
                    ended,
                }),
                None,
            )
            .await;
        let (transport, _) = http.initialize(LEGACY_VERSION).await;
        // Session saturation must not block modern stateless discovery.
        assert_eq!(
            status(http.legacy_post(legacy::initialization(LEGACY_VERSION), None)).await,
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(status(http.discover()).await, StatusCode::OK);
        let body = json!({"jsonrpc":"2.0","id":101,"method":"tools/call",
            "params":{"name":"telegram_ask","arguments":{"session_key":session.session_key,"text":"Question"}}});
        let response = if cancel {
            Either::Left(
                http.legacy_post(body, Some(&transport))
                    .send()
                    .await
                    .unwrap(),
            )
        } else {
            // Direct HTTP body drop fixes the abandonment instant independently
            // of socket scheduling before advancing the controlled clock.
            Either::Right(
                endpoint
                    .handle(
                        Request::builder()
                            .method("POST")
                            .uri("/mcp")
                            .header(header::HOST, http.address.to_string())
                            .header(header::CONTENT_TYPE, "application/json")
                            .header(header::ACCEPT, "application/json, text/event-stream")
                            .header("mcp-session-id", &transport)
                            .header("mcp-protocol-version", LEGACY_VERSION)
                            .body(Full::new(Bytes::from(body.to_string())))
                            .unwrap(),
                    )
                    .await,
            )
        };
        assert_eq!(
            match &response {
                Either::Left(response) => response.status(),
                Either::Right(response) => response.status(),
            },
            StatusCode::OK
        );
        let respond = if cancel {
            Some(mock.next().await) // Hold an admitted send across explicit cancellation.
        } else {
            // Complete Telegram's shorter network deadline before advancing the
            // transport abandonment clock; the question remains waiting for a reply.
            mock.accept(&database, 7).await;
            None
        };
        assert_eq!(endpoint.inner.admission.available_permits(), 0);
        // Observe actual HTTP retirement, including socket disconnect in the
        // explicit-cancellation case. SDK work remains charged independently.
        drop(response);
        let released = tokio::time::timeout(
            Duration::from_secs(2),
            endpoint.inner.admission.clone().acquire_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        drop(released);
        if cancel {
            assert_eq!(status(http.legacy_post(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":101}}), Some(&transport))).await, StatusCode::ACCEPTED);
        } else {
            tokio::time::pause();
            tokio::time::advance(
                crate::config::MCP_SESSION_ABANDONED + crate::config::MCP_SESSION_SWEEP,
            )
            .await;
        }
        // The adapter's abandonment path also includes the SDK's response-drain grace.
        tokio::time::timeout(Duration::from_secs(10), endings.recv())
            .await
            .unwrap()
            .unwrap();
        drop(
            database
                .lock()
                .unwrap()
                .inbox_notifications
                .waiter(session.session_key.clone())
                .unwrap(),
        );
        if !cancel {
            tokio::time::resume();
        }
        if let Some(respond) = respond {
            let changed = database.lock().unwrap().route_changes();
            let committed = changed.notified();
            tokio::pin!(committed);
            committed.as_mut().enable();
            respond.send(sent(7).1).unwrap();
            tokio::time::timeout(Duration::from_secs(2), committed)
                .await
                .unwrap();
        }
        if cancel {
            assert_eq!(
                status(http.session_operation(http::Method::DELETE, &transport)).await,
                StatusCode::ACCEPTED
            );
        }
        tokio::time::timeout(
            Duration::from_secs(2),
            endpoint.inner.sessions.wait_reclaimed(1),
        )
        .await
        .unwrap();
        let (replacement, _) = http.initialize(LEGACY_VERSION).await;
        assert_ne!(replacement, transport);
        assert_eq!(
            http.register(json!({"external_session_id":"reconnect","title":"Same"}))
                .await["session_key"],
            session.session_key
        );
        ingest(
            &IngressService::new(&fixture.config).unwrap(),
            &database,
            11,
            7,
        )
        .unwrap();
        let recovered = Client::Legacy(&replacement)
            .success(
                &http,
                "telegram_inbox",
                json!({"session_key":session.session_key}),
            )
            .await;
        assert_eq!(recovered["replies"][0]["telegram_update_id"], 11);
        assert_eq!(
            recovered["replies"][0]["processing_started_at"],
            Value::Null
        );
        http.finish().await;
        state.stop();
        state.drain().await.unwrap();
    }
}
