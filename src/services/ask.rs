//! Shared bounded ask/after-send waits; durable replies outlive every wait.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::time::Instant;

use crate::{
    clients::telegram::SentMessage,
    config::Delivery,
    error::AppError,
    repositories::{ClaimedReply, Database, InboxNotifications, InboxSubscription, ReplyTarget},
};

use super::{inbox::InboxService, outbound::OutboundService, unix_millis};

pub struct AskService {
    database: Arc<Mutex<Database>>,
    outbound: OutboundService,
    inbox: InboxService,
    notifications: InboxNotifications,
    timeout: Option<Duration>,
}

pub struct AskedQuestion {
    pub sent: SentMessage,
    pub reply: Option<ClaimedReply>,
    /// False means waiting was disabled; true with no reply means the wait expired.
    pub waited: bool,
}

pub struct WaitedReply {
    pub target: ReplyTarget,
    pub reply: Option<ClaimedReply>,
    pub waited: bool,
}

impl AskService {
    pub fn new(
        database: Arc<Mutex<Database>>,
        outbound: OutboundService,
        settings: &Delivery,
    ) -> Result<Self, AppError> {
        if !(1..=86_400).contains(&settings.wait_timeout_seconds) {
            return Err(AppError::InvalidConfig("delivery.wait_timeout_seconds"));
        }
        let notifications = database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .inbox_notifications
            .clone();
        Ok(Self {
            database,
            outbound,
            inbox: InboxService::new(settings)?,
            notifications,
            timeout: settings
                .wait_fallback_enabled
                .then(|| Duration::from_secs(settings.wait_timeout_seconds)),
        })
    }

    /// Reserve and arm before sending, then recheck SQLite after route bookkeeping.
    /// Cancellation drops only this interest; accepted sends finish in OutboundService.
    pub async fn ask(&self, session_key: &str, text: &str) -> Result<AskedQuestion, AppError> {
        let subscription = if self.timeout.is_some() {
            let database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
            database.session(session_key)?;
            Some(self.notifications.waiter(session_key.to_owned())?)
        } else {
            None
        };
        let sent = self.outbound.send_text(session_key, text).await?;
        let reply = if let (Some(subscription), Some(timeout)) = (subscription, self.timeout) {
            self.wait(
                session_key,
                ReplyTarget {
                    chat_id: sent.chat.id,
                    message_id: sent.message_id,
                },
                subscription,
                timeout,
            )
            .await
            .map_err(|error| {
                tracing::warn!(code = error.code(), "Question wait failed after send");
                AppError::AskWaitFailed {
                    telegram_chat_id: sent.chat.id,
                    outbound_message_id: sent.message_id,
                }
            })?
        } else {
            None
        };
        Ok(AskedQuestion {
            sent,
            reply,
            waited: self.timeout.is_some(),
        })
    }

    /// Authorize and arm an existing text/document route without sending anything.
    /// Disabled waiting still validates the target, but never registers or claims.
    pub async fn wait_for_reply(
        &self,
        session_key: &str,
        outbound_message_id: i64,
    ) -> Result<WaitedReply, AppError> {
        let target = ReplyTarget {
            chat_id: self.outbound.chat_id(),
            message_id: outbound_message_id,
        };
        let subscription = {
            let database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
            database.session(session_key)?;
            if target.message_id <= 0
                || database
                    .route(target.chat_id, target.message_id, unix_millis()?)?
                    .is_none_or(|route| route.session_key != session_key)
            {
                return Err(AppError::InvalidWaitTarget);
            }
            // Serialize authorization/arming with ingress, then query durable state.
            self.timeout
                .map(|_| self.notifications.waiter(session_key.to_owned()))
                .transpose()?
        };
        let reply = if let (Some(subscription), Some(timeout)) = (subscription, self.timeout) {
            self.wait(session_key, target, subscription, timeout)
                .await?
        } else {
            None
        };
        Ok(WaitedReply {
            target,
            reply,
            waited: self.timeout.is_some(),
        })
    }

    async fn wait(
        &self,
        session_key: &str,
        target: ReplyTarget,
        mut subscription: InboxSubscription,
        timeout: Duration,
    ) -> Result<Option<ClaimedReply>, AppError> {
        let deadline = Instant::now() + timeout;
        let stopped = subscription.stopped.clone();
        loop {
            if stopped.is_cancelled() {
                return Err(AppError::McpUnavailable);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let claim = {
                let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
                self.inbox
                    .claim(&mut database, session_key, Some(target), 1, unix_millis()?)?
                    .pop()
            };
            if claim.is_some() {
                return Ok(claim);
            }
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => return Ok(None),
                _ = stopped.cancelled() => return Err(AppError::McpUnavailable),
                change = subscription.recv() => {
                    if change.is_none() { return Err(AppError::McpUnavailable); }
                }
            }
        }
    }
}
