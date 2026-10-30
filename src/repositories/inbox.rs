//! Bounded read-only snapshots and the common transactional inbox/ask claim path.

use std::{fmt, sync::atomic::Ordering};

use rusqlite::{Connection, Row, TransactionBehavior};

use super::{
    Database,
    registry::{expiry, random_key, usage},
};
use crate::error::AppError;

const MAX_PAGE_ITEMS: u32 = 100;
pub(super) const TOKEN_BYTES: u64 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyState {
    Pending,
    Claimed,
    Processing,
}

/// Receipt order, independent of claims. Telegram batches are ordered by update ID.
#[derive(Clone, PartialEq, Eq)]
pub struct InboxCursor {
    pub created_at: i64,
    pub telegram_update_id: i64,
}

/// Composite outbound identity for a waiting ask. Always scoped by session as well.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReplyTarget {
    pub chat_id: i64,
    pub message_id: i64,
}

/// A resource snapshot carries delivery state, never a token granting ownership.
#[derive(PartialEq, Eq)]
pub struct InboxReply {
    pub reply_id: String,
    pub telegram_update_id: i64,
    pub telegram_chat_id: i64,
    pub telegram_message_id: i64,
    pub reply_to_bot_message_id: i64,
    pub text_or_caption: Option<String>,
    pub attachment_metadata_json: Option<String>,
    pub state: ReplyState,
    pub claimed_at: Option<i64>,
    pub claim_expires_at: Option<i64>,
    pub processing_started_at: Option<i64>,
    pub created_at: i64,
    pub expires_at: i64,
}

impl InboxReply {
    pub fn cursor(&self) -> InboxCursor {
        InboxCursor {
            created_at: self.created_at,
            telegram_update_id: self.telegram_update_id,
        }
    }
}

#[derive(PartialEq, Eq)]
pub struct InboxPage {
    pub replies: Vec<InboxReply>,
    /// Present only when more retained replies follow this page.
    pub next: Option<InboxCursor>,
}

/// Only a committed claim returns its fresh fencing token.
#[derive(PartialEq, Eq)]
pub struct ClaimedReply {
    pub reply: InboxReply,
    pub claim_token: String,
}

macro_rules! redacted_debug {
    ($($name:ident),+) => {$(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    )+};
}
redacted_debug!(
    InboxCursor,
    ReplyTarget,
    InboxReply,
    InboxPage,
    ClaimedReply
);

impl Database {
    /// Read all delivery states without claiming, refreshing activity or exposing tokens.
    /// Each page contains at most 100 items and `limits.reply_bytes` of combined payload.
    pub fn inbox_snapshot(
        &self,
        session_key: &str,
        after: Option<&InboxCursor>,
        limit: u32,
        now: i64,
    ) -> Result<InboxPage, AppError> {
        validate_query(limit, now)?;
        if after.is_some_and(|c| c.created_at < 0 || c.telegram_update_id < 0) {
            return Err(AppError::InvalidRequest("inbox cursor"));
        }
        ensure_session(&self.connection, session_key)?;
        let (selected, more) = select_replies(
            &self.connection,
            Selection {
                session_key,
                after,
                target: None,
                claimable: false,
                limit,
                byte_limit: self.registry.reply_bytes,
                now,
            },
        )?;
        let replies: Vec<_> = selected.into_iter().map(|s| s.reply).collect();
        let next = if more {
            replies.last().map(InboxReply::cursor)
        } else {
            None
        };
        Ok(InboxPage { replies, next })
    }

    pub(crate) fn claim_replies(
        &mut self,
        session_key: &str,
        target: Option<ReplyTarget>,
        limit: u32,
        lease_ms: i64,
        now: i64,
    ) -> Result<Vec<ClaimedReply>, AppError> {
        validate_query(limit, now)?;
        if target.is_some_and(|t| t.chat_id == 0 || t.message_id <= 0) {
            return Err(AppError::InvalidRequest("reply target"));
        }
        let lease_end = expiry(now, lease_ms)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_session(&transaction, session_key)?;
        let (selected, _) = select_replies(
            &transaction,
            Selection {
                session_key,
                after: None,
                target,
                claimable: true,
                limit,
                byte_limit: self.registry.reply_bytes,
                now,
            },
        )?;
        if !selected.is_empty() {
            let old_bytes: u64 = selected.iter().map(|s| s.token_bytes).sum();
            if usage(&transaction)?.metadata_bytes - old_bytes
                + TOKEN_BYTES * selected.len() as u64
                + self.reserved_routes.bytes.load(Ordering::SeqCst)
                > self.registry.metadata_bytes
            {
                return Err(AppError::StorageCapacity);
            }
        }
        let mut claimed = Vec::with_capacity(selected.len());
        for selected in selected {
            let mut reply = selected.reply;
            let token = random_key()?;
            let expires_at = lease_end.min(reply.expires_at);
            let changed = transaction.execute(
                "UPDATE replies SET state = 'claimed', claimed_at = ?3,
                 claim_token = ?4, claim_expires_at = ?5
                 WHERE session_key = ?1 AND reply_id = ?2 AND expires_at > ?3
                 AND (state = 'pending' OR (state = 'claimed' AND claim_expires_at <= ?3))",
                (session_key, &reply.reply_id, now, &token, expires_at),
            )?;
            if changed != 1 {
                return Err(AppError::StorageFailure);
            }
            reply.state = ReplyState::Claimed;
            reply.claimed_at = Some(now);
            reply.claim_expires_at = Some(expires_at);
            claimed.push(ClaimedReply {
                reply,
                claim_token: token,
            });
        }
        transaction.commit()?;
        if !claimed.is_empty() {
            self.inbox_notifications.publish(session_key);
        }
        Ok(claimed)
    }
}

