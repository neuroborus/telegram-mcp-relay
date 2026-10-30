//! Questions use actual HTTP sends, common durable claims and independent event streams.

use super::*;
use std::sync::Mutex;

use crate::{
    clients::telegram::TelegramClient,
    repositories::ReplyState,
    services::{
        ask::AskService, inbox::InboxService, ingress::IngressService, outbound::OutboundService,
        unix_millis,
    },
};
use subscriptions::{ingest, route};
use tokio::sync::oneshot;
use tools::failure;

struct Mock {
    client: TelegramClient,
    requests: mpsc::Receiver<(Value, oneshot::Sender<Value>)>,
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
        let (requests, receive) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept(), if connections.len() < 4 => {
                        let (socket, _) = accepted.unwrap();
                        let requests = requests.clone();
                        connections.spawn(async move {
                            let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                                let requests = requests.clone();
                                async move {
                                    assert_eq!(request.uri().path(), "/botSynthetic/sendMessage");
                                    let body = Limited::new(request.into_body(), 8_192).collect().await.unwrap().to_bytes();
                                    let (respond, reply) = oneshot::channel::<Value>();
                                    requests.send((serde_json::from_slice(&body).unwrap(), respond)).await.unwrap();
                                    let body = reply.await.unwrap().to_string();
                                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(body))))
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
            requests: receive,
            task,
        }
    }

    async fn next(&mut self) -> oneshot::Sender<Value> {
        let (body, respond) = tokio::time::timeout(Duration::from_secs(2), self.requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(body["chat_id"], -101);
        assert!(body["text"].as_str().unwrap().chars().count() <= 1_000);
        respond
    }

    async fn accept(&mut self, database: &Arc<Mutex<Database>>, id: i64) {
        let respond = self.next().await;
        let changed = database.lock().unwrap().route_changes();
        let committed = changed.notified();
        tokio::pin!(committed);
        committed.as_mut().enable();
        respond.send(sent(id)).unwrap();
        committed.await;
    }

    fn shared(&self, fixture: &Fixture) -> (Arc<Mutex<Database>>, Arc<McpHandler>) {
        let database = Arc::new(Mutex::new(
            Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
        ));
        let state = Arc::new(
            McpHandler::new(database.clone(), self.client.clone(), &fixture.config).unwrap(),
        );
        (database, state)
    }

    fn session(&self, database: &Arc<Mutex<Database>>) -> crate::repositories::Session {
        database
            .lock()
            .unwrap()
            .register_session(None, None, "界", unix_millis().unwrap())
            .unwrap()
    }

    fn asks(&self, fixture: &Fixture, database: Arc<Mutex<Database>>) -> AskService {
        AskService::new(
            database.clone(),
            OutboundService::new(database, self.client.clone(), &fixture.config).unwrap(),
            &fixture.config.delivery,
        )
        .unwrap()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn sent(id: i64) -> Value {
    json!({"ok":true,"result":{"message_id":id,"date":unix_millis().unwrap()/1000,"chat":{"id":-101}}})
}

#[tokio::test]
async fn shutdown_before_wait_recheck_preserves_late_reply_without_claiming_or_acknowledging() {
    let fixture = Fixture::new();
    let mut mock = Mock::new(&fixture.config).await;
    let (database, _) = mock.shared(&fixture);
    let session = mock.session(&database);
    let asks = mock.asks(&fixture, database.clone());
    let asked = asks.ask(&session.session_key, "Question");
    tokio::pin!(asked);
    assert!(futures_util::poll!(&mut asked).is_pending());
    mock.accept(&database, 7).await;
    // Shutdown can race the first wait recheck after the admitted send commits.
    database.lock().unwrap().inbox_notifications.close();
    let ingress = IngressService::new(&fixture.config).unwrap();
    ingest(&ingress, &database, 11, 7).unwrap();
    assert!(matches!(
        asked.await,
        Err(AppError::AskWaitFailed {
            telegram_chat_id: -101,
            outbound_message_id: 7
        })
    ));
    let db = database.lock().unwrap();
    let replies = db
        .inbox_snapshot(&session.session_key, None, 10, unix_millis().unwrap())
        .unwrap();
    assert_eq!(replies.replies.len(), 1);
    assert_eq!(replies.replies[0].state, ReplyState::Pending);
    assert_eq!(replies.replies[0].processing_started_at, None);
    assert!(mock.requests.try_recv().is_err());
}

#[tokio::test]
async fn handler_shutdown_drains_an_active_ask_and_resource_listener() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 4;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let http = fixture.serve(state.clone(), None).await;
    let session = http.register(json!({"title":"Shutdown"})).await;
    let key = session["session_key"].as_str().unwrap();
    let uri = session["inbox_uri"].as_str().unwrap();
    let filter = json!({"resourceSubscriptions":[uri]});
    let mut events = http.listen(filter.clone(), filter).await;
    events.updated(uri).await; // Listener setup completed before shutdown.
    let asked = http.start_ask(key, "Question");
    mock.accept(&database, 7).await;
    state.stop();
    tokio::time::timeout(Duration::from_secs(2), state.drain())
        .await
        .unwrap()
        .unwrap();
    failure(&asked.await.unwrap(), "ASK_WAIT_FAILED");
    // Keep HTTP/SSE clients connected throughout draining; they cannot extend its lifetime.
    drop(events);
    let rejected = http
        .rpc(
            "tools/call",
            json!({"name":"telegram_send",
        "arguments":{"session_key":key,"text":"Late send"}}),
        )
        .await;
    assert_eq!(rejected["error"]["code"], -32603);
    ingest(
        &IngressService::new(&fixture.config).unwrap(),
        &database,
        11,
        7,
    )
    .unwrap();
    let db = database.lock().unwrap();
    let snapshot = db
        .inbox_snapshot(key, None, 10, unix_millis().unwrap())
        .unwrap();
    assert_eq!(snapshot.replies.len(), 1);
    assert_eq!(snapshot.replies[0].state, ReplyState::Pending);
    assert!(mock.requests.try_recv().is_err());
}

impl Http {
    fn start_ask(&self, key: &str, text: &str) -> JoinHandle<Value> {
        let mut body = message("tools/call");
        body["params"]["name"] = "telegram_ask".into();
        body["params"]["arguments"] = json!({"session_key":key,"text":text});
        let request = self.post(body);
        tokio::spawn(async move {
            let body: Value = request.send().await.unwrap().json().await.unwrap();
            assert!(body.get("error").is_none(), "{body}");
            body["result"].clone()
        })
    }
}

#[tokio::test]
async fn waiter_and_events_share_one_reply_and_inbox_cannot_claim_it_twice() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 8;
    fixture.config.limits.waiters = 1;
    fixture.config.limits.subscriptions = 1;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let http = fixture.serve(state, None).await;
    let session = http.register(json!({"title":"界"})).await;
    let key = session["session_key"].as_str().unwrap();
    let uri = session["inbox_uri"].as_str().unwrap();
    let filter = json!({"resourceSubscriptions":[uri]});
    let mut events = http.listen(filter.clone(), filter).await;
    events.updated(uri).await;
    let asked = http.start_ask(key, &"a".repeat(996));
    let respond = mock.next().await;
    failure(
        &http
            .tool("telegram_ask", json!({"session_key":key,"text":"private"}))
            .await,
        "WAIT_CAPACITY",
    );
    let changed = database.lock().unwrap().route_changes();
    let committed = changed.notified();
    tokio::pin!(committed);
    committed.as_mut().enable();
    respond.send(sent(7)).unwrap();
    committed.await;
    let ingress = IngressService::new(&fixture.config).unwrap();
    ingest(&ingress, &database, 11, 7).unwrap();
    let result = asked.await.unwrap();
    assert_eq!(result["isError"], false, "{result}");
    let result = &result["structuredContent"];
    assert_eq!(result["status"], "reply_received");
    assert_eq!(result["outbound_message_id"], 7);
    let claim = &result["reply"];
    assert_eq!(claim["telegram_update_id"], 11);
    assert_eq!(claim["state"], "claimed");
    assert_eq!(claim["processing_started_at"], Value::Null);
    assert_eq!(claim["claim_token"].as_str().unwrap().len(), 64);
    events.updated(uri).await;
    let snapshot = http.snapshot(uri).await;
    assert_eq!(snapshot["replies"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["replies"][0]["reply_id"], claim["reply_id"]);
    assert_eq!(
        http.tool("telegram_inbox", json!({"session_key":key}))
            .await["structuredContent"]["replies"],
        json!([])
    );
    // Completion is still explicit; repeated incoming updates do not create new work.
    assert!(matches!(
        ingest(&ingress, &database, 11, 7).unwrap(),
        crate::repositories::IngestOutcome::Duplicate
    ));
    assert!(mock.requests.try_recv().is_err());
}

#[tokio::test]
async fn slow_listener_overflow_preserves_a_healthy_stream_waiter_and_claims() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 8;
    fixture.config.limits.subscriptions = 2;
    fixture.config.limits.subscription_queue = 1;
    fixture.config.limits.waiters = 1;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let http = fixture.serve(state, None).await;
    let a = http.register(json!({"title":"Same"})).await;
    let b = http.register(json!({"title":"Same"})).await;
    let key = a["session_key"].as_str().unwrap();
    let other_key = b["session_key"].as_str().unwrap();
    let uri = a["inbox_uri"].as_str().unwrap();
    let other_uri = b["inbox_uri"].as_str().unwrap();
    route(&database, other_key, 6);
    let hub = database.lock().unwrap().inbox_notifications.clone();
    // Retain a real bounded interest without draining it; OS socket buffers cannot
    // decide when this synthetic slow listener saturates.
    let slow = hub.subscribe(vec![key.into(), other_key.into()]).unwrap();
    let filter = json!({"resourceSubscriptions":[uri]});
    let mut healthy = http.listen(filter.clone(), filter).await;
    healthy.updated(uri).await;
    let asked = http.start_ask(key, "Question");
    mock.accept(&database, 7).await;
    let ingress = IngressService::new(&fixture.config).unwrap();
    // No yield between distinct commits: only the undrained two-inbox interest overflows.
    ingest(&ingress, &database, 11, 7).unwrap();
    ingest(&ingress, &database, 12, 6).unwrap();
    assert!(slow.stopped.is_cancelled());
    assert!(hub.subscribe(vec![key.into()]).is_err());
    let result = asked.await.unwrap();
    assert_eq!(result["isError"], false);
    let claim = &result["structuredContent"]["reply"];
    assert_eq!(claim["telegram_update_id"], 11);
    healthy.updated(uri).await;
    assert_eq!(
        http.snapshot(uri).await["replies"][0]["reply_id"],
        claim["reply_id"]
    );
    assert_eq!(
        http.snapshot(other_uri).await["replies"][0]["state"],
        "pending"
    );
    drop(slow);
    // Admission released by retirement can immediately serve a real reconnect/catch-up.
    let filter = json!({"resourceSubscriptions":[other_uri]});
    let mut recovered = http.listen(filter.clone(), filter).await;
    recovered.updated(other_uri).await;
    let (first, second) = tokio::join!(
        http.tool("telegram_inbox", json!({"session_key":other_key})),
        http.tool("telegram_inbox", json!({"session_key":other_key})),
    );
    assert_eq!(first["isError"], false);
    assert_eq!(second["isError"], false);
    let claims: Vec<_> = [first, second]
        .into_iter()
        .flat_map(|r| {
            r["structuredContent"]["replies"]
                .as_array()
                .unwrap()
                .clone()
        })
        .collect();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0]["telegram_update_id"], 12);
    assert_ne!(claims[0]["reply_id"], claim["reply_id"]);
    recovered.updated(other_uri).await;
    let reopened = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    for (key, expected) in [(key, claim), (other_key, &claims[0])] {
        let page = reopened
            .inbox_snapshot(key, None, 10, unix_millis().unwrap())
            .unwrap();
        assert_eq!(page.replies.len(), 1);
        assert_eq!(
            page.replies[0].reply_id,
            expected["reply_id"].as_str().unwrap()
        );
        assert_eq!(page.replies[0].state, ReplyState::Claimed);
        assert_eq!(page.replies[0].processing_started_at, None);
    }
    assert!(mock.requests.try_recv().is_err());
}

