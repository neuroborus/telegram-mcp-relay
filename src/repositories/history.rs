//! Bounded observations and private attachment provenance, composed into ingress transactions.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, named_params};

use super::{
    Database, Session,
    registry::{expiry, random_key, usage},
};
use crate::error::AppError;

// Maximum file ID, unique ID, filename and MIME bytes validated by insert_attachment.
const DOCUMENT_METADATA_BYTES: u64 = 4_096 + 512 + 1_024 + 256;

#[derive(Clone, Copy)]
pub struct MessageSource {
    pub chat_id: i64,
    pub message_id: i64,
    pub sender_user_id: Option<u64>,
}

#[derive(Clone, Copy)]
pub enum MessageDirection {
    Incoming,
    Outgoing,
    Service,
}

#[derive(Clone, Copy)]
pub enum AttachmentKind {
    Document,
    Photo,
}

impl MessageDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Incoming => "incoming",
            Self::Outgoing => "outgoing",
            Self::Service => "service",
        }
    }
}

/// Already authorized by the service. Display snapshots are never routing authority.
pub struct JournalInput<'a> {
    pub source: MessageSource,
    pub direction: MessageDirection,
    pub body: Option<&'a str>,
    pub chat_label: Option<&'a str>,
    pub sender_label: Option<&'a str>,
    pub session: Option<&'a Session>,
    pub message_date: i64,
    pub reply_to_message_id: Option<i64>,
}

/// Service-authorized adapter provenance; never infer a missing legacy sender.
/// No serialization, arbitrary JSON, paths or binary payloads.
#[derive(Clone, Copy)]
pub struct AttachmentInput<'a> {
    pub source: MessageSource,
    pub reply_id: Option<&'a str>,
    pub message_handle: Option<&'a str>,
    pub file_id: &'a str,
    pub file_unique_id: &'a str,
    pub filename: Option<&'a str>,
    pub mime_type: Option<&'a str>,
    pub file_size: Option<u64>,
    pub kind: AttachmentKind,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

pub struct StoredMessage {
    pub message_handle: String,
    pub expires_at: i64,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct HistoryUsage {
    pub records: u64,
    pub attachments: u64,
    pub body_bytes: u64,
    pub metadata_bytes: u64,
}

#[derive(Default)]
pub(super) struct ReservedHistory {
    records: AtomicU64,
    attachments: AtomicU64,
    body_bytes: AtomicU64,
    metadata_bytes: AtomicU64,
}

impl ReservedHistory {
    fn change(&self, charge: &HistoryUsage, reserve: bool) {
        for (counter, bytes) in [
            (&self.records, charge.records),
            (&self.attachments, charge.attachments),
            (&self.body_bytes, charge.body_bytes),
            (&self.metadata_bytes, charge.metadata_bytes),
        ] {
            if reserve {
                counter.fetch_add(bytes, Ordering::SeqCst);
            } else {
                counter.fetch_sub(bytes, Ordering::SeqCst);
            }
        }
    }
}

struct HistoryReservation {
    shared: Arc<ReservedHistory>,
    charge: HistoryUsage,
}

impl Drop for HistoryReservation {
    fn drop(&mut self) {
        self.shared.change(&self.charge, false);
    }
}

/// Admission-time presentation, never reconstructed from a later session rename.
pub(crate) struct OutgoingJournal {
    body: String,
    session: Option<Session>,
    service_parent: Option<i64>,
    document: bool,
    reservation: HistoryReservation,
}

pub(crate) struct ConfirmedMessage<'a> {
    pub source: MessageSource,
    pub message_date: i64,
    pub document: Option<AttachmentInput<'a>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct HistoryCoverage {
    pub observed_from: Option<i64>,
    /// Timestamp of the last distinct stored observation, not continuous coverage through now.
    pub observed_through: Option<i64>,
    /// Restart/disable periods are possible gaps, not proof that every message was seen.
    pub has_gaps: bool,
}

