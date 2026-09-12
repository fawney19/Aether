use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use aether_data_contracts::DataLayerError;
use aether_runtime_state::RuntimeQueueEntry;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

const DEFAULT_READ_PAYLOAD_BUDGET_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_READ_BATCH_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;

static READ_BUDGET: LazyLock<Arc<QueueReadBudget>> = LazyLock::new(|| {
    let limit = std::env::var("AETHER_USAGE_QUEUE_READ_PAYLOAD_BUDGET_BYTES").ok();
    let batch = std::env::var("AETHER_USAGE_QUEUE_READ_BATCH_PAYLOAD_BYTES").ok();
    Arc::new(QueueReadBudget::new(
        configured_bytes(limit.as_deref(), DEFAULT_READ_PAYLOAD_BUDGET_BYTES),
        configured_bytes(batch.as_deref(), DEFAULT_READ_BATCH_PAYLOAD_BYTES),
    ))
});

pub(crate) fn shared_queue_read_budget() -> Arc<QueueReadBudget> {
    Arc::clone(&READ_BUDGET)
}

pub(crate) fn queue_read_budget_metrics() -> QueueReadBudgetSnapshot {
    READ_BUDGET.snapshot()
}

fn maximum_budget_bytes() -> usize {
    Semaphore::MAX_PERMITS.min(u32::MAX as usize)
}

