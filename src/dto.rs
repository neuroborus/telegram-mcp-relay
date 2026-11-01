//! Typed MCP tool arguments/results. Sensitive arguments deliberately have no Debug impl.

use std::path::PathBuf;

use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::domain::outbound::MessageEntity;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct GuidanceRequest {}

#[derive(Clone, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GuidanceResult {
    /// Version of this instruction/result contract, independent of the MCP revision.
    pub version: u32,
    pub tools: Vec<GuidanceTool>,
    pub capabilities: GuidanceCapabilities,
    pub limits: GuidanceLimits,
    pub instructions: Vec<String>,
}

#[derive(Clone, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GuidanceTool {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GuidanceProtocol {
    pub version: String,
    pub resource_subscriptions: bool,
}

#[derive(Clone, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GuidanceCapabilities {
    pub protocols: Vec<GuidanceProtocol>,
    pub wait_enabled: bool,
    pub history_enabled: bool,
    pub attachment_downloads_enabled: bool,
    pub typing_enabled: bool,
    pub guidance_handshake_required: bool,
    pub host_concurrency_known: bool,
    pub idle_host_wake_guaranteed: bool,
}

/// Nonsecret effective ceilings/budgets, including configured values for disabled features.
#[derive(Clone, Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GuidanceLimits {
    pub title_graphemes: usize,
    pub directory_scalars: usize,
    pub text_scalars: usize,
    pub text_utf16_units: usize,
    pub caption_utf16_units: usize,
    pub request_bytes: u32,
    pub concurrent_requests: u32,
    pub control_requests: usize,
    pub background_sends: u32,
    pub waiters: u32,
    pub subscriptions: u32,
    pub subscription_queue: u32,
    pub wait_timeout_seconds: u64,
    pub text_send_network_seconds: u64,
    pub upload_network_seconds: u64,
    pub claim_lease_seconds: u64,
    pub typing_max_seconds: u64,
    pub transport_idle_seconds: u64,
    pub transport_abandon_seconds: u64,
    pub upload_bytes: u64,
    pub download_bytes: u64,
    pub download_concurrency: u32,
    pub download_total_bytes: u64,
    pub download_files: u32,
    pub download_timeout_seconds: u64,
    pub download_retention_seconds: u64,
    pub reply_retention_days: u64,
    pub route_retention_days: u64,
    pub history_retention_days: u64,
    pub history_page_limit: u32,
    pub history_response_bytes: u32,
}

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
    /// Opt-in directory display context, not file authority. Omitted/blank clears it.
    /// Normalized to one safe line, at most 100 Unicode scalars; avoid personal paths.
    pub directory_label: Option<String>,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RegisterResult {
    pub session_key: String,
    pub title: String,
    pub directory_label: Option<String>,
    /// Stable session inbox address.
    pub inbox_uri: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendRequest {
    pub session_key: String,
    /// Verbatim body; use entities for formatting, never implicit Markdown/HTML parsing.
    /// The server adds native bold/italic headers. The complete visible
    /// title, optional directory, separators and body must fit 1,000 Unicode scalars.
    pub text: String,
    /// Optional native entities, at most 100; UTF-16 offsets relative to text only.
    /// Omitted/empty means literal text. Do not include the server-owned header.
    #[serde(default)]
    #[schemars(length(max = 100))]
    pub entities: Vec<MessageEntity>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendFileRequest {
    pub session_key: String,
    /// Absolute local path beneath a configured approved root, without symlinks.
    pub path: PathBuf,
    /// Verbatim description; use caption_entities for formatting. The server adds the header.
    /// The complete caption must fit
    /// 1,000 scalars and 1,024 UTF-16 units.
    #[serde(default)]
    pub caption: String,
    /// Optional native entities, at most 100; UTF-16 offsets relative to caption only.
    /// Omitted/empty means literal text; no Markdown/HTML parsing or header reproduction.
    #[serde(default)]
    #[schemars(length(max = 100))]
    pub caption_entities: Vec<MessageEntity>,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendResult {
    pub telegram_chat_id: i64,
    /// Success confirms both Telegram acceptance and a committed reply route.
    pub outbound_message_id: i64,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum AskStatus {
    Sent,
    ReplyReceived,
    WaitTimeout,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AskResult {
    pub telegram_chat_id: i64,
    pub outbound_message_id: i64,
    /// Sent means waiting is disabled. Timeout grants no permission; later replies remain valid.
    pub status: AskStatus,
    /// A claim is not processing or completion. Preserve its reply ID and current token.
    pub reply: Option<ClaimedReplyResult>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct WaitRequest {
    pub session_key: String,
    /// Saved successful text/document send in the configured chat. Never resend to wait.
    pub outbound_message_id: i64,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(crate = "rmcp::schemars")]
pub enum WaitStatus {
    Disabled,
    ReplyReceived,
    WaitTimeout,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct WaitResult {
    pub telegram_chat_id: i64,
    pub outbound_message_id: i64,
    pub status: WaitStatus,
    /// Disabled waiting acquires no claim. Returned claims require processing/ack separately.
    pub reply: Option<ClaimedReplyResult>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct HistoryRequest {
    pub session_key: String,
    /// Defaults to the configured chat; current allowlists apply to every page.
    pub chat_id: Option<i64>,
    /// Public display identity, never another session's private key.
    pub session_display_id: Option<String>,
    /// Literal case-sensitive substring in retained bodies or display labels.
    pub query: Option<String>,
    /// Opaque ordering bookmark; repeat the same chat/display/query filters.
    pub cursor: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct HistoryAttachment {
    pub attachment_handle: String,
    pub kind: String,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub expires_at: i64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetAttachmentRequest {
    pub session_key: String,
    /// Opaque retained own-reply or authorized history handle; never a Telegram ID or URL.
    pub attachment_handle: String,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct LocalFileIdentity {
    /// Linux device/inode identities encoded as hexadecimal strings, not Telegram IDs.
    pub device: String,
    pub inode: String,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetAttachmentResult {
    pub attachment: HistoryAttachment,
    pub download_id: String,
    /// Confined daemon-local file. Remote clients do not share this filesystem.
    pub path: PathBuf,
    pub file_identity: LocalFileIdentity,
    pub size_bytes: u64,
    /// File expiry, which can precede the retained attachment handle's expiry.
    pub expires_at: i64,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct HistoryMessage {
    pub message_handle: String,
    pub chat_id: i64,
    pub message_id: i64,
    pub direction: String,
    pub sender_user_id: Option<u64>,
    pub chat_label: Option<String>,
    pub sender_label: Option<String>,
    pub session_display_id: Option<String>,
    pub session_title: Option<String>,
    pub directory_label: Option<String>,
    pub text_or_caption: Option<String>,
    /// For service entries this identifies the triggering message, not a Telegram Reply.
    pub related_message_id: Option<i64>,
    pub message_date: i64,
    pub observed_at: i64,
    pub expires_at: i64,
    pub attachments: Vec<HistoryAttachment>,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct HistoryCoverage {
    pub local_only: bool,
    /// Per-chat/user observation start is not durably tracked; null means unknown.
    pub observation_start: Option<i64>,
    pub retention_days: u64,
    pub possible_gaps: bool,
    /// Daemon-level recorded gaps; false never guarantees complete chat coverage.
    pub known_gaps: bool,
}

#[derive(Serialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct HistoryResult {
    pub messages: Vec<HistoryMessage>,
    pub next_cursor: Option<String>,
    /// A bounded scan stopped; an empty page does not imply no later matches.
    pub scan_incomplete: bool,
    pub coverage: HistoryCoverage,
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
    /// Safe retained handles. Insufficient legacy provenance yields no downloadable handles.
    pub attachments: Vec<HistoryAttachment>,
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