macro_rules! redacted {
    ($($kind:ty),+) => {$(impl fmt::Debug for $kind {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("[REDACTED]") }
    })+};
}
redacted!(
    MessageSource,
    MessageDirection,
    JournalInput<'_>,
    AttachmentInput<'_>,
    AttachmentKind,
    StoredMessage,
    OutgoingJournal,
    ConfirmedMessage<'_>
);

impl Database {
    /// Reserve worst-case bounded returned document metadata, plus the exact body/display.
    /// RAM reservations release on every error/cancellation and never promise disk success.
    pub(crate) fn reserve_outgoing(
        &self,
        body: &str,
        session: Option<&Session>,
        document: bool,
        service_parent: Option<i64>,
    ) -> Result<Option<OutgoingJournal>, AppError> {
        if !self.history.enabled {
            return Ok(None);
        }
        let display = session.map_or(0, |s| {
            s.title.as_str().len() + s.directory_label.as_ref().map_or(0, |d| d.as_str().len())
        }) as u64;
        if body.len() as u64 + display > u64::from(self.history.max_record_bytes) {
            return Err(AppError::StorageCapacity);
        }
        let reservation = HistoryReservation {
            shared: Arc::clone(&self.reserved_history),
            charge: HistoryUsage {
                records: 1,
                attachments: u64::from(document),
                body_bytes: body.len() as u64,
                metadata_bytes: 512
                    + display
                    + if session.is_some() { 256 } else { 0 }
                    + if document {
                        512 + u64::from(self.history.max_record_bytes).min(DOCUMENT_METADATA_BYTES)
                    } else {
                        0
                    },
            },
        };
        reservation.shared.change(&reservation.charge, true);
        check_capacity(
            &self.connection,
            &self.history,
            &self.registry,
            self.reserved_routes.bytes.load(Ordering::SeqCst),
            &self.reserved_history,
            self.main_file_limit_bytes,
        )?;
        Ok(Some(OutgoingJournal {
            body: body.to_owned(),
            session: session.cloned(),
            service_parent,
            document,
            reservation,
        }))
    }

    pub(crate) fn mark_observation_gap(&self) -> Result<(), AppError> {
        if self.history.enabled {
            self.connection.execute(
                "UPDATE history_coverage SET has_gaps = 1 WHERE observed_from IS NOT NULL",
                [],
            )?;
        }
        Ok(())
    }
    pub fn history_usage(&self) -> Result<HistoryUsage, AppError> {
        history_usage(&self.connection)
    }

    /// Local coverage only. None means no observation, not empty Telegram history.
    pub fn history_coverage(&self) -> Result<HistoryCoverage, AppError> {
        Ok(self.connection.query_row(
            "SELECT observed_from, observed_through, has_gaps FROM history_coverage WHERE singleton_id = 1", [],
            |r| Ok(HistoryCoverage { observed_from: r.get(0)?, observed_through: r.get(1)?, has_gaps: r.get(2)? }),
        )?)
    }

    /// Standalone storage entry point. Ingress composes the same insert inside its cursor
    /// transaction; this wrapper changes no claims/cursor and emits no notifications.
    pub fn record_message(
        &mut self,
        input: &JournalInput<'_>,
        now: i64,
    ) -> Result<StoredMessage, AppError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = insert_message(&transaction, &self.history, input, now)?;
        check_capacity(
            &transaction,
            &self.history,
            &self.registry,
            self.reserved_routes.bytes.load(Ordering::SeqCst),
            &self.reserved_history,
            self.main_file_limit_bytes,
        )?;
        transaction.commit()?;
        Ok(stored)
    }

    /// Own-reply provenance is retained even with shared history disabled. Each supplied
    /// reference must match the message and be unexpired. The service verifies the sender;
    /// shared references additionally match the journal's retained sender provenance.
    pub fn record_attachment(
        &mut self,
        input: &AttachmentInput<'_>,
        now: i64,
    ) -> Result<String, AppError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let handle = insert_attachment(&transaction, &self.history, input, now)?;
        check_capacity(
            &transaction,
            &self.history,
            &self.registry,
            self.reserved_routes.bytes.load(Ordering::SeqCst),
            &self.reserved_history,
            self.main_file_limit_bytes,
        )?;
        transaction.commit()?;
        Ok(handle)
    }
}

