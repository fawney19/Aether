use super::*;
use crate::dead_letter_encoding::DeadLetterEncodingBudget;
use crate::worker::UsageWorkerObservation;
use aether_runtime_state::RuntimeQueueTransferOutcome;

struct TransferProbe {
    inner: RuntimeState,
    native: bool,
    fail_write: AtomicBool,
    lose_reply: AtomicBool,
    ack_calls: Mutex<Vec<Vec<String>>>,
    append_calls: AtomicUsize,
}

impl TransferProbe {
    fn new(native: bool) -> Self {
        Self {
            inner: RuntimeState::memory(MemoryRuntimeStateConfig::default()),
            native,
            fail_write: AtomicBool::new(false),
            lose_reply: AtomicBool::new(false),
            ack_calls: Mutex::new(Vec::new()),
            append_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl RuntimeQueueStore for TransferProbe {
    async fn ensure_consumer_group(
        &self,
        stream: &str,
        group: &str,
        start_id: &str,
    ) -> Result<(), DataLayerError> {
        self.inner
            .ensure_consumer_group(stream, group, start_id)
            .await
    }

    async fn append_fields_with_maxlen(
        &self,
        stream: &str,
        fields: &BTreeMap<String, String>,
        maxlen: Option<usize>,
    ) -> Result<String, DataLayerError> {
        self.append_calls.fetch_add(1, Ordering::Relaxed);
        if self.fail_write.load(Ordering::Acquire) {
            return Err(DataLayerError::TimedOut("test append failure".to_string()));
        }
        self.inner
            .append_fields_with_maxlen(stream, fields, maxlen)
            .await
    }

    async fn try_transfer_pending_to_stream(
        &self,
        source: &str,
        group: &str,
        entry_id: &str,
        destination: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<Option<RuntimeQueueTransferOutcome>, DataLayerError> {
        if !self.native {
            return Ok(None);
        }
        if self.fail_write.load(Ordering::Acquire) {
            return Err(DataLayerError::TimedOut(
                "test atomic transfer failure".to_string(),
            ));
        }
        let result = self
            .inner
            .try_transfer_pending_to_stream(source, group, entry_id, destination, fields)
            .await?;
        if self.lose_reply.swap(false, Ordering::AcqRel) {
            return Err(DataLayerError::TimedOut(
                "test committed transfer reply lost".to_string(),
            ));
        }
        Ok(result)
    }

    async fn read_group(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        count: usize,
        block_ms: Option<u64>,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        self.inner
            .read_group(stream, group, consumer, count, block_ms)
            .await
    }

    async fn claim_stale(
        &self,
        stream: &str,
        group: &str,
        consumer: &str,
        start_id: &str,
        config: RuntimeQueueReclaimConfig,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        self.inner
            .claim_stale(stream, group, consumer, start_id, config)
            .await
    }

    async fn ack(
        &self,
        stream: &str,
        group: &str,
        ids: &[String],
    ) -> Result<usize, DataLayerError> {
        self.ack_calls.lock().expect("ack calls").push(ids.to_vec());
        self.inner.ack(stream, group, ids).await
    }

    async fn delete(&self, stream: &str, ids: &[String]) -> Result<usize, DataLayerError> {
        self.inner.delete(stream, ids).await
    }

    async fn stats(
        &self,
        stream: &str,
        group: Option<&str>,
    ) -> Result<RuntimeQueueStats, DataLayerError> {
        self.inner.stats(stream, group).await
    }
}

async fn transfer_worker(
    native: bool,
) -> (
    Arc<TransferProbe>,
    UsageQueueWorker,
    Arc<SelectiveFailingRecorder>,
    tokio::sync::mpsc::Receiver<UsageWorkerObservation>,
) {
    let runner = Arc::new(TransferProbe::new(native));
    let recorder = Arc::new(SelectiveFailingRecorder::default());
    let (telemetry, observations) = tokio::sync::mpsc::channel(32);
    let mut worker = UsageQueueWorker::new(
        runner.clone(),
        recorder.clone(),
        UsageRuntimeConfig {
            enabled: true,
            consumer_batch_size: 10,
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        },
        None,
    )
    .expect("worker")
    .with_supervisor(UsageWorkerControl::default(), telemetry);
    worker.queue =
        worker
            .queue
            .with_dead_letter_encoding_budget(Arc::new(DeadLetterEncodingBudget::new(
                64 * 1024 * 1024,
                4,
            )));
    worker.queue.ensure_consumer_group().await.expect("group");
    (runner, worker, recorder, observations)
}

async fn malformed_entries(
    runner: &TransferProbe,
    worker: &UsageQueueWorker,
) -> Vec<RuntimeQueueEntry> {
    runner
        .inner
        .append_fields_with_maxlen(
            &worker.config.stream_key,
            &BTreeMap::from([
                ("payload".to_string(), "malformed\u{0000}\n\"\\".to_string()),
                (
                    "legacy".to_string(),
                    "preserve all original fields".to_string(),
                ),
            ]),
            None,
        )
        .await
        .expect("raw append");
    worker
        .queue
        .read_group(&worker.consumer)
        .await
        .expect("read")
}

fn observed_totals(
    observations: &mut tokio::sync::mpsc::Receiver<UsageWorkerObservation>,
) -> (usize, usize) {
    let mut totals = (0, 0);
    while let Ok(observation) = observations.try_recv() {
        totals.0 += observation.acked_entries;
        totals.1 += observation.dead_lettered_entries;
    }
    totals
}

#[tokio::test]
async fn native_transfer_replay_does_not_append_or_ack_twice() {
    let (runner, worker, recorder, mut observations) = transfer_worker(true).await;
    let entries = malformed_entries(&runner, &worker).await;
    worker
        .process_entries(entries.clone())
        .await
        .expect("transfer");
    worker.process_entries(entries).await.expect("stale replay");
    assert_eq!(worker.queue.stats().await.expect("stats").group_pending, 0);
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        1
    );
    assert_eq!(observed_totals(&mut observations), (1, 1));
    assert!(runner.ack_calls.lock().expect("acks").is_empty());
    assert_eq!(runner.append_calls.load(Ordering::Relaxed), 0);
    assert!(recorder.calls.lock().expect("record calls").is_empty());
}

#[tokio::test]
async fn committed_transfer_lost_reply_retries_without_duplicate_or_false_metrics() {
    let (runner, worker, _, mut observations) = transfer_worker(true).await;
    let entries = malformed_entries(&runner, &worker).await;
    runner.lose_reply.store(true, Ordering::Release);
    assert!(matches!(
        worker.process_entries(entries.clone()).await,
        Err(DataLayerError::TimedOut(_))
    ));
    worker.process_entries(entries).await.expect("retry");
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        1
    );
    assert_eq!(worker.queue.stats().await.expect("stats").stream_length, 0);
    assert_eq!(observed_totals(&mut observations), (0, 0));
    assert!(runner.ack_calls.lock().expect("acks").is_empty());
    assert_eq!(runner.append_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn source_no_longer_pending_is_not_reported_as_archived_or_deleted() {
    let (runner, worker, _, mut observations) = transfer_worker(true).await;
    let entries = malformed_entries(&runner, &worker).await;
    runner
        .inner
        .ack(
            &worker.config.stream_key,
            &worker.config.consumer_group,
            &[entries[0].id.clone()],
        )
        .await
        .expect("external ack");
    worker
        .process_entries(entries)
        .await
        .expect("no longer pending");
    assert_eq!(worker.queue.stats().await.expect("stats").stream_length, 1);
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        0
    );
    assert_eq!(observed_totals(&mut observations), (0, 0));
    assert!(runner.ack_calls.lock().expect("acks").is_empty());
}

#[tokio::test]
async fn failed_transfer_acknowledges_successful_prefix_and_preserves_suffix_for_retry() {
    let (runner, worker, recorder, mut observations) = transfer_worker(true).await;
    for request_id in ["prefix", "req-worker-poison", "suffix"] {
        let mut event = sample_event();
        event.request_id = request_id.to_string();
        worker.queue.enqueue(&event).await.expect("enqueue");
    }
    let entries = worker
        .queue
        .read_group(&worker.consumer)
        .await
        .expect("read batch");
    let retry = entries[1..].to_vec();
    let prefix_id = entries[0].id.clone();
    runner.fail_write.store(true, Ordering::Release);
    assert!(worker.process_entries(entries).await.is_err());
    assert_eq!(
        recorder.calls.lock().expect("calls").as_slice(),
        ["prefix", "req-worker-poison"]
    );
    assert_eq!(
        *runner.ack_calls.lock().expect("acks"),
        vec![vec![prefix_id]]
    );
    assert_eq!(worker.queue.stats().await.expect("stats").group_pending, 2);
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        0
    );
    assert_eq!(observed_totals(&mut observations), (1, 0));
    runner.fail_write.store(false, Ordering::Release);
    worker.process_entries(retry).await.expect("retry suffix");
    assert_eq!(worker.queue.stats().await.expect("stats").stream_length, 0);
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        1
    );
    assert_eq!(observed_totals(&mut observations), (2, 1));
    assert_eq!(runner.append_calls.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn legacy_transfer_fallback_only_acknowledges_after_append_succeeds() {
    let (runner, worker, _, mut observations) = transfer_worker(false).await;
    let entries = malformed_entries(&runner, &worker).await;
    runner.fail_write.store(true, Ordering::Release);
    assert!(worker.process_entries(entries.clone()).await.is_err());
    assert!(runner.ack_calls.lock().expect("acks").is_empty());
    assert_eq!(worker.queue.stats().await.expect("stats").group_pending, 1);
    assert_eq!(observed_totals(&mut observations), (0, 0));
    runner.fail_write.store(false, Ordering::Release);
    worker.process_entries(entries).await.expect("legacy retry");
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        1
    );
    assert_eq!(worker.queue.stats().await.expect("stats").stream_length, 0);
    assert_eq!(runner.ack_calls.lock().expect("acks").len(), 1);
    assert_eq!(observed_totals(&mut observations), (1, 1));
}

#[tokio::test]
async fn encoding_rejection_preserves_pending_original_until_budget_allows_retry() {
    let (runner, mut worker, _, mut observations) = transfer_worker(true).await;
    let small = Arc::new(DeadLetterEncodingBudget::new(1, 1));
    worker.queue = worker.queue.with_dead_letter_encoding_budget(small.clone());
    let entries = malformed_entries(&runner, &worker).await;
    let original = entries[0].fields.clone();
    assert!(worker.process_entries(entries.clone()).await.is_err());
    assert_eq!(small.snapshot().reserved_bytes, 0);
    assert_eq!(small.snapshot().active_jobs, 0);
    assert_eq!(worker.queue.stats().await.expect("stats").group_pending, 1);
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        0
    );
    assert_eq!(observed_totals(&mut observations), (0, 0));
    worker.queue = worker
        .queue
        .with_dead_letter_encoding_budget(Arc::new(DeadLetterEncodingBudget::new(64 * 1024, 1)));
    worker
        .process_entries(entries)
        .await
        .expect("retry with capacity");
    runner
        .inner
        .ensure_consumer_group(&worker.config.dlq_stream_key, "inspect", "0-0")
        .await
        .expect("dlq group");
    let dlq = runner
        .inner
        .read_group(
            &worker.config.dlq_stream_key,
            "inspect",
            "inspector",
            1,
            Some(1),
        )
        .await
        .expect("dlq read");
    let payload: serde_json::Value =
        serde_json::from_str(&dlq[0].fields["payload"]).expect("wire JSON");
    assert_eq!(
        payload["fields"],
        serde_json::to_value(original).expect("original JSON")
    );
    assert_eq!(observed_totals(&mut observations), (1, 1));
}

#[tokio::test]
async fn normal_record_replay_reports_actual_ack_count() {
    let (_, worker, _, mut observations) = transfer_worker(true).await;
    worker
        .queue
        .enqueue(&sample_event())
        .await
        .expect("enqueue");
    let entries = worker
        .queue
        .read_group(&worker.consumer)
        .await
        .expect("read");
    worker
        .process_entries(entries.clone())
        .await
        .expect("first record");
    worker.process_entries(entries).await.expect("replay");
    assert_eq!(observed_totals(&mut observations), (1, 0));
}

#[tokio::test]
async fn oversized_dead_letter_does_not_block_healthy_entries_in_the_same_batch() {
    let (runner, mut worker, recorder, mut observations) = transfer_worker(true).await;
    let bad = malformed_entries(&runner, &worker).await;
    for request_id in ["healthy-first", "healthy-second"] {
        let mut event = sample_event();
        event.request_id = request_id.to_string();
        worker.queue.enqueue(&event).await.expect("healthy enqueue");
    }
    let mut entries = bad.clone();
    entries.extend(
        worker
            .queue
            .read_group(&worker.consumer)
            .await
            .expect("healthy read"),
    );
    worker.queue = worker
        .queue
        .with_dead_letter_encoding_budget(Arc::new(DeadLetterEncodingBudget::new(1, 1)));
    assert!(worker.process_entries(entries).await.is_err());
    assert_eq!(
        recorder.calls.lock().expect("calls").as_slice(),
        ["healthy-first", "healthy-second"]
    );
    let stats = worker.queue.stats().await.expect("stats");
    assert_eq!((stats.group_pending, stats.stream_length), (1, 1));
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        0
    );
    assert_eq!(observed_totals(&mut observations), (2, 0));
    assert!(worker.process_entries(bad.clone()).await.is_err());
    assert_eq!(observed_totals(&mut observations), (0, 0));
    worker.queue = worker
        .queue
        .with_dead_letter_encoding_budget(Arc::new(DeadLetterEncodingBudget::new(64 * 1024, 1)));
    worker
        .process_entries(bad)
        .await
        .expect("archive after raising budget");
    assert_eq!(worker.queue.stats().await.expect("stats").group_pending, 0);
    assert_eq!(
        worker.queue.dlq_stats().await.expect("dlq").stream_length,
        1
    );
    assert_eq!(observed_totals(&mut observations), (1, 1));
}
