//! Validate before effects and finish admitted sends even if the caller disappears.

use std::sync::{Arc, Mutex};

use tokio::sync::{Semaphore, watch};
use tokio_util::task::TaskTracker;

use crate::{
    clients::telegram::{SentMessage, TelegramClient},
    config::Config,
    domain::outbound::{
        FormattedMessage, MessageEntity, format_caption, format_text, validate_body_entities,
    },
    error::{AppError, TelegramError},
    repositories::{
        AttachmentInput, AttachmentKind, ConfirmedMessage, Database, MessageSource,
        OutgoingJournal, OutgoingKind, PreparedStartAck, ReplyTarget, RouteKind, RouteReservation,
        Session,
    },
};

use super::{files::ValidatedFile, typing::TypingService, unix_millis};

enum Delivery {
    Text(FormattedMessage),
    Document(FormattedMessage, ValidatedFile),
    StartAck {
        text: FormattedMessage,
        target: ReplyTarget,
        reply_id: String,
    },
}

pub(super) struct ConfirmedSend {
    pub message: SentMessage,
    pub bookkeeping: Result<(), AppError>,
}

/// Clones share send admission and the application's single database/client.
/// Caller cancellation detaches admitted work; shutdown closes admission and drains it.
#[derive(Clone)]
pub struct OutboundService {
    database: Arc<Mutex<Database>>,
    telegram: TelegramClient,
    chat_id: i64,
    allowed_chats: Arc<[i64]>,
    admission: Arc<Semaphore>,
    tasks: TaskTracker,
    failed: watch::Sender<bool>,
    now: fn() -> Result<i64, AppError>,
    typing: TypingService,
}

