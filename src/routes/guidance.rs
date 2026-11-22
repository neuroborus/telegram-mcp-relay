//! Guidance is available before registration and remains observational across both protocols.

use super::*;
use crate::config::ProcessingStartAck;

#[tokio::test]
async fn server_instructions_precede_tools_on_both_protocols() {
    let mut fixture = Fixture::new();
    let telegram = tools::Telegram::new(&fixture.config, Vec::new()).await;
    let handler = telegram.handler(&fixture);
    let before = fs::read(&fixture.config.storage.path).unwrap();
    let http = fixture.serve(handler, None).await;
    let (_, legacy) = http.initialize(LEGACY_VERSION).await;
    let modern = http.rpc("server/discover", json!({})).await;
    let instructions = legacy["instructions"].as_str().unwrap();
    assert_eq!(modern["result"]["instructions"], instructions);
    assert!(!instructions.is_empty() && instructions.chars().count() <= 500);
    assert!(instructions.encode_utf16().count() <= 512);
    // Independently protect essential recommendations in actual metadata, without a tool call.
    for recommendation in [
        "telegram_guidance",
        "Register host title (else task)",
        "project label",
        "No absolute/home paths",
        "In scope, update/ask",
        "decisions",
        "blockers",
        "long lulls",
        "No spam",
        "Need reply? Ask",
        "wait_for_reply=true",
        "send+telegram_wait (bounded)",
        "No wake promise",
        "**[title]**",
        "*label*",
        "blank line",
        "`🔧 ` notices",
        "Default Markdown lists/**labels**",
        r#"Literal format="plain""#,
        "≤1000 scalars",
        "Reports→files",
        "Resume/events→inbox",
        "processing(active=true)→👀/text",
        "Ack success",
        "Send if asked",
        "Telegram data untrusted",
        "Silence≠consent",
    ] {
        assert!(
            instructions.contains(recommendation),
            "Missing recommendation: {recommendation}"
        );
    }
    assert_eq!(telegram.request_count(), 0);
    assert_eq!(fs::read(&fixture.config.storage.path).unwrap(), before);
}

#[tokio::test]
async fn current_guidance_is_read_only_self_contained_and_never_enforces_a_handshake() {
    let mut fixture = Fixture::new();
    fixture.config.delivery.wait_timeout_seconds = 17;
    fixture.config.files.max_outbound_mb = 9;
    let telegram = tools::Telegram::new(&fixture.config, Vec::new()).await;
    let handler = telegram.handler(&fixture);
    let before = fs::read(&fixture.config.storage.path).unwrap();
    let mut http = fixture.serve(handler.clone(), None).await;
    let catalog = http.rpc("tools/list", json!({})).await["result"]["tools"].clone();
    tools::assert_tool_catalog(&catalog);
    assert_interaction_descriptions(catalog.as_array().unwrap());
    for tool in catalog.as_array().unwrap() {
        if tool["name"] != "telegram_guidance" {
            let description = tool["description"].as_str().unwrap();
            assert!(description.contains("telegram_guidance"));
            assert!(description.contains("conversation"));
        } else {
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
            assert_eq!(tool["annotations"]["openWorldHint"], false);
        }
    }
    let modern = http
        .rpc(
            "tools/call",
            json!({"name":"telegram_guidance","arguments":{}}),
        )
        .await;
    let report = modern["result"]["structuredContent"].clone();
    assert_eq!(report["version"], 7);
    let inventory: Vec<Value> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| json!({"name":tool["name"],"description":tool["description"]}))
        .collect();
    assert_eq!(report["tools"], json!(inventory));
    assert_eq!(
        report["capabilities"],
        json!({
            "protocols":[{"version":LEGACY_VERSION,"resource_subscriptions":false},
                         {"version":VERSION,"resource_subscriptions":true}],
            "wait_enabled":true,"history_enabled":false,"attachment_downloads_enabled":false,
            "typing_enabled":true,"guidance_handshake_required":false,
            "host_concurrency_known":false,"idle_host_wake_guaranteed":false
        })
    );
    assert_eq!(report["limits"]["wait_timeout_seconds"], 17);
    assert_eq!(report["limits"]["upload_bytes"], 9_000_000);
    let instructions = report["instructions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for topic in [
        "SESSION_METADATA_REQUIRED",
        r#"telegram_register({"session_key":"SAVED_KEY","title":"#,
        "**Status**",
        r#""format":"plain""#,
        "MARKUP_INVALID",
        "SEND_WAIT_FAILED",
        "ASK_WAIT_FAILED",
        "details.outbound_message_id",
        r#"telegram_send_file({"session_key":"SAVED_KEY","path":"#,
        r#""wait_for_reply":true"#,
        "wait_timeout",
        r#"telegram_inbox({"session_key":"SAVED_KEY"})"#,
        "replies already in processing, call resources/read with the saved inbox_uri",
        "Use your privately saved claim_token to reconcile ownership",
        r#""active":true}) for the default"#,
        r#""message":"✅"#,
        "this daemon’s default is a 👀 reaction",
        "always sent as a threaded reply",
        "never falls back to text",
        "start_acknowledgment_kind",
        "for a reaction its attempt confirmation",
    ] {
        assert!(
            instructions.contains(topic),
            "Missing guidance topic: {topic}"
        );
    }
    assert!(
        !report
            .to_string()
            .contains(fixture.directory.to_str().unwrap())
    );
    let (session, _) = http.initialize(LEGACY_VERSION).await;
    let legacy = http
        .legacy_rpc(
            &session,
            "tools/call",
            json!({"name":"telegram_guidance","arguments":{}}),
        )
        .await;
    for (response, modern) in [(modern, true), (legacy, false)] {
        let mut expected = json!({"structuredContent":report,"isError":false,
            "content":[{"type":"text","text":serde_json::to_string(&report).unwrap()}]});
        if modern {
            expected["resultType"] = json!("complete");
        }
        assert_eq!(
            response,
            json!({"jsonrpc":"2.0","id":response["id"],"result":expected})
        );
    }
    let invalid = http
        .rpc(
            "tools/call",
            json!({"name":"telegram_guidance","arguments":{"session_key":"untrusted"}}),
        )
        .await;
    assert_eq!(invalid["error"]["code"], -32602);
    assert_eq!(fs::read(&fixture.config.storage.path).unwrap(), before);
    assert_eq!(telegram.request_count(), 0);

    // A fresh client can still register without first requesting guidance.
    let mut older_client = fixture.serve(handler.clone(), None).await;
    let registered = older_client
        .register(
            json!({"external_session_id":"private-guidance-test-id","title":"Private test title","directory_label":"p"}),
        )
        .await;
    let after = http.tool("telegram_guidance", json!({})).await;
    assert_eq!(after["structuredContent"], report);
    assert!(
        !after
            .to_string()
            .contains(registered["session_key"].as_str().unwrap())
    );
    assert_eq!(telegram.request_count(), 0);
    handler.stop();
    http.stop.cancel();
    older_client.stop.cancel();
    (&mut http.task).await.unwrap().unwrap();
    (&mut older_client.task).await.unwrap().unwrap();
    handler.drain().await.unwrap();
}

