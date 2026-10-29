//! Safe application errors: never retain parser input, secret values or filesystem paths.

use std::fmt;

/// Startup failures with static, safe field names for diagnostics and future tool results.
#[derive(Debug, PartialEq, Eq)]
pub enum AppError {
    ConfigRead,
    InvalidConfig(&'static str),
    SecretUnavailable(&'static str),
}

impl AppError {
    /// Stable machine-readable classification, independent of the underlying private cause.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ConfigRead => "CONFIG_READ_FAILED",
            Self::InvalidConfig(_) => "CONFIG_INVALID",
            Self::SecretUnavailable(_) => "SECRET_UNAVAILABLE",
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
        }
    }
}

impl std::error::Error for AppError {}
