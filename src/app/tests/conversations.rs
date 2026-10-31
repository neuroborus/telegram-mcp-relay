//! Interleaved conversation acceptance through the complete daemon and both HTTP peers.

use super::*;
use std::collections::HashSet;

const TEXT_A: &str = "outbound-only text for thread A";
const TEXT_B: &str = "outbound-only later text for thread B";
const TEXT_C: &str = "outbound-only text for thread C";
const CAPTION: &str = "outbound-only document description";
const DOCUMENT: &[u8] = b"outbound-only document bytes\0\xff\n";

pub(super) fn accepted(id: i64) -> Value {
    json!({"ok":true,"result":{"message_id":id,"chat":{"id":-101},
        "date":unix_millis().unwrap()/1000}})
}

fn assert_document(request: &TelegramRequest, caption: &str) {
    assert_eq!(request.method, "sendDocument");
    assert_eq!(request.part("chat_id"), b"-101");
    assert_eq!(request.part("caption"), caption.as_bytes());
    assert_eq!(request.part("document"), DOCUMENT);
}

pub(super) async fn snapshot(address: SocketAddr, session: &Value) -> Value {
    let result = rpc(
        address,
        "resources/read",
        json!({"uri":session["inbox_uri"]}),
    )
    .await;
    serde_json::from_str(result["contents"][0]["text"].as_str().unwrap()).unwrap()
}

pub(super) fn incoming(update: i64, parent: i64, text: Option<&str>) -> Value {
    let mut message = json!({"message_id":update+1000,"date":1,"chat":{"id":-101},
        "from":{"id":7,"is_bot":false},"reply_to_message":{"message_id":parent,
            "from":{"id":99,"is_bot":true}}});
    if let Some(text) = text {
        message["text"] = text.into();
    }
    json!({"update_id":update,"message":message})
}