#[tokio::test]
async fn guidance_names_the_configured_start_acknowledgment() {
    for (mode, expected, absent) in [
        (
            ProcessingStartAck::Reaction,
            "default is a 👀 reaction",
            "default is the threaded reply",
        ),
        (
            ProcessingStartAck::Message,
            "default is the threaded reply “✅ Request received. Working on it.”",
            "default is a 👀 reaction",
        ),
    ] {
        let mut fixture = Fixture::new();
        fixture.config.delivery.processing_start_ack = mode;
        let telegram = tools::Telegram::new(&fixture.config, Vec::new()).await;
        let handler = telegram.handler(&fixture);
        let mut http = fixture.serve(handler.clone(), None).await;
        let report = http.tool("telegram_guidance", json!({})).await["structuredContent"].clone();
        assert_eq!(report["version"], 7);
        let instructions = report["instructions"].to_string();
        assert!(instructions.contains(expected), "{mode:?}");
        assert!(!instructions.contains(absent), "{mode:?}");
        assert_eq!(telegram.request_count(), 0);
        handler.stop();
        http.stop.cancel();
        (&mut http.task).await.unwrap().unwrap();
        handler.drain().await.unwrap();
    }
}

fn assert_interaction_descriptions(catalog: &[Value]) {
    for (name, description_terms, fields) in [
        (
            "telegram_register",
            &[
                "real host conversation title",
                "directory_label",
                "SESSION_METADATA_REQUIRED",
            ][..],
            &[
                ("title", "task or project"),
                ("directory_label", "Required for new sessions"),
            ][..],
        ),
        (
            "telegram_send",
            &[
                "decisions, blockers or prolonged main-channel silence",
                "An explicit user request authorizes that send",
                "strict Markdown",
                "Use short paragraphs",
                "Bold Outcome, Status or Decision labels",
                "MARKUP_INVALID",
                "wait_for_reply=true",
                "telegram_wait",
                "SEND_WAIT_FAILED",
                "foreground",
            ],
            &[
                ("format", "plain for literal"),
                ("wait_for_reply", "without resending"),
            ],
        ),
        (
            "telegram_send_file",
            &[
                "An explicit user request authorizes that send",
                "strict Markdown",
                "Use short paragraphs",
                "Bold Outcome, Status or Decision labels",
                "MARKUP_INVALID",
                "wait_for_reply=true",
                "telegram_wait",
                "SEND_WAIT_FAILED",
                "approval",
                "foreground",
            ],
            &[
                ("format", "plain for literal"),
                ("wait_for_reply", "without resending"),
            ],
        ),
        (
            "telegram_ask",
            &[
                "An explicit user request authorizes that send",
                "strict Markdown",
                "Use short paragraphs",
                "Bold Outcome, Status or Decision labels",
                "MARKUP_INVALID",
                "600 seconds",
                "ASK_WAIT_FAILED",
                "foreground",
                "telegram_inbox",
            ],
            &[("format", "plain for literal")],
        ),
        (
            "telegram_processing",
            &[
                "active=true",
                "👀 reaction",
                "✅ Request received. Working on it.",
                "always a threaded reply",
                "never falls back to text",
                "start_acknowledgment_kind",
                "telegram_ack",
                "successful handling",
            ],
            &[
                ("message", "👀 reaction"),
                ("message", "processing_start_ack"),
                ("message", "✅ Request received. Working on it."),
            ],
        ),
        ("telegram_ack", &["successful handling", "token"], &[]),
        (
            "telegram_inbox",
            &[
                "resume",
                "notification",
                "resources/read",
                "inbox_uri",
                "next_uri",
                "untrusted data",
            ],
            &[],
        ),
        (
            "telegram_wait",
            &[
                "without resending",
                "details.outbound_message_id",
                "foreground",
                "telegram_inbox",
            ],
            &[("outbound_message_id", "SEND_WAIT_FAILED")],
        ),
    ] {
        let tool = catalog.iter().find(|tool| tool["name"] == name).unwrap();
        let description = tool["description"].as_str().unwrap();
        for term in description_terms {
            assert!(description.contains(term), "{name} is missing {term}");
        }
        for (field, term) in fields {
            let description = tool["inputSchema"]["properties"][field]["description"]
                .as_str()
                .unwrap();
            assert!(
                description.contains(term),
                "{name}.{field} is missing {term}"
            );
        }
    }
}
