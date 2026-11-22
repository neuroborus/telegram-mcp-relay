//! After-send waits reuse claims, deadlines and cancellation without resending.

use super::*;
use crate::{
    config::ProcessingStartAck,
    repositories::{ReplyState, ReplyTarget, RouteKind},
    services::{inbox::InboxService, ingress::IngressService, unix_millis},
};
use ask::Mock;
use futures_util::FutureExt;
use std::sync::Mutex;
use subscriptions::{ingest, route};
use tokio::sync::oneshot;
use tools::{Telegram, protocol_tool, sent};

async fn call(http: &Http, transport: Option<&str>, name: &str, args: Value) -> Value {
    let params = json!({"name":name,"arguments":args});
    let response = if let Some(transport) = transport {
        http.legacy_rpc(transport, "tools/call", params).await
    } else {
        http.rpc("tools/call", params).await
    };
    tool_data(response)
}

fn tool_data(response: Value) -> Value {
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["isError"], false, "{response}");
    response["result"]["structuredContent"].clone()
}

fn assert_tool_result(response: &Value, legacy: bool, data: &Value, is_error: bool) {
    let mut expected = json!({
        "structuredContent":data,"isError":is_error,
        "content":[{"type":"text","text":data.to_string()}]
    });
    if !legacy {
        expected["resultType"] = json!("complete");
    }
    assert_eq!(
        response,
        &json!({"jsonrpc":"2.0","id":response["id"],"result":expected})
    );
}

