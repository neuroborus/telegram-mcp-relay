//! Actual modern HTTP subscriptions, backed by the shared SQLite commit path.

use super::*;
use crate::{
    clients::telegram::{TelegramClient, Update},
    repositories::{IngestOutcome, RouteKind},
    services::{ingress::IngressService, unix_millis},
};
use std::sync::Mutex;

fn shared(fixture: &Fixture) -> (Arc<Mutex<Database>>, Arc<McpHandler>) {
    let database = Arc::new(Mutex::new(
        Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
    ));
    let handler = Arc::new(
        McpHandler::new(
            database.clone(),
            TelegramClient::for_test(&fixture.config, "http://127.0.0.1:1/botSynthetic/".into())
                .unwrap(),
            &fixture.config,
        )
        .unwrap(),
    );
    (database, handler)
}

fn route(database: &Arc<Mutex<Database>>, key: &str, parent: i64) {
    let mut database = database.lock().unwrap();
    let reservation = database.reserve_route(key).unwrap();
    let now = unix_millis().unwrap();
    database
        .persist_route(reservation, -101, parent, RouteKind::Text, now, now)
        .unwrap();
}

fn update(id: i64, parent: i64) -> Update {
    serde_json::from_value(json!({"update_id":id,"message":{
        "message_id":id+100,"date":1,"chat":{"id":-101},"from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":parent},"text":"synthetic reply"}}))
    .unwrap()
}

fn ingest(
    ingress: &IngressService,
    database: &Arc<Mutex<Database>>,
    id: i64,
    parent: i64,
) -> Result<IngestOutcome, AppError> {
    let now = unix_millis().unwrap();
    ingress.ingest(&mut database.lock().unwrap(), &update(id, parent), now)
}

struct Events {
    response: reqwest::Response,
    pending: Vec<u8>,
}

