//! Real MCP requests prove read/claim separation and durable token fencing across reconnects.

use super::*;
use crate::{
    repositories::{IngestOutcome, RouteKind},
    services::{ingress::IngressService, unix_millis},
};
use tools::failure;

impl Http {
    pub(super) async fn snapshot(&self, uri: &str) -> Value {
        let result = self.rpc("resources/read", json!({"uri":uri})).await;
        assert!(result.get("error").is_none(), "{result}");
        let result = &result["result"];
        assert_eq!(result["cacheScope"], "private");
        assert_eq!(result["ttlMs"], 0);
        assert_eq!(result["contents"][0]["uri"], uri);
        assert_eq!(result["contents"][0]["mimeType"], "application/json");
        serde_json::from_str(result["contents"][0]["text"].as_str().unwrap()).unwrap()
    }

    async fn success(&self, name: &str, args: Value) -> Value {
        let result = self.tool(name, args).await;
        assert_eq!(result["isError"], false, "{result}");
        result["structuredContent"].clone()
    }
}

fn seed(fixture: &Fixture, session: &Value, parent: i64, ids: &[i64]) {
    let mut database = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    let now = unix_millis().unwrap();
    let reservation = database
        .reserve_route(session["session_key"].as_str().unwrap())
        .unwrap();
    database
        .persist_route(reservation, -101, parent, RouteKind::Document, now, now)
        .unwrap();
    let ingress = IngressService::new(&fixture.config).unwrap();
    for (index, id) in ids.iter().enumerate() {
        let mut update = json!({"update_id":id,"message":{"message_id":id+100,"date":1,
            "chat":{"id":-101},"from":{"id":7,"is_bot":false},
            "reply_to_message":{"message_id":parent},"text":format!("reply {id}")}});
        if index == 1 {
            update["message"].as_object_mut().unwrap().remove("text");
            update["message"]["document"] =
                json!({"file_id":"synthetic","file_unique_id":"unique"});
        }
        assert!(matches!(
            ingress
                .ingest(&mut database, &serde_json::from_value(update).unwrap(), now)
                .unwrap(),
            IngestOutcome::ReplyStored { .. }
        ));
    }
}

#[tokio::test]
async fn stable_templates_and_direct_paginated_snapshots_never_claim_or_enumerate_sessions() {
    let mut fixture = Fixture::new();
    let state = fixture.state.clone();
    let http = fixture.serve(state, None).await;
    let templates = http.rpc("resources/templates/list", json!({})).await;
    assert_eq!(
        templates["result"]["resourceTemplates"][0]["uriTemplate"],
        "telegram-bridge://sessions/{session_key}/inbox"
    );
    let listing = http.rpc("resources/list", json!({})).await;
    assert_eq!(listing["result"]["resources"], json!([]));
    let first = http.register(json!({"title":"Same"})).await;
    let second = http.register(json!({"title":"Same"})).await;
    seed(&fixture, &first, 5, &[11, 12, 13]);
    seed(&fixture, &second, 6, &[14]);
    let database = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    let before = database
        .session(first["session_key"].as_str().unwrap())
        .unwrap();
    let uri = first["inbox_uri"].as_str().unwrap();
    let page = http.snapshot(&format!("{uri}?limit=2")).await;
    assert_eq!(page["replies"].as_array().unwrap().len(), 2);
    assert_eq!(page["replies"][0]["telegram_update_id"], 11);
    assert_eq!(page["replies"][1]["telegram_update_id"], 12);
    assert_eq!(page["replies"][0]["reply_to_bot_message_id"], 5);
    assert_eq!(page["replies"][1]["text_or_caption"], Value::Null);
    assert_eq!(
        page["replies"][1]["attachment_metadata"]["document"]["file_id"],
        "synthetic"
    );
    assert_eq!(page, http.snapshot(&format!("{uri}?limit=2")).await);
    let last = http.snapshot(page["next_uri"].as_str().unwrap()).await;
    assert_eq!(last["replies"].as_array().unwrap().len(), 1);
    assert_eq!(last["replies"][0]["telegram_update_id"], 13);
    assert_eq!(last["next_uri"], Value::Null);
    let other = http.snapshot(second["inbox_uri"].as_str().unwrap()).await;
    assert_eq!(other["replies"].as_array().unwrap().len(), 1);
    assert_eq!(other["replies"][0]["telegram_update_id"], 14);
    for reply in page["replies"].as_array().unwrap() {
        assert_eq!(reply["state"], "pending");
        assert_eq!(reply["claimed_at"], Value::Null);
        assert!(reply.get("claim_token").is_none());
    }
    assert_eq!(
        database
            .session(first["session_key"].as_str().unwrap())
            .unwrap(),
        before
    );
    assert_eq!(
        http.rpc("resources/templates/list", json!({})).await,
        templates
    );
    assert_eq!(http.rpc("resources/list", json!({})).await, listing);
    let claimed = http
        .success(
            "telegram_inbox",
            json!({"session_key":first["session_key"],"limit":1}),
        )
        .await;
    assert_eq!(
        claimed["replies"][0]["reply_id"],
        page["replies"][0]["reply_id"]
    );
    assert_eq!(claimed["replies"][0]["state"], "claimed");
    let snapshot = http.snapshot(uri).await;
    assert_eq!(snapshot["replies"][0]["state"], "claimed");
    assert!(!snapshot.to_string().contains("claim_token"));
}