#[tokio::test]
async fn initial_recheck_and_competing_inbox_claims_preserve_question_targeting() {
    for inbox_wins in [false, true] {
        let fixture = Fixture::new();
        let mut mock = Mock::new(&fixture.config).await;
        let (database, _) = mock.shared(&fixture);
        let session = mock.session(&database);
        route(&database, &session.session_key, 6);
        let asks = mock.asks(&fixture, database.clone());
        let asked = asks.ask(&session.session_key, "Question");
        tokio::pin!(asked);
        assert!(futures_util::poll!(asked.as_mut()).is_pending());
        mock.accept(&database, 7).await;
        // Do not poll the ask until after receipt. This separate connection has
        // no interests, proving that the initial SQLite check needs no signal.
        let independent = Arc::new(Mutex::new(
            Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
        ));
        let ingress = IngressService::new(&fixture.config).unwrap();
        ingest(&ingress, &independent, 10, 6).unwrap();
        ingest(&ingress, &independent, 11, 7).unwrap();
        let winner = if inbox_wins {
            let claim = InboxService::new(&fixture.config.delivery)
                .unwrap()
                .claim(
                    &mut database.lock().unwrap(),
                    &session.session_key,
                    Some(crate::repositories::ReplyTarget {
                        chat_id: -101,
                        message_id: 7,
                    }),
                    1,
                    unix_millis().unwrap(),
                )
                .unwrap()
                .pop()
                .unwrap();
            assert!(futures_util::poll!(asked.as_mut()).is_pending());
            ingest(&ingress, &database, 12, 7).unwrap();
            Some(claim)
        } else {
            None
        };
        let result = asked.await.unwrap();
        assert!(result.waited);
        let claim = result.reply.unwrap();
        assert_eq!(
            claim.reply.telegram_update_id,
            if inbox_wins { 12 } else { 11 }
        );
        assert_eq!(claim.reply.reply_to_bot_message_id, 7);
        if let Some(winner) = winner {
            assert_eq!(winner.reply.telegram_update_id, 11);
            assert_ne!(winner.reply.reply_id, claim.reply.reply_id);
            assert_ne!(winner.claim_token, claim.claim_token);
        }
        let page = database
            .lock()
            .unwrap()
            .inbox_snapshot(&session.session_key, None, 100, unix_millis().unwrap())
            .unwrap();
        assert_eq!(page.replies.len(), if inbox_wins { 3 } else { 2 });
        assert_eq!(page.replies[0].state, ReplyState::Pending);
        assert!(
            page.replies[1..]
                .iter()
                .all(|r| r.state == ReplyState::Claimed)
        );
    }
}