fn validate_query(limit: u32, now: i64) -> Result<(), AppError> {
    if !(1..=MAX_PAGE_ITEMS).contains(&limit) {
        return Err(AppError::InvalidRequest("inbox limit"));
    }
    if now < 0 {
        return Err(AppError::InvalidRequest("timestamp"));
    }
    Ok(())
}

pub(super) fn ensure_session(connection: &Connection, session_key: &str) -> Result<(), AppError> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_key = ?1)",
        [session_key],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(AppError::SessionNotFound);
    }
    Ok(())
}

struct SelectedReply {
    reply: InboxReply,
    token_bytes: u64,
}

struct Selection<'a> {
    session_key: &'a str,
    after: Option<&'a InboxCursor>,
    target: Option<ReplyTarget>,
    claimable: bool,
    limit: u32,
    byte_limit: u64,
    now: i64,
}

fn select_replies(
    connection: &Connection,
    selection: Selection<'_>,
) -> Result<(Vec<SelectedReply>, bool), AppError> {
    let Selection {
        session_key,
        after,
        target,
        claimable,
        limit,
        byte_limit,
        now,
    } = selection;
    let mut statement = connection.prepare(
        "SELECT reply_id, telegram_update_id, telegram_chat_id, telegram_message_id,
         reply_to_bot_message_id, text_or_caption, attachment_metadata_json, state,
         claimed_at, claim_expires_at, processing_started_at, created_at, expires_at,
         coalesce(length(CAST(claim_token AS BLOB)), 0),
         coalesce(length(CAST(text_or_caption AS BLOB)), 0)
            + coalesce(length(CAST(attachment_metadata_json AS BLOB)), 0)
         FROM replies WHERE session_key = ?1 AND expires_at > ?2
         AND (?3 IS NULL OR (created_at, telegram_update_id) > (?3, ?4))
         AND (?5 IS NULL OR (telegram_chat_id = ?5 AND reply_to_bot_message_id = ?6))
         AND (?7 = 0 OR state = 'pending' OR (state = 'claimed' AND claim_expires_at <= ?2))
         ORDER BY created_at, telegram_update_id LIMIT ?8",
    )?;
    let mut rows = statement.query((
        session_key,
        now,
        after.map(|c| c.created_at),
        after.map(|c| c.telegram_update_id),
        target.map(|t| t.chat_id),
        target.map(|t| t.message_id),
        claimable,
        limit + 1,
    ))?;
    let mut selected = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        let payload_bytes: u64 = row.get(14)?;
        if selected.len() == limit as usize || bytes + payload_bytes > byte_limit {
            if selected.is_empty() {
                // A lower configured limit must not silently skip an older retained reply.
                return Err(AppError::StorageCapacity);
            }
            return Ok((selected, true));
        }
        bytes += payload_bytes;
        selected.push(SelectedReply {
            reply: reply_row(row)?,
            token_bytes: row.get(13)?,
        });
    }
    Ok((selected, false))
}

fn reply_row(row: &Row<'_>) -> rusqlite::Result<InboxReply> {
    let state: String = row.get(7)?;
    Ok(InboxReply {
        reply_id: row.get(0)?,
        telegram_update_id: row.get(1)?,
        telegram_chat_id: row.get(2)?,
        telegram_message_id: row.get(3)?,
        reply_to_bot_message_id: row.get(4)?,
        text_or_caption: row.get(5)?,
        attachment_metadata_json: row.get(6)?,
        state: match state.as_str() {
            "pending" => ReplyState::Pending,
            "claimed" => ReplyState::Claimed,
            "processing" => ReplyState::Processing,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
        claimed_at: row.get(8)?,
        claim_expires_at: row.get(9)?,
        processing_started_at: row.get(10)?,
        created_at: row.get(11)?,
        expires_at: row.get(12)?,
    })
}
