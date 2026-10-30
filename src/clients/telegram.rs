//! One reusable HTTPS Bot API client, bounded decoding and conservative send uncertainty.

use std::{fmt, time::Duration};

use reqwest::{Client, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;

use crate::{
    config::{Config, Secret},
    domain::outbound::format_notice,
    error::{AppError, TelegramError},
};

mod types;
mod upload;
pub use types::*;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const ATTEMPTS: usize = 3;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct TelegramClient {
    http: Client,
    endpoint: String,
    response_bytes: usize,
    poll_seconds: u64,
    #[cfg(test)]
    retry_delays: Option<std::sync::Arc<std::sync::Mutex<Vec<Duration>>>>,
}

impl fmt::Debug for TelegramClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TelegramClient([REDACTED])")
    }
}

#[derive(Deserialize)]
struct Envelope<T> {
    ok: bool,
    result: Option<T>,
    error_code: Option<u16>,
    parameters: Option<Parameters>,
}

#[derive(Deserialize)]
struct Parameters {
    retry_after: Option<u64>,
}

impl TelegramClient {
    #[cfg(test)]
    pub(crate) fn for_test(config: &Config, endpoint: String) -> Result<Self, AppError> {
        Self::build(config, endpoint, false)
    }

    /// Uses only Telegram's cloud endpoint. Mock URLs exist only in crate tests.
    pub fn new(config: &Config, token: &Secret) -> Result<Self, AppError> {
        Self::build(
            config,
            format!("https://api.telegram.org/bot{}/", token.expose()),
            true,
        )
    }