#[tokio::test]
async fn timeout_keeps_routes_and_late_replies_and_releases_waiter_admission() {
    let mut fixture = Fixture::new();
    fixture.config.limits.waiters = 1;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, _) = mock.shared(&fixture);
    let session = mock.session(&database);
    let asks = mock.asks(&fixture, database.clone());
    let asked = asks.ask(&session.session_key, "Question");
    tokio::pin!(asked);
    assert!(futures_util::poll!(asked.as_mut()).is_pending());
    mock.accept(&database, 7).await;
    tokio::time::pause();
    assert!(futures_util::poll!(asked.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(599)).await;
    assert!(futures_util::poll!(asked.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(1)).await;
    let result = asked.await.unwrap();
    assert!(result.waited);
    assert!(result.reply.is_none());
    tokio::time::resume();
    let hub = database.lock().unwrap().inbox_notifications.clone();
    let _released = hub.waiter(session.session_key.clone()).unwrap();
    let ingress = IngressService::new(&fixture.config).unwrap();
    ingest(&ingress, &database, 11, 7).unwrap();
    let reopened = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    assert!(
        reopened
            .route(-101, 7, unix_millis().unwrap())
            .unwrap()
            .is_some()
    );
    let page = reopened
        .inbox_snapshot(&session.session_key, None, 100, unix_millis().unwrap())
        .unwrap();
    assert_eq!(page.replies[0].state, ReplyState::Pending);
    assert!(mock.requests.try_recv().is_err());
}

#[tokio::test]
async fn disabled_wait_keeps_events_and_invalid_questions_have_zero_effects() {
    let mut fixture = Fixture::new();
    fixture.config.delivery.wait_fallback_enabled = false;
    fixture.config.limits.concurrent_requests = 8;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let http = fixture.serve(state, None).await;
    let session = http.register(json!({"title":"界"})).await;
    let key = session["session_key"].as_str().unwrap();
    let uri = session["inbox_uri"].as_str().unwrap();
    let filter = json!({"resourceSubscriptions":[uri]});
    let mut events = http.listen(filter.clone(), filter).await;
    events.updated(uri).await;
    let before = database.lock().unwrap().session(key).unwrap();
    for (key, text, code) in [
        (key, "a".repeat(997), "TEXT_TOO_LONG"),
        ("private", "private".into(), "SESSION_NOT_FOUND"),
    ] {
        let error = http
            .tool("telegram_ask", json!({"session_key":key,"text":text}))
            .await;
        failure(&error, code);
        assert!(!error.to_string().contains("private"));
    }
    let protocol = http.rpc("tools/call", json!({"name":"telegram_ask","arguments":{"session_key":key,"text":"private","timeout":1}})).await;
    assert_eq!(protocol["error"]["code"], -32602);
    assert!(!protocol.to_string().contains("private"));
    assert_eq!(database.lock().unwrap().session(key).unwrap(), before);
    assert!(mock.requests.try_recv().is_err());
    let asked = http.start_ask(key, "Question");
    mock.next().await.send(sent(7)).unwrap();
    let result = asked.await.unwrap();
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["status"], "sent");
    assert_eq!(result["structuredContent"]["reply"], Value::Null);
    ingest(
        &IngressService::new(&fixture.config).unwrap(),
        &database,
        11,
        7,
    )
    .unwrap();
    events.updated(uri).await;
    assert_eq!(
        http.tool("telegram_inbox", json!({"session_key":key}))
            .await["structuredContent"]["replies"][0]["telegram_update_id"],
        11
    );
}

