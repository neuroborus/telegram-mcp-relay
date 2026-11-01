use super::*;
use telegram_mcp_relay::{dto::HistoryRequest, services::history::HistoryService};

fn fixture() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.0.telegram.chat_id = -101;
    fixture.0.server.auth_token_env = Some("SYNTHETIC_AUTH".into());
    fixture
}

fn policy(fixture: &Fixture) -> Config {
    Config {
        server: fixture.0.server.clone(),
        telegram: fixture.0.telegram.clone(),
        history: fixture.0.history.clone(),
        storage: fixture.0.storage.clone(),
        limits: fixture.0.limits.clone(),
        ..Config::default()
    }
}

fn request(key: &str) -> HistoryRequest {
    HistoryRequest {
        session_key: key.into(),
        chat_id: None,
        session_display_id: None,
        query: None,
        cursor: None,
        limit: None,
    }
}

#[test]
fn shared_observation_revalidates_policy_and_never_exposes_or_mutates_private_recovery_state() {
    let fixture = fixture();
    let mut db = fixture.open();
    let observer = db
        .register_session(Some("private-observer"), None, "Observer", None, 0)
        .unwrap();
    let owner = db
        .register_session(Some("private-owner"), None, "Owner", Some("module"), 0)
        .unwrap();
    let route = db.reserve_route(&owner.session_key).unwrap();
    db.persist_route(route, -101, 5, RouteKind::Text, 0, 0)
        .unwrap();
    let update = serde_json::from_value(json!({"update_id":1,"message":{"message_id":7,"date":0,
        "chat":{"id":-101},"from":{"id":9,"is_bot":false},"text":"Reply for Owner",
        "reply_to_message":{"message_id":5}}}))
    .unwrap();
    IngressService::new(&fixture.0)
        .unwrap()
        .ingest(&mut db, &update, 10)
        .unwrap();
    let inbox = InboxService::new(&fixture.0.delivery).unwrap();
    let claim = inbox
        .claim(&mut db, &owner.session_key, None, 1, 11)
        .unwrap()
        .pop()
        .unwrap();
    inbox
        .processing(
            &mut db,
            &owner.session_key,
            &claim.reply.reply_id,
            &claim.claim_token,
            true,
            12,
        )
        .unwrap();
    let resumed = inbox
        .reconcile_processing(
            &mut db,
            &owner.session_key,
            &claim.reply.reply_id,
            &claim.claim_token,
            "private-reconciliation",
            13,
        )
        .unwrap();
    let recovery = || {
        fixture
            .sql()
            .query_row(
                "SELECT json_array(state, claim_token, claimed_at, processing_started_at,
         reconciliation_request_id, reconciliation_from_token) FROM replies",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
    };
    let before = recovery();
    let mut excluded = input();
    excluded.source.chat_id = -102;
    excluded.body = Some("Foreign retained body");
    db.record_message(&excluded, 14).unwrap();
    excluded.source.chat_id = -101;
    excluded.source.message_id = 99;
    excluded.direction = MessageDirection::Outgoing;
    excluded.body = Some("Insufficient legacy provenance");
    db.record_message(&excluded, 14).unwrap();
    let mut outgoing = input();
    outgoing.source.message_id = 8;
    outgoing.source.sender_user_id = None;
    outgoing.direction = MessageDirection::Outgoing;
    outgoing.session = Some(&owner);
    outgoing.body = Some("Report from Owner");
    let message = db.record_message(&outgoing, 14).unwrap();
    db.record_attachment(
        &AttachmentInput {
            source: outgoing.source,
            reply_id: None,
            message_handle: Some(&message.message_handle),
            file_id: "private-adapter-file",
            file_unique_id: "private-adapter-unique",
            filename: Some("synthetic.txt"),
            mime_type: Some("text/plain"),
            file_size: Some(12),
            kind: AttachmentKind::Document,
            width: None,
            height: None,
        },
        14,
    )
    .unwrap();
    let service = HistoryService::new(&fixture.0).unwrap();
    let mut query = request(&observer.session_key);
    let usage_before = db.history_usage().unwrap();
    let coverage_before = db.history_coverage().unwrap();
    let owner_before = db.session(&owner.session_key).unwrap();
    let page = service.read(&db, &query, 20).unwrap();
    assert_eq!(page.messages.len(), 2);
    assert!(page.coverage.local_only && page.coverage.possible_gaps);
    assert_eq!(page.coverage.observation_start, None);
    assert_eq!(
        page.coverage.retention_days,
        fixture.0.history.retention_days
    );
    assert_eq!(
        page.messages[1].attachments[0].filename.as_deref(),
        Some("synthetic.txt")
    );
    let rendered = serde_json::to_string(&page).unwrap();
    for secret in [
        &owner.session_key,
        &observer.session_key,
        &claim.claim_token,
        &claim.reply.reply_id,
        &resumed.claim_token,
        "private-owner",
        "private-observer",
        "private-reconciliation",
        "private-adapter-file",
        "private-adapter-unique",
        "Foreign retained body",
        "Insufficient legacy provenance",
    ] {
        assert!(!rendered.contains(secret));
    }
    assert_eq!(recovery(), before);
    assert_eq!(db.session(&observer.session_key).unwrap(), observer);
    query.session_display_id = page.messages[0].session_display_id.clone();
    assert_eq!(service.read(&db, &query, 21).unwrap().messages.len(), 2);
    query.chat_id = Some(-102);
    assert!(matches!(
        service.read(&db, &query, 21),
        Err(AppError::HistoryForbidden)
    ));
    query.chat_id = None;
    let mut revoked = policy(&fixture);
    revoked.telegram.allowed_user_ids = vec![10];
    assert_eq!(
        HistoryService::new(&revoked)
            .unwrap()
            .read(&db, &query, 21)
            .unwrap()
            .messages
            .len(),
        1
    );
    revoked.history.enabled = false;
    assert!(matches!(
        HistoryService::new(&revoked).unwrap().read(&db, &query, 21),
        Err(AppError::HistoryDisabled)
    ));
    assert_eq!(recovery(), before);
    assert_eq!(db.history_usage().unwrap(), usage_before);
    assert_eq!(db.history_coverage().unwrap(), coverage_before);
    assert_eq!(db.session(&owner.session_key).unwrap(), owner_before);
    query.session_key = "unknown".into();
    assert!(matches!(
        service.read(&db, &query, 21),
        Err(AppError::SessionNotFound)
    ));
}

