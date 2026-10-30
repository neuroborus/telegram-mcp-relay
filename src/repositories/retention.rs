//! Indexed, bounded expiry deletion. Child lifetimes never depend on their parent TTL.

use std::{collections::HashSet, fmt};

use rusqlite::TransactionBehavior;

use super::Database;
use crate::error::AppError;

const BATCH: usize = 100;

/// A fixed cutoff and bounded session cursor. Restarting a sweep is safe after cancellation.
/// Keep this state local to one sweep; failed transactions do not advance it.
pub struct CleanupSweep {
    now: i64,
    after: Option<(i64, String)>,
    complete: bool,
}

impl CleanupSweep {
    pub fn new(now: i64) -> Result<Self, AppError> {
        if now < 0 {
            return Err(AppError::InvalidRequest("cleanup timestamp"));
        }
        Ok(Self {
            now,
            after: None,
            complete: false,
        })
    }
}

#[derive(PartialEq, Eq)]
pub struct CleanupBatch {
    pub removed: usize,
    pub complete: bool,
    pub(crate) expired_reply_ids: Vec<String>,
}

impl fmt::Debug for CleanupBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CleanupBatch([REDACTED])")
    }
}

impl Database {
    /// At most 100 rows per child table and 100 session candidates per transaction.
    /// Delete children first, then scan expired sessions once, including protected ones.
    /// No pressure eviction, cascade deletion, cursor reset or physical vacuum occurs here.
    pub fn cleanup_batch(&mut self, sweep: &mut CleanupSweep) -> Result<CleanupBatch, AppError> {
        if sweep.complete {
            return Ok(CleanupBatch {
                removed: 0,
                complete: true,
                expired_reply_ids: Vec::new(),
            });
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = {
            let mut statement = transaction.prepare(
                "DELETE FROM replies WHERE reply_id IN (
                    SELECT reply_id FROM replies WHERE expires_at <= ?1
                    ORDER BY expires_at LIMIT 100) RETURNING session_key, reply_id",
            )?;
            statement
                .query_map([sweep.now], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut removed = changed.len();
        let mut more_children = changed.len() == BATCH;
        for (table, key, columns) in [
            ("acknowledgment_receipts", "reply_id", "reply_id"),
            (
                "notice_attempts",
                "telegram_update_id",
                "telegram_update_id",
            ),
            (
                "update_dispositions",
                "telegram_update_id",
                "telegram_update_id",
            ),
            (
                "message_routes",
                "(telegram_chat_id, telegram_message_id)",
                "telegram_chat_id, telegram_message_id",
            ),
        ] {
            // Both identifiers are fixed repository constants, never caller input.
            let count = transaction.execute(
                &format!(
                    "DELETE FROM {table} WHERE {key} IN (
                        SELECT {columns} FROM {table} WHERE expires_at <= ?1
                        ORDER BY expires_at LIMIT 100)"
                ),
                [sweep.now],
            )?;
            removed += count;
            more_children |= count == BATCH;
        }
        let mut after = sweep.after.clone();
        let mut complete = false;
        if !more_children {
            let candidates = {
                let mut statement = transaction.prepare(
                    "SELECT expires_at, session_key FROM sessions
                     WHERE expires_at <= ?1 AND (expires_at, session_key) > (?2, ?3)
                     ORDER BY expires_at, session_key LIMIT 100",
                )?;
                statement
                    .query_map(
                        (
                            sweep.now,
                            after.as_ref().map_or(-1, |c| c.0),
                            after.as_ref().map_or("", |c| c.1.as_str()),
                        ),
                        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
                    )?
                    .collect::<Result<Vec<_>, _>>()?
            };
            // Snapshot admitted sends while holding the shared database guard. Weak
            // entries retain no session identity after their reservation has settled.
            self.pending_routes.retain(|r| r.strong_count() != 0);
            let pinned: HashSet<_> = self
                .pending_routes
                .iter()
                .filter_map(|r| r.upgrade())
                .collect();
            for (_, key) in &candidates {
                if pinned.contains(key) {
                    continue;
                }
                removed += transaction.execute(
                    "DELETE FROM sessions WHERE session_key = ?1
                     AND NOT EXISTS(SELECT 1 FROM message_routes WHERE session_key = ?1)
                     AND NOT EXISTS(SELECT 1 FROM replies WHERE session_key = ?1)
                     AND NOT EXISTS(SELECT 1 FROM acknowledgment_receipts WHERE session_key = ?1)",
                    [key],
                )?;
            }
            complete = candidates.len() < BATCH;
            if let Some(last) = candidates.into_iter().last() {
                after = Some(last);
            }
        }
        transaction.commit()?;
        sweep.after = after;
        sweep.complete = complete;
        let mut expired_reply_ids = Vec::with_capacity(changed.len());
        for (key, reply_id) in changed {
            self.inbox_notifications.publish(&key);
            expired_reply_ids.push(reply_id);
        }
        Ok(CleanupBatch {
            removed,
            complete,
            expired_reply_ids,
        })
    }
}
