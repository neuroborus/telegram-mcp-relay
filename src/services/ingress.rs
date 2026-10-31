//! Authorize Telegram input before retaining any incoming content or identifiers.

use std::io::{self, Write};

use serde::Serialize;

use crate::{
    clients::telegram::{Attachment, Update},
    config::{Config, INCOMING_ATTACHMENT_LIMIT},
    domain::outbound::is_service_notice,
    error::AppError,
    repositories::{
        AttachmentInput, AttachmentKind, Database, IncomingObservation, IncomingReply,
        IngestOutcome, JournalInput, MessageDirection, MessageSource, PendingRoutes,
    },
};

pub struct IngressService {
    chats: Vec<i64>,
    users: Vec<u64>,
    reply_bytes: usize,
    capture: bool,
}

impl IngressService {
    pub fn new(config: &Config) -> Result<Self, AppError> {
        const MAX_ID: u64 = (1 << 52) - 1;
        let telegram = &config.telegram;
        if telegram.allowed_chat_ids.is_empty()
            || telegram.allowed_chat_ids.len() > 256
            || telegram
                .allowed_chat_ids
                .iter()
                .any(|id| *id == 0 || id.unsigned_abs() > MAX_ID)
            || telegram.allowed_user_ids.is_empty()
            || telegram.allowed_user_ids.len() > 256
            || telegram
                .allowed_user_ids
                .iter()
                .any(|id| *id == 0 || *id > MAX_ID)
        {
            return Err(AppError::InvalidConfig("Telegram ingress allowlists"));
        }
        if config.limits.reply_bytes == 0 || config.limits.reply_bytes > 16_777_216 {
            return Err(AppError::InvalidConfig("limits.reply_bytes"));
        }
        Ok(Self {
            chats: telegram.allowed_chat_ids.clone(),
            users: telegram.allowed_user_ids.clone(),
            reply_bytes: config.limits.reply_bytes as usize,
            capture: config.history.enabled,
        })
    }

    /// Synchronous commit receipt: only ReplyStored may trigger an inbox-change signal.
    /// The single poller must stop at an error and never submit a later update past it.
    pub fn ingest(
        &self,
        database: &mut Database,
        update: &Update,
        received_at: i64,
    ) -> Result<IngestOutcome, AppError> {
        let pending = database.pending_routes();
        self.ingest_observed(database, update, received_at, &pending)
    }

