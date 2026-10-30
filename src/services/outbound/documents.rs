use super::*;

fn assert_unrouted(database: &Arc<Mutex<Database>>, session: &Session) {
    let database = database.lock().unwrap();
    assert_eq!(database.session(&session.session_key).unwrap(), *session);
    assert!(database.route(-101, 7, 200_000).unwrap().is_none());
}

#[tokio::test]
async fn captions_validate_both_final_limits_without_http_routes_or_activity() {
    let fixture = Fixture::new();
    let database = fixture.open();
    let session = fixture.register(&database);
    let mock = Mock::new(&fixture.config, vec![]).await;
    let service = mock.service(&fixture, &database);
    assert_eq!(
        service
            .send_file("unknown", fixture.file(b"private bytes"), "caption")
            .await
            .unwrap_err(),
        AppError::SessionNotFound
    );
    for (body, scalars, units) in [
        ("a".repeat(997), 1_001, 1_001),
        ("😀".repeat(510) + "a", 515, 1_025),
    ] {
        let error = service
            .send_file(&session.session_key, fixture.file(b"private bytes"), &body)
            .await
            .unwrap_err();
        let AppError::TextTooLong(details) = &error else {
            panic!("{error}")
        };
        assert_eq!(
            (details.actual_characters, details.actual_utf16_units),
            (scalars, units)
        );
        assert_eq!((details.limit, details.native_limit_utf16), (1_000, 1_024));
        assert!(!format!("{error:?} {error}").contains(&body));
    }
    assert_eq!(mock.count(), 0);
    assert_unrouted(&database, &session);
    assert_eq!(
        fixture
            .sql()
            .query_row("SELECT count(*) FROM message_routes", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(service.admission.available_permits(), 1);
    let mut database = database.lock().unwrap();
    let _first = database.reserve_route(&session.session_key).unwrap();
    let _second = database.reserve_route(&session.session_key).unwrap();
}

#[tokio::test]
async fn storage_pressure_prevents_upload_and_truncation_remains_uncertain_without_a_route() {
    let mut fixture = Fixture::new();
    fixture.config.limits.routes = 1;
    let database = fixture.open();
    let session = fixture.register(&database);
    let mock = Mock::new(&fixture.config, vec![]).await;
    let service = mock.service(&fixture, &database);
    let reservation = database
        .lock()
        .unwrap()
        .reserve_route(&session.session_key)
        .unwrap();
    assert_eq!(
        service
            .send_file(&session.session_key, fixture.file(b"report"), "")
            .await
            .unwrap_err(),
        AppError::StorageCapacity
    );
    drop(reservation);
    let file = fixture.file(b"report");
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.directory.join("report.bin"))
        .unwrap()
        .set_len(0)
        .unwrap();
    assert_eq!(
        service
            .send_file(&session.session_key, file, "")
            .await
            .unwrap_err(),
        AppError::Telegram(TelegramError::DeliveryUncertain)
    );
    assert_eq!(mock.count(), 0);
    assert_eq!(service.admission.available_permits(), 1);
    assert_unrouted(&database, &session);
    let _reservation = database
        .lock()
        .unwrap()
        .reserve_route(&session.session_key)
        .unwrap();
}

#[tokio::test]
async fn stalled_document_response_has_a_finite_deadline_without_replay() {
    let fixture = Fixture::new();
    let database = fixture.open();
    let session = fixture.register(&database);
    let (_release, blocked) = oneshot::channel();
    let mut mock = Mock::new(&fixture.config, vec![Response::Held(blocked, sent(7))]).await;
    let service = mock.service(&fixture, &database);
    let caller_service = service.clone();
    let key = session.session_key.clone();
    let file = fixture.file(b"report");
    let caller = tokio::spawn(async move { caller_service.send_file(&key, file, "").await });
    // Pause after real file I/O; an idle paused clock must not outrun blocking filesystem work.
    mock.seen.recv().await.unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(600)).await;
    assert_eq!(
        caller.await.unwrap().unwrap_err(),
        AppError::Telegram(TelegramError::DeliveryUncertain)
    );
    assert_eq!(mock.count(), 1);
    assert_eq!(service.admission.available_permits(), 1);
    assert_unrouted(&database, &session);
}