pub(super) fn insert_outgoing(
    transaction: &Transaction<'_>,
    policy: &crate::config::History,
    journal: Option<OutgoingJournal>,
    sent: &ConfirmedMessage<'_>,
    now: i64,
) -> Result<(), AppError> {
    let Some(journal) = journal else {
        return Ok(());
    };
    let stored = insert_message(
        transaction,
        policy,
        &JournalInput {
            source: sent.source,
            direction: if journal.service_parent.is_some() {
                MessageDirection::Service
            } else {
                MessageDirection::Outgoing
            },
            body: Some(&journal.body),
            chat_label: None,
            sender_label: None,
            session: journal.session.as_ref(),
            message_date: sent.message_date,
            reply_to_message_id: journal.service_parent,
        },
        now,
    )?;
    if journal.document
        && let Some(attachment) = &sent.document
    {
        insert_attachment(
            transaction,
            policy,
            &AttachmentInput {
                source: sent.source,
                reply_id: None,
                message_handle: Some(&stored.message_handle),
                ..*attachment
            },
            now,
        )?;
    }
    // Replace this reservation by committed charges under the caller's database lock.
    drop(journal.reservation);
    Ok(())
}

pub(super) fn insert_message(
    transaction: &Transaction<'_>,
    policy: &crate::config::History,
    input: &JournalInput<'_>,
    now: i64,
) -> Result<StoredMessage, AppError> {
    if !policy.enabled {
        return Err(AppError::HistoryDisabled);
    }
    validate_source(input.source)?;
    if input.message_date < 0
        || input.reply_to_message_id.is_some_and(|id| id <= 0)
        || (matches!(input.direction, MessageDirection::Incoming)
            && input.source.sender_user_id.is_none())
    {
        return Err(AppError::InvalidRequest("journal source"));
    }
    let title = input.session.map(|s| s.title.as_str());
    let directory = input
        .session
        .and_then(|s| s.directory_label.as_ref().map(|d| d.as_str()));
    let display_bytes = bounded_fields(&[
        (input.chat_label, 1_024),
        (input.sender_label, 1_024),
        (title, 4_096),
        (directory, 400),
    ])?;
    let body_bytes = input.body.map_or(0, str::len) as u64;
    if body_bytes + display_bytes > u64::from(policy.max_record_bytes) {
        return Err(AppError::StorageCapacity);
    }
    let expires_at = expiry(now, policy.retention_days as i64 * 86_400_000)?;
    let existing = transaction.query_row(
            "SELECT message_handle, expires_at FROM journal_records WHERE chat_id = ?1 AND message_id = ?2
             AND sender_user_id IS ?3 AND direction = ?4",
            (input.source.chat_id, input.source.message_id, input.source.sender_user_id, input.direction.as_str()),
            |r| Ok(StoredMessage { message_handle: r.get(0)?, expires_at: r.get(1)? }),
        ).optional()?;
    if let Some(existing) = existing {
        return Ok(existing);
    }
    let display_id = input
        .session
        .map(|s| session_display(transaction, &s.session_key))
        .transpose()?;
    let message_handle = random_key()?;
    transaction.execute(
            "INSERT INTO journal_records(message_handle, chat_id, message_id, direction, sender_user_id,
             chat_label, sender_label, session_display_id, session_title, directory_label, body,
             message_date, observed_at, expires_at, body_bytes, metadata_bytes, reply_to_message_id)
             VALUES(:handle, :chat, :message, :direction, :sender,
             :chat_label, :sender_label, :display, :title, :directory, :body, :date, :now, :expiry, :body_bytes, :metadata, :parent)",
            named_params! {
                ":handle": message_handle, ":chat": input.source.chat_id, ":message": input.source.message_id,
                ":direction": input.direction.as_str(), ":sender": input.source.sender_user_id,
                ":chat_label": input.chat_label, ":sender_label": input.sender_label, ":display": display_id,
                ":title": title, ":directory": directory, ":body": input.body,
                ":date": input.message_date, ":now": now, ":expiry": expires_at,
                ":body_bytes": body_bytes, ":metadata": 512 + display_bytes,
                ":parent": input.reply_to_message_id,
            },
        )?;
    transaction.execute(
        "UPDATE history_coverage SET observed_from = coalesce(observed_from, ?1),
             observed_through = max(coalesce(observed_through, ?1), ?1) WHERE singleton_id = 1",
        [now],
    )?;
    Ok(StoredMessage {
        message_handle,
        expires_at,
    })
}

