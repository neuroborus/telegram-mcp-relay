//! After-send waits reuse claims, deadlines and cancellation without Telegram effects.

use super::*;
use crate::{
    repositories::{ReplyState, ReplyTarget, RouteKind},
    services::{inbox::InboxService, ingress::IngressService, unix_millis},
};
use ask::Mock;
use std::sync::Mutex;
use subscriptions::{ingest, route};
use tools::{Telegram, sent};

async fn call(http: &Http, transport: Option<&str>, name: &str, args: Value) -> Value {
    let params = json!({"name":name,"arguments":args});
    let response = if let Some(transport) = transport {
        http.legacy_rpc(transport, "tools/call", params).await
    } else {
        http.rpc("tools/call", params).await
    };
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["isError"], false, "{response}");
    response["result"]["structuredContent"].clone()
}

fn session(database: &Arc<Mutex<Database>>) -> String {
    database
        .lock()
        .unwrap()
        .register_session(None, None, "Synthetic", None, unix_millis().unwrap())
        .unwrap()
        .session_key
}

fn snapshot(database: &Arc<Mutex<Database>>, key: &str) -> crate::repositories::InboxPage {
    database
        .lock()
        .unwrap()
        .inbox_snapshot(key, None, 100, unix_millis().unwrap())
        .unwrap()
}

#[tokio::test]
async fn text_and_file_then_wait_catch_up_without_signals_or_resending_on_both_protocols() {
    let mut fixture = Fixture::new();
    let telegram = Telegram::new(&fixture.config, (7..11).map(sent).collect()).await;
    let state = telegram.handler(&fixture);
    let http = fixture.serve(state, None).await;
    let (transport, _) = http.initialize(LEGACY_VERSION).await;
    let database = Arc::new(Mutex::new(
        Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
    ));
    let ingress = IngressService::new(&fixture.config).unwrap();
    let path = fixture.directory.join("report.txt");
    fs::write(&path, b"synthetic file").unwrap();
    for transport in [None, Some(transport.as_str())] {
        let registered = call(
            &http,
            transport,
            "telegram_register",
            json!({"title":"Synthetic"}),
        )
        .await;
        let key = &registered["session_key"];
        for (name, args) in [
            ("telegram_send", json!({"session_key":key,"text":"Status"})),
            (
                "telegram_send_file",
                json!({"session_key":key,"path":path,"caption":"Report"}),
            ),
        ] {
            let sent = call(&http, transport, name, args).await;
            let parent = sent["outbound_message_id"].as_i64().unwrap();
            // This independent connection cannot signal the handler's RAM interests.
            ingest(&ingress, &database, parent + 100, parent).unwrap();
            let reply = call(
                &http,
                transport,
                "telegram_wait",
                json!({"session_key":key,"outbound_message_id":parent}),
            )
            .await;
            assert_eq!(reply["status"], "reply_received");
            assert_eq!(reply["reply"]["reply_to_bot_message_id"], parent);
            assert_eq!(reply["reply"]["state"], "claimed");
            assert_eq!(reply["reply"]["processing_started_at"], Value::Null);
            assert_eq!(reply["reply"]["claim_token"].as_str().unwrap().len(), 64);
        }
    }
    assert_eq!(telegram.request_count(), 4);
}

