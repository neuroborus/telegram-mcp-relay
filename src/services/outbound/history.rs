use super::*;
use crate::{
    repositories::{CleanupSweep, HistoryUsage, JournalInput, MessageDirection, MessageSource},
    services::ingress::IngressService,
};

fn document(id: i64) -> Value {
    let mut response = sent(id);
    response["result"]["document"] = json!({"file_id":"private-file","file_unique_id":"unique",
        "file_name":"remote.txt","mime_type":"text/plain","file_size":12});
    response
}

#[tokio::test]
async fn missing_returned_document_metadata_never_invents_provenance_and_capture_is_opt_in() {
    for enabled in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.history.enabled = enabled;
        let database = fixture.open();
        let session = fixture.register(&database);
        let mock = Mock::new(&fixture.config, vec![Response::Json(200, sent(7))]).await;
        mock.service(&fixture, &database)
            .send_file(
                &session.session_key,
                fixture.file(b"source bytes"),
                "Caption",
            )
            .await
            .unwrap();
        let usage = database.lock().unwrap().history_usage().unwrap();
        assert_eq!(usage.records, u64::from(enabled));
        assert_eq!(usage.attachments, 0);
        assert!(
            database
                .lock()
                .unwrap()
                .route(-101, 7, 200_000)
                .unwrap()
                .is_some()
        );
    }
}

#[tokio::test]
async fn confirmed_snapshots_and_documents_survive_cancellation_rename_restart_and_expiry() {
    let mut fixture = Fixture::new();
    fixture.config.history.enabled = true;
    fixture.config.history.retention_days = 1;
    let database = fixture.open();
    let session = fixture.register(&database);
    let (release, held) = oneshot::channel();
    let mut mock = Mock::new(
        &fixture.config,
        vec![
            Response::Held(held, sent(7)),
            Response::Json(200, document(8)),
        ],
    )
    .await;
    {
        let service = mock.service(&fixture, &database);
        let sender = service.clone();
        let key = session.session_key.clone();
        let caller = tokio::spawn(async move { sender.send_text(&key, "Literal *🚀*").await });
        mock.seen.recv().await.unwrap();
        database
            .lock()
            .unwrap()
            .register_session(
                None,
                Some(&session.session_key),
                "Renamed",
                Some("module"),
                200_001,
            )
            .unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        service.close();
        let drain = service.drain();
        tokio::pin!(drain);
        assert!(futures_util::poll!(&mut drain).is_pending());
        release.send(()).unwrap();
        drain.await.unwrap();
    }
    let service = mock.service(&fixture, &database);
    service
        .send_file(
            &session.session_key,
            fixture.file(b"source bytes"),
            "Caption",
        )
        .await
        .unwrap();
    let rows: Vec<(String, String, Option<String>)> = fixture
        .sql()
        .prepare(
            "SELECT body, session_title, directory_label FROM journal_records ORDER BY message_id",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("界\nLiteral *🚀*".into(), "界".into(), None),
            (
                "Renamed\nmodule\nCaption".into(),
                "Renamed".into(),
                Some("module".into())
            )
        ]
    );
    let metadata: (String, String, String, i64) = fixture
        .sql()
        .query_row(
            "SELECT file_id, filename, mime_type, file_size FROM attachment_records",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        metadata,
        (
            "private-file".into(),
            "remote.txt".into(),
            "text/plain".into(),
            12
        )
    );
    let bytes = fs::read(&fixture.config.storage.path).unwrap();
    for excluded in [
        fixture.directory.to_str().unwrap(),
        "source bytes",
        "report.bin",
    ] {
        assert!(
            !bytes
                .windows(excluded.len())
                .any(|part| part == excluded.as_bytes())
        );
    }
    let duplicate = JournalInput {
        source: MessageSource {
            chat_id: -101,
            message_id: 7,
            sender_user_id: None,
        },
        direction: MessageDirection::Outgoing,
        body: Some("Not a replacement"),
        chat_label: None,
        sender_label: None,
        session: None,
        message_date: 100_000,
        reply_to_message_id: None,
    };
    let first = database
        .lock()
        .unwrap()
        .record_message(&duplicate, 200_001)
        .unwrap();
    let again = database
        .lock()
        .unwrap()
        .record_message(&duplicate, 200_002)
        .unwrap();
    assert_eq!(
        (first.message_handle, first.expires_at),
        (again.message_handle, again.expires_at)
    );
    assert_eq!(mock.count(), 2);
    drop(service);
    drop(database);
    let database = fixture.open();
    let mut db = database.lock().unwrap();
    assert_eq!(db.history_usage().unwrap().records, 2);
    let mut sweep = CleanupSweep::new(86_600_000).unwrap();
    while !db.cleanup_batch(&mut sweep).unwrap().complete {}
    assert_eq!(db.history_usage().unwrap().records, 0);
    assert_eq!(db.history_usage().unwrap().attachments, 0);
    assert!(db.route(-101, 7, 86_600_000).unwrap().is_some());
}