#[test]
fn bounded_sparse_keyset_scans_resume_across_ties_expiry_and_cleanup_without_losing_matches() {
    let mut fixture = fixture();
    fixture.0.history.scan_records = 2;
    fixture.0.history.page_limit = 1;
    let mut db = fixture.open();
    let session = db
        .register_session(None, None, "Observer", None, 0)
        .unwrap();
    for id in 1..=7 {
        let mut record = input();
        record.source.message_id = id;
        record.body = Some(if [3, 4, 7].contains(&id) {
            "needle"
        } else {
            "unrelated"
        });
        if id == 4 {
            record.source.sender_user_id = Some(10);
        }
        db.record_message(&record, 10).unwrap();
        // Fix tie order so the first bounded page ends on the expired anchor.
        fixture
            .sql()
            .execute(
                "UPDATE journal_records SET message_handle = ?1 WHERE message_id = ?2",
                (format!("{id:064x}"), id),
            )
            .unwrap();
    }
    fixture
        .sql()
        .execute(
            "UPDATE journal_records SET expires_at = 50 WHERE message_id = 2",
            [],
        )
        .unwrap();
    let service = HistoryService::new(&fixture.0).unwrap();
    let mut query = request(&session.session_key);
    query.query = Some("needle".into());
    let mut ids = Vec::new();
    let mut saw_empty_continuation = false;
    let mut saved = None;
    for _ in 0..10 {
        let page = service.read(&db, &query, 100).unwrap();
        ids.extend(page.messages.iter().map(|m| m.message_id));
        saw_empty_continuation |= page.messages.is_empty() && page.scan_incomplete;
        assert_eq!(page.scan_incomplete, page.next_cursor.is_some());
        if saved.is_none() {
            saved = page.next_cursor.clone();
        }
        clean(&mut db, 100);
        query.cursor = page.next_cursor;
        if query.cursor.is_none() {
            break;
        }
    }
    ids.sort();
    assert_eq!(ids, [3, 7]);
    assert!(saw_empty_continuation);
    query.cursor = saved;
    query.query = Some("different".into());
    assert!(matches!(
        service.read(&db, &query, 100),
        Err(AppError::InvalidHistoryCursor)
    ));
    query.query = Some("needle".into());
    assert!(matches!(
        service.read(&db, &query, DAY + 100),
        Err(AppError::InvalidHistoryCursor)
    ));
    query.cursor = Some("not-a-cursor".into());
    assert!(matches!(
        service.read(&db, &query, 100),
        Err(AppError::InvalidHistoryCursor)
    ));
}

