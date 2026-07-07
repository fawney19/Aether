use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, RequestCandidateWriteRepository, UpsertRequestCandidateRecord,
};
use aether_runtime::{MetricKind, MetricSample};
use tokio::sync::mpsc;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, warn};

const MODE_ENV: &str = "AETHER_GATEWAY_REQUEST_CANDIDATE_WRITE_MODE";
const QUEUE_CAPACITY_ENV: &str = "AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_CAPACITY";
const BATCH_SIZE_ENV: &str = "AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_BATCH_SIZE";
const FLUSH_INTERVAL_MS_ENV: &str = "AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_FLUSH_INTERVAL_MS";
const WORKERS_ENV: &str = "AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_WORKERS";
const QUEUE_FULL_ENV: &str = "AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_FULL";
const DB_WRITE_CONCURRENCY_LIMIT_ENV: &str =
    "AETHER_GATEWAY_REQUEST_CANDIDATE_DB_WRITE_CONCURRENCY_LIMIT";
const DB_BATCH_SIZE_ENV: &str = "AETHER_GATEWAY_REQUEST_CANDIDATE_DB_BATCH_SIZE";

const DEFAULT_QUEUE_CAPACITY: usize = 65_536;
const DEFAULT_BATCH_SIZE: usize = 512;
const DEFAULT_DB_BATCH_SIZE: usize = 128;
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 50;
const DEFAULT_WORKERS: usize = 2;
const FAILED_FLUSH_RETRY_DELAY_MS: u64 = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestCandidateWriteMode {
    Sync,
    Async,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestCandidateQueueFullPolicy {
    Drop,
    Sync,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestCandidateQueueConfig {
    pub(crate) mode: RequestCandidateWriteMode,
    pub(crate) capacity: usize,
    pub(crate) batch_size: usize,
    pub(crate) db_batch_size: usize,
    pub(crate) flush_interval: Duration,
    pub(crate) workers: usize,
    pub(crate) db_write_concurrency_limit: Option<usize>,
    pub(crate) full_policy: RequestCandidateQueueFullPolicy,
}

impl Default for RequestCandidateQueueConfig {
    fn default() -> Self {
        Self {
            mode: RequestCandidateWriteMode::Sync,
            capacity: DEFAULT_QUEUE_CAPACITY,
            batch_size: DEFAULT_BATCH_SIZE,
            db_batch_size: DEFAULT_DB_BATCH_SIZE,
            flush_interval: Duration::from_millis(DEFAULT_FLUSH_INTERVAL_MS),
            workers: DEFAULT_WORKERS,
            db_write_concurrency_limit: None,
            full_policy: RequestCandidateQueueFullPolicy::Sync,
        }
    }
}

impl RequestCandidateQueueConfig {
    pub(crate) fn from_env() -> Self {
        let mut config = Self::default();
        config.mode = match env_string(MODE_ENV).as_deref() {
            Some("sync") | Some("inline") => RequestCandidateWriteMode::Sync,
            Some("async") | Some("queued") | Some("queue") => RequestCandidateWriteMode::Async,
            _ => RequestCandidateWriteMode::Async,
        };
        config.capacity = env_usize(QUEUE_CAPACITY_ENV, DEFAULT_QUEUE_CAPACITY).max(1);
        config.batch_size = env_usize(BATCH_SIZE_ENV, DEFAULT_BATCH_SIZE).max(1);
        config.db_batch_size = env_usize(DB_BATCH_SIZE_ENV, DEFAULT_DB_BATCH_SIZE).max(1);
        config.flush_interval =
            Duration::from_millis(env_u64(FLUSH_INTERVAL_MS_ENV, DEFAULT_FLUSH_INTERVAL_MS).max(1));
        config.workers = env_usize(WORKERS_ENV, DEFAULT_WORKERS).clamp(1, 32);
        config.db_write_concurrency_limit =
            env_optional_usize(DB_WRITE_CONCURRENCY_LIMIT_ENV).map(|limit| limit.clamp(1, 32));
        config.full_policy = match env_string(QUEUE_FULL_ENV).as_deref() {
            Some("drop") | Some("best_effort") | Some("best-effort") => {
                RequestCandidateQueueFullPolicy::Drop
            }
            Some("sync") | Some("fallback_sync") | Some("fallback-sync") => {
                RequestCandidateQueueFullPolicy::Sync
            }
            _ => RequestCandidateQueueFullPolicy::Sync,
        };
        config
    }

    pub(crate) fn async_enabled(&self) -> bool {
        matches!(self.mode, RequestCandidateWriteMode::Async)
    }
}

#[derive(Debug, Default)]
struct RequestCandidateQueueMetrics {
    queued_current: AtomicUsize,
    pending_current: AtomicUsize,
    enqueued_total: AtomicU64,
    dropped_total: AtomicU64,
    flushed_total: AtomicU64,
    flush_failed_total: AtomicU64,
    flush_batches_total: AtomicU64,
    flush_sql_ops_total: AtomicU64,
    flush_sql_records_total: AtomicU64,
    db_write_in_flight: AtomicUsize,
    db_write_max_in_flight: AtomicUsize,
    db_write_wait_total: AtomicU64,
    compacted_total: AtomicU64,
    sync_fallback_total: AtomicU64,
}

#[derive(Clone)]
pub(crate) struct RequestCandidateQueueRuntime {
    senders: Vec<mpsc::Sender<UpsertRequestCandidateRecord>>,
    repository: Arc<dyn RequestCandidateWriteRepository>,
    config: RequestCandidateQueueConfig,
    metrics: Arc<RequestCandidateQueueMetrics>,
    db_write_gate: Option<Arc<RequestCandidateDbWriteGate>>,
}

impl std::fmt::Debug for RequestCandidateQueueRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestCandidateQueueRuntime")
            .field("config", &self.config)
            .field(
                "queued_current",
                &self.metrics.queued_current.load(Ordering::Acquire),
            )
            .finish_non_exhaustive()
    }
}

