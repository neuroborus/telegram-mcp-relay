//! Bounded delivery and reply-ingress orchestration over domain rules and effect adapters.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::AppError;

pub mod ask;
pub mod files;
pub mod inbox;
pub mod ingress;
pub mod notices;
pub mod outbound;
pub mod polling;
pub mod typing;

pub(crate) fn unix_millis() -> Result<i64, AppError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or(AppError::InvalidRequest("timestamp"))
}