#[tokio::test]
async fn armed_wait_shares_ask_admission_and_never_steals_an_unrelated_or_owned_reply() {
    let mut fixture = Fixture::new();
    fixture.config.limits.waiters = 1;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, _) = mock.shared(&fixture);
    let key = session(&database);
    for parent in [6, 7] {
        route(&database, &key, parent);
    }
    let waits = mock.asks(&fixture, database.clone());
    let ingress = IngressService::new(&fixture.config).unwrap();
    ingest(&ingress, &database, 10, 6).unwrap();
    let mut waiting = Box::pin(waits.wait_for_reply(&key, 7));
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    assert!(matches!(
        waits.ask(&key, "Question").await,
        Err(AppError::WaitCapacity)
    ));
    assert!(matches!(
        waits.wait_for_reply(&key, 7).await,
        Err(AppError::WaitCapacity)
    ));
    // Commit after arming and let an inbox consumer win before polling the waiter.
    ingest(&ingress, &database, 11, 7).unwrap();
    let inbox = InboxService::new(&fixture.config.delivery).unwrap();
    let winner = inbox
        .claim(
            &mut database.lock().unwrap(),
            &key,
            Some(ReplyTarget {
                chat_id: -101,
                message_id: 7,
            }),
            1,
            unix_millis().unwrap(),
        )
        .unwrap()
        .pop()
        .unwrap();
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    ingest(&ingress, &database, 12, 7).unwrap();
    let received = waiting.await.unwrap().reply.unwrap();
    assert_eq!(received.reply.telegram_update_id, 12);
    assert_ne!(received.claim_token, winner.claim_token);
    let remaining = inbox
        .claim(
            &mut database.lock().unwrap(),
            &key,
            None,
            100,
            unix_millis().unwrap(),
        )
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].reply.telegram_update_id, 10);
    drop(
        database
            .lock()
            .unwrap()
            .inbox_notifications
            .waiter(key)
            .unwrap(),
    );
    mock.assert_idle();
}

#[tokio::test]
async fn disabled_wait_validates_session_chat_retention_and_ownership_without_claims() {
    let mut fixture = Fixture::new();
    fixture.config.delivery.wait_fallback_enabled = false;
    fixture.config.limits.waiters = 1;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let key = session(&database);
    let foreign = session(&database);
    route(&database, &key, 7);
    route(&database, &foreign, 8);
    for (chat, parent, created) in [(-102, 9, unix_millis().unwrap()), (-101, 10, 1)] {
        let mut db = database.lock().unwrap();
        let reserved = db.reserve_route(&key).unwrap();
        db.persist_route(
            reserved,
            chat,
            parent,
            RouteKind::Document,
            created,
            unix_millis().unwrap(),
        )
        .unwrap();
    }
    ingest(
        &IngressService::new(&fixture.config).unwrap(),
        &database,
        11,
        7,
    )
    .unwrap();
    let waits = mock.asks(&fixture, database.clone());
    for parent in [-1, 0, 8, 9, 10, 99] {
        assert!(matches!(
            waits.wait_for_reply(&key, parent).await,
            Err(AppError::InvalidWaitTarget)
        ));
    }
    assert!(matches!(
        waits.wait_for_reply(&foreign, 7).await,
        Err(AppError::InvalidWaitTarget)
    ));
    assert!(matches!(
        waits.wait_for_reply("unknown", 7).await,
        Err(AppError::SessionNotFound)
    ));
    // Disabled waiting does not need waiter admission and leaves the early reply pending.
    let _occupied = database
        .lock()
        .unwrap()
        .inbox_notifications
        .waiter(key.clone())
        .unwrap();
    let result = waits.wait_for_reply(&key, 7).await.unwrap();
    assert!(!result.waited);
    assert!(result.reply.is_none());
    let http = fixture.serve(state, None).await;
    let result = call(
        &http,
        None,
        "telegram_wait",
        json!({"session_key":key,"outbound_message_id":7}),
    )
    .await;
    assert_eq!(result["status"], "disabled");
    assert_eq!(result["reply"], Value::Null);
    let invalid = http
        .tool(
            "telegram_wait",
            json!({"session_key":key,"outbound_message_id":8}),
        )
        .await;
    tools::failure(&invalid, "WAIT_TARGET_INVALID");
    let malformed = http
        .rpc(
            "tools/call",
            json!({"name":"telegram_wait",
        "arguments":{"session_key":key,"outbound_message_id":7,"timeout":1}}),
        )
        .await;
    assert_eq!(malformed["error"]["code"], -32602);
    let page = snapshot(&database, &key);
    assert_eq!(page.replies[0].state, ReplyState::Pending);
    mock.assert_idle();
}

