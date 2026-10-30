use super::*;
use std::{fs, os::unix::fs::DirBuilderExt};

use crate::config::Limits;
use futures_util::poll;

struct Fixture(Storage);

impl Fixture {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "cleanup-scheduler-{:016x}",
            u64::from_ne_bytes(random)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        Self(Storage {
            path: directory.join("relay.sqlite"),
            ..Storage::default()
        })
    }

    fn service(&self) -> CleanupService {
        let db = Arc::new(Mutex::new(
            Database::open(&self.0, &Limits::default()).unwrap(),
        ));
        let mut service = CleanupService::new(db, &self.0).unwrap();
        service.now = || Ok(40 * 86_400_000);
        service
    }

    fn expired(&self, service: &CleanupService) -> String {
        service
            .database
            .lock()
            .unwrap()
            .register_session(None, None, "Same", 0)
            .unwrap()
            .session_key
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(self.0.path.parent().unwrap()).unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn startup_daily_schedule_exclusivity_and_shutdown_use_one_timer() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let first = fixture.expired(&service);
    let (shutdown, receive) = watch::channel(false);
    let mut running = Box::pin(service.run(receive.clone()));
    assert!(poll!(running.as_mut()).is_pending());
    assert_eq!(
        service.database.lock().unwrap().session(&first),
        Err(AppError::SessionNotFound)
    );
    assert_eq!(
        service.run(receive.clone()).await,
        Err(AppError::CleanupSchedulerActive)
    );
    let second = fixture.expired(&service);
    tokio::time::advance(Duration::from_secs(23 * 3_600)).await;
    assert!(poll!(running.as_mut()).is_pending());
    assert!(service.database.lock().unwrap().session(&second).is_ok());
    tokio::time::advance(Duration::from_secs(3_600)).await;
    assert!(poll!(running.as_mut()).is_pending());
    assert_eq!(
        service.database.lock().unwrap().session(&second),
        Err(AppError::SessionNotFound)
    );
    shutdown.send(true).unwrap();
    running.await.unwrap();
    // Already-cancelled startup performs no cleanup and releases admission immediately.
    let third = fixture.expired(&service);
    service.run(receive).await.unwrap();
    assert!(service.database.lock().unwrap().session(&third).is_ok());
}

#[tokio::test(start_paused = true)]
async fn cancellation_between_batches_preserves_progress_and_releases_admission() {
    let fixture = Fixture::new();
    let service = fixture.service();
    for _ in 0..105 {
        fixture.expired(&service);
    }
    let (shutdown, receive) = watch::channel(false);
    let mut running = Box::pin(service.run(receive.clone()));
    assert!(poll!(running.as_mut()).is_pending());
    let count = || {
        rusqlite::Connection::open(&fixture.0.path)
            .unwrap()
            .query_row("SELECT count(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(count(), 5);
    drop(running);
    let mut resumed = Box::pin(service.run(receive));
    assert!(poll!(resumed.as_mut()).is_pending());
    assert_eq!(count(), 0);
    shutdown.send(true).unwrap();
    resumed.await.unwrap();
    let invalid = Storage {
        cleanup_interval_hours: 0,
        ..Storage::default()
    };
    assert!(matches!(
        CleanupService::new(service.database.clone(), &invalid),
        Err(AppError::InvalidConfig(_))
    ));
}
