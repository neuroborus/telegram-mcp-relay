//! One startup/periodic expiry and reclamation sweep, yielding between short transactions.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::{oneshot, watch};

use crate::{
    config::Storage,
    error::AppError,
    repositories::{CleanupBatch, CleanupSweep, Database},
};

use super::{typing::TypingService, unix_millis};

pub struct CleanupService {
    database: Arc<Mutex<Database>>,
    interval: Duration,
    now: fn() -> Result<i64, AppError>,
    typing: Option<TypingService>,
}

impl CleanupService {
    pub fn new(database: Arc<Mutex<Database>>, settings: &Storage) -> Result<Self, AppError> {
        if !(1..=168).contains(&settings.cleanup_interval_hours) {
            return Err(AppError::InvalidConfig("storage.cleanup_interval_hours"));
        }
        Ok(Self {
            database,
            interval: Duration::from_secs(settings.cleanup_interval_hours * 3_600),
            now: unix_millis,
            typing: None,
        })
    }

    /// Share the scheduler used by inbox processing and outbound delivery.
    pub fn with_typing(mut self, typing: TypingService) -> Self {
        self.typing = Some(typing);
        self
    }

    /// Release remembered typing caps only after durable expiry, under the database guard.
    /// A failed transaction preserves both the reply and its original monotonic cap.
    pub fn cleanup_batch(&self, sweep: &mut CleanupSweep) -> Result<CleanupBatch, AppError> {
        let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
        let batch = database.cleanup_batch(sweep)?;
        if let Some(typing) = &self.typing {
            for reply_id in &batch.expired_reply_ids {
                typing.complete(reply_id);
            }
        }
        Ok(batch)
    }

    /// Clean on startup, then sleep for the configured interval after each complete sweep.
    /// Cancellation can stop between transactions; the next run scans durable state anew.
    pub async fn run(&self, shutdown: watch::Receiver<bool>) -> Result<(), AppError> {
        self.run_ready(shutdown, None).await
    }

    /// Notify composition only after the complete startup sweep and reclamation succeed.
    pub(crate) async fn run_ready(
        &self,
        mut shutdown: watch::Receiver<bool>,
        ready: Option<oneshot::Sender<()>>,
    ) -> Result<(), AppError> {
        let admission = self
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .cleanup_admission
            .clone();
        let _permit = admission
            .try_acquire_owned()
            .map_err(|_| AppError::CleanupSchedulerActive)?;
        tokio::select! {
            biased;
            _ = async {
                loop {
                    if *shutdown.borrow_and_update() || shutdown.changed().await.is_err() {
                        break;
                    }
                }
            } => Ok(()),
            result = self.consume(ready) => result,
        }
    }

    async fn consume(&self, mut ready: Option<oneshot::Sender<()>>) -> Result<(), AppError> {
        loop {
            let mut sweep = CleanupSweep::new((self.now)()?)?;
            loop {
                let batch = self.cleanup_batch(&mut sweep)?;
                if batch.complete {
                    break;
                }
                tokio::task::yield_now().await;
            }
            // Snapshot a finite work budget. Concurrent churn cannot extend this sweep;
            // leftover free pages are reusable immediately and reconsidered next time.
            let mut remaining = self
                .database
                .lock()
                .map_err(|_| AppError::StorageFailure)?
                .disk_usage()?
                .free_pages;
            while remaining > 0 {
                tokio::task::yield_now().await;
                let usage = self
                    .database
                    .lock()
                    .map_err(|_| AppError::StorageFailure)?
                    .reclaim_batch()?;
                remaining = remaining.saturating_sub(crate::repositories::RECLAIM_PAGES);
                if usage.free_pages == 0 {
                    break;
                }
            }
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

#[cfg(test)]
mod tests;
