//! Durable processing ownership and atomic payload-to-receipt completion.

use std::{fmt, sync::atomic::Ordering};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use super::{
    Database, Session,
    inbox::{ReplyTarget, TOKEN_BYTES, ensure_session},
    registry::{expiry, random_key, session_by, usage},
};
use crate::{
    domain::outbound::{FormattedMessage, START_ACK_TEXT, format_text},
    error::AppError,
};

/// Durable first start, independent of any in-memory typing activity.
#[derive(PartialEq, Eq)]
pub struct ProcessingStatus {
    pub telegram_chat_id: i64,
    pub processing_started_at: Option<i64>,
}

#[derive(Clone, Copy)]
pub(crate) enum ProcessingAction<'a> {
    Start { message: Option<&'a str> },
    Stop,
}

pub(crate) struct ProcessingTransition {
    pub status: ProcessingStatus,
    pub start_ack: Option<PreparedStartAck>,
}

pub(crate) struct PreparedStartAck {
    pub session: Session,
    pub reply_id: String,
    pub target: ReplyTarget,
    pub text: FormattedMessage,
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
        action: ProcessingAction<'_>,
        now: i64,
    ) -> Result<ProcessingTransition, AppError> {
        validate_transition(reply_id, claim_token, now)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let session = if matches!(action, ProcessingAction::Start { .. }) {
            Some(
                session_by(&transaction, "session_key", session_key)?
                    .ok_or(AppError::SessionNotFound)?,
            )
        } else {
            ensure_session(&transaction, session_key)?;
            None
        };
        let directory = session
            .as_ref()
            .map(Session::outbound_directory_label)
            .transpose()?;
        let mut owned = owned_reply(&transaction, session_key, reply_id, claim_token, now)?;
        let first_start = owned.status.processing_started_at.is_none();
        // Validate explicit text on every active call. The default is only used on first start.
        let body = match action {
            ProcessingAction::Start { message } => {
                message.or(first_start.then_some(START_ACK_TEXT))
            }
            ProcessingAction::Stop => None,
        };
        let prepared = match (session.as_ref(), body) {
            (Some(session), Some(body)) => {
                Some((format_text(&session.title, directory, body)?, body))
            }
            _ => None,
        };
        let start_ack = if let Some((session, (text, body))) = session.zip(prepared)
            && first_start
        {
            if usage(&transaction)?.metadata_bytes
                + self.reserved_routes.bytes.load(Ordering::SeqCst)
                + body.len() as u64
                > self.registry.metadata_bytes
            {
                return Err(AppError::StorageCapacity);
            }
            transaction.execute(
                "UPDATE replies SET state = 'processing', processing_started_at = ?4,
                 claim_expires_at = NULL, start_ack_state = 'attempted', start_ack_text = ?5
                 WHERE session_key = ?1 AND reply_id = ?2 AND claim_token = ?3",
                (session_key, reply_id, claim_token, now, body),
            )?;
            owned.status.processing_started_at = Some(now);
            Some(PreparedStartAck {
                session,
                reply_id: reply_id.to_owned(),
                target: ReplyTarget {
                    chat_id: owned.status.telegram_chat_id,
                    message_id: owned.message_id,
                },
                text,
            })
        } else {
            None
        };
        // Inactivity stops typing only; durable ownership and first start never reset.
        transaction.commit()?;
        if start_ack.is_some() {
            self.inbox_notifications.publish(session_key);
        }
        Ok(ProcessingTransition {
            status: owned.status,
            start_ack,
        })
    }

    /// The attempt belongs to the reply, even if its claim was rotated mid-send.
    /// Completion or expiry may already have removed the reply and its retained text.
    pub(crate) fn confirm_start_ack(&self, reply_id: &str) -> Result<(), AppError> {
        self.connection.execute(
            "UPDATE replies SET start_ack_state = 'sent'
             WHERE reply_id = ?1 AND start_ack_state = 'attempted'",
            [reply_id],
        )?;
        Ok(())
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
        // A receipt sheds attachments, the recovery proof and the start acknowledgment text.
        if used.receipts >= u64::from(self.registry.receipts)
            || used.metadata_bytes
                - owned.attachment_bytes
                - owned.reconciliation_bytes
                - owned.start_ack_bytes
                + self.reserved_routes.bytes.load(Ordering::SeqCst)
                > self.registry.metadata_bytes
        {
            return Err(AppError::StorageCapacity);
        }
        transaction.execute(
            "INSERT INTO acknowledgment_receipts(reply_id, session_key, claim_token,
             telegram_update_id, created_at, expires_at, telegram_chat_id, telegram_message_id)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                reply_id,
                session_key,
                claim_token,
                owned.update_id,
                now,
                expires_at,
                owned.status.telegram_chat_id,
                owned.message_id,
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
    message_id: i64,
    attachment_bytes: u64,
    reconciliation_bytes: u64,
    start_ack_bytes: u64,
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
                + coalesce(length(CAST(reconciliation_from_token AS BLOB)), 0), telegram_message_id,
             coalesce(length(CAST(start_ack_text AS BLOB)), 0)
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
                    message_id: r.get(5)?,
                    start_ack_bytes: r.get(6)?,
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