    fn build(config: &Config, endpoint: String, https_only: bool) -> Result<Self, AppError> {
        if !(1..=16_777_216).contains(&config.limits.telegram_response_bytes)
            || !(1..=50).contains(&config.telegram.long_poll_timeout_seconds)
        {
            return Err(AppError::InvalidConfig("Telegram client bounds"));
        }
        let http = Client::builder()
            .https_only(https_only)
            .no_proxy()
            .redirect(Policy::none())
            .referer(false)
            // Disable transport-level retries too: outgoing sends have no idempotency key.
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| TelegramError::Transport)?;
        Ok(Self {
            http,
            endpoint,
            response_bytes: config.limits.telegram_response_bytes as usize,
            poll_seconds: config.telegram.long_poll_timeout_seconds,
            #[cfg(test)]
            retry_delays: None,
        })
    }

    /// One bounded batch, without changing any durable cursor. No negative-offset dropping.
    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        limit: u8,
    ) -> Result<Vec<Update>, AppError> {
        if offset.is_some_and(|offset| offset < 0) || !(1..=100).contains(&limit) {
            return Err(AppError::InvalidRequest("Telegram update batch"));
        }
        let mut payload = json!({"limit": limit, "timeout": self.poll_seconds,
            "allowed_updates": ["message"]});
        if let Some(offset) = offset {
            payload["offset"] = offset.into();
        }
        let updates: Vec<Update> = self
            .call(
                "getUpdates",
                &payload,
                true,
                Duration::from_secs(self.poll_seconds + 10),
            )
            .await?;
        if updates.len() > usize::from(limit) {
            return Err(TelegramError::InvalidResponse.into());
        }
        Ok(updates)
    }

    /// Inspect conflicts without deleting webhooks or pending updates.
    pub async fn get_webhook_info(&self) -> Result<WebhookInfo, AppError> {
        Ok(self
            .call("getWebhookInfo", &json!({}), true, REQUEST_TIMEOUT)
            .await?)
    }

    /// Accepts final plain text. Services add titles, authorize and reserve routes first.
    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<SentMessage, AppError> {
        let text = format_notice(None, text)?;
        let sent: SentMessage = self
            .call(
                "sendMessage",
                &json!({"chat_id": chat_id, "text": text}),
                false,
                REQUEST_TIMEOUT,
            )
            .await?;
        validate_sent(&sent, chat_id)?;
        Ok(sent)
    }

    pub async fn send_chat_action(&self, chat_id: i64) -> Result<(), AppError> {
        // The shared typing scheduler owns refreshes, cancellation and rate-limit cooldowns.
        // Internal retries could outlive a lease or send twice within one refresh interval.
        let sent: bool = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.request(
                "sendChatAction",
                &json!({"chat_id": chat_id, "action": "typing"}),
            ),
        )
        .await
        .unwrap_or(Err(TelegramError::Deadline))?;
        if !sent {
            return Err(TelegramError::InvalidResponse.into());
        }
        Ok(())
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        payload: &impl Serialize,
        safe_to_retry: bool,
        deadline: Duration,
    ) -> Result<T, TelegramError> {
        let mut attempt = 0;
        loop {
            let result = tokio::time::timeout(deadline, self.request(method, payload))
                .await
                .unwrap_or(Err(TelegramError::Deadline));
            let error = match result {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            if !safe_to_retry {
                return Err(send_error(error));
            }
            let Some(delay) = retry_delay(&error, attempt, jitter()) else {
                return Err(error);
            };
            tracing::debug!(attempt = attempt + 1, "Retrying Telegram read");
            self.pause_retry(delay).await;
            attempt += 1;
        }
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        payload: &impl Serialize,
    ) -> Result<T, TelegramError> {
        self.execute(
            self.http
                .post(format!("{}{method}", self.endpoint))
                .json(payload),
        )
        .await
    }

    async fn execute<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, TelegramError> {
        let mut response = request.send().await.map_err(|_| TelegramError::Transport)?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|size| size > self.response_bytes as u64)
        {
            return Err(TelegramError::ResponseTooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| TelegramError::Transport)?
        {
            if chunk.len() > self.response_bytes - body.len() {
                return Err(TelegramError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        decode(status, &body)
    }

    async fn pause_retry(&self, delay: Duration) {
        #[cfg(test)]
        if let Some(delays) = &self.retry_delays {
            delays.lock().unwrap().push(delay);
            return;
        }
        tokio::time::sleep(delay).await;
    }
}

fn validate_sent(sent: &SentMessage, chat_id: i64) -> Result<(), TelegramError> {
    if sent.message_id <= 0 || sent.date < 0 || sent.chat.id != chat_id {
        return Err(TelegramError::DeliveryUncertain);
    }
    Ok(())
}

fn send_error(error: TelegramError) -> TelegramError {
    match error {
        TelegramError::Rejected {
            code: 400..=499, ..
        } => error,
        _ => TelegramError::DeliveryUncertain,
    }
}

fn decode<T: DeserializeOwned>(status: StatusCode, body: &[u8]) -> Result<T, TelegramError> {
    // Descriptions, parser errors and unknown fields may contain credentials or user data.
    let envelope: Envelope<T> = serde_json::from_slice(body).map_err(|_| {
        if status.is_server_error() {
            TelegramError::Rejected {
                code: status.as_u16(),
                retry_after_seconds: None,
            }
        } else {
            TelegramError::InvalidResponse
        }
    })?;
    if envelope.ok && status.is_success() && envelope.error_code.is_none() {
        return envelope.result.ok_or(TelegramError::InvalidResponse);
    }
    if !envelope.ok
        && envelope.result.is_none()
        && let Some(code @ 400..=599) = envelope.error_code
        && (status.is_success() || status.as_u16() == code)
    {
        return Err(TelegramError::Rejected {
            code,
            retry_after_seconds: envelope.parameters.and_then(|p| p.retry_after),
        });
    }
    Err(TelegramError::InvalidResponse)
}

fn retry_delay(error: &TelegramError, attempt: usize, jitter: u32) -> Option<Duration> {
    if attempt + 1 >= ATTEMPTS {
        return None;
    }
    let retry_after = match error {
        TelegramError::Transport | TelegramError::Deadline => 0,
        TelegramError::Rejected {
            code: 429 | 500..=599,
            retry_after_seconds,
        } => retry_after_seconds.unwrap_or(0),
        _ => return None,
    };
    let base_ms = 250 << attempt;
    let delay = Duration::from_millis(base_ms + u64::from(jitter) % base_ms)
        .max(Duration::from_secs(retry_after));
    // Never shorten a server delay to fit the retry budget; return it to the caller instead.
    (delay <= MAX_RETRY_DELAY).then_some(delay)
}

fn jitter() -> u32 {
    let mut bytes = [0; 4];
    // A failed entropy source only removes jitter; it cannot expand the retry budget.
    let _ = getrandom::fill(&mut bytes);
    u32::from_ne_bytes(bytes)
}

#[cfg(test)]
mod tests;
