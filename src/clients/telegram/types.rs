//! Minimal Bot API transport types; unknown fields are discarded, never persisted here.

use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Chat {
    pub id: i64,
    pub title: Option<String>,
}

#[derive(Deserialize)]
pub struct User {
    pub id: u64,
    pub is_bot: bool,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub username: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
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
    #[serde(default, deserialize_with = "photos")]
    pub photo: Vec<Attachment>,
}

fn photos<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Attachment>, D::Error> {
    let photos = Vec::<Attachment>::deserialize(deserializer)?;
    if photos.len() > crate::config::INCOMING_ATTACHMENT_LIMIT {
        return Err(serde::de::Error::custom("photo metadata capacity"));
    }
    Ok(photos)
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
