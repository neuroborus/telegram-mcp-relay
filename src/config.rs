//! Central TOML settings, validation and protected startup secret resolution.

use std::{
    fmt,
    fs::File,
    io::Read,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

use crate::error::AppError;

mod secrets;
pub use secrets::{Secret, Secrets};

/// Fixed final-text policy; this is deliberately not a TOML setting.
pub const OUTBOUND_SCALAR_LIMIT: usize = 1_000;
// Internal MCP transport bounds, separate from durable application sessions.
pub(crate) const MCP_SESSION_IDLE: std::time::Duration = std::time::Duration::from_secs(300);
pub(crate) const MCP_SESSION_INIT: std::time::Duration = std::time::Duration::from_secs(10);
pub(crate) const MCP_SESSION_ABANDONED: std::time::Duration = std::time::Duration::from_secs(30);
pub(crate) const MCP_SESSION_SWEEP: std::time::Duration = std::time::Duration::from_secs(1);
pub(crate) const MCP_SESSION_QUEUE: usize = 16;
const CONFIG_BYTES: usize = 65_536;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub server: Server,
    pub telegram: Telegram,
    pub delivery: Delivery,
    pub activity: Activity,
    pub storage: Storage,
    pub notifications: Notifications,
    pub files: Files,
    pub limits: Limits,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Config([REDACTED])")
    }
}

// Keep each setting's type and default together; every section rejects unknown keys.
macro_rules! settings {
    ($name:ident { $($field:ident: $kind:ty = $default:expr),* $(,)? }) => {
        #[derive(Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $name { $(pub $field: $kind,)* }
        impl Default for $name {
            fn default() -> Self { Self { $($field: $default,)* } }
        }
    };
}

settings!(Server {
    listen: SocketAddr = SocketAddr::from(([127, 0, 0, 1], 8765)),
    auth_token_env: Option<String> = None,
    auth_token_file: Option<PathBuf> = None,
    create_auth_token_file: bool = false,
});
settings!(Telegram {
    bot_token_env: Option<String> = None,
    bot_token_file: Option<PathBuf> = None,
    chat_id: i64 = 0,
    allowed_chat_ids: Vec<i64> = Vec::new(),
    allowed_user_ids: Vec<u64> = Vec::new(),
    long_poll_timeout_seconds: u64 = 45,
});
settings!(Delivery {
    wait_fallback_enabled: bool = true,
    wait_timeout_seconds: u64 = 600,
    processing_start_grace_seconds: u64 = 60,
    claim_lease_seconds: u64 = 60,
});
settings!(Activity {
    typing_enabled: bool = true,
    typing_refresh_seconds: u64 = 4,
    typing_max_duration_seconds: u64 = 600,
});
settings!(Storage {
    path: PathBuf = "~/.local/state/telegram-mcp-relay/relay.sqlite".into(),
    session_ttl_days: u64 = 30,
    route_ttl_days: u64 = 30,
    reply_ttl_days: u64 = 30,
    cleanup_interval_hours: u64 = 24,
    max_database_bytes: u32 = 67_108_864,
});
settings!(Notifications {
    notify_unconfirmed_processing: bool = true,
    notify_unroutable: bool = true,
});
settings!(Files {
    allowed_roots: Vec<PathBuf> = Vec::new(),
    max_outbound_mb: u32 = 50,
});
settings!(Limits {
    request_bytes: u32 = 65_536,
    telegram_response_bytes: u32 = 2_097_152,
    concurrent_requests: u32 = 32,
    background_tasks: u32 = 64,
    waiters: u32 = 16,
    subscriptions: u32 = 64,
    subscription_queue: u32 = 16,
    inbox_items: u32 = 10_000,
    reply_bytes: u32 = 65_536,
    inbox_bytes: u32 = 16_777_216,
    metadata_bytes: u32 = 16_777_216,
    sessions: u32 = 1_024,
    routes: u32 = 100_000,
    receipts: u32 = 10_000,
    upload_buffer_bytes: u32 = 65_536,
});

// All scalar budgets are positive and finite; field names remain static in errors.
macro_rules! bounds {
    ($config:ident.$section:ident, $($field:ident: $max:expr),+ $(,)?) => {
        $(require($config.$section.$field > 0 && $config.$section.$field <= $max,
            concat!(stringify!($section), ".", stringify!($field)))?;)+
    };
}

impl Config {
    /// Read a bounded TOML file. HOME is used only for an initial `~/` component.
    pub fn load(path: &Path) -> Result<Self, AppError> {
        let mut input = String::new();
        let file = File::open(path).map_err(|_| AppError::ConfigRead)?;
        file.take(CONFIG_BYTES as u64 + 1)
            .read_to_string(&mut input)
            .map_err(|_| AppError::ConfigRead)?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::parse(&input, home.as_deref())
    }

    /// Parse and validate without filesystem mutations, environment changes or network I/O.
    pub fn parse(input: &str, home: Option<&Path>) -> Result<Self, AppError> {
        require(input.len() <= CONFIG_BYTES, "document size")?;
        let mut config: Self =
            toml::from_str(input).map_err(|_| AppError::InvalidConfig("TOML document"))?;
        config.validate(home)?;
        Ok(config)
    }