impl RequestCandidateQueueRuntime {
    pub(crate) fn spawn(
        repository: Arc<dyn RequestCandidateWriteRepository>,
        mut config: RequestCandidateQueueConfig,
    ) -> Arc<Self> {
        config.workers = config.workers.min(config.capacity).max(1);
        let mut senders = Vec::with_capacity(config.workers);
        let mut receivers = Vec::with_capacity(config.workers);
        for worker_index in 0..config.workers {
            let capacity = worker_queue_capacity(config.capacity, config.workers, worker_index);
            let (sender, receiver) = mpsc::channel(capacity);
            senders.push(sender);
            receivers.push(receiver);
        }
        let db_write_gate = config
            .db_write_concurrency_limit
            .map(RequestCandidateDbWriteGate::new)
            .map(Arc::new);
        let runtime = Arc::new(Self {
            senders,
            repository,
            config,
            metrics: Arc::new(RequestCandidateQueueMetrics::default()),
            db_write_gate,
        });
        runtime.spawn_workers(receivers);
        runtime
    }

    pub(crate) async fn enqueue_or_fallback(
        &self,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), aether_data::DataLayerError> {
        let worker_index = self.worker_index_for(&record);
        let Some(sender) = self.senders.get(worker_index) else {
            self.metrics
                .sync_fallback_total
                .fetch_add(1, Ordering::AcqRel);
            return self.repository.upsert(record).await.map(|_| ());
        };
        self.metrics.queued_current.fetch_add(1, Ordering::AcqRel);
        self.metrics.pending_current.fetch_add(1, Ordering::AcqRel);
        match sender.try_send(record) {
            Ok(()) => {
                self.metrics.enqueued_total.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(record)) => {
                decrement_atomic_usize(&self.metrics.queued_current);
                decrement_atomic_usize(&self.metrics.pending_current);
                warn!(
                    event_name = "request_candidate_queue_full",
                    log_type = "event",
                    full_policy = ?self.config.full_policy,
                    worker_index,
                    queued = self.metrics.queued_current.load(Ordering::Acquire),
                    capacity = self.config.capacity,
                    "gateway request candidate async queue is full"
                );
                match self.config.full_policy {
                    RequestCandidateQueueFullPolicy::Drop => {
                        self.metrics.dropped_total.fetch_add(1, Ordering::AcqRel);
                        Ok(())
                    }
                    RequestCandidateQueueFullPolicy::Sync => {
                        self.metrics
                            .sync_fallback_total
                            .fetch_add(1, Ordering::AcqRel);
                        self.repository.upsert(record).await.map(|_| ())
                    }
                }
            }
            Err(mpsc::error::TrySendError::Closed(record)) => {
                decrement_atomic_usize(&self.metrics.queued_current);
                decrement_atomic_usize(&self.metrics.pending_current);
                self.metrics
                    .sync_fallback_total
                    .fetch_add(1, Ordering::AcqRel);
                self.repository.upsert(record).await.map(|_| ())
            }
        }
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        vec![
            MetricSample::new(
                "request_candidate_queue_depth",
                "Current number of request candidate records waiting in the async persistence queue.",
                MetricKind::Gauge,
                self.metrics.queued_current.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_pending_depth",
                "Current number of request candidate records accepted into the async persistence queue but not yet flushed.",
                MetricKind::Gauge,
                self.metrics.pending_current.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_capacity",
                "Configured request candidate async persistence queue capacity.",
                MetricKind::Gauge,
                self.config.capacity as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_enqueued_total",
                "Total request candidate records accepted into the async persistence queue.",
                MetricKind::Counter,
                self.metrics.enqueued_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_dropped_total",
                "Total request candidate records dropped because the async persistence queue was full.",
                MetricKind::Counter,
                self.metrics.dropped_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_flushed_total",
                "Total request candidate records flushed by async persistence workers.",
                MetricKind::Counter,
                self.metrics.flushed_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_flush_failed_total",
                "Total request candidate records that failed during async persistence flush.",
                MetricKind::Counter,
                self.metrics.flush_failed_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_flush_batches_total",
                "Total async request candidate persistence flush batches.",
                MetricKind::Counter,
                self.metrics.flush_batches_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_flush_sql_ops_total",
                "Total repository batch upsert operations issued by async request candidate persistence workers after compaction.",
                MetricKind::Counter,
                self.metrics.flush_sql_ops_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_flush_sql_records_total",
                "Total request candidate records submitted to repository batch upsert operations after compaction.",
                MetricKind::Counter,
                self.metrics.flush_sql_records_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_db_batch_size",
                "Maximum request candidate records submitted in one async DB batch upsert after compaction.",
                MetricKind::Gauge,
                self.config.db_batch_size as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_db_write_concurrency_limit",
                "Maximum concurrent request candidate async DB write batches; zero means unlimited.",
                MetricKind::Gauge,
                self.config.db_write_concurrency_limit.unwrap_or_default() as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_db_write_in_flight",
                "Current request candidate async DB write batches in flight.",
                MetricKind::Gauge,
                self.metrics.db_write_in_flight.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_db_write_max_in_flight",
                "Maximum observed request candidate async DB write batches in flight.",
                MetricKind::Gauge,
                self.metrics.db_write_max_in_flight.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_db_write_wait_total",
                "Total request candidate async DB write batches that had to wait for the DB write gate.",
                MetricKind::Counter,
                self.metrics.db_write_wait_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_compacted_total",
                "Total request candidate records compacted before async persistence because a later queued record covered the same request candidate slot.",
                MetricKind::Counter,
                self.metrics.compacted_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_queue_sync_fallback_total",
                "Total request candidate records synchronously persisted after async queue fallback.",
                MetricKind::Counter,
                self.metrics.sync_fallback_total.load(Ordering::Acquire),
            ),
        ]
    }