#[tokio::test]
async fn streams_original_handle_with_plain_caption_and_restart_visible_document_routes() {
    let fixture = Fixture::new();
    let database = fixture.open();
    let session = fixture.register(&database);
    let mock = Mock::new(
        &fixture.config,
        vec![Response::Json(200, sent(7)), Response::Json(200, sent(8))],
    )
    .await;
    let service = mock.service(&fixture, &database);
    let bytes = b"synthetic report\0\xff\n";
    let file = fixture.file(bytes);
    // Growth and pathname replacement after validation cannot add bytes or switch inodes.
    let path = fixture.directory.join("report.bin");
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"not admitted")
        .unwrap();
    fs::rename(&path, fixture.directory.join("original.bin")).unwrap();
    fs::write(&path, b"replacement").unwrap();
    let caption = "a".repeat(996);
    service
        .send_file(&session.session_key, file, &caption)
        .await
        .unwrap();
    service
        .send_file(&session.session_key, fixture.file(b""), &"😀".repeat(510))
        .await
        .unwrap();
    {
        let requests = mock.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for (request, description) in requests.iter().zip([caption, "😀".repeat(510)]) {
            assert_eq!(request.method, "/botSynthetic/sendDocument");
            assert_eq!(request.part("chat_id").1, b"-101");
            assert_eq!(
                request.part("caption").1,
                format!("[界]\n{description}").as_bytes()
            );
            let (headers, _) = request.part("document");
            assert!(
                std::str::from_utf8(headers)
                    .unwrap()
                    .contains("filename=\"report.bin\"")
            );
            assert!(!request.body.windows(10).any(|b| b == b"parse_mode"));
        }
        assert_eq!(requests[0].part("document").1, bytes);
        assert!(requests[1].part("document").1.is_empty());
    }
    drop(service);
    drop(database);
    let database = fixture.open();
    let database = database.lock().unwrap();
    for id in [7, 8] {
        let route = database.route(-101, id, 200_000).unwrap().unwrap();
        assert_eq!(
            (route.session_key, route.kind, route.created_at),
            (session.session_key.clone(), RouteKind::Document, 100_000)
        );
        assert!(database.route(-102, id, 200_000).unwrap().is_none());
    }
    assert_eq!(
        database
            .session(&session.session_key)
            .unwrap()
            .last_activity_at,
        200_000
    );
    let stored = fs::read(&fixture.config.storage.path).unwrap();
    assert!(!stored.windows(bytes.len()).any(|b| b == bytes));
    assert!(!stored.windows(100).any(|b| b == "a".repeat(100).as_bytes()));
}

#[tokio::test]
async fn multipart_upload_delivers_every_chunk_and_the_final_partial_chunk() {
    let fixture = Fixture::new();
    let database = fixture.open();
    let session = fixture.register(&database);
    let mock = Mock::new(&fixture.config, vec![Response::Json(200, sent(7))]).await;
    let service = mock.service(&fixture, &database);
    let bytes: Vec<u8> = (0..131_075).map(|index| (index % 251) as u8).collect();
    let file = fixture.file(&bytes);
    service
        .send_file(&session.session_key, file, "Multichunk report")
        .await
        .unwrap();
    let requests = mock.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].part("document").1, bytes);
    assert_eq!(
        requests[0].part("caption").1,
        "[界]\nMultichunk report".as_bytes()
    );
    let route = database
        .lock()
        .unwrap()
        .route(-101, 7, 200_000)
        .unwrap()
        .unwrap();
    assert_eq!(route.kind, RouteKind::Document);
    assert_eq!(route.session_key, session.session_key);
}

