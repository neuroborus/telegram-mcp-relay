//! Minimal Bot API transport types; unknown fields are discarded, never persisted here.

use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Chat {
    pub id: i64,
}

#[derive(Deserialize)]
pub struct User {
    pub id: u64,
    pub is_bot: bool,
}

#[derive(Deserialize)]
pub struct SentMessage {
    pub message_id: i64,
    pub date: i64,
    pub chat: Chat,
}

/// Minimal target metadata for routing and service-loop suppression; never recurse.
#[derive(Deserialize)]
pub struct ReplyTarget {
    pub message_id: i64,
    pub from: Option<User>,
    pub text: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub struct Attachment {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<u64>,
}

#[derive(Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub date: i64,
    pub chat: Chat,
    pub from: Option<User>,
    pub reply_to_message: Option<ReplyTarget>,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub document: Option<Attachment>,
    #[serde(default)]
    pub photo: Vec<Attachment>,
}

#[derive(Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Deserialize)]
pub struct WebhookInfo {
    pub url: String,
    pub pending_update_count: u64,
}

// Diagnostic output must not expose message bodies, file IDs, webhook URLs or identities.
macro_rules! redacted_debug {
    ($($name:ty),+ $(,)?) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    )+};
}
redacted_debug!(
    Chat,
    User,
    SentMessage,
    ReplyTarget,
    Attachment,
    Message,
    Update,
    WebhookInfo
);