struct Observed {
    state: Arc<McpHandler>,
    ended: mpsc::Sender<()>,
}

#[tokio::test]
async fn failed_wait_reports_the_committed_question_and_preserves_unclaimed_reply() {
    let mut fixture = Fixture::new();
    fixture.config.limits.waiters = 1;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, state) = mock.shared(&fixture);
    let http = fixture.serve(state, None).await;
    let session = http.register(json!({"title":"界"})).await;
    let key = session["session_key"].as_str().unwrap();
    let asked = http.start_ask(key, "Question");
    mock.accept(&database, 7).await;
    let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_claim BEFORE UPDATE OF claim_token ON replies
        BEGIN SELECT RAISE(ABORT, 'private failure'); END;",
    )
    .unwrap();
    ingest(
        &IngressService::new(&fixture.config).unwrap(),
        &database,
        11,
        7,
    )
    .unwrap();
    let error = asked.await.unwrap();
    failure(&error, "ASK_WAIT_FAILED");
    assert_eq!(
        error["structuredContent"]["details"],
        json!({"telegram_chat_id":-101,"outbound_message_id":7})
    );
    assert!(
        error["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("do not resend automatically")
    );
    assert!(!error.to_string().contains("private"));
    let hub = database.lock().unwrap().inbox_notifications.clone();
    drop(hub.waiter(key.to_owned()).unwrap());
    let snapshot = http.snapshot(session["inbox_uri"].as_str().unwrap()).await;
    assert_eq!(snapshot["replies"][0]["state"], "pending");
    assert!(
        database
            .lock()
            .unwrap()
            .route(-101, 7, unix_millis().unwrap())
            .unwrap()
            .is_some()
    );
    sql.execute_batch("DROP TRIGGER fail_claim").unwrap();
    assert_eq!(
        http.tool("telegram_inbox", json!({"session_key":key}))
            .await["structuredContent"]["replies"][0]["reply_id"],
        snapshot["replies"][0]["reply_id"]
    );
    assert!(mock.requests.try_recv().is_err());
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
    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.state.get_tool(name)
    }
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        self.state.supported_protocol_versions()
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
async fn disconnect_cancels_waiter_but_an_admitted_send_finishes_and_replies_recover() {
    for stage in 0..3 {
        let mut fixture = Fixture::new();
        fixture.config.limits.waiters = 1;
        fixture.config.limits.concurrent_requests = 4;
        fixture.config.limits.subscriptions = 1;
        let mut mock = Mock::new(&fixture.config).await;
        let (database, state) = mock.shared(&fixture);
        let session = mock.session(&database);
        let (ended, mut endings) = mpsc::channel(4);
        let http = fixture
            .serve(Arc::new(Observed { state, ended }), None)
            .await;
        let uri = session.inbox_uri();
        let filter = json!({"resourceSubscriptions":[uri]});
        let mut events = http.listen(filter.clone(), filter).await;
        events.updated(&uri).await;
        let mut body = message("tools/call");
        body["params"]["name"] = "telegram_ask".into();
        body["params"]["arguments"] = json!({"session_key":session.session_key,"text":"Question"});
        let mut socket = TcpStream::connect(http.address).await.unwrap();
        let payload = body.to_string();
        socket.write_all(format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMcp-Method: tools/call\r\nMcp-Name: telegram_ask\r\nMCP-Protocol-Version: {VERSION}\r\nContent-Length: {}\r\n\r\n{payload}", http.address, payload.len()).as_bytes()).await.unwrap();
        let mut respond = Some(mock.next().await);
        let changed = database.lock().unwrap().route_changes();
        let committed = changed.notified();
        tokio::pin!(committed);
        committed.as_mut().enable();
        if stage > 0 {
            respond.take().unwrap().send(sent(7)).unwrap();
            committed.as_mut().await;
        }
        let ingress = IngressService::new(&fixture.config).unwrap();
        if stage == 2 {
            ingest(&ingress, &database, 11, 7).unwrap();
        }
        drop(socket);
        tokio::time::timeout(Duration::from_secs(2), endings.recv())
            .await
            .unwrap()
            .unwrap();
        let hub = database.lock().unwrap().inbox_notifications.clone();
        let _released = hub.waiter(session.session_key.clone()).unwrap();
        if let Some(respond) = respond {
            respond.send(sent(7)).unwrap();
            committed.await;
        }
        if stage < 2 {
            ingest(&ingress, &database, 11, 7).unwrap();
        }
        // Disconnecting an ask releases only its waiter; this subscription stays live.
        events.updated(&uri).await;
        let reopened = Arc::new(Mutex::new(
            Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
        ));
        let snapshot = reopened
            .lock()
            .unwrap()
            .inbox_snapshot(&session.session_key, None, 100, unix_millis().unwrap())
            .unwrap();
        assert_eq!(snapshot.replies.len(), 1);
        let claim = InboxService::new(&fixture.config.delivery)
            .unwrap()
            .claim(
                &mut reopened.lock().unwrap(),
                &session.session_key,
                None,
                1,
                unix_millis().unwrap() + fixture.config.delivery.claim_lease_seconds as i64 * 1_000,
            )
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claim.reply.telegram_update_id, 11);
        assert_eq!(claim.reply.processing_started_at, None);
        assert_eq!(claim.reply.reply_id, snapshot.replies[0].reply_id);
        assert!(mock.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn validation_and_rejected_sends_release_waiter_admission_without_routes_or_replays() {
    let mut fixture = Fixture::new();
    fixture.config.limits.waiters = 1;
    let mut mock = Mock::new(&fixture.config).await;
    let (database, _) = mock.shared(&fixture);
    let session = mock.session(&database);
    let asks = mock.asks(&fixture, database.clone());
    let hub = database.lock().unwrap().inbox_notifications.clone();
    assert!(matches!(
        asks.ask(&session.session_key, &"😀".repeat(997)).await,
        Err(AppError::TextTooLong(_))
    ));
    drop(hub.waiter(session.session_key.clone()).unwrap());
    assert!(mock.requests.try_recv().is_err());
    for code in ["TELEGRAM_REJECTED", "TELEGRAM_DELIVERY_UNCERTAIN"] {
        let asked = asks.ask(&session.session_key, "private question");
        tokio::pin!(asked);
        assert!(futures_util::poll!(asked.as_mut()).is_pending());
        mock.next()
            .await
            .send(if code == "TELEGRAM_REJECTED" {
                json!({"ok":false,"error_code":403,"description":"private rejection"})
            } else {
                json!({"private":"invalid response"})
            })
            .unwrap();
        let error = asked.await.err().unwrap();
        assert_eq!(error.code(), code);
        assert!(!error.to_string().contains("private"));
        drop(hub.waiter(session.session_key.clone()).unwrap());
        assert!(mock.requests.try_recv().is_err());
    }
    let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    assert_eq!(
        sql.query_row("SELECT count(*) FROM message_routes", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
