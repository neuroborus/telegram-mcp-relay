//! Typed MCP tool arguments/results. Sensitive arguments deliberately have no Debug impl.

use std::path::PathBuf;

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct RegisterRequest {
    /// Stable conversation identity; preserve it across connections and restarts.
    pub external_session_id: Option<String>,
    /// Saved handle to resume when no external identity is available.
    pub session_key: Option<String>,
    /// Display label, normalized to at most 50 grapheme clusters.
    pub title: String,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RegisterResult {
    pub session_key: String,
    pub title: String,
    /// Stable session inbox address.
    pub inbox_uri: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendRequest {
    pub session_key: String,
    /// Plain text; the final title, separator and text must fit 1,000 Unicode scalars.
    pub text: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendFileRequest {
    pub session_key: String,
    /// Absolute local path beneath a configured approved root, without symlinks.
    pub path: PathBuf,
    /// Optional description; the final caption must fit 1,000 scalars and 1,024 UTF-16 units.
    #[serde(default)]
    pub caption: String,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendResult {
    pub telegram_chat_id: i64,
    /// Success confirms both Telegram acceptance and a committed reply route.
    pub outbound_message_id: i64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct InboxRequest {
    pub session_key: String,
    /// Claim 1–100 eligible replies, within the configured payload budget. Default: 100.
    #[serde(default = "inbox_limit")]
    pub limit: u32,
}

pub(crate) fn inbox_limit() -> u32 {
    100
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum DeliveryState {
    Pending,
    Claimed,
    Processing,
}

/// Resource data contains no claim token. Timestamps are Unix milliseconds.
#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Reply {
    pub reply_id: String,
    pub telegram_update_id: i64,
    pub telegram_chat_id: i64,
    pub telegram_message_id: i64,
    pub reply_to_bot_message_id: i64,
    pub text_or_caption: Option<String>,
    pub attachment_metadata: Option<serde_json::Value>,
    pub state: DeliveryState,
    pub claimed_at: Option<i64>,
    pub claim_expires_at: Option<i64>,
    pub processing_started_at: Option<i64>,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ClaimedReplyResult {
    #[serde(flatten)]
    pub reply: Reply,
    /// Preserve this current fence for processing and completion.
    pub claim_token: String,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct InboxResult {
    pub replies: Vec<ClaimedReplyResult>,
}

#[derive(Serialize)]
pub struct InboxSnapshot {
    pub replies: Vec<Reply>,
    /// Follow this URI for the next read-only page; null means the current end.
    pub next_uri: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProcessingRequest {
    pub session_key: String,
    pub reply_id: String,
    pub claim_token: String,
    /// Supply either active or reconcile_request_id, never both. False preserves ownership.
    pub active: Option<bool>,
    /// Explicit resume: save a fresh random ID (1–64 UTF-8 bytes) and current token before
    /// calling. Retry with the same pair after response loss; preserve the returned token.
    pub reconcile_request_id: Option<String>,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProcessingResult {
    pub reply_id: String,
    pub claim_token: String,
    /// Original durable first start in Unix milliseconds; inactivity never resets it.
    pub processing_started_at: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct AckRequest {
    pub session_key: String,
    pub reply_id: String,
    pub claim_token: String,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AckResult {
    pub reply_id: String,
    pub acknowledged: bool,
}