    fn spawn_workers(
        self: &Arc<Self>,
        receivers: Vec<mpsc::Receiver<UpsertRequestCandidateRecord>>,
    ) {
        for (worker_index, receiver) in receivers.into_iter().enumerate() {
            let repository = Arc::clone(&self.repository);
            let config = self.config.clone();
            let metrics = Arc::clone(&self.metrics);
            let db_write_gate = self.db_write_gate.clone();
            tokio::spawn(async move {
                run_worker(
                    repository,
                    config,
                    metrics,
                    db_write_gate,
                    worker_index,
                    receiver,
                )
                .await;
            });
        }
    }

    fn worker_index_for(&self, record: &UpsertRequestCandidateRecord) -> usize {
        let worker_count = self.senders.len();
        if worker_count <= 1 {
            return 0;
        }
        (request_candidate_slot_hash(record) % worker_count as u64) as usize
    }
}

async fn run_worker(
    repository: Arc<dyn RequestCandidateWriteRepository>,
    config: RequestCandidateQueueConfig,
    metrics: Arc<RequestCandidateQueueMetrics>,
    db_write_gate: Option<Arc<RequestCandidateDbWriteGate>>,
    worker_index: usize,
    mut receiver: mpsc::Receiver<UpsertRequestCandidateRecord>,
) {
    let mut ticker = interval(config.flush_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut batch = Vec::with_capacity(config.batch_size);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if !batch.is_empty() {
                    flush_batch(
                        &repository,
                        &config,
                        &metrics,
                        db_write_gate.as_ref(),
                        worker_index,
                        &mut batch,
                    ).await;
                }
            }
            received = receiver.recv() => {
                match received {
                    Some(record) => {
                        decrement_atomic_usize(&metrics.queued_current);
                        batch.push(record);
                        if batch.len() >= config.batch_size {
                            flush_batch(
                                &repository,
                                &config,
                                &metrics,
                                db_write_gate.as_ref(),
                                worker_index,
                                &mut batch,
                            ).await;
                        }
                    }
                    None => {
                        if !batch.is_empty() {
                            flush_batch(
                                &repository,
                                &config,
                                &metrics,
                                db_write_gate.as_ref(),
                                worker_index,
                                &mut batch,
                            ).await;
                        }
                        break;
                    }
                }
            }
        }
    }
}

