//! One bounded dispatcher; durable attempt decisions precede all network effects.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::watch;

use crate::{
    clients::telegram::TelegramClient,
    config::Config,
    domain::outbound::{UNCONFIRMED_NOTICE, UNROUTABLE_NOTICE, format_notice},
    error::{AppError, TelegramError},
    repositories::{Database, NoticeKind, NoticePolicy},
};

use super::unix_millis;

pub struct NoticeService {
    database: Arc<Mutex<Database>>,
    telegram: TelegramClient,
    policy: NoticePolicy,
    chats: Vec<i64>,
    now: fn() -> Result<i64, AppError>,
}

impl NoticeService {
    pub fn new(
        database: Arc<Mutex<Database>>,
        telegram: TelegramClient,
        config: &Config,
    ) -> Result<Self, AppError> {
        if !(1..=256).contains(&config.telegram.allowed_chat_ids.len())
            || config
                .telegram
                .allowed_chat_ids
                .iter()
                .any(|id| *id == 0 || id.unsigned_abs() > (1 << 52) - 1)
        {
            return Err(AppError::InvalidConfig("Telegram notice allowlist"));
        }
        Ok(Self {
            database,
            telegram,
            policy: NoticePolicy::new(&config.delivery, &config.notifications)?,
            chats: config.telegram.allowed_chat_ids.clone(),
            now: unix_millis,
        })
    }

    /// One scheduler per shared database, one due attempt per second, one HTTP request
    /// at a time. Cancellation consumes any admitted attempt, including unknown delivery.
    /// Startup scans durable eligibility; reconnects have no independent notice trigger.
    pub async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), AppError> {
        let admission = self
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .notice_admission
            .clone();
        let _permit = admission
            .try_acquire_owned()
            .map_err(|_| AppError::NoticeSchedulerActive)?;
        tokio::select! {
            biased;
            _ = async {
                loop {
                    if *shutdown.borrow_and_update() || shutdown.changed().await.is_err() {
                        break;
                    }
                }
            } => Ok(()),
            result = self.consume() => result,
        }
    }

    async fn consume(&self) -> Result<(), AppError> {
        let mut ticks = tokio::time::interval(Duration::from_secs(1));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            if let Err(error) = self.dispatch_one((self.now)()?).await {
                tracing::debug!(code = error.code(), "Telegram notice attempt not completed");
                if let AppError::Telegram(TelegramError::Rejected {
                    retry_after_seconds: Some(mut seconds),
                    ..
                }) = error
                {
                    // Delay other notices too, without retrying this consumed attempt.
                    while seconds > 0 {
                        let part = seconds.min(30);
                        tokio::time::sleep(Duration::from_secs(part)).await;
                        seconds -= part;
                    }
                }
            }
        }
    }

    async fn dispatch_one(&self, now: i64) -> Result<(), AppError> {
        let (attempt, text) = {
            let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
            let Some(attempt) = database.take_notice_attempts(&self.policy, 1, now)?.pop() else {
                return Ok(());
            };
            if !self.chats.contains(&attempt.telegram_chat_id) {
                return Ok(()); // An old authorization never overrides the current allowlist.
            }
            let text = match &attempt.kind {
                NoticeKind::Unconfirmed { session_key } => {
                    let session = database.session(session_key)?;
                    format_notice(Some(&session.title), UNCONFIRMED_NOTICE)?
                }
                NoticeKind::Unroutable => format_notice(None, UNROUTABLE_NOTICE)?,
            };
            (attempt, text)
        };
        let sent = self
            .telegram
            .send_message(attempt.telegram_chat_id, &text)
            .await?;
        self.database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .record_notice_message(&attempt, sent.message_id)
    }
}

#[cfg(test)]
mod tests;
