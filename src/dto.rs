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
