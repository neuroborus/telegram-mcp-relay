//! Stable session identity, immutable message routes and pre-send metadata admission.

use std::{
    fmt::{self, Write},
    sync::{
        Arc, Weak,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior};
use tokio::sync::Notify;

use super::Database;
use crate::{
    config::{Limits, Storage},
    domain::outbound::SessionTitle,
    error::AppError,
};

const DAY_MS: i64 = 86_400_000;
// Logical admission charges include fixed row/index overhead, independently of SQLite pages.
pub(super) const ROW_BYTES: u64 = 128;

#[derive(Clone, PartialEq, Eq)]
pub struct Session {
    pub session_key: String,
    pub external_session_id: Option<String>,
    pub title: SessionTitle,
    pub created_at: i64,
    pub last_activity_at: i64,
    pub expires_at: i64,
}

impl Session {
    pub fn inbox_uri(&self) -> String {
        format!("telegram-bridge://sessions/{}/inbox", self.session_key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteKind {
    Text,
    Document,
}

impl RouteKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Document => "document",
        }
    }
}

#[derive(PartialEq, Eq)]
pub struct MessageRoute {
    pub session_key: String,
    pub kind: RouteKind,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Owned capacity for one in-flight send. Dropping it releases unused capacity.
/// Contains no database guard and may cross an await; it is valid only for its owner database.
pub struct RouteReservation {
    session_key: String,
    bytes: u64,
    reserved: Arc<ReservedRoutes>,
    pending: Option<Arc<String>>,
}

impl Drop for RouteReservation {
    fn drop(&mut self) {
        self.reserved.count.fetch_sub(1, Ordering::SeqCst);
        self.reserved.bytes.fetch_sub(self.bytes, Ordering::SeqCst);
        // Mark this admission settled before waking a snapshot's waiter.
        drop(self.pending.take());
        self.reserved.changed.notify_waiters();
    }
}

// Handles, external identities and titles must never appear in default diagnostics.
macro_rules! redacted_debug {
    ($($name:ident),+) => {$(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    )+};
}
redacted_debug!(Session, MessageRoute, RouteReservation);

#[derive(Default)]
pub(super) struct ReservedRoutes {
    count: AtomicU32,
    pub(super) bytes: AtomicU64,
    changed: Arc<Notify>,
}

/// A finite observation of admissions; later sends cannot extend its lifetime.
pub(crate) struct PendingRoutes(Vec<Weak<String>>);

impl PendingRoutes {
    pub(crate) fn is_pending(&self) -> bool {
        self.0.iter().any(|route| route.strong_count() != 0)
    }
}

pub(super) struct RegistryPolicy {
    pub(super) session_ttl_ms: i64,
    route_ttl_ms: i64,
    pub(super) reply_ttl_ms: i64,
    sessions: u32,
    routes: u32,
    pub(super) metadata_bytes: u64,
    pub(super) inbox_items: u32,
    pub(super) reply_bytes: u64,
    pub(super) inbox_bytes: u64,
    pub(super) receipts: u32,
}

impl RegistryPolicy {
    pub(super) fn new(storage: &Storage, limits: &Limits) -> Result<Self, AppError> {
        for (value, max, field) in [
            (storage.session_ttl_days, 3_650, "storage.session_ttl_days"),
            (storage.route_ttl_days, 3_650, "storage.route_ttl_days"),
            (storage.reply_ttl_days, 3_650, "storage.reply_ttl_days"),
            (u64::from(limits.sessions), 100_000, "limits.sessions"),
            (u64::from(limits.routes), 1_000_000, "limits.routes"),
            (
                u64::from(limits.inbox_items),
                1_000_000,
                "limits.inbox_items",
            ),
            (u64::from(limits.receipts), 1_000_000, "limits.receipts"),
            (
                u64::from(limits.reply_bytes),
                u64::from(limits.inbox_bytes.min(limits.telegram_response_bytes)),
                "limits.reply_bytes",
            ),
            (
                u64::from(limits.inbox_bytes),
                1_073_741_824,
                "limits.inbox_bytes",
            ),
            (
                u64::from(limits.metadata_bytes),
                1_073_741_824,
                "limits.metadata_bytes",
            ),
        ] {
            if value == 0 || value > max {
                return Err(AppError::InvalidConfig(field));
            }
        }
        Ok(Self {
            session_ttl_ms: storage.session_ttl_days as i64 * DAY_MS,
            route_ttl_ms: storage.route_ttl_days as i64 * DAY_MS,
            reply_ttl_ms: storage.reply_ttl_days as i64 * DAY_MS,
            sessions: limits.sessions,
            routes: limits.routes,
            metadata_bytes: u64::from(limits.metadata_bytes),
            inbox_items: limits.inbox_items,
            reply_bytes: u64::from(limits.reply_bytes),
            inbox_bytes: u64::from(limits.inbox_bytes),
            receipts: limits.receipts,
        })
    }
}

impl Database {
    /// Capture once when input is observed, then reuse across every ingress retry.
    pub(crate) fn pending_routes(&mut self) -> PendingRoutes {
        self.pending_routes
            .retain(|route| route.strong_count() != 0);
        PendingRoutes(self.pending_routes.clone())
    }

    pub(crate) fn route_changes(&self) -> Arc<Notify> {
        Arc::clone(&self.reserved_routes.changed)
    }

    /// Register by stable external ID or explicitly resume a saved handle. If both are
    /// supplied they must already identify the same session. With neither, create a new one.
    /// Registration/rename never changes routes, replies or processing ownership.
    pub fn register_session(
        &mut self,
        external_session_id: Option<&str>,
        session_key: Option<&str>,
        title: &str,
        now: i64,
    ) -> Result<Session, AppError> {
        if external_session_id.is_some_and(|id| id.is_empty() || id.contains('\0')) {
            return Err(AppError::InvalidRequest("external_session_id"));
        }
        let expires_at = expiry(now, self.registry.session_ttl_ms)?;
        let title = SessionTitle::new(title);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = if let Some(key) = session_key {
            let session =
                session_by(&transaction, "session_key", key)?.ok_or(AppError::SessionNotFound)?;
            if external_session_id.is_some()
                && external_session_id != session.external_session_id.as_deref()
            {
                return Err(AppError::SessionIdentityConflict);
            }
            Some(session)
        } else if let Some(external) = external_session_id {
            session_by(&transaction, "external_session_id", external)?
        } else {
            None
        };
        let mut session = if let Some(existing) = &existing {
            existing.clone()
        } else {
            Session {
                session_key: random_key()?,
                external_session_id: external_session_id.map(str::to_owned),
                title: title.clone(),
                created_at: now,
                last_activity_at: now,
                expires_at,
            }
        };
        session.title = title;
        session.last_activity_at = session.last_activity_at.max(now);
        session.expires_at = session.expires_at.max(expires_at);
        let used = usage(&transaction)?;
        let old_bytes = if let Some(existing) = &existing {
            // Schema v1 can contain titles not yet normalized by the registration API.
            let title_bytes: u64 = transaction.query_row(
                "SELECT length(CAST(title AS BLOB)) FROM sessions WHERE session_key = ?1",
                [&existing.session_key],
                |r| r.get(0),
            )?;
            session_bytes(existing) - existing.title.as_str().len() as u64 + title_bytes
        } else {
            0
        };
        if (existing.is_none() && used.sessions >= u64::from(self.registry.sessions))
            || used.metadata_bytes - old_bytes
                + session_bytes(&session)
                + self.reserved_routes.bytes.load(Ordering::SeqCst)
                > self.registry.metadata_bytes
        {
            return Err(AppError::StorageCapacity);
        }
        let sql = if existing.is_some() {
            "UPDATE sessions SET title = ?3, last_activity_at = ?5, expires_at = ?6
             WHERE session_key = ?1 AND external_session_id IS ?2 AND created_at = ?4"
        } else {
            // A random-key collision must fail rather than adopt another session's identity.
            "INSERT INTO sessions VALUES(?1, ?2, ?3, ?4, ?5, ?6)"
        };
        transaction.execute(
            sql,
            (
                &session.session_key,
                &session.external_session_id,
                session.title.as_str(),
                session.created_at,
                session.last_activity_at,
                session.expires_at,
            ),
        )?;
        transaction.commit()?;
        Ok(session)
    }

    /// Lookup does not refresh activity. A retained identity remains resumable even if its
    /// session TTL passed while still-valid child records keep it from cleanup.
    pub fn session(&self, session_key: &str) -> Result<Session, AppError> {
        session_by(&self.connection, "session_key", session_key)?.ok_or(AppError::SessionNotFound)
    }

    /// Reserve before external effects, after validating the complete outbound content.
    /// One shared Database owns all admissions, including simultaneous in-flight sends.
    pub fn reserve_route(&mut self, session_key: &str) -> Result<RouteReservation, AppError> {
        let session = self.session(session_key)?;
        let used = usage(&self.connection)?;
        let route_bytes = ROW_BYTES + session.session_key.len() as u64;
        if used.routes + u64::from(self.reserved_routes.count.load(Ordering::SeqCst))
            >= u64::from(self.registry.routes)
            || used.metadata_bytes + self.reserved_routes.bytes.load(Ordering::SeqCst) + route_bytes
                > self.registry.metadata_bytes
        {
            return Err(AppError::StorageCapacity);
        }
        // The admission pins recovery identity until bookkeeping settles or is dropped.
        let pending = Arc::new(session.session_key.clone());
        self.pending_routes
            .retain(|route| route.strong_count() != 0);
        self.pending_routes.push(Arc::downgrade(&pending));
        self.reserved_routes.count.fetch_add(1, Ordering::SeqCst);
        self.reserved_routes
            .bytes
            .fetch_add(route_bytes, Ordering::SeqCst);
        Ok(RouteReservation {
            session_key: session.session_key,
            bytes: route_bytes,
            reserved: Arc::clone(&self.reserved_routes),
            pending: Some(pending),
        })
    }

    /// Record a successful text/document send and refresh session activity atomically.
    /// `created_at` is the original Telegram message time, not bookkeeping/retry time.
    /// A failed commit after Telegram success must be reported as uncertain by the service.
    pub fn persist_route(
        &mut self,
        reservation: RouteReservation,
        chat_id: i64,
        message_id: i64,
        kind: RouteKind,
        created_at: i64,
        now: i64,
    ) -> Result<(), AppError> {
        if !Arc::ptr_eq(&reservation.reserved, &self.reserved_routes) {
            return Err(AppError::InvalidRequest("route reservation"));
        }
        if chat_id == 0 || message_id <= 0 {
            return Err(AppError::InvalidRequest("Telegram message identity"));
        }
        let route_expiry = expiry(created_at, self.registry.route_ttl_ms)?;
        let session_expiry = expiry(now, self.registry.session_ttl_ms)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = transaction.execute(
            "INSERT INTO message_routes VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(telegram_chat_id, telegram_message_id) DO NOTHING",
            (
                chat_id,
                message_id,
                &reservation.session_key,
                kind.as_str(),
                created_at,
                route_expiry,
            ),
        )?;
        if inserted == 0 {
            return Err(AppError::RouteConflict);
        }
        transaction.execute(
            "UPDATE sessions SET last_activity_at = max(last_activity_at, ?2),
             expires_at = max(expires_at, ?3) WHERE session_key = ?1",
            (&reservation.session_key, now, session_expiry),
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Composite ID lookup is read-only, and excludes routes at their expiry boundary.
    /// Session activity/expiry never changes the routing decision.
    pub fn route(
        &self,
        chat_id: i64,
        message_id: i64,
        now: i64,
    ) -> Result<Option<MessageRoute>, AppError> {
        if now < 0 {
            return Err(AppError::InvalidRequest("timestamp"));
        }
        Ok(self
            .connection
            .query_row(
                "SELECT session_key, kind, created_at, expires_at FROM message_routes
             WHERE telegram_chat_id = ?1 AND telegram_message_id = ?2 AND expires_at > ?3",
                (chat_id, message_id, now),
                |row| {
                    let kind: String = row.get(1)?;
                    Ok(MessageRoute {
                        session_key: row.get(0)?,
                        kind: match kind.as_str() {
                            "text" => RouteKind::Text,
                            "document" => RouteKind::Document,
                            _ => return Err(rusqlite::Error::InvalidQuery),
                        },
                        created_at: row.get(2)?,
                        expires_at: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }
}

pub(super) fn expiry(now: i64, ttl: i64) -> Result<i64, AppError> {
    if now < 0 {
        return Err(AppError::InvalidRequest("timestamp"));
    }
    now.checked_add(ttl)
        .ok_or(AppError::InvalidRequest("timestamp"))
}

pub(super) fn random_key() -> Result<String, AppError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| AppError::RandomUnavailable)?;
    let mut key = String::with_capacity(64);
    for byte in bytes {
        write!(key, "{byte:02x}").map_err(|_| AppError::RandomUnavailable)?;
    }
    Ok(key)
}

fn session_row(row: &Row<'_>) -> rusqlite::Result<Session> {
    let title: String = row.get(2)?;
    Ok(Session {
        session_key: row.get(0)?,
        external_session_id: row.get(1)?,
        title: SessionTitle::new(&title),
        created_at: row.get(3)?,
        last_activity_at: row.get(4)?,
        expires_at: row.get(5)?,
    })
}

fn session_by(
    connection: &Connection,
    column: &str,
    value: &str,
) -> Result<Option<Session>, AppError> {
    // The column is selected only by repository code, never caller input.
    Ok(connection.query_row(
        &format!("SELECT session_key, external_session_id, title, created_at, last_activity_at, expires_at
                  FROM sessions WHERE {column} = ?1"), [value], session_row,
    ).optional()?)
}

fn session_bytes(session: &Session) -> u64 {
    ROW_BYTES
        + session.session_key.len() as u64
        + session
            .external_session_id
            .as_ref()
            .map_or(0, |s| s.len() as u64)
        + session.title.as_str().len() as u64
}

pub(super) struct Usage {
    pub sessions: u64,
    pub routes: u64,
    pub metadata_bytes: u64,
    pub replies: u64,
    pub payload_bytes: u64,
    pub receipts: u64,
}

pub(super) fn usage(connection: &Connection) -> Result<Usage, AppError> {
    // Fixed cursor/compacted-ignore bookkeeping is covered by the physical database cap.
    Ok(connection.query_row(
        "SELECT (SELECT count(*) FROM sessions), (SELECT count(*) FROM message_routes),
         (SELECT coalesce(sum(128 + length(CAST(session_key AS BLOB))
             + coalesce(length(CAST(external_session_id AS BLOB)), 0)
             + length(CAST(title AS BLOB))), 0) FROM sessions)
         + (SELECT coalesce(sum(128 + length(CAST(session_key AS BLOB))), 0) FROM message_routes)
         + (SELECT coalesce(sum(128 + length(CAST(reply_id AS BLOB)) + length(CAST(session_key AS BLOB))
             + coalesce(length(CAST(claim_token AS BLOB)), 0)
             + coalesce(length(CAST(reconciliation_request_id AS BLOB)), 0)
             + coalesce(length(CAST(reconciliation_from_token AS BLOB)), 0)
             + coalesce(length(CAST(attachment_metadata_json AS BLOB)), 0)), 0) FROM replies)
         + (SELECT coalesce(sum(128 + length(CAST(reply_id AS BLOB)) + length(CAST(session_key AS BLOB))
             + length(CAST(claim_token AS BLOB))), 0) FROM acknowledgment_receipts)
         + (SELECT count(*) * 128 FROM notice_attempts)
         + (SELECT count(*) * 128 FROM update_dispositions WHERE kind = 'unroutable'),
         (SELECT count(*) FROM replies),
         (SELECT coalesce(sum(coalesce(length(CAST(text_or_caption AS BLOB)), 0)
             + coalesce(length(CAST(attachment_metadata_json AS BLOB)), 0)), 0) FROM replies),
         (SELECT count(*) FROM acknowledgment_receipts)
         + (SELECT count(*) FROM notice_attempts)
         + (SELECT count(*) FROM update_dispositions WHERE kind = 'unroutable')",
        [],
        |r| Ok(Usage { sessions: r.get(0)?, routes: r.get(1)?, metadata_bytes: r.get(2)?,
            replies: r.get(3)?, payload_bytes: r.get(4)?, receipts: r.get(5)? }),
    )?)
}