#[tokio::test]
async fn cancellation_timeout_and_shutdown_preserve_late_reply_and_restart_recovery() {
    for ending in ["cancel", "timeout", "shutdown"] {
        let mut fixture = Fixture::new();
        fixture.config.limits.waiters = 1;
        let mut mock = Mock::new(&fixture.config).await;
        let (database, _) = mock.shared(&fixture);
        let key = session(&database);
        route(&database, &key, 7);
        let waits = mock.asks(&fixture, database.clone());
        let mut waiting = Box::pin(waits.wait_for_reply(&key, 7));
        assert!(futures_util::poll!(waiting.as_mut()).is_pending());
        match ending {
            "cancel" => drop(waiting),
            "timeout" => {
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(599)).await;
                assert!(futures_util::poll!(waiting.as_mut()).is_pending());
                tokio::time::advance(Duration::from_secs(1)).await;
                assert!(waiting.await.unwrap().reply.is_none());
                tokio::time::resume();
            }
            _ => {
                database.lock().unwrap().inbox_notifications.close();
                assert!(matches!(waiting.await, Err(AppError::McpUnavailable)));
            }
        }
        if ending != "shutdown" {
            drop(
                database
                    .lock()
                    .unwrap()
                    .inbox_notifications
                    .waiter(key.clone())
                    .unwrap(),
            );
        }
        ingest(
            &IngressService::new(&fixture.config).unwrap(),
            &database,
            11,
            7,
        )
        .unwrap();
        let reopened = Arc::new(Mutex::new(
            Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
        ));
        let saved = snapshot(&reopened, &key);
        assert_eq!(saved.replies[0].state, ReplyState::Pending);
        let resumed = mock.asks(&fixture, reopened.clone());
        let received = resumed
            .wait_for_reply(&key, 7)
            .await
            .unwrap()
            .reply
            .unwrap();
        assert_eq!(received.reply.reply_id, saved.replies[0].reply_id);
        assert!(
            reopened
                .lock()
                .unwrap()
                .route(-101, 7, unix_millis().unwrap())
                .unwrap()
                .is_some()
        );
        mock.assert_idle();
    }
}

struct Observed {
    state: Arc<McpHandler>,
    started: tokio::sync::Notify,
    ended: tokio::sync::Notify,
}

struct Ended<'a>(&'a tokio::sync::Notify);
impl Drop for Ended<'_> {
    fn drop(&mut self) {
        self.0.notify_one();
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
    fn accepted_subscription_filter(
        &self,
        filter: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.state.accepted_subscription_filter(filter)
    }
    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        self.state.listen(context).await
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
        let _ended = if request.name == "telegram_wait" {
            self.started.notify_one();
            Some(Ended(&self.ended))
        } else {
            None
        };
        self.state.call_tool(request, context).await
    }
}

