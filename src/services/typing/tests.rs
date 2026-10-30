use super::*;

fn service(config: &Config) -> TypingService {
    let client =
        TelegramClient::for_test(config, "http://127.0.0.1:1/botSynthetic/".into()).unwrap();
    let service = TypingService::new(client, config).unwrap();
    // Drive the same scheduler decisions directly with a controlled clock, without HTTP.
    service.state.lock().unwrap().running = true;
    service
}

fn config() -> Config {
    let mut config = Config::default();
    config.telegram.allowed_chat_ids = vec![-101, -102];
    config
}

fn start(service: &TypingService, session: &str, reply: &str, chat: i64, first: i64, now: i64) {
    service.start(
        session,
        reply,
        &ProcessingStatus {
            telegram_chat_id: chat,
            processing_started_at: Some(first),
        },
        now,
    );
}

#[tokio::test(start_paused = true)]
async fn merge_chats_stop_independently_and_keep_cooldowns_through_idle() {
    let service = service(&config());
    let now = Instant::now();
    start(&service, "a", "1", -101, 0, 0);
    start(&service, "b", "2", -101, 0, 0);
    assert_eq!(sent(&service, now), vec![-101]);
    assert!(
        matches!(service.state.lock().unwrap().work(now, service.refresh),
        Work::Active {wake, requests} if wake == now + service.refresh && requests.is_empty())
    );
    service.stop("1");
    assert_eq!(sent(&service, now + service.refresh), vec![-101]);
    service.stop_session("b");
    assert!(matches!(
        service
            .state
            .lock()
            .unwrap()
            .work(now + service.refresh, service.refresh),
        Work::Idle
    ));
    assert!(!service.state.lock().unwrap().running);
    // Re-enable manual driving after observing idle; a quick stop/start cannot flood a chat.
    service.state.lock().unwrap().running = true;
    start(&service, "c", "3", -101, 0, 0);
    start(&service, "d", "4", -102, 0, 0);
    assert_eq!(sent(&service, now + service.refresh), vec![-102]);
    assert!(sent(&service, now + service.refresh).is_empty());
}

#[tokio::test(start_paused = true)]
async fn original_start_caps_repeated_starts_and_resume_without_periodic_idle_work() {
    let service = service(&config());
    let now = Instant::now();
    start(&service, "a", "1", -101, 1_000, 1_000);
    tokio::time::advance(Duration::from_secs(599)).await;
    start(&service, "a", "1", -101, 1_000, 600_000);
    assert_eq!(
        service.state.lock().unwrap().replies["1"].deadline,
        now + Duration::from_secs(600)
    );
    // A backwards wall clock cannot lengthen an existing lease either.
    start(&service, "a", "1", -101, 1_000, 1_000);
    service.stop("1"); // Reconciliation clears old RAM ownership, then active resumes it.
    start(&service, "a", "1", -101, 1_000, 600_000);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(
        service
            .state
            .lock()
            .unwrap()
            .work(Instant::now(), service.refresh),
        Work::Idle
    ));
    start(&service, "a", "1", -101, 1_000, 601_000);
    assert!(!service.state.lock().unwrap().replies["1"].active);
    tokio::time::advance(Duration::from_secs(86_400)).await;
    assert!(matches!(
        service
            .state
            .lock()
            .unwrap()
            .work(Instant::now(), service.refresh),
        Work::Idle
    ));
}

#[tokio::test(start_paused = true)]
async fn disabled_unconfirmed_unapproved_and_bounded_activity_remain_silent() {
    let mut config = config();
    config.activity.typing_enabled = false;
    let disabled = service(&config);
    start(&disabled, "a", "1", -101, 0, 0);
    assert!(disabled.state.lock().unwrap().replies.is_empty());
    config.activity.typing_enabled = true;
    config.limits.inbox_items = 1;
    let service = service(&config);
    service.start(
        "a",
        "1",
        &ProcessingStatus {
            telegram_chat_id: -101,
            processing_started_at: None,
        },
        0,
    );
    start(&service, "a", "1", -103, 0, 0);
    assert!(service.state.lock().unwrap().replies.is_empty());
    start(&service, "a", "1", -101, 0, 0);
    start(&service, "b", "2", -101, 0, 0);
    assert_eq!(service.state.lock().unwrap().replies.len(), 1);
    let restarted = super::TypingService::new(service.telegram.clone(), &config).unwrap();
    assert!(restarted.state.lock().unwrap().replies.is_empty());
    assert!(!restarted.state.lock().unwrap().running);
}

