//! Subscription, claim and reconnect acceptance through the production composition.

use super::*;
use std::{collections::HashSet, time::Duration};

use super::conversations::{accepted, incoming, snapshot};

pub(super) struct Events {
    response: reqwest::Response,
    pending: Vec<u8>,
}

impl Events {
    async fn listen(address: SocketAddr, uri: &str) -> Self {
        Self::listen_authenticated(address, uri, None).await
    }

    pub(super) async fn listen_authenticated(
        address: SocketAddr,
        uri: &str,
        token: Option<&str>,
    ) -> Self {
        let filter = json!({"resourceSubscriptions":[uri]});
        let mut request = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("http://{address}/mcp"))
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "subscriptions/listen")
            .json(
                &json!({"jsonrpc":"2.0","id":7,"method":"subscriptions/listen",
                "params":{"notifications":filter,"_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities":{}}}}),
            );
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let mut stream = Self {
            response,
            pending: Vec::new(),
        };
        let ack = stream.next().await;
        assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
        assert_eq!(ack["params"]["notifications"], filter);
        assert_eq!(
            ack["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            7
        );
        stream.updated(uri).await; // Listener armed and initial catch-up invalidation delivered.
        stream
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(end) = self.pending.windows(2).position(|w| w == b"\n\n") {
                    let frame: Vec<_> = self.pending.drain(..end + 2).collect();
                    if let Some(data) = std::str::from_utf8(&frame)
                        .unwrap()
                        .lines()
                        .find_map(|line| line.strip_prefix("data: "))
                    {
                        return serde_json::from_str(data).unwrap();
                    }
                } else {
                    self.pending.extend_from_slice(
                        &self.response.chunk().await.unwrap().expect("open SSE"),
                    );
                    assert!(self.pending.len() <= 16_384);
                }
            }
        })
        .await
        .unwrap()
    }

    pub(super) async fn updated(&mut self, uri: &str) {
        let event = self.next().await;
        assert_eq!(
            event["method"], "notifications/resources/updated",
            "{event}"
        );
        assert_eq!(event["params"]["uri"], uri);
        assert_eq!(
            event["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            7
        );
    }
}

async fn begin(fixture: &mut Fixture, mock: &mut Mock) -> (Running, TelegramRequest) {
    let running = fixture.start(mock.client.clone());
    mock.next("getWebhookInfo")
        .await
        .respond(json!({"ok":true,"result":{
        "url":"","pending_update_count":0}}));
    (running, mock.next("getUpdates").await)
}

async fn restart(fixture: &mut Fixture, running: &mut Running, poll: TelegramRequest) {
    running.stop.cancel();
    (&mut running.task).await.unwrap().unwrap();
    drop(poll);
    // Retire the old endpoint, then retain a newly bound listener without port-reuse races.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    fixture.config.server.listen = listener.local_addr().unwrap();
    fixture.listener = Some(listener);
}

fn handle(session: &Value, claim: &Value) -> Value {
    json!({"session_key":session["session_key"],"reply_id":claim["reply_id"],
        "claim_token":claim["claim_token"]})
}

async fn invalid_claim(address: SocketAddr, name: &str, arguments: Value) {
    let result = rpc(
        address,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
    .await;
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["code"], "CLAIM_INVALID");
}

