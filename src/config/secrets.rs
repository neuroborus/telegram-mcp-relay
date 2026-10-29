//! Descriptor-checked Unix secret files and explicitly exposed, redacted secret values.

use std::{
    fmt::{self, Write as _},
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::Path,
};

use rustix::{
    fs::{Mode, OFlags, open, openat},
    process::geteuid,
};

use super::Config;
use crate::error::AppError;

/// Sensitive text has no Display/Serialize implementation; Debug is always redacted.
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug)]
pub struct Secrets {
    pub bot_token: Secret,
    pub auth_token: Option<Secret>,
}

impl Config {
    /// Resolve only after settings validation. Callers must not log exposed values.
    pub fn load_secrets(&self) -> Result<Secrets, AppError> {
        self.resolve_secrets(|name| std::env::var(name).ok())
    }

    // Inject environment access so tests never mutate global process environment.
    pub(super) fn resolve_secrets(
        &self,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Secrets, AppError> {
        let bot = &self.telegram;
        let bot_token = read_source(
            bot.bot_token_env.as_deref(),
            bot.bot_token_file.as_deref(),
            false,
            &env,
        )
        .ok_or(AppError::SecretUnavailable("telegram.bot_token"))?;
        let valid_bot = bot_token.0.split_once(':').is_some_and(|(id, key)| {
            !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_digit())
                && !key.is_empty()
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        });
        if !valid_bot {
            return Err(AppError::SecretUnavailable("telegram.bot_token"));
        }
        let server = &self.server;
        let auth_token = if server.auth_token_env.is_some() || server.auth_token_file.is_some() {
            let token = read_source(
                server.auth_token_env.as_deref(),
                server.auth_token_file.as_deref(),
                server.create_auth_token_file,
                &env,
            )
            .ok_or(AppError::SecretUnavailable("server.auth_token"))?;
            if token.0.len() < 32
                || !token
                    .0
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
            {
                return Err(AppError::SecretUnavailable("server.auth_token"));
            }
            Some(token)
        } else {
            None
        };
        Ok(Secrets {
            bot_token,
            auth_token,
        })
    }
}

fn read_source(
    env_name: Option<&str>,
    path: Option<&Path>,
    create: bool,
    env: &impl Fn(&str) -> Option<String>,
) -> Option<Secret> {
    let value = if let Some(path) = path {
        read_file(path, create)?
    } else {
        env(env_name.unwrap_or("TELEGRAM_BOT_TOKEN"))?
    };
    // Permit a conventional trailing line ending, never whitespace inside a credential.
    let value = value.trim_end_matches(['\r', '\n']);
    (!value.is_empty() && value.len() <= 4_096).then(|| Secret(value.to_owned()))
}

fn read_file(path: &Path, create: bool) -> Option<String> {
    let parent = File::from(
        open(
            path.parent()?,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?,
    );
    let name = path.file_name()?;
    if create {
        let metadata = parent.metadata().ok()?;
        if metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return None;
        }
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).ok()?;
        let mut token = String::with_capacity(64);
        for byte in bytes {
            write!(token, "{byte:02x}").ok()?;
        }
        match openat(
            &parent,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => {
                let mut file = File::from(fd);
                file.write_all(token.as_bytes()).ok()?;
                file.sync_all().ok()?;
            }
            Err(rustix::io::Errno::EXIST) => (),
            Err(_) => return None,
        }
    }
    let file = File::from(
        openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?,
    );
    let metadata = file.metadata().ok()?;
    if !metadata.is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.mode() & 0o400 == 0
        || metadata.nlink() != 1
        || metadata.len() > 4_098
    {
        return None;
    }
    let mut value = String::new();
    file.take(4_099).read_to_string(&mut value).ok()?;
    (value.len() <= 4_098).then_some(value)
}
