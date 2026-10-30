//! Validate before effects and finish admitted sends even if the caller disappears.

use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::Semaphore;

use crate::{
    clients::telegram::{SentMessage, TelegramClient},
    config::Config,
    domain::outbound::format_text,
    error::{AppError, TelegramError},
    repositories::{Database, RouteKind, RouteReservation},
};

/// Clones share send admission and the application's single database/client.
/// Caller cancellation detaches admitted work; process shutdown must drain it separately.
#[derive(Clone)]
pub struct OutboundService {
    database: Arc<Mutex<Database>>,
    telegram: TelegramClient,
    chat_id: i64,
    admission: Arc<Semaphore>,
    now: fn() -> Result<i64, AppError>,
}

impl OutboundService {
    /// Use the one configured, allowlisted destination; callers cannot override it.
    pub fn new(
        database: Arc<Mutex<Database>>,
        telegram: TelegramClient,
        config: &Config,
    ) -> Result<Self, AppError> {
        if config.telegram.chat_id == 0
            || config.telegram.chat_id.unsigned_abs() > (1 << 52) - 1
            || !config
                .telegram
                .allowed_chat_ids
                .contains(&config.telegram.chat_id)
        {
            return Err(AppError::InvalidConfig("telegram.chat_id"));
        }
        if !(1..=1_024).contains(&config.limits.background_tasks) {
            return Err(AppError::InvalidConfig("limits.background_tasks"));
        }
        Ok(Self {
            database,
            telegram,
            chat_id: config.telegram.chat_id,
            admission: Arc::new(Semaphore::new(config.limits.background_tasks as usize)),
            now: unix_millis,
        })
    }

    /// Validate the session and complete plain text before reserving capacity or sending.
    /// Success means Telegram accepted the message AND its reply route committed.
    pub async fn send_text(&self, session_key: &str, body: &str) -> Result<SentMessage, AppError> {
        let (text, reservation, permit) = {
            let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
            let session = database.session(session_key)?;
            let text = format_text(&session.title, body)?;
            let permit = Arc::clone(&self.admission)
                .try_acquire_owned()
                .map_err(|_| AppError::SendCapacity)?;
            let reservation = database.reserve_route(session_key)?;
            (text, reservation, permit)
        };
        // No await separates admission from transfer to the owned task. Dropping the join
        // handle cannot abort the send or the synchronous post-send transaction.
        let service = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = service.send_reserved(text, reservation).await;
            if let Err(error) = &result {
                // Detached callers must not hide uncertainty; log only a safe static code.
                tracing::warn!(code = error.code(), "Admitted Telegram send failed");
            }
            result
        })
        .await
        .map_err(|_| AppError::Telegram(TelegramError::DeliveryUncertain))?
    }

    async fn send_reserved(
        &self,
        text: String,
        reservation: RouteReservation,
    ) -> Result<SentMessage, AppError> {
        let sent = self.telegram.send_message(self.chat_id, &text).await?;
        self.record_success(reservation, &sent, RouteKind::Text)?;
        Ok(sent)
    }

    fn record_success(
        &self,
        reservation: RouteReservation,
        sent: &SentMessage,
        kind: RouteKind,
    ) -> Result<(), AppError> {
        // Bot API dates are seconds; every repository timestamp is milliseconds.
        let created_at = sent
            .date
            .checked_mul(1_000)
            .ok_or(AppError::SendBookkeepingFailed)?;
        let now = (self.now)().map_err(|_| AppError::SendBookkeepingFailed)?;
        self.database
            .lock()
            .map_err(|_| AppError::SendBookkeepingFailed)?
            .persist_route(
                reservation,
                self.chat_id,
                sent.message_id,
                kind,
                created_at,
                now,
            )
            .map_err(|_| AppError::SendBookkeepingFailed)
    }
}

fn unix_millis() -> Result<i64, AppError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or(AppError::InvalidRequest("timestamp"))
}

#[cfg(test)]
mod tests;