#[tokio::test]
async fn shutdown_drains_legacy_question_and_modern_send_with_a_connected_resource_listener() {
    let mut fixture = Fixture::new().await;
    fixture.config.activity.typing_enabled = false;
    fixture.config.limits.concurrent_requests = 8;
    let mut mock = Mock::new(&fixture.config).await;
    let address = fixture.config.server.listen;
    let (mut running, poll) = begin(&mut fixture, &mut mock).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let post = |body: Value| {
        client
            .post(format!("http://{address}/mcp"))
            .header("accept", "application/json, text/event-stream")
            .json(&body)
    };
    let initialized = post(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"synthetic-shutdown-client","version":"1"}}}),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(initialized.status(), http::StatusCode::OK);
    let transport = initialized.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = initialized.text().await.unwrap();
    let data = body
        .lines()
        .find_map(|line| line.strip_prefix("data: ").filter(|data| !data.is_empty()))
        .unwrap();
    let result: Value = serde_json::from_str(data).unwrap();
    assert_eq!(result["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(fixture.count("sessions"), 0);
    assert_eq!(
        post(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .header("mcp-session-id", &transport)
            .header("mcp-protocol-version", "2025-06-18")
            .send()
            .await
            .unwrap()
            .status(),
        http::StatusCode::ACCEPTED
    );
    let session = tool(
        address,
        "telegram_register",
        json!({"title":"Mixed shutdown"}),
    )
    .await;
    let key = session["session_key"].as_str().unwrap();
    let uri = session["inbox_uri"].as_str().unwrap();
    let events = Events::listen(address, uri).await;
    let older = post(
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"telegram_ask","arguments":{"session_key":key,"text":"Question"}}}),
    )
    .header("mcp-session-id", &transport)
    .header("mcp-protocol-version", "2025-06-18")
    .send()
    .await
    .unwrap();
    assert_eq!(older.status(), http::StatusCode::OK);
    let question = mock.next("sendMessage").await;
    assert_eq!(question.payload["text"], "[Mixed shutdown]\n\nQuestion");
    let send_key = key.to_owned();
    let modern = tokio::spawn(async move {
        tool(
            address,
            "telegram_send",
            json!({"session_key":send_key,"text":"Result"}),
        )
        .await
    });
    let send = mock.next("sendMessage").await;
    assert_eq!(send.payload["text"], "[Mixed shutdown]\n\nResult");
    running.stop.cancel();
    running.retired.cancelled().await;
    assert!(!running.task.is_finished());
    modern.abort();
    assert!(modern.await.unwrap_err().is_cancelled());
    question.respond(accepted(7));
    // The second admitted effect keeps shutdown pending until its own bookkeeping finishes.
    assert!(!running.task.is_finished());
    send.respond(accepted(8));
    tokio::time::timeout(Duration::from_secs(10), &mut running.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop((older, events, poll)); // Connected SSE clients cannot prolong daemon shutdown.
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
    let now = unix_millis().unwrap();
    let mut database = fixture.database();
    for id in [7, 8] {
        assert_eq!(
            database.route(-101, id, now).unwrap().unwrap().session_key,
            key
        );
        IngressService::new(&fixture.config)
            .unwrap()
            .ingest(
                &mut database,
                &serde_json::from_value(incoming(10 + id, id, Some("After shutdown"))).unwrap(),
                now,
            )
            .unwrap();
    }
    let page = database.inbox_snapshot(key, None, 10, now).unwrap();
    assert_eq!(page.replies.len(), 2);
    assert!(
        page.replies
            .iter()
            .all(|reply| reply.state == crate::repositories::ReplyState::Pending)
    );
    assert_eq!(fixture.count("acknowledgment_receipts"), 0);
    assert_eq!(fixture.count("sessions"), 1);
}