    fn validate(&mut self, home: Option<&Path>) -> Result<(), AppError> {
        require(
            self.server.listen.ip().is_loopback() && self.server.listen.port() != 0,
            "server.listen",
        )?;
        validate_source(
            &self.server.auth_token_env,
            &self.server.auth_token_file,
            "server.auth_token",
        )?;
        validate_source(
            &self.telegram.bot_token_env,
            &self.telegram.bot_token_file,
            "telegram.bot_token",
        )?;
        require(
            !self.server.create_auth_token_file || self.server.auth_token_file.is_some(),
            "server.create_auth_token_file",
        )?;
        let t = &self.telegram;
        const MAX_ID: u64 = (1 << 52) - 1;
        require(
            t.chat_id != 0
                && t.chat_id.unsigned_abs() <= MAX_ID
                && t.allowed_chat_ids.contains(&t.chat_id),
            "telegram.chat_id",
        )?;
        require(
            !t.allowed_chat_ids.is_empty()
                && t.allowed_chat_ids.len() <= 256
                && t.allowed_chat_ids
                    .iter()
                    .all(|id| *id != 0 && id.unsigned_abs() <= MAX_ID),
            "telegram.allowed_chat_ids",
        )?;
        require(
            !t.allowed_user_ids.is_empty()
                && t.allowed_user_ids.len() <= 256
                && t.allowed_user_ids.iter().all(|id| *id > 0 && *id <= MAX_ID),
            "telegram.allowed_user_ids",
        )?;
        bounds!(self.telegram, long_poll_timeout_seconds: 50);
        bounds!(self.delivery, wait_timeout_seconds: 86_400,
            processing_start_grace_seconds: 86_400, claim_lease_seconds: 600);
        bounds!(self.activity, typing_refresh_seconds: 5, typing_max_duration_seconds: 600);
        bounds!(self.storage, session_ttl_days: 3_650, route_ttl_days: 3_650,
            reply_ttl_days: 3_650, cleanup_interval_hours: 168);
        bounds!(self.files, max_outbound_mb: 50);
        require(
            self.activity.typing_refresh_seconds <= self.activity.typing_max_duration_seconds,
            "activity.typing_refresh_seconds",
        )?;
        let l = &self.limits;
        bounds!(self.limits, request_bytes: 1_048_576, telegram_response_bytes: 16_777_216,
            concurrent_requests: 1_024, background_tasks: 1_024, waiters: l.concurrent_requests,
            subscriptions: 1_024, subscription_queue: 1_024, inbox_items: 1_000_000,
            reply_bytes: l.telegram_response_bytes.min(l.inbox_bytes), inbox_bytes: 1_073_741_824,
            metadata_bytes: 1_073_741_824, sessions: 100_000, routes: 1_000_000,
            receipts: 1_000_000, upload_buffer_bytes: 65_536);
        require(
            (1_048_576..=1_073_741_824).contains(&self.storage.max_database_bytes)
                && u64::from(l.inbox_bytes) + u64::from(l.metadata_bytes)
                    < u64::from(self.storage.max_database_bytes),
            "storage.max_database_bytes",
        )?;
        self.storage.path = expand_path(&self.storage.path, home)?;
        require(self.storage.path.file_name().is_some(), "storage.path")?;
        for path in [
            &mut self.server.auth_token_file,
            &mut self.telegram.bot_token_file,
        ]
        .into_iter()
        .flatten()
        {
            *path = expand_path(path, home)?;
            require(path.file_name().is_some(), "secret file path")?;
        }
        require(
            !self.files.allowed_roots.is_empty() && self.files.allowed_roots.len() <= 64,
            "files.allowed_roots",
        )?;
        for path in &mut self.files.allowed_roots {
            *path = expand_path(path, home)?;
            require(path.parent().is_some(), "files.allowed_roots")?;
        }
        Ok(())
    }
}

fn require(valid: bool, field: &'static str) -> Result<(), AppError> {
    if valid {
        Ok(())
    } else {
        Err(AppError::InvalidConfig(field))
    }
}

fn validate_source(
    env: &Option<String>,
    file: &Option<PathBuf>,
    field: &'static str,
) -> Result<(), AppError> {
    require(env.is_none() || file.is_none(), field)?;
    if let Some(name) = env {
        let valid = name
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()));
        require(!name.is_empty() && valid, field)?;
    }
    Ok(())
}

fn expand_path(path: &Path, home: Option<&Path>) -> Result<PathBuf, AppError> {
    let expanded = if path.starts_with("~") {
        home.ok_or(AppError::InvalidConfig("HOME"))?.join(
            path.strip_prefix("~")
                .map_err(|_| AppError::InvalidConfig("path"))?,
        )
    } else {
        path.to_path_buf()
    };
    require(
        expanded.is_absolute()
            && !expanded
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            && !expanded.as_os_str().as_encoded_bytes().contains(&0),
        "path",
    )?;
    Ok(expanded)
}

#[cfg(test)]
mod tests;
