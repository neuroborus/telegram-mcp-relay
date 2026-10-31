use std::{fs, os::unix::fs::DirBuilderExt};

use rusqlite::Connection;
use serde_json::{Value, json};
use telegram_mcp_relay::{
    clients::telegram::Update,
    config::{Config, Limits, Storage},
    error::AppError,
    repositories::{Database, IngestOutcome, RouteKind},
    services::ingress::IngressService,
};

const DAY: i64 = 86_400_000;

struct Fixture(Config);

impl Fixture {
    fn new(limits: Limits) -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("ingress-test-{:016x}", u64::from_ne_bytes(random)));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let mut config = Config {
            limits,
            storage: Storage {
                path: directory.join("relay.sqlite"),
                reply_ttl_days: 2,
                ..Storage::default()
            },
            ..Config::default()
        };
        config.telegram.allowed_chat_ids = vec![-101, -102];
        config.telegram.allowed_user_ids = vec![7];
        Self(config)
    }

    fn open(&self) -> Database {
        Database::open(&self.0.storage, &self.0.limits).unwrap()
    }
    fn service(&self) -> IngressService {
        IngressService::new(&self.0).unwrap()
    }
    fn sql(&self) -> Connection {
        Connection::open(&self.0.storage.path).unwrap()
    }

    fn seed(&self, database: &mut Database) -> (String, String) {
        let a = database
            .register_session(None, None, "Same", None, 100)
            .unwrap()
            .session_key;
        let b = database
            .register_session(None, None, "Same", None, 100)
            .unwrap()
            .session_key;
        for (key, chat, kind) in [(&a, -101, RouteKind::Text), (&b, -102, RouteKind::Document)] {
            let reservation = database.reserve_route(key).unwrap();
            database
                .persist_route(reservation, chat, 5, kind, 100, 100)
                .unwrap();
        }
        (a, b)
    }

    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.storage.path.parent().unwrap()).unwrap();
    }
}

fn input(id: i64, message: i64, chat: i64) -> Value {
    json!({"update_id":id, "message":{"message_id":message, "date":1,
        "chat":{"id":chat}, "from":{"id":7,"is_bot":false},
        "reply_to_message":{"message_id":5,"text":"discarded quoted body"},
        "text":"Synthetic reply"}, "discarded":"full update secret"})
}

fn ingest(
    service: &IngressService,
    database: &mut Database,
    value: Value,
    now: i64,
) -> Result<IngestOutcome, AppError> {
    let update: Update = serde_json::from_value(value).unwrap();
    service.ingest(database, &update, now)
}

fn stored(result: IngestOutcome, key: &str) -> String {
    let debug = format!("{result:?}");
    match result {
        IngestOutcome::ReplyStored {
            reply_id,
            session_key,
        } => {
            assert_eq!(session_key, key);
            assert!(!debug.contains(key) && !debug.contains(&reply_id));
            reply_id
        }
        other => panic!("expected committed reply, got {other:?}"),
    }
}