async fn flush_batch(
    repository: &Arc<dyn RequestCandidateWriteRepository>,
    config: &RequestCandidateQueueConfig,
    metrics: &RequestCandidateQueueMetrics,
    db_write_gate: Option<&Arc<RequestCandidateDbWriteGate>>,
    worker_index: usize,
    batch: &mut Vec<UpsertRequestCandidateRecord>,
) {
    let records = std::mem::take(batch);
    if records.is_empty() {
        return;
    }
    let source_count = records.len();
    let records = compact_records_for_flush(records);
    let compacted = source_count.saturating_sub(records.len());
    if compacted > 0 {
        metrics
            .compacted_total
            .fetch_add(compacted as u64, Ordering::AcqRel);
    }
    metrics.flush_batches_total.fetch_add(1, Ordering::AcqRel);
    let mut failed = 0_u64;
    let mut retry_records = Vec::new();

    for chunk in records.chunks(config.db_batch_size) {
        let source_count = chunk
            .iter()
            .map(|record| record.source_count)
            .sum::<usize>();
        let record_count = chunk.len();
        let upsert_records = chunk
            .iter()
            .map(|record| record.record.clone())
            .collect::<Vec<_>>();
        metrics.flush_sql_ops_total.fetch_add(1, Ordering::AcqRel);
        metrics
            .flush_sql_records_total
            .fetch_add(record_count as u64, Ordering::AcqRel);
        let _db_write_permit = match db_write_gate {
            Some(gate) => Some(gate.acquire(metrics).await),
            None => None,
        };
        if let Err(err) = repository.upsert_many(upsert_records.clone()).await {
            failed = failed.saturating_add(source_count as u64);
            decrement_atomic_usize_by(
                &metrics.pending_current,
                source_count.saturating_sub(record_count),
            );
            warn!(
                event_name = "request_candidate_async_flush_failed",
                log_type = "event",
                worker_index,
                record_count,
                source_count,
                error = ?err,
                "gateway failed to asynchronously persist request candidate DB batch"
            );
            retry_records.extend(upsert_records);
        } else {
            metrics
                .flushed_total
                .fetch_add(source_count as u64, Ordering::AcqRel);
            decrement_atomic_usize_by(&metrics.pending_current, source_count);
        }
    }
    if failed > 0 {
        metrics
            .flush_failed_total
            .fetch_add(failed, Ordering::AcqRel);
        tokio::time::sleep(Duration::from_millis(FAILED_FLUSH_RETRY_DELAY_MS)).await;
        for record in retry_records {
            batch.push(record);
        }
    }
    debug!(
        event_name = "request_candidate_async_flush_completed",
        log_type = "event",
        worker_index,
        failed,
        "gateway completed request candidate async flush batch"
    );
}

#[derive(Debug)]
struct CompactedRequestCandidateRecord {
    record: UpsertRequestCandidateRecord,
    source_count: usize,
}