pub(super) fn insert_attachment(
    transaction: &Transaction<'_>,
    policy: &crate::config::History,
    input: &AttachmentInput<'_>,
    now: i64,
) -> Result<String, AppError> {
    validate_source(input.source)?;
    let sender = input.source.sender_user_id;
    if input.reply_id.is_some() && sender.is_none() {
        return Err(AppError::InvalidRequest("attachment source"));
    }
    let kind = match input.kind {
        AttachmentKind::Document if input.width.is_none() && input.height.is_none() => "document",
        AttachmentKind::Photo
            if input.width.is_some_and(|v| v > 0) && input.height.is_some_and(|v| v > 0) =>
        {
            "photo"
        }
        _ => return Err(AppError::InvalidRequest("attachment dimensions")),
    };
    let bytes = bounded_fields(&[
        (Some(input.file_id), 4_096),
        (Some(input.file_unique_id), 512),
        (input.filename, 1_024),
        (input.mime_type, 256),
    ])?;
    if input.file_id.is_empty()
        || input.file_unique_id.is_empty()
        || now < 0
        || input.file_size.is_some_and(|size| size > i64::MAX as u64)
        || bytes > u64::from(policy.max_record_bytes)
    {
        return Err(AppError::InvalidRequest("attachment metadata"));
    }
    let reply_expiry: Option<i64> = if let Some(reply) = input.reply_id {
        transaction
            .query_row(
                "SELECT expires_at FROM replies WHERE reply_id = ?1
                AND telegram_chat_id = ?2 AND telegram_message_id = ?3 AND expires_at > ?4",
                (reply, input.source.chat_id, input.source.message_id, now),
                |r| r.get(0),
            )
            .optional()?
    } else {
        None
    };
    let journal_expiry: Option<i64> = if policy.enabled
        && let Some(message) = input.message_handle
    {
        transaction
            .query_row(
                "SELECT expires_at FROM journal_records WHERE message_handle = ?1
                AND chat_id = ?2 AND message_id = ?3 AND sender_user_id IS ?4 AND expires_at > ?5",
                (
                    message,
                    input.source.chat_id,
                    input.source.message_id,
                    sender,
                    now,
                ),
                |r| r.get(0),
            )
            .optional()?
    } else {
        None
    };
    if (input.reply_id.is_some() && reply_expiry.is_none())
        || (input.message_handle.is_some() && journal_expiry.is_none())
    {
        return Err(AppError::InvalidRequest("attachment provenance"));
    }
    let expires_at = reply_expiry
        .into_iter()
        .chain(journal_expiry)
        .max()
        .ok_or(AppError::InvalidRequest("attachment provenance"))?;
    let handle = transaction.query_row(
            "INSERT INTO attachment_records VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(chat_id, message_id, file_unique_id) DO UPDATE SET
                reply_id = coalesce(attachment_records.reply_id, excluded.reply_id),
                message_handle = coalesce(attachment_records.message_handle, excluded.message_handle),
                expires_at = max(attachment_records.expires_at, excluded.expires_at)
             WHERE attachment_records.sender_user_id IS excluded.sender_user_id
                AND attachment_records.kind = excluded.kind
             RETURNING attachment_handle",
            rusqlite::params![random_key()?, input.source.chat_id, input.source.message_id, sender,
                input.reply_id, input.message_handle, input.file_id, input.file_unique_id,
                input.filename, input.mime_type, input.file_size, expires_at, 512 + bytes,
                kind, input.width, input.height], |r| r.get(0),
        )?;
    Ok(handle)
}