#[tokio::test(start_paused = true)]
async fn stop_wakes_the_driver_immediately_and_idle_driver_retires() {
    let service = service(&config());
    start(&service, "a", "1", -101, 0, 0);
    let driver = run(
        Arc::downgrade(&service.state),
        service.changed.subscribe(),
        |_| std::future::pending(),
        service.refresh,
    );
    tokio::pin!(driver);
    assert!(futures_util::poll!(driver.as_mut()).is_pending());
    service.stop("1");
    // No advance to the HTTP timeout or refresh boundary is needed to cancel and retire.
    assert!(futures_util::poll!(driver.as_mut()).is_ready());
    assert!(!service.state.lock().unwrap().running);
}

// Model completed requests when checking scheduling decisions without running the driver.
fn sent(service: &TypingService, now: Instant) -> Vec<i64> {
    let mut state = service.state.lock().unwrap();
    let Work::Active { requests, .. } = state.work(now, service.refresh) else {
        return vec![];
    };
    requests
        .into_iter()
        .map(|(chat, _)| {
            state.in_flight.remove(&chat);
            chat
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn stalled_chat_does_not_delay_a_healthy_chats_initial_action_or_refreshes() {
    let service = service(&config());
    start(&service, "a", "1", -101, 0, 0);
    start(&service, "b", "2", -102, 0, 0);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let driver = run(
        Arc::downgrade(&service.state),
        service.changed.subscribe(),
        move |chat| {
            let recorded = recorded.clone();
            async move {
                recorded.lock().unwrap().push((chat, Instant::now()));
                if chat == -101 {
                    std::future::pending::<()>().await;
                }
                Ok(())
            }
        },
        service.refresh,
    );
    tokio::pin!(driver);
    let first = Instant::now();
    for tick in 0..3 {
        if tick > 0 {
            tokio::time::advance(service.refresh).await;
        }
        assert!(futures_util::poll!(driver.as_mut()).is_pending());
        for chat in [-101, -102] {
            let calls = calls.lock().unwrap();
            let calls: Vec<_> = calls.iter().filter(|(id, _)| *id == chat).collect();
            assert_eq!(calls.len(), tick + 1);
            assert_eq!(calls[tick].1, first + service.refresh * tick as u32);
        }
        assert!(service.state.lock().unwrap().in_flight.len() <= 2);
    }
    service.stop("1");
    service.stop("2");
    assert!(futures_util::poll!(driver.as_mut()).is_ready());
}

#[tokio::test(start_paused = true)]
async fn clock_rollback_cannot_reset_stopped_or_expired_caps_and_caps_stay_bounded() {
    let mut config = config();
    config.limits.inbox_items = 1;
    let service = service(&config);
    let first = Instant::now();
    start(&service, "a", "1", -101, 1_000, 1_000);
    tokio::time::advance(Duration::from_secs(599)).await;
    for stop_session in [false, true] {
        if stop_session {
            service.stop_session("a");
        } else {
            service.stop("1");
        }
        start(&service, "a", "1", -101, 1_000, 1_000); // Wall clock moved backwards.
        assert_eq!(
            service.state.lock().unwrap().replies["1"].deadline,
            first + Duration::from_secs(600)
        );
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(sent(&service, Instant::now()).is_empty());
    start(&service, "a", "1", -101, 1_000, 1_000); // Even after expiry, not another 600 seconds.
    assert!(!service.state.lock().unwrap().replies["1"].active);
    start(&service, "b", "2", -102, 1_000, 1_000);
    assert_eq!(service.state.lock().unwrap().replies.len(), 1); // No cap eviction under pressure.
    service.complete("1");
    service.state.lock().unwrap().running = true; // Continue manual driver decisions.
    start(&service, "b", "2", -102, 1_000, 1_000);
    assert_eq!(sent(&service, Instant::now()), vec![-102]);
}
