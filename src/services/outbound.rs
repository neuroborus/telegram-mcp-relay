//! Validate before effects and finish admitted sends even if the caller disappears.

use std::sync::{Arc, Mutex};

use tokio::sync::{Semaphore, watch};
use tokio_util::task::TaskTracker;

use crate::{
    clients::telegram::{SentMessage, TelegramClient},
    config::Config,
    domain::outbound::{FormattedMessage, format_caption, format_text},
    error::{AppError, TelegramError},
    repositories::{
        AttachmentInput, AttachmentKind, ConfirmedMessage, Database, MessageSource,
        OutgoingJournal, RouteKind, RouteReservation,
    },
};

use super::{files::ValidatedFile, typing::TypingService, unix_millis};

enum Delivery {
    Text(FormattedMessage),
    Document(FormattedMessage, ValidatedFile),
}

/// Clones share send admission and the application's single database/client.
/// Caller cancellation detaches admitted work; shutdown closes admission and drains it.
#[derive(Clone)]
pub struct OutboundService {
    database: Arc<Mutex<Database>>,
    telegram: TelegramClient,
    chat_id: i64,
    admission: Arc<Semaphore>,
    tasks: TaskTracker,
    failed: watch::Sender<bool>,
    now: fn() -> Result<i64, AppError>,
    typing: TypingService,
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
            typing: TypingService::new(telegram.clone(), config)?,
            database,
            telegram,
            chat_id: config.telegram.chat_id,
            admission: Arc::new(Semaphore::new(config.limits.background_tasks as usize)),
            tasks: TaskTracker::new(),
            failed: watch::channel(false).0,
            now: unix_millis,
        })
    }

    /// Use this scheduler for confirmed inbox processing and all clones of this sender.
    pub fn typing(&self) -> TypingService {
        self.typing.clone()
    }

    pub(crate) fn chat_id(&self) -> i64 {
        self.chat_id
    }

    pub(crate) fn close(&self) {
        self.admission.close();
        self.tasks.close();
    }

    /// TaskTracker releases finished task storage immediately, including detached results.
    /// Existing text/upload deadlines bound network work; bookkeeping is never aborted.
    pub(crate) async fn drain(&self) -> Result<(), AppError> {
        self.tasks.wait().await;
        if *self.failed.borrow() {
            return Err(AppError::ServiceStopped);
        }
        Ok(())
    }

    pub(crate) async fn failed(&self) {
        let mut failed = self.failed.subscribe();
        while !*failed.borrow_and_update() {
            if failed.changed().await.is_err() {
                return;
            }
        }
    }

    /// Validate the session and complete plain text before reserving capacity or sending.
    /// Success means Telegram accepted the message AND its reply route committed.
    pub async fn send_text(&self, session_key: &str, body: &str) -> Result<SentMessage, AppError> {
        self.send(session_key, body, None).await
    }

    /// Consume the original confined handle; no caller path is reopened or persisted.
    /// Both caption limits include the session title and are checked before any upload.
    pub async fn send_file(
        &self,
        session_key: &str,
        file: ValidatedFile,
        caption: &str,
    ) -> Result<SentMessage, AppError> {
        self.send(session_key, caption, Some(file)).await
    }

    async fn send(
        &self,
        session_key: &str,
        body: &str,
        file: Option<ValidatedFile>,
    ) -> Result<SentMessage, AppError> {
        // Track before admission: close/wait cannot miss a task that already has a permit,
        // even when another thread closes admission before the task is spawned.
        let task = self.tasks.token();
        let (delivery, reservation, journal, permit) = {
            let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
            let session = database.session(session_key)?;
            let delivery = if let Some(file) = file {
                Delivery::Document(
                    format_caption(&session.title, session.directory_label.as_ref(), body)?,
                    file,
                )
            } else {
                Delivery::Text(format_text(
                    &session.title,
                    session.directory_label.as_ref(),
                    body,
                )?)
            };
            let permit = Arc::clone(&self.admission)
                .try_acquire_owned()
                .map_err(|_| AppError::SendCapacity)?;
            let reservation = database.reserve_route(session_key)?;
            let (text, document) = match &delivery {
                Delivery::Text(text) => (text, false),
                Delivery::Document(text, _) => (text, true),
            };
            let journal =
                database.reserve_outgoing(text.as_str(), Some(&session), document, None)?;
            (delivery, reservation, journal, permit)
        };
        // No await separates admission from transfer to the owned task. Dropping the join
        // handle cannot abort the send or the synchronous post-send transaction.
        let service = self.clone();
        let session_key = session_key.to_owned();
        let outcome = SendOutcome(Some(self.failed.clone()));
        let handle = self.tasks.spawn(async move {
            let _permit = permit;
            let result = service
                .send_reserved(delivery, reservation, journal, &session_key)
                .await;
            if let Err(error) = &result {
                // Detached callers must not hide uncertainty; log only a safe static code.
                tracing::warn!(code = error.code(), "Admitted Telegram send failed");
            }
            outcome.complete();
            result
        });
        // The tracked future now owns retirement, including destruction of its captures.
        drop(task);
        handle
            .await
            .map_err(|_| AppError::Telegram(TelegramError::DeliveryUncertain))?
    }

    async fn send_reserved(
        &self,
        delivery: Delivery,
        reservation: RouteReservation,
        journal: Option<OutgoingJournal>,
        session_key: &str,
    ) -> Result<SentMessage, AppError> {
        let (sent, kind) = match delivery {
            Delivery::Text(text) => (
                self.telegram.send_message(self.chat_id, &text).await?,
                RouteKind::Text,
            ),
            Delivery::Document(caption, file) => {
                let (file, filename, size) = file.into_parts();
                (
                    self.telegram
                        .send_document(self.chat_id, caption, file, filename, size)
                        .await?,
                    RouteKind::Document,
                )
            }
        };
        // A known accepted result clears this session even if route persistence fails.
        // Rejected or ambiguous sends cannot establish that the agent sent its result.
        self.typing.stop_session(session_key);
        self.record_success(reservation, &sent, kind, journal)?;
        Ok(sent)
    }

    fn record_success(
        &self,
        reservation: RouteReservation,
        sent: &SentMessage,
        kind: RouteKind,
        journal: Option<OutgoingJournal>,
    ) -> Result<(), AppError> {
        let confirmed = confirmed_message(sent)?;
        let now = (self.now)().map_err(|_| AppError::SendBookkeepingFailed)?;
        self.database
            .lock()
            .map_err(|_| AppError::SendBookkeepingFailed)?
            .persist_outgoing_route(reservation, &confirmed, kind, journal, now)
            .map_err(|_| AppError::SendBookkeepingFailed)
    }
}

