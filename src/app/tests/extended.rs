//! Authenticated mixed-protocol acceptance through the one shared production daemon.

use super::*;
use std::{
    collections::HashSet,
    os::unix::fs::MetadataExt,
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    conversations::{accepted, incoming},
    recovery::Events,
};

const AUTH: &str = "synthetic-extended-bearer-0123456789012345";

#[derive(Clone)]
struct Client {
    address: SocketAddr,
    http: reqwest::Client,
    transport: Option<String>,
    ids: Arc<AtomicU64>,
}

impl Client {
    fn modern(address: SocketAddr) -> Self {
        Self {
            address,
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            transport: None,
            ids: Arc::new(AtomicU64::new(10)),
        }
    }

    fn post(&self, value: Value) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .post(format!("http://{}/mcp", self.address))
            .bearer_auth(AUTH)
            .header("accept", "application/json, text/event-stream");
        if let Some(transport) = &self.transport {
            request = request
                .header("mcp-session-id", transport)
                .header("mcp-protocol-version", "2025-06-18");
        } else if value["method"] != "initialize" {
            request = request
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", value["method"].as_str().unwrap());
            if value["method"] == "tools/call" {
                request = request.header("mcp-name", value["params"]["name"].as_str().unwrap());
            } else if value["method"] == "resources/read" {
                request = request.header("mcp-name", value["params"]["uri"].as_str().unwrap());
            }
        }
        request.json(&value)
    }

    async fn legacy(address: SocketAddr) -> Self {
        let mut client = Self::modern(address);
        let response = client
            .post(
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"synthetic-extended-client","version":"1"}}}),
            )
            .send()
            .await
            .unwrap();
        let transport = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        let initialized = decode(response, true).await;
        assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            initialized["result"]["capabilities"],
            json!({"tools":{},"resources":{}})
        );
        client.transport = Some(transport);
        let response = client
            .post(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::ACCEPTED);
        client
    }

    async fn rpc(&self, method: &str, mut params: Value) -> Value {
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        if self.transport.is_none() {
            params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{}});
        }
        let response = self
            .post(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        let body = decode(response, self.transport.is_some()).await;
        assert_eq!(body["id"], id);
        assert!(body.get("error").is_none(), "{body}");
        if method == "tools/call" {
            let result = &body["result"];
            let mut expected = json!({"structuredContent":result["structuredContent"],"isError":result["isError"],
                "content":[{"type":"text","text":serde_json::to_string(&result["structuredContent"]).unwrap()}]});
            assert!(result["isError"].is_boolean());
            if self.transport.is_none() {
                expected["resultType"] = json!("complete");
            }
            assert_eq!(body, json!({"jsonrpc":"2.0","id":id,"result":expected}));
        }
        body["result"].clone()
    }

    async fn tool(&self, name: &str, arguments: Value) -> Value {
        let result = self
            .rpc("tools/call", json!({"name":name,"arguments":arguments}))
            .await;
        assert_eq!(result["isError"], false, "{result}");
        result["structuredContent"].clone()
    }

    async fn failure(&self, name: &str, arguments: Value, code: &str) {
        let result = self
            .rpc("tools/call", json!({"name":name,"arguments":arguments}))
            .await;
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["code"], code);
    }

    async fn snapshot(&self, session: &Value) -> Value {
        let result = self
            .rpc("resources/read", json!({"uri":session["inbox_uri"]}))
            .await;
        serde_json::from_str(result["contents"][0]["text"].as_str().unwrap()).unwrap()
    }
}

async fn decode(response: reqwest::Response, legacy: bool) -> Value {
    assert_eq!(response.status(), http::StatusCode::OK);
    if !legacy {
        return response.json().await.unwrap();
    }
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let text = tokio::time::timeout(std::time::Duration::from_secs(2), response.text())
        .await
        .unwrap()
        .unwrap();
    let messages: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| !data.is_empty())
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert_eq!(messages.len(), 1);
    messages.into_iter().next().unwrap()
}