fn compact_records_for_flush(
    records: Vec<UpsertRequestCandidateRecord>,
) -> Vec<CompactedRequestCandidateRecord> {
    let mut latest_slot = HashMap::<(String, u32, u32), usize>::new();
    let mut compacted = Vec::<CompactedRequestCandidateRecord>::with_capacity(records.len());
    for record in records {
        let slot = (
            record.request_id.clone(),
            record.candidate_index,
            record.retry_index,
        );
        match latest_slot.get(&slot).copied() {
            Some(index) => {
                merge_request_candidate_record_for_flush(&mut compacted[index].record, record);
                compacted[index].source_count = compacted[index].source_count.saturating_add(1);
            }
            _ => {
                latest_slot.insert(slot, compacted.len());
                compacted.push(CompactedRequestCandidateRecord {
                    record,
                    source_count: 1,
                });
            }
        }
    }
    compacted
}

fn request_candidate_slot_hash(record: &UpsertRequestCandidateRecord) -> u64 {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;

    let mut hash = FNV_OFFSET;
    for byte in record.request_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for byte in record.candidate_index.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for byte in record.retry_index.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn worker_queue_capacity(total_capacity: usize, workers: usize, worker_index: usize) -> usize {
    let workers = workers.max(1);
    let base = total_capacity / workers;
    let remainder = total_capacity % workers;
    (base + usize::from(worker_index < remainder)).max(1)
}

#[derive(Debug)]
struct RequestCandidateDbWriteGate {
    semaphore: tokio::sync::Semaphore,
}

impl RequestCandidateDbWriteGate {
    fn new(limit: usize) -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(limit.max(1)),
        }
    }

    async fn acquire<'a>(
        &'a self,
        metrics: &'a RequestCandidateQueueMetrics,
    ) -> RequestCandidateDbWritePermit<'a> {
        if self.semaphore.available_permits() == 0 {
            metrics.db_write_wait_total.fetch_add(1, Ordering::AcqRel);
        }
        let permit = self
            .semaphore
            .acquire()
            .await
            .expect("request candidate DB write gate semaphore should not be closed");
        let in_flight = metrics.db_write_in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        metrics
            .db_write_max_in_flight
            .fetch_max(in_flight, Ordering::AcqRel);
        RequestCandidateDbWritePermit {
            metrics,
            _permit: permit,
        }
    }
}

struct RequestCandidateDbWritePermit<'a> {
    metrics: &'a RequestCandidateQueueMetrics,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl Drop for RequestCandidateDbWritePermit<'_> {
    fn drop(&mut self) {
        self.metrics
            .db_write_in_flight
            .fetch_sub(1, Ordering::AcqRel);
    }
}

fn merge_request_candidate_record(
    target: &mut UpsertRequestCandidateRecord,
    incoming: UpsertRequestCandidateRecord,
) {
    if !incoming.id.trim().is_empty() {
        target.id = incoming.id;
    }
    target.request_id = incoming.request_id;
    target.candidate_index = incoming.candidate_index;
    target.retry_index = incoming.retry_index;
    target.status = incoming.status;

    take_if_some(&mut target.user_id, incoming.user_id);
    take_if_some(&mut target.api_key_id, incoming.api_key_id);
    take_if_some(&mut target.username, incoming.username);
    take_if_some(&mut target.api_key_name, incoming.api_key_name);
    take_if_some(&mut target.provider_id, incoming.provider_id);
    take_if_some(&mut target.endpoint_id, incoming.endpoint_id);
    take_if_some(&mut target.key_id, incoming.key_id);
    take_if_some(&mut target.skip_reason, incoming.skip_reason);
    take_if_some(&mut target.is_cached, incoming.is_cached);
    take_if_some(&mut target.status_code, incoming.status_code);
    take_if_some(&mut target.error_type, incoming.error_type);
    take_if_some(&mut target.error_message, incoming.error_message);
    take_if_some(&mut target.latency_ms, incoming.latency_ms);
    take_if_some(
        &mut target.concurrent_requests,
        incoming.concurrent_requests,
    );
    merge_json_value(&mut target.extra_data, incoming.extra_data);
    merge_json_value(
        &mut target.required_capabilities,
        incoming.required_capabilities,
    );

    if target.created_at_unix_ms.is_none() {
        target.created_at_unix_ms = incoming.created_at_unix_ms;
    }
    take_if_some(&mut target.started_at_unix_ms, incoming.started_at_unix_ms);
    take_if_some(
        &mut target.finished_at_unix_ms,
        incoming.finished_at_unix_ms,
    );
}

