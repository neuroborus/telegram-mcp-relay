//! MCP inbox serialization and URI validation; durable transitions stay service-owned.

use rmcp::model::{CacheScope, PaginatedRequestParams, ReadResourceResult, ResourceContents};

use super::*;
use crate::{
    dto::{ClaimedReplyResult, DeliveryState, InboxSnapshot, Reply, inbox_limit},
    repositories::{InboxCursor, InboxReply, ReplyState},
};

impl McpHandler {
    pub(super) fn claim_inbox(&self, request: InboxRequest) -> Result<InboxResult, AppError> {
        let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
        let claims = self.inbox.claim(
            &mut database,
            &request.session_key,
            None,
            request.limit,
            unix_millis()?,
        )?;
        Ok(InboxResult {
            replies: claims.into_iter().map(claim).collect::<Result<_, _>>()?,
        })
    }

    pub(super) fn processing(
        &self,
        request: ProcessingRequest,
    ) -> Result<ProcessingResult, AppError> {
        let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
        let now = unix_millis()?;
        let (status, token) = match (request.active, request.reconcile_request_id) {
            (Some(active), None) => (
                self.inbox.processing(
                    &mut database,
                    &request.session_key,
                    &request.reply_id,
                    &request.claim_token,
                    active,
                    now,
                )?,
                request.claim_token,
            ),
            (None, Some(id)) => {
                let resumed = self.inbox.reconcile_processing(
                    &mut database,
                    &request.session_key,
                    &request.reply_id,
                    &request.claim_token,
                    &id,
                    now,
                )?;
                (resumed.status, resumed.claim_token)
            }
            _ => return Err(AppError::InvalidRequest("processing action")),
        };
        Ok(ProcessingResult {
            reply_id: request.reply_id,
            claim_token: token,
            processing_started_at: status.processing_started_at,
        })
    }

    pub(super) fn acknowledge(&self, request: AckRequest) -> Result<AckResult, AppError> {
        let mut database = self.database.lock().map_err(|_| AppError::StorageFailure)?;
        self.inbox.acknowledge(
            &mut database,
            &request.session_key,
            &request.reply_id,
            &request.claim_token,
            unix_millis()?,
        )?;
        Ok(AckResult {
            reply_id: request.reply_id,
            acknowledged: true,
        })
    }

    pub(super) fn read_inbox(&self, uri: &str) -> Result<ReadResourceResult, ErrorData> {
        let (key, after, limit) = parse_uri(uri)?;
        let snapshot = (|| {
            let page = self
                .database
                .lock()
                .map_err(|_| AppError::StorageFailure)?
                .inbox_snapshot(key, after.as_ref(), limit, unix_millis()?)?;
            let next_uri = page.next.map(|cursor| {
                format!(
                    "telegram-bridge://sessions/{key}/inbox?limit={limit}&after={},{}",
                    cursor.created_at, cursor.telegram_update_id
                )
            });
            let snapshot = InboxSnapshot {
                replies: page
                    .replies
                    .into_iter()
                    .map(reply)
                    .collect::<Result<_, _>>()?,
                next_uri,
            };
            serde_json::to_string(&snapshot).map_err(|_| AppError::StorageFailure)
        })()
        .map_err(resource_error)?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(snapshot, uri).with_mime_type("application/json"),
        ])
        .with_ttl_ms(0)
        .with_cache_scope(CacheScope::Private))
    }
}

pub(super) fn claim(
    claim: crate::repositories::ClaimedReply,
) -> Result<ClaimedReplyResult, AppError> {
    Ok(ClaimedReplyResult {
        reply: reply(claim.reply)?,
        claim_token: claim.claim_token,
    })
}

pub(super) fn reply(reply: InboxReply) -> Result<Reply, AppError> {
    Ok(Reply {
        reply_id: reply.reply_id,
        telegram_update_id: reply.telegram_update_id,
        telegram_chat_id: reply.telegram_chat_id,
        telegram_message_id: reply.telegram_message_id,
        reply_to_bot_message_id: reply.reply_to_bot_message_id,
        text_or_caption: reply.text_or_caption,
        attachment_metadata: reply
            .attachment_metadata_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(|_| AppError::StorageFailure)?,
        state: match reply.state {
            ReplyState::Pending => DeliveryState::Pending,
            ReplyState::Claimed => DeliveryState::Claimed,
            ReplyState::Processing => DeliveryState::Processing,
        },
        claimed_at: reply.claimed_at,
        claim_expires_at: reply.claim_expires_at,
        processing_started_at: reply.processing_started_at,
        created_at: reply.created_at,
        expires_at: reply.expires_at,
    })
}

pub(super) fn validate_listing(request: Option<PaginatedRequestParams>) -> Result<(), ErrorData> {
    if request.is_some_and(|request| request.cursor.is_some()) {
        return Err(ErrorData::invalid_params(
            "Invalid resource listing cursor",
            None,
        ));
    }
    Ok(())
}

fn parse_uri(uri: &str) -> Result<(&str, Option<InboxCursor>, u32), ErrorData> {
    let invalid = || ErrorData::invalid_params("Invalid inbox URI", None);
    let (base, query) = uri.split_once('?').unwrap_or((uri, ""));
    let key = base
        .strip_prefix("telegram-bridge://sessions/")
        .and_then(|s| s.strip_suffix("/inbox"))
        .ok_or_else(invalid)?;
    if key.len() != 64
        || !key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    let mut limit = None;
    let mut after = None;
    for pair in query.split('&').filter(|_| !query.is_empty()) {
        let (name, value) = pair.split_once('=').ok_or_else(invalid)?;
        match name {
            "limit" if limit.is_none() => {
                let value: u32 = value.parse().map_err(|_| invalid())?;
                if !(1..=100).contains(&value) {
                    return Err(invalid());
                }
                limit = Some(value);
            }
            "after" if after.is_none() => {
                let (created, update) = value.split_once(',').ok_or_else(invalid)?;
                let created_at: i64 = created.parse().map_err(|_| invalid())?;
                let telegram_update_id: i64 = update.parse().map_err(|_| invalid())?;
                if created_at < 0 || telegram_update_id < 0 {
                    return Err(invalid());
                }
                after = Some(InboxCursor {
                    created_at,
                    telegram_update_id,
                });
            }
            _ => return Err(invalid()),
        }
    }
    Ok((key, after, limit.unwrap_or_else(inbox_limit)))
}

/// Subscriptions identify the complete inbox, never a query page or URI pattern.
pub(super) fn subscription_key(uri: &str) -> Result<&str, ErrorData> {
    if uri.contains('?') {
        return Err(ErrorData::invalid_params("Invalid inbox URI", None));
    }
    parse_uri(uri).map(|(key, _, _)| key)
}

fn resource_error(error: AppError) -> ErrorData {
    match error {
        AppError::SessionNotFound => ErrorData::resource_not_found("Inbox unavailable", None),
        _ => ErrorData::internal_error("Inbox unavailable", Some(json!({"code":error.code()}))),
    }
}
