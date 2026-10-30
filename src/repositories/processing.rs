//! Durable processing ownership and atomic payload-to-receipt completion.

use std::{fmt, sync::atomic::Ordering};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{
    Database,
    inbox::{TOKEN_BYTES, ensure_session},
    registry::{expiry, random_key, usage},
};
use crate::error::AppError;

/// Durable first start, independent of any in-memory typing activity.
#[derive(PartialEq, Eq)]
pub struct ProcessingStatus {
    pub telegram_chat_id: i64,
    pub processing_started_at: Option<i64>,
}

impl fmt::Debug for ProcessingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProcessingStatus([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct ReconciledProcessing {
    pub claim_token: String,
    pub status: ProcessingStatus,
}

impl fmt::Debug for ReconciledProcessing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReconciledProcessing([REDACTED])")
    }
}

impl Database {
    pub(crate) fn processing(
        &mut self,
        session_key: &str,
        reply_id: &str,
        claim_token: &str,
        active: bool,
        now: i64,
    ) -> Result<ProcessingStatus, AppError> {
        validate_transition(reply_id, claim_token, now)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_session(&transaction, session_key)?;
        let mut owned = owned_reply(&transaction, session_key, reply_id, claim_token, now)?;
        let started = active && owned.status.processing_started_at.is_none();
        if started {
            transaction.execute(
                "UPDATE replies SET state = 'processing', processing_started_at = ?4,
                 claim_expires_at = NULL
                 WHERE session_key = ?1 AND reply_id = ?2 AND claim_token = ?3",
                (session_key, reply_id, claim_token, now),
            )?;
            owned.status.processing_started_at = Some(now);
        }
        // Inactivity stops typing only; durable ownership and first start never reset.
        transaction.commit()?;
        if started {
            self.inbox_notifications.publish(session_key);
        }
        Ok(owned.status)
    }