impl Events {
    async fn initial(&mut self, mut uris: Vec<&str>) {
        uris.sort_unstable();
        for uri in uris {
            self.updated(uri).await;
        }
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(end) = self.pending.windows(2).position(|w| w == b"\n\n") {
                    let frame: Vec<_> = self.pending.drain(..end + 2).collect();
                    let frame = std::str::from_utf8(&frame).unwrap();
                    if let Some(data) = frame.lines().find_map(|line| line.strip_prefix("data: ")) {
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

    async fn updated(&mut self, uri: &str) {
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
        assert!(event["params"].get("text").is_none());
    }
}

impl Http {
    async fn listen(&self, filter: Value, accepted: Value) -> Events {
        let mut request = message("subscriptions/listen");
        request["params"]["notifications"] = filter;
        let response = self.post(request).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let mut stream = Events {
            response,
            pending: Vec::new(),
        };
        let ack = stream.next().await;
        assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
        assert_eq!(ack["params"]["notifications"], accepted);
        assert_eq!(
            ack["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            7
        );
        stream
    }
}

#[tokio::test]
async fn accepted_filters_fan_out_only_authorized_committed_inboxes() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 8;
    let (database, state) = shared(&fixture);
    let http = fixture.serve(state, None).await;
    let a = http.register(json!({"title":"Same"})).await;
    let b = http.register(json!({"title":"Same"})).await;
    let uri = a["inbox_uri"].as_str().unwrap();
    let other = b["inbox_uri"].as_str().unwrap();
    let key = a["session_key"].as_str().unwrap();
    route(&database, key, 1);
    route(&database, b["session_key"].as_str().unwrap(), 2);
    let missing = format!("telegram-bridge://sessions/{}/inbox", "0".repeat(64));
    let mut single = http
        .listen(
            json!({"resourceSubscriptions":[uri,uri,missing,
        format!("{uri}?limit=1"),"telegram-bridge://sessions/{session_key}/inbox","private-uri"],
        "toolsListChanged":true,"resourcesListChanged":true}),
            json!({"resourceSubscriptions":[uri,uri]}),
        )
        .await;
    // Duplicated accepted entries produce one initial invalidation per inbox.
    single.updated(uri).await;
    let mut both = http
        .listen(
            json!({"resourceSubscriptions":[uri,other]}),
            json!({"resourceSubscriptions":[uri,other]}),
        )
        .await;
    both.initial(vec![uri, other]).await;
    let ingress = IngressService::new(&fixture.config).unwrap();
    for (id, parent) in [(11, 2), (12, 1)] {
        assert!(matches!(
            ingest(&ingress, &database, id, parent).unwrap(),
            IngestOutcome::ReplyStored { .. }
        ));
    }
    single.updated(uri).await;
    both.updated(other).await;
    both.updated(uri).await;
    assert_eq!(
        http.snapshot(uri).await["replies"][0]["telegram_update_id"],
        12
    );
    assert_eq!(
        http.snapshot(other).await["replies"][0]["telegram_update_id"],
        11
    );

    // A storage failure after INSERT but before cursor commit must publish nothing.
    let mut observer = database
        .lock()
        .unwrap()
        .inbox_notifications
        .subscribe(vec![key.into()])
        .unwrap();
    let connection = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_cursor BEFORE UPDATE ON telegram_cursor
        BEGIN SELECT RAISE(ABORT, 'synthetic'); END;",
        )
        .unwrap();
    assert!(ingest(&ingress, &database, 13, 1).is_err());
    assert!(matches!(
        observer.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert_eq!(database.lock().unwrap().update_cursor().unwrap(), Some(12));
    connection
        .execute_batch("DROP TRIGGER reject_cursor")
        .unwrap();
    ingest(&ingress, &database, 13, 1).unwrap();
    assert_eq!(observer.try_recv().unwrap(), key);
    single.updated(uri).await;
    both.updated(uri).await;
    assert_eq!(
        http.snapshot(uri).await["replies"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(matches!(
        ingest(&ingress, &database, 13, 1).unwrap(),
        IngestOutcome::Duplicate
    ));
    ingest(&ingress, &database, 14, 99).unwrap();
    let mut unauthorized = update(15, 1);
    unauthorized
        .message
        .as_mut()
        .unwrap()
        .from
        .as_mut()
        .unwrap()
        .id = 99;
    ingress
        .ingest(
            &mut database.lock().unwrap(),
            &unauthorized,
            unix_millis().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        observer.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));

    // Ownership changes invalidate the same resource, independently of reply arrival.
    let claimed = http
        .tool("telegram_inbox", json!({"session_key":key,"limit":1}))
        .await;
    let claim = &claimed["structuredContent"]["replies"][0];
    let mut args =
        json!({"session_key":key,"reply_id":claim["reply_id"],"claim_token":claim["claim_token"]});
    single.updated(uri).await;
    both.updated(uri).await;
    assert_eq!(observer.try_recv().unwrap(), key);
    assert_eq!(claimed["isError"], false);
    args["active"] = true.into();
    for (name, arguments, changed) in [
        ("telegram_processing", args.clone(), true),
        ("telegram_processing", args.clone(), false),
        (
            "telegram_processing",
            json!({"active":true,"session_key":key,"reply_id":claim["reply_id"],"claim_token":"stale"}),
            false,
        ),
        (
            "telegram_ack",
            json!({"session_key":key,"reply_id":claim["reply_id"],"claim_token":claim["claim_token"]}),
            true,
        ),
        (
            "telegram_ack",
            json!({"session_key":key,"reply_id":claim["reply_id"],"claim_token":claim["claim_token"]}),
            false,
        ),
    ] {
        let stale = arguments["claim_token"] == "stale";
        assert_eq!(http.tool(name, arguments).await["isError"], stale);
        if changed {
            single.updated(uri).await;
            both.updated(uri).await;
            assert_eq!(observer.try_recv().unwrap(), key);
        } else {
            assert!(matches!(
                observer.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
        }
    }
}

struct Observed {
    state: Arc<McpHandler>,
    finished: mpsc::Sender<()>,
    gate: Arc<Semaphore>,
}

struct Finished(mpsc::Sender<()>);
impl Drop for Finished {
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
    fn accepted_subscription_filter(
        &self,
        filter: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.state.accepted_subscription_filter(filter)
    }
    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        let _finished = Finished(self.finished.clone());
        self.gate.acquire().await.unwrap().forget();
        self.state.listen(context).await
    }
}

#[tokio::test]
async fn listener_capacity_disconnect_overflow_and_restart_preserve_durable_replies() {
    let mut fixture = Fixture::new();
    fixture.config.limits.concurrent_requests = 8;
    fixture.config.limits.subscriptions = 2;
    fixture.config.limits.subscription_queue = 1;
    let (database, state) = shared(&fixture);
    let session = database
        .lock()
        .unwrap()
        .register_session(Some("thread"), None, "Same", unix_millis().unwrap())
        .unwrap();
    route(&database, &session.session_key, 1);
    let other = database
        .lock()
        .unwrap()
        .register_session(None, None, "Same", unix_millis().unwrap())
        .unwrap();
    route(&database, &other.session_key, 2);
    let uri = session.inbox_uri();
    let other_uri = other.inbox_uri();
    let filter = json!({"resourceSubscriptions":[uri,other_uri]});
    let (finished, mut ended) = mpsc::channel(8);
    let gate = Arc::new(Semaphore::new(0));
    let mut http = fixture
        .serve(
            Arc::new(Observed {
                state,
                finished,
                gate: gate.clone(),
            }),
            None,
        )
        .await;
    let mut stream = http.listen(filter.clone(), filter.clone()).await;
    let ingress = IngressService::new(&fixture.config).unwrap();
    // The SDK has acknowledged, but the real listener has not armed its interest yet.
    ingest(&ingress, &database, 10, 1).unwrap();
    gate.add_permits(8);
    stream.initial(vec![&uri, &other_uri]).await;
    let held = database
        .lock()
        .unwrap()
        .inbox_notifications
        .subscribe(vec![session.session_key.clone()])
        .unwrap();
    let mut rejected = http.listen(filter.clone(), filter.clone()).await;
    let error = rejected.next().await;
    assert_eq!(error["error"]["code"], -32603);
    assert!(!error.to_string().contains(&session.session_key));
    ended.recv().await.unwrap();
    drop(rejected);
    drop(held);
    drop(stream);
    // A completion guard observes actual retirement, without polling or sleeps.
    ended.recv().await.unwrap();
    let mut stream = http.listen(filter.clone(), filter.clone()).await;
    stream.initial(vec![&uri, &other_uri]).await;
    // Distinct inbox commits occur before the listener can drain its one-slot queue.
    for (id, parent) in [(11, 1), (12, 2)] {
        ingest(&ingress, &database, id, parent).unwrap();
    }
    let complete = stream.next().await;
    assert_eq!(
        complete["result"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        7
    );
    ended.recv().await.unwrap();
    drop(stream);
    http.finish().await;
    // An offline commit is recovered via reads after a fresh handler/database opens.
    ingest(&ingress, &database, 13, 1).unwrap();
    let (_, reopened) = shared(&fixture);
    let http = fixture.serve(reopened, None).await;
    let mut stream = http.listen(filter.clone(), filter).await;
    stream.initial(vec![&uri, &other_uri]).await;
    let snapshot = http.snapshot(&uri).await;
    assert_eq!(snapshot["replies"].as_array().unwrap().len(), 3);
    assert_eq!(
        http.snapshot(&other_uri).await["replies"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        snapshot["replies"]
            .as_array()
            .unwrap()
            .iter()
            .all(|reply| reply["state"] == "pending")
    );
}

#[tokio::test]
async fn empty_unknown_and_oversized_filters_are_not_retained() {
    let mut fixture = Fixture::new();
    fixture.config.limits.subscriptions = 1;
    let (_, state) = shared(&fixture);
    let http = fixture.serve(state, None).await;
    let session = http.register(json!({"title":"Same"})).await;
    let uri = &session["inbox_uri"];
    for filter in [
        json!({}),
        json!({"toolsListChanged":true}),
        json!({"resourceSubscriptions":["private-uri"]}),
        json!({"resourceSubscriptions":[uri,uri]}),
    ] {
        let mut stream = http.listen(filter, json!({})).await;
        let complete = stream.next().await;
        assert_eq!(complete["id"], 7);
        assert_eq!(
            complete["result"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
            7
        );
    }
    let filter = json!({"resourceSubscriptions":[uri]});
    let mut stream = http.listen(filter.clone(), filter).await;
    stream.updated(uri.as_str().unwrap()).await;
}
