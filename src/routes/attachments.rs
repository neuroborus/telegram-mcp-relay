//! Typed attachment access over both actual protocol transports and a synthetic Bot API.
use super::*;
use crate::{
    repositories::RouteKind,
    services::{ingress::IngressService, unix_millis},
};
use std::os::unix::fs::MetadataExt;

#[tokio::test]
async fn attachments_expose_safe_own_handles_and_confined_files_across_both_protocols() {
    let mut fixture = Fixture::new();
    for name in ["exports", "downloads"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(fixture.directory.join(name))
            .unwrap();
    }
    fixture.config.files.allowed_roots = vec![fixture.directory.join("exports")];
    fixture.config.downloads.enabled = true;
    fixture.config.downloads.roots = vec![fixture.directory.join("downloads")];
    let now = unix_millis().unwrap();
    let mut db = Database::open(&fixture.config.storage, &fixture.config.limits).unwrap();
    let owner = db.register_session(None, None, "Owner", None, now).unwrap();
    let observer = db
        .register_session(None, None, "Observer", None, now)
        .unwrap();
    let reservation = db.reserve_route(&owner.session_key).unwrap();
    db.persist_route(reservation, -101, 11, RouteKind::Text, now, now)
        .unwrap();
    let update = serde_json::from_value(json!({"update_id":1,"message":{
        "message_id":23,"date":now/1000,"chat":{"id":-101},"from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":11},"document":{"file_id":"private-file",
        "file_unique_id":"stable","file_name":"../../untrusted.txt","mime_type":"text/plain","file_size":1}}})).unwrap();
    IngressService::new(&fixture.config)
        .unwrap()
        .ingest(&mut db, &update, now)
        .unwrap();
    let source = db.inbox_snapshot(&owner.session_key, None, 1, now).unwrap();
    let source_expiry = now + 600_000;
    rusqlite::Connection::open(&fixture.config.storage.path)
        .unwrap()
        .execute(
            "UPDATE attachment_records SET expires_at = ?1",
            [source_expiry],
        )
        .unwrap();
    let metadata = json!({"ok":true,"result":{"file_id":"private-file","file_unique_id":"stable","file_size":1,"file_path":"documents/file.bin"}});
    let telegram = tools::Telegram::new(
        &fixture.config,
        vec![
            (200, metadata.clone()),
            (200, json!(7)),
            (200, metadata),
            (200, json!(7)),
            (
                400,
                json!({"ok":false,"error_code":400,"description":"private diagnostic"}),
            ),
        ],
    )
    .await;
    let handler = telegram.handler(&fixture);
    let mut http = fixture.serve(handler.clone(), None).await;
    tools::assert_tool_catalog(&http.rpc("tools/list", json!({})).await["result"]["tools"]);
    let resource = http
        .rpc("resources/read", json!({"uri":owner.inbox_uri()}))
        .await;
    let snapshot: Value =
        serde_json::from_str(resource["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    let handle = snapshot["replies"][0]["attachments"][0]["attachment_handle"]
        .as_str()
        .unwrap();
    assert_eq!(handle.len(), 64);
    assert_eq!(
        db.inbox_snapshot(&owner.session_key, None, 1, now).unwrap(),
        source
    );
    tools::failure(
        &http
            .tool(
                "telegram_get_attachment",
                json!({"session_key":observer.session_key,"attachment_handle":handle}),
            )
            .await,
        "ATTACHMENT_UNAVAILABLE",
    );
    assert_eq!(telegram.request_count(), 0);
    let args = json!({"session_key":owner.session_key,"attachment_handle":handle});
    for field in ["file_id", "url", "path"] {
        let mut invalid = args.clone();
        invalid[field] = json!("untrusted");
        let response = http
            .rpc(
                "tools/call",
                json!({"name":"telegram_get_attachment","arguments":invalid}),
            )
            .await;
        assert_eq!(response["error"]["code"], -32602);
    }
    assert_eq!(telegram.request_count(), 0);
    let modern = http
        .rpc(
            "tools/call",
            json!({"name":"telegram_get_attachment","arguments":args}),
        )
        .await;
    let response = http
        .legacy_post(legacy::initialization(LEGACY_VERSION), None)
        .send()
        .await
        .unwrap();
    let transport = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    legacy::sse_message(response).await;
    assert_eq!(
        status(http.legacy_post(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            Some(&transport)
        ))
        .await,
        StatusCode::ACCEPTED
    );
    let response = http
        .legacy_post(
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
        "params":{"name":"telegram_get_attachment","arguments":args}}),
            Some(&transport),
        )
        .send()
        .await
        .unwrap();
    let legacy = legacy::sse_message(response).await;
    for (response, modern) in [(modern, true), (legacy, false)] {
        let payload = &response["result"]["structuredContent"];
        assert_eq!(payload["attachment"]["attachment_handle"], handle);
        assert_eq!(payload["attachment"]["filename"], "../../untrusted.txt");
        assert_eq!(payload["size_bytes"], 1);
        assert!(!payload.to_string().contains("private-file"));
        let path = std::path::Path::new(payload["path"].as_str().unwrap());
        assert!(path.starts_with(&fixture.config.downloads.roots[0]));
        assert_eq!(fs::read(path).unwrap(), b"7");
        let metadata = fs::metadata(path).unwrap();
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(
            payload["file_identity"]["device"],
            format!("{:016x}", metadata.dev())
        );
        assert_eq!(
            payload["file_identity"]["inode"],
            format!("{:016x}", metadata.ino())
        );
        assert_eq!(payload["expires_at"], source_expiry);
        assert_eq!(payload["attachment"]["expires_at"], source_expiry);
        let recorded_expiry: i64 = rusqlite::Connection::open(&fixture.config.storage.path)
            .unwrap()
            .query_row(
                "SELECT expires_at FROM download_records WHERE download_id = ?1",
                [payload["download_id"].as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded_expiry, source_expiry);
        let mut expected = json!({"structuredContent":payload,"isError":false,
            "content":[{"type":"text","text":serde_json::to_string(payload).unwrap()}]});
        if modern {
            expected["resultType"] = json!("complete");
        }
        assert_eq!(response, json!({"jsonrpc":"2.0","id":7,"result":expected}));
    }
    let claimed = http
        .tool("telegram_inbox", json!({"session_key":owner.session_key}))
        .await;
    assert_eq!(
        claimed["structuredContent"]["replies"][0]["attachments"][0]["attachment_handle"],
        handle
    );
    let before = db
        .inbox_snapshot(&owner.session_key, None, 1, unix_millis().unwrap())
        .unwrap();
    let failed = http.tool("telegram_get_attachment", args).await;
    tools::failure(&failed, "DOWNLOAD_UNAVAILABLE");
    assert!(!failed.to_string().contains("private diagnostic"));
    assert_eq!(
        db.inbox_snapshot(&owner.session_key, None, 1, unix_millis().unwrap())
            .unwrap(),
        before
    );
    assert_eq!(db.update_cursor().unwrap(), Some(1));
    assert_eq!(
        fs::read_dir(&fixture.config.downloads.roots[0])
            .unwrap()
            .count(),
        2
    );
    handler.stop();
    http.stop.cancel();
    (&mut http.task).await.unwrap().unwrap();
    handler.drain().await.unwrap();
}
