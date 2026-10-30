//! Modern SDK subscriptions. SQLite is the recovery source; streams are only invalidations.

use std::time::Duration;

use crate::repositories::InboxSubscription;

use super::*;

impl McpHandler {
    pub(super) fn accept_inboxes(&self, requested: &SubscriptionFilter) -> SubscriptionFilter {
        let Some(uris) = &requested.resource_subscriptions else {
            return SubscriptionFilter::default();
        };
        if uris.len() > self.notifications.limit {
            return SubscriptionFilter::default();
        }
        let Ok(database) = self.database.lock() else {
            return SubscriptionFilter::default();
        };
        let accepted = uris.iter().filter(|uri| {
            inbox::subscription_key(uri).is_ok_and(|key| database.session(key).is_ok())
        });
        // Other categories are not advertised. Unknown/malformed inbox URIs are omitted.
        SubscriptionFilter::builder()
            .resource_subscriptions(accepted.cloned())
            .build()
    }

    pub(super) async fn listen_inboxes(
        &self,
        context: SubscriptionContext,
    ) -> Result<(), ErrorData> {
        let mut keys = context
            .accepted()
            .resource_subscriptions
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|uri| inbox::subscription_key(uri).map(str::to_owned))
            .collect::<Result<Vec<_>, _>>()?;
        keys.sort_unstable();
        keys.dedup();
        if keys.is_empty() {
            return Ok(());
        }
        let mut subscription = {
            let database = self.database.lock().map_err(|_| unavailable())?;
            for key in &keys {
                database.session(key).map_err(|_| unavailable())?;
            }
            self.notifications
                .subscribe(keys.clone())
                .map_err(|_| unavailable())?
        };
        // The SDK sends its acknowledgment before invoking listen. Arm first, then
        // invalidate every accepted inbox so a commit in that setup gap is not lost.
        for key in keys {
            if !notify(&context, &subscription, key).await {
                return Ok(());
            }
        }
        let stopped = subscription.stopped.clone();
        loop {
            let key = tokio::select! {
                biased;
                _ = context.cancelled() => return Ok(()),
                _ = stopped.cancelled() => return Ok(()),
                key = subscription.recv() => match key {
                    Some(key) => key,
                    None => return Ok(()),
                },
            };
            if !notify(&context, &subscription, key).await {
                return Ok(());
            }
        }
    }
}

fn unavailable() -> ErrorData {
    ErrorData::internal_error("Inbox subscription unavailable", None)
}

async fn notify(
    context: &SubscriptionContext,
    subscription: &InboxSubscription,
    key: String,
) -> bool {
    let uri = format!("telegram-bridge://sessions/{key}/inbox");
    tokio::select! {
        biased;
        _ = context.cancelled() => false,
        _ = subscription.stopped.cancelled() => false,
        result = tokio::time::timeout(Duration::from_secs(5), context.sink().notify_resource_updated(uri)) => {
            matches!(result, Ok(Ok(())))
        }
    }
}
