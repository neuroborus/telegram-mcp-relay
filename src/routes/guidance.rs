//! Guidance is available before registration and remains observational across both protocols.

use super::*;

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
    assert_eq!(report["version"], 1);
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
    assert!(!report["instructions"].as_array().unwrap().is_empty());
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
            json!({"external_session_id":"private-guidance-test-id","title":"Private test title"}),
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
