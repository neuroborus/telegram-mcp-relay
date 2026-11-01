//! Authorized observation only: no claim, activity refresh or other durable effect.

use std::io::{self, Write};

use serde::{Deserialize, Serialize};

use crate::{
    config::{Config, History},
    dto::{HistoryAttachment, HistoryCoverage, HistoryMessage, HistoryRequest, HistoryResult},
    error::AppError,
    repositories::{Database, HistoryPosition, HistoryRecord, HistoryScan, ScanDecision},
};

pub struct HistoryService {
    settings: History,
    chat_id: i64,
    chats: Vec<i64>,
    users: Vec<u64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    chat_id: i64,
    session_display_id: Option<String>,
    query: Option<String>,
    after: HistoryPosition,
    expires_at: i64,
}

impl HistoryService {
    pub fn new(config: &Config) -> Result<Self, AppError> {
        config.history.validate(
            &config.storage,
            &config.limits,
            config.server.auth_token_env.is_some() || config.server.auth_token_file.is_some(),
        )?;
        if config.history.enabled
            && (config.telegram.chat_id == 0
                || !config
                    .telegram
                    .allowed_chat_ids
                    .contains(&config.telegram.chat_id)
                || !(1..=256).contains(&config.telegram.allowed_chat_ids.len())
                || !(1..=256).contains(&config.telegram.allowed_user_ids.len())
                || config
                    .telegram
                    .allowed_chat_ids
                    .iter()
                    .any(|id| *id == 0 || id.unsigned_abs() > (1 << 52) - 1)
                || config
                    .telegram
                    .allowed_user_ids
                    .iter()
                    .any(|id| *id == 0 || *id > (1 << 52) - 1))
        {
            return Err(AppError::InvalidConfig("history allowlists"));
        }
        Ok(Self {
            settings: config.history.clone(),
            chat_id: config.telegram.chat_id,
            chats: config.telegram.allowed_chat_ids.clone(),
            users: config.telegram.allowed_user_ids.clone(),
        })
    }