#[test]
fn response_bytes_query_input_and_search_work_are_bounded_without_truncation() {
    let mut fixture = fixture();
    fixture.0.history.response_bytes = 2_048;
    let mut db = fixture.open();
    let session = db
        .register_session(None, None, "Observer", None, 0)
        .unwrap();
    let body = "x".repeat(100);
    for id in 1..=10 {
        let mut record = input();
        record.source.message_id = id;
        record.body = Some(&body);
        db.record_message(&record, id).unwrap();
    }
    let service = HistoryService::new(&fixture.0).unwrap();
    let mut query = request(&session.session_key);
    let mut ids = Vec::new();
    for index in 0..10 {
        let page = service.read(&db, &query, 100).unwrap();
        assert!(!page.messages.is_empty() && page.messages.len() < 10);
        if index == 0 {
            assert!(page.next_cursor.is_some());
        }
        assert_eq!(page.scan_incomplete, page.next_cursor.is_some());
        assert!(serde_json::to_vec(&page).unwrap().len() <= 2_048);
        assert!(
            page.messages
                .iter()
                .all(|m| m.text_or_caption.as_deref() == Some(body.as_str()))
        );
        ids.extend(page.messages.iter().map(|m| m.message_id));
        query.cursor = page.next_cursor;
        if query.cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids, (1..=10).collect::<Vec<_>>());
    assert!(query.cursor.is_none());
    query.query = Some("界".repeat(100));
    assert!(matches!(
        service.read(&db, &query, 100),
        Err(AppError::InvalidRequest(_))
    ));
    query.query = None;
    query.limit = Some(0);
    assert!(matches!(
        service.read(&db, &query, 100),
        Err(AppError::InvalidRequest(_))
    ));
    query.limit = None;
    let large = "\u{1f}".repeat(1_000);
    let mut record = input();
    record.source.message_id = 11;
    record.body = Some(&large);
    db.record_message(&record, 11).unwrap();
    query.query = Some("\u{1f}".into());
    assert!(matches!(
        service.read(&db, &query, 100),
        Err(AppError::HistoryResponseTooLarge)
    ));
    let mut sparse = policy(&fixture);
    sparse.history.response_bytes = 65_536;
    let large = "q".repeat(50_000);
    for id in 12..=15 {
        let mut record = input();
        record.source.message_id = id;
        record.body = Some(&large);
        db.record_message(&record, id).unwrap();
    }
    query.query = Some("absent".into());
    let page = HistoryService::new(&sparse)
        .unwrap()
        .read(&db, &query, 100)
        .unwrap();
    assert!(page.messages.is_empty() && page.scan_incomplete);
}