    /// The poller preserves the batch's admission snapshot across retries and updates.
    pub(crate) fn ingest_observed(
        &self,
        database: &mut Database,
        update: &Update,
        received_at: i64,
        pending: &PendingRoutes,
    ) -> Result<IngestOutcome, AppError> {
        if update.update_id < 0 || received_at < 0 {
            return Err(AppError::InvalidRequest(
                "Telegram update timestamp or identity",
            ));
        }
        let message = update.message.as_ref().filter(|m| {
            self.chats.contains(&m.chat.id)
                && m.from
                    .as_ref()
                    .is_some_and(|u| !u.is_bot && self.users.contains(&u.id))
                && m.message_id > 0
                && (m.text.is_some()
                    || m.caption.is_some()
                    || m.document.is_some()
                    || !m.photo.is_empty())
        });
        let source = message.map(|m| (m.chat.id, m.message_id));
        if database.update_processed(update.update_id, source)? {
            return Ok(IngestOutcome::Duplicate);
        }
        let target = message
            .and_then(|m| m.reply_to_message.as_ref())
            .filter(|r| r.message_id > 0);
        let Some(message) = message else {
            return database.ingest_update(update.update_id, None, None, None, received_at);
        };
        // Unroutable content never consumes inbox payload admission. Optional observation
        // is separate; the transaction still resolves the route authoritatively.
        let known_route = if let Some(target) = target {
            database
                .route(message.chat.id, target.message_id, received_at)?
                .is_some()
        } else {
            false
        };
        let mut route_reply = target.is_some();
        // Known success IDs work without quoted content.
        if !known_route
            && let Some(target) = target
            && database.is_notice_message(message.chat.id, target.message_id)?
        {
            route_reply = false;
        }
        // Only sends already admitted when this input was observed could have created
        // its target. Newer sends must not prolong this update's deferral.
        if route_reply && !known_route && pending.is_pending() {
            return Err(AppError::RoutePending);
        }
        // Reserved bot wording covers a lost response or failed success bookkeeping.
        // Wait for pending ordinary routes first: an agent may use the same wording.
        if route_reply
            && !known_route
            && let Some(target) = target
            && target.from.as_ref().is_some_and(|user| user.is_bot)
            && target.text.as_deref().is_some_and(is_service_notice)
        {
            route_reply = false;
        }
        let text = known_route
            .then(|| message.text.as_deref().or(message.caption.as_deref()))
            .flatten();
        let remaining = self
            .reply_bytes
            .checked_sub(text.map_or(0, str::len))
            .ok_or(AppError::StorageCapacity)?;
        let attachments =
            if known_route && (message.document.is_some() || !message.photo.is_empty()) {
                let mut output = BoundedJson {
                    bytes: Vec::new(),
                    remaining,
                };
                serde_json::to_writer(
                    &mut output,
                    &Attachments {
                        document: message.document.as_ref(),
                        photo: &message.photo,
                    },
                )
                .map_err(|_| AppError::StorageCapacity)?;
                Some(String::from_utf8(output.bytes).map_err(|_| AppError::StorageFailure)?)
            } else {
                None
            };
        let sender = message
            .from
            .as_ref()
            .ok_or(AppError::InvalidRequest("authorized sender"))?;
        let chat_label = display([message.chat.title.as_deref()].into_iter().flatten());
        let sender_label = display(
            [
                sender.first_name.as_deref(),
                sender.last_name.as_deref(),
                sender.username.as_deref(),
            ]
            .into_iter()
            .flatten(),
        );
        let observation = if self.capture || known_route {
            if message.photo.len() + usize::from(message.document.is_some())
                > INCOMING_ATTACHMENT_LIMIT
            {
                return Err(AppError::StorageCapacity);
            }
            let source = MessageSource {
                chat_id: message.chat.id,
                message_id: message.message_id,
                sender_user_id: Some(sender.id),
            };
            let metadata = message
                .document
                .iter()
                .map(|a| (a, AttachmentKind::Document))
                .chain(message.photo.iter().map(|a| (a, AttachmentKind::Photo)))
                .map(|(a, kind)| AttachmentInput {
                    source,
                    reply_id: None,
                    message_handle: None,
                    file_id: &a.file_id,
                    file_unique_id: &a.file_unique_id,
                    filename: a.file_name.as_deref(),
                    mime_type: a.mime_type.as_deref(),
                    file_size: a.file_size,
                    kind,
                    width: a.width,
                    height: a.height,
                })
                .collect();
            Some(IncomingObservation {
                capture: self.capture,
                journal: JournalInput {
                    source,
                    direction: MessageDirection::Incoming,
                    body: message.text.as_deref().or(message.caption.as_deref()),
                    chat_label: chat_label.as_deref(),
                    sender_label: sender_label.as_deref(),
                    session: None,
                    message_date: message
                        .date
                        .checked_mul(1_000)
                        .filter(|date| *date >= 0)
                        .ok_or(AppError::InvalidRequest("Telegram message date"))?,
                    reply_to_message_id: target.map(|t| t.message_id),
                },
                attachments: metadata,
            })
        } else {
            None
        };
        let reply = target.filter(|_| route_reply).map(|target| IncomingReply {
            chat_id: message.chat.id,
            message_id: message.message_id,
            reply_to_message_id: target.message_id,
            text,
            attachments: attachments.as_deref(),
        });
        database.ingest_update(update.update_id, source, reply, observation, received_at)
    }
}

fn display<'a>(parts: impl Iterator<Item = &'a str>) -> Option<String> {
    let label: String = parts
        .flat_map(|part| part.chars().chain(std::iter::once(' ')))
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .filter(|ch| !ch.is_control())
        .take(256)
        .collect();
    let label = label.trim();
    (!label.is_empty()).then(|| label.to_owned())
}

#[derive(Serialize)]
struct Attachments<'a> {
    document: Option<&'a Attachment>,
    photo: &'a [Attachment],
}

struct BoundedJson {
    bytes: Vec<u8>,
    remaining: usize,
}

impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::other("attachment metadata capacity"));
        }
        self.remaining -= bytes.len();
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