fn session(database: &Arc<Mutex<Database>>) -> String {
    database
        .lock()
        .unwrap()
        .register_session(None, None, "Synthetic", Some("p"), unix_millis().unwrap())
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
    let (http, endpoint) = fixture.serve_endpoint(state, None).await;
    let (transport, _) = http.initialize(LEGACY_VERSION).await;
    let call = async |transport: Option<&str>, name: &str, args: Value| {
        protocol_tool(&http, &endpoint, transport, name, args).await
    };
    let database = Arc::new(Mutex::new(
        Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
    ));
    let ingress = IngressService::new(&fixture.config).unwrap();
    let path = fixture.directory.join("report.txt");
    fs::write(&path, b"synthetic file").unwrap();
    for (transport, first_id) in [(None, 7), (Some(transport.as_str()), 9)] {
        let registered = call(
            transport,
            "telegram_register",
            json!({"title":"Synthetic","directory_label":"p"}),
        )
        .await;
        let key = &tool_data(registered)["session_key"];
        for (name, args, parent) in [
            (
                "telegram_send",
                json!({"session_key":key,"text":"Status"}),
                first_id,
            ),
            (
                "telegram_send_file",
                json!({"session_key":key,"path":path,"caption":"Report"}),
                first_id + 1,
            ),
        ] {
            let response = call(transport, name, args).await;
            assert_tool_result(
                &response,
                transport.is_some(),
                &json!({"telegram_chat_id":-101,"outbound_message_id":parent}),
                false,
            );
            // This independent connection cannot signal the handler's RAM interests.
            ingest(&ingress, &database, parent + 100, parent).unwrap();
            let reply = tool_data(
                call(
                    transport,
                    "telegram_wait",
                    json!({"session_key":key,"outbound_message_id":parent}),
                )
                .await,
            );
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
async fn opted_in_sends_claim_file_approval_and_leave_late_replies_for_resume() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new();
        let wait_budget = Duration::from_secs(fixture.config.delivery.wait_timeout_seconds);
        let telegram = Telegram::new(&fixture.config, (7..9).map(sent).collect()).await;
        let (database, handler) = telegram.shared(&fixture);
        let key = session(&database);
        let hub = database.lock().unwrap().inbox_notifications.clone();
        let ingress = IngressService::new(&fixture.config).unwrap();
        let path = fixture.directory.join("approval.txt");
        let document = b"synthetic report";
        fs::write(&path, document).unwrap();
        let (mut http, endpoint) = fixture.serve_endpoint(handler, None).await;
        let transport = if legacy {
            Some(http.initialize(LEGACY_VERSION).await.0)
        } else {
            None
        };
        let call = async |name: &str, args: Value| {
            protocol_tool(&http, &endpoint, transport.as_deref(), name, args).await
        };

        let request = |id: i64, name: &str, arguments: Value| {
            let mut body = message("tools/call");
            body["id"] = id.into();
            body["params"]["name"] = name.into();
            body["params"]["arguments"] = arguments;
            if let Some(transport) = &transport {
                body["params"].as_object_mut().unwrap().remove("_meta");
                http.legacy_post(body, Some(transport))
            } else {
                http.post(body)
            }
        };
        let read_response = async |pending: JoinHandle<reqwest::Result<reqwest::Response>>,
                                   id: i64| {
            // Start response deadlines after the reply or clock advance releases the waiter.
            let response = tokio::time::timeout(Duration::from_secs(2), pending)
                .await
                .expect("Combined call must return after the wait")
                .unwrap()
                .unwrap();
            let body = if legacy {
                let body = legacy::sse_message(response).await;
                endpoint
                    .inner
                    .sessions
                    .wait_work_released(endpoint.inner.concurrency as u32)
                    .await;
                body
            } else {
                response.json::<Value>().await.unwrap()
            };
            assert_eq!(body["id"], id);
            body
        };
        let wait_for_admission = async || {
            tokio::time::timeout(Duration::from_secs(2), hub.waiter_admitted.notified())
                .await
                .expect("Combined call must arm its reply waiter");
        };
        let pending = tokio::spawn(
            request(
                200,
                "telegram_send_file",
                json!({"session_key":key,"path":path,"caption":"Please approve","wait_for_reply":true}),
            )
            .send(),
        );
        wait_for_admission().await;
        telegram.assert_document_upload(0, document);
        ingest(&ingress, &database, 10, 7).unwrap();
        let approved = read_response(pending, 200).await;
        let reply = &approved["result"]["structuredContent"]["reply"];
        assert_tool_result(
            &approved,
            legacy,
            &json!({
                "telegram_chat_id":-101,"outbound_message_id":7,"status":"reply_received",
                "reply":reply
            }),
            false,
        );
        assert_eq!(reply["reply_to_bot_message_id"], 7);
        assert_eq!(reply["state"], "claimed");
        assert_eq!(reply["processing_started_at"], Value::Null);
        assert_eq!(reply["claim_token"].as_str().unwrap().len(), 64);

        let pending = tokio::spawn(
            request(
                201,
                "telegram_send",
                json!({"session_key":key,"text":"Ready for review","wait_for_reply":true}),
            )
            .send(),
        );
        wait_for_admission().await;
        tokio::time::pause();
        tokio::time::advance(wait_budget).await;
        tokio::time::resume();
        let timed_out = read_response(pending, 201).await;
        assert_tool_result(
            &timed_out,
            legacy,
            &json!({"telegram_chat_id":-101,"outbound_message_id":8,"status":"wait_timeout"}),
            false,
        );
        ingest(&ingress, &database, 11, 8).unwrap();
        let resumed = tool_data(call("telegram_inbox", json!({"session_key":key})).await);
        let late = resumed["replies"].as_array().unwrap();
        assert_eq!(late.len(), 1);
        assert_eq!(late[0]["reply_to_bot_message_id"], 8);
        assert_eq!(late[0]["state"], "claimed");
        assert_eq!(late[0]["claim_token"].as_str().unwrap().len(), 64);
        assert_eq!(telegram.request_count(), 2);
        http.finish().await;
    }
}

#[tokio::test]
async fn opted_in_sends_report_disabled_waits_and_recover_post_send_admission_failure() {
    for enabled in [false, true] {
        for legacy in [false, true] {
            let mut fixture = Fixture::new();
            fixture.config.delivery.wait_fallback_enabled = enabled;
            fixture.config.limits.waiters = 1;
            let telegram = Telegram::new(&fixture.config, (7..9).map(sent).collect()).await;
            let (database, handler) = telegram.shared(&fixture);
            let key = session(&database);
            let hub = database.lock().unwrap().inbox_notifications.clone();
            let occupied = hub.waiter(key.clone()).unwrap();
            let (mut http, endpoint) = fixture.serve_endpoint(handler, None).await;
            let transport = if legacy {
                Some(http.initialize(LEGACY_VERSION).await.0)
            } else {
                None
            };
            let call = async |name: &str, args: Value| {
                protocol_tool(&http, &endpoint, transport.as_deref(), name, args).await
            };
            let one_way = call(
                "telegram_send",
                json!({"session_key":key,"text":"One-way status","wait_for_reply":false}),
            )
            .await;
            assert_tool_result(
                &one_way,
                legacy,
                &json!({"telegram_chat_id":-101,"outbound_message_id":7,"status":"disabled"}),
                false,
            );
            let response = call(
                "telegram_send",
                json!({"session_key":key,"text":"Please review","wait_for_reply":true}),
            )
            .await;
            drop(occupied);
            if enabled {
                let error = &response["result"]["structuredContent"];
                assert_tool_result(&response, legacy, error, true);
                assert_eq!(error["code"], "SEND_WAIT_FAILED");
                assert_eq!(
                    error["details"],
                    json!({"telegram_chat_id":-101,"outbound_message_id":8})
                );
                ingest(
                    &IngressService::new(&fixture.config).unwrap(),
                    &database,
                    10,
                    8,
                )
                .unwrap();
                let recovered = tool_data(
                    call(
                        "telegram_wait",
                        json!({"session_key":key,"outbound_message_id":error["details"]["outbound_message_id"]}),
                    )
                    .await,
                );
                assert_eq!(recovered["status"], "reply_received");
                assert_eq!(recovered["reply"]["reply_to_bot_message_id"], 8);
            } else {
                assert_tool_result(
                    &response,
                    legacy,
                    &json!({"telegram_chat_id":-101,"outbound_message_id":8,"status":"disabled"}),
                    false,
                );
            }
            assert_eq!(telegram.request_count(), 2);
            http.finish().await;
        }
    }
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
        let _ended = if matches!(request.name.as_ref(), "telegram_wait" | "telegram_send") {
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
        // The held Telegram response is a threaded text start acknowledgment.
        fixture.config.delivery.processing_start_ack = ProcessingStartAck::Message;
        let mut mock = Mock::new(&fixture.config).await;
        let (database, state) = mock.shared(&fixture);
        let key = session(&database);
        for parent in [6, 7] {
            route(&database, &key, parent);
        }
        let ingress = IngressService::new(&fixture.config).unwrap();
        ingest(&ingress, &database, 10, 6).unwrap();
        ingest(&ingress, &database, 11, 6).unwrap();
        let claims = InboxService::new(&fixture.config.delivery)
            .unwrap()
            .claim(
                &mut database.lock().unwrap(),
                &key,
                None,
                2,
                unix_millis().unwrap(),
            )
            .unwrap();
        let [claim, other] = claims.as_slice() else {
            panic!("Expected two claims");
        };
        let observed = Arc::new(Observed {
            state,
            started: tokio::sync::Notify::new(),
            ended: tokio::sync::Notify::new(),
        });
        let (mut http, endpoint) = fixture.serve_endpoint(observed.clone(), None).await;
        let transport = if legacy {
            Some(http.initialize(LEGACY_VERSION).await.0)
        } else {
            None
        };
        let request = |id: i64, name: &str, arguments: Value| {
            let mut body = message("tools/call");
            body["id"] = id.into();
            body["params"]["name"] = name.into();
            body["params"]["arguments"] = arguments;
            if let Some(transport) = &transport {
                body["params"].as_object_mut().unwrap().remove("_meta");
                http.legacy_post(body, Some(transport))
            } else {
                http.post(body)
            }
        };
        let wait = request(
            200,
            "telegram_wait",
            json!({"session_key":key,"outbound_message_id":7}),
        );
        let pending = tokio::spawn(wait.send());
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
        let rejected = http.client.execute(forged).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
        // The rejected response holds the HTTP recovery permit until its body retires.
        rejected.bytes().await.unwrap();
        let mut action = json!({"session_key":key,"reply_id":claim.reply.reply_id,"claim_token":claim.claim_token,"active":true});
        let processing = tokio::spawn(request(201, "telegram_processing", action.clone()).send());
        let respond = mock.next().await;
        // Both the wait and the first processing call remain outstanding during recovery.
        tokio::time::timeout(Duration::from_secs(2), async {
            action["active"] = false.into();
            call(
                &http,
                transport.as_deref(),
                "telegram_processing",
                action.clone(),
            )
            .await;
            endpoint.inner.sessions.wait_recovery_released().await;
            let completed = call(
                &http,
                transport.as_deref(),
                "telegram_ack",
                json!({"session_key":key,"reply_id":other.reply.reply_id,"claim_token":other.claim_token}),
            )
            .await;
            assert_eq!(completed["acknowledged"], true);
            endpoint.inner.sessions.wait_recovery_released().await;
        })
        .await
        .expect("Recovery calls must complete while waiting for the Telegram response");
        respond.send(sent(8).1).unwrap();
        // Start the legacy SSE read deadline only after the deliberate Telegram hold.
        let response = processing.await.unwrap().unwrap();
        let started = if legacy {
            legacy::sse_message(response).await
        } else {
            response.json().await.unwrap()
        };
        assert_eq!(tool_data(started)["start_acknowledgment"], "sent");
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
        ingest(&ingress, &database, 12, 7).unwrap();
        // Start the response-read deadline only after releasing the held waiter.
        let response = pending.await.unwrap().unwrap();
        let result = if legacy {
            legacy::sse_message(response).await
        } else {
            response.json::<Value>().await.unwrap()
        };
        assert_eq!(
            result["result"]["structuredContent"]["status"],
            "reply_received"
        );
        mock.assert_idle();
        http.finish().await;
    }
}

#[tokio::test]
async fn legacy_cancelled_combined_upload_releases_work_before_delivery() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 1;
    let (upload_arrived, uploaded) = oneshot::channel();
    let (release_upload, held) = oneshot::channel();
    let telegram = Telegram::with_responses(
        &fixture.config,
        [async move {
            upload_arrived.send(()).unwrap();
            held.await.unwrap();
            sent(7)
        }],
    )
    .await;
    let (database, handler) = telegram.shared(&fixture);
    let key = session(&database);
    let hub = database.lock().unwrap().inbox_notifications.clone();
    let path = fixture.directory.join("approval.txt");
    let document = b"synthetic report";
    fs::write(&path, document).unwrap();
    let (mut http, endpoint) = fixture.serve_endpoint(handler, None).await;
    let (transport, _) = http.initialize(LEGACY_VERSION).await;
    let upload = http.legacy_post(
        json!({"jsonrpc":"2.0","id":101,"method":"tools/call","params":{
            "name":"telegram_send_file","arguments":{
                "session_key":key,"path":path,"caption":"Please approve","wait_for_reply":true
            }
        }}),
        Some(&transport),
    );
    let response = tokio::time::timeout(Duration::from_secs(2), upload.send())
        .await
        .expect("Legacy request must open its SSE response during delivery")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(2), uploaded)
        .await
        .expect("Telegram must receive the document before cancellation")
        .unwrap();
    telegram.assert_document_upload(0, document);
    assert_eq!(endpoint.inner.admission.available_permits(), 0);
    assert_eq!(
        status(http.legacy_post(
            json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":101}}),
            Some(&transport),
        ))
        .await,
        StatusCode::ACCEPTED
    );
    drop(response);
    endpoint.inner.sessions.wait_work_released(1).await;

    let changed = database.lock().unwrap().route_changes();
    let committed = changed.notified();
    tokio::pin!(committed);
    committed.as_mut().enable();
    release_upload.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), committed)
        .await
        .expect("Admitted upload must finish bookkeeping after cancellation");
    let route = database
        .lock()
        .unwrap()
        .route(-101, 7, unix_millis().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(route.session_key, key);
    assert_eq!(route.kind, RouteKind::Document);
    assert!(hub.waiter_admitted.notified().now_or_never().is_none());
    assert_eq!(telegram.request_count(), 1);
    http.finish().await;
}