fn merge_request_candidate_record_for_flush(
    target: &mut UpsertRequestCandidateRecord,
    incoming: UpsertRequestCandidateRecord,
) {
    let target_status = target.status;
    let incoming_status = incoming.status;
    let next_status = merged_request_candidate_status(target_status, incoming_status);
    merge_request_candidate_record(target, incoming);
    target.status = next_status;
}

fn merged_request_candidate_status(
    current: RequestCandidateStatus,
    incoming: RequestCandidateStatus,
) -> RequestCandidateStatus {
    match (
        request_candidate_status_is_terminal(current),
        request_candidate_status_is_terminal(incoming),
    ) {
        (_, true) => incoming,
        (true, false) => current,
        (false, false) => incoming,
    }
}

fn request_candidate_status_is_terminal(status: RequestCandidateStatus) -> bool {
    matches!(
        status,
        RequestCandidateStatus::Success
            | RequestCandidateStatus::Failed
            | RequestCandidateStatus::Cancelled
    )
}

#[cfg(test)]
fn request_candidate_status_discriminant(status: RequestCandidateStatus) -> u8 {
    match status {
        RequestCandidateStatus::Available => 0,
        RequestCandidateStatus::Unused => 1,
        RequestCandidateStatus::Pending => 2,
        RequestCandidateStatus::Streaming => 3,
        RequestCandidateStatus::Success => 4,
        RequestCandidateStatus::Failed => 5,
        RequestCandidateStatus::Cancelled => 6,
        RequestCandidateStatus::Skipped => 7,
    }
}

fn take_if_some<T>(target: &mut Option<T>, incoming: Option<T>) {
    if incoming.is_some() {
        *target = incoming;
    }
}

fn merge_json_value(target: &mut Option<serde_json::Value>, incoming: Option<serde_json::Value>) {
    match (target.as_mut(), incoming) {
        (Some(serde_json::Value::Object(target)), Some(serde_json::Value::Object(incoming))) => {
            target.extend(incoming);
        }
        (_, Some(incoming)) => {
            *target = Some(incoming);
        }
        (_, None) => {}
    }
}

fn env_string(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_optional_usize(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn decrement_atomic_usize(value: &AtomicUsize) {
    decrement_atomic_usize_by(value, 1);
}

fn decrement_atomic_usize_by(value: &AtomicUsize, amount: usize) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_sub(amount))
    });
}

#[cfg(test)]
mod tests {
    use super::{
        compact_records_for_flush, RequestCandidateQueueConfig, RequestCandidateQueueRuntime,
    };
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::DataLayerError;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateReadRepository, RequestCandidateStatus, RequestCandidateWriteRepository,
        StoredRequestCandidate, UpsertRequestCandidateRecord,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[derive(Default)]
    struct DelayedPendingRequestCandidateRepository {
        inner: InMemoryRequestCandidateRepository,
    }

