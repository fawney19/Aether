use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use super::super::error::GatewayError;
use super::super::provider_transport;
use tokio::sync::Notify;

#[derive(Debug, Clone)]
pub(crate) enum ProviderTransportSnapshotFlightResult {
    Published(Arc<provider_transport::GatewayProviderTransportSnapshot>),
    Missing,
    Invalidated,
    Retry,
    Error(GatewayError),
}

/// One transport snapshot load per cache key. The completion value is kept
/// alongside the notification so a waiter that is scheduled after the
/// broadcast can still observe the result without a lost wakeup.
#[derive(Debug)]
pub(crate) struct ProviderTransportSnapshotFlight {
    generation: u64,
    notify: Arc<Notify>,
    result: StdMutex<Option<ProviderTransportSnapshotFlightResult>>,
}

impl ProviderTransportSnapshotFlight {
    pub(crate) fn new(generation: u64) -> Self {
        Self {
            generation,
            notify: Arc::new(Notify::new()),
            result: StdMutex::new(None),
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    fn result(&self) -> Option<ProviderTransportSnapshotFlightResult> {
        self.result
            .lock()
            .map(|result| result.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// Completes the flight once. A clear/cancellation may win the race with
    /// the leader; in that case the old result must not overwrite Invalidated.
    pub(crate) fn complete(&self, result: ProviderTransportSnapshotFlightResult) -> bool {
        let completed = match self.result.lock() {
            Ok(mut current) => {
                if current.is_some() {
                    false
                } else {
                    *current = Some(result);
                    true
                }
            }
            Err(poisoned) => {
                let mut current = poisoned.into_inner();
                if current.is_some() {
                    false
                } else {
                    *current = Some(result);
                    true
                }
            }
        };
        if completed {
            self.notify.notify_waiters();
        }
        completed
    }

    pub(crate) async fn wait(&self) -> ProviderTransportSnapshotFlightResult {
        loop {
            if let Some(result) = self.result() {
                return result;
            }

            // Register before checking the completion state a second time.
            // This closes the result-check/notify race even when the task has
            // not been polled yet when the leader broadcasts completion.
            let mut notified = Box::pin(self.notify.notified());
            notified.as_mut().enable();
            if let Some(result) = self.result() {
                return result;
            }
            notified.await;
        }
    }
}

pub(crate) const AUTH_API_KEY_LAST_USED_TTL: Duration = Duration::from_secs(60);
pub(crate) const AUTH_API_KEY_LAST_USED_MAX_ENTRIES: usize = 10_000;
// Keep normal freshness short for cross-node configuration propagation. A
// stale entry is served while one background refresh runs, so an idle burst
// does not turn expiry into a synchronous database wait.
pub(crate) const PROVIDER_TRANSPORT_SNAPSHOT_CACHE_TTL: Duration = Duration::from_secs(1);
// Keep the last known transport usable across normal idle periods. The first
// stale request starts a single background refresh, and every local catalog or
// credential mutation advances the generation and clears the entry.
pub(crate) const PROVIDER_TRANSPORT_SNAPSHOT_CACHE_STALE_TTL: Duration =
    Duration::from_secs(5 * 60);
pub(crate) const PROVIDER_TRANSPORT_SNAPSHOT_CACHE_MAX_ENTRIES: usize = 1_024;

#[derive(Debug, Clone)]
pub(crate) struct CachedProviderTransportSnapshot {
    pub(crate) loaded_at: std::time::Instant,
    pub(crate) generation: u64,
    pub(crate) snapshot: Arc<provider_transport::GatewayProviderTransportSnapshot>,
}
