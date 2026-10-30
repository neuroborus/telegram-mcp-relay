//! Atomic ordered ingestion. Returned changes describe committed state, never intent.

use std::{fmt, sync::atomic::Ordering};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{
    Database,
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

impl Database {
    /// Last resolved ID, not a global high-water mark: Telegram can reset its ID sequence.
    pub fn update_cursor(&self) -> Result<Option<i64>, AppError> {
        Ok(self.connection.query_row(
            "SELECT last_durably_processed_update_id FROM telegram_cursor WHERE singleton_id = 1",
            [],
            |r| r.get(0),
        )?)
    }

    pub(crate) fn update_processed(&self, update_id: i64) -> Result<bool, AppError> {
        update_processed(&self.connection, update_id)
    }

    /// Submit each ordered Telegram batch without skipping failed input. IDs can have gaps
    /// and reset after idle periods; numeric comparison cannot establish prior processing.
    pub(crate) fn ingest_update(
        &mut self,
        update_id: i64,
        reply: Option<IncomingReply<'_>>,
        now: i64,
    ) -> Result<IngestOutcome, AppError> {
        if update_id < 0 {
            return Err(AppError::InvalidRequest("Telegram update identity"));
        }
        let expires_at = expiry(now, self.registry.reply_ttl_ms)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if update_processed(&transaction, update_id)? {
            return Ok(IngestOutcome::Duplicate);
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
        let duplicate: bool = if let Some(reply) = &reply {
            transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM replies WHERE telegram_chat_id = ?1
                 AND telegram_message_id = ?2)",
                (reply.chat_id, reply.message_id),
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
                "INSERT INTO update_dispositions VALUES(?1, ?2, ?3, ?4, ?5, ?6)
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
        transaction.execute(
            "UPDATE telegram_cursor SET last_durably_processed_update_id = ?1 WHERE singleton_id = 1",
            [update_id],
        )?;
        transaction.commit()?;
        Ok(outcome)
    }
}

fn update_processed(connection: &Connection, update_id: i64) -> Result<bool, AppError> {
    Ok(connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM telegram_cursor WHERE last_durably_processed_update_id = ?1
            UNION ALL SELECT 1 FROM replies WHERE telegram_update_id = ?1
            UNION ALL SELECT 1 FROM acknowledgment_receipts WHERE telegram_update_id = ?1
            UNION ALL SELECT 1 FROM update_dispositions WHERE telegram_update_id = ?1)",
        [update_id],
        |r| r.get(0),
    )?)
}
