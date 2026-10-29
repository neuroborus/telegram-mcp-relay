//! Safe application errors: never retain parser input, secret values or filesystem paths.

use std::fmt;

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
}

impl AppError {
    /// Stable machine-readable classification, independent of the underlying private cause.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ConfigRead => "CONFIG_READ_FAILED",
            Self::InvalidConfig(_) => "CONFIG_INVALID",
            Self::SecretUnavailable(_) => "SECRET_UNAVAILABLE",
            Self::EmptyText => "TEXT_EMPTY",
            Self::TextTooLong(_) => "TEXT_TOO_LONG",
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
        }
    }
}

impl std::error::Error for AppError {}