fn configure(fixture: &mut Fixture) -> Secret {
    for name in ["exports", "downloads"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(fixture.directory.join(name))
            .unwrap();
    }
    for (name, value) in [("bot", "123:synthetic"), ("auth", AUTH)] {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(fixture.directory.join(name))
            .unwrap();
        std::io::Write::write_all(&mut file, value.as_bytes()).unwrap();
    }
    fixture.config.telegram.bot_token_file = Some(fixture.directory.join("bot"));
    fixture.config.server.auth_token_file = Some(fixture.directory.join("auth"));
    fixture.config.files.allowed_roots = vec![fixture.directory.join("exports")];
    fixture.config.downloads.enabled = true;
    fixture.config.downloads.roots = vec![fixture.directory.join("downloads")];
    fixture.config.downloads.max_file_bytes = 1024;
    let unit = rustix::fs::statvfs(&fixture.config.downloads.roots[0])
        .unwrap()
        .f_frsize
        .max(512);
    fixture.config.downloads.max_total_bytes = 1024_u64.div_ceil(unit) * unit * 8;
    fixture.config.downloads.max_files = 8;
    fixture.config.history.enabled = true;
    fixture.config.activity.typing_enabled = false;
    fixture.config.notifications.notify_unconfirmed_processing = false;
    fixture.config.notifications.notify_unroutable = false;
    fixture.config.limits.concurrent_requests = 16;
    fixture.config.limits.waiters = 3;
    fixture.config.load_secrets().unwrap().auth_token.unwrap()
}

async fn begin(
    fixture: &mut Fixture,
    mock: &mut Mock,
    auth: &Secret,
) -> (Running, TelegramRequest) {
    let running = fixture.start_authenticated(mock.client.clone(), Some(auth));
    mock.next("getWebhookInfo")
        .await
        .respond(json!({"ok":true,"result":{"url":"","pending_update_count":0}}));
    (running, mock.next("getUpdates").await)
}

