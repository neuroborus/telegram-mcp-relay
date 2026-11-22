//! Real MCP requests prove read/claim separation and durable token fencing across reconnects.

use super::*;
use crate::config::ProcessingStartAck;
use crate::{
    repositories::{IngestOutcome, RouteKind},
    services::{ingress::IngressService, unix_millis},
};
use tokio::sync::oneshot;
use tools::{Telegram, failure, protocol_tool, reacted, rejected, sent, sent_in};

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

fn text_reply_update(chat: i64, parent: i64, id: i64) -> Value {
    json!({"update_id":id,"message":{"message_id":id+100,"date":1,
        "chat":{"id":chat},"from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":parent},"text":format!("reply {id}")}})
}

pub(super) fn open(fixture: &Fixture) -> Database {
    Database::open(&fixture.config.storage, &fixture.config.limits).unwrap()
}

pub(super) fn seed(
    fixture: &Fixture,
    database: &mut Database,
    session: &Value,
    chat: i64,
    parent: i64,
    ids: &[i64],
) {
    let now = unix_millis().unwrap();
    let reservation = database
        .reserve_route(session["session_key"].as_str().unwrap())
        .unwrap();
    database
        .persist_route(reservation, chat, parent, RouteKind::Document, now, now)
        .unwrap();
    let ingress = IngressService::new(&fixture.config).unwrap();
    for (index, &id) in ids.iter().enumerate() {
        let mut update = text_reply_update(chat, parent, id);
        if index == 1 {
            update["message"].as_object_mut().unwrap().remove("text");
            update["message"]["document"] =
                json!({"file_id":"synthetic","file_unique_id":"unique"});
        }
        assert!(matches!(
            ingress
                .ingest(database, &serde_json::from_value(update).unwrap(), now)
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
    let first = http
        .register(json!({"title":"Same","directory_label":"p"}))
        .await;
    let second = http
        .register(json!({"title":"Same","directory_label":"p"}))
        .await;
    seed(
        &fixture,
        &mut open(&fixture),
        &first,
        -101,
        5,
        &[11, 12, 13],
    );
    seed(&fixture, &mut open(&fixture), &second, -101, 6, &[14]);
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
    let legacy = format!(
        "{uri}?limit=2&after={},{}",
        page["replies"][1]["created_at"], page["replies"][1]["telegram_update_id"]
    );
    assert_eq!(http.snapshot(&legacy).await, last);
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
    fixture.config.activity.typing_enabled = false;
    let telegram = Telegram::new(&fixture.config, vec![reacted()]).await;
    let state = telegram.handler(&fixture);
    let mut http = fixture.serve(state, None).await;
    let session = http
        .register(json!({"external_session_id":"thread","title":"Same","directory_label":"p"}))
        .await;
    let other = http
        .register(json!({"title":"Same","directory_label":"p"}))
        .await;
    seed(&fixture, &mut open(&fixture), &session, -101, 5, &[11, 12]);
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
    assert_eq!(started["start_acknowledgment"], "sent");
    assert_eq!(started["start_acknowledgment_kind"], "reaction");
    let mut existing = started;
    existing["start_acknowledgment"] = "already_started".into();
    existing
        .as_object_mut()
        .unwrap()
        .remove("start_acknowledgment_kind");
    assert_eq!(http.success("telegram_processing", start).await, existing);
    let mut stop = args.clone();
    stop["active"] = false.into();
    assert_eq!(http.success("telegram_processing", stop).await, existing);
    let resume = json!({"session_key":key,"reply_id":a["reply_id"],"claim_token":a["claim_token"],
        "reconcile_request_id":"synthetic-resume-id"});
    let rotated = http.success("telegram_processing", resume.clone()).await;
    assert_ne!(rotated["claim_token"], a["claim_token"]);
    assert_eq!(
        rotated["processing_started_at"],
        existing["processing_started_at"]
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
async fn processing_sends_one_threaded_acknowledgment_without_guidance_on_both_protocols() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.activity.typing_enabled = false;
        fixture.config.delivery.processing_start_ack = ProcessingStartAck::Message;
        let telegram = Telegram::new(
            &fixture.config,
            vec![
                sent(31),
                sent(32),
                (200, json!(null)), // A lost or invalid result cannot establish delivery.
                (400, json!({"ok":false,"error_code":400})),
                sent(35),
            ],
        )
        .await;
        let state = telegram.handler(&fixture);
        let (mut http, endpoint) = fixture.serve_endpoint(state.clone(), None).await;
        let transport = if legacy {
            Some(http.initialize(LEGACY_VERSION).await.0)
        } else {
            None
        };
        let call = |name, args| protocol_tool(&http, &endpoint, transport.as_deref(), name, args);
        let session = call(
            "telegram_register",
            json!({"title":"🚀","directory_label":"relay"}),
        )
        .await["result"]["structuredContent"]
            .clone();
        let key = session["session_key"].as_str().unwrap();
        let parent = 5;
        let update_ids = [11, 12, 13, 14, 15];
        seed(
            &fixture,
            &mut open(&fixture),
            &session,
            -101,
            parent,
            &update_ids,
        );
        let claimed = call("telegram_inbox", json!({"session_key":key})).await;
        assert_eq!(claimed["result"]["isError"], false);
        let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
        sql.execute_batch(
            "CREATE TRIGGER fail_ack_route BEFORE INSERT ON message_routes
             WHEN NEW.telegram_message_id = 35
             BEGIN SELECT RAISE(ABORT, 'synthetic route failure'); END;",
        )
        .unwrap();
        let mut recovered = Vec::new();
        for (index, (body, outcome, id, error_code, stored_state)) in [
            (None, "sent", Some(31), None, "sent"),
            (
                Some("Starting *literal* [work"),
                "sent",
                Some(32),
                None,
                "sent",
            ),
            (None, "uncertain", None, None, "attempted"),
            (None, "failed", None, None, "attempted"),
            (
                None,
                "sent",
                Some(35),
                Some("SEND_BOOKKEEPING_FAILED"),
                "sent",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let claim = &claimed["result"]["structuredContent"]["replies"][index];
            let args = json!({"session_key":key,"reply_id":claim["reply_id"],
                "claim_token":claim["claim_token"]});
            let reply_id = claim["reply_id"].as_str().unwrap();
            let stored = || {
                sql.query_row(
                    "SELECT start_ack_state, start_ack_text FROM replies WHERE reply_id = ?1",
                    [reply_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                        ))
                    },
                )
                .unwrap()
            };
            if index == 0 {
                for (action, code) in [
                    (json!({"active":false,"message":"No"}), "REQUEST_INVALID"),
                    (
                        json!({"reconcile_request_id":"resume","message":"No"}),
                        "REQUEST_INVALID",
                    ),
                    (
                        json!({"active":true,"message":"x".repeat(990)}),
                        "TEXT_TOO_LONG",
                    ),
                ] {
                    let mut invalid = args.clone();
                    invalid
                        .as_object_mut()
                        .unwrap()
                        .extend(action.as_object().unwrap().clone());
                    let invalid = call("telegram_processing", invalid).await;
                    let result = &invalid["result"];
                    failure(result, code);
                    if code == "TEXT_TOO_LONG" {
                        let guidance = result["structuredContent"]["details"]["guidance"]
                            .as_str()
                            .unwrap();
                        assert!(guidance.contains("shorter message"));
                        assert!(guidance.contains("re-register the same session"));
                    }
                }
                let page = http.snapshot(session["inbox_uri"].as_str().unwrap()).await;
                assert_eq!(page["replies"][0]["state"], "claimed");
                assert_eq!(stored(), (None, None));
                assert_eq!(telegram.request_count(), 0);
            }
            let mut start = args.clone();
            start["active"] = true.into();
            if let Some(body) = body {
                start["message"] = body.into();
            }
            let result = call("telegram_processing", start.clone()).await;
            assert_eq!(result["result"]["isError"], false, "{result}");
            let result = &result["result"]["structuredContent"];
            assert_eq!(result["start_acknowledgment"], outcome);
            assert_eq!(result["start_acknowledgment_kind"], "message");
            assert!(result["processing_started_at"].is_i64());
            assert_eq!(
                result.get("start_acknowledgment_error_code"),
                error_code.map(Value::from).as_ref(),
            );
            match id {
                Some(id) => {
                    assert_eq!(result["outbound_message_id"], id);
                    let db =
                        Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
                    let route = db.route(-101, id, unix_millis().unwrap()).unwrap();
                    if error_code.is_some() {
                        assert!(route.is_none());
                    } else {
                        let route = route.unwrap();
                        assert_eq!(route.session_key, key);
                        assert_eq!(route.kind, RouteKind::Text);
                    }
                }
                None => assert!(result.get("outbound_message_id").is_none()),
            }
            let body = body.unwrap_or("✅ Request received. Working on it.");
            assert_eq!(stored(), (Some(stored_state.into()), Some(body.into())));
            assert_eq!(
                telegram.text_request(index),
                json!({"chat_id":-101,"text":format!("[🚀]\nrelay\n\n{body}"),
                    "entities":[{"type":"bold","offset":0,"length":4},
                                {"type":"italic","offset":5,"length":5}],
                    "reply_parameters":{"message_id":claim["telegram_message_id"],
                                        "allow_sending_without_reply":true}})
            );
            if index == 0 {
                let mut database =
                    Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
                let update =
                    serde_json::from_value(text_reply_update(-101, parent, update_ids[0])).unwrap();
                assert!(matches!(
                    IngressService::new(&fixture.config)
                        .unwrap()
                        .ingest(&mut database, &update, unix_millis().unwrap())
                        .unwrap(),
                    IngestOutcome::Duplicate
                ));
                let page = http.snapshot(session["inbox_uri"].as_str().unwrap()).await;
                let mut copies =
                    page["replies"].as_array().unwrap().iter().filter(|reply| {
                        reply["telegram_message_id"] == claim["telegram_message_id"]
                    });
                let original = copies.next().unwrap();
                assert_eq!(original["reply_id"], reply_id);
                assert_eq!(original["state"], "processing");
                assert_eq!(
                    original["processing_started_at"],
                    result["processing_started_at"]
                );
                assert!(copies.next().is_none());
            }
            start["message"] = "Must not replace or resend the original".into();
            let repeated = call("telegram_processing", start).await;
            assert_eq!(repeated["result"]["isError"], false);
            assert_eq!(
                repeated["result"]["structuredContent"]["start_acknowledgment"],
                "already_started"
            );
            let mut resume = args;
            resume["reconcile_request_id"] = format!("synthetic-resume-{index}").into();
            let reconciliation = call("telegram_processing", resume).await;
            assert_eq!(reconciliation["result"]["isError"], false);
            let rotated = &reconciliation["result"]["structuredContent"];
            assert_eq!(rotated["start_acknowledgment"], "already_started");
            recovered.push(json!({"session_key":key,"reply_id":reply_id,
                "claim_token":rotated["claim_token"],"active":true}));
            assert_eq!(stored(), (Some(stored_state.into()), Some(body.into())));
            assert_eq!(telegram.request_count(), index + 1);
        }
        http.finish().await;
        state.stop();
        state.drain().await.unwrap();
        drop(endpoint);
        drop(state);

        // Schema-12 attempts survive a restart under the new reaction default unchanged.
        let rows = || {
            let mut statement = sql
                .prepare(
                    "SELECT reply_id, start_ack_state, start_ack_text, processing_started_at
                     FROM replies WHERE start_ack_state IS NOT NULL ORDER BY reply_id",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let before = rows();
        assert_eq!(before.len(), 5);
        assert!(before.iter().any(|row| row.1 == "attempted"));
        assert!(before.iter().any(|row| row.1 == "sent"));
        fixture.config.delivery.processing_start_ack = ProcessingStartAck::Reaction;
        let (mut http, endpoint) = fixture
            .serve_endpoint(telegram.handler(&fixture), None)
            .await;
        assert_eq!(
            Database::open(&fixture.config.storage, &fixture.config.limits)
                .unwrap()
                .schema_version()
                .unwrap(),
            12
        );
        let transport = if legacy {
            Some(http.initialize(LEGACY_VERSION).await.0)
        } else {
            None
        };
        for start in recovered {
            let result = protocol_tool(
                &http,
                &endpoint,
                transport.as_deref(),
                "telegram_processing",
                start,
            )
            .await;
            assert_eq!(result["result"]["isError"], false, "{result}");
            assert_eq!(
                result["result"]["structuredContent"]["start_acknowledgment"],
                "already_started"
            );
        }
        assert_eq!(telegram.request_count(), 5);
        assert_eq!(rows(), before);
        http.finish().await;
    }
}

/// Signal arrival when requested, then answer with the queued or held response.
async fn respond(
    arrived: Option<oneshot::Sender<()>>,
    response: oneshot::Receiver<(u16, Value)>,
) -> (u16, Value) {
    if let Some(arrived) = arrived {
        arrived.send(()).unwrap();
    }
    response.await.unwrap()
}

#[tokio::test]
async fn processing_reacts_once_by_default_on_both_protocols() {
    for legacy in [false, true] {
        let mut fixture = Fixture::new();
        // Only acknowledgment effects reach the mock Bot API.
        fixture.config.activity.typing_enabled = false;
        fixture.config.telegram.allowed_chat_ids = vec![-101, -202];
        fixture.config.history.enabled = true;
        let auth = fixture.auth();
        let (arrived, held_arrived) = oneshot::channel();
        let (release, held) = oneshot::channel();
        let ready = [
            reacted(),
            rejected(),
            (200, json!({"ok":true,"result":null})),
            reacted(),
            sent_in(-202, 41),
        ]
        .map(|response| {
            let (queued, response_rx) = oneshot::channel();
            queued.send(response).unwrap();
            (None, response_rx)
        });
        let telegram = Telegram::with_responses(
            &fixture.config,
            ready
                .into_iter()
                .chain([(Some(arrived), held)])
                .map(|(arrived, response)| respond(arrived, response)),
        )
        .await;
        let state = telegram.history_handler(&fixture);
        let (mut http, endpoint) = fixture.serve_endpoint(state.clone(), Some(&auth)).await;
        http.authorize();
        let transport = if legacy {
            Some(http.initialize(LEGACY_VERSION).await.0)
        } else {
            None
        };
        let call = |name, args| protocol_tool(&http, &endpoint, transport.as_deref(), name, args);
        let session = call(
            "telegram_register",
            json!({"title":"Relay","directory_label":"relay"}),
        )
        .await["result"]["structuredContent"]
            .clone();
        let other = call(
            "telegram_register",
            json!({"title":"Other","directory_label":"relay"}),
        )
        .await["result"]["structuredContent"]
            .clone();
        let key = session["session_key"].as_str().unwrap();
        let parent = 5;
        seed(
            &fixture,
            &mut open(&fixture),
            &session,
            -202,
            parent,
            &[11, 12, 13, 14, 15, 16, 17],
        );
        let claimed = call("telegram_inbox", json!({"session_key":key})).await;
        let replies = claimed["result"]["structuredContent"]["replies"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(replies.len(), 7);
        let start = |index: usize| {
            json!({"session_key":key,"reply_id":replies[index]["reply_id"],
                "claim_token":replies[index]["claim_token"],"active":true})
        };
        let sql = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
        let stored = |index: usize| {
            sql.query_row(
                "SELECT start_ack_state, start_ack_text, length(CAST(start_ack_text AS BLOB)),
                 processing_started_at FROM replies WHERE reply_id = ?1",
                [replies[index]["reply_id"].as_str().unwrap()],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .unwrap()
        };
        let effects = || {
            sql.query_row(
                "SELECT (SELECT count(*) FROM message_routes), (SELECT count(*) FROM journal_records)",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap()
        };
        let reaction = |index: usize| {
            json!({"chat_id":-202,"message_id":replies[index]["telegram_message_id"],
                "reaction":[{"type":"emoji","emoji":"👀"}]})
        };

        // Stale and foreign claims fail before any durable start or Telegram effect.
        let mut stale = start(0);
        stale["claim_token"] = "stale".into();
        let mut foreign = start(0);
        foreign["session_key"] = other["session_key"].clone();
        for invalid in [stale, foreign] {
            failure(
                &call("telegram_processing", invalid).await["result"],
                "CLAIM_INVALID",
            );
        }
        assert_eq!(stored(0), (None, None, None, None));
        assert_eq!(telegram.request_count(), 0);

        // Rejected and uncertain reactions never fall back to text; processing stays started.
        let trigger = format!(
            "CREATE TRIGGER fail_reaction_confirmation BEFORE UPDATE OF start_ack_state ON replies
             WHEN NEW.reply_id = '{}' AND OLD.start_ack_state = 'attempted'
             AND NEW.start_ack_state = 'sent'
             BEGIN SELECT RAISE(ABORT, 'synthetic confirmation failure'); END;",
            replies[3]["reply_id"].as_str().unwrap()
        );
        sql.execute_batch(&trigger).unwrap();
        for (index, outcome, error_code, stored_state) in [
            (0, "sent", None, "sent"),
            (1, "failed", None, "attempted"),
            (2, "uncertain", None, "attempted"),
            (3, "sent", Some("SEND_BOOKKEEPING_FAILED"), "attempted"),
        ] {
            let before = effects();
            let result = call("telegram_processing", start(index)).await;
            assert_eq!(result["result"]["isError"], false, "{result}");
            let result = &result["result"]["structuredContent"];
            assert_eq!(result["start_acknowledgment"], outcome);
            assert_eq!(result["start_acknowledgment_kind"], "reaction");
            assert!(result.get("outbound_message_id").is_none());
            assert_eq!(
                result.get("start_acknowledgment_error_code"),
                error_code.map(Value::from).as_ref(),
            );
            assert_eq!(telegram.request_count(), index + 1);
            assert_eq!(
                telegram.json_request(index, "setMessageReaction"),
                reaction(index)
            );
            // A reaction creates no outbound message, route or history record.
            assert_eq!(effects(), before);
            assert_eq!(
                stored(index),
                (
                    Some(stored_state.into()),
                    Some("👀".into()),
                    Some(4),
                    result["processing_started_at"].as_i64()
                )
            );
        }
        sql.execute_batch("DROP TRIGGER fail_reaction_confirmation;")
            .unwrap();

        // An explicit literal message is threaded text in reaction mode.
        let before = effects();
        let mut explicit = start(4);
        explicit["message"] = "Checking release logs".into();
        let result = call("telegram_processing", explicit).await;
        let result = &result["result"]["structuredContent"];
        assert_eq!(result["start_acknowledgment"], "sent");
        assert_eq!(result["start_acknowledgment_kind"], "message");
        assert_eq!(result["outbound_message_id"], 41);
        assert_eq!(
            telegram.text_request(4),
            json!({"chat_id":-202,"text":"[Relay]\nrelay\n\nChecking release logs",
                "entities":[{"type":"bold","offset":0,"length":7},
                            {"type":"italic","offset":8,"length":5}],
                "reply_parameters":{"message_id":replies[4]["telegram_message_id"],
                                    "allow_sending_without_reply":true}})
        );
        let route = open(&fixture)
            .route(-202, 41, unix_millis().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            (route.session_key.as_str(), route.kind),
            (key, RouteKind::Text)
        );
        assert_eq!(effects(), (before.0 + 1, before.1 + 1));
        assert_eq!(stored(4).0.as_deref(), Some("sent"));

        // A competing start while the first reaction is in flight cannot react again.
        let first = call("telegram_processing", start(5));
        let competing = async {
            held_arrived.await.unwrap();
            let params = json!({"name":"telegram_processing","arguments":start(5)});
            let competing = match transport.as_deref() {
                Some(transport) => http.legacy_rpc(transport, "tools/call", params).await,
                None => http.rpc("tools/call", params).await,
            };
            let competing = &competing["result"]["structuredContent"];
            assert_eq!(competing["start_acknowledgment"], "already_started");
            assert!(competing.get("start_acknowledgment_kind").is_none());
            release.send(reacted()).unwrap();
        };
        let (first, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(first, competing)
        })
        .await
        .unwrap();
        let first = &first["result"]["structuredContent"];
        assert_eq!(first["start_acknowledgment"], "sent");
        assert_eq!(first["start_acknowledgment_kind"], "reaction");
        assert_eq!(telegram.request_count(), 6);
        assert_eq!(telegram.json_request(5, "setMessageReaction"), reaction(5));
        assert_eq!(stored(5).0.as_deref(), Some("sent"));

        // Redelivery, repeated, inactive and reconciled calls consume no new attempt.
        let retained = || (0..replies.len()).map(&stored).collect::<Vec<_>>();
        let snapshot = retained();
        let update = serde_json::from_value(text_reply_update(-202, parent, 11)).unwrap();
        assert!(matches!(
            IngressService::new(&fixture.config)
                .unwrap()
                .ingest(&mut open(&fixture), &update, unix_millis().unwrap())
                .unwrap(),
            IngestOutcome::Duplicate
        ));
        let mut current = start(0);
        let mut resume = current.clone();
        resume.as_object_mut().unwrap().remove("active");
        resume["reconcile_request_id"] = "synthetic-reaction-resume".into();
        for action in [
            json!({"active":true}),
            json!({"active":false}),
            json!({"active":true,"message":"Must not send"}),
        ] {
            let mut repeated = current.clone();
            repeated
                .as_object_mut()
                .unwrap()
                .extend(action.as_object().unwrap().clone());
            let repeated = call("telegram_processing", repeated).await;
            let repeated = &repeated["result"]["structuredContent"];
            assert_eq!(repeated["start_acknowledgment"], "already_started");
            assert!(repeated.get("start_acknowledgment_kind").is_none());
        }
        let rotated = call("telegram_processing", resume).await;
        let rotated = &rotated["result"]["structuredContent"];
        assert_eq!(rotated["start_acknowledgment"], "already_started");
        assert!(rotated.get("start_acknowledgment_kind").is_none());
        current["claim_token"] = rotated["claim_token"].clone();
        assert_eq!(retained(), snapshot);
        assert_eq!(telegram.request_count(), 6);
        http.finish().await;
        state.stop();
        state.drain().await.unwrap();
        drop(endpoint);
        drop(state);

        // After restart without -202 in the allowlist, a retained claim starts but cannot react.
        fixture.config.telegram.allowed_chat_ids = vec![-101];
        let state = telegram.history_handler(&fixture);
        let (mut http, endpoint) = fixture.serve_endpoint(state.clone(), Some(&auth)).await;
        http.authorize();
        let transport = if legacy {
            Some(http.initialize(LEGACY_VERSION).await.0)
        } else {
            None
        };
        let call = |name, args| protocol_tool(&http, &endpoint, transport.as_deref(), name, args);
        let recovered = call("telegram_processing", current).await;
        let recovered = &recovered["result"]["structuredContent"];
        assert_eq!(recovered["start_acknowledgment"], "already_started");
        assert!(recovered.get("start_acknowledgment_kind").is_none());
        assert_eq!(retained(), snapshot);
        let unauthorized = call("telegram_processing", start(6)).await;
        assert_eq!(unauthorized["result"]["isError"], false, "{unauthorized}");
        let unauthorized = &unauthorized["result"]["structuredContent"];
        assert_eq!(unauthorized["start_acknowledgment"], "failed");
        assert_eq!(unauthorized["start_acknowledgment_kind"], "reaction");
        assert!(unauthorized["processing_started_at"].is_i64());
        assert_eq!(
            stored(6),
            (
                Some("attempted".into()),
                Some("👀".into()),
                Some(4),
                unauthorized["processing_started_at"].as_i64()
            )
        );
        assert_eq!(telegram.request_count(), 6);
        http.finish().await;
        state.stop();
        state.drain().await.unwrap();
    }
}

#[tokio::test]
async fn invalid_inbox_uris_and_claim_arguments_fail_safely_without_effects() {
    let mut fixture = Fixture::new();
    let state = fixture.state.clone();
    let http = fixture.serve(state, None).await;
    let session = http
        .register(json!({"title":"Same","directory_label":"p"}))
        .await;
    seed(&fixture, &mut open(&fixture), &session, -101, 5, &[11]);
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
        format!("{uri}?after=1,2,"),
        format!("{uri}?after=1,2,private,extra"),
        format!("{uri}?after=1,2,{}", "a".repeat(65)),
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