#[tokio::test]
async fn rejected_ambiguous_and_failed_bookkeeping_never_archive_or_replay_and_release_capacity() {
    for failure in 0..6 {
        let mut fixture = Fixture::new();
        fixture.config.history.enabled = true;
        fixture.config.history.max_records = 1;
        fixture.config.history.max_attachments = 1;
        let database = fixture.open();
        let session = fixture.register(&database);
        if matches!(failure, 2 | 3) {
            let table = if failure == 2 {
                "journal_records"
            } else {
                "attachment_records"
            };
            fixture
                .sql()
                .execute_batch(&format!(
                    "CREATE TRIGGER fail_capture BEFORE INSERT ON {table}
                BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;"
                ))
                .unwrap();
        }
        let response = match failure {
            0 => Response::Json(403, json!({"ok":false,"error_code":403})),
            1 => Response::Disconnect,
            4 | 5 => {
                let mut body = document(7);
                body["result"]["document"]["file_id"] =
                    if failure == 4 { json!("") } else { Value::Null };
                Response::Json(200, body)
            }
            _ => Response::Json(200, document(7)),
        };
        let mock = Mock::new(
            &fixture.config,
            vec![response, Response::Json(200, document(8))],
        )
        .await;
        let service = mock.service(&fixture, &database);
        let error = service
            .send_file(
                &session.session_key,
                fixture.file(b"source bytes"),
                "Caption",
            )
            .await
            .unwrap_err();
        let expected = match failure {
            0 => AppError::Telegram(TelegramError::Rejected {
                code: 403,
                retry_after_seconds: None,
            }),
            1 | 5 => AppError::Telegram(TelegramError::DeliveryUncertain),
            _ => AppError::SendBookkeepingFailed,
        };
        assert_eq!(error, expected);
        assert_eq!(mock.count(), 1);
        {
            let db = database.lock().unwrap();
            assert_eq!(db.history_usage().unwrap(), HistoryUsage::default());
            assert_eq!(db.history_coverage().unwrap().observed_from, None);
            assert_eq!(db.session(&session.session_key).unwrap(), session);
            assert!(db.route(-101, 7, 200_000).unwrap().is_none());
        }
        fixture
            .sql()
            .execute_batch("DROP TRIGGER IF EXISTS fail_capture")
            .unwrap();
        service
            .send_file(
                &session.session_key,
                fixture.file(b"source bytes"),
                "Intentional next send",
            )
            .await
            .unwrap();
        assert_eq!(mock.count(), 2);
        assert_eq!(
            database
                .lock()
                .unwrap()
                .history_usage()
                .unwrap()
                .attachments,
            1
        );
    }
}

#[tokio::test]
async fn outstanding_sends_reserve_each_history_budget_against_sends_and_ingress() {
    for budget in 0..4 {
        let mut fixture = Fixture::new();
        fixture.config.history.enabled = true;
        fixture.config.limits.background_tasks = 2;
        fixture.config.telegram.allowed_user_ids = vec![7];
        match budget {
            0 => fixture.config.history.max_records = 1,
            1 => {
                fixture.config.history.max_record_bytes = 1_024;
                fixture.config.history.max_body_bytes = 1_024;
            }
            2 => {
                fixture.config.history.max_record_bytes = 1_024;
                fixture.config.history.max_metadata_bytes = 1_024;
            }
            _ => fixture.config.history.max_attachments = 1,
        }
        let database = fixture.open();
        let session = fixture.register(&database);
        let (release, held) = oneshot::channel();
        let mut mock = Mock::new(
            &fixture.config,
            vec![Response::Held(
                held,
                if budget == 3 { document(7) } else { sent(7) },
            )],
        )
        .await;
        let service = mock.service(&fixture, &database);
        let sender = service.clone();
        let key = session.session_key.clone();
        let file = (budget == 3).then(|| fixture.file(b"source"));
        let caller = tokio::spawn(async move {
            if let Some(file) = file {
                sender.send_file(&key, file, "Body").await
            } else {
                sender.send_text(&key, &"x".repeat(600)).await
            }
        });
        mock.seen.recv().await.unwrap();
        let second = if budget == 3 {
            service
                .send_file(&session.session_key, fixture.file(b"second"), "Body")
                .await
        } else {
            service
                .send_text(&session.session_key, &"x".repeat(600))
                .await
        };
        assert_eq!(second.unwrap_err(), AppError::StorageCapacity);
        let mut update = json!({"update_id":1,"message":{"message_id":99,"date":1,
            "chat":{"id":-101},"from":{"id":7,"is_bot":false},"text":"x".repeat(600)}});
        if budget == 3 {
            update["message"]["document"] = document(99)["result"]["document"].clone();
        }
        {
            let mut db = database.lock().unwrap();
            assert_eq!(
                IngressService::new(&fixture.config).unwrap().ingest(
                    &mut db,
                    &serde_json::from_value(update).unwrap(),
                    200_000
                ),
                Err(AppError::StorageCapacity)
            );
            assert_eq!(db.update_cursor().unwrap(), None);
            assert_eq!(db.history_usage().unwrap(), HistoryUsage::default());
        }
        release.send(()).unwrap();
        caller.await.unwrap().unwrap();
        assert_eq!(mock.count(), 1);
        assert_eq!(database.lock().unwrap().history_usage().unwrap().records, 1);
    }
}