fn recovery_state(fixture: &Fixture) -> Vec<String> {
    let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    let mut query = sql.prepare("SELECT json_array(reply_id, session_key, state, claim_token, claimed_at,
        claim_expires_at, processing_started_at, reconciliation_request_id, reconciliation_from_token)
        FROM replies ORDER BY reply_id").unwrap();
    query
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn metadata() -> Value {
    json!({"ok":true,"result":{"file_id":"private-incoming-file","file_unique_id":"stable-incoming",
        "file_size":1,"file_path":"documents/attachment.bin"}})
}

async fn download(
    client: &Client,
    fixture: &Fixture,
    mock: &mut Mock,
    key: &Value,
    handle: &Value,
) -> Value {
    let caller = client.clone();
    let args = json!({"session_key":key,"attachment_handle":handle});
    let call = tokio::spawn(async move { caller.tool("telegram_get_attachment", args).await });
    let request = mock.next("getFile").await;
    assert_eq!(request.payload, json!({"file_id":"private-incoming-file"}));
    request.respond(metadata());
    let bytes = mock.next("attachment.bin").await;
    assert!(bytes.body.is_empty());
    bytes.respond(json!(7));
    let result = call.await.unwrap();
    let path = PathBuf::from(result["path"].as_str().unwrap());
    assert!(path.starts_with(&fixture.config.downloads.roots[0]));
    assert_eq!(fs::read(&path).unwrap(), b"7");
    assert_eq!(result["size_bytes"], 1);
    assert_eq!(result["attachment"]["attachment_handle"], *handle);
    let stat = fs::metadata(&path).unwrap();
    assert_eq!(stat.mode() & 0o7777, 0o600);
    assert_eq!(
        result["file_identity"]["device"],
        format!("{:016x}", stat.dev())
    );
    assert_eq!(
        result["file_identity"]["inode"],
        format!("{:016x}", stat.ino())
    );
    assert!(!result.to_string().contains("private-incoming-file"));
    result
}

#[tokio::test]
async fn shared_composition_preserves_claims_and_attachment_authority_across_protocols_and_restart()
{
    let mut fixture = Fixture::new().await;
    let auth = configure(&mut fixture);
    let mut mock = Mock::new(&fixture.config).await;
    let (mut running, poll) = begin(&mut fixture, &mut mock, &auth).await;
    let address = fixture.config.server.listen;
    let modern = Client::modern(address);
    let legacy = Client::legacy(address).await;
    for client in [&modern, &legacy] {
        let catalog = client.rpc("tools/list", json!({})).await;
        crate::routes::tests::tools::assert_tool_catalog(&catalog["tools"]);
        let guidance = client.tool("telegram_guidance", json!({})).await;
        assert_eq!(guidance["capabilities"]["history_enabled"], true);
        assert_eq!(
            guidance["capabilities"]["attachment_downloads_enabled"],
            true
        );
        assert_eq!(guidance["limits"]["download_bytes"], 1024);
    }
    let owner = modern.tool("telegram_register", json!({"external_session_id":"extended-owner","title":"Same","directory_label":"module"})).await;
    let observer = legacy
        .tool(
            "telegram_register",
            json!({"external_session_id":"extended-observer","title":"Same"}),
        )
        .await;
    assert_ne!(owner["session_key"], observer["session_key"]);
    let file = fixture.config.files.allowed_roots[0].join("report.bin");
    fs::write(&file, b"explicit synthetic report").unwrap();
    let sender = modern.clone();
    let args = json!({"session_key":owner["session_key"],"path":file,"caption":"Literal <report>"});
    let sent = tokio::spawn(async move { sender.tool("telegram_send_file", args).await });
    let request = mock.next("sendDocument").await;
    assert_eq!(request.part("caption"), b"Same\nmodule\nLiteral <report>");
    assert_eq!(
        serde_json::from_slice::<Value>(request.part("caption_entities")).unwrap(),
        json!([{"type":"bold","offset":0,"length":4},{"type":"italic","offset":5,"length":6}])
    );
    let mut accepted_file = accepted(11);
    accepted_file["result"]["document"] = json!({"file_id":"outgoing-file","file_unique_id":"outgoing-stable",
        "file_name":"report.bin","mime_type":"application/octet-stream","file_size":fs::metadata(&file).unwrap().len()});
    request.respond(accepted_file);
    assert_eq!(sent.await.unwrap()["outbound_message_id"], 11);

    let uri = owner["inbox_uri"].as_str().unwrap();
    let observer_uri = observer["inbox_uri"].as_str().unwrap();
    let mut events = Events::listen_authenticated(address, uri, Some(AUTH)).await;
    let mut observer_events = Events::listen_authenticated(address, observer_uri, Some(AUTH)).await;
    let waiter = legacy.clone();
    let args = json!({"session_key":owner["session_key"],"outbound_message_id":11});
    let waiting = tokio::spawn(async move { waiter.tool("telegram_wait", args).await });
    let asker = modern.clone();
    let args = json!({"session_key":observer["session_key"],"text":"Focused question"});
    let asking = tokio::spawn(async move { asker.tool("telegram_ask", args).await });
    mock.next("sendMessage").await.respond(accepted(13));
    let mut attachment = incoming(21, 11, None);
    attachment["message"]["caption"] = json!("Incoming file");
    attachment["message"]["document"] = json!({"file_id":"private-incoming-file","file_unique_id":"stable-incoming",
        "file_name":"../../untrusted.txt","mime_type":"text/plain","file_size":1});
    poll.respond(json!({"ok":true,"result":[attachment, incoming(22,11,Some("Second reply")),incoming(23,13,Some("Answer"))]}));
    let poll = mock.next("getUpdates").await;
    assert_eq!(poll.payload["offset"], 24);
    let (waited, asked, inbox, history) = tokio::join!(
        waiting,
        asking,
        modern.tool(
            "telegram_inbox",
            json!({"session_key":owner["session_key"],"limit":1})
        ),
        legacy.tool(
            "telegram_history",
            json!({"session_key":observer["session_key"]})
        )
    );
    let waited = waited.unwrap();
    let asked = asked.unwrap();
    assert_eq!(waited["status"], "reply_received");
    assert_eq!(asked["status"], "reply_received");
    let claim = &inbox["replies"][0];
    let claims = [&waited["reply"], &asked["reply"], claim];
    for claimed in claims {
        assert_eq!(claimed["state"], "claimed");
        assert_eq!(claimed["processing_started_at"], Value::Null);
    }
    assert_eq!(
        claims
            .iter()
            .map(|c| c["reply_id"].as_str().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert_eq!(
        claims
            .iter()
            .map(|c| c["claim_token"].as_str().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    assert_eq!(fixture.count("replies"), 3);
    events.updated(uri).await;
    observer_events.updated(observer_uri).await;
    let page = modern.snapshot(&owner).await;
    let handle = page["replies"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r["attachments"].as_array().unwrap())
        .next()
        .unwrap()["attachment_handle"]
        .clone();
    assert!(
        !page
            .to_string()
            .contains(claim["claim_token"].as_str().unwrap())
    );
    assert!(
        history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|m| m["attachments"].as_array().unwrap())
            .any(|a| a["attachment_handle"] == handle)
    );
    for private in [
        owner["session_key"].as_str().unwrap(),
        claim["claim_token"].as_str().unwrap(),
        "private-incoming-file",
    ] {
        assert!(!history.to_string().contains(private));
    }
    let before = recovery_state(&fixture);
    assert_eq!(
        modern
            .tool(
                "telegram_history",
                json!({"session_key":observer["session_key"]})
            )
            .await,
        history
    );
    assert_eq!(recovery_state(&fixture), before);
    let foreign = json!({"session_key":observer["session_key"],"reply_id":claim["reply_id"],"claim_token":claim["claim_token"]});
    legacy
        .failure("telegram_ack", foreign.clone(), "CLAIM_INVALID")
        .await;
    let mut foreign_start = foreign;
    foreign_start["active"] = json!(true);
    modern
        .failure("telegram_processing", foreign_start, "CLAIM_INVALID")
        .await;
    let shared = download(
        &legacy,
        &fixture,
        &mut mock,
        &observer["session_key"],
        &handle,
    )
    .await;
    let own = download(&modern, &fixture, &mut mock, &owner["session_key"], &handle).await;
    assert_ne!(shared["path"], own["path"]);
    assert_eq!(recovery_state(&fixture), before);
    assert_eq!(fixture.count("acknowledgment_receipts"), 0);

    let mut action = json!({"session_key":owner["session_key"],"reply_id":claim["reply_id"],"claim_token":claim["claim_token"],"active":true});
    legacy.tool("telegram_processing", action.clone()).await;
    action.as_object_mut().unwrap().remove("active");
    action["reconcile_request_id"] = json!("saved-extended-reconciliation");
    let rotated = modern.tool("telegram_processing", action.clone()).await;
    let before_restart = recovery_state(&fixture);
    let page_before_restart = modern.snapshot(&owner).await;

    // Abandon a caller while its authorized download is waiting for bytes, then stop
    // with both subscriptions connected. Shutdown must drain and retire only that copy.
    let caller = modern.clone();
    let args = json!({"session_key":owner["session_key"],"attachment_handle":handle});
    let canceled = tokio::spawn(async move { caller.tool("telegram_get_attachment", args).await });
    mock.next("getFile").await.respond(metadata());
    let partial = mock.next("attachment.bin").await;
    assert_eq!(fixture.count("download_records"), 3);
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    running.stop.cancel();
    (&mut running.task).await.unwrap().unwrap();
    drop(partial);
    drop(poll);
    drop(events);
    drop(observer_events);
    assert_eq!(fixture.count("download_records"), 2);
    for completed in [&shared, &own] {
        assert_eq!(fs::read(completed["path"].as_str().unwrap()).unwrap(), b"7");
    }
    assert_eq!(fs::read(&file).unwrap(), b"explicit synthetic report");
    assert_eq!(
        fs::read_dir(&fixture.config.downloads.roots[0])
            .unwrap()
            .count(),
        2
    );
    assert_eq!(recovery_state(&fixture), before_restart);

    // A new daemon instance/transport applies current policy without cached grants.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    fixture.config.server.listen = listener.local_addr().unwrap();
    fixture.listener = Some(listener);
    fixture.config.history.enabled = false;
    let (mut restarted, poll) = begin(&mut fixture, &mut mock, &auth).await;
    assert_eq!(poll.payload["offset"], 24);
    let modern_after = Client::modern(fixture.config.server.listen);
    let legacy_after = Client::legacy(fixture.config.server.listen).await;
    let resumed = legacy_after
        .tool(
            "telegram_register",
            json!({"external_session_id":"extended-owner","title":"Resumed"}),
        )
        .await;
    assert_eq!(resumed["session_key"], owner["session_key"]);
    assert_eq!(modern_after.snapshot(&owner).await, page_before_restart);
    let mut reconnected =
        Events::listen_authenticated(fixture.config.server.listen, uri, Some(AUTH)).await;
    let guidance = modern_after.tool("telegram_guidance", json!({})).await;
    assert_eq!(guidance["capabilities"]["history_enabled"], false);
    legacy_after
        .failure(
            "telegram_get_attachment",
            json!({"session_key":observer["session_key"],"attachment_handle":handle}),
            "ATTACHMENT_UNAVAILABLE",
        )
        .await;
    modern_after
        .failure(
            "telegram_history",
            json!({"session_key":observer["session_key"]}),
            "HISTORY_DISABLED",
        )
        .await;
    assert!(matches!(
        mock.requests.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    download(
        &modern_after,
        &fixture,
        &mut mock,
        &owner["session_key"],
        &handle,
    )
    .await;
    assert_eq!(
        legacy_after.tool("telegram_processing", action).await,
        rotated
    );
    assert_eq!(recovery_state(&fixture), before_restart);
    let completion = json!({"session_key":owner["session_key"],"reply_id":claim["reply_id"],"claim_token":rotated["claim_token"]});
    modern_after.tool("telegram_ack", completion).await;
    reconnected.updated(uri).await;
    assert_eq!(fixture.count("replies"), 2);
    assert_eq!(fixture.count("acknowledgment_receipts"), 1);
    restarted.stop.cancel();
    (&mut restarted.task).await.unwrap().unwrap();
    drop(poll);
}
