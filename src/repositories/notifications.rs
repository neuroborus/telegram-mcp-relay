//! Bounded, best-effort interests in committed inbox changes. No reply payloads live here.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{config::Limits, error::AppError};

#[derive(Clone)]
pub(crate) struct InboxNotifications {
    interests: Arc<Mutex<Vec<Interest>>>,
    pub(crate) limit: usize,
    queue: usize,
    waiter_limit: usize,
}

struct Interest {
    waiter: bool,
    keys: Vec<String>,
    pending: Vec<String>,
    sender: mpsc::Sender<String>,
    stopped: CancellationToken,
}

/// Dropping a listener releases its keys, queue and admission, including on cancellation.
pub(crate) struct InboxSubscription {
    changes: mpsc::Receiver<String>,
    pub(crate) stopped: CancellationToken,
    owner: InboxNotifications,
    sender: mpsc::Sender<String>,
}

impl InboxNotifications {
    pub(crate) fn new(limits: &Limits) -> Result<Self, AppError> {
        if !(1..=1_024).contains(&limits.subscriptions)
            || !(1..=1_024).contains(&limits.subscription_queue)
        {
            return Err(AppError::InvalidConfig("limits subscriptions"));
        }
        if !(1..=1_024).contains(&limits.waiters) {
            return Err(AppError::InvalidConfig("limits.waiters"));
        }
        Ok(Self {
            interests: Arc::default(),
            limit: limits.subscriptions as usize,
            queue: limits.subscription_queue as usize,
            waiter_limit: limits.waiters as usize,
        })
    }

    /// Keys have already been authorized under the database guard. Arm before catch-up.
    pub(crate) fn subscribe(&self, keys: Vec<String>) -> Result<InboxSubscription, AppError> {
        if keys.is_empty() || keys.len() > self.limit {
            return Err(AppError::InvalidRequest("inbox subscriptions"));
        }
        self.interest(keys, false)
    }

    /// Reserve before sending. Waiters cannot consume resource-listener admission.
    pub(crate) fn waiter(&self, key: String) -> Result<InboxSubscription, AppError> {
        self.interest(vec![key], true)
    }

    fn interest(&self, keys: Vec<String>, waiter: bool) -> Result<InboxSubscription, AppError> {
        let mut interests = self
            .interests
            .lock()
            .map_err(|_| AppError::McpUnavailable)?;
        let limit = if waiter {
            self.waiter_limit
        } else {
            self.limit
        };
        if interests.iter().filter(|i| i.waiter == waiter).count() >= limit {
            return Err(if waiter {
                AppError::WaitCapacity
            } else {
                AppError::McpUnavailable
            });
        }
        let (sender, changes) = mpsc::channel(if waiter { 1 } else { self.queue });
        let stopped = CancellationToken::new();
        interests.push(Interest {
            waiter,
            keys,
            pending: Vec::new(),
            sender: sender.clone(),
            stopped: stopped.clone(),
        });
        Ok(InboxSubscription {
            changes,
            stopped,
            owner: self.clone(),
            sender,
        })
    }

    /// Call only after commit. A slow listener never delays or fails a storage operation.
    pub(crate) fn publish(&self, key: &str) {
        if let Ok(mut interests) = self.interests.lock() {
            for interest in interests.iter_mut() {
                if !interest.stopped.is_cancelled()
                    && interest.keys.iter().any(|candidate| candidate == key)
                    && !interest.pending.iter().any(|pending| pending == key)
                {
                    if interest.sender.try_send(key.to_owned()).is_ok() {
                        interest.pending.push(key.to_owned());
                    } else {
                        // Overflow ends only this stream; SQLite supplies missed state.
                        interest.stopped.cancel();
                    }
                }
            }
        }
    }
}

impl InboxSubscription {
    pub(crate) async fn recv(&mut self) -> Option<String> {
        let key = self.changes.recv().await?;
        self.received(&key);
        Some(key)
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&mut self) -> Result<String, mpsc::error::TryRecvError> {
        let key = self.changes.try_recv()?;
        self.received(&key);
        Ok(key)
    }

    fn received(&self, key: &str) {
        if let Ok(mut interests) = self.owner.interests.lock()
            && let Some(interest) = interests
                .iter_mut()
                .find(|i| i.sender.same_channel(&self.sender))
        {
            interest.pending.retain(|pending| pending != key);
        }
    }
}

impl Drop for InboxSubscription {
    fn drop(&mut self) {
        if let Ok(mut interests) = self.owner.interests.lock() {
            interests.retain(|interest| !interest.sender.same_channel(&self.sender));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overflow_is_per_listener_and_drop_releases_admission() {
        let hub = InboxNotifications::new(&Limits {
            subscriptions: 2,
            subscription_queue: 1,
            ..Limits::default()
        })
        .unwrap();
        let mut slow = hub.subscribe(vec!["a".into(), "b".into()]).unwrap();
        let mut fast = hub.subscribe(vec!["a".into(), "b".into()]).unwrap();
        assert!(hub.subscribe(vec!["a".into()]).is_err());
        hub.publish("other");
        assert!(slow.try_recv().is_err());
        assert!(fast.try_recv().is_err());
        for _ in 0..3 {
            hub.publish("a");
            assert_eq!(fast.try_recv().unwrap(), "a");
        }
        assert!(!slow.stopped.is_cancelled());
        hub.publish("b");
        assert_eq!(fast.try_recv().unwrap(), "b");
        assert!(slow.stopped.is_cancelled());
        assert!(!fast.stopped.is_cancelled());
        assert_eq!(slow.try_recv().unwrap(), "a");
        assert!(slow.try_recv().is_err());
        drop(slow);
        let replacement = hub.subscribe(vec!["b".into()]).unwrap();
        hub.publish("b");
        assert_eq!(fast.try_recv().unwrap(), "b");
        assert!(!replacement.stopped.is_cancelled());
    }

    #[test]
    fn invalid_budgets_and_excessive_interest_sets_are_rejected() {
        for waiters in [0, 1_025] {
            assert!(
                InboxNotifications::new(&Limits {
                    waiters,
                    ..Limits::default()
                })
                .is_err()
            );
        }
        for (subscriptions, subscription_queue) in [(0, 1), (1, 0), (1_025, 1), (1, 1_025)] {
            assert!(
                InboxNotifications::new(&Limits {
                    subscriptions,
                    subscription_queue,
                    ..Limits::default()
                })
                .is_err()
            );
        }
        let hub = InboxNotifications::new(&Limits {
            subscriptions: 1,
            ..Limits::default()
        })
        .unwrap();
        assert!(hub.subscribe(Vec::new()).is_err());
        assert!(hub.subscribe(vec!["a".into(), "b".into()]).is_err());
    }
}