#[tokio::test]
async fn subscribed_ask_and_inbox_claims_reconcile_and_recover_missed_acknowledgments() {
    let mut fixture = Fixture::new().await;
    fixture.config.activity.typing_enabled = false;
    fixture.config.limits.waiters = 1;
    fixture.config.limits.subscriptions = 1;
    fixture.config.limits.subscription_queue = 1;
    fixture.config.limits.concurrent_requests = 8;
    fixture.config.limits.inbox_items = 3;
    let mut mock = Mock::new(&fixture.config).await;
    let address = fixture.config.server.listen;
    let (mut running, poll) = begin(&mut fixture, &mut mock).await;
    let session = tool(
        address,
        "telegram_register",
        json!({
        "external_session_id":"recovery-thread","title":"Recovery"}),
    )
    .await;
    let uri = session["inbox_uri"].as_str().unwrap();
    let mut events = Events::listen(address, uri).await;
    let asked = tool(
        address,
        "telegram_ask",
        json!({
        "session_key":session["session_key"],"text":"Question"}),
    );
    tokio::pin!(asked);
    let send = tokio::select! {
        request = mock.next("sendMessage") => request,
        _ = &mut asked => panic!("ask completed before Telegram response"),
    };
    assert_eq!(send.payload["text"], "[Recovery]\n\nQuestion");
    send.respond(accepted(5));
    let updates = vec![
        incoming(10, 5, Some("First")),
        incoming(11, 5, Some("Second")),
        incoming(12, 5, Some("Third")),
    ];
    poll.respond(json!({"ok":true,"result":updates}));
    let poll = mock.next("getUpdates").await;
    assert_eq!(poll.payload["offset"], 13);
    let (asked, a, b) = tokio::join!(
        &mut asked,
        tool(
            address,
            "telegram_inbox",
            json!({"session_key":session["session_key"],"limit":1})
        ),
        tool(
            address,
            "telegram_inbox",
            json!({"session_key":session["session_key"],"limit":1})
        ),
    );
    assert_eq!(asked["status"], "reply_received");
    assert_eq!(asked["outbound_message_id"], 5);
    let claim = &asked["reply"];
    let a = &a["replies"][0];
    let b = &b["replies"][0];
    let ids: HashSet<_> = [claim, a, b]
        .map(|c| c["reply_id"].as_str().unwrap())
        .into();
    let tokens: HashSet<_> = [claim, a, b]
        .map(|c| c["claim_token"].as_str().unwrap())
        .into();
    assert_eq!(ids.len(), 3);
    assert_eq!(tokens.len(), 3);
    assert_eq!(fixture.count("replies"), 3);
    events.updated(uri).await; // A waiter never disables the independent event stream.
    let page = snapshot(address, &session).await;
    assert_eq!(page["replies"].as_array().unwrap().len(), 3);
    for reply in page["replies"].as_array().unwrap() {
        assert!(ids.contains(reply["reply_id"].as_str().unwrap()));
        assert_eq!(reply["state"], "claimed");
        assert!(reply.get("claim_token").is_none());
    }

    let original = handle(&session, claim);
    let mut start = original.clone();
    start["active"] = true.into();
    let started = tool(address, "telegram_processing", start).await;
    assert!(started["processing_started_at"].is_i64());
    let mut resume = original.clone();
    resume["reconcile_request_id"] = "resume-a".into();
    let mut competing = resume.clone();
    competing["reconcile_request_id"] = "resume-b".into();
    let (left, right) = tokio::join!(
        rpc(
            address,
            "tools/call",
            json!({"name":"telegram_processing","arguments":resume})
        ),
        rpc(
            address,
            "tools/call",
            json!({"name":"telegram_processing","arguments":competing})
        ),
    );
    let (rotated, retry, rejected) = if left["isError"] == false {
        (left["structuredContent"].clone(), resume, right)
    } else {
        (right["structuredContent"].clone(), competing, left)
    };
    assert_eq!(rejected["isError"], true);
    assert_eq!(rejected["structuredContent"]["code"], "CLAIM_INVALID");
    assert_ne!(rotated["claim_token"], claim["claim_token"]);
    assert_eq!(rotated["reply_id"], claim["reply_id"]);
    assert_eq!(
        rotated["processing_started_at"],
        started["processing_started_at"]
    );
    invalid_claim(address, "telegram_ack", original.clone()).await;
    let completed = handle(&session, a);
    // Discard the completion response: recovery must use the receipt, without redoing work.
    tool(address, "telegram_ack", completed.clone()).await;
    let before = snapshot(address, &session).await;
    restart(&mut fixture, &mut running, poll).await;
    drop(events);

    let address = fixture.config.server.listen;
    let (mut running, poll) = begin(&mut fixture, &mut mock).await;
    assert_eq!(poll.payload["offset"], 13);
    let resumed = tool(
        address,
        "telegram_register",
        json!({
        "external_session_id":"recovery-thread","title":"Resumed"}),
    )
    .await;
    assert_eq!(resumed["session_key"], session["session_key"]);
    let mut events = Events::listen(address, uri).await;
    assert_eq!(snapshot(address, &session).await, before);
    assert_eq!(
        tool(
            address,
            "telegram_inbox",
            json!({"session_key":session["session_key"]})
        )
        .await["replies"],
        json!([])
    );
    assert_eq!(tool(address, "telegram_processing", retry).await, rotated);
    let mut stale_start = original.clone();
    stale_start["active"] = true.into();
    invalid_claim(address, "telegram_processing", stale_start).await;
    invalid_claim(address, "telegram_ack", original.clone()).await;
    assert_eq!(
        tool(address, "telegram_ack", completed).await,
        json!({"reply_id":a["reply_id"],"acknowledged":true})
    );
    // Telegram may redeliver an already committed update, including a completed reply.
    poll.respond(json!({"ok":true,"result":updates}));
    let poll = mock.next("getUpdates").await;
    assert_eq!(poll.payload["offset"], 13);
    assert_eq!(snapshot(address, &session).await, before);
    assert_eq!(fixture.count("replies"), 2);
    assert_eq!(fixture.count("acknowledgment_receipts"), 1);
    let mut current = original;
    current["claim_token"] = rotated["claim_token"].clone();
    tool(address, "telegram_ack", current).await;
    tool(address, "telegram_ack", handle(&session, b)).await;
    events.updated(uri).await;
    assert_eq!(snapshot(address, &session).await["replies"], json!([]));
    assert_eq!(fixture.count("acknowledgment_receipts"), 3);
    assert_eq!(fixture.count("notice_attempts"), 0);
    running.stop.cancel();
    (&mut running.task).await.unwrap().unwrap();
    drop(poll);
    drop(events);
    assert!(mock.requests.try_recv().is_err());
}

