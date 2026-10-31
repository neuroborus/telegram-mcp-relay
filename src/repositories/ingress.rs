//! Atomic ordered ingestion. Returned changes describe committed state, never intent.

use std::{fmt, sync::atomic::Ordering};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{
    AttachmentInput, Database, JournalInput,
    history::{check_capacity, insert_attachment, insert_message},
    registry::{ROW_BYTES, expiry, random_key, usage},
};
use crate::error::AppError;

/// A commit receipt for later subscriber/waiter signaling. Duplicate/ignored/unroutable
/// updates never signal a new inbox item. Notification delivery is not durable delivery.
#[derive(PartialEq, Eq)]
pub enum IngestOutcome {
    ReplyStored {
        reply_id: String,
        session_key: String,
    },
    Duplicate,
    Ignored,
    Unroutable,
}

impl fmt::Debug for IngestOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IngestOutcome([REDACTED])")
    }
}

/// Authorized and bounded at the service boundary; no full Telegram update is retained.
pub(crate) struct IncomingReply<'a> {
    pub chat_id: i64,
    pub message_id: i64,
    pub reply_to_message_id: i64,
    pub text: Option<&'a str>,
    pub attachments: Option<&'a str>,
}

pub(crate) struct IncomingObservation<'a> {
    pub capture: bool,
    pub journal: JournalInput<'a>,
    pub attachments: Vec<AttachmentInput<'a>>,
}

