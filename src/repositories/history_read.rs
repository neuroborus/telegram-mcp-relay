//! Indexed, read-only journal scans. Filters never hide uncharged scan work.

use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::Database;
use crate::{config::INCOMING_ATTACHMENT_LIMIT, error::AppError};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryPosition {
    pub observed_at: i64,
    pub handle: String,
}

pub(crate) struct HistoryRecord {
    pub position: HistoryPosition,
    pub chat_id: i64,
    pub message_id: i64,
    pub direction: String,
    pub sender_user_id: Option<u64>,
    pub chat_label: Option<String>,
    pub sender_label: Option<String>,
    pub session_display_id: Option<String>,
    pub session_title: Option<String>,
    pub directory_label: Option<String>,
    pub body: Option<String>,
    pub parent: Option<i64>,
    pub message_date: i64,
    pub expires_at: i64,
}

#[derive(Clone)]
pub(crate) struct HistoryFile {
    pub handle: String,
    pub kind: String,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub expires_at: i64,
}

pub(crate) struct HistoryScan<'a> {
    pub chat_id: i64,
    pub after: Option<&'a HistoryPosition>,
    pub records: u32,
    pub bytes: u64,
}

pub(crate) enum ScanDecision {
    Continue,
    StopBefore,
    StopAfter,
}

impl Database {
    pub(crate) fn history_enabled(&self) -> bool {
        self.history.enabled
    }

    /// Read one record at a time, with one bounded lookahead. Even expired/unauthorized
    /// records count; selecting only matches in SQL could otherwise scan the whole chat.
    pub(crate) fn scan_history(
        &self,
        scan: HistoryScan<'_>,
        mut visit: impl FnMut(HistoryRecord) -> Result<ScanDecision, AppError>,
    ) -> Result<Option<HistoryPosition>, AppError> {
        let mut statement = self.connection.prepare(
            "SELECT observed_at, message_handle, chat_id, message_id, direction, sender_user_id,
             chat_label, sender_label, session_display_id, session_title, directory_label, body,
             reply_to_message_id, message_date, expires_at, body_bytes + metadata_bytes
             FROM journal_records WHERE chat_id = ?1 AND (observed_at, message_handle) > (?2, ?3)
             ORDER BY observed_at, message_handle LIMIT ?4",
        )?;
        let mut rows = statement.query(params![
            scan.chat_id,
            scan.after.map_or(-1, |p| p.observed_at),
            scan.after.map_or("", |p| p.handle.as_str()),
            u64::from(scan.records) + 1
        ])?;
        let mut after = scan.after.cloned();
        let mut count = 0;
        let mut bytes = 0_u64;
        let mut stopped = false;
        while let Some(row) = rows.next()? {
            let charge: u64 = row.get(15)?;
            if stopped
                || count == scan.records
                || count > 0 && charge > scan.bytes.saturating_sub(bytes)
            {
                return Ok(after);
            }
            // All bridge-written records have at most the hard 1 MiB body/display budget.
            if charge > 1_048_576 + 512 {
                return Err(AppError::StorageFailure);
            }
            let position = HistoryPosition {
                observed_at: row.get(0)?,
                handle: row.get(1)?,
            };
            let decision = visit(HistoryRecord {
                position: position.clone(),
                chat_id: row.get(2)?,
                message_id: row.get(3)?,
                direction: row.get(4)?,
                sender_user_id: row.get(5)?,
                chat_label: row.get(6)?,
                sender_label: row.get(7)?,
                session_display_id: row.get(8)?,
                session_title: row.get(9)?,
                directory_label: row.get(10)?,
                body: row.get(11)?,
                parent: row.get(12)?,
                message_date: row.get(13)?,
                expires_at: row.get(14)?,
            })?;
            match decision {
                ScanDecision::StopBefore => return Ok(after),
                ScanDecision::StopAfter => stopped = true,
                ScanDecision::Continue => (),
            }
            after = Some(position);
            count += 1;
            bytes += charge;
        }
        Ok(None)
    }

    /// Only safe display metadata/handles leave this projection; never adapter file IDs.
    pub(crate) fn history_files(
        &self,
        message: &HistoryRecord,
        now: i64,
    ) -> Result<Vec<HistoryFile>, AppError> {
        let mut statement = self.connection.prepare(
            "SELECT attachment_handle, kind, filename, mime_type, file_size, width, height,
             expires_at, chat_id, message_id, sender_user_id FROM attachment_records
             WHERE message_handle = ?1 LIMIT ?2",
        )?;
        let mut rows =
            statement.query((&message.position.handle, INCOMING_ATTACHMENT_LIMIT + 1))?;
        let mut files = Vec::new();
        let mut count = 0;
        while let Some(row) = rows.next()? {
            count += 1;
            if count > INCOMING_ATTACHMENT_LIMIT {
                return Err(AppError::StorageFailure);
            }
            if row.get::<_, i64>(7)? <= now
                || row.get::<_, i64>(8)? != message.chat_id
                || row.get::<_, i64>(9)? != message.message_id
                || row.get::<_, Option<u64>>(10)? != message.sender_user_id
            {
                continue;
            }
            files.push(HistoryFile {
                handle: row.get(0)?,
                kind: row.get(1)?,
                filename: row.get(2)?,
                mime_type: row.get(3)?,
                file_size: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                expires_at: row.get::<_, i64>(7)?.min(message.expires_at),
            });
        }
        Ok(files)
    }
}