fn configured_bytes(raw: Option<&str>, fallback: usize) -> usize {
    raw.and_then(|raw| raw.trim().parse::<u128>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(maximum_budget_bytes() as u128) as usize)
        .unwrap_or(fallback.min(maximum_budget_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueueReadBudgetSnapshot {
    pub(crate) limit_bytes: usize,
    pub(crate) batch_limit_bytes: usize,
    pub(crate) reserved_bytes: usize,
    pub(crate) waiters: usize,
    pub(crate) wait_total: u64,
    /// Observed field names plus values, without cloning the returned strings.
    pub(crate) actual_field_bytes_total: u64,
    pub(crate) oversized_entries_total: u64,
    pub(crate) oversized_batches_total: u64,
}

/// A process-wide reservation based on the current producer payload limit.
/// Historical or externally written messages can exceed the estimate. Field names,
/// allocation capacity, RESP decoding, and decoded JSON are not an RSS bound here.
pub(crate) struct QueueReadBudget {
    limit_bytes: usize,
    batch_limit_bytes: usize,
    permits: Arc<Semaphore>,
    reserved_bytes: AtomicUsize,
    waiters: AtomicUsize,
    wait_total: AtomicU64,
    actual_field_bytes_total: AtomicU64,
    oversized_entries_total: AtomicU64,
    oversized_batches_total: AtomicU64,
}

impl QueueReadBudget {
    pub(crate) fn new(limit_bytes: usize, batch_limit_bytes: usize) -> Self {
        let limit_bytes = limit_bytes.clamp(1, maximum_budget_bytes());
        let batch_limit_bytes = batch_limit_bytes.clamp(1, limit_bytes);
        Self {
            limit_bytes,
            batch_limit_bytes,
            permits: Arc::new(Semaphore::new(limit_bytes)),
            reserved_bytes: AtomicUsize::new(0),
            waiters: AtomicUsize::new(0),
            wait_total: AtomicU64::new(0),
            actual_field_bytes_total: AtomicU64::new(0),
            oversized_entries_total: AtomicU64::new(0),
            oversized_batches_total: AtomicU64::new(0),
        }
    }

    pub(crate) fn snapshot(&self) -> QueueReadBudgetSnapshot {
        QueueReadBudgetSnapshot {
            limit_bytes: self.limit_bytes,
            batch_limit_bytes: self.batch_limit_bytes,
            reserved_bytes: self.reserved_bytes.load(Ordering::Relaxed),
            waiters: self.waiters.load(Ordering::Relaxed),
            wait_total: self.wait_total.load(Ordering::Relaxed),
            actual_field_bytes_total: self.actual_field_bytes_total.load(Ordering::Relaxed),
            oversized_entries_total: self.oversized_entries_total.load(Ordering::Relaxed),
            oversized_batches_total: self.oversized_batches_total.load(Ordering::Relaxed),
        }
    }

    pub(crate) async fn reserve(
        self: &Arc<Self>,
        requested_count: usize,
        payload_limit: usize,
    ) -> Result<(usize, QueueReadReservation), DataLayerError> {
        if payload_limit == 0 || payload_limit > self.limit_bytes {
            return Err(DataLayerError::InvalidConfiguration(format!(
                "usage queue payload limit {payload_limit} must be positive and not exceed the {}-byte read payload budget",
                self.limit_bytes
            )));
        }
        // A single valid payload may exceed the preferred batch target, but never
        // the total budget. Clamp before multiplying or converting to u32 permits.
        let count = requested_count
            .max(1)
            .min((self.batch_limit_bytes / payload_limit).max(1));
        let reserved_bytes = count * payload_limit;
        let permits = reserved_bytes as u32;
        let permit = match Arc::clone(&self.permits).try_acquire_many_owned(permits) {
            Ok(permit) => permit,
            Err(TryAcquireError::NoPermits) => {
                self.wait_total.fetch_add(1, Ordering::Relaxed);
                self.waiters.fetch_add(1, Ordering::Relaxed);
                let _waiting = WaitingReservation { budget: self };
                Arc::clone(&self.permits)
                    .acquire_many_owned(permits)
                    .await
                    .map_err(|_| closed_budget_error())?
            }
            Err(TryAcquireError::Closed) => return Err(closed_budget_error()),
        };
        self.reserved_bytes
            .fetch_add(reserved_bytes, Ordering::Relaxed);
        Ok((
            count,
            QueueReadReservation {
                budget: Arc::clone(self),
                reserved_bytes,
                permit: Some(permit),
            },
        ))
    }
}

fn closed_budget_error() -> DataLayerError {
    DataLayerError::InvalidConfiguration("usage queue read payload budget is closed".to_string())
}

struct WaitingReservation<'a> {
    budget: &'a QueueReadBudget,
}

impl Drop for WaitingReservation<'_> {
    fn drop(&mut self) {
        self.budget.waiters.fetch_sub(1, Ordering::Relaxed);
    }
}

// Deliberately not Clone: every concurrently retained batch needs its own lease.
pub(crate) struct QueueReadReservation {
    budget: Arc<QueueReadBudget>,
    reserved_bytes: usize,
    permit: Option<OwnedSemaphorePermit>,
}

impl QueueReadReservation {
    pub(crate) fn observe_entries(&mut self, entries: &[RuntimeQueueEntry], payload_limit: usize) {
        let mut value_bytes = 0usize;
        let mut field_bytes = 0usize;
        let mut oversized_entries = 0u64;
        for entry in entries {
            let mut entry_value_bytes = 0usize;
            for (key, value) in &entry.fields {
                entry_value_bytes = entry_value_bytes.saturating_add(value.len());
                field_bytes = field_bytes
                    .saturating_add(key.len())
                    .saturating_add(value.len());
            }
            value_bytes = value_bytes.saturating_add(entry_value_bytes);
            oversized_entries += u64::from(entry_value_bytes > payload_limit);
        }
        self.budget.actual_field_bytes_total.fetch_add(
            u64::try_from(field_bytes).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.budget
            .oversized_entries_total
            .fetch_add(oversized_entries, Ordering::Relaxed);
        if value_bytes > self.reserved_bytes {
            self.budget
                .oversized_batches_total
                .fetch_add(1, Ordering::Relaxed);
        }
        // Shrink unused payload estimates. Never wait for an upgrade after reading
        // an oversized historical batch: other batches may hold all remaining bytes.
        self.shrink_to(value_bytes.min(self.reserved_bytes));
    }

    fn shrink_to(&mut self, retained_bytes: usize) {
        let released = self.reserved_bytes.saturating_sub(retained_bytes);
        if released == 0 {
            return;
        }
        let permit = self
            .permit
            .as_mut()
            .expect("positive reservation must hold a permit")
            .split(released)
            .expect("released bytes must belong to this reservation");
        self.reserved_bytes -= released;
        self.budget
            .reserved_bytes
            .fetch_sub(released, Ordering::Relaxed);
        drop(permit);
    }
}

impl Drop for QueueReadReservation {
    fn drop(&mut self) {
        self.budget
            .reserved_bytes
            .fetch_sub(self.reserved_bytes, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::task::Poll;

    use super::*;

    #[tokio::test]
    async fn queue_read_budget_cancelled_wait_preserves_current_reservations() {
        let budget = Arc::new(QueueReadBudget::new(32, 16));
        let (_, first) = budget.reserve(8, 8).await.unwrap();
        let (_, second) = budget.reserve(8, 8).await.unwrap();
        let mut pending = Box::pin(budget.reserve(1, 8));
        std::future::poll_fn(|cx| {
            assert!(pending.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(budget.snapshot().reserved_bytes, 32);
        assert_eq!(budget.snapshot().waiters, 1);
        drop(pending);
        assert_eq!(budget.snapshot().waiters, 0);
        assert_eq!(budget.snapshot().wait_total, 1);
        drop(first);
        let (_, replacement) = budget.reserve(2, 8).await.unwrap();
        assert_eq!(budget.snapshot().reserved_bytes, 32);
        drop((second, replacement));
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.permits.available_permits(), 32);
    }

    #[tokio::test]
    async fn queue_read_budget_counts_payload_values_and_observes_legacy_excess_without_waiting() {
        let budget = Arc::new(QueueReadBudget::new(16, 16));
        let (count, mut reservation) = budget.reserve(2, 8).await.unwrap();
        assert_eq!(count, 2);
        let entries = [RuntimeQueueEntry {
            id: "1-0".to_string(),
            fields: BTreeMap::from([("payload".to_string(), "x".repeat(8))]),
        }];
        reservation.observe_entries(&entries, 8);
        assert_eq!(budget.snapshot().reserved_bytes, 8);
        assert_eq!(budget.snapshot().actual_field_bytes_total, 15);
        assert_eq!(budget.snapshot().oversized_entries_total, 0);
        let (_, mut legacy) = budget.reserve(1, 8).await.unwrap();
        let legacy_entries = [RuntimeQueueEntry {
            id: "2-0".to_string(),
            fields: BTreeMap::from([
                ("payload".to_string(), "x".repeat(8)),
                ("extra".to_string(), "y".repeat(24)),
            ]),
        }];
        legacy.observe_entries(&legacy_entries, 8);
        assert_eq!(budget.snapshot().reserved_bytes, 16);
        assert_eq!(budget.snapshot().actual_field_bytes_total, 59);
        assert_eq!(budget.snapshot().oversized_entries_total, 1);
        assert_eq!(budget.snapshot().oversized_batches_total, 1);
        assert_eq!(budget.snapshot().wait_total, 0);
        drop((reservation, legacy));
        assert_eq!(budget.snapshot().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn queue_read_budget_empty_response_releases_all_reserved_bytes() {
        let budget = Arc::new(QueueReadBudget::new(16, 16));
        let (_, mut reservation) = budget.reserve(2, 8).await.unwrap();
        reservation.observe_entries(&[], 8);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.permits.available_permits(), 16);
        drop(reservation);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn queue_read_budget_concurrent_shrink_and_drop_preserve_shared_capacity() {
        let budget = Arc::new(QueueReadBudget::new(64, 16));
        let barrier = Arc::new(tokio::sync::Barrier::new(16));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let budget = Arc::clone(&budget);
            let barrier = Arc::clone(&barrier);
            tasks.spawn(async move {
                barrier.wait().await;
                for _ in 0..16 {
                    let (count, mut reservation) = budget.reserve(128, 8).await.unwrap();
                    assert_eq!(count, 2);
                    assert!(budget.snapshot().reserved_bytes <= 64);
                    reservation.observe_entries(
                        &[RuntimeQueueEntry {
                            id: "1-0".to_string(),
                            fields: BTreeMap::from([(
                                "payload".to_string(),
                                "12345678".to_string(),
                            )]),
                        }],
                        8,
                    );
                    tokio::task::yield_now().await;
                    drop(reservation);
                }
            });
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while let Some(result) = tasks.join_next().await {
                result.expect("reservation task");
            }
        })
        .await
        .expect("all shared reservations complete");
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.snapshot().waiters, 0);
        assert_eq!(budget.snapshot().actual_field_bytes_total, 16 * 16 * 15);
        assert_eq!(budget.permits.available_permits(), 64);
    }

    #[tokio::test]
    async fn queue_read_budget_large_values_cannot_overflow_or_wait_for_impossible_permits() {
        let budget = Arc::new(QueueReadBudget::new(usize::MAX, usize::MAX));
        let maximum = maximum_budget_bytes();
        assert_eq!(budget.snapshot().limit_bytes, maximum);
        let (count, reservation) = budget.reserve(usize::MAX, 1).await.unwrap();
        assert_eq!(count, maximum);
        assert_eq!(budget.snapshot().reserved_bytes, maximum);
        drop(reservation);
        assert!(matches!(
            budget.reserve(1, maximum + 1).await,
            Err(DataLayerError::InvalidConfiguration(_))
        ));
        assert!(matches!(
            budget.reserve(1, 0).await,
            Err(DataLayerError::InvalidConfiguration(_))
        ));
        let small_batch = Arc::new(QueueReadBudget::new(32, 4));
        let (count, reservation) = small_batch.reserve(usize::MAX, 16).await.unwrap();
        assert_eq!(count, 1);
        assert_eq!(small_batch.snapshot().reserved_bytes, 16);
        drop(reservation);
    }

    #[test]
    fn queue_read_budget_env_uses_positive_defaults_and_caps_extreme_values() {
        for raw in [None, Some(""), Some("0"), Some("-1"), Some("bad")] {
            assert_eq!(configured_bytes(raw, 128), 128);
        }
        assert_eq!(configured_bytes(Some(" 42 "), 128), 42);
        assert_eq!(
            configured_bytes(Some(&u128::MAX.to_string()), 128),
            maximum_budget_bytes()
        );
    }
}