impl Database {
    /// Last resolved ID, not a global high-water mark: Telegram can reset its ID sequence.
    pub fn update_cursor(&self) -> Result<Option<i64>, AppError> {
        Ok(self.connection.query_row(
            "SELECT last_durably_processed_update_id FROM telegram_cursor WHERE singleton_id = 1",
            [],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn update_processed(
        &self,
        update_id: i64,
        source: Option<(i64, i64)>,
    ) -> Result<bool, AppError> {
        update_processed(&self.connection, update_id, source)
    }

    /// Submit each ordered Telegram batch without skipping failed input. IDs can have gaps
    /// and reset after idle periods; numeric comparison cannot establish prior processing.
    pub(crate) fn ingest_update(
        &mut self,
        update_id: i64,
        source: Option<(i64, i64)>,
        reply: Option<IncomingReply<'_>>,
        observation: Option<IncomingObservation<'_>>,
        now: i64,
    ) -> Result<IngestOutcome, AppError> {
        if update_id < 0 {
            return Err(AppError::InvalidRequest("Telegram update identity"));
        }
        let expires_at = expiry(now, self.registry.reply_ttl_ms)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if update_processed(&transaction, update_id, source)? {
            return Ok(IngestOutcome::Duplicate);
        }
        if self.history.enabled && observation.as_ref().is_some_and(|o| o.capture) {
            transaction.execute("UPDATE history_coverage SET has_gaps = 1 WHERE observed_from IS NOT NULL
                AND (SELECT last_durably_processed_update_id FROM telegram_cursor WHERE singleton_id = 1) > ?1", [update_id])?;
        }
        let reserved_bytes = self.reserved_routes.bytes.load(Ordering::SeqCst);
        let session_key: Option<String> = if let Some(reply) = &reply {
            transaction
                .query_row(
                    "SELECT session_key FROM message_routes WHERE telegram_chat_id = ?1
                 AND telegram_message_id = ?2 AND expires_at > ?3",
                    (reply.chat_id, reply.reply_to_message_id, now),
                    |r| r.get(0),
                )
                .optional()?
        } else {
            None
        };
        let duplicate: bool = if let Some((chat, message)) = source {
            transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM replies WHERE telegram_chat_id = ?1
                 AND telegram_message_id = ?2
                 UNION ALL SELECT 1 FROM update_dispositions WHERE kind = 'unroutable'
                 AND telegram_chat_id = ?1 AND telegram_message_id = ?2
                 UNION ALL SELECT 1 FROM notice_attempts WHERE telegram_chat_id = ?1
                 AND trigger_message_id = ?2
                 UNION ALL SELECT 1 FROM journal_records WHERE chat_id = ?1 AND message_id = ?2
                 AND telegram_update_id IS NOT NULL
                 UNION ALL SELECT 1 FROM acknowledgment_receipts WHERE telegram_chat_id = ?1
                 AND telegram_message_id = ?2)",
                (chat, message),
                |r| r.get(0),
            )?
        } else {
            false
        };
        let outcome = if duplicate {
            IngestOutcome::Duplicate
        } else if let (Some(reply), Some(session_key)) = (&reply, session_key) {
            let used = usage(&transaction)?;
            let payload_bytes = reply.text.map_or(0, str::len) as u64
                + reply.attachments.map_or(0, str::len) as u64;
            let reply_id = random_key()?;
            let metadata_bytes = ROW_BYTES
                + reply_id.len() as u64
                + session_key.len() as u64
                + reply.attachments.map_or(0, str::len) as u64;
            if used.replies >= u64::from(self.registry.inbox_items)
                || payload_bytes > self.registry.reply_bytes
                || used.payload_bytes + payload_bytes > self.registry.inbox_bytes
                || used.metadata_bytes + reserved_bytes + metadata_bytes
                    > self.registry.metadata_bytes
            {
                return Err(AppError::StorageCapacity);
            }
            let session_expiry = expiry(now, self.registry.session_ttl_ms)?;
            transaction.execute(
                "INSERT INTO replies(reply_id, telegram_update_id, telegram_chat_id,
                     telegram_message_id, reply_to_bot_message_id, session_key, text_or_caption,
                     attachment_metadata_json, created_at, expires_at)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                (
                    &reply_id,
                    update_id,
                    reply.chat_id,
                    reply.message_id,
                    reply.reply_to_message_id,
                    &session_key,
                    reply.text,
                    reply.attachments,
                    now,
                    expires_at,
                ),
            )?;
            transaction.execute(
                "UPDATE sessions SET last_activity_at = max(last_activity_at, ?2),
                     expires_at = max(expires_at, ?3) WHERE session_key = ?1",
                (&session_key, now, session_expiry),
            )?;
            IngestOutcome::ReplyStored {
                reply_id,
                session_key,
            }
        } else {
            let target = reply.as_ref().map(|r| (r.chat_id, r.message_id));
            let (kind, outcome) = if target.is_some() {
                let used = usage(&transaction)?;
                if used.receipts >= u64::from(self.registry.receipts)
                    || used.metadata_bytes + reserved_bytes + ROW_BYTES
                        > self.registry.metadata_bytes
                {
                    return Err(AppError::StorageCapacity);
                }
                ("unroutable", IngestOutcome::Unroutable)
            } else {
                ("ignored", IngestOutcome::Ignored)
            };
            transaction.execute(
                "INSERT INTO update_dispositions(telegram_update_id, kind, telegram_chat_id,
                 telegram_message_id, created_at, expires_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(kind) WHERE kind = 'ignored' DO UPDATE SET
                    telegram_update_id = excluded.telegram_update_id,
                    created_at = excluded.created_at, expires_at = excluded.expires_at",
                (
                    update_id,
                    kind,
                    target.map(|t| t.0),
                    target.map(|t| t.1),
                    now,
                    expires_at,
                ),
            )?;
            outcome
        };
        if let Some(observation) = observation {
            let source = observation.journal.source;
            let owner: Option<(String, String)> = transaction
                .query_row(
                    "SELECT reply_id, session_key FROM replies WHERE telegram_chat_id = ?1
                 AND telegram_message_id = ?2 AND expires_at > ?3",
                    (source.chat_id, source.message_id, now),
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let session = owner
                .as_ref()
                .map(|(_, key)| super::registry::session_by(&transaction, "session_key", key))
                .transpose()?
                .flatten();
            let saved = if observation.capture && self.history.enabled {
                Some(insert_message(
                    &transaction,
                    &self.history,
                    &JournalInput {
                        session: session.as_ref(),
                        ..observation.journal
                    },
                    now,
                )?)
            } else {
                None
            };
            let message_handle = saved
                .as_ref()
                .filter(|s| s.expires_at > now)
                .map(|s| s.message_handle.as_str());
            if let Some(handle) = message_handle {
                transaction.execute(
                    "UPDATE journal_records SET telegram_update_id = ?1
                     WHERE message_handle = ?2 AND telegram_update_id IS NULL",
                    (update_id, handle),
                )?;
            }
            let has_attachments = !observation.attachments.is_empty();
            if owner.is_some() || message_handle.is_some() {
                for attachment in observation.attachments {
                    insert_attachment(
                        &transaction,
                        &self.history,
                        &AttachmentInput {
                            reply_id: owner.as_ref().map(|(id, _)| id.as_str()),
                            message_handle,
                            ..attachment
                        },
                        now,
                    )?;
                }
            }
            if saved.is_some() || owner.is_some() && has_attachments {
                check_capacity(
                    &transaction,
                    &self.history,
                    &self.registry,
                    reserved_bytes,
                    &self.reserved_history,
                    self.main_file_limit_bytes,
                )?;
            }
        }
        transaction.execute(
            "UPDATE telegram_cursor SET last_durably_processed_update_id = ?1,
             telegram_chat_id = ?2, telegram_message_id = ?3 WHERE singleton_id = 1",
            (update_id, source.map(|s| s.0), source.map(|s| s.1)),
        )?;
        transaction.commit()?;
        if let IngestOutcome::ReplyStored { session_key, .. } = &outcome {
            self.inbox_notifications.publish(session_key);
        }
        Ok(outcome)
    }
}

fn update_processed(
    connection: &Connection,
    update_id: i64,
    source: Option<(i64, i64)>,
) -> Result<bool, AppError> {
    let Some((chat_id, message_id)) = source else {
        // Ignored input retains no source identity. Only the current ignored cursor
        // proves a harmless retry; an older ID could belong to another sequence.
        return Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM telegram_cursor
             WHERE last_durably_processed_update_id = ?1 AND telegram_chat_id IS NULL)",
            [update_id],
            |r| r.get(0),
        )?);
    };
    Ok(connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM telegram_cursor WHERE last_durably_processed_update_id = ?1
                AND telegram_chat_id = ?2 AND telegram_message_id = ?3
            UNION ALL SELECT 1 FROM replies WHERE telegram_update_id = ?1
                AND telegram_chat_id = ?2 AND telegram_message_id = ?3
            UNION ALL SELECT 1 FROM acknowledgment_receipts WHERE telegram_update_id = ?1
                AND telegram_chat_id = ?2 AND telegram_message_id = ?3
            UNION ALL SELECT 1 FROM notice_attempts WHERE telegram_update_id = ?1
                AND telegram_chat_id = ?2 AND trigger_message_id = ?3
            UNION ALL SELECT 1 FROM update_dispositions WHERE telegram_update_id = ?1
                AND kind = 'unroutable' AND telegram_chat_id = ?2 AND telegram_message_id = ?3
            UNION ALL SELECT 1 FROM journal_records WHERE telegram_update_id = ?1
                AND chat_id = ?2 AND message_id = ?3)",
        (update_id, chat_id, message_id),
        |r| r.get(0),
    )?)
}
