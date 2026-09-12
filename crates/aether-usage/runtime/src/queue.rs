use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use aether_data_contracts::DataLayerError;
use aether_runtime_state::{
    RuntimeQueueEntry, RuntimeQueueReclaimConfig, RuntimeQueueReclaimPage, RuntimeQueueStats,
    RuntimeQueueStore, RuntimeQueueTransferOutcome,
};

use super::config::UsageRuntimeConfig;
use super::event::{EncodedUsageEvent, UsageEvent};
use crate::dead_letter_encoding::{shared_dead_letter_encoding_budget, DeadLetterEncodingBudget};
use crate::queue_read_budget::{shared_queue_read_budget, QueueReadBudget, QueueReadReservation};

static PAYLOAD_DOWNGRADED_TOTAL: AtomicU64 = AtomicU64::new(0);
static PAYLOAD_REJECTED_TOTAL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn payload_encoding_totals() -> (u64, u64) {
    (
        PAYLOAD_DOWNGRADED_TOTAL.load(Ordering::Relaxed),
        PAYLOAD_REJECTED_TOTAL.load(Ordering::Relaxed),
    )
}

pub(crate) fn is_permanent_enqueue_error(error: &DataLayerError) -> bool {
    matches!(error, DataLayerError::InvalidInput(_))
}

#[derive(Clone)]
pub struct UsageQueue {
    runner: Arc<dyn RuntimeQueueStore>,
    config: UsageRuntimeConfig,
    stream: String,
    group: String,
    dlq_stream: String,
    read_budget: Arc<QueueReadBudget>,
    dead_letter_encoding_budget: Arc<DeadLetterEncodingBudget>,
}

#[derive(Debug)]
pub(crate) enum UsageDeadLetterOutcome {
    Transferred {
        destination_id: String,
        acked: usize,
    },
    Appended {
        destination_id: String,
    },
    NotPending,
    EncodingDeferred {
        error: DataLayerError,
    },
}

pub(crate) struct ReservedUsageReadBatch {
    pub(crate) entries: Vec<RuntimeQueueEntry>,
    pub(crate) requested_count: usize,
    // Keep last: batch data must be dropped before its reservation is returned.
    pub(crate) reservation: QueueReadReservation,
}

pub(crate) struct ReservedUsageReclaimPage {
    pub(crate) page: RuntimeQueueReclaimPage,
    pub(crate) reservation: QueueReadReservation,
}