#[tokio::test]
async fn committed_reply_without_a_wakeup_or_next_poll_recovers_after_restart() {
    let mut fixture = Fixture::new().await;
    let mut mock = Mock::new(&fixture.config).await;
    let address = fixture.config.server.listen;
    let (mut running, poll) = begin(&mut fixture, &mut mock).await;
    assert!(poll.payload.get("offset").is_none());
    let session = tool(address, "telegram_register", json!({"title":"Recovery"})).await;
    let key = session["session_key"].as_str().unwrap();
    let uri = session["inbox_uri"].as_str().unwrap();
    let mut events = Events::listen(address, uri).await;
    let update = incoming(10, 5, Some("Recover without notification"));
    // An independent writer has no live interests. This models loss after SQLite commit,
    // before waking this daemon's clients or confirming a later Telegram polling offset.
    let id = {
        let mut db = fixture.database();
        let now = unix_millis().unwrap();
        let reservation = db.reserve_route(key).unwrap();
        db.persist_route(reservation, -101, 5, RouteKind::Text, now, now)
            .unwrap();
        match IngressService::new(&fixture.config)
            .unwrap()
            .ingest(
                &mut db,
                &serde_json::from_value(update.clone()).unwrap(),
                now,
            )
            .unwrap()
        {
            crate::repositories::IngestOutcome::ReplyStored { reply_id, .. } => reply_id,
            other => panic!("expected stored reply, got {other:?}"),
        }
    };
    assert_eq!(fixture.database().update_cursor().unwrap(), Some(10));
    {
        let next = events.next();
        tokio::pin!(next);
        assert!(futures_util::poll!(&mut next).is_pending());
    }
    restart(&mut fixture, &mut running, poll).await;
    drop(events);

    let address = fixture.config.server.listen;
    let (mut running, poll) = begin(&mut fixture, &mut mock).await;
    assert_eq!(poll.payload["offset"], 11);
    let mut events = Events::listen(address, uri).await;
    let page = snapshot(address, &session).await;
    assert_eq!(page["replies"].as_array().unwrap().len(), 1);
    assert_eq!(page["replies"][0]["reply_id"], id);
    assert_eq!(page["replies"][0]["state"], "pending");
    poll.respond(json!({"ok":true,"result":[update]}));
    let poll = mock.next("getUpdates").await;
    assert_eq!(poll.payload["offset"], 11);
    assert_eq!(snapshot(address, &session).await, page);
    let claim = tool(address, "telegram_inbox", json!({"session_key":key})).await;
    assert_eq!(claim["replies"].as_array().unwrap().len(), 1);
    assert_eq!(claim["replies"][0]["reply_id"], id);
    events.updated(uri).await;
    running.stop.cancel();
    (&mut running.task).await.unwrap().unwrap();
    drop(poll);
    drop(events);
    assert_eq!(fixture.count("replies"), 1);
    assert_eq!(fixture.count("acknowledgment_receipts"), 0);
    assert_eq!(fixture.count("notice_attempts"), 0);
}
