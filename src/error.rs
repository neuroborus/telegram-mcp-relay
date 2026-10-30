//! Safe application errors: never retain parser input, secret values or filesystem paths.

use std::fmt;

/// Safe Bot API failures. Never retain reqwest errors, URLs or Telegram descriptions.
#[derive(Debug, PartialEq, Eq)]
pub enum TelegramError {
    Transport,
    Deadline,
    ResponseTooLarge,
    InvalidResponse,
    Rejected {
        code: u16,
        retry_after_seconds: Option<u64>,
    },
    DeliveryUncertain,
}

impl TelegramError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Transport => "TELEGRAM_UNAVAILABLE",
            Self::Deadline => "TELEGRAM_TIMEOUT",
            Self::ResponseTooLarge => "TELEGRAM_RESPONSE_TOO_LARGE",
            Self::InvalidResponse => "TELEGRAM_RESPONSE_INVALID",
            Self::Rejected { .. } => "TELEGRAM_REJECTED",
            Self::DeliveryUncertain => "TELEGRAM_DELIVERY_UNCERTAIN",
        }
    }
}

impl fmt::Display for TelegramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeliveryUncertain => write!(
                f,
                "{}: Telegram may have accepted the send; do not resend automatically",
                self.code()
            ),
            _ => f.write_str(self.code()),
        }
    }
}

impl std::error::Error for TelegramError {}

/// Outbound rejection details suitable for structured tool results; contains no input text.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct TextTooLong {
    pub limit: usize,
    pub actual_characters: usize,
    pub native_limit_utf16: usize,
    pub actual_utf16_units: usize,
    pub guidance: &'static str,
}

/// Application failures with safe details for diagnostics and future tool results.
#[derive(Debug, PartialEq, Eq)]
pub enum AppError {
    ConfigRead,
    InvalidConfig(&'static str),
    SecretUnavailable(&'static str),
    EmptyText,
    TextTooLong(TextTooLong),
    InvalidRequest(&'static str),
    FileUnavailable,
    FileTooLarge { limit_bytes: u64, actual_bytes: u64 },
    SessionNotFound,
    SessionIdentityConflict,
    InvalidClaim,
    RouteConflict,
    RoutePending,
    PollerActive,
    SendCapacity,
    SendBookkeepingFailed,
    RandomUnavailable,
    StorageUnavailable,
    StorageFailure,
    StorageCapacity,
    UnsupportedSchema(i64),
    Telegram(TelegramError),
}

impl AppError {
    /// Stable machine-readable classification, independent of the underlying private cause.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Telegram(error) => error.code(),
            Self::ConfigRead => "CONFIG_READ_FAILED",
            Self::InvalidConfig(_) => "CONFIG_INVALID",
            Self::SecretUnavailable(_) => "SECRET_UNAVAILABLE",
            Self::EmptyText => "TEXT_EMPTY",
            Self::TextTooLong(_) => "TEXT_TOO_LONG",
            Self::InvalidRequest(_) => "REQUEST_INVALID",
            Self::FileUnavailable => "FILE_UNAVAILABLE",
            Self::FileTooLarge { .. } => "FILE_TOO_LARGE",
            Self::SessionNotFound => "SESSION_NOT_FOUND",
            Self::SessionIdentityConflict => "SESSION_IDENTITY_CONFLICT",
            Self::InvalidClaim => "CLAIM_INVALID",
            Self::RouteConflict => "ROUTE_CONFLICT",
            Self::RoutePending => "ROUTE_PENDING",
            Self::PollerActive => "TELEGRAM_POLL_ACTIVE",
            Self::SendCapacity => "SEND_CAPACITY",
            Self::SendBookkeepingFailed => "SEND_BOOKKEEPING_FAILED",
            Self::RandomUnavailable => "RANDOM_UNAVAILABLE",
            Self::StorageUnavailable => "STORAGE_UNAVAILABLE",
            Self::StorageFailure => "STORAGE_FAILED",
            Self::StorageCapacity => "STORAGE_CAPACITY",
            Self::UnsupportedSchema(_) => "STORAGE_SCHEMA_UNSUPPORTED",
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Telegram(error) => fmt::Display::fmt(error, f),
            Self::ConfigRead => write!(f, "{}: cannot read bounded configuration", self.code()),
            Self::InvalidConfig(field) => write!(f, "{}: invalid {field}", self.code()),
            Self::SecretUnavailable(field) => {
                write!(
                    f,
                    "{}: missing, invalid or unprotected {field}",
                    self.code()
                )
            }
            Self::EmptyText => write!(f, "{}: outgoing text must not be empty", self.code()),
            Self::InvalidRequest(field) => write!(f, "{}: invalid {field}", self.code()),
            Self::FileUnavailable => write!(
                f,
                "{}: expected a readable regular file beneath an approved root without symlinks or traversal",
                self.code()
            ),
            Self::FileTooLarge {
                limit_bytes,
                actual_bytes,
            } => write!(
                f,
                "{}: file has {actual_bytes} bytes (limit {limit_bytes})",
                self.code()
            ),
            Self::SessionNotFound => write!(f, "{}: unknown session handle", self.code()),
            Self::SessionIdentityConflict => {
                write!(f, "{}: session identities disagree", self.code())
            }
            Self::InvalidClaim => write!(
                f,
                "{}: claim is absent, expired or no longer current",
                self.code()
            ),
            Self::RouteConflict => write!(f, "{}: message already has a route", self.code()),
            Self::RoutePending => write!(f, "{}: send bookkeeping is still pending", self.code()),
            Self::PollerActive => {
                write!(f, "{}: a Telegram poller is already running", self.code())
            }
            Self::SendCapacity => write!(f, "{}: admitted send limit reached", self.code()),
            Self::SendBookkeepingFailed => write!(
                f,
                "{}: Telegram accepted the send, but its reply route could not be saved; do not resend automatically",
                self.code()
            ),
            Self::RandomUnavailable => write!(f, "{}: OS randomness unavailable", self.code()),
            Self::TextTooLong(details) => write!(
                f,
                "{}: final text has {} Unicode scalars (limit {}) and {} UTF-16 units (native limit {}). {}",
                self.code(),
                details.actual_characters,
                details.limit,
                details.actual_utf16_units,
                details.native_limit_utf16,
                details.guidance,
            ),
            Self::StorageUnavailable => write!(
                f,
                "{}: database location is unavailable or unprotected",
                self.code()
            ),
            Self::StorageFailure => write!(f, "{}: database operation failed", self.code()),
            Self::StorageCapacity => write!(f, "{}: database capacity exhausted", self.code()),
            Self::UnsupportedSchema(version) => write!(
                f,
                "{}: unrecognized database schema version {version}",
                self.code()
            ),
        }
    }
}

impl std::error::Error for AppError {}

impl From<TelegramError> for AppError {
    fn from(error: TelegramError) -> Self {
        Self::Telegram(error)
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        // SQLite errors may contain SQL, paths or data; retain only a safe classification.
        match error {
            rusqlite::Error::SqliteFailure(cause, _)
                if cause.code == rusqlite::ErrorCode::DiskFull =>
            {
                Self::StorageCapacity
            }
            _ => Self::StorageFailure,
        }
    }
}