fn validate_source(source: MessageSource) -> Result<(), AppError> {
    const MAX_ID: u64 = (1 << 52) - 1;
    if source.chat_id == 0
        || source.chat_id.unsigned_abs() > MAX_ID
        || source.message_id <= 0
        || source
            .sender_user_id
            .is_some_and(|id| id == 0 || id > MAX_ID)
    {
        return Err(AppError::InvalidRequest("message source"));
    }
    Ok(())
}

fn bounded_fields(fields: &[(Option<&str>, usize)]) -> Result<u64, AppError> {
    let mut bytes = 0;
    for (value, limit) in fields {
        if let Some(value) = value {
            if value.len() > *limit || value.contains('\0') {
                return Err(AppError::InvalidRequest("display metadata"));
            }
            bytes += value.len() as u64;
        }
    }
    Ok(bytes)
}

fn session_display(connection: &Connection, key: &str) -> Result<String, AppError> {
    super::inbox::ensure_session(connection, key)?;
    if let Some(id) = connection
        .query_row(
            "SELECT display_id FROM session_displays WHERE session_key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?
    {
        return Ok(id);
    }
    let id = random_key()?;
    connection.execute("INSERT INTO session_displays VALUES(?1, ?2)", (key, &id))?;
    Ok(id)
}

fn history_usage(connection: &Connection) -> Result<HistoryUsage, AppError> {
    Ok(connection.query_row("SELECT records, attachments, body_bytes, metadata_bytes FROM history_usage WHERE singleton_id = 1", [],
        |r| Ok(HistoryUsage { records: r.get(0)?, attachments: r.get(1)?, body_bytes: r.get(2)?, metadata_bytes: r.get(3)? }))?)
}

pub(super) fn check_capacity(
    connection: &Connection,
    policy: &crate::config::History,
    registry: &super::registry::RegistryPolicy,
    reserved: u64,
    history: &ReservedHistory,
    physical_limit: u64,
) -> Result<(), AppError> {
    let used = history_usage(connection)?;
    let inbox = usage(connection)?;
    let pages: u64 = connection.pragma_query_value(None, "page_count", |r| r.get(0))?;
    let free: u64 = connection.pragma_query_value(None, "freelist_count", |r| r.get(0))?;
    let page_size: u64 = connection.pragma_query_value(None, "page_size", |r| r.get(0))?;
    // Leave unused inbox/metadata budgets as physical page headroom. Shared overhead can
    // still reject writes; no pressure operation deletes valid records or confirms input.
    let headroom = registry.inbox_bytes.saturating_sub(inbox.payload_bytes)
        + registry.metadata_bytes.saturating_sub(inbox.metadata_bytes)
        + reserved
        + history.body_bytes.load(Ordering::SeqCst)
        + history.metadata_bytes.load(Ordering::SeqCst);
    if used.records + history.records.load(Ordering::SeqCst) > u64::from(policy.max_records)
        || used.attachments + history.attachments.load(Ordering::SeqCst)
            > u64::from(policy.max_attachments)
        || used.body_bytes + history.body_bytes.load(Ordering::SeqCst)
            > u64::from(policy.max_body_bytes)
        || used.metadata_bytes + history.metadata_bytes.load(Ordering::SeqCst)
            > u64::from(policy.max_metadata_bytes)
        || pages.checked_sub(free).ok_or(AppError::StorageFailure)? * page_size + headroom
            > physical_limit
    {
        return Err(AppError::StorageCapacity);
    }
    Ok(())
}
