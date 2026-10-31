//! Durable, bounded attempt decisions. No network effects or per-reply timers.

use std::{fmt, sync::atomic::Ordering};

use rusqlite::TransactionBehavior;

use super::{
    Database,
    registry::{ROW_BYTES, usage},
};
use crate::{
    config::{Delivery, Notifications},
    error::AppError,
};

pub struct NoticePolicy {
    grace_ms: i64,
    unconfirmed: bool,
    unroutable: bool,
}

impl NoticePolicy {
    pub fn new(delivery: &Delivery, notifications: &Notifications) -> Result<Self, AppError> {
        if !(1..=86_400).contains(&delivery.processing_start_grace_seconds) {
            return Err(AppError::InvalidConfig(
                "delivery.processing_start_grace_seconds",
            ));
        }
        Ok(Self {
            grace_ms: delivery.processing_start_grace_seconds as i64 * 1_000,
            unconfirmed: notifications.notify_unconfirmed_processing,
            unroutable: notifications.notify_unroutable,
        })
    }
}

#[derive(PartialEq, Eq)]
pub enum NoticeKind {
    Unconfirmed { session_key: String },
    Unroutable,
}

/// Returned only after commit. Authorizes one dispatch, never a retry after uncertainty.
/// Dropping a result (including a crash before dispatch) consumes that attempt permanently.
#[derive(PartialEq, Eq)]
pub struct NoticeAttempt {
    pub telegram_update_id: i64,
    pub telegram_chat_id: i64,
    pub trigger_message_id: i64,
    pub kind: NoticeKind,
}

impl fmt::Debug for NoticeAttempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NoticeAttempt([REDACTED])")
    }
}

impl fmt::Debug for NoticeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NoticeKind([REDACTED])")
    }
}

impl Database {
    pub(crate) fn is_notice_message(
        &self,
        chat_id: i64,
        message_id: i64,
    ) -> Result<bool, AppError> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM notice_attempts
             WHERE telegram_chat_id = ?1 AND notice_message_id = ?2)",
            (chat_id, message_id),
            |r| r.get(0),
        )?)
    }

    /// Fixed-width success metadata uses the already admitted attempt, never a route.
    /// Failure leaves the durable attempt consumed and cannot authorize another send.
    pub(crate) fn record_notice_message(
        &mut self,
        attempt: &NoticeAttempt,
        message_id: i64,
    ) -> Result<(), AppError> {
        let changed = self.connection.execute(
            "UPDATE notice_attempts SET notice_message_id = ?4
             WHERE telegram_update_id = ?1 AND telegram_chat_id = ?2 AND trigger_message_id = ?3
                AND (notice_message_id IS NULL OR notice_message_id = ?4)",
            (
                attempt.telegram_update_id,
                attempt.telegram_chat_id,
                attempt.trigger_message_id,
                message_id,
            ),
        )?;
        if changed != 1 {
            return Err(AppError::StorageFailure);
        }
        Ok(())
    }

    /// Arbitrate with first processing start under the same SQLite write lock.
    /// The first committed decision wins; a later start cannot undo an admitted attempt.
    /// Eligibility uses original receipt/expiry, independent of claims and typing/waits.
    /// Capacity or SQL failure rolls back the entire bounded batch without losing replies.
    pub fn take_notice_attempts(
        &mut self,
        policy: &NoticePolicy,
        limit: u32,
        now: i64,
    ) -> Result<Vec<NoticeAttempt>, AppError> {
        if now < 0 || !(1..=100).contains(&limit) {
            return Err(AppError::InvalidRequest("notice batch timestamp or limit"));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let candidates = {
            let mut statement = transaction.prepare(
                "SELECT telegram_update_id, telegram_chat_id, telegram_message_id,
                    session_key, expires_at, created_at FROM replies
                 WHERE ?1 AND processing_started_at IS NULL AND notice_attempted = 0
                    AND created_at <= ?3 AND expires_at > ?4
                 UNION ALL
                 SELECT telegram_update_id, telegram_chat_id, telegram_message_id,
                    NULL, expires_at, created_at FROM update_dispositions
                 WHERE ?2 AND kind = 'unroutable' AND notice_attempted = 0
                    AND created_at <= ?4 AND expires_at > ?4
                 ORDER BY created_at, telegram_update_id LIMIT ?5",
            )?;
            statement
                .query_map(
                    (
                        policy.unconfirmed,
                        policy.unroutable,
                        now - policy.grace_ms,
                        now,
                        limit,
                    ),
                    |r| {
                        let key: Option<String> = r.get(3)?;
                        Ok((
                            NoticeAttempt {
                                telegram_update_id: r.get(0)?,
                                telegram_chat_id: r.get(1)?,
                                trigger_message_id: r.get(2)?,
                                kind: key.map_or(NoticeKind::Unroutable, |session_key| {
                                    NoticeKind::Unconfirmed { session_key }
                                }),
                            },
                            r.get::<_, i64>(4)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut attempts = Vec::new();
        if !candidates.is_empty() {
            let mut used = usage(&transaction)?;
            let reserved = self.reserved_routes.bytes.load(Ordering::SeqCst);
            for (attempt, expires_at) in candidates {
                let already_attempted: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM notice_attempts
                     WHERE telegram_chat_id = ?1 AND trigger_message_id = ?2)",
                    (attempt.telegram_chat_id, attempt.trigger_message_id),
                    |r| r.get(0),
                )?;
                let (kind, mark) = match &attempt.kind {
                    NoticeKind::Unconfirmed { .. } => (
                        "unconfirmed",
                        "UPDATE replies SET notice_attempted = 1
                         WHERE telegram_chat_id = ?1 AND telegram_message_id = ?2",
                    ),
                    NoticeKind::Unroutable => (
                        "unroutable",
                        "UPDATE update_dispositions SET notice_attempted = 1
                         WHERE kind = 'unroutable' AND telegram_chat_id = ?1
                         AND telegram_message_id = ?2",
                    ),
                };
                transaction
                    .execute(mark, (attempt.telegram_chat_id, attempt.trigger_message_id))?;
                if already_attempted {
                    continue;
                }
                if used.receipts >= u64::from(self.registry.receipts)
                    || used.metadata_bytes + reserved + ROW_BYTES > self.registry.metadata_bytes
                {
                    return Err(AppError::StorageCapacity);
                }
                transaction.execute(
                    "INSERT INTO notice_attempts(telegram_update_id, telegram_chat_id,
                     trigger_message_id, kind, created_at, expires_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    (
                        attempt.telegram_update_id,
                        attempt.telegram_chat_id,
                        attempt.trigger_message_id,
                        kind,
                        now,
                        expires_at,
                    ),
                )?;
                used.receipts += 1;
                used.metadata_bytes += ROW_BYTES;
                attempts.push(attempt);
            }
        }
        transaction.commit()?;
        Ok(attempts)
    }
}