#[tokio::test]
async fn document_failures_never_replay_or_invent_routes_and_release_admission() {
    for (response, bookkeeping_failure, expected) in [
        (
            Response::Json(
                429,
                json!({"ok":false,"error_code":429,"parameters":{"retry_after":1}}),
            ),
            false,
            AppError::Telegram(TelegramError::Rejected {
                code: 429,
                retry_after_seconds: Some(1),
            }),
        ),
        (
            Response::Json(502, json!({"ok":false,"error_code":502})),
            false,
            AppError::Telegram(TelegramError::DeliveryUncertain),
        ),
        (
            Response::Json(
                200,
                json!({"ok":true,"result":{"message_id":7,"date":100,"chat":{"id":-102}}}),
            ),
            false,
            AppError::Telegram(TelegramError::DeliveryUncertain),
        ),
        (
            Response::Json(200, json!({"ok":true,"result":{}})),
            false,
            AppError::Telegram(TelegramError::DeliveryUncertain),
        ),
        (
            Response::Disconnect,
            false,
            AppError::Telegram(TelegramError::DeliveryUncertain),
        ),
        (
            Response::Json(200, sent(7)),
            true,
            AppError::SendBookkeepingFailed,
        ),
    ] {
        let fixture = Fixture::new();
        let database = fixture.open();
        let session = fixture.register(&database);
        if bookkeeping_failure {
            fixture.sql().execute_batch("CREATE TRIGGER fail_activity BEFORE UPDATE ON sessions BEGIN SELECT RAISE(ABORT, 'private failure'); END").unwrap();
        }
        let mock = Mock::new(&fixture.config, vec![response]).await;
        let service = mock.service(&fixture, &database);
        let error = service
            .send_file(
                &session.session_key,
                fixture.file(b"private bytes"),
                "private caption",
            )
            .await
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains("private"));
        assert_eq!(mock.count(), 1);
        assert_eq!(service.admission.available_permits(), 1);
        assert_unrouted(&database, &session);
        let mut database = database.lock().unwrap();
        let _first = database.reserve_route(&session.session_key).unwrap();
        let _second = database.reserve_route(&session.session_key).unwrap();
    }
}

#[tokio::test]
async fn canceled_document_caller_keeps_shared_capacity_until_bookkeeping_finishes() {
    let mut fixture = Fixture::new();
    fixture.config.limits.routes = 1;
    let database = fixture.open();
    let session = fixture.register(&database);
    let (release, blocked) = oneshot::channel();
    let mut mock = Mock::new(&fixture.config, vec![Response::Held(blocked, sent(7))]).await;
    let service = mock.service(&fixture, &database);
    let caller_service = service.clone();
    let key = session.session_key.clone();
    let file = fixture.file(b"synthetic report");
    let caller = tokio::spawn(async move { caller_service.send_file(&key, file, "").await });
    mock.seen.recv().await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(
        service
            .send_text(&session.session_key, "second")
            .await
            .unwrap_err(),
        AppError::SendCapacity
    );
    assert_eq!(
        service
            .send_file(&session.session_key, fixture.file(b"second"), "second")
            .await
            .unwrap_err(),
        AppError::SendCapacity
    );
    assert!(matches!(
        database.lock().unwrap().reserve_route(&session.session_key),
        Err(AppError::StorageCapacity)
    ));
    assert_eq!(
        mock.requests.lock().unwrap()[0].part("caption").1,
        "[界]".as_bytes()
    );
    release.send(()).unwrap();
    let idle = service.admission.acquire().await.unwrap();
    assert_eq!(mock.count(), 1);
    drop(idle);
    drop(service);
    drop(database);
    let database = fixture.open();
    let route = database
        .lock()
        .unwrap()
        .route(-101, 7, 200_000)
        .unwrap()
        .unwrap();
    assert_eq!(
        (route.session_key, route.kind),
        (session.session_key, RouteKind::Document)
    );
}