impl OutboundService {
    /// Use the configured chat for outbound text and file tools. A start acknowledgment
    /// goes to the owned reply's allowlisted chat.
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
            allowed_chats: config.telegram.allowed_chat_ids.clone().into(),
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
        self.send_text_with_entities(session_key, body, &[]).await
    }

    /// Body-relative native formatting; validation precedes all send/storage effects.
    pub async fn send_text_with_entities(
        &self,
        session_key: &str,
        body: &str,
        entities: &[MessageEntity],
    ) -> Result<SentMessage, AppError> {
        self.send(session_key, body, entities, None).await
    }

    /// Check the complete caption before opening a caller-supplied path.
    /// Delivery validates again against the current session metadata.
    pub(crate) fn validate_caption(
        &self,
        session_key: &str,
        caption: &str,
        entities: &[MessageEntity],
    ) -> Result<(), AppError> {
        validate_body_entities(caption, entities)?;
        let session = self
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .session(session_key)?;
        format_caption(
            &session.title,
            Some(session.outbound_directory_label()?),
            caption,
        )?;
        Ok(())
    }

    /// Consume the original confined handle; no caller path is reopened or persisted.
    /// Both caption limits include the session title and are checked before any upload.
    pub async fn send_file(
        &self,
        session_key: &str,
        file: ValidatedFile,
        caption: &str,
    ) -> Result<SentMessage, AppError> {
        self.send_file_with_entities(session_key, file, caption, &[])
            .await
    }

    /// Caption-relative native formatting; the confined original file remains streamed.
    pub async fn send_file_with_entities(
        &self,
        session_key: &str,
        file: ValidatedFile,
        caption: &str,
        entities: &[MessageEntity],
    ) -> Result<SentMessage, AppError> {
        self.send(session_key, caption, entities, Some(file)).await
    }

    async fn send(
        &self,
        session_key: &str,
        body: &str,
        entities: &[MessageEntity],
        file: Option<ValidatedFile>,
    ) -> Result<SentMessage, AppError> {
        let confirmed = self
            .send_prepared(|database| {
                let session = database.session(session_key)?;
                let directory = Some(session.outbound_directory_label()?);
                let delivery = if let Some(file) = file {
                    Delivery::Document(
                        format_caption(&session.title, directory, body)?
                            .with_body_entities(body, entities)?,
                        file,
                    )
                } else {
                    Delivery::Text(
                        format_text(&session.title, directory, body)?
                            .with_body_entities(body, entities)?,
                    )
                };
                Ok((session, delivery))
            })
            .await?;
        confirmed.bookkeeping?;
        Ok(confirmed.message)
    }

    pub(super) async fn send_start_ack(
        &self,
        ack: PreparedStartAck,
    ) -> Result<ConfirmedSend, AppError> {
        if !self.allowed_chats.contains(&ack.target.chat_id) {
            return Err(AppError::InvalidRequest("reply chat authorization"));
        }
        self.send_prepared(move |_| {
            Ok((
                ack.session,
                Delivery::StartAck {
                    text: ack.text,
                    target: ack.target,
                    reply_id: ack.reply_id,
                },
            ))
        })
        .await
    }

    async fn send_prepared(
        &self,
        prepare: impl FnOnce(&Database) -> Result<(Session, Delivery), AppError>,
    ) -> Result<ConfirmedSend, AppError> {
        // Track before admission: close/wait cannot miss a task that already has a permit.
        let task = self.tasks.token();
        let (delivery, reservation, journal, permit, session_key) = {
            let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
            let (session, delivery) = prepare(&database)?;
            let permit = Arc::clone(&self.admission)
                .try_acquire_owned()
                .map_err(|_| AppError::SendCapacity)?;
            let reservation = database.reserve_route(&session.session_key)?;
            let (text, kind) = match &delivery {
                Delivery::Text(text) => (text, OutgoingKind::Text { reply_to: None }),
                Delivery::Document(text, _) => (text, OutgoingKind::Document),
                Delivery::StartAck { text, target, .. } => (
                    text,
                    OutgoingKind::Text {
                        reply_to: Some(target.message_id),
                    },
                ),
            };
            let journal = database.reserve_outgoing(text.as_str(), Some(&session), kind)?;
            (delivery, reservation, journal, permit, session.session_key)
        };
        // No await separates admission from transfer to the owned task. Dropping the join
        // handle cannot abort the send or the synchronous post-send transaction.
        let service = self.clone();
        let outcome = SendOutcome(Some(self.failed.clone()));
        let handle = self.tasks.spawn(async move {
            let _permit = permit;
            let result = service
                .send_reserved(delivery, reservation, journal, &session_key)
                .await;
            let error = match &result {
                Ok(confirmed) => confirmed.bookkeeping.as_ref().err(),
                Err(error) => Some(error),
            };
            if let Some(error) = error {
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
    ) -> Result<ConfirmedSend, AppError> {
        let (sent, kind, start_ack_id) = match delivery {
            Delivery::Text(text) => (
                self.telegram.send_message(self.chat_id, &text).await?,
                RouteKind::Text,
                None,
            ),
            Delivery::Document(caption, file) => {
                let (file, filename, size) = file.into_parts();
                (
                    self.telegram
                        .send_document(self.chat_id, caption, file, filename, size)
                        .await?,
                    RouteKind::Document,
                    None,
                )
            }
            Delivery::StartAck {
                text,
                target,
                reply_id,
            } => (
                self.telegram
                    .send_message_with_reply(target.chat_id, &text, Some(target.message_id))
                    .await?,
                RouteKind::Text,
                Some(reply_id),
            ),
        };
        // Agent results clear typing. The start acknowledgment leaves work active.
        if start_ack_id.is_none() {
            self.typing.stop_session(session_key);
        }
        let bookkeeping =
            self.record_success(reservation, &sent, kind, journal, start_ack_id.as_deref());
        Ok(ConfirmedSend {
            message: sent,
            bookkeeping,
        })
    }

    fn record_success(
        &self,
        reservation: RouteReservation,
        sent: &SentMessage,
        kind: RouteKind,
        journal: Option<OutgoingJournal>,
        start_ack_id: Option<&str>,
    ) -> Result<(), AppError> {
        let route = confirmed_message(sent).and_then(|message| Ok((message, (self.now)()?)));
        let mut database = self
            .database
            .lock()
            .map_err(|_| AppError::SendBookkeepingFailed)?;
        // Telegram confirmation is independent of committing reply routing and history.
        let marked = start_ack_id.map_or(Ok(()), |id| database.confirm_start_ack(id));
        let persisted = route.and_then(|(confirmed, now)| {
            database.persist_outgoing_route(reservation, &confirmed, kind, journal, now)
        });
        marked
            .and(persisted)
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