    #[async_trait::async_trait]
    impl RequestCandidateWriteRepository for DelayedPendingRequestCandidateRepository {
        async fn upsert(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<StoredRequestCandidate, DataLayerError> {
            if candidate.status == RequestCandidateStatus::Pending {
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            self.inner.upsert(candidate).await
        }

        async fn delete_created_before(
            &self,
            created_before_unix_secs: u64,
            limit: usize,
        ) -> Result<usize, DataLayerError> {
            self.inner
                .delete_created_before(created_before_unix_secs, limit)
                .await
        }
    }

    #[derive(Default)]
    struct CountingBatchRequestCandidateRepository {
        inner: InMemoryRequestCandidateRepository,
        upsert_calls: AtomicUsize,
        upsert_many_calls: AtomicUsize,
        active_upsert_many: AtomicUsize,
        max_active_upsert_many: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl RequestCandidateWriteRepository for CountingBatchRequestCandidateRepository {
        async fn upsert(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<StoredRequestCandidate, DataLayerError> {
            self.upsert_calls.fetch_add(1, Ordering::AcqRel);
            self.inner.upsert(candidate).await
        }

        async fn upsert_many(
            &self,
            candidates: Vec<UpsertRequestCandidateRecord>,
        ) -> Result<usize, DataLayerError> {
            self.upsert_many_calls.fetch_add(1, Ordering::AcqRel);
            let active = self.active_upsert_many.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active_upsert_many
                .fetch_max(active, Ordering::AcqRel);
            tokio::time::sleep(Duration::from_millis(30)).await;
            let count = candidates.len();
            for candidate in candidates {
                self.inner.upsert(candidate).await?;
            }
            self.active_upsert_many.fetch_sub(1, Ordering::AcqRel);
            Ok(count)
        }

        async fn delete_created_before(
            &self,
            created_before_unix_secs: u64,
            limit: usize,
        ) -> Result<usize, DataLayerError> {
            self.inner
                .delete_created_before(created_before_unix_secs, limit)
                .await
        }
    }

    fn record(
        request_id: &str,
        candidate_index: u32,
        retry_index: u32,
        status: RequestCandidateStatus,
    ) -> UpsertRequestCandidateRecord {
        UpsertRequestCandidateRecord {
            id: format!("{request_id}-{candidate_index}-{retry_index}-{status:?}"),
            request_id: request_id.to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            candidate_index,
            retry_index,
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            status,
            skip_reason: None,
            is_cached: None,
            status_code: None,
            error_type: None,
            error_message: None,
            latency_ms: None,
            concurrent_requests: None,
            extra_data: None,
            required_capabilities: None,
            created_at_unix_ms: None,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
        }
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn unset(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.as_ref() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn from_env_defaults_to_async_with_sync_full_fallback() {
        let _mode = EnvGuard::unset(super::MODE_ENV);
        let _full = EnvGuard::unset(super::QUEUE_FULL_ENV);

        let config = RequestCandidateQueueConfig::from_env();

        assert_eq!(config.mode, super::RequestCandidateWriteMode::Async);
        assert_eq!(
            config.full_policy,
            super::RequestCandidateQueueFullPolicy::Sync
        );
    }

    #[test]
    fn compact_merges_same_slot_without_losing_terminal_fields() {
        let mut first_success = record("req", 0, 0, RequestCandidateStatus::Success);
        first_success.provider_id = Some("provider-a".to_string());
        first_success.extra_data = Some(serde_json::json!({"first": true}));
        let mut second_success = record("req", 0, 0, RequestCandidateStatus::Success);
        second_success.latency_ms = Some(123);
        second_success.extra_data = Some(serde_json::json!({"second": true}));

        let compacted = compact_records_for_flush(vec![
            record("req", 0, 0, RequestCandidateStatus::Pending),
            first_success,
            record("req", 0, 1, RequestCandidateStatus::Failed),
            second_success,
        ]);

        assert_eq!(compacted.len(), 2);
        assert_eq!(compacted[0].record.status, RequestCandidateStatus::Success);
        assert_eq!(compacted[0].source_count, 3);
        assert_eq!(
            compacted[0].record.provider_id.as_deref(),
            Some("provider-a")
        );
        assert_eq!(compacted[0].record.latency_ms, Some(123));
        assert_eq!(
            compacted[0].record.extra_data,
            Some(serde_json::json!({"first": true, "second": true}))
        );
        assert_eq!(compacted[1].record.status, RequestCandidateStatus::Failed);
        assert_eq!(compacted[1].source_count, 1);
    }

    #[test]
    fn compact_keeps_terminal_status_when_later_intermediate_status_arrives() {
        let compacted = compact_records_for_flush(vec![
            record("req", 0, 0, RequestCandidateStatus::Success),
            record("req", 0, 0, RequestCandidateStatus::Streaming),
            record("req", 0, 0, RequestCandidateStatus::Unused),
        ]);

        assert_eq!(compacted.len(), 1);
        assert_eq!(compacted[0].record.status, RequestCandidateStatus::Success);
        assert_eq!(compacted[0].source_count, 3);
    }

    #[tokio::test]
    async fn async_queue_flushes_enqueued_records() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 16,
                batch_size: 2,
                db_batch_size: 128,
                flush_interval: Duration::from_millis(10),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Drop,
            },
        );

        runtime
            .enqueue_or_fallback(record("req", 0, 0, RequestCandidateStatus::Success))
            .await
            .unwrap();

        for _ in 0..50 {
            let rows = repository.list_by_request_id("req").await.unwrap();
            if rows.len() == 1 {
                assert_eq!(rows[0].status, RequestCandidateStatus::Success);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        panic!("async request candidate queue did not flush record in time");
    }

    #[tokio::test]
    async fn async_queue_flushes_records_with_batch_repository_call() {
        let repository = Arc::new(CountingBatchRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 16,
                batch_size: 4,
                db_batch_size: 128,
                flush_interval: Duration::from_millis(100),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Drop,
            },
        );

        for index in 0..4 {
            runtime
                .enqueue_or_fallback(record(
                    "req-batch",
                    index,
                    0,
                    RequestCandidateStatus::Success,
                ))
                .await
                .unwrap();
        }

        for _ in 0..50 {
            if runtime.metrics.pending_current.load(Ordering::Acquire) == 0 {
                assert_eq!(repository.upsert_many_calls.load(Ordering::Acquire), 1);
                assert_eq!(repository.upsert_calls.load(Ordering::Acquire), 0);
                assert_eq!(
                    runtime.metrics.flush_sql_ops_total.load(Ordering::Acquire),
                    1
                );
                assert_eq!(
                    runtime
                        .metrics
                        .flush_sql_records_total
                        .load(Ordering::Acquire),
                    4
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        panic!("async request candidate queue did not finish batch flush in time");
    }

    #[tokio::test]
    async fn async_queue_splits_compacted_flush_into_db_batches() {
        let repository = Arc::new(CountingBatchRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 16,
                batch_size: 5,
                db_batch_size: 2,
                flush_interval: Duration::from_millis(100),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Drop,
            },
        );

        for index in 0..5 {
            runtime
                .enqueue_or_fallback(record(
                    "req-db-batch",
                    index,
                    0,
                    RequestCandidateStatus::Success,
                ))
                .await
                .unwrap();
        }

        for _ in 0..50 {
            if runtime.metrics.pending_current.load(Ordering::Acquire) == 0 {
                assert_eq!(repository.upsert_many_calls.load(Ordering::Acquire), 3);
                assert_eq!(
                    runtime.metrics.flush_batches_total.load(Ordering::Acquire),
                    1
                );
                assert_eq!(
                    runtime.metrics.flush_sql_ops_total.load(Ordering::Acquire),
                    3
                );
                assert_eq!(
                    runtime
                        .metrics
                        .flush_sql_records_total
                        .load(Ordering::Acquire),
                    5
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        panic!("async request candidate queue did not split DB batches in time");
    }

    #[tokio::test]
    async fn async_queue_db_write_gate_limits_concurrent_batch_writes() {
        let repository = Arc::new(CountingBatchRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 64,
                batch_size: 1,
                db_batch_size: 128,
                flush_interval: Duration::from_millis(100),
                workers: 4,
                db_write_concurrency_limit: Some(2),
                full_policy: super::RequestCandidateQueueFullPolicy::Drop,
            },
        );

        for index in 0..8 {
            runtime
                .enqueue_or_fallback(record(
                    &format!("req-gate-{index}"),
                    0,
                    0,
                    RequestCandidateStatus::Success,
                ))
                .await
                .unwrap();
        }

        for _ in 0..100 {
            if runtime.metrics.pending_current.load(Ordering::Acquire) == 0 {
                assert_eq!(repository.max_active_upsert_many.load(Ordering::Acquire), 2);
                assert_eq!(
                    runtime
                        .metrics
                        .db_write_max_in_flight
                        .load(Ordering::Acquire),
                    2
                );
                assert!(runtime.metrics.db_write_wait_total.load(Ordering::Acquire) > 0);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        panic!("async request candidate queue did not finish gated writes in time");
    }

    #[tokio::test]
    async fn async_queue_preserves_same_slot_order_with_multiple_workers() {
        let repository = Arc::new(DelayedPendingRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 16,
                batch_size: 1,
                db_batch_size: 128,
                flush_interval: Duration::from_millis(100),
                workers: 2,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Drop,
            },
        );

        runtime
            .enqueue_or_fallback(record("req-order", 0, 0, RequestCandidateStatus::Pending))
            .await
            .unwrap();
        runtime
            .enqueue_or_fallback(record("req-order", 0, 0, RequestCandidateStatus::Success))
            .await
            .unwrap();

        for _ in 0..50 {
            if runtime.metrics.pending_current.load(Ordering::Acquire) == 0 {
                let rows = repository
                    .inner
                    .list_by_request_id("req-order")
                    .await
                    .unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].status, RequestCandidateStatus::Success);
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        panic!("async request candidate queue did not finish ordered same-slot writes in time");
    }
}
