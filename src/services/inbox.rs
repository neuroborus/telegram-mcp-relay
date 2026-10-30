//! Reply ownership shared by inbox tools and question waiters; no network effects or timers.

use crate::{
    config::Delivery,
    error::AppError,
    repositories::{ClaimedReply, Database, ProcessingStatus, ReconciledProcessing, ReplyTarget},
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

    /// Confirm the first start durably. Inactivity preserves processing ownership;
    /// the returned first start anchors future in-memory typing, without resetting it.
    pub fn processing(
        &self,
        database: &mut Database,
        session_key: &str,
        reply_id: &str,
        claim_token: &str,
        active: bool,
        now: i64,
    ) -> Result<ProcessingStatus, AppError> {
        database.processing(session_key, reply_id, claim_token, active, now)
    }

    /// Save a fresh, unguessable request ID (1–64 UTF-8 bytes) with the token before resume.
    /// Identical retries recover the committed result until a later rotation or completion;
    /// distinct requests compete through the token fence and preserve the first start.
    pub fn reconcile_processing(
        &self,
        database: &mut Database,
        session_key: &str,
        reply_id: &str,
        claim_token: &str,
        request_id: &str,
        now: i64,
    ) -> Result<ReconciledProcessing, AppError> {
        database.reconcile_processing(session_key, reply_id, claim_token, request_id, now)
    }

    /// Complete only this valid claim, atomically removing payloads and saving a
    /// bounded idempotency receipt. A prior processing-start call is not required.
    pub fn acknowledge(
        &self,
        database: &mut Database,
        session_key: &str,
        reply_id: &str,
        claim_token: &str,
        now: i64,
    ) -> Result<(), AppError> {
        database.acknowledge_reply(session_key, reply_id, claim_token, now)
    }
}
