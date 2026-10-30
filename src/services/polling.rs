//! One cancellable consumer; unresolved input never permits a later confirming offset.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::watch;

use crate::{
    clients::telegram::TelegramClient,
    config::Config,
    error::{AppError, TelegramError},
    repositories::{Database, IngestOutcome},
};

use super::{ingress::IngressService, unix_millis};

const BATCH_SIZE: u8 = 100;

pub struct PollingService {
    database: Arc<Mutex<Database>>,
    telegram: TelegramClient,
    ingress: IngressService,
    inbox_changes: watch::Sender<u64>,
}

impl PollingService {
    pub fn new(
        database: Arc<Mutex<Database>>,
        telegram: TelegramClient,
        config: &Config,
    ) -> Result<Self, AppError> {
        Ok(Self {
            database,
            telegram,
            ingress: IngressService::new(config)?,
            inbox_changes: watch::channel(0).0,
        })
    }

    /// Coalesced post-commit wakeups, not a replay log. Read SQLite to recover state.
    pub fn inbox_changes(&self) -> watch::Receiver<u64> {
        self.inbox_changes.subscribe()
    }

    /// Run until shutdown is true, its sender disappears, or a terminal error occurs.
    /// Dropping this future cancels HTTP/backoff and releases the shared poll lease.
    /// Synchronous ingestion commits cannot be interrupted halfway by cancellation.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), AppError> {
        let admission = self
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .poll_admission
            .clone();
        let _permit = admission
            .try_acquire_owned()
            .map_err(|_| AppError::PollerActive)?;
        tokio::select! {
            biased;
            _ = stopped(&mut shutdown) => Ok(()),
            result = self.consume() => result,
        }
    }

    async fn consume(&self) -> Result<(), AppError> {
        let mut failures = 0;
        loop {
            match self.telegram.get_webhook_info().await {
                Ok(info) if info.url.is_empty() => break,
                Ok(_) => {
                    return Err(TelegramError::Rejected {
                        code: 409,
                        retry_after_seconds: None,
                    }
                    .into());
                }
                Err(error) => retry_network(error, &mut failures).await?,
            }
        }
        let changes = self
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .route_changes();
        failures = 0;
        loop {
            let offset = self
                .database
                .lock()
                .map_err(|_| AppError::StorageFailure)?
                .update_cursor()?
                .map(|id| id.checked_add(1).ok_or(TelegramError::InvalidResponse))
                .transpose()?;
            let updates = match self.telegram.get_updates(offset, BATCH_SIZE).await {
                Ok(updates) => updates,
                Err(error) => {
                    retry_network(error, &mut failures).await?;
                    continue;
                }
            };
            // Reject the entire malformed batch before any cursor mutation. IDs may
            // contain gaps or restart below an older sequence, but each batch is ordered.
            if updates
                .iter()
                .any(|u| u.update_id < 0 || u.update_id == i64::MAX)
                || updates.windows(2).any(|w| w[0].update_id >= w[1].update_id)
            {
                return Err(TelegramError::InvalidResponse.into());
            }
            failures = 0;
            let received_at = unix_millis()?;
            let pending = self
                .database
                .lock()
                .map_err(|_| AppError::StorageFailure)?
                .pending_routes();
            for update in updates {
                let mut pressure = 0;
                loop {
                    // Arm before checking under the admission/ingestion guard. A route
                    // may settle between the check and await without losing its wakeup.
                    let changed = changes.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    let result = self
                        .database
                        .lock()
                        .map_err(|_| AppError::StorageFailure)
                        .and_then(|mut db| {
                            self.ingress
                                .ingest_observed(&mut db, &update, received_at, &pending)
                        });
                    match result {
                        Ok(outcome) => {
                            if matches!(outcome, IngestOutcome::ReplyStored { .. }) {
                                self.inbox_changes
                                    .send_modify(|revision| *revision = revision.wrapping_add(1));
                            }
                            break;
                        }
                        Err(AppError::RoutePending) => changed.await,
                        Err(
                            AppError::StorageCapacity
                            | AppError::StorageFailure
                            | AppError::StorageUnavailable,
                        ) => {
                            // Retain this bounded batch and retry exactly this update.
                            // No new getUpdates can confirm it or subsequent input.
                            pause(backoff(&mut pressure, None)).await;
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }
}

async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    loop {
        let stopping = *shutdown.borrow_and_update();
        if stopping || shutdown.changed().await.is_err() {
            return;
        }
    }
}

async fn retry_network(error: AppError, failures: &mut u32) -> Result<(), AppError> {
    let retry_after = match &error {
        AppError::Telegram(TelegramError::Rejected {
            code: 429 | 500..=599,
            retry_after_seconds,
        }) => *retry_after_seconds,
        AppError::Telegram(
            TelegramError::Transport
            | TelegramError::Deadline
            | TelegramError::InvalidResponse
            | TelegramError::ResponseTooLarge,
        ) => None,
        // 409 conflicts and other permanent rejections require operator action.
        _ => return Err(error),
    };
    tracing::debug!(code = error.code(), "Telegram polling deferred");
    pause(backoff(failures, retry_after)).await;
    Ok(())
}

fn backoff(failures: &mut u32, retry_after: Option<u64>) -> Duration {
    let base_ms = 1_000_u64 << (*failures).min(4);
    *failures = failures.saturating_add(1);
    let mut random = [0; 4];
    let _ = getrandom::fill(&mut random);
    Duration::from_millis((base_ms + u64::from(u32::from_ne_bytes(random)) % base_ms).min(30_000))
        .max(Duration::from_secs(retry_after.unwrap_or(0)))
}

async fn pause(mut delay: Duration) {
    // Never shorten retry_after or overflow a timer for an untrusted huge value.
    while !delay.is_zero() {
        let part = delay.min(Duration::from_secs(30));
        tokio::time::sleep(part).await;
        delay -= part;
    }
}

#[cfg(test)]
mod tests;
