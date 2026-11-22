//! Reply ownership shared by inbox tools and question waiters, with post-commit RAM activity.

use std::sync::Mutex;

use super::{outbound::OutboundService, typing::TypingService, unix_millis};

use crate::{
    config::{Delivery, ProcessingStartAck},
    dto::{ProcessingRequest, ProcessingResult, StartAcknowledgment, StartAcknowledgmentKind},
    error::{AppError, TelegramError},
    repositories::{
        ClaimedReply, Database, ProcessingAction, ProcessingStatus, ProcessingTransition,
        ReconciledProcessing, ReplyTarget, StartAckEffect,
    },
};

pub struct InboxService {
    claim_lease_ms: i64,
    start_ack: ProcessingStartAck,
    typing: Option<TypingService>,
}

impl InboxService {
    pub fn new(settings: &Delivery) -> Result<Self, AppError> {
        if !(1..=600).contains(&settings.claim_lease_seconds) {
            return Err(AppError::InvalidConfig("delivery.claim_lease_seconds"));
        }
        Ok(Self {
            claim_lease_ms: settings.claim_lease_seconds as i64 * 1_000,
            start_ack: settings.processing_start_ack,
            typing: None,
        })
    }

    /// Attach the same scheduler used by outbound delivery; claims alone never start it.
    pub fn with_typing(mut self, typing: TypingService) -> Self {
        self.typing = Some(typing);
        self
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

    /// Apply the state and typing transition without a Telegram sender. A first start
    /// still consumes its acknowledgment attempt, as it would before a process crash.
    pub fn processing(
        &self,
        database: &mut Database,
        session_key: &str,
        reply_id: &str,
        claim_token: &str,
        active: bool,
        now: i64,
    ) -> Result<ProcessingStatus, AppError> {
        let action = if active {
            ProcessingAction::Start {
                message: None,
                default: self.start_ack,
            }
        } else {
            ProcessingAction::Stop
        };
        self.transition(database, session_key, reply_id, claim_token, action, now)
            .map(|transition| transition.status)
    }

    fn transition(
        &self,
        database: &mut Database,
        session_key: &str,
        reply_id: &str,
        claim_token: &str,
        action: ProcessingAction<'_>,
        now: i64,
    ) -> Result<ProcessingTransition, AppError> {
        let transition = database
            .processing(session_key, reply_id, claim_token, action, now)
            .map_err(|mut error| {
                if let AppError::TextTooLong(details) = &mut error {
                    details.guidance = "Retry telegram_processing with a shorter message, or re-register the same session with a shorter title and project label.";
                }
                error
            })?;
        if let Some(typing) = &self.typing {
            match action {
                ProcessingAction::Start { .. } => {
                    typing.start(session_key, reply_id, &transition.status, now);
                }
                ProcessingAction::Stop => typing.stop(reply_id),
            }
        }
        Ok(transition)
    }

    pub(crate) async fn process(
        &self,
        database: &Mutex<Database>,
        outbound: &OutboundService,
        request: ProcessingRequest,
    ) -> Result<ProcessingResult, AppError> {
        let (status, token, start_ack) = {
            let mut database = database.lock().map_err(|_| AppError::StorageFailure)?;
            let now = unix_millis()?;
            match (
                request.active,
                request.reconcile_request_id,
                request.message,
            ) {
                (Some(active), None, message) if active || message.is_none() => {
                    let action = if active {
                        ProcessingAction::Start {
                            message: message.as_deref(),
                            default: self.start_ack,
                        }
                    } else {
                        ProcessingAction::Stop
                    };
                    let transition = self.transition(
                        &mut database,
                        &request.session_key,
                        &request.reply_id,
                        &request.claim_token,
                        action,
                        now,
                    )?;
                    (transition.status, request.claim_token, transition.start_ack)
                }
                (None, Some(id), None) => {
                    let resumed = self.reconcile_processing(
                        &mut database,
                        &request.session_key,
                        &request.reply_id,
                        &request.claim_token,
                        &id,
                        now,
                    )?;
                    (resumed.status, resumed.claim_token, None)
                }
                _ => {
                    return Err(AppError::InvalidRequest(
                        "processing action (supply exactly one of active or reconcile_request_id, and use message only with active=true)",
                    ));
                }
            }
        };
        // A consumed attempt never falls back to another effect; processing stays started.
        let (start_acknowledgment, kind, outbound_message_id, start_acknowledgment_error_code) =
            if let Some(ack) = start_ack {
                let kind = match ack.effect {
                    StartAckEffect::Text(_) => StartAcknowledgmentKind::Message,
                    StartAckEffect::Reaction => StartAcknowledgmentKind::Reaction,
                };
                let sent = match kind {
                    StartAcknowledgmentKind::Message => {
                        outbound.send_start_ack(ack).await.map(|confirmed| {
                            (Some(confirmed.message.message_id), confirmed.bookkeeping)
                        })
                    }
                    StartAcknowledgmentKind::Reaction => outbound
                        .react_start_ack(ack)
                        .await
                        .map(|bookkeeping| (None, bookkeeping)),
                };
                match sent {
                    Ok((message_id, bookkeeping)) => (
                        StartAcknowledgment::Sent,
                        Some(kind),
                        message_id,
                        bookkeeping.err().map(|error| error.code()),
                    ),
                    Err(AppError::Telegram(TelegramError::DeliveryUncertain)) => {
                        (StartAcknowledgment::Uncertain, Some(kind), None, None)
                    }
                    Err(_) => (StartAcknowledgment::Failed, Some(kind), None, None),
                }
            } else {
                (StartAcknowledgment::AlreadyStarted, None, None, None)
            };
        Ok(ProcessingResult {
            reply_id: request.reply_id,
            claim_token: token,
            processing_started_at: status.processing_started_at,
            start_acknowledgment,
            start_acknowledgment_kind: kind,
            outbound_message_id,
            start_acknowledgment_error_code,
        })
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
        let (resumed, rotated) =
            database.reconcile_processing(session_key, reply_id, claim_token, request_id, now)?;
        if rotated && let Some(typing) = &self.typing {
            typing.stop(reply_id);
        }
        Ok(resumed)
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
        database.acknowledge_reply(session_key, reply_id, claim_token, now)?;
        if let Some(typing) = &self.typing {
            typing.complete(reply_id);
        }
        Ok(())
    }
}