    pub fn read(
        &self,
        database: &Database,
        request: &HistoryRequest,
        now: i64,
    ) -> Result<HistoryResult, AppError> {
        if !self.settings.enabled || !database.history_enabled() {
            return Err(AppError::HistoryDisabled);
        }
        database.session(&request.session_key)?;
        let chat = request.chat_id.unwrap_or(self.chat_id);
        if !self.chats.contains(&chat) {
            return Err(AppError::HistoryForbidden);
        }
        let query = request.query.as_deref().filter(|q| !q.is_empty());
        let limit = request.limit.unwrap_or(self.settings.page_limit);
        if now < 0
            || !(1..=self.settings.page_limit).contains(&limit)
            || query
                .is_some_and(|q| q.len() > self.settings.query_bytes as usize || q.contains('\0'))
            || request
                .session_display_id
                .as_deref()
                .is_some_and(|id| !valid_handle(id))
        {
            return Err(AppError::InvalidRequest("history filters"));
        }
        let latest_expiry = now
            .checked_add(self.settings.retention_days as i64 * 86_400_000)
            .ok_or(AppError::InvalidRequest("history timestamp"))?;
        let mut cursor = Cursor {
            version: 1,
            chat_id: chat,
            session_display_id: request.session_display_id.clone(),
            query: query.map(str::to_owned),
            after: HistoryPosition {
                observed_at: i64::MAX,
                handle: "f".repeat(64),
            },
            expires_at: latest_expiry,
        };
        let after = if let Some(encoded) = &request.cursor {
            let saved = decode_cursor(encoded, self.settings.query_bytes as usize)?;
            if saved.version != 1
                || saved.chat_id != chat
                || saved.session_display_id != cursor.session_display_id
                || saved.query != cursor.query
                || saved.expires_at <= now
                || saved.expires_at > latest_expiry
                || saved.after.observed_at < 0
                || !valid_handle(&saved.after.handle)
            {
                return Err(AppError::InvalidHistoryCursor);
            }
            cursor.expires_at = saved.expires_at;
            Some(saved.after)
        } else {
            None
        };
        let mut result = HistoryResult {
            messages: Vec::new(),
            next_cursor: Some(encode_cursor(&cursor)?),
            scan_incomplete: false,
            coverage: HistoryCoverage {
                local_only: true,
                observation_start: None,
                retention_days: self.settings.retention_days,
                possible_gaps: true,
                known_gaps: database.history_coverage()?.has_gaps,
            },
        };
        // Reserve the maximum bookmark/framing size once. Count serialization without
        // allocating a second body, and never truncate a message or attachment list to fit.
        let budget = self.settings.response_bytes as usize;
        let mut used = json_size(&result, budget)?;
        let next = database.scan_history(
            HistoryScan {
                chat_id: chat,
                after: after.as_ref(),
                records: self.settings.scan_records,
                bytes: 2 * u64::from(
                    self.settings
                        .response_bytes
                        .max(self.settings.max_record_bytes),
                ),
            },
            |record| {
                if record.expires_at <= now
                    || !self.authorized(&record)
                    || request
                        .session_display_id
                        .as_ref()
                        .is_some_and(|id| record.session_display_id.as_ref() != Some(id))
                    || query.is_some_and(|query| !matches(&record, query))
                {
                    return Ok(ScanDecision::Continue);
                }
                let attachments = database
                    .history_files(&record, now)?
                    .into_iter()
                    .map(|file| HistoryAttachment {
                        attachment_handle: file.handle,
                        kind: file.kind,
                        filename: file.filename,
                        mime_type: file.mime_type,
                        file_size: file.file_size,
                        width: file.width,
                        height: file.height,
                        expires_at: file.expires_at,
                    })
                    .collect();
                let message = HistoryMessage {
                    message_handle: record.position.handle,
                    chat_id: record.chat_id,
                    message_id: record.message_id,
                    direction: record.direction,
                    sender_user_id: record.sender_user_id,
                    chat_label: record.chat_label,
                    sender_label: record.sender_label,
                    session_display_id: record.session_display_id,
                    session_title: record.session_title,
                    directory_label: record.directory_label,
                    text_or_caption: record.body,
                    related_message_id: record.parent,
                    message_date: record.message_date,
                    observed_at: record.position.observed_at,
                    expires_at: record.expires_at,
                    attachments,
                };
                let size = match json_size(&message, budget) {
                    Ok(size) => size + usize::from(!result.messages.is_empty()),
                    Err(AppError::HistoryResponseTooLarge) if !result.messages.is_empty() => {
                        return Ok(ScanDecision::StopBefore);
                    }
                    Err(error) => return Err(error),
                };
                if size > budget.saturating_sub(used) {
                    if result.messages.is_empty() {
                        return Err(AppError::HistoryResponseTooLarge);
                    }
                    return Ok(ScanDecision::StopBefore);
                }
                used += size;
                result.messages.push(message);
                Ok(if result.messages.len() == limit as usize {
                    ScanDecision::StopAfter
                } else {
                    ScanDecision::Continue
                })
            },
        )?;
        result.scan_incomplete = next.is_some();
        result.next_cursor = next
            .map(|position| {
                cursor.after = position;
                encode_cursor(&cursor)
            })
            .transpose()?;
        Ok(result)
    }

    fn authorized(&self, record: &HistoryRecord) -> bool {
        match record.direction.as_str() {
            "incoming" => record
                .sender_user_id
                .is_some_and(|id| self.users.contains(&id)),
            "outgoing" | "service" => record.sender_user_id.is_none(),
            _ => false,
        }
    }
}

fn matches(record: &HistoryRecord, query: &str) -> bool {
    [
        &record.body,
        &record.chat_label,
        &record.sender_label,
        &record.session_title,
        &record.directory_label,
    ]
    .iter()
    .any(|field| field.as_deref().is_some_and(|text| text.contains(query)))
}

fn valid_handle(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// Bookmarks carry only filters/order/expiry, never credentials or inbox recovery handles.
// They are not an authorization grant: current policy is always applied to every record.
fn encode_cursor(cursor: &Cursor) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(cursor).map_err(|_| AppError::StorageFailure)?;
    const HEX: &[u8] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 15) as usize] as char);
    }
    Ok(encoded)
}

fn decode_cursor(encoded: &str, query_limit: usize) -> Result<Cursor, AppError> {
    if encoded.len() > 2 * (query_limit * 6 + 512)
        || !encoded.len().is_multiple_of(2)
        || !encoded.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppError::InvalidHistoryCursor);
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).map_err(|_| AppError::InvalidHistoryCursor)?;
            u8::from_str_radix(pair, 16).map_err(|_| AppError::InvalidHistoryCursor)
        })
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::InvalidHistoryCursor)
}

fn json_size(value: &impl Serialize, limit: usize) -> Result<usize, AppError> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes) {
                return Err(io::Error::other("history response capacity"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| AppError::HistoryResponseTooLarge)?;
    Ok(counter.bytes)
}
