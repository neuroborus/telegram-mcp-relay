//! Best-effort RAM activity; one lazy scheduler merges confirmed work per chat.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use tokio::{sync::watch, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    clients::telegram::TelegramClient,
    config::Config,
    error::{AppError, TelegramError},
    repositories::ProcessingStatus,
};

/// Share this handle between inbox transitions, cleanup and outbound delivery. It holds no
/// payloads or claim tokens, and is never reconstructed from stored processing replies at startup.
#[derive(Clone)]
pub struct TypingService {
    state: Arc<Mutex<State>>,
    changed: watch::Sender<()>,
    failed: watch::Sender<bool>,
    telegram: TelegramClient,
    refresh: Duration,
    maximum_ms: i64,
    limit: usize,
    chats: Vec<i64>,
    enabled: bool,
}

struct Lease {
    session: String,
    chat: i64,
    deadline: Instant,
    active: bool,
}

struct InFlight {
    deadline: Instant,
    cancelled: CancellationToken,
}

#[derive(Default)]
struct State {
    // Retain inactive/expired caps until acknowledgment or retention deletion.
    // Never evict one to admit activity.
    replies: HashMap<String, Lease>,
    in_flight: BTreeMap<i64, InFlight>,
    // Keep cooldowns through stop/start races. None means an unrepresentable retry_after.
    next_attempt: HashMap<i64, Option<Instant>>,
    running: bool,
    closed: bool,
    worker: Option<JoinHandle<()>>,
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.abort();
        }
    }
}

// Aborts and panics must wake daemon supervision even before the worker's first poll.
struct WorkerOutcome(Option<watch::Sender<bool>>);

impl WorkerOutcome {
    fn complete(mut self) {
        self.0 = None;
    }
}

impl Drop for WorkerOutcome {
    fn drop(&mut self) {
        if let Some(failed) = &self.0 {
            failed.send_replace(true);
        }
    }
}

impl TypingService {
    pub fn new(telegram: TelegramClient, config: &Config) -> Result<Self, AppError> {
        let settings = &config.activity;
        if !(1..=5).contains(&settings.typing_refresh_seconds)
            || !(settings.typing_refresh_seconds..=600)
                .contains(&settings.typing_max_duration_seconds)
        {
            return Err(AppError::InvalidConfig("activity typing bounds"));
        }
        if !(1..=256).contains(&config.telegram.allowed_chat_ids.len())
            || !(1..=1_000_000).contains(&config.limits.inbox_items)
        {
            return Err(AppError::InvalidConfig("typing activity bounds"));
        }
        Ok(Self {
            state: Arc::default(),
            changed: watch::channel(()).0,
            failed: watch::channel(false).0,
            telegram,
            refresh: Duration::from_secs(settings.typing_refresh_seconds),
            maximum_ms: settings.typing_max_duration_seconds as i64 * 1_000,
            limit: config.limits.inbox_items as usize,
            chats: config.telegram.allowed_chat_ids.clone(),
            enabled: settings.typing_enabled,
        })
    }

    /// Event-driven supervision; normal idle retirement and HTTP failures are not fatal.
    pub(crate) async fn failed(&self) {
        let mut failed = self.failed.subscribe();
        while !*failed.borrow_and_update() {
            if failed.changed().await.is_err() {
                return;
            }
        }
    }

    pub(crate) fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            state.replies.clear();
            state.next_attempt.clear();
            self.changed.send_replace(());
        }
    }

    /// Wake the lazy worker, cancel outstanding actions and await its retirement.
    pub(crate) async fn drain(&self) -> Result<(), AppError> {
        let worker = self
            .state
            .lock()
            .map_err(|_| AppError::ServiceStopped)?
            .worker
            .take();
        if let Some(worker) = worker {
            worker.await.map_err(|_| AppError::ServiceStopped)?;
        }
        if *self.failed.borrow() {
            return Err(AppError::ServiceStopped);
        }
        Ok(())
    }

    /// Called synchronously after the fenced durable transition, under its database guard.
    pub(crate) fn start(&self, session: &str, reply: &str, status: &ProcessingStatus, now: i64) {
        let Some(first) = status.processing_started_at else {
            return;
        };
        if !self.enabled || !self.chats.contains(&status.telegram_chat_id) {
            return;
        }
        let remaining = self
            .maximum_ms
            .saturating_sub(now.saturating_sub(first).max(0));
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.closed {
            return;
        }
        let instant = Instant::now();
        let deadline = instant + Duration::from_millis(remaining.max(0) as u64);
        let active = if let Some(lease) = state.replies.get_mut(reply) {
            // The original monotonic cap survives inactivity, reconciliation and expiry,
            // even if the wall clock later moves backwards.
            lease.deadline = lease.deadline.min(deadline);
            lease.active = lease.deadline > instant;
            lease.active
        } else {
            if state.replies.len() >= self.limit {
                return; // Durable ownership succeeds; existing caps are never evicted.
            }
            state.replies.insert(
                reply.to_owned(),
                Lease {
                    session: session.to_owned(),
                    chat: status.telegram_chat_id,
                    deadline,
                    active: deadline > instant,
                },
            );
            deadline > instant
        };
        if !active {
            self.changed.send_replace(());
            return;
        }
        if !state.running {
            state.running = true;
            let outcome = WorkerOutcome(Some(self.failed.clone()));
            let driver = run(
                Arc::downgrade(&self.state),
                self.changed.subscribe(),
                {
                    let telegram = self.telegram.clone();
                    move |chat| {
                        let telegram = telegram.clone();
                        async move { telegram.send_chat_action(chat).await }
                    }
                },
                self.refresh,
            );
            state.worker = Some(tokio::spawn(async move {
                if driver.await.is_ok() {
                    outcome.complete();
                }
            }));
        }
        self.changed.send_replace(());
    }

    pub(crate) fn stop(&self, reply: &str) {
        if let Ok(mut state) = self.state.lock()
            && let Some(lease) = state.replies.get_mut(reply)
            && lease.active
        {
            lease.active = false;
            self.changed.send_replace(());
        }
    }

    pub(crate) fn stop_session(&self, session: &str) {
        if let Ok(mut state) = self.state.lock() {
            let mut changed = false;
            for lease in state
                .replies
                .values_mut()
                .filter(|lease| lease.session == session)
            {
                changed |= lease.active;
                lease.active = false;
            }
            if changed {
                self.changed.send_replace(());
            }
        }
    }

    /// Acknowledgment or retention deletion makes further durable starts impossible.
    pub(crate) fn complete(&self, reply: &str) {
        if let Ok(mut state) = self.state.lock()
            && state.replies.remove(reply).is_some()
        {
            self.changed.send_replace(());
        }
    }
}