impl UsageQueue {
    pub fn new(
        runner: Arc<dyn RuntimeQueueStore>,
        config: UsageRuntimeConfig,
    ) -> Result<Self, DataLayerError> {
        config.validate()?;
        Ok(Self {
            runner,
            stream: config.stream_key.clone(),
            group: config.consumer_group.clone(),
            dlq_stream: config.dlq_stream_key.clone(),
            config,
            read_budget: shared_queue_read_budget(),
            dead_letter_encoding_budget: shared_dead_letter_encoding_budget(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_read_budget(mut self, read_budget: Arc<QueueReadBudget>) -> Self {
        self.read_budget = read_budget;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_dead_letter_encoding_budget(
        mut self,
        budget: Arc<DeadLetterEncodingBudget>,
    ) -> Self {
        self.dead_letter_encoding_budget = budget;
        self
    }

    pub async fn ensure_consumer_group(&self) -> Result<(), DataLayerError> {
        self.runner
            .ensure_consumer_group(&self.stream, &self.group, "0-0")
            .await
    }

    pub async fn enqueue(&self, event: &UsageEvent) -> Result<String, DataLayerError> {
        let encoded = self.encode_event(event)?;
        self.runner
            .append_fields_with_maxlen(
                &self.stream,
                &encoded.fields,
                Some(self.config.stream_maxlen),
            )
            .await
    }

    pub(crate) fn validate_event(&self, event: &UsageEvent) -> Result<(), DataLayerError> {
        self.encode_event(event).map(|_| ())
    }

    fn encode_event(&self, event: &UsageEvent) -> Result<EncodedUsageEvent, DataLayerError> {
        let encoded = match event.to_bounded_stream_fields(self.config.queue_payload_max_bytes) {
            Ok(encoded) => encoded,
            Err(error) => {
                if is_permanent_enqueue_error(&error) {
                    PAYLOAD_REJECTED_TOTAL.fetch_add(1, Ordering::Relaxed);
                }
                return Err(error);
            }
        };
        if encoded.diagnostics_omitted {
            PAYLOAD_DOWNGRADED_TOTAL.fetch_add(1, Ordering::Relaxed);
        }
        Ok(encoded)
    }

    pub async fn read_group(
        &self,
        consumer: &str,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        self.runner
            .read_group(
                &self.stream,
                &self.group,
                consumer,
                self.config.consumer_batch_size.max(1),
                Some(self.config.consumer_block_ms.max(1)),
            )
            .await
    }

    /// Workers retain this lease through processing. The public Vec API remains
    /// compatible, but cannot preserve a reservation after returning its entries.
    pub(crate) async fn read_group_reserved(
        &self,
        consumer: &str,
    ) -> Result<ReservedUsageReadBatch, DataLayerError> {
        let (requested_count, mut reservation) = self
            .read_budget
            .reserve(
                self.config.consumer_batch_size,
                self.config.queue_payload_max_bytes,
            )
            .await?;
        let entries = self
            .runner
            .read_group(
                &self.stream,
                &self.group,
                consumer,
                requested_count,
                Some(self.config.consumer_block_ms.max(1)),
            )
            .await?;
        reservation.observe_entries(&entries, self.config.queue_payload_max_bytes);
        Ok(ReservedUsageReadBatch {
            entries,
            requested_count,
            reservation,
        })
    }

    pub async fn claim_stale(
        &self,
        consumer: &str,
        start_id: &str,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        Ok(self.claim_stale_page(consumer, start_id).await?.entries)
    }

    pub async fn claim_stale_page(
        &self,
        consumer: &str,
        start_id: &str,
    ) -> Result<RuntimeQueueReclaimPage, DataLayerError> {
        self.runner
            .claim_stale_page(
                &self.stream,
                &self.group,
                consumer,
                start_id,
                RuntimeQueueReclaimConfig {
                    min_idle_ms: self.config.reclaim_idle_ms,
                    count: self.config.reclaim_count,
                },
            )
            .await
    }

    pub(crate) async fn claim_stale_page_reserved(
        &self,
        consumer: &str,
        start_id: &str,
    ) -> Result<ReservedUsageReclaimPage, DataLayerError> {
        let (requested_count, mut reservation) = self
            .read_budget
            .reserve(
                self.config.reclaim_count,
                self.config.queue_payload_max_bytes,
            )
            .await?;
        let page = self
            .runner
            .claim_stale_page(
                &self.stream,
                &self.group,
                consumer,
                start_id,
                RuntimeQueueReclaimConfig {
                    min_idle_ms: self.config.reclaim_idle_ms,
                    count: requested_count,
                },
            )
            .await?;
        reservation.observe_entries(&page.entries, self.config.queue_payload_max_bytes);
        Ok(ReservedUsageReclaimPage { page, reservation })
    }

    pub async fn ack_and_delete(&self, ids: &[String]) -> Result<(), DataLayerError> {
        self.ack_and_delete_counted(ids).await.map(|_| ())
    }

    pub(crate) async fn ack_and_delete_counted(
        &self,
        ids: &[String],
    ) -> Result<usize, DataLayerError> {
        let acked = self.runner.ack(&self.stream, &self.group, ids).await?;
        self.runner.delete(&self.stream, ids).await?;
        Ok(acked)
    }

    pub async fn push_dead_letter(
        &self,
        entry: &RuntimeQueueEntry,
        error: &str,
    ) -> Result<String, DataLayerError> {
        let reservation = self.dead_letter_encoding_budget.try_reserve(entry, error)?;
        let encoded = reservation
            .encode_owned(entry.clone(), error.to_string())
            .await?;
        self.runner
            .append_fields_with_maxlen(&self.dlq_stream, &encoded.fields, None)
            .await
    }

    pub(crate) async fn transfer_dead_letter_owned(
        &self,
        entry: RuntimeQueueEntry,
        error: String,
    ) -> Result<UsageDeadLetterOutcome, DataLayerError> {
        let reservation = match self.dead_letter_encoding_budget.try_reserve(&entry, &error) {
            Ok(reservation) => reservation,
            Err(error) => return Ok(UsageDeadLetterOutcome::EncodingDeferred { error }),
        };
        let encoded = match reservation.encode_owned(entry, error).await {
            Ok(encoded) => encoded,
            Err(error) => return Ok(UsageDeadLetterOutcome::EncodingDeferred { error }),
        };
        match self
            .runner
            .try_transfer_pending_to_stream(
                &self.stream,
                &self.group,
                &encoded.entry_id,
                &self.dlq_stream,
                &encoded.fields,
            )
            .await?
        {
            Some(RuntimeQueueTransferOutcome::Transferred {
                destination_id,
                acked,
                ..
            }) => Ok(UsageDeadLetterOutcome::Transferred {
                destination_id,
                acked,
            }),
            Some(RuntimeQueueTransferOutcome::NotPending) => Ok(UsageDeadLetterOutcome::NotPending),
            None => Ok(UsageDeadLetterOutcome::Appended {
                destination_id: self
                    .runner
                    .append_fields_with_maxlen(&self.dlq_stream, &encoded.fields, None)
                    .await?,
            }),
        }
    }

    pub async fn stats(&self) -> Result<RuntimeQueueStats, DataLayerError> {
        self.runner.stats(&self.stream, Some(&self.group)).await
    }

    pub async fn dlq_stats(&self) -> Result<RuntimeQueueStats, DataLayerError> {
        self.runner.stats(&self.dlq_stream, None).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
struct UsageQueueRuntimeSettings {
    command_timeout_ms: Option<u64>,
    read_block_ms: Option<u64>,
    read_count: usize,
}

#[cfg(test)]
fn usage_queue_runtime_settings(config: &UsageRuntimeConfig) -> UsageQueueRuntimeSettings {
    let read_block_ms = config.consumer_block_ms.max(1);
    let command_timeout_ms = read_block_ms.saturating_add(2_000).max(5_000);
    UsageQueueRuntimeSettings {
        command_timeout_ms: Some(command_timeout_ms),
        read_block_ms: Some(read_block_ms),
        read_count: config.consumer_batch_size.max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        usage_queue_runtime_settings, UsageDeadLetterOutcome, UsageQueue, UsageQueueRuntimeSettings,
    };
    use crate::dead_letter_encoding::DeadLetterEncodingBudget;
    use crate::queue_read_budget::QueueReadBudget;
    use crate::UsageRuntimeConfig;
    use aether_data_contracts::DataLayerError;
    use aether_runtime_state::{
        MemoryRuntimeStateConfig, RuntimeQueueEntry, RuntimeQueueReclaimConfig, RuntimeQueueStats,
        RuntimeQueueStore, RuntimeQueueTransferOutcome, RuntimeState,
    };
    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::Arc;
    use std::task::Poll;
    use std::time::Duration;

    struct HeldDeadLetterStore {
        started: tokio::sync::Notify,
    }

    #[async_trait]
    impl RuntimeQueueStore for HeldDeadLetterStore {
        async fn ensure_consumer_group(
            &self,
            _stream: &str,
            _group: &str,
            _start_id: &str,
        ) -> Result<(), DataLayerError> {
            unreachable!("store only exercises dead-letter writes")
        }

        async fn append_fields_with_maxlen(
            &self,
            _stream: &str,
            fields: &BTreeMap<String, String>,
            _maxlen: Option<usize>,
        ) -> Result<String, DataLayerError> {
            assert!(fields.contains_key("payload"));
            self.started.notify_one();
            std::future::pending().await
        }

        async fn read_group(
            &self,
            _stream: &str,
            _group: &str,
            _consumer: &str,
            _count: usize,
            _block_ms: Option<u64>,
        ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
            unreachable!("store only exercises dead-letter writes")
        }

        async fn claim_stale(
            &self,
            _stream: &str,
            _group: &str,
            _consumer: &str,
            _start_id: &str,
            _config: RuntimeQueueReclaimConfig,
        ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
            unreachable!("store only exercises dead-letter writes")
        }

        async fn try_transfer_pending_to_stream(
            &self,
            _source: &str,
            _group: &str,
            _entry_id: &str,
            _destination: &str,
            fields: &BTreeMap<String, String>,
        ) -> Result<Option<RuntimeQueueTransferOutcome>, DataLayerError> {
            assert!(fields.contains_key("payload"));
            self.started.notify_one();
            std::future::pending().await
        }

        async fn ack(
            &self,
            _stream: &str,
            _group: &str,
            _ids: &[String],
        ) -> Result<usize, DataLayerError> {
            unreachable!("store only exercises dead-letter writes")
        }

        async fn delete(&self, _stream: &str, _ids: &[String]) -> Result<usize, DataLayerError> {
            unreachable!("store only exercises dead-letter writes")
        }

        async fn stats(
            &self,
            _stream: &str,
            _group: Option<&str>,
        ) -> Result<RuntimeQueueStats, DataLayerError> {
            unreachable!("store only exercises dead-letter writes")
        }
    }

    #[tokio::test]
    async fn dead_letter_encoding_storage_wait_keeps_budget_until_public_or_owned_call_is_cancelled(
    ) {
        for owned_transfer in [false, true] {
            let store = Arc::new(HeldDeadLetterStore {
                started: tokio::sync::Notify::new(),
            });
            let budget = Arc::new(DeadLetterEncodingBudget::new(4096, 1));
            let queue = UsageQueue::new(store.clone(), UsageRuntimeConfig::default())
                .unwrap()
                .with_dead_letter_encoding_budget(Arc::clone(&budget));
            let entry = RuntimeQueueEntry {
                id: "1-0".to_string(),
                fields: BTreeMap::from([("payload".to_string(), "original fields".to_string())]),
            };
            let task = tokio::spawn(async move {
                if owned_transfer {
                    queue
                        .transfer_dead_letter_owned(entry, "failure".to_string())
                        .await
                        .map(|_| ())
                } else {
                    queue.push_dead_letter(&entry, "failure").await.map(|_| ())
                }
            });
            tokio::time::timeout(Duration::from_secs(2), store.started.notified())
                .await
                .unwrap();
            assert_eq!(budget.snapshot().encoded_total, 1);
            assert_eq!(budget.snapshot().active_jobs, 1);
            assert!(budget.snapshot().reserved_bytes > 0);
            task.abort();
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
            assert_eq!(budget.snapshot().active_jobs, 0);
            assert_eq!(budget.snapshot().reserved_bytes, 0);
        }
    }

    #[tokio::test]
    async fn dead_letter_encoding_owned_defers_oversize_but_preserves_public_and_store_errors() {
        let runner = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(runner, UsageRuntimeConfig::default())
            .unwrap()
            .with_dead_letter_encoding_budget(Arc::new(DeadLetterEncodingBudget::new(1, 1)));
        let entry = RuntimeQueueEntry {
            id: "1-0".to_string(),
            fields: BTreeMap::from([("payload".to_string(), "original fields".to_string())]),
        };
        assert!(matches!(
            queue
                .transfer_dead_letter_owned(entry.clone(), "failure".to_string())
                .await,
            Ok(UsageDeadLetterOutcome::EncodingDeferred {
                error: DataLayerError::InvalidInput(_),
            })
        ));
        assert!(matches!(
            queue.push_dead_letter(&entry, "failure").await,
            Err(DataLayerError::InvalidInput(_))
        ));

        let budget = Arc::new(DeadLetterEncodingBudget::new(4096, 1));
        let queue = queue.with_dead_letter_encoding_budget(Arc::clone(&budget));
        // The absent source causes a native store error after successful encoding.
        assert!(matches!(
            queue
                .transfer_dead_letter_owned(entry, "failure".to_string())
                .await,
            Err(DataLayerError::InvalidInput(_))
        ));
        assert_eq!(budget.snapshot().encoded_total, 1);
        assert_eq!(budget.snapshot().active_jobs, 0);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(queue.dlq_stats().await.unwrap().stream_length, 0);
    }

    #[tokio::test]
    async fn dead_letter_encoding_owned_defers_capacity_without_starting_store_work() {
        let store = Arc::new(HeldDeadLetterStore {
            started: tokio::sync::Notify::new(),
        });
        let budget = Arc::new(DeadLetterEncodingBudget::new(4096, 1));
        let queue = UsageQueue::new(store, UsageRuntimeConfig::default())
            .unwrap()
            .with_dead_letter_encoding_budget(Arc::clone(&budget));
        let entry = RuntimeQueueEntry {
            id: "1-0".to_string(),
            fields: BTreeMap::from([("payload".to_string(), "original fields".to_string())]),
        };
        let held = budget.try_reserve(&entry, "failure").unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            queue.transfer_dead_letter_owned(entry, "failure".to_string()),
        )
        .await
        .unwrap();
        assert!(matches!(
            result,
            Ok(UsageDeadLetterOutcome::EncodingDeferred {
                error: DataLayerError::TimedOut(_),
            })
        ));
        assert_eq!(budget.snapshot().encoded_total, 0);
        assert_eq!(budget.snapshot().active_jobs, 1);
        assert_eq!(budget.snapshot().capacity_rejected_total, 1);
        drop(held);
        assert_eq!(budget.snapshot().active_jobs, 0);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
    }

    #[test]
    fn usage_queue_applies_runtime_block_and_batch_settings() {
        let config = UsageRuntimeConfig {
            enabled: true,
            consumer_block_ms: 750,
            consumer_batch_size: 123,
            ..UsageRuntimeConfig::default()
        };
        let queue = UsageQueue::new(
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
            config,
        )
        .expect("usage queue should build from runtime config");

        assert_eq!(
            usage_queue_runtime_settings(&queue.config),
            UsageQueueRuntimeSettings {
                command_timeout_ms: Some(5_000),
                read_block_ms: Some(750),
                read_count: 123,
            }
        );
    }

    fn reserved_test_queue(runner: Arc<RuntimeState>, budget: Arc<QueueReadBudget>) -> UsageQueue {
        UsageQueue::new(
            runner,
            UsageRuntimeConfig {
                enabled: true,
                queue_payload_max_bytes: 8,
                consumer_batch_size: 128,
                consumer_block_ms: 1,
                reclaim_count: 128,
                reclaim_idle_ms: 1,
                ..UsageRuntimeConfig::default()
            },
        )
        .expect("test queue")
        .with_read_budget(budget)
    }

    #[tokio::test]
    async fn queue_read_budget_read_and_reclaim_share_a_reservation_across_clones() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let budget = Arc::new(QueueReadBudget::new(16, 16));
        let queue = reserved_test_queue(Arc::clone(&runtime), Arc::clone(&budget));
        let other = queue.clone();
        queue.ensure_consumer_group().await.unwrap();
        for _ in 0..6 {
            runtime
                .append_fields_with_maxlen(
                    &queue.stream,
                    &BTreeMap::from([("payload".to_string(), "12345678".to_string())]),
                    None,
                )
                .await
                .unwrap();
        }
        let first = queue.read_group_reserved("reader").await.unwrap();
        assert_eq!(first.requested_count, 2);
        assert_eq!(first.entries.len(), 2);
        let first_ids = first
            .entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        let extra_pending = runtime
            .read_group(&queue.stream, &queue.group, "previous-reader", 2, None)
            .await
            .unwrap();
        assert_eq!(extra_pending.len(), 2);
        let next_cursor = extra_pending[0].id.clone();
        drop(extra_pending);
        assert_eq!(budget.snapshot().reserved_bytes, 16);

        let mut reclaim = Box::pin(other.claim_stale_page_reserved("reclaimer", "0-0"));
        std::future::poll_fn(|cx| {
            assert!(reclaim.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(budget.snapshot().waiters, 1);
        tokio::time::sleep(Duration::from_millis(5)).await;
        drop(first);
        let claimed = reclaim.await.unwrap();
        assert_eq!(claimed.page.entries.len(), 2);
        assert_eq!(claimed.page.next_start_id, next_cursor);
        assert_eq!(
            claimed
                .page
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            first_ids
        );
        assert_eq!(budget.snapshot().reserved_bytes, 16);
        assert_eq!(budget.snapshot().waiters, 0);

        let mut next_read = Box::pin(queue.read_group_reserved("reader"));
        std::future::poll_fn(|cx| {
            assert!(next_read.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        drop(claimed);
        let next = next_read.await.unwrap();
        assert_eq!(next.entries.len(), 2);
        assert!(next
            .entries
            .iter()
            .all(|entry| !first_ids.contains(&entry.id)));
        drop(next);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.snapshot().wait_total, 2);
    }

    #[tokio::test]
    async fn queue_read_budget_errors_and_empty_pages_release_reservations() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let budget = Arc::new(QueueReadBudget::new(16, 16));
        let queue = reserved_test_queue(runtime, Arc::clone(&budget));
        assert!(queue.read_group_reserved("reader").await.is_err());
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert!(queue
            .claim_stale_page_reserved("reader", "0-0")
            .await
            .is_err());
        assert_eq!(budget.snapshot().reserved_bytes, 0);

        queue.ensure_consumer_group().await.unwrap();
        let empty = queue.read_group_reserved("reader").await.unwrap();
        assert!(empty.entries.is_empty());
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        let page = queue
            .claim_stale_page_reserved("reader", "0-0")
            .await
            .unwrap();
        assert!(page.page.entries.is_empty());
        assert_eq!(page.page.next_start_id, "0-0");
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        drop((empty, page));
        assert_eq!(budget.snapshot().reserved_bytes, 0);
    }

    #[test]
    fn queue_read_budget_new_queues_and_clones_share_process_budget() {
        let runtime = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let first = UsageQueue::new(runtime.clone(), UsageRuntimeConfig::default()).unwrap();
        let second = UsageQueue::new(runtime, UsageRuntimeConfig::default()).unwrap();
        let cloned = first.clone();
        assert!(Arc::ptr_eq(&first.read_budget, &second.read_budget));
        assert!(Arc::ptr_eq(&first.read_budget, &cloned.read_budget));
    }
}
