//! Authorize Telegram input before retaining any incoming content or identifiers.

use std::io::{self, Write};

use serde::Serialize;

use crate::{
    clients::telegram::{Attachment, Update},
    config::Config,
    domain::outbound::is_service_notice,
    error::AppError,
    repositories::{Database, IncomingReply, IngestOutcome, PendingRoutes},
};

pub struct IngressService {
    chats: Vec<i64>,
    users: Vec<u64>,
    reply_bytes: usize,
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
        if database.update_processed(update.update_id)? {
            return Ok(IngestOutcome::Duplicate);
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
        let target = message
            .and_then(|m| m.reply_to_message.as_ref())
            .filter(|r| r.message_id > 0);
        let (Some(message), Some(target)) = (message, target) else {
            return database.ingest_update(update.update_id, None, received_at);
        };
        // Unroutable input retains only a disposition, so its discarded contents need no
        // inbox admission. The transaction still resolves the route authoritatively.
        let known_route = database
            .route(message.chat.id, target.message_id, received_at)?
            .is_some();
        // Known success IDs work without quoted content.
        if !known_route && database.is_notice_message(message.chat.id, target.message_id)? {
            return database.ingest_update(update.update_id, None, received_at);
        }
        // Only sends already admitted when this input was observed could have created
        // its target. Newer sends must not prolong this update's deferral.
        if !known_route && pending.is_pending() {
            return Err(AppError::RoutePending);
        }
        // Reserved bot wording covers a lost response or failed success bookkeeping.
        // Wait for pending ordinary routes first: an agent may use the same wording.
        if !known_route
            && target.from.as_ref().is_some_and(|user| user.is_bot)
            && target.text.as_deref().is_some_and(is_service_notice)
        {
            return database.ingest_update(update.update_id, None, received_at);
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
        database.ingest_update(
            update.update_id,
            Some(IncomingReply {
                chat_id: message.chat.id,
                message_id: message.message_id,
                reply_to_message_id: target.message_id,
                text,
                attachments: attachments.as_deref(),
            }),
            received_at,
        )
    }
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
