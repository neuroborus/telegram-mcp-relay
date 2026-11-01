use super::*;
use crate::app::probe::run_until;

#[tokio::test]
async fn probe_runs_shared_tools_and_drains_an_accepted_send() {
    let mut fixture = Fixture::new().await;
    let mut mock = Mock::new(&fixture.config).await;
    let address = fixture.config.server.listen;
    let listener = fixture.listener.take().unwrap();
    let (ready, startup) = oneshot::channel();
    let stop = CancellationToken::new();
    let running = run_until(
        &fixture.config,
        mock.address,
        listener,
        ready,
        stop.cancelled(),
    );
    tokio::pin!(running);
    let exercise = async {
        startup.await.unwrap();
        mock.next("getWebhookInfo")
            .await
            .respond(json!({"ok":true,"result":{"url":"","pending_update_count":0}}));
        let poll = mock.next("getUpdates").await;
        let session = tool(
            address,
            "telegram_register",
            json!({"title":"Resource probe"}),
        )
        .await;
        let key = session["session_key"].as_str().unwrap().to_owned();
        let send_key = key.clone();
        let caller = tokio::spawn(async move {
            tool(
                address,
                "telegram_send",
                json!({"session_key":send_key,"text":"Synthetic"}),
            )
            .await
        });
        let send = mock.next("sendMessage").await;
        assert_eq!(send.payload["text"], "[Resource probe]\n\nSynthetic");
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        stop.cancel();
        drop(poll);
        // Request accepted by the actual adapter remains admitted after caller cancellation.
        let now = unix_millis().unwrap();
        send.respond(json!({"ok":true,"result":{
            "message_id":71,"date":now/1000,"chat":{"id":-101}}}));
        (key, now)
    };
    let (result, (key, now)) = tokio::join!(&mut running, exercise);
    result.unwrap();
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
    assert_eq!(
        fixture
            .database()
            .route(-101, 71, now)
            .unwrap()
            .unwrap()
            .session_key,
        key
    );
}

#[tokio::test]
async fn probe_rejects_credentials_remote_peers_and_listener_mismatch_before_storage() {
    for case in 0..9 {
        let mut fixture = Fixture::new().await;
        let mut mock = Mock::new(&fixture.config).await;
        let mut mock_address = mock.address;
        match case {
            0 => fixture.config.telegram.bot_token_env = Some("SYNTHETIC_UNUSED".into()),
            1 => fixture.config.telegram.bot_token_file = Some(fixture.directory.join("absent")),
            2 => fixture.config.server.auth_token_env = Some("SYNTHETIC_UNUSED".into()),
            3 => fixture.config.server.auth_token_file = Some(fixture.directory.join("absent")),
            4 => fixture.config.server.create_auth_token_file = true,
            5 => mock_address = "192.0.2.1:9000".parse().unwrap(),
            6 => mock_address = "127.0.0.1:0".parse().unwrap(),
            7 => fixture.config.server.listen = "127.0.0.1:0".parse().unwrap(),
            8 => fixture.config.server.listen = "0.0.0.0:9000".parse().unwrap(),
            _ => unreachable!(),
        }
        let (ready, startup) = oneshot::channel();
        assert!(matches!(
            run_until(
                &fixture.config,
                mock_address,
                fixture.listener.take().unwrap(),
                ready,
                std::future::pending(),
            )
            .await,
            Err(AppError::InvalidConfig(_))
        ));
        assert!(startup.await.is_err());
        assert!(mock.requests.try_recv().is_err());
        assert!(fs::read_dir(&fixture.directory).unwrap().next().is_none());
    }
}

#[tokio::test]
async fn probe_reports_no_readiness_if_startup_cleanup_fails() {
    let mut fixture = Fixture::new().await;
    let mut mock = Mock::new(&fixture.config).await;
    let _database = fixture.database();
    let lock = rusqlite::Connection::open(&fixture.config.storage.path).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let (ready, startup) = oneshot::channel();
    assert_eq!(
        run_until(
            &fixture.config,
            mock.address,
            fixture.listener.take().unwrap(),
            ready,
            std::future::pending(),
        )
        .await,
        Err(AppError::StorageFailure)
    );
    assert!(startup.await.is_err());
    assert!(mock.requests.try_recv().is_err());
    assert!(
        tokio::net::TcpStream::connect(fixture.config.server.listen)
            .await
            .is_err()
    );
    lock.execute_batch("ROLLBACK").unwrap();
}
