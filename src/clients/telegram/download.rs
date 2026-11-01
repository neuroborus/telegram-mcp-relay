//! Private cloud downloads into an already owned writer; no caller URLs or path opening.

use std::{net::IpAddr, time::Duration};

use reqwest::{StatusCode, Url, header};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::{REQUEST_TIMEOUT, TelegramClient};
use crate::{
    config::{CLOUD_DOWNLOAD_BYTES, DOWNLOAD_TIMEOUT_MAX},
    error::{AppError, TelegramError},
};

const MAX_DOWNLOAD_TIME: Duration = Duration::from_secs(DOWNLOAD_TIMEOUT_MAX);
const WRITE_CHUNK_BYTES: usize = 65_536;

#[derive(Deserialize)]
struct File {
    file_id: String,
    file_unique_id: String,
    file_size: Option<u64>,
    file_path: Option<String>,
}

/// Production is fixed to Telegram HTTPS. Only explicit synthetic constructors use HTTP.
pub(super) fn file_endpoint(endpoint: &str, https_only: bool) -> Result<Url, AppError> {
    let invalid = || AppError::InvalidConfig("Telegram client endpoint");
    let mut url = Url::parse(endpoint).map_err(|_| invalid())?;
    let origin_allowed = if https_only {
        url.scheme() == "https"
            && url.host_str() == Some("api.telegram.org")
            && url.port_or_known_default() == Some(443)
    } else {
        url.scheme() == "http"
            && url
                .host_str()
                .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback())
            && url.port_or_known_default().is_some_and(|port| port != 0)
    };
    let token = url
        .path()
        .strip_prefix("/bot")
        .and_then(|path| path.strip_suffix('/'))
        .ok_or_else(invalid)?;
    if !origin_allowed
        || url.as_str() != endpoint
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || token.is_empty()
        || token.len() > 4_096
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b":_-".contains(&byte))
    {
        return Err(invalid());
    }
    let path = format!("/file/bot{token}/");
    url.set_path(&path);
    Ok(url)
}

impl TelegramClient {
    /// Services must authorize retained provenance and reserve an owned output first.
    /// One deadline covers metadata retries, network reads, writer backpressure and flush.
    /// Cancellation drops HTTP work. The owner must retire its writer's outstanding I/O
    /// and remove partial output on any failure before releasing storage reservations.
    pub(crate) async fn download_file<W: AsyncWrite + Unpin>(
        &self,
        file_id: &str,
        file_unique_id: &str,
        limit_bytes: u64,
        deadline: Duration,
        output: &mut W,
    ) -> Result<u64, AppError> {
        if !valid_id(file_id, 4_096) || !valid_id(file_unique_id, 512) {
            return Err(AppError::InvalidRequest("retained attachment identity"));
        }
        if limit_bytes == 0 || deadline.is_zero() || deadline > MAX_DOWNLOAD_TIME {
            return Err(AppError::InvalidConfig("download bounds"));
        }
        let limit = limit_bytes.min(CLOUD_DOWNLOAD_BYTES);
        tokio::time::timeout(deadline, async {
            let file = self.get_file(file_id, file_unique_id, limit).await?;
            let path = file.file_path.ok_or(AppError::DownloadUnavailable)?;
            if !valid_path(&path) {
                return Err(TelegramError::InvalidResponse.into());
            }
            let url = self
                .file_endpoint
                .join(&path)
                .map_err(|_| TelegramError::InvalidResponse)?;
            if url.origin() != self.file_endpoint.origin()
                || !url.path().starts_with(self.file_endpoint.path())
            {
                return Err(TelegramError::InvalidResponse.into());
            }
            self.stream_file(url, limit, output).await
        })
        .await
        .unwrap_or(Err(TelegramError::Deadline.into()))
    }

    async fn get_file(
        &self,
        file_id: &str,
        file_unique_id: &str,
        limit: u64,
    ) -> Result<File, AppError> {
        let file: File = self
            .call(
                "getFile",
                &json!({"file_id": file_id}),
                true,
                REQUEST_TIMEOUT,
            )
            .await
            .map_err(|error| match error {
                TelegramError::Rejected {
                    code: 400 | 404 | 410,
                    ..
                } => AppError::DownloadUnavailable,
                error => error.into(),
            })?;
        // Telegram can return a different valid file_id for the same stable file identity.
        if !valid_id(&file.file_id, 4_096) || file.file_unique_id != file_unique_id {
            return Err(TelegramError::InvalidResponse.into());
        }
        if let Some(size) = file.file_size {
            check_size(size, limit)?;
        }
        Ok(file)
    }

    async fn stream_file<W: AsyncWrite + Unpin>(
        &self,
        url: Url,
        limit: u64,
        output: &mut W,
    ) -> Result<u64, AppError> {
        let mut response = self
            .http
            .get(url)
            .header(header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|_| TelegramError::Transport)?;
        match response.status() {
            StatusCode::OK => (),
            StatusCode::NOT_FOUND | StatusCode::GONE => return Err(AppError::DownloadUnavailable),
            status if status.is_client_error() || status.is_server_error() => {
                return Err(TelegramError::Rejected {
                    code: status.as_u16(),
                    retry_after_seconds: None,
                }
                .into());
            }
            _ => return Err(TelegramError::InvalidResponse.into()),
        }
        if response
            .headers()
            .get_all(header::CONTENT_ENCODING)
            .iter()
            .any(|value| value.as_bytes() != b"identity")
        {
            return Err(TelegramError::InvalidResponse.into());
        }
        if let Some(size) = response.content_length() {
            check_size(size, limit)?;
        }
        let mut total = 0_u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| TelegramError::Transport)?
        {
            // Neither the API size nor HTTP headers authorize bytes beyond the hard ceiling.
            total = total.saturating_add(chunk.len() as u64);
            check_size(total, limit)?;
            for part in chunk.chunks(WRITE_CHUNK_BYTES) {
                output
                    .write_all(part)
                    .await
                    .map_err(|_| AppError::DownloadWriteFailed)?;
            }
        }
        output
            .flush()
            .await
            .map_err(|_| AppError::DownloadWriteFailed)?;
        Ok(total)
    }
}

fn valid_id(value: &str, max: usize) -> bool {
    // IDs are opaque JSON values, never URL components or filesystem authority.
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn valid_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4_096
        && value.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
}

fn check_size(size: u64, limit: u64) -> Result<(), AppError> {
    if size > limit {
        return Err(AppError::FileTooLarge {
            limit_bytes: limit,
            actual_bytes: size,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