#[test]
fn distinct_replies_and_attachment_only_documents_route_atomically_across_restart() {
    let fixture = Fixture::new(Limits::default());
    let mut database = fixture.open();
    let service = fixture.service();
    let (a, b) = fixture.seed(&mut database);
    let long_text = "界".repeat(1_001); // Incoming text has no outbound 1,000-scalar cap.
    let mut first = input(11, 20, -101);
    first["message"]["text"] = long_text.clone().into();
    let id = stored(
        ingest(&service, &mut database, first.clone(), 1_000).unwrap(),
        &a,
    );
    // A commit receipt is usable immediately from another connection, before any signaling.
    assert_eq!(fixture.count("replies"), 1);
    let id2 = stored(
        ingest(&service, &mut database, input(12, 21, -101), 1_001).unwrap(),
        &a,
    );
    assert_ne!(id, id2);
    let mut document = input(13, 20, -102);
    document["message"].as_object_mut().unwrap().remove("text");
    document["message"]["document"] = json!({"file_id":"synthetic-file", "file_unique_id":"unique-file",
        "file_name":"../../untrusted.txt", "mime_type":"text/plain", "file_size":512});
    stored(
        ingest(&service, &mut database, document, 1_002).unwrap(),
        &b,
    );
    let (text, metadata, created, expires): (Option<String>, String, i64, i64) = fixture.sql().query_row(
        "SELECT text_or_caption, attachment_metadata_json, created_at, expires_at FROM replies WHERE telegram_update_id = 13",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).unwrap();
    assert_eq!(text, None);
    let metadata: Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(metadata["document"]["file_name"], "../../untrusted.txt");
    assert_eq!(metadata["document"]["file_id"], "synthetic-file");
    assert_eq!(metadata["document"]["file_size"], 512);
    assert_eq!((created, expires), (1_002, 1_002 + 2 * DAY));
    assert_eq!(database.session(&a).unwrap().last_activity_at, 1_001);
    assert_eq!(database.session(&b).unwrap().last_activity_at, 1_002);
    drop(database); // Simulate commit followed by loss before signal/next poll.
    let mut database = fixture.open();
    assert_eq!(database.update_cursor().unwrap(), Some(13));
    assert_eq!(
        ingest(&service, &mut database, first, 2_000),
        Ok(IngestOutcome::Duplicate)
    );
    assert_eq!(
        ingest(&service, &mut database, input(14, 20, -101), 2_000),
        Ok(IngestOutcome::Duplicate)
    );
    assert_eq!(database.update_cursor().unwrap(), Some(14));
    assert_eq!(fixture.count("replies"), 3);
    let (saved_id, saved_text): (String, String) = fixture
        .sql()
        .query_row(
            "SELECT reply_id, text_or_caption FROM replies WHERE telegram_update_id = 11",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((saved_id, saved_text), (id, long_text));
    let bytes = fs::read(&fixture.0.storage.path).unwrap();
    for discarded in ["discarded quoted body", "full update secret"] {
        assert!(
            !bytes
                .windows(discarded.len())
                .any(|w| w == discarded.as_bytes())
        );
    }
}

#[test]
fn authorization_ignored_input_and_expired_or_unknown_routes_have_safe_durable_dispositions() {
    let fixture = Fixture::new(Limits::default());
    let mut database = fixture.open();
    let service = fixture.service();
    let (a, _) = fixture.seed(&mut database);
    for (i, patch) in [
        json!({"from":{"id":8,"is_bot":false}}),
        json!({"chat":{"id":-103}}),
        json!({"from":{"id":7,"is_bot":true}}),
        json!({"from":null}),
        json!({"reply_to_message":null}),
        json!({"text":null}),
        json!({"message_id":0}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut value = input(i as i64, 20, -101);
        value["message"]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert_eq!(
            ingest(&service, &mut database, value, 1_000),
            Ok(IngestOutcome::Ignored)
        );
    }
    let mut unknown = input(10, 30, -101);
    unknown["message"]["reply_to_message"]["message_id"] = 6.into();
    unknown["message"]["text"] = "x".repeat(65_537).into(); // Discarded, never inbox payload.
    assert_eq!(
        ingest(&service, &mut database, unknown.clone(), 1_000),
        Ok(IngestOutcome::Unroutable)
    );
    assert_eq!(
        ingest(&service, &mut database, input(11, 31, -101), 100 + 30 * DAY),
        Ok(IngestOutcome::Unroutable)
    );
    assert_eq!(fixture.count("replies"), 0);
    assert_eq!(fixture.count("update_dispositions"), 3);
    let leaked: i64 = fixture
        .sql()
        .query_row(
            "SELECT count(*) FROM update_dispositions WHERE kind = 'ignored'
         AND (telegram_chat_id IS NOT NULL OR telegram_message_id IS NOT NULL)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(leaked, 0);
    assert_eq!(database.session(&a).unwrap().last_activity_at, 100);
    drop(database);
    let mut database = fixture.open();
    assert_eq!(
        ingest(&service, &mut database, unknown, 100 + 30 * DAY),
        Ok(IngestOutcome::Duplicate)
    );
    assert_eq!(fixture.count("update_dispositions"), 3);
}

#[test]
fn lower_ids_after_week_long_idle_are_new_but_retained_redeliveries_stay_duplicates() {
    let mut fixture = Fixture::new(Limits::default());
    fixture.0.storage.reply_ttl_days = 30;
    let mut database = fixture.open();
    let service = fixture.service();
    let (a, _) = fixture.seed(&mut database);
    let old = input(100, 20, -101);
    let old_id = stored(
        ingest(&service, &mut database, old.clone(), 1_000).unwrap(),
        &a,
    );
    let mut unknown = input(101, 21, -101);
    unknown["message"]["reply_to_message"]["message_id"] = 6.into();
    assert_eq!(
        ingest(&service, &mut database, unknown.clone(), 1_001),
        Ok(IngestOutcome::Unroutable)
    );
    drop(database);
    let mut database = fixture.open();
    let now = 8 * DAY;
    assert_eq!(
        ingest(&service, &mut database, old.clone(), now),
        Ok(IngestOutcome::Duplicate)
    );
    assert_eq!(database.update_cursor().unwrap(), Some(101));
    let fresh = input(50, 22, -101);
    let fresh_id = stored(
        ingest(&service, &mut database, fresh.clone(), now).unwrap(),
        &a,
    );
    assert_ne!(old_id, fresh_id);
    assert_eq!(database.update_cursor().unwrap(), Some(50));
    drop(database); // Commit survives loss before the next Telegram poll or notification.
    let mut database = fixture.open();
    for value in [fresh, old, unknown] {
        assert_eq!(
            ingest(&service, &mut database, value, now + 1),
            Ok(IngestOutcome::Duplicate)
        );
        assert_eq!(database.update_cursor().unwrap(), Some(50));
    }
    stored(
        ingest(&service, &mut database, input(51, 23, -101), now + 2).unwrap(),
        &a,
    );
    assert_eq!(database.update_cursor().unwrap(), Some(51));
    assert_eq!(fixture.count("replies"), 3);
    assert_eq!(fixture.count("update_dispositions"), 1);
}

#[test]
fn unauthorized_flood_compacts_without_consuming_full_receipt_or_metadata_budgets() {
    let fixture = Fixture::new(Limits {
        receipts: 1,
        // Two sessions/routes, one receipt and one accepted reply fill variable metadata.
        metadata_bytes: 2 * (128 + 64 + 4) + 2 * (128 + 64) + (128 + 7 + 64 + 5) + 256,
        ..Limits::default()
    });
    let mut database = fixture.open();
    let service = fixture.service();
    let (a, _) = fixture.seed(&mut database);
    fixture
        .sql()
        .execute(
            "INSERT INTO acknowledgment_receipts VALUES('receipt', ?1, 'valid', 999, 100, 10000)",
            [&a],
        )
        .unwrap();
    for id in 1..=32 {
        let mut value = input(id, 20 + id, -101);
        value["message"]["from"]["id"] = 8.into();
        assert_eq!(
            ingest(&service, &mut database, value, 1_000 + id),
            Ok(IngestOutcome::Ignored)
        );
    }
    assert_eq!(fixture.count("update_dispositions"), 1);
    assert_eq!(fixture.count("acknowledgment_receipts"), 1);
    drop(database);
    let mut database = fixture.open();
    assert_eq!(database.update_cursor().unwrap(), Some(32));
    stored(
        ingest(&service, &mut database, input(33, 100, -101), 2_000).unwrap(),
        &a,
    );
    let mut unauthorized = input(34, 101, -101);
    unauthorized["message"]["from"]["id"] = 8.into();
    assert_eq!(
        ingest(&service, &mut database, unauthorized.clone(), 2_001),
        Ok(IngestOutcome::Ignored)
    );
    for value in [unauthorized, input(999, 102, -101)] {
        assert_eq!(
            ingest(&service, &mut database, value, 2_002),
            Ok(IngestOutcome::Duplicate)
        );
        assert_eq!(database.update_cursor().unwrap(), Some(34));
    }
    assert_eq!(fixture.count("replies"), 1);
    assert_eq!(fixture.count("acknowledgment_receipts"), 1);
    assert_eq!(fixture.count("update_dispositions"), 1);
}

#[test]
fn failures_after_reply_and_activity_or_at_cursor_roll_back_everything() {
    let fixture = Fixture::new(Limits::default());
    let mut database = fixture.open();
    let service = fixture.service();
    let (a, _) = fixture.seed(&mut database);
    for table in ["sessions", "telegram_cursor"] {
        fixture
            .sql()
            .execute_batch(&format!(
                "CREATE TRIGGER synthetic_failure BEFORE UPDATE ON {table}
            BEGIN SELECT RAISE(ABORT, 'private SQL detail'); END"
            ))
            .unwrap();
        let result = ingest(&service, &mut database, input(11, 20, -101), 1_000);
        assert_eq!(result, Err(AppError::StorageFailure));
        assert!(!format!("{result:?}").contains("private SQL detail"));
        assert_eq!(fixture.count("replies"), 0);
        assert_eq!(database.update_cursor().unwrap(), None);
        assert_eq!(database.session(&a).unwrap().last_activity_at, 100);
        if table == "telegram_cursor" {
            assert_eq!(
                ingest(&service, &mut database, json!({"update_id":11}), 1_000),
                Err(AppError::StorageFailure)
            );
            assert_eq!(fixture.count("update_dispositions"), 0);
        }
        fixture
            .sql()
            .execute_batch("DROP TRIGGER synthetic_failure")
            .unwrap();
    }
    stored(
        ingest(&service, &mut database, input(11, 20, -101), 1_000).unwrap(),
        &a,
    );
    for (id, now) in [(-1, 1_000), (12, -1), (12, i64::MAX)] {
        assert!(matches!(
            ingest(&service, &mut database, input(id, 21, -101), now),
            Err(AppError::InvalidRequest(_))
        ));
        assert_eq!(database.update_cursor().unwrap(), Some(11));
    }
}

#[test]
fn item_payload_and_metadata_pressure_preserve_replies_and_outstanding_send_admission() {
    for limits in [
        Limits {
            inbox_items: 1,
            ..Limits::default()
        },
        Limits {
            reply_bytes: 15,
            inbox_bytes: 15,
            ..Limits::default()
        },
        // Two sessions/routes, one reply, and one admitted route exactly fill metadata.
        Limits {
            metadata_bytes: 2 * (128 + 64 + 4) + 2 * (128 + 64) + 256 + 192,
            ..Limits::default()
        },
    ] {
        let fixture = Fixture::new(limits);
        let mut database = fixture.open();
        let service = fixture.service();
        let (a, _) = fixture.seed(&mut database);
        let reservation = database.reserve_route(&a).unwrap();
        stored(
            ingest(&service, &mut database, input(11, 20, -101), 1_000).unwrap(),
            &a,
        );
        assert_eq!(
            ingest(&service, &mut database, input(12, 21, -101), 1_001),
            Err(AppError::StorageCapacity)
        );
        assert_eq!(database.update_cursor().unwrap(), Some(11));
        assert_eq!(database.session(&a).unwrap().last_activity_at, 1_000);
        assert_eq!(fixture.count("replies"), 1);
        database
            .persist_route(reservation, -101, 6, RouteKind::Text, 1_001, 1_001)
            .unwrap();
        assert!(database.route(-101, 6, 1_001).unwrap().is_some());
        // Incoming metadata must also constrain future send/session admissions.
        if fixture.0.limits.metadata_bytes < 2_000 {
            assert!(matches!(
                database.reserve_route(&a),
                Err(AppError::StorageCapacity)
            ));
            assert_eq!(
                database.register_session(None, Some(&a), "Larger title", None, 2_000),
                Err(AppError::StorageCapacity)
            );
        }
    }
}

#[test]
fn serialized_attachment_budget_and_disposition_bound_do_not_advance_unresolved_input() {
    let fixture = Fixture::new(Limits {
        reply_bytes: 256,
        receipts: 1,
        ..Limits::default()
    });
    let mut database = fixture.open();
    let service = fixture.service();
    let (a, _) = fixture.seed(&mut database);
    let mut value = input(11, 20, -101);
    value["message"].as_object_mut().unwrap().remove("text");
    value["message"]["caption"] = "Synthetic photo".into();
    value["message"]["photo"] =
        json!([{"file_id":"photo", "file_unique_id":"photo-unique", "file_size":5}]);
    stored(
        ingest(&service, &mut database, value.clone(), 1_000).unwrap(),
        &a,
    );
    let photo: (String, String) = fixture
        .sql()
        .query_row(
            "SELECT text_or_caption, json_extract(attachment_metadata_json, '$.photo[0].file_id')
         FROM replies WHERE telegram_update_id = 11",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(photo, ("Synthetic photo".into(), "photo".into()));
    value["update_id"] = 12.into();
    value["message"]["message_id"] = 21.into();
    value["message"]["photo"][0]["file_id"] = "\u{0001}".repeat(100).into();
    assert_eq!(
        ingest(&service, &mut database, value, 1_001),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(database.update_cursor().unwrap(), Some(11));
    assert_eq!(
        ingest(&service, &mut database, json!({"update_id":12}), 1_002),
        Ok(IngestOutcome::Ignored)
    );
    assert_eq!(
        ingest(&service, &mut database, json!({"update_id":13}), 1_003),
        Ok(IngestOutcome::Ignored)
    );
    let mut unknown = input(14, 21, -101);
    unknown["message"]["reply_to_message"]["message_id"] = 6.into();
    assert_eq!(
        ingest(&service, &mut database, unknown.clone(), 1_004),
        Ok(IngestOutcome::Unroutable)
    );
    unknown["update_id"] = 15.into();
    unknown["message"]["message_id"] = 22.into();
    assert_eq!(
        ingest(&service, &mut database, unknown, 1_005),
        Err(AppError::StorageCapacity)
    );
    assert_eq!(database.update_cursor().unwrap(), Some(14));
    assert_eq!(fixture.count("replies"), 1);
}

#[test]
fn physical_disk_limit_rolls_back_ingestion_without_losing_committed_replies() {
    let mut fixture = Fixture::new(Limits {
        reply_bytes: 1_048_576,
        ..Limits::default()
    });
    fixture.0.storage.max_database_bytes = 1_048_576;
    let mut database = fixture.open();
    let service = fixture.service();
    let (a, _) = fixture.seed(&mut database);
    let mut value = input(11, 20, -101);
    value["message"]["text"] = "x".repeat(524_288).into();
    stored(
        ingest(&service, &mut database, value.clone(), 1_000).unwrap(),
        &a,
    );
    value["update_id"] = 12.into();
    value["message"]["message_id"] = 21.into();
    assert_eq!(
        ingest(&service, &mut database, value, 1_001),
        Err(AppError::StorageCapacity)
    );
    drop(database);
    let database = fixture.open();
    assert_eq!(database.update_cursor().unwrap(), Some(11));
    assert_eq!(database.session(&a).unwrap().last_activity_at, 1_000);
    assert_eq!(fixture.count("replies"), 1);
}