#[tokio::test]
async fn saturated_waits_leave_processing_ack_and_inbox_capacity_on_both_protocols() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.limits.concurrent_requests = 1;
        fixture.config.limits.waiters = 1;
        fixture.config.activity.typing_enabled = false;
        let mut mock = Mock::new(&fixture.config).await;
        let (database, state) = mock.shared(&fixture);
        let key = session(&database);
        for parent in [6, 7] {
            route(&database, &key, parent);
        }
        let ingress = IngressService::new(&fixture.config).unwrap();
        ingest(&ingress, &database, 10, 6).unwrap();
        let claim = InboxService::new(&fixture.config.delivery)
            .unwrap()
            .claim(
                &mut database.lock().unwrap(),
                &key,
                None,
                1,
                unix_millis().unwrap(),
            )
            .unwrap()
            .pop()
            .unwrap();
        let observed = Arc::new(Observed {
            state,
            started: tokio::sync::Notify::new(),
            ended: tokio::sync::Notify::new(),
        });
        let (http, endpoint) = fixture.serve_endpoint(observed.clone(), None).await;
        let transport = if legacy {
            Some(http.initialize(LEGACY_VERSION).await.0)
        } else {
            None
        };
        let args = json!({"session_key":key,"outbound_message_id":7});
        let mut body = message("tools/call");
        body["id"] = 200.into();
        body["params"]["name"] = "telegram_wait".into();
        body["params"]["arguments"] = args;
        let request = if let Some(transport) = &transport {
            http.legacy_post(
                json!({"jsonrpc":"2.0","id":200,"method":"tools/call",
                "params":{"name":"telegram_wait","arguments":body["params"]["arguments"]}}),
                Some(transport),
            )
        } else {
            http.post(body)
        };
        let pending = tokio::spawn(async move {
            let response = request.send().await.unwrap();
            if legacy {
                legacy::sse_message(response).await
            } else {
                response.json::<Value>().await.unwrap()
            }
        });
        observed.started.notified().await;
        assert_eq!(endpoint.inner.admission.available_permits(), 0);
        // A forged fast-tool header cannot put a send on the reserved recovery lane.
        let mut send = message("tools/call");
        send["params"]["name"] = "telegram_send".into();
        send["params"]["arguments"] = json!({"session_key":key,"text":"Do not send"});
        let mut forged = http.post(send).build().unwrap();
        forged
            .headers_mut()
            .insert("mcp-name", "telegram_ack".parse().unwrap());
        assert_eq!(
            http.client.execute(forged).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        let mut action = json!({"session_key":key,"reply_id":claim.reply.reply_id,"claim_token":claim.claim_token,"active":true});
        for active in [true, false] {
            action["active"] = active.into();
            call(
                &http,
                transport.as_deref(),
                "telegram_processing",
                action.clone(),
            )
            .await;
            endpoint.inner.sessions.wait_recovery_released().await;
        }
        action.as_object_mut().unwrap().remove("active");
        call(&http, transport.as_deref(), "telegram_ack", action).await;
        endpoint.inner.sessions.wait_recovery_released().await;
        assert_eq!(
            call(
                &http,
                transport.as_deref(),
                "telegram_inbox",
                json!({"session_key":key})
            )
            .await["replies"],
            json!([])
        );
        endpoint.inner.sessions.wait_recovery_released().await;
        ingest(&ingress, &database, 11, 7).unwrap();
        let result = pending.await.unwrap();
        assert_eq!(
            result["result"]["structuredContent"]["status"],
            "reply_received"
        );
        mock.assert_idle();
    }
}

#[tokio::test]
async fn disconnected_wait_retires_its_interest_and_keeps_events_and_late_reply() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 4;
    fixture.config.limits.waiters = 1;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let key = session(&database);
    route(&database, &key, 7);
    let observed = Arc::new(Observed {
        state,
        started: tokio::sync::Notify::new(),
        ended: tokio::sync::Notify::new(),
    });
    let http = fixture.serve(observed.clone(), None).await;
    // The real resource stream uses the same shared handler alongside the wait.
    let uri = format!("telegram-bridge://sessions/{key}/inbox");
    let filter = json!({"resourceSubscriptions":[uri]});
    let mut events = http.listen(filter.clone(), filter).await;
    events.updated(&uri).await;
    let mut body = message("tools/call");
    body["params"]["name"] = "telegram_wait".into();
    body["params"]["arguments"] = json!({"session_key":key,"outbound_message_id":7});
    let payload = body.to_string();
    let mut socket = TcpStream::connect(http.address).await.unwrap();
    socket.write_all(format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMcp-Method: tools/call\r\nMcp-Name: telegram_wait\r\nMCP-Protocol-Version: {VERSION}\r\nContent-Length: {}\r\n\r\n{payload}", http.address, payload.len()).as_bytes()).await.unwrap();
    observed.started.notified().await;
    drop(socket);
    tokio::time::timeout(Duration::from_secs(2), observed.ended.notified())
        .await
        .unwrap();
    drop(
        database
            .lock()
            .unwrap()
            .inbox_notifications
            .waiter(key.clone())
            .unwrap(),
    );
    ingest(
        &IngressService::new(&fixture.config).unwrap(),
        &database,
        11,
        7,
    )
    .unwrap();
    events.updated(&uri).await;
    assert_eq!(
        snapshot(&database, &key).replies[0].state,
        ReplyState::Pending
    );
    mock.assert_idle();
}
