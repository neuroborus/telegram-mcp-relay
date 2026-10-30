//! Claim policy shared by inbox tools and question waiters; no network effects or timers.

use crate::{
    config::Delivery,
    error::AppError,
    repositories::{ClaimedReply, Database, ReplyTarget},
};

pub struct InboxService {
    claim_lease_ms: i64,
}

impl InboxService {
    pub fn new(settings: &Delivery) -> Result<Self, AppError> {
        if !(1..=600).contains(&settings.claim_lease_seconds) {
            return Err(AppError::InvalidConfig("delivery.claim_lease_seconds"));
        }
        Ok(Self {
            claim_lease_ms: settings.claim_lease_seconds as i64 * 1_000,
        })
    }

    /// Acquire pending or lease-expired pre-processing replies in receipt order.
    /// A question target narrows eligibility; it never changes session authorization.
    /// Returning/dropping a claim does not acknowledge it or start processing.
    pub fn claim(
        &self,
        database: &mut Database,
        session_key: &str,
        target: Option<ReplyTarget>,
        limit: u32,
        now: i64,
    ) -> Result<Vec<ClaimedReply>, AppError> {
        database.claim_replies(session_key, target, limit, self.claim_lease_ms, now)
    }
}