enum Work {
    Idle,
    Active {
        wake: Instant,
        requests: Vec<(i64, CancellationToken)>,
    },
}

impl State {
    fn work(&mut self, now: Instant, refresh: Duration) -> Work {
        let mut chats = BTreeMap::<i64, Instant>::new();
        let mut wake = None;
        for lease in self.replies.values_mut() {
            lease.active &= lease.deadline > now;
            if lease.active {
                wake = Some(wake.unwrap_or(lease.deadline).min(lease.deadline));
                chats
                    .entry(lease.chat)
                    .and_modify(|deadline| *deadline = (*deadline).max(lease.deadline))
                    .or_insert(lease.deadline);
            }
        }
        for (chat, request) in &self.in_flight {
            if request.deadline <= now || !chats.contains_key(chat) {
                request.cancelled.cancel();
            }
        }
        let Some(mut wake) = wake else {
            self.running = false;
            self.in_flight.clear();
            return Work::Idle;
        };
        let mut requests = Vec::new();
        // Admit every due chat in stable order. No chat waits for another HTTP result.
        for (chat, deadline) in chats {
            if let Some(request) = self.in_flight.get(&chat) {
                if request.deadline > now {
                    wake = wake.min(request.deadline);
                }
                continue;
            }
            let Some(due) = self.next_attempt.get(&chat).copied().unwrap_or(Some(now)) else {
                continue;
            };
            if due > now {
                wake = wake.min(due);
                continue;
            }
            let cancelled = CancellationToken::new();
            let deadline = deadline.min(now + refresh);
            self.in_flight.insert(
                chat,
                InFlight {
                    deadline,
                    cancelled: cancelled.clone(),
                },
            );
            self.next_attempt.insert(chat, Some(now + refresh));
            requests.push((chat, cancelled));
            wake = wake.min(deadline);
        }
        Work::Active { wake, requests }
    }
}

async fn request(
    chat: i64,
    cancelled: CancellationToken,
    send: impl Future<Output = Result<(), AppError>>,
) -> (i64, Option<Result<(), AppError>>) {
    let result = tokio::select! {
        biased;
        _ = cancelled.cancelled() => None,
        result = send => Some(result),
    };
    (chat, result)
}

async fn run<F, Fut>(
    state: Weak<Mutex<State>>,
    mut changed: watch::Receiver<()>,
    send: F,
    refresh: Duration,
) -> Result<(), AppError>
where
    F: Fn(i64) -> Fut,
    Fut: Future<Output = Result<(), AppError>>,
{
    let mut pending = FuturesUnordered::new();
    loop {
        let work = {
            let Some(state) = state.upgrade() else {
                return Ok(());
            };
            let mut state = state.lock().map_err(|_| AppError::ServiceStopped)?;
            changed.borrow_and_update();
            state.work(Instant::now(), refresh)
        };
        let Work::Active { wake, requests } = work else {
            return Ok(());
        };
        for (chat, cancelled) in requests {
            pending.push(request(chat, cancelled, send(chat)));
        }
        tokio::select! {
            biased;
            _ = changed.changed() => {},
            Some((chat, result)) = pending.next(), if !pending.is_empty() => {
                let Some(state) = state.upgrade() else { return Ok(()) };
                let mut state = state.lock().map_err(|_| AppError::ServiceStopped)?;
                state.in_flight.remove(&chat);
                if let Some(Err(error)) = result {
                    if let AppError::Telegram(TelegramError::Rejected {
                        retry_after_seconds: Some(seconds), ..
                    }) = &error {
                        state.next_attempt.insert(chat,
                            Instant::now().checked_add(Duration::from_secs(*seconds).max(refresh)));
                    }
                    tracing::debug!(code = error.code(), "Telegram typing deferred");
                }
            },
            _ = tokio::time::sleep_until(wake) => {},
        }
    }
}

#[cfg(test)]
mod tests;