pub(super) fn confirmed_message(sent: &SentMessage) -> Result<ConfirmedMessage<'_>, AppError> {
    let source = MessageSource {
        chat_id: sent.chat.id,
        message_id: sent.message_id,
        sender_user_id: None,
    };
    Ok(ConfirmedMessage {
        source,
        // Bot API dates are seconds; repository timestamps are milliseconds.
        message_date: sent
            .date
            .checked_mul(1_000)
            .ok_or(AppError::SendBookkeepingFailed)?,
        document: sent.document.as_ref().map(|file| AttachmentInput {
            source,
            reply_id: None,
            message_handle: None,
            file_id: &file.file_id,
            file_unique_id: &file.file_unique_id,
            filename: file.file_name.as_deref(),
            mime_type: file.mime_type.as_deref(),
            file_size: file.file_size,
            kind: AttachmentKind::Document,
            width: None,
            height: None,
        }),
    })
}

// Unwinding an admitted task must reach supervision even if its caller disconnected.
struct SendOutcome(Option<watch::Sender<bool>>);

impl SendOutcome {
    fn complete(mut self) {
        self.0 = None;
    }
}

impl Drop for SendOutcome {
    fn drop(&mut self) {
        if let Some(failed) = &self.0 {
            failed.send_replace(true);
        }
    }
}

#[cfg(test)]
mod tests;