#[tokio::test]
async fn disconnected_waits_retire_interest_and_keep_events_and_late_replies() {
    for combined in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.limits.concurrent_requests = 4;
        fixture.config.limits.waiters = 1;
        let mut mock = Mock::new(&fixture.config).await;
        let (database, state) = mock.shared(&fixture);
        let key = session(&database);
        let hub = database.lock().unwrap().inbox_notifications.clone();
        let (name, args) = if combined {
            (
                "telegram_send",
                json!({"session_key":key,"text":"Please review","wait_for_reply":true}),
            )
        } else {
            route(&database, &key, 7);
            (
                "telegram_wait",
                json!({"session_key":key,"outbound_message_id":7}),
            )
        };
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
        body["params"]["name"] = name.into();
        body["params"]["arguments"] = args;
        let payload = body.to_string();
        let mut socket = TcpStream::connect(http.address).await.unwrap();
        socket.write_all(format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMcp-Method: tools/call\r\nMcp-Name: {name}\r\nMCP-Protocol-Version: {VERSION}\r\nContent-Length: {}\r\n\r\n{payload}", http.address, payload.len()).as_bytes()).await.unwrap();
        if combined {
            tokio::time::timeout(Duration::from_secs(2), mock.accept(&database, 7))
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), hub.waiter_admitted.notified())
            .await
            .expect("Waiter must hold admission before disconnect");
        assert!(matches!(
            hub.waiter(key.clone()),
            Err(AppError::WaitCapacity)
        ));
        drop(socket);
        tokio::time::timeout(Duration::from_secs(2), observed.ended.notified())
            .await
            .unwrap();
        drop(hub.waiter(key.clone()).unwrap());
        ingest(
            &IngressService::new(&fixture.config).unwrap(),
            &database,
            11,
            7,
        )
        .unwrap();
        events.updated(&uri).await;
        let page = snapshot(&database, &key);
        assert_eq!(page.replies[0].state, ReplyState::Pending);
        assert_eq!(page.replies[0].reply_to_bot_message_id, 7);
        mock.assert_idle();
    }
}
