//! Read-only attachment provenance. Display metadata and handles never grant inbox ownership.

use rusqlite::{OptionalExtension, params};

use super::{Database, HistoryFile, MessageSource};
use crate::{config::INCOMING_ATTACHMENT_LIMIT, error::AppError};

#[derive(Clone)]
pub(crate) struct RetainedAttachment {
    pub source: MessageSource,
    pub file_id: String,
    pub file_unique_id: String,
    pub display: HistoryFile,
    pub own_expires_at: Option<i64>,
    pub shared_expires_at: Option<i64>,
    pub direction: Option<String>,
}

impl Database {
    pub(crate) fn retained_attachment(
        &self,
        session: &str,
        handle: &str,
        now: i64,
    ) -> Result<Option<RetainedAttachment>, AppError> {
        self.connection.query_row(
            "SELECT a.chat_id, a.message_id, a.sender_user_id, a.file_id, a.file_unique_id,
             a.kind, a.filename, a.mime_type, a.file_size, a.width, a.height, a.expires_at,
             CASE WHEN r.session_key = ?2 AND r.telegram_chat_id = a.chat_id
                AND r.telegram_message_id = a.message_id AND r.expires_at > ?3 THEN r.expires_at END,
             CASE WHEN j.chat_id = a.chat_id AND j.message_id = a.message_id
                AND j.sender_user_id IS a.sender_user_id AND j.expires_at > ?3 THEN j.expires_at END,
             j.direction FROM attachment_records a
             LEFT JOIN replies r ON r.reply_id = a.reply_id
             LEFT JOIN journal_records j ON j.message_handle = a.message_handle
             WHERE a.attachment_handle = ?1 AND a.expires_at > ?3",
            params![handle, session, now], |row| Ok(RetainedAttachment {
                source: MessageSource { chat_id: row.get(0)?, message_id: row.get(1)?, sender_user_id: row.get(2)? },
                file_id: row.get(3)?, file_unique_id: row.get(4)?,
                display: HistoryFile { handle: handle.to_owned(), kind: row.get(5)?, filename: row.get(6)?,
                    mime_type: row.get(7)?, file_size: row.get(8)?, width: row.get(9)?, height: row.get(10)?, expires_at: row.get(11)? },
                own_expires_at: row.get(12)?, shared_expires_at: row.get(13)?, direction: row.get(14)?,
            })).optional().map_err(Into::into)
    }

    pub(crate) fn own_attachment_handles(
        &self,
        session: &str,
        reply: &str,
        now: i64,
    ) -> Result<Vec<String>, AppError> {
        let mut query = self.connection.prepare(
            "SELECT a.attachment_handle FROM attachment_records a JOIN replies r ON r.reply_id = a.reply_id
             WHERE a.reply_id = ?1 AND r.session_key = ?2 AND r.expires_at > ?3 AND a.expires_at > ?3
             ORDER BY a.attachment_handle LIMIT ?4")?;
        let handles = query
            .query_map(
                params![reply, session, now, INCOMING_ATTACHMENT_LIMIT + 1],
                |row| row.get(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if handles.len() > INCOMING_ATTACHMENT_LIMIT {
            return Err(AppError::StorageFailure);
        }
        Ok(handles)
    }
}