    /// Return the recovered/new result and whether this call committed a new token rotation.
    pub(crate) fn reconcile_processing(
        &mut self,
        session_key: &str,
        reply_id: &str,
        claim_token: &str,
        request_id: &str,
        now: i64,
    ) -> Result<(ReconciledProcessing, bool), AppError> {
        validate_transition(reply_id, claim_token, now)?;
        if request_id.is_empty() || request_id.len() > 64 || request_id.contains('\0') {
            return Err(AppError::InvalidRequest("reconciliation request ID"));
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_session(&transaction, session_key)?;
        // Recover a lost committed response without admitting a new ownership change.
        let recovered = transaction
            .query_row(
                "SELECT claim_token, telegram_chat_id, processing_started_at FROM replies
                 WHERE session_key = ?1 AND reply_id = ?2 AND reconciliation_from_token = ?3
                 AND reconciliation_request_id = ?4 AND state = 'processing' AND expires_at > ?5",
                (session_key, reply_id, claim_token, request_id, now),
                |r| {
                    Ok(ReconciledProcessing {
                        claim_token: r.get(0)?,
                        status: ProcessingStatus {
                            telegram_chat_id: r.get(1)?,
                            processing_started_at: r.get(2)?,
                        },
                    })
                },
            )
            .optional()?;
        if let Some(recovered) = recovered {
            return Ok((recovered, false));
        }
        let owned = owned_reply(&transaction, session_key, reply_id, claim_token, now)?;
        if owned.status.processing_started_at.is_none() {
            return Err(AppError::InvalidClaim);
        }
        // Replace the last recovery proof and token, including outstanding reservations.
        if usage(&transaction)?.metadata_bytes - owned.reconciliation_bytes
            + TOKEN_BYTES
            + request_id.len() as u64
            + self.reserved_routes.bytes.load(Ordering::SeqCst)
            > self.registry.metadata_bytes
        {
            return Err(AppError::StorageCapacity);
        }
        let token = random_key()?;
        let changed = transaction.execute(
            "UPDATE replies SET claim_token = ?4, claimed_at = ?5,
             reconciliation_request_id = ?6, reconciliation_from_token = ?3
             WHERE session_key = ?1 AND reply_id = ?2 AND claim_token = ?3
             AND state = 'processing'",
            (session_key, reply_id, claim_token, &token, now, request_id),
        )?;
        if changed != 1 {
            return Err(AppError::InvalidClaim);
        }
        transaction.commit()?;
        self.inbox_notifications.publish(session_key);
        Ok((
            ReconciledProcessing {
                claim_token: token,
                status: owned.status,
            },
            true,
        ))
    }

    pub(crate) fn acknowledge_reply(
        &mut self,
        session_key: &str,
        reply_id: &str,
        claim_token: &str,
        now: i64,
    ) -> Result<(), AppError> {
        validate_transition(reply_id, claim_token, now)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_session(&transaction, session_key)?;
        let completed: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM acknowledgment_receipts
             WHERE session_key = ?1 AND reply_id = ?2 AND claim_token = ?3
             AND expires_at > ?4)",
            (session_key, reply_id, claim_token, now),
            |r| r.get(0),
        )?;
        if completed {
            return Ok(());
        }
        let owned = owned_reply(&transaction, session_key, reply_id, claim_token, now)?;
        let expires_at = expiry(now, self.registry.reply_ttl_ms)?;
        let used = usage(&transaction)?;
        // A receipt sheds attachments and the latest reconciliation recovery proof.
        if used.receipts >= u64::from(self.registry.receipts)
            || used.metadata_bytes - owned.attachment_bytes - owned.reconciliation_bytes
                + self.reserved_routes.bytes.load(Ordering::SeqCst)
                > self.registry.metadata_bytes
        {
            return Err(AppError::StorageCapacity);
        }
        transaction.execute(
            "INSERT INTO acknowledgment_receipts(reply_id, session_key, claim_token,
             telegram_update_id, created_at, expires_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            (
                reply_id,
                session_key,
                claim_token,
                owned.update_id,
                now,
                expires_at,
            ),
        )?;
        transaction.execute(
            "DELETE FROM replies WHERE session_key = ?1 AND reply_id = ?2 AND claim_token = ?3",
            (session_key, reply_id, claim_token),
        )?;
        transaction.commit()?;
        self.inbox_notifications.publish(session_key);
        Ok(())
    }
}

struct OwnedReply {
    status: ProcessingStatus,
    update_id: i64,
    attachment_bytes: u64,
    reconciliation_bytes: u64,
}

fn owned_reply(
    connection: &Connection,
    session_key: &str,
    reply_id: &str,
    claim_token: &str,
    now: i64,
) -> Result<OwnedReply, AppError> {
    connection
        .query_row(
            "SELECT telegram_chat_id, processing_started_at, telegram_update_id,
             coalesce(length(CAST(attachment_metadata_json AS BLOB)), 0),
             coalesce(length(CAST(reconciliation_request_id AS BLOB)), 0)
                + coalesce(length(CAST(reconciliation_from_token AS BLOB)), 0)
             FROM replies WHERE session_key = ?1 AND reply_id = ?2 AND claim_token = ?3
             AND expires_at > ?4 AND (state = 'processing'
                OR (state = 'claimed' AND claim_expires_at > ?4))",
            (session_key, reply_id, claim_token, now),
            |r| {
                Ok(OwnedReply {
                    status: ProcessingStatus {
                        telegram_chat_id: r.get(0)?,
                        processing_started_at: r.get(1)?,
                    },
                    update_id: r.get(2)?,
                    attachment_bytes: r.get(3)?,
                    reconciliation_bytes: r.get(4)?,
                })
            },
        )
        .optional()?
        .ok_or(AppError::InvalidClaim)
}

fn validate_transition(reply_id: &str, claim_token: &str, now: i64) -> Result<(), AppError> {
    if reply_id.is_empty() || claim_token.is_empty() {
        return Err(AppError::InvalidClaim);
    }
    if now < 0 {
        return Err(AppError::InvalidRequest("timestamp"));
    }
    Ok(())
}