#[tokio::test]
async fn competing_inbox_claims_reconcile_and_complete_durably_through_mcp() {
    let mut fixture = Fixture::new();
    let state = fixture.state.clone();
    let mut http = fixture.serve(state, None).await;
    let session = http
        .register(json!({"external_session_id":"thread","title":"Same"}))
        .await;
    let other = http.register(json!({"title":"Same"})).await;
    seed(&fixture, &session, 5, &[11, 12]);
    let key = &session["session_key"];
    let (a, b) = tokio::join!(
        http.success("telegram_inbox", json!({"session_key":key,"limit":1})),
        http.success("telegram_inbox", json!({"session_key":key,"limit":1})),
    );
    let a = &a["replies"][0];
    let b = &b["replies"][0];
    assert_ne!(a["reply_id"], b["reply_id"]);
    assert_ne!(a["claim_token"], b["claim_token"]);
    let args = json!({"session_key":key,"reply_id":a["reply_id"],"claim_token":a["claim_token"]});
    let mut start = args.clone();
    start["active"] = true.into();
    let started = http.success("telegram_processing", start.clone()).await;
    assert!(started["processing_started_at"].is_i64());
    assert_eq!(http.success("telegram_processing", start).await, started);
    let mut stop = args.clone();
    stop["active"] = false.into();
    assert_eq!(http.success("telegram_processing", stop).await, started);
    let resume = json!({"session_key":key,"reply_id":a["reply_id"],"claim_token":a["claim_token"],
        "reconcile_request_id":"synthetic-resume-id"});
    let rotated = http.success("telegram_processing", resume.clone()).await;
    assert_ne!(rotated["claim_token"], a["claim_token"]);
    assert_eq!(
        rotated["processing_started_at"],
        started["processing_started_at"]
    );
    http.finish().await;
    let reopened = Arc::new(
        McpHandler::new(
            Arc::new(std::sync::Mutex::new(
                Database::open(&fixture.config.storage, &fixture.config.limits).unwrap(),
            )),
            crate::clients::telegram::TelegramClient::for_test(
                &fixture.config,
                "http://127.0.0.1:1/botSynthetic/".into(),
            )
            .unwrap(),
            &fixture.config,
        )
        .unwrap(),
    );
    let http = fixture.serve(reopened, None).await;
    let resumed = http
        .register(json!({"external_session_id":"thread","title":"Renamed"}))
        .await;
    assert_eq!(resumed["session_key"], *key);
    let page = http.snapshot(session["inbox_uri"].as_str().unwrap()).await;
    assert_eq!(page["replies"].as_array().unwrap().len(), 2);
    assert!(
        page["replies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["reply_id"] == a["reply_id"] && r["state"] == "processing")
    );
    // Recover the response lost before reconnect using the saved old token/request pair.
    assert_eq!(http.success("telegram_processing", resume).await, rotated);
    let mut current = args.clone();
    current["claim_token"] = rotated["claim_token"].clone();
    for name in ["telegram_processing", "telegram_ack"] {
        let mut stale = args.clone();
        let mut wrong_session = current.clone();
        wrong_session["session_key"] = other["session_key"].clone();
        if name == "telegram_processing" {
            stale["active"] = true.into();
            wrong_session["active"] = false.into();
        }
        failure(&http.tool(name, stale).await, "CLAIM_INVALID");
        failure(&http.tool(name, wrong_session).await, "CLAIM_INVALID");
    }
    let ack = http.success("telegram_ack", current.clone()).await;
    assert_eq!(ack, json!({"reply_id":a["reply_id"],"acknowledged":true}));
    assert_eq!(http.success("telegram_ack", current).await, ack);
    http.success(
        "telegram_ack",
        json!({"session_key":key,"reply_id":b["reply_id"],"claim_token":b["claim_token"]}),
    )
    .await;
    assert_eq!(
        http.snapshot(session["inbox_uri"].as_str().unwrap()).await["replies"],
        json!([])
    );
}

#[tokio::test]
async fn invalid_inbox_uris_and_claim_arguments_fail_safely_without_effects() {
    let mut fixture = Fixture::new();
    let state = fixture.state.clone();
    let http = fixture.serve(state, None).await;
    let session = http.register(json!({"title":"Same"})).await;
    seed(&fixture, &session, 5, &[11]);
    let uri = session["inbox_uri"].as_str().unwrap();
    for invalid in [
        "private-uri".into(),
        format!("{uri}/private"),
        format!("{uri}?private=value"),
        format!("{uri}?limit=0"),
        format!("{uri}?limit=101"),
        format!("{uri}?limit=1&limit=2"),
        format!("{uri}?after=-1,2"),
        format!("{uri}?after=private"),
        format!("{uri}#private"),
    ] {
        let result = http.rpc("resources/read", json!({"uri":invalid})).await;
        assert_eq!(result["error"]["code"], -32602);
        assert!(!result.to_string().contains("private"));
    }
    let unknown = format!("telegram-bridge://sessions/{}/inbox", "0".repeat(64));
    let missing = http.rpc("resources/read", json!({"uri":unknown})).await;
    assert_eq!(missing["error"]["code"], -32602);
    assert!(!missing.to_string().contains(&unknown));
    for method in ["resources/list", "resources/templates/list"] {
        let result = http.rpc(method, json!({"cursor":"private"})).await;
        assert_eq!(result["error"]["code"], -32602);
        assert!(!result.to_string().contains("private"));
    }
    for (name, args, code) in [
        (
            "telegram_inbox",
            json!({"session_key":"private"}),
            "SESSION_NOT_FOUND",
        ),
        (
            "telegram_inbox",
            json!({"session_key":session["session_key"],"limit":101}),
            "REQUEST_INVALID",
        ),
        (
            "telegram_processing",
            json!({"session_key":session["session_key"],"reply_id":"private","claim_token":"private","active":true}),
            "CLAIM_INVALID",
        ),
        (
            "telegram_processing",
            json!({"session_key":session["session_key"],"reply_id":"private","claim_token":"private","active":false,"reconcile_request_id":"private"}),
            "REQUEST_INVALID",
        ),
        (
            "telegram_ack",
            json!({"session_key":session["session_key"],"reply_id":"private","claim_token":"private"}),
            "CLAIM_INVALID",
        ),
    ] {
        let result = http.tool(name, args).await;
        failure(&result, code);
        assert!(!result.to_string().contains("private"));
    }
    for name in ["telegram_inbox", "telegram_processing", "telegram_ack"] {
        let result = http
            .rpc(
                "tools/call",
                json!({"name":name,"arguments":{"private":true}}),
            )
            .await;
        assert_eq!(result["error"]["code"], -32602);
        assert!(!result.to_string().contains("private"));
    }
    assert_eq!(http.snapshot(uri).await["replies"][0]["state"], "pending");
}