#[tokio::test]
async fn three_sessions_route_interleaved_old_text_and_document_replies_after_rename() {
    let mut fixture = Fixture::new().await;
    let mut mock = Mock::new(&fixture.config).await;
    let address = fixture.config.server.listen;
    let mut running = fixture.start(mock.client.clone());
    mock.next("getWebhookInfo")
        .await
        .respond(json!({"ok":true,"result":{"url":"","pending_update_count":0}}));
    let poll = mock.next("getUpdates").await;
    let (a, b, c) = tokio::join!(
        tool(
            address,
            "telegram_register",
            json!({"external_session_id":"a","title":"Shared"})
        ),
        tool(
            address,
            "telegram_register",
            json!({"external_session_id":"b","title":"Shared"})
        ),
        tool(
            address,
            "telegram_register",
            json!({"external_session_id":"c","title":"Shared"})
        ),
    );
    let keys: HashSet<_> = [&a, &b, &c]
        .map(|s| s["session_key"].as_str().unwrap())
        .into();
    assert_eq!(keys.len(), 3);
    let path = fixture.directory.join("outbound-report.bin");
    fs::write(&path, DOCUMENT).unwrap();

    // All three sessions have sends in flight; response order cannot define ownership.
    let sends = async {
        tokio::join!(
            tool(
                address,
                "telegram_send",
                json!({"session_key":a["session_key"],"text":TEXT_A})
            ),
            tool(
                address,
                "telegram_send_file",
                json!({"session_key":b["session_key"],"path":path,"caption":CAPTION})
            ),
            tool(
                address,
                "telegram_send",
                json!({"session_key":c["session_key"],"text":TEXT_C})
            ),
        )
    };
    tokio::pin!(sends);
    let mut pending = Vec::new();
    for _ in 0..3 {
        let request = tokio::select! {
            request = mock.requests.recv() => request.unwrap(),
            _ = &mut sends => panic!("sends completed before Telegram responses"),
        };
        let id = if request.method == "sendDocument" {
            assert_document(&request, &format!("Shared\n{CAPTION}"));
            102
        } else {
            assert_eq!(request.method, "sendMessage");
            assert_eq!(request.payload["chat_id"], -101);
            if request.payload["text"] == format!("Shared\n{TEXT_A}") {
                101
            } else {
                assert_eq!(request.payload["text"], format!("Shared\n{TEXT_C}"));
                103
            }
        };
        pending.push((id, request));
    }
    assert_eq!(
        pending
            .iter()
            .map(|(id, _)| *id)
            .collect::<HashSet<_>>()
            .len(),
        3
    );
    for (id, request) in pending.into_iter().rev() {
        request.respond(accepted(id));
    }
    let (sent_a, sent_b, sent_c) = sends.await;
    for (sent, id) in [(sent_a, 101), (sent_b, 102), (sent_c, 103)] {
        assert_eq!(
            sent,
            json!({"telegram_chat_id":-101,"outbound_message_id":id})
        );
    }

    let renamed = tool(
        address,
        "telegram_register",
        json!({
        "external_session_id":"a","session_key":a["session_key"],"title":" Renamed\n A "}),
    )
    .await;
    assert_eq!(renamed["session_key"], a["session_key"]);
    assert_eq!(renamed["inbox_uri"], a["inbox_uri"]);
    assert_eq!(renamed["title"], "Renamed A");

    // Later activity includes both kinds for every session, after A changes its label.
    for (session, name, id, title) in [
        (&a, "telegram_send_file", 104, "Renamed A"),
        (&b, "telegram_send", 105, "Shared"),
        (&c, "telegram_send_file", 106, "Shared"),
    ] {
        let arguments = if name == "telegram_send" {
            json!({"session_key":session["session_key"],"text":TEXT_B})
        } else {
            json!({"session_key":session["session_key"],"path":path,"caption":CAPTION})
        };
        let send = tool(address, name, arguments);
        tokio::pin!(send);
        let request = tokio::select! {
            request = mock.requests.recv() => request.unwrap(),
            _ = &mut send => panic!("send completed before Telegram response"),
        };
        if name == "telegram_send" {
            assert_eq!(request.method, "sendMessage");
            assert_eq!(
                request.payload,
                json!({"chat_id":-101,"text":format!("{title}\n{TEXT_B}"),"entities":[{"type":"bold","offset":0,"length":title.encode_utf16().count()}]})
            );
        } else {
            assert_document(&request, &format!("{title}\n{CAPTION}"));
        }
        request.respond(accepted(id));
        assert_eq!(
            send.await,
            json!({"telegram_chat_id":-101,"outbound_message_id":id})
        );
    }
    fs::remove_file(&path).unwrap(); // Routes do not depend on the original file remaining.

    let document = json!({"file_id":"incoming-document","file_unique_id":"document-unique",
        "file_name":"incoming.txt","mime_type":"text/plain","file_size":17});
    let photos = json!([
        {"file_id":"small-photo","file_unique_id":"small-unique","file_size":10,"width":10,"height":10},
        {"file_id":"large-photo","file_unique_id":"large-unique","file_size":20,"width":20,"height":20}
    ]);
    let mut attachment_only = incoming(200, 102, None);
    attachment_only["message"]["document"] = document.clone();
    let mut old_text = incoming(201, 101, Some("A first reply to old text"));
    old_text["message"]["reply_to_message"]["text"] = format!("Shared\n{TEXT_A}").into();
    let mut photo_reply = incoming(202, 103, None);
    photo_reply["message"]["caption"] = "C photo caption".into();
    photo_reply["message"]["photo"] = photos.clone();
    poll.respond(json!({"ok":true,"result":[
        attachment_only, old_text, photo_reply,
        incoming(203, 102, Some("[Renamed A]\nB second reply to old document")),
        incoming(204, 101, Some("A second reply to old text")),
        incoming(205, 106, Some("C reply to later document")),
        incoming(206, 104, Some("A reply to renamed document"))
    ]}));
    let held_poll = mock.next("getUpdates").await;
    assert_eq!(held_poll.payload["offset"], 207); // All seven replies committed.

    let (snapshot_a, snapshot_b, snapshot_c) = tokio::join!(
        snapshot(address, &a),
        snapshot(address, &b),
        snapshot(address, &c),
    );
    let mut reply_ids = HashSet::new();
    for (page, expected) in [
        (
            &snapshot_a,
            vec![
                (201, 101, Some("A first reply to old text")),
                (204, 101, Some("A second reply to old text")),
                (206, 104, Some("A reply to renamed document")),
            ],
        ),
        (
            &snapshot_b,
            vec![
                (200, 102, None),
                (
                    203,
                    102,
                    Some("[Renamed A]\nB second reply to old document"),
                ),
            ],
        ),
        (
            &snapshot_c,
            vec![
                (202, 103, Some("C photo caption")),
                (205, 106, Some("C reply to later document")),
            ],
        ),
    ] {
        let replies = page["replies"].as_array().unwrap();
        assert_eq!(replies.len(), expected.len());
        assert_eq!(page["next_uri"], Value::Null);
        for (reply, (update, parent, text)) in replies.iter().zip(expected) {
            assert_eq!(reply["telegram_update_id"], update);
            assert_eq!(reply["reply_to_bot_message_id"], parent);
            assert_eq!(reply["telegram_chat_id"], -101);
            assert_eq!(reply["telegram_message_id"], update + 1000);
            assert_eq!(reply["text_or_caption"], json!(text));
            if update != 200 && update != 202 {
                assert_eq!(reply["attachment_metadata"], Value::Null);
            }
            assert_eq!(reply["state"], "pending");
            assert!(reply.get("claim_token").is_none());
            assert!(reply_ids.insert(reply["reply_id"].as_str().unwrap()));
        }
    }
    assert_eq!(snapshot_b["replies"][0]["text_or_caption"], Value::Null);
    assert_eq!(
        snapshot_b["replies"][0]["attachment_metadata"],
        json!({"document":document,"photo":[]})
    );
    let mut expected_photos = photos;
    for photo in expected_photos.as_array_mut().unwrap() {
        photo["file_name"] = Value::Null;
        photo["mime_type"] = Value::Null;
    }
    assert_eq!(
        snapshot_c["replies"][0]["text_or_caption"],
        "C photo caption"
    );
    assert_eq!(
        snapshot_c["replies"][0]["attachment_metadata"],
        json!({"document":null,"photo":expected_photos})
    );

    let (claims_a, claims_b, claims_c) = tokio::join!(
        tool(
            address,
            "telegram_inbox",
            json!({"session_key":a["session_key"]})
        ),
        tool(
            address,
            "telegram_inbox",
            json!({"session_key":b["session_key"]})
        ),
        tool(
            address,
            "telegram_inbox",
            json!({"session_key":c["session_key"]})
        ),
    );
    for (claims, page) in [
        (&claims_a, &snapshot_a),
        (&claims_b, &snapshot_b),
        (&claims_c, &snapshot_c),
    ] {
        let claims = claims["replies"].as_array().unwrap();
        let replies = page["replies"].as_array().unwrap();
        assert_eq!(claims.len(), replies.len());
        for (claim, reply) in claims.iter().zip(replies) {
            assert_eq!(claim["reply_id"], reply["reply_id"]);
            assert_eq!(claim["text_or_caption"], reply["text_or_caption"]);
            assert_eq!(claim["attachment_metadata"], reply["attachment_metadata"]);
            assert_eq!(claim["state"], "claimed");
            assert!(!claim["claim_token"].as_str().unwrap().is_empty());
        }
    }
    running.stop.cancel();
    (&mut running.task).await.unwrap().unwrap();
    drop(held_poll);

    let db = fixture.database();
    assert_eq!(db.update_cursor().unwrap(), Some(206));
    for (id, session, kind) in [
        (101, &a, RouteKind::Text),
        (102, &b, RouteKind::Document),
        (103, &c, RouteKind::Text),
        (104, &a, RouteKind::Document),
        (105, &b, RouteKind::Text),
        (106, &c, RouteKind::Document),
    ] {
        let route = db.route(-101, id, unix_millis().unwrap()).unwrap().unwrap();
        assert_eq!(route.session_key, session["session_key"].as_str().unwrap());
        assert_eq!(route.kind, kind);
    }
    assert_eq!(fixture.count("replies"), 7);
    assert_eq!(fixture.count("acknowledgment_receipts"), 0);
    drop(db);

    // Inspect all surviving fixture files, including SQLite free pages/sidecars if present.
    let files: Vec<_> = fs::read_dir(&fixture.directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files, vec![fixture.config.storage.path.clone()]);
    let stored = fs::read(&files[0]).unwrap();
    assert!(
        stored
            .windows(b"incoming-document".len())
            .any(|b| b == b"incoming-document")
    );
    for marker in [
        TEXT_A.as_bytes(),
        TEXT_B.as_bytes(),
        TEXT_C.as_bytes(),
        CAPTION.as_bytes(),
        DOCUMENT,
        path.to_str().unwrap().as_bytes(),
    ] {
        assert!(!stored.windows(marker.len()).any(|bytes| bytes == marker));
    }
}
