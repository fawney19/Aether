use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, RequestCandidateWriteRepository, UpsertRequestCandidateRecord,
};
use aether_runtime::{MetricKind, MetricSample};
use tokio::sync::{mpsc, Semaphore};
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
const RUNTIME_THREADS_ENV: &str = "AETHER_GATEWAY_REQUEST_CANDIDATE_QUEUE_RUNTIME_THREADS";

const DEFAULT_QUEUE_CAPACITY: usize = 65_536;
const DEFAULT_BATCH_SIZE: usize = 512;
const DEFAULT_DB_BATCH_SIZE: usize = 512;
const MAX_DB_BATCH_SIZE: usize = 1_000;
const DEFAULT_FLUSH_INTERVAL_MS: u64 = 50;
const PRIORITY_MICRO_BATCH_INTERVAL_MS: u64 = 1;
const MAX_CONSECUTIVE_ACTIVE_FLUSHES: usize = 2;
// Keep normal candidate writes from being indefinitely delayed by a continuous
// lifecycle stream. Priority batches still flush immediately.
const MAX_CONSECUTIVE_PRIORITY_FLUSHES: usize = 4;
const DEFAULT_WORKERS: usize = 2;
const DEFAULT_RUNTIME_THREADS: usize = 1;
const MAX_RUNTIME_THREADS: usize = 8;
const RUNTIME_THREAD_STACK_BYTES: usize = 2 * 1024 * 1024;
const RUNTIME_THREAD_NAME: &str = "aether-candidate-queue";
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
        config.db_batch_size =
            env_usize(DB_BATCH_SIZE_ENV, DEFAULT_DB_BATCH_SIZE).clamp(1, MAX_DB_BATCH_SIZE);
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
    priority_queued_current: AtomicUsize,
    priority_max_queued: AtomicUsize,
    priority_pending_current: AtomicUsize,
    active_queued_current: AtomicUsize,
    active_max_queued: AtomicUsize,
    active_pending_current: AtomicUsize,
    terminal_queued_current: AtomicUsize,
    terminal_max_queued: AtomicUsize,
    terminal_pending_current: AtomicUsize,
    terminal_barrier_pending: AtomicUsize,
    terminal_barrier_max_pending: AtomicUsize,
    enqueued_total: AtomicU64,
    priority_enqueued_total: AtomicU64,
    priority_async_overflow_total: AtomicU64,
    dropped_total: AtomicU64,
    flushed_total: AtomicU64,
    priority_flushed_total: AtomicU64,
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

#[derive(Debug)]
struct RequestCandidateTerminalBarrier {
    ready: AtomicBool,
}

impl RequestCandidateTerminalBarrier {
    fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
        }
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    fn release(&self, metrics: &RequestCandidateQueueMetrics) {
        if self
            .ready
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            decrement_atomic_usize(&metrics.terminal_barrier_pending);
        }
    }
}

#[derive(Debug)]
enum RequestCandidateActiveQueueMessage {
    Record(UpsertRequestCandidateRecord),
    Barrier(Arc<RequestCandidateTerminalBarrier>),
}

#[derive(Debug)]
struct RequestCandidateTerminalQueueRecord {
    record: UpsertRequestCandidateRecord,
    barrier: Arc<RequestCandidateTerminalBarrier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestCandidateLifecycleLane {
    Active,
    Terminal,
}

#[derive(Debug)]
enum RequestCandidatePriorityEnqueueError {
    Full(UpsertRequestCandidateRecord),
    Closed(UpsertRequestCandidateRecord),
}

#[derive(Clone)]
pub(crate) struct RequestCandidateQueueRuntime {
    senders: Vec<mpsc::Sender<UpsertRequestCandidateRecord>>,
    active_senders: Vec<mpsc::Sender<RequestCandidateActiveQueueMessage>>,
    terminal_senders: Vec<mpsc::Sender<RequestCandidateTerminalQueueRecord>>,
    normal_admission: Arc<Semaphore>,
    priority_admission: Arc<Semaphore>,
    priority_capacity: usize,
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
            .field(
                "priority_queued_current",
                &self.metrics.priority_queued_current.load(Ordering::Acquire),
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
        config.batch_size = config.batch_size.max(1);
        config.db_batch_size = config.db_batch_size.clamp(1, MAX_DB_BATCH_SIZE);
        let mut senders = Vec::with_capacity(config.workers);
        let mut receivers = Vec::with_capacity(config.workers);
        let priority_capacity = priority_queue_capacity(config.capacity, config.workers);
        let mut active_senders = Vec::with_capacity(config.workers);
        let mut active_receivers = Vec::with_capacity(config.workers);
        let mut terminal_senders = Vec::with_capacity(config.workers);
        let mut terminal_receivers = Vec::with_capacity(config.workers);
        for worker_index in 0..config.workers {
            let capacity = worker_queue_capacity(config.capacity, config.workers, worker_index);
            let (sender, receiver) = mpsc::channel(capacity);
            senders.push(sender);
            receivers.push(receiver);
            let (active_sender, active_receiver) = mpsc::channel(capacity);
            active_senders.push(active_sender);
            active_receivers.push(active_receiver);
            let (terminal_sender, terminal_receiver) = mpsc::channel(capacity);
            terminal_senders.push(terminal_sender);
            terminal_receivers.push(terminal_receiver);
        }
        let db_write_gate = config
            .db_write_concurrency_limit
            .map(RequestCandidateDbWriteGate::new)
            .map(Arc::new);
        let runtime = Arc::new(Self {
            senders,
            active_senders,
            terminal_senders,
            normal_admission: Arc::new(Semaphore::new(config.capacity)),
            priority_admission: Arc::new(Semaphore::new(priority_capacity)),
            priority_capacity,
            repository,
            config,
            metrics: Arc::new(RequestCandidateQueueMetrics::default()),
            db_write_gate,
        });
        runtime.spawn_workers(receivers, active_receivers, terminal_receivers);
        runtime
    }

    pub(crate) async fn enqueue_or_fallback(
        &self,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), aether_data::DataLayerError> {
        let worker_index = self.worker_index_for(&record);
        if request_candidate_status_is_priority(record.status) {
            return self
                .enqueue_priority_or_fallback(worker_index, record)
                .await;
        }

        let Some(sender) = self.senders.get(worker_index) else {
            self.metrics
                .sync_fallback_total
                .fetch_add(1, Ordering::AcqRel);
            return self.repository.upsert(record).await.map(|_| ());
        };
        let admission = match self.normal_admission.try_acquire() {
            Ok(admission) => admission,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return self.handle_normal_queue_full(worker_index, record).await;
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                self.metrics
                    .sync_fallback_total
                    .fetch_add(1, Ordering::AcqRel);
                return self.repository.upsert(record).await.map(|_| ());
            }
        };
        self.metrics.queued_current.fetch_add(1, Ordering::AcqRel);
        self.metrics.pending_current.fetch_add(1, Ordering::AcqRel);
        match sender.try_send(record) {
            Ok(()) => {
                admission.forget();
                self.metrics.enqueued_total.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(record)) => {
                drop(admission);
                decrement_atomic_usize(&self.metrics.queued_current);
                decrement_atomic_usize(&self.metrics.pending_current);
                self.handle_normal_queue_full(worker_index, record).await
            }
            Err(mpsc::error::TrySendError::Closed(record)) => {
                drop(admission);
                decrement_atomic_usize(&self.metrics.queued_current);
                decrement_atomic_usize(&self.metrics.pending_current);
                self.metrics
                    .sync_fallback_total
                    .fetch_add(1, Ordering::AcqRel);
                self.repository.upsert(record).await.map(|_| ())
            }
        }
    }

    async fn handle_normal_queue_full(
        &self,
        worker_index: usize,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), aether_data::DataLayerError> {
        warn!(
            event_name = "request_candidate_queue_full",
            log_type = "event",
            full_policy = ?self.config.full_policy,
            priority = false,
            worker_index,
            queued = self.metrics.queued_current.load(Ordering::Acquire),
            capacity = self.config.capacity,
            "gateway request candidate async queue is full"
        );
        match self.config.full_policy {
            RequestCandidateQueueFullPolicy::Sync => {
                self.metrics
                    .sync_fallback_total
                    .fetch_add(1, Ordering::AcqRel);
                self.repository.upsert(record).await.map(|_| ())
            }
            RequestCandidateQueueFullPolicy::Drop => {
                self.metrics.dropped_total.fetch_add(1, Ordering::AcqRel);
                Ok(())
            }
        }
    }

    /// Enqueue a lifecycle status without yielding to the Tokio scheduler.
    ///
    /// The bounded priority lane keeps a synchronous fast path. A returned
    /// record must be handed to the async enqueue path, which waits for
    /// admission without dropping lifecycle state.
    pub(crate) fn try_enqueue_priority_status(
        &self,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), UpsertRequestCandidateRecord> {
        if !request_candidate_status_is_priority(record.status) {
            return Err(record);
        }
        let worker_index = self.worker_index_for(&record);
        match self.try_enqueue_priority(worker_index, record) {
            Ok(()) => Ok(()),
            Err(RequestCandidatePriorityEnqueueError::Full(record)) => {
                self.observe_priority_backpressure(worker_index);
                Err(record)
            }
            Err(RequestCandidatePriorityEnqueueError::Closed(record)) => Err(record),
        }
    }

    async fn enqueue_priority_or_fallback(
        &self,
        worker_index: usize,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), aether_data::DataLayerError> {
        let record = match self
            .enqueue_priority_with_backpressure(worker_index, record)
            .await
        {
            Ok(()) => return Ok(()),
            Err(record) => record,
        };
        self.metrics
            .sync_fallback_total
            .fetch_add(1, Ordering::AcqRel);
        self.repository.upsert(record).await.map(|_| ())
    }

    fn try_enqueue_priority(
        &self,
        worker_index: usize,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), RequestCandidatePriorityEnqueueError> {
        if request_candidate_status_is_active(record.status) {
            return self.try_enqueue_active(worker_index, record);
        }
        if request_candidate_status_is_terminal(record.status) {
            return self.try_enqueue_terminal(worker_index, record);
        }
        Err(RequestCandidatePriorityEnqueueError::Closed(record))
    }

    fn try_enqueue_active(
        &self,
        worker_index: usize,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), RequestCandidatePriorityEnqueueError> {
        let Some(sender) = self.active_senders.get(worker_index) else {
            return Err(RequestCandidatePriorityEnqueueError::Closed(record));
        };
        let admission = match self.priority_admission.try_acquire() {
            Ok(admission) => admission,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(RequestCandidatePriorityEnqueueError::Full(record));
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(RequestCandidatePriorityEnqueueError::Closed(record));
            }
        };
        let sender_permit = match sender.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(_)) => {
                return Err(RequestCandidatePriorityEnqueueError::Full(record));
            }
            Err(_) => return Err(RequestCandidatePriorityEnqueueError::Closed(record)),
        };
        self.begin_lifecycle_enqueue(RequestCandidateLifecycleLane::Active);
        sender_permit.send(RequestCandidateActiveQueueMessage::Record(record));
        admission.forget();
        self.finish_lifecycle_enqueue();
        Ok(())
    }

    fn try_enqueue_terminal(
        &self,
        worker_index: usize,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), RequestCandidatePriorityEnqueueError> {
        let (Some(active_sender), Some(terminal_sender)) = (
            self.active_senders.get(worker_index),
            self.terminal_senders.get(worker_index),
        ) else {
            return Err(RequestCandidatePriorityEnqueueError::Closed(record));
        };
        let admission = match self.priority_admission.try_acquire() {
            Ok(admission) => admission,
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                return Err(RequestCandidatePriorityEnqueueError::Full(record));
            }
            Err(tokio::sync::TryAcquireError::Closed) => {
                return Err(RequestCandidatePriorityEnqueueError::Closed(record));
            }
        };
        let active_permit = match active_sender.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(_)) => {
                return Err(RequestCandidatePriorityEnqueueError::Full(record));
            }
            Err(_) => return Err(RequestCandidatePriorityEnqueueError::Closed(record)),
        };
        let terminal_permit = match terminal_sender.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(_)) => {
                return Err(RequestCandidatePriorityEnqueueError::Full(record));
            }
            Err(_) => return Err(RequestCandidatePriorityEnqueueError::Closed(record)),
        };
        self.begin_lifecycle_enqueue(RequestCandidateLifecycleLane::Terminal);
        let barrier_pending = self
            .metrics
            .terminal_barrier_pending
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        self.metrics
            .terminal_barrier_max_pending
            .fetch_max(barrier_pending, Ordering::AcqRel);
        let barrier = Arc::new(RequestCandidateTerminalBarrier::new());
        active_permit.send(RequestCandidateActiveQueueMessage::Barrier(Arc::clone(
            &barrier,
        )));
        terminal_permit.send(RequestCandidateTerminalQueueRecord { record, barrier });
        admission.forget();
        self.finish_lifecycle_enqueue();
        Ok(())
    }

    async fn enqueue_priority_with_backpressure(
        &self,
        worker_index: usize,
        record: UpsertRequestCandidateRecord,
    ) -> Result<(), UpsertRequestCandidateRecord> {
        let (Some(active_sender), Some(terminal_sender)) = (
            self.active_senders.get(worker_index),
            self.terminal_senders.get(worker_index),
        ) else {
            return Err(record);
        };
        let lane = if request_candidate_status_is_active(record.status) {
            RequestCandidateLifecycleLane::Active
        } else if request_candidate_status_is_terminal(record.status) {
            RequestCandidateLifecycleLane::Terminal
        } else {
            return Err(record);
        };
        if self.priority_admission.available_permits() == 0
            || active_sender.capacity() == 0
            || (lane == RequestCandidateLifecycleLane::Terminal && terminal_sender.capacity() == 0)
        {
            self.observe_priority_backpressure(worker_index);
        }
        let active_permit = match active_sender.reserve().await {
            Ok(permit) => permit,
            Err(_) => return Err(record),
        };
        if lane == RequestCandidateLifecycleLane::Active {
            let admission = match Arc::clone(&self.priority_admission).acquire_owned().await {
                Ok(admission) => admission,
                Err(_) => return Err(record),
            };
            self.begin_lifecycle_enqueue(lane);
            active_permit.send(RequestCandidateActiveQueueMessage::Record(record));
            admission.forget();
            self.finish_lifecycle_enqueue();
            return Ok(());
        }

        let terminal_permit = match terminal_sender.reserve().await {
            Ok(permit) => permit,
            Err(_) => return Err(record),
        };
        let admission = match Arc::clone(&self.priority_admission).acquire_owned().await {
            Ok(admission) => admission,
            Err(_) => return Err(record),
        };
        self.begin_lifecycle_enqueue(lane);
        let barrier_pending = self
            .metrics
            .terminal_barrier_pending
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        self.metrics
            .terminal_barrier_max_pending
            .fetch_max(barrier_pending, Ordering::AcqRel);
        let barrier = Arc::new(RequestCandidateTerminalBarrier::new());
        active_permit.send(RequestCandidateActiveQueueMessage::Barrier(Arc::clone(
            &barrier,
        )));
        terminal_permit.send(RequestCandidateTerminalQueueRecord { record, barrier });
        admission.forget();
        self.finish_lifecycle_enqueue();
        Ok(())
    }

    fn begin_lifecycle_enqueue(&self, lane: RequestCandidateLifecycleLane) {
        self.metrics.queued_current.fetch_add(1, Ordering::AcqRel);
        self.metrics.pending_current.fetch_add(1, Ordering::AcqRel);
        let priority_queued = self
            .metrics
            .priority_queued_current
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        self.metrics
            .priority_max_queued
            .fetch_max(priority_queued, Ordering::AcqRel);
        self.metrics
            .priority_pending_current
            .fetch_add(1, Ordering::AcqRel);
        match lane {
            RequestCandidateLifecycleLane::Active => {
                let queued = self
                    .metrics
                    .active_queued_current
                    .fetch_add(1, Ordering::AcqRel)
                    + 1;
                self.metrics
                    .active_max_queued
                    .fetch_max(queued, Ordering::AcqRel);
                self.metrics
                    .active_pending_current
                    .fetch_add(1, Ordering::AcqRel);
            }
            RequestCandidateLifecycleLane::Terminal => {
                let queued = self
                    .metrics
                    .terminal_queued_current
                    .fetch_add(1, Ordering::AcqRel)
                    + 1;
                self.metrics
                    .terminal_max_queued
                    .fetch_max(queued, Ordering::AcqRel);
                self.metrics
                    .terminal_pending_current
                    .fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    fn finish_lifecycle_enqueue(&self) {
        self.metrics.enqueued_total.fetch_add(1, Ordering::AcqRel);
        self.metrics
            .priority_enqueued_total
            .fetch_add(1, Ordering::AcqRel);
    }

    fn observe_priority_backpressure(&self, worker_index: usize) {
        let backpressure_total = self
            .metrics
            .priority_async_overflow_total
            .fetch_add(1, Ordering::AcqRel)
            + 1;
        if should_log_queue_counter(backpressure_total) {
            warn!(
                event_name = "request_candidate_priority_queue_backpressure",
                log_type = "event",
                worker_index,
                queued = self.metrics.priority_queued_current.load(Ordering::Acquire),
                capacity = self.priority_capacity,
                backpressure_total,
                "gateway applied bounded lifecycle candidate queue backpressure"
            );
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
                "request_candidate_priority_queue_depth",
                "Current number of lifecycle request candidate records waiting in the priority persistence queue.",
                MetricKind::Gauge,
                self.metrics.priority_queued_current.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_priority_queue_pending_depth",
                "Current number of lifecycle request candidate records accepted into the priority persistence queue but not yet flushed.",
                MetricKind::Gauge,
                self.metrics.priority_pending_current.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_active_queue_depth",
                "Current number of pending or streaming request candidate records waiting in the active lifecycle lane.",
                MetricKind::Gauge,
                self.metrics.active_queued_current.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_active_queue_pending_depth",
                "Current number of active lifecycle records accepted but not yet flushed.",
                MetricKind::Gauge,
                self.metrics.active_pending_current.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_active_queue_max_depth",
                "Maximum observed active lifecycle backlog depth since process start.",
                MetricKind::Gauge,
                self.metrics.active_max_queued.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_terminal_queue_depth",
                "Current number of terminal request candidate records waiting in the terminal lifecycle lane.",
                MetricKind::Gauge,
                self.metrics.terminal_queued_current.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_terminal_queue_pending_depth",
                "Current number of terminal lifecycle records accepted but not yet flushed.",
                MetricKind::Gauge,
                self.metrics.terminal_pending_current.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_terminal_queue_max_depth",
                "Maximum observed terminal lifecycle backlog depth since process start.",
                MetricKind::Gauge,
                self.metrics.terminal_max_queued.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_terminal_barrier_pending",
                "Current number of terminal records waiting for earlier active lifecycle writes to commit.",
                MetricKind::Gauge,
                self.metrics
                    .terminal_barrier_pending
                    .load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_terminal_barrier_max_pending",
                "Maximum observed number of unresolved terminal ordering barriers since process start.",
                MetricKind::Gauge,
                self.metrics
                    .terminal_barrier_max_pending
                    .load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_capacity",
                "Configured request candidate async persistence queue capacity.",
                MetricKind::Gauge,
                self.config.capacity as u64,
            ),
            MetricSample::new(
                "request_candidate_priority_queue_capacity",
                "Configured hard admission limit shared by the active and terminal lifecycle queues.",
                MetricKind::Gauge,
                self.priority_capacity as u64,
            ),
            MetricSample::new(
                "request_candidate_priority_queue_soft_limit",
                "Compatibility alias for the bounded priority lifecycle admission limit.",
                MetricKind::Gauge,
                self.priority_capacity as u64,
            ),
            MetricSample::new(
                "request_candidate_priority_queue_max_depth",
                "Maximum observed priority lifecycle backlog depth since process start.",
                MetricKind::Gauge,
                self.metrics.priority_max_queued.load(Ordering::Acquire) as u64,
            ),
            MetricSample::new(
                "request_candidate_priority_queue_over_limit_depth",
                "Current priority lifecycle backlog above its configured admission limit; expected to remain zero.",
                MetricKind::Gauge,
                self.metrics
                    .priority_queued_current
                    .load(Ordering::Acquire)
                    .saturating_sub(self.priority_capacity) as u64,
            ),
            MetricSample::new(
                "request_candidate_queue_enqueued_total",
                "Total request candidate records accepted into the async persistence queue.",
                MetricKind::Counter,
                self.metrics.enqueued_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_priority_queue_enqueued_total",
                "Total lifecycle request candidate records accepted into the priority persistence queue.",
                MetricKind::Counter,
                self.metrics.priority_enqueued_total.load(Ordering::Acquire),
            ),
            MetricSample::new(
                "request_candidate_priority_queue_async_overflow_total",
                "Total lifecycle enqueue attempts that encountered bounded queue backpressure.",
                MetricKind::Counter,
                self.metrics
                    .priority_async_overflow_total
                    .load(Ordering::Acquire),
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
                "request_candidate_priority_queue_flushed_total",
                "Total lifecycle request candidate records flushed by priority persistence workers.",
                MetricKind::Counter,
                self.metrics.priority_flushed_total.load(Ordering::Acquire),
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
        active_receivers: Vec<mpsc::Receiver<RequestCandidateActiveQueueMessage>>,
        terminal_receivers: Vec<mpsc::Receiver<RequestCandidateTerminalQueueRecord>>,
    ) {
        for (worker_index, ((receiver, active_receiver), terminal_receiver)) in receivers
            .into_iter()
            .zip(active_receivers)
            .zip(terminal_receivers)
            .enumerate()
        {
            let repository = Arc::clone(&self.repository);
            let config = self.config.clone();
            let metrics = Arc::clone(&self.metrics);
            let db_write_gate = self.db_write_gate.clone();
            let normal_admission = Arc::clone(&self.normal_admission);
            let priority_admission = Arc::clone(&self.priority_admission);
            spawn_on_request_candidate_background_runtime(async move {
                run_worker(
                    repository,
                    config,
                    metrics,
                    db_write_gate,
                    worker_index,
                    receiver,
                    active_receiver,
                    terminal_receiver,
                    normal_admission,
                    priority_admission,
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

fn spawn_on_request_candidate_background_runtime<F>(task: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    request_candidate_background_runtime().handle().spawn(task)
}

fn request_candidate_background_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<&'static tokio::runtime::Runtime> = OnceLock::new();

    RUNTIME.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(request_candidate_background_runtime_threads())
            .thread_name(RUNTIME_THREAD_NAME)
            .thread_stack_size(RUNTIME_THREAD_STACK_BYTES)
            .build()
            .expect("request candidate background runtime should build");
        Box::leak(Box::new(runtime))
    })
}

fn request_candidate_background_runtime_threads() -> usize {
    parse_request_candidate_background_runtime_threads(
        std::env::var(RUNTIME_THREADS_ENV).ok().as_deref(),
    )
}

fn parse_request_candidate_background_runtime_threads(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or(DEFAULT_RUNTIME_THREADS)
        .clamp(1, MAX_RUNTIME_THREADS)
}

fn push_active_message(
    message: RequestCandidateActiveQueueMessage,
    batch: &mut Vec<UpsertRequestCandidateRecord>,
    barriers: &mut Vec<Arc<RequestCandidateTerminalBarrier>>,
    metrics: &RequestCandidateQueueMetrics,
) {
    match message {
        RequestCandidateActiveQueueMessage::Record(record) => {
            decrement_atomic_usize(&metrics.queued_current);
            decrement_atomic_usize(&metrics.priority_queued_current);
            decrement_atomic_usize(&metrics.active_queued_current);
            batch.push(record);
        }
        RequestCandidateActiveQueueMessage::Barrier(barrier) => barriers.push(barrier),
    }
}

/// Give active lifecycle events a very short coalescing window so a burst does
/// not turn into one database transaction per event. Terminal barriers share
/// this FIFO and are only released after the collected active records commit.
async fn collect_active_micro_batch(
    receiver: &mut mpsc::Receiver<RequestCandidateActiveQueueMessage>,
    batch: &mut Vec<UpsertRequestCandidateRecord>,
    barriers: &mut Vec<Arc<RequestCandidateTerminalBarrier>>,
    batch_size: usize,
    metrics: &RequestCandidateQueueMetrics,
    receiver_open: &mut bool,
) {
    if batch.len() >= batch_size.max(1) || !*receiver_open {
        return;
    }

    // Wait on the receiver for the bounded window instead of sleeping and
    // polling once. Polling makes the batch size depend on scheduler timing:
    // a sender that is ready during the window can still be missed by a
    // `try_recv` immediately after the sleep.
    let deadline = tokio::time::sleep(Duration::from_millis(PRIORITY_MICRO_BATCH_INTERVAL_MS));
    tokio::pin!(deadline);
    while batch.len() < batch_size.max(1) {
        tokio::select! {
            received = receiver.recv(), if *receiver_open => match received {
                Some(message) => push_active_message(message, batch, barriers, metrics),
                None => {
                    *receiver_open = false;
                    break;
                }
            },
            _ = &mut deadline => break,
        }
    }
}

fn collect_ready_terminal_batch(
    receiver: &mut mpsc::Receiver<RequestCandidateTerminalQueueRecord>,
    front: &mut Option<RequestCandidateTerminalQueueRecord>,
    batch: &mut Vec<UpsertRequestCandidateRecord>,
    batch_size: usize,
    metrics: &RequestCandidateQueueMetrics,
    receiver_open: &mut bool,
) {
    if !batch.is_empty() {
        return;
    }
    while batch.len() < batch_size.max(1) {
        if front.is_none() && *receiver_open {
            match receiver.try_recv() {
                Ok(record) => *front = Some(record),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    *receiver_open = false;
                    break;
                }
            }
        }
        let Some(record) = front.as_ref() else {
            break;
        };
        if !record.barrier.is_ready() {
            break;
        }
        let record = front.take().expect("ready terminal front should exist");
        decrement_atomic_usize(&metrics.queued_current);
        decrement_atomic_usize(&metrics.priority_queued_current);
        decrement_atomic_usize(&metrics.terminal_queued_current);
        batch.push(record.record);
    }
}

fn release_terminal_barriers(
    barriers: &mut Vec<Arc<RequestCandidateTerminalBarrier>>,
    metrics: &RequestCandidateQueueMetrics,
) {
    for barrier in barriers.drain(..) {
        barrier.release(metrics);
    }
}

fn collect_ready_normal_batch(
    receiver: &mut mpsc::Receiver<UpsertRequestCandidateRecord>,
    batch: &mut Vec<UpsertRequestCandidateRecord>,
    batch_size: usize,
    metrics: &RequestCandidateQueueMetrics,
    receiver_open: &mut bool,
) {
    while *receiver_open && batch.len() < batch_size.max(1) {
        match receiver.try_recv() {
            Ok(record) => {
                decrement_atomic_usize(&metrics.queued_current);
                batch.push(record);
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                *receiver_open = false;
            }
        }
    }
}

async fn run_worker(
    repository: Arc<dyn RequestCandidateWriteRepository>,
    config: RequestCandidateQueueConfig,
    metrics: Arc<RequestCandidateQueueMetrics>,
    db_write_gate: Option<Arc<RequestCandidateDbWriteGate>>,
    worker_index: usize,
    mut receiver: mpsc::Receiver<UpsertRequestCandidateRecord>,
    mut active_receiver: mpsc::Receiver<RequestCandidateActiveQueueMessage>,
    mut terminal_receiver: mpsc::Receiver<RequestCandidateTerminalQueueRecord>,
    normal_admission: Arc<Semaphore>,
    priority_admission: Arc<Semaphore>,
) {
    let mut ticker = interval(config.flush_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut batch = Vec::with_capacity(config.batch_size);
    let mut active_batch = Vec::with_capacity(config.batch_size);
    let mut active_barriers = Vec::new();
    let mut terminal_batch = Vec::with_capacity(config.batch_size);
    let mut terminal_front: Option<RequestCandidateTerminalQueueRecord> = None;
    let mut receiver_open = true;
    let mut active_receiver_open = true;
    let mut terminal_receiver_open = true;
    let mut consecutive_active_flushes = 0_usize;
    let mut consecutive_priority_flushes = 0_usize;

    while receiver_open
        || active_receiver_open
        || terminal_receiver_open
        || !batch.is_empty()
        || !active_batch.is_empty()
        || !active_barriers.is_empty()
        || !terminal_batch.is_empty()
        || terminal_front.is_some()
    {
        if consecutive_priority_flushes >= MAX_CONSECUTIVE_PRIORITY_FLUSHES {
            collect_ready_normal_batch(
                &mut receiver,
                &mut batch,
                config.batch_size,
                &metrics,
                &mut receiver_open,
            );
            if !batch.is_empty() {
                flush_batch(
                    &repository,
                    &config,
                    &metrics,
                    db_write_gate.as_ref(),
                    worker_index,
                    RequestCandidateQueueLane::Normal,
                    &mut batch,
                    Some(&normal_admission),
                )
                .await;
                consecutive_priority_flushes = 0;
                continue;
            }
        }

        // A terminal record can already be parked at the FIFO front while its
        // active barrier is unresolved. Once that barrier is released, the
        // terminal receiver cannot wake this worker because the record has
        // already been received. Flush it here when no active work is waiting;
        // sustained active traffic still uses the 2:1 fairness path below.
        if active_batch.is_empty()
            && active_receiver.is_empty()
            && terminal_front
                .as_ref()
                .is_some_and(|record| record.barrier.is_ready())
        {
            collect_ready_terminal_batch(
                &mut terminal_receiver,
                &mut terminal_front,
                &mut terminal_batch,
                config.batch_size,
                &metrics,
                &mut terminal_receiver_open,
            );
            if !terminal_batch.is_empty() {
                flush_batch(
                    &repository,
                    &config,
                    &metrics,
                    db_write_gate.as_ref(),
                    worker_index,
                    RequestCandidateQueueLane::Terminal,
                    &mut terminal_batch,
                    Some(&priority_admission),
                )
                .await;
                consecutive_active_flushes = 0;
                consecutive_priority_flushes = consecutive_priority_flushes.saturating_add(1);
                continue;
            }
        }

        if consecutive_active_flushes >= MAX_CONSECUTIVE_ACTIVE_FLUSHES {
            collect_ready_terminal_batch(
                &mut terminal_receiver,
                &mut terminal_front,
                &mut terminal_batch,
                config.batch_size,
                &metrics,
                &mut terminal_receiver_open,
            );
            if !terminal_batch.is_empty() {
                flush_batch(
                    &repository,
                    &config,
                    &metrics,
                    db_write_gate.as_ref(),
                    worker_index,
                    RequestCandidateQueueLane::Terminal,
                    &mut terminal_batch,
                    Some(&priority_admission),
                )
                .await;
                consecutive_active_flushes = 0;
                consecutive_priority_flushes = consecutive_priority_flushes.saturating_add(1);
                continue;
            }
        }

        tokio::select! {
            biased;
            received = active_receiver.recv(), if active_receiver_open => {
                match received {
                    Some(message) => {
                        push_active_message(
                            message,
                            &mut active_batch,
                            &mut active_barriers,
                            &metrics,
                        );
                        collect_active_micro_batch(
                            &mut active_receiver,
                            &mut active_batch,
                            &mut active_barriers,
                            config.batch_size,
                            &metrics,
                            &mut active_receiver_open,
                        )
                        .await;
                        let had_active_records = !active_batch.is_empty();
                        if had_active_records {
                            flush_batch(
                                &repository,
                                &config,
                                &metrics,
                                db_write_gate.as_ref(),
                                worker_index,
                                RequestCandidateQueueLane::Active,
                                &mut active_batch,
                                Some(&priority_admission),
                            ).await;
                        }
                        if active_batch.is_empty() {
                            release_terminal_barriers(&mut active_barriers, &metrics);
                        }
                        // Barriers are active-lane work too. Counting only DB
                        // flushes lets a continuous barrier stream win the
                        // biased select forever and starve its own terminal
                        // records.
                        consecutive_active_flushes =
                            consecutive_active_flushes.saturating_add(1);
                        consecutive_priority_flushes =
                            consecutive_priority_flushes.saturating_add(1);
                    }
                    None => {
                        active_receiver_open = false;
                        if !active_batch.is_empty() {
                            flush_batch(
                                &repository,
                                &config,
                                &metrics,
                                db_write_gate.as_ref(),
                                worker_index,
                                RequestCandidateQueueLane::Active,
                                &mut active_batch,
                                Some(&priority_admission),
                            ).await;
                        }
                        if active_batch.is_empty() {
                            release_terminal_barriers(&mut active_barriers, &metrics);
                        }
                    }
                }
            }
            _ = ticker.tick() => {
                if !active_batch.is_empty() {
                    flush_batch(
                        &repository,
                        &config,
                        &metrics,
                        db_write_gate.as_ref(),
                        worker_index,
                        RequestCandidateQueueLane::Active,
                        &mut active_batch,
                        Some(&priority_admission),
                    ).await;
                }
                if active_batch.is_empty() {
                    release_terminal_barriers(&mut active_barriers, &metrics);
                }
                collect_ready_terminal_batch(
                    &mut terminal_receiver,
                    &mut terminal_front,
                    &mut terminal_batch,
                    config.batch_size,
                    &metrics,
                    &mut terminal_receiver_open,
                );
                if !terminal_batch.is_empty() {
                    flush_batch(
                        &repository,
                        &config,
                        &metrics,
                        db_write_gate.as_ref(),
                        worker_index,
                        RequestCandidateQueueLane::Terminal,
                        &mut terminal_batch,
                        Some(&priority_admission),
                    ).await;
                }
                consecutive_priority_flushes = 0;
                consecutive_active_flushes = 0;
                if !batch.is_empty() {
                    flush_batch(
                        &repository,
                        &config,
                        &metrics,
                        db_write_gate.as_ref(),
                        worker_index,
                        RequestCandidateQueueLane::Normal,
                        &mut batch,
                        Some(&normal_admission),
                    ).await;
                }
            }
            received = terminal_receiver.recv(), if terminal_receiver_open && terminal_front.is_none() => {
                match received {
                    Some(record) => {
                        terminal_front = Some(record);
                        collect_ready_terminal_batch(
                            &mut terminal_receiver,
                            &mut terminal_front,
                            &mut terminal_batch,
                            config.batch_size,
                            &metrics,
                            &mut terminal_receiver_open,
                        );
                        if !terminal_batch.is_empty() {
                            flush_batch(
                                &repository,
                                &config,
                                &metrics,
                                db_write_gate.as_ref(),
                                worker_index,
                                RequestCandidateQueueLane::Terminal,
                                &mut terminal_batch,
                                Some(&priority_admission),
                            ).await;
                            consecutive_active_flushes = 0;
                            consecutive_priority_flushes =
                                consecutive_priority_flushes.saturating_add(1);
                        }
                    }
                    None => terminal_receiver_open = false,
                }
            }
            received = receiver.recv(), if receiver_open => {
                match received {
                    Some(record) => {
                        consecutive_priority_flushes = 0;
                        decrement_atomic_usize(&metrics.queued_current);
                        batch.push(record);
                        if batch.len() >= config.batch_size {
                            flush_batch(
                                &repository,
                                &config,
                                &metrics,
                                db_write_gate.as_ref(),
                                worker_index,
                                RequestCandidateQueueLane::Normal,
                                &mut batch,
                                Some(&normal_admission),
                            ).await;
                        }
                    }
                    None => {
                        receiver_open = false;
                        if !batch.is_empty() {
                            flush_batch(
                                &repository,
                                &config,
                                &metrics,
                                db_write_gate.as_ref(),
                                worker_index,
                                RequestCandidateQueueLane::Normal,
                                &mut batch,
                                Some(&normal_admission),
                            ).await;
                        }
                    }
                }
            }
        }
    }

    if !active_batch.is_empty() {
        flush_batch(
            &repository,
            &config,
            &metrics,
            db_write_gate.as_ref(),
            worker_index,
            RequestCandidateQueueLane::Active,
            &mut active_batch,
            Some(&priority_admission),
        )
        .await;
    }
    if active_batch.is_empty() {
        release_terminal_barriers(&mut active_barriers, &metrics);
    }
    collect_ready_terminal_batch(
        &mut terminal_receiver,
        &mut terminal_front,
        &mut terminal_batch,
        config.batch_size,
        &metrics,
        &mut terminal_receiver_open,
    );
    if !terminal_batch.is_empty() {
        flush_batch(
            &repository,
            &config,
            &metrics,
            db_write_gate.as_ref(),
            worker_index,
            RequestCandidateQueueLane::Terminal,
            &mut terminal_batch,
            Some(&priority_admission),
        )
        .await;
    }
    if !batch.is_empty() {
        flush_batch(
            &repository,
            &config,
            &metrics,
            db_write_gate.as_ref(),
            worker_index,
            RequestCandidateQueueLane::Normal,
            &mut batch,
            Some(&normal_admission),
        )
        .await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestCandidateQueueLane {
    Normal,
    Active,
    Terminal,
}

async fn flush_batch(
    repository: &Arc<dyn RequestCandidateWriteRepository>,
    config: &RequestCandidateQueueConfig,
    metrics: &RequestCandidateQueueMetrics,
    db_write_gate: Option<&Arc<RequestCandidateDbWriteGate>>,
    worker_index: usize,
    lane: RequestCandidateQueueLane,
    batch: &mut Vec<UpsertRequestCandidateRecord>,
    admission: Option<&Semaphore>,
) {
    let records = std::mem::take(batch);
    if records.is_empty() {
        return;
    }
    let source_count = records.len();
    let flush_plan = compact_records_for_flush(records);
    let compacted = source_count.saturating_sub(flush_plan.record_count());
    if compacted > 0 {
        metrics
            .compacted_total
            .fetch_add(compacted as u64, Ordering::AcqRel);
    }
    metrics.flush_batches_total.fetch_add(1, Ordering::AcqRel);
    let mut retry_normal = Vec::new();
    let mut retry_pending = Vec::new();
    let mut retry_streaming = Vec::new();
    let mut retry_terminal = Vec::new();
    let mut blocked_slots = HashSet::new();
    let mut failed = 0_u64;

    let normal_result = flush_ordered_stage(
        repository,
        config,
        metrics,
        db_write_gate,
        worker_index,
        lane,
        flush_plan.normal,
        &blocked_slots,
        &mut retry_normal,
    )
    .await;
    failed = failed.saturating_add(normal_result.failed_source_count);
    blocked_slots.extend(normal_result.failed_slots);

    let pending_result = flush_ordered_stage(
        repository,
        config,
        metrics,
        db_write_gate,
        worker_index,
        lane,
        flush_plan.pending,
        &blocked_slots,
        &mut retry_pending,
    )
    .await;
    failed = failed.saturating_add(pending_result.failed_source_count);
    blocked_slots.extend(pending_result.failed_slots);

    let streaming_result = flush_ordered_stage(
        repository,
        config,
        metrics,
        db_write_gate,
        worker_index,
        lane,
        flush_plan.streaming,
        &blocked_slots,
        &mut retry_streaming,
    )
    .await;
    failed = failed.saturating_add(streaming_result.failed_source_count);
    blocked_slots.extend(streaming_result.failed_slots);

    let terminal_result = flush_ordered_stage(
        repository,
        config,
        metrics,
        db_write_gate,
        worker_index,
        lane,
        flush_plan.terminal,
        &blocked_slots,
        &mut retry_terminal,
    )
    .await;
    failed = failed.saturating_add(terminal_result.failed_source_count);

    if failed > 0 {
        metrics
            .flush_failed_total
            .fetch_add(failed, Ordering::AcqRel);
        tokio::time::sleep(Duration::from_millis(FAILED_FLUSH_RETRY_DELAY_MS)).await;
        batch.extend(retry_normal);
        batch.extend(retry_pending);
        batch.extend(retry_streaming);
        batch.extend(retry_terminal);
    }
    let released = source_count.saturating_sub(batch.len());
    if released > 0 {
        if let Some(admission) = admission {
            admission.add_permits(released);
        }
    }
    debug!(
        event_name = "request_candidate_async_flush_completed",
        log_type = "event",
        worker_index,
        lane = ?lane,
        failed,
        "gateway completed request candidate async flush batch"
    );
}

#[allow(clippy::too_many_arguments)]
async fn flush_ordered_stage(
    repository: &Arc<dyn RequestCandidateWriteRepository>,
    config: &RequestCandidateQueueConfig,
    metrics: &RequestCandidateQueueMetrics,
    db_write_gate: Option<&Arc<RequestCandidateDbWriteGate>>,
    worker_index: usize,
    lane: RequestCandidateQueueLane,
    records: Vec<CompactedRequestCandidateRecord>,
    blocked_slots: &HashSet<RequestCandidateSlot>,
    retry_records: &mut Vec<UpsertRequestCandidateRecord>,
) -> FlushCompactedRecordsResult {
    let mut ready_records = Vec::with_capacity(records.len());
    for record in records {
        if blocked_slots.contains(&request_candidate_slot(&record.record)) {
            compact_retry_record(metrics, lane, &record);
            retry_records.push(record.record);
        } else {
            ready_records.push(record);
        }
    }
    flush_compacted_records(
        repository,
        config,
        metrics,
        db_write_gate,
        worker_index,
        lane,
        ready_records,
        retry_records,
    )
    .await
}

#[derive(Debug, Default)]
struct FlushCompactedRecordsResult {
    failed_source_count: u64,
    failed_slots: HashSet<RequestCandidateSlot>,
}

#[allow(clippy::too_many_arguments)]
async fn flush_compacted_records(
    repository: &Arc<dyn RequestCandidateWriteRepository>,
    config: &RequestCandidateQueueConfig,
    metrics: &RequestCandidateQueueMetrics,
    db_write_gate: Option<&Arc<RequestCandidateDbWriteGate>>,
    worker_index: usize,
    lane: RequestCandidateQueueLane,
    records: Vec<CompactedRequestCandidateRecord>,
    retry_records: &mut Vec<UpsertRequestCandidateRecord>,
) -> FlushCompactedRecordsResult {
    let mut result = FlushCompactedRecordsResult::default();
    for chunk in records.chunks(config.db_batch_size.max(1)) {
        let source_count = chunk
            .iter()
            .map(|record| record.source_count)
            .sum::<usize>();
        let record_count = chunk.len();
        let upsert_records = chunk
            .iter()
            .map(|record| record.record.clone())
            .collect::<Vec<_>>();
        debug_assert!(request_candidate_slots_are_unique(&upsert_records));
        metrics.flush_sql_ops_total.fetch_add(1, Ordering::AcqRel);
        metrics
            .flush_sql_records_total
            .fetch_add(record_count as u64, Ordering::AcqRel);
        let _db_write_permit = match db_write_gate {
            Some(gate) => Some(gate.acquire(metrics).await),
            None => None,
        };
        let _db_write_in_flight = RequestCandidateDbWriteInFlightGuard::new(metrics);
        if let Err(err) = repository.upsert_many(upsert_records).await {
            result.failed_source_count = result
                .failed_source_count
                .saturating_add(source_count as u64);
            result.failed_slots.extend(
                chunk
                    .iter()
                    .map(|record| request_candidate_slot(&record.record)),
            );
            decrement_atomic_usize_by(
                &metrics.pending_current,
                source_count.saturating_sub(record_count),
            );
            decrement_lifecycle_pending_by(
                metrics,
                lane,
                source_count.saturating_sub(record_count),
            );
            warn!(
                event_name = "request_candidate_async_flush_failed",
                log_type = "event",
                worker_index,
                lane = ?lane,
                record_count,
                source_count,
                error = ?err,
                "gateway failed to asynchronously persist request candidate DB batch"
            );
            retry_records.extend(chunk.iter().map(|record| record.record.clone()));
        } else {
            metrics
                .flushed_total
                .fetch_add(source_count as u64, Ordering::AcqRel);
            decrement_atomic_usize_by(&metrics.pending_current, source_count);
            if lane != RequestCandidateQueueLane::Normal {
                metrics
                    .priority_flushed_total
                    .fetch_add(source_count as u64, Ordering::AcqRel);
            }
            decrement_lifecycle_pending_by(metrics, lane, source_count);
        }
    }
    result
}

fn compact_retry_record(
    metrics: &RequestCandidateQueueMetrics,
    lane: RequestCandidateQueueLane,
    record: &CompactedRequestCandidateRecord,
) {
    let compacted = record.source_count.saturating_sub(1);
    decrement_atomic_usize_by(&metrics.pending_current, compacted);
    decrement_lifecycle_pending_by(metrics, lane, compacted);
}

fn decrement_lifecycle_pending_by(
    metrics: &RequestCandidateQueueMetrics,
    lane: RequestCandidateQueueLane,
    count: usize,
) {
    if lane == RequestCandidateQueueLane::Normal {
        return;
    }
    decrement_atomic_usize_by(&metrics.priority_pending_current, count);
    match lane {
        RequestCandidateQueueLane::Active => {
            decrement_atomic_usize_by(&metrics.active_pending_current, count);
        }
        RequestCandidateQueueLane::Terminal => {
            decrement_atomic_usize_by(&metrics.terminal_pending_current, count);
        }
        RequestCandidateQueueLane::Normal => {}
    }
}

#[derive(Debug)]
struct CompactedRequestCandidateRecord {
    record: UpsertRequestCandidateRecord,
    source_count: usize,
}

type RequestCandidateSlot = (String, u32, u32);

#[derive(Debug, Default)]
struct CompactedRequestCandidateFlushPlan {
    normal: Vec<CompactedRequestCandidateRecord>,
    pending: Vec<CompactedRequestCandidateRecord>,
    streaming: Vec<CompactedRequestCandidateRecord>,
    terminal: Vec<CompactedRequestCandidateRecord>,
}

impl CompactedRequestCandidateFlushPlan {
    fn record_count(&self) -> usize {
        self.normal
            .len()
            .saturating_add(self.pending.len())
            .saturating_add(self.streaming.len())
            .saturating_add(self.terminal.len())
    }
}

#[derive(Debug, Default)]
struct RequestCandidateSlotCompaction {
    normal: Option<CompactedRequestCandidateRecord>,
    pending: Option<CompactedRequestCandidateRecord>,
    streaming: Option<CompactedRequestCandidateRecord>,
    terminal: Option<CompactedRequestCandidateRecord>,
}

impl RequestCandidateSlotCompaction {
    fn merge(&mut self, record: UpsertRequestCandidateRecord) {
        let stage = match record.status {
            RequestCandidateStatus::Available | RequestCandidateStatus::Unused => &mut self.normal,
            RequestCandidateStatus::Pending => &mut self.pending,
            RequestCandidateStatus::Streaming => &mut self.streaming,
            RequestCandidateStatus::Success
            | RequestCandidateStatus::Failed
            | RequestCandidateStatus::Cancelled
            | RequestCandidateStatus::Skipped => &mut self.terminal,
        };
        match stage.as_mut() {
            Some(compacted) => {
                merge_request_candidate_record_for_flush(&mut compacted.record, record);
                compacted.source_count = compacted.source_count.saturating_add(1);
            }
            None => {
                *stage = Some(CompactedRequestCandidateRecord {
                    record,
                    source_count: 1,
                });
            }
        }
    }
}

fn append_compacted_stage(
    target: &mut Vec<CompactedRequestCandidateRecord>,
    stage: Option<CompactedRequestCandidateRecord>,
    inherited: &mut Option<UpsertRequestCandidateRecord>,
    status: Option<RequestCandidateStatus>,
    has_later_stage: bool,
) {
    let Some(mut stage) = stage else {
        return;
    };
    if let Some(mut inherited_record) = inherited.take() {
        merge_request_candidate_record_for_flush(&mut inherited_record, stage.record);
        stage.record = inherited_record;
    }
    if let Some(status) = status {
        stage.record.status = status;
    }
    if has_later_stage {
        *inherited = Some(stage.record.clone());
    }
    target.push(stage);
}

fn compact_records_for_flush(
    records: Vec<UpsertRequestCandidateRecord>,
) -> CompactedRequestCandidateFlushPlan {
    let mut latest_slot = HashMap::<RequestCandidateSlot, usize>::new();
    let mut compacted = Vec::<RequestCandidateSlotCompaction>::with_capacity(records.len());
    for record in records {
        let slot = request_candidate_slot(&record);
        match latest_slot.get(&slot).copied() {
            Some(index) => compacted[index].merge(record),
            _ => {
                latest_slot.insert(slot, compacted.len());
                let mut slot = RequestCandidateSlotCompaction::default();
                slot.merge(record);
                compacted.push(slot);
            }
        }
    }

    let mut plan = CompactedRequestCandidateFlushPlan {
        normal: Vec::with_capacity(compacted.len()),
        pending: Vec::with_capacity(compacted.len()),
        streaming: Vec::with_capacity(compacted.len()),
        terminal: Vec::with_capacity(compacted.len()),
    };
    for slot in compacted {
        let has_pending = slot.pending.is_some();
        let has_streaming = slot.streaming.is_some();
        let has_terminal = slot.terminal.is_some();
        let mut inherited = None;
        append_compacted_stage(
            &mut plan.normal,
            slot.normal,
            &mut inherited,
            None,
            has_pending || has_streaming || has_terminal,
        );
        append_compacted_stage(
            &mut plan.pending,
            slot.pending,
            &mut inherited,
            Some(RequestCandidateStatus::Pending),
            has_streaming || has_terminal,
        );
        append_compacted_stage(
            &mut plan.streaming,
            slot.streaming,
            &mut inherited,
            Some(RequestCandidateStatus::Streaming),
            has_terminal,
        );
        append_compacted_stage(
            &mut plan.terminal,
            slot.terminal,
            &mut inherited,
            None,
            false,
        );
    }
    plan
}

fn request_candidate_slot(record: &UpsertRequestCandidateRecord) -> RequestCandidateSlot {
    (
        record.request_id.clone(),
        record.candidate_index,
        record.retry_index,
    )
}

fn request_candidate_slots_are_unique(records: &[UpsertRequestCandidateRecord]) -> bool {
    let mut slots = HashSet::with_capacity(records.len());
    records
        .iter()
        .all(|record| slots.insert(request_candidate_slot(record)))
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

fn priority_queue_capacity(total_capacity: usize, workers: usize) -> usize {
    total_capacity.max(workers.max(1))
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
    ) -> tokio::sync::SemaphorePermit<'a> {
        if self.semaphore.available_permits() == 0 {
            metrics.db_write_wait_total.fetch_add(1, Ordering::AcqRel);
        }
        self.semaphore
            .acquire()
            .await
            .expect("request candidate DB write gate semaphore should not be closed")
    }
}

struct RequestCandidateDbWriteInFlightGuard<'a> {
    metrics: &'a RequestCandidateQueueMetrics,
}

impl<'a> RequestCandidateDbWriteInFlightGuard<'a> {
    fn new(metrics: &'a RequestCandidateQueueMetrics) -> Self {
        let in_flight = metrics.db_write_in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        metrics
            .db_write_max_in_flight
            .fetch_max(in_flight, Ordering::AcqRel);
        Self { metrics }
    }
}

impl Drop for RequestCandidateDbWriteInFlightGuard<'_> {
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
            | RequestCandidateStatus::Skipped
    )
}

fn request_candidate_status_is_active(status: RequestCandidateStatus) -> bool {
    matches!(
        status,
        RequestCandidateStatus::Pending | RequestCandidateStatus::Streaming
    )
}

fn request_candidate_status_is_priority(status: RequestCandidateStatus) -> bool {
    request_candidate_status_is_active(status) || request_candidate_status_is_terminal(status)
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

fn should_log_queue_counter(value: u64) -> bool {
    value <= 8 || value.is_power_of_two() || value.is_multiple_of(1_000)
}

#[cfg(test)]
mod tests {
    use super::{
        collect_active_micro_batch, compact_records_for_flush, flush_batch,
        parse_request_candidate_background_runtime_threads, run_worker,
        spawn_on_request_candidate_background_runtime, RequestCandidateActiveQueueMessage,
        RequestCandidateQueueConfig, RequestCandidateQueueLane, RequestCandidateQueueMetrics,
        RequestCandidateQueueRuntime, RequestCandidateTerminalBarrier,
        RequestCandidateTerminalQueueRecord, MAX_CONSECUTIVE_ACTIVE_FLUSHES,
        MAX_CONSECUTIVE_PRIORITY_FLUSHES,
    };
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::DataLayerError;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateReadRepository, RequestCandidateStatus, RequestCandidateWriteRepository,
        StoredRequestCandidate, UpsertRequestCandidateRecord,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Semaphore;

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

    struct BlockingFirstBatchRequestCandidateRepository {
        inner: InMemoryRequestCandidateRepository,
        batches: Mutex<Vec<Vec<UpsertRequestCandidateRecord>>>,
        block_first: AtomicBool,
        first_batch_started: tokio::sync::Notify,
        release_first_batch: tokio::sync::Notify,
    }

    struct BlockingStreamingBatchRequestCandidateRepository {
        inner: InMemoryRequestCandidateRepository,
        block_streaming: AtomicBool,
        streaming_batch_started: tokio::sync::Notify,
        release_streaming_batch: tokio::sync::Notify,
    }

    struct FailUntilReleasedRequestCandidateRepository {
        inner: InMemoryRequestCandidateRepository,
        failing: AtomicBool,
        attempts: AtomicUsize,
        first_attempt: tokio::sync::Notify,
    }

    impl Default for FailUntilReleasedRequestCandidateRepository {
        fn default() -> Self {
            Self {
                inner: InMemoryRequestCandidateRepository::default(),
                failing: AtomicBool::new(true),
                attempts: AtomicUsize::new(0),
                first_attempt: tokio::sync::Notify::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl RequestCandidateWriteRepository for FailUntilReleasedRequestCandidateRepository {
        async fn upsert(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<StoredRequestCandidate, DataLayerError> {
            self.inner.upsert(candidate).await
        }

        async fn upsert_many(
            &self,
            candidates: Vec<UpsertRequestCandidateRecord>,
        ) -> Result<usize, DataLayerError> {
            if self.attempts.fetch_add(1, Ordering::AcqRel) == 0 {
                self.first_attempt.notify_one();
            }
            if self.failing.load(Ordering::Acquire) {
                return Err(DataLayerError::UnexpectedValue(
                    "injected sustained request candidate failure".to_string(),
                ));
            }
            let count = candidates.len();
            for candidate in candidates {
                self.inner.upsert(candidate).await?;
            }
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

    #[derive(Default)]
    struct FailThenBlockPendingRetryRequestCandidateRepository {
        inner: InMemoryRequestCandidateRepository,
        batches: Mutex<Vec<Vec<UpsertRequestCandidateRecord>>>,
        pending_attempts: AtomicUsize,
        retry_started: tokio::sync::Notify,
        release_retry: tokio::sync::Notify,
    }

    impl Default for BlockingStreamingBatchRequestCandidateRepository {
        fn default() -> Self {
            Self {
                inner: InMemoryRequestCandidateRepository::default(),
                block_streaming: AtomicBool::new(true),
                streaming_batch_started: tokio::sync::Notify::new(),
                release_streaming_batch: tokio::sync::Notify::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl RequestCandidateWriteRepository for BlockingStreamingBatchRequestCandidateRepository {
        async fn upsert(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<StoredRequestCandidate, DataLayerError> {
            self.inner.upsert(candidate).await
        }

        async fn upsert_many(
            &self,
            candidates: Vec<UpsertRequestCandidateRecord>,
        ) -> Result<usize, DataLayerError> {
            if candidates
                .first()
                .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Streaming)
                && self.block_streaming.swap(false, Ordering::AcqRel)
            {
                self.streaming_batch_started.notify_one();
                self.release_streaming_batch.notified().await;
            }
            let count = candidates.len();
            for candidate in candidates {
                self.inner.upsert(candidate).await?;
            }
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

    #[async_trait::async_trait]
    impl RequestCandidateWriteRepository for FailThenBlockPendingRetryRequestCandidateRepository {
        async fn upsert(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<StoredRequestCandidate, DataLayerError> {
            self.inner.upsert(candidate).await
        }

        async fn upsert_many(
            &self,
            candidates: Vec<UpsertRequestCandidateRecord>,
        ) -> Result<usize, DataLayerError> {
            self.batches
                .lock()
                .expect("pending retry batch recorder lock")
                .push(candidates.clone());
            if candidates
                .first()
                .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Pending)
            {
                match self.pending_attempts.fetch_add(1, Ordering::AcqRel) {
                    0 => {
                        return Err(DataLayerError::UnexpectedValue(
                            "injected pending batch failure".to_string(),
                        ));
                    }
                    1 => {
                        self.retry_started.notify_one();
                        self.release_retry.notified().await;
                    }
                    _ => {}
                }
            }
            let count = candidates.len();
            for candidate in candidates {
                self.inner.upsert(candidate).await?;
            }
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

    #[derive(Default)]
    struct RecordingBatchRequestCandidateRepository {
        inner: InMemoryRequestCandidateRepository,
        batches: Mutex<Vec<Vec<UpsertRequestCandidateRecord>>>,
        fail_next_batch: AtomicBool,
    }

    impl RecordingBatchRequestCandidateRepository {
        fn fail_next_batch(&self) {
            self.fail_next_batch.store(true, Ordering::Release);
        }

        fn batches(&self) -> Vec<Vec<UpsertRequestCandidateRecord>> {
            self.batches.lock().expect("batch recorder lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl RequestCandidateWriteRepository for RecordingBatchRequestCandidateRepository {
        async fn upsert(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<StoredRequestCandidate, DataLayerError> {
            self.inner.upsert(candidate).await
        }

        async fn upsert_many(
            &self,
            candidates: Vec<UpsertRequestCandidateRecord>,
        ) -> Result<usize, DataLayerError> {
            assert!(super::request_candidate_slots_are_unique(&candidates));
            self.batches
                .lock()
                .expect("batch recorder lock")
                .push(candidates.clone());
            if self.fail_next_batch.swap(false, Ordering::AcqRel) {
                return Err(DataLayerError::UnexpectedValue(
                    "injected request candidate batch failure".to_string(),
                ));
            }
            let count = candidates.len();
            for candidate in candidates {
                self.inner.upsert(candidate).await?;
            }
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

    impl Default for BlockingFirstBatchRequestCandidateRepository {
        fn default() -> Self {
            Self {
                inner: InMemoryRequestCandidateRepository::default(),
                batches: Mutex::new(Vec::new()),
                block_first: AtomicBool::new(true),
                first_batch_started: tokio::sync::Notify::new(),
                release_first_batch: tokio::sync::Notify::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl RequestCandidateWriteRepository for BlockingFirstBatchRequestCandidateRepository {
        async fn upsert(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<StoredRequestCandidate, DataLayerError> {
            self.inner.upsert(candidate).await
        }

        async fn upsert_many(
            &self,
            candidates: Vec<UpsertRequestCandidateRecord>,
        ) -> Result<usize, DataLayerError> {
            self.batches
                .lock()
                .expect("batch recorder lock")
                .push(candidates.clone());
            if self.block_first.swap(false, Ordering::AcqRel) {
                self.first_batch_started.notify_one();
                self.release_first_batch.notified().await;
            }
            let count = candidates.len();
            for candidate in candidates {
                self.inner.upsert(candidate).await?;
            }
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
        let _db_batch = EnvGuard::unset(super::DB_BATCH_SIZE_ENV);

        let config = RequestCandidateQueueConfig::from_env();

        assert_eq!(config.mode, super::RequestCandidateWriteMode::Async);
        assert_eq!(
            config.full_policy,
            super::RequestCandidateQueueFullPolicy::Sync
        );
        assert_eq!(config.db_batch_size, 512);
    }

    #[test]
    fn candidate_background_runtime_threads_are_bounded() {
        assert_eq!(parse_request_candidate_background_runtime_threads(None), 1);
        assert_eq!(
            parse_request_candidate_background_runtime_threads(Some("0")),
            1
        );
        assert_eq!(
            parse_request_candidate_background_runtime_threads(Some("3")),
            3
        );
        assert_eq!(
            parse_request_candidate_background_runtime_threads(Some("999")),
            8
        );
        assert_eq!(
            parse_request_candidate_background_runtime_threads(Some("not-a-number")),
            1
        );
    }

    #[tokio::test]
    async fn candidate_queue_tasks_run_on_dedicated_runtime() {
        let thread_name = spawn_on_request_candidate_background_runtime(async {
            std::thread::current()
                .name()
                .unwrap_or_default()
                .to_string()
        })
        .await
        .expect("candidate background task should complete");

        assert_eq!(thread_name, "aether-candidate-queue");
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

        assert_eq!(compacted.normal.len(), 0);
        assert_eq!(compacted.pending.len(), 1);
        assert_eq!(compacted.streaming.len(), 0);
        assert_eq!(compacted.terminal.len(), 2);
        assert_eq!(
            compacted.pending[0].record.status,
            RequestCandidateStatus::Pending
        );
        assert_eq!(compacted.pending[0].source_count, 1);
        assert_eq!(
            compacted.terminal[0].record.status,
            RequestCandidateStatus::Success
        );
        assert_eq!(compacted.terminal[0].source_count, 2);
        assert_eq!(
            compacted.terminal[0].record.provider_id.as_deref(),
            Some("provider-a")
        );
        assert_eq!(compacted.terminal[0].record.latency_ms, Some(123));
        assert_eq!(
            compacted.terminal[0].record.extra_data,
            Some(serde_json::json!({"first": true, "second": true}))
        );
        assert_eq!(
            compacted.terminal[1].record.status,
            RequestCandidateStatus::Failed
        );
        assert_eq!(compacted.terminal[1].source_count, 1);
    }

    #[test]
    fn compact_splits_lifecycle_stages_in_monotonic_order() {
        let compacted = compact_records_for_flush(vec![
            record("req", 0, 0, RequestCandidateStatus::Success),
            record("req", 0, 0, RequestCandidateStatus::Streaming),
            record("req", 0, 0, RequestCandidateStatus::Unused),
        ]);

        assert_eq!(compacted.normal.len(), 1);
        assert_eq!(compacted.pending.len(), 0);
        assert_eq!(compacted.streaming.len(), 1);
        assert_eq!(compacted.terminal.len(), 1);
        assert_eq!(
            compacted.normal[0].record.status,
            RequestCandidateStatus::Unused
        );
        assert_eq!(compacted.normal[0].source_count, 1);
        assert_eq!(
            compacted.streaming[0].record.status,
            RequestCandidateStatus::Streaming
        );
        assert_eq!(compacted.streaming[0].source_count, 1);
        assert_eq!(
            compacted.terminal[0].record.status,
            RequestCandidateStatus::Success
        );
        assert_eq!(compacted.terminal[0].source_count, 1);
    }

    #[tokio::test]
    async fn active_micro_batch_collects_events_arriving_within_one_ms() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let metrics = RequestCandidateQueueMetrics::default();
        metrics.queued_current.store(1, Ordering::Release);
        metrics.priority_queued_current.store(1, Ordering::Release);
        metrics.active_queued_current.store(1, Ordering::Release);
        let first = record("micro-batch", 0, 0, RequestCandidateStatus::Streaming);
        let second = record("micro-batch", 1, 0, RequestCandidateStatus::Pending);
        // Seed the channel before starting the bounded receive window. Using
        // a scheduler yield here makes the test occasionally miss a sender
        // that did not run until after the intentionally tiny 1ms deadline.
        sender
            .send(RequestCandidateActiveQueueMessage::Record(second))
            .await
            .expect("micro-batch receiver open");
        let mut batch = vec![first];
        let mut barriers = Vec::new();
        let mut receiver_open = true;
        collect_active_micro_batch(
            &mut receiver,
            &mut batch,
            &mut barriers,
            4,
            &metrics,
            &mut receiver_open,
        )
        .await;

        assert_eq!(batch.len(), 2);
        assert!(barriers.is_empty());
        assert_eq!(metrics.queued_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.priority_queued_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.active_queued_current.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn sustained_priority_batches_give_ready_normal_records_a_turn() {
        let recorder = Arc::new(RecordingBatchRequestCandidateRepository::default());
        let repository: Arc<dyn RequestCandidateWriteRepository> = recorder.clone();
        let config = RequestCandidateQueueConfig {
            capacity: 16,
            batch_size: 1,
            db_batch_size: 1,
            flush_interval: Duration::from_secs(5),
            workers: 1,
            ..RequestCandidateQueueConfig::default()
        };
        let metrics = Arc::new(RequestCandidateQueueMetrics::default());
        let normal_admission = Arc::new(Semaphore::new(0));
        let priority_admission = Arc::new(Semaphore::new(0));
        let (normal_sender, normal_receiver) = tokio::sync::mpsc::channel(1);
        let (active_sender, active_receiver) = tokio::sync::mpsc::channel(16);
        let (terminal_sender, terminal_receiver) = tokio::sync::mpsc::channel(16);

        normal_sender
            .send(record(
                "normal-fairness",
                0,
                0,
                RequestCandidateStatus::Available,
            ))
            .await
            .expect("normal receiver open");
        for index in 0..(MAX_CONSECUTIVE_PRIORITY_FLUSHES * 2) {
            active_sender
                .send(RequestCandidateActiveQueueMessage::Record(record(
                    &format!("priority-fairness-{index}"),
                    0,
                    0,
                    RequestCandidateStatus::Streaming,
                )))
                .await
                .expect("priority receiver open");
        }
        drop(normal_sender);
        drop(active_sender);
        drop(terminal_sender);

        let priority_count = MAX_CONSECUTIVE_PRIORITY_FLUSHES * 2;
        metrics
            .queued_current
            .store(priority_count + 1, Ordering::Release);
        metrics
            .pending_current
            .store(priority_count + 1, Ordering::Release);
        metrics
            .priority_queued_current
            .store(priority_count, Ordering::Release);
        metrics
            .priority_pending_current
            .store(priority_count, Ordering::Release);
        metrics
            .active_queued_current
            .store(priority_count, Ordering::Release);
        metrics
            .active_pending_current
            .store(priority_count, Ordering::Release);

        run_worker(
            repository,
            config,
            Arc::clone(&metrics),
            None,
            0,
            normal_receiver,
            active_receiver,
            terminal_receiver,
            normal_admission,
            priority_admission,
        )
        .await;

        let batches = recorder.batches();
        let normal_batch_index = batches
            .iter()
            .position(|batch| {
                batch
                    .iter()
                    .any(|candidate| candidate.request_id == "normal-fairness")
            })
            .expect("normal record should be flushed");
        assert!(normal_batch_index <= MAX_CONSECUTIVE_PRIORITY_FLUSHES);
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.priority_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.active_pending_current.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn sustained_active_batches_give_ready_terminal_records_a_turn() {
        let recorder = Arc::new(RecordingBatchRequestCandidateRepository::default());
        let repository: Arc<dyn RequestCandidateWriteRepository> = recorder.clone();
        let config = RequestCandidateQueueConfig {
            capacity: 16,
            batch_size: 1,
            db_batch_size: 1,
            flush_interval: Duration::from_secs(5),
            workers: 1,
            ..RequestCandidateQueueConfig::default()
        };
        let metrics = Arc::new(RequestCandidateQueueMetrics::default());
        let normal_admission = Arc::new(Semaphore::new(0));
        let priority_admission = Arc::new(Semaphore::new(0));
        let (normal_sender, normal_receiver) = tokio::sync::mpsc::channel(1);
        let (active_sender, active_receiver) = tokio::sync::mpsc::channel(16);
        let (terminal_sender, terminal_receiver) = tokio::sync::mpsc::channel(16);
        let active_count = MAX_CONSECUTIVE_ACTIVE_FLUSHES * 3;
        for index in 0..active_count {
            active_sender
                .send(RequestCandidateActiveQueueMessage::Record(record(
                    &format!("active-fairness-{index}"),
                    0,
                    0,
                    RequestCandidateStatus::Streaming,
                )))
                .await
                .expect("active receiver open");
        }
        let barrier = Arc::new(RequestCandidateTerminalBarrier::new());
        barrier.ready.store(true, Ordering::Release);
        terminal_sender
            .send(RequestCandidateTerminalQueueRecord {
                record: record("terminal-fairness", 0, 0, RequestCandidateStatus::Success),
                barrier,
            })
            .await
            .expect("terminal receiver open");
        drop(normal_sender);
        drop(active_sender);
        drop(terminal_sender);

        let priority_count = active_count + 1;
        metrics
            .queued_current
            .store(priority_count, Ordering::Release);
        metrics
            .pending_current
            .store(priority_count, Ordering::Release);
        metrics
            .priority_queued_current
            .store(priority_count, Ordering::Release);
        metrics
            .priority_pending_current
            .store(priority_count, Ordering::Release);
        metrics
            .active_queued_current
            .store(active_count, Ordering::Release);
        metrics
            .active_pending_current
            .store(active_count, Ordering::Release);
        metrics.terminal_queued_current.store(1, Ordering::Release);
        metrics.terminal_pending_current.store(1, Ordering::Release);

        run_worker(
            repository,
            config,
            Arc::clone(&metrics),
            None,
            0,
            normal_receiver,
            active_receiver,
            terminal_receiver,
            normal_admission,
            priority_admission,
        )
        .await;

        let batches = recorder.batches();
        let terminal_batch_index = batches
            .iter()
            .position(|batch| {
                batch
                    .iter()
                    .any(|candidate| candidate.request_id == "terminal-fairness")
            })
            .expect("terminal record should be flushed");
        assert!(terminal_batch_index <= MAX_CONSECUTIVE_ACTIVE_FLUSHES);
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.priority_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.active_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.terminal_pending_current.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn sustained_barrier_stream_gives_ready_terminal_records_a_turn() {
        let recorder = Arc::new(RecordingBatchRequestCandidateRepository::default());
        let repository: Arc<dyn RequestCandidateWriteRepository> = recorder.clone();
        let config = RequestCandidateQueueConfig {
            capacity: 32,
            batch_size: 1,
            db_batch_size: 1,
            flush_interval: Duration::from_secs(5),
            workers: 1,
            ..RequestCandidateQueueConfig::default()
        };
        let metrics = Arc::new(RequestCandidateQueueMetrics::default());
        let normal_admission = Arc::new(Semaphore::new(0));
        let priority_admission = Arc::new(Semaphore::new(0));
        let (normal_sender, normal_receiver) = tokio::sync::mpsc::channel(1);
        let (active_sender, active_receiver) = tokio::sync::mpsc::channel(32);
        let (terminal_sender, terminal_receiver) = tokio::sync::mpsc::channel(1);
        drop(normal_sender);

        let terminal_barrier = Arc::new(RequestCandidateTerminalBarrier::new());
        terminal_barrier.ready.store(true, Ordering::Release);
        terminal_sender
            .send(RequestCandidateTerminalQueueRecord {
                record: record(
                    "terminal-amid-barriers",
                    0,
                    0,
                    RequestCandidateStatus::Success,
                ),
                barrier: terminal_barrier,
            })
            .await
            .expect("terminal receiver open");
        drop(terminal_sender);
        metrics.queued_current.store(1, Ordering::Release);
        metrics.pending_current.store(1, Ordering::Release);
        metrics.priority_queued_current.store(1, Ordering::Release);
        metrics.priority_pending_current.store(1, Ordering::Release);
        metrics.terminal_queued_current.store(1, Ordering::Release);
        metrics.terminal_pending_current.store(1, Ordering::Release);

        for _ in 0..32 {
            let barrier = Arc::new(RequestCandidateTerminalBarrier::new());
            barrier.ready.store(true, Ordering::Release);
            active_sender
                .send(RequestCandidateActiveQueueMessage::Barrier(barrier))
                .await
                .expect("active receiver open");
        }
        let keep_producing = Arc::new(AtomicBool::new(true));
        let keep_producing_for_task = Arc::clone(&keep_producing);
        let producer = tokio::spawn(async move {
            while keep_producing_for_task.load(Ordering::Acquire) {
                let barrier = Arc::new(RequestCandidateTerminalBarrier::new());
                barrier.ready.store(true, Ordering::Release);
                if active_sender
                    .send(RequestCandidateActiveQueueMessage::Barrier(barrier))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let worker = tokio::spawn(run_worker(
            repository,
            config,
            Arc::clone(&metrics),
            None,
            0,
            normal_receiver,
            active_receiver,
            terminal_receiver,
            normal_admission,
            priority_admission,
        ));
        let terminal_flushed = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if recorder.batches().iter().any(|batch| {
                    batch
                        .iter()
                        .any(|record| record.request_id == "terminal-amid-barriers")
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();

        keep_producing.store(false, Ordering::Release);
        producer.abort();
        let _ = producer.await;
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .expect("worker should stop after the barrier producer closes")
            .expect("barrier fairness worker should complete");
        assert!(
            terminal_flushed,
            "continuous barrier traffic must not starve a ready terminal record"
        );
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.terminal_pending_current.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn released_terminal_front_flushes_without_waiting_for_ticker() {
        let recorder = Arc::new(BlockingFirstBatchRequestCandidateRepository::default());
        let repository: Arc<dyn RequestCandidateWriteRepository> = recorder.clone();
        let config = RequestCandidateQueueConfig {
            capacity: 2,
            batch_size: 1,
            db_batch_size: 1,
            flush_interval: Duration::from_secs(5),
            workers: 1,
            ..RequestCandidateQueueConfig::default()
        };
        let metrics = Arc::new(RequestCandidateQueueMetrics::default());
        let normal_admission = Arc::new(Semaphore::new(0));
        let priority_admission = Arc::new(Semaphore::new(0));
        let (normal_sender, normal_receiver) = tokio::sync::mpsc::channel(1);
        let (active_sender, active_receiver) = tokio::sync::mpsc::channel(2);
        let (terminal_sender, terminal_receiver) = tokio::sync::mpsc::channel(2);
        let barrier = Arc::new(RequestCandidateTerminalBarrier::new());

        terminal_sender
            .send(RequestCandidateTerminalQueueRecord {
                record: record(
                    "terminal-after-barrier",
                    0,
                    0,
                    RequestCandidateStatus::Success,
                ),
                barrier: Arc::clone(&barrier),
            })
            .await
            .expect("terminal receiver open");
        normal_sender
            .send(record(
                "normal-wakeup-sentinel",
                0,
                0,
                RequestCandidateStatus::Available,
            ))
            .await
            .expect("normal receiver open");

        metrics.queued_current.store(2, Ordering::Release);
        metrics.pending_current.store(2, Ordering::Release);
        metrics.priority_queued_current.store(1, Ordering::Release);
        metrics.priority_pending_current.store(1, Ordering::Release);
        metrics.terminal_queued_current.store(1, Ordering::Release);
        metrics.terminal_pending_current.store(1, Ordering::Release);
        metrics.terminal_barrier_pending.store(1, Ordering::Release);

        let worker_metrics = Arc::clone(&metrics);
        let worker = tokio::spawn(async move {
            run_worker(
                repository,
                config,
                worker_metrics,
                None,
                0,
                normal_receiver,
                active_receiver,
                terminal_receiver,
                normal_admission,
                priority_admission,
            )
            .await;
        });

        tokio::time::timeout(
            Duration::from_secs(1),
            recorder.first_batch_started.notified(),
        )
        .await
        .expect("normal sentinel should start after terminal is parked at the front");
        recorder.release_first_batch.notify_one();
        active_sender
            .send(RequestCandidateActiveQueueMessage::Barrier(barrier))
            .await
            .expect("active receiver open");
        drop(normal_sender);
        drop(active_sender);
        drop(terminal_sender);

        tokio::time::timeout(Duration::from_millis(500), worker)
            .await
            .expect("released terminal front must not wait for the five-second ticker")
            .expect("candidate worker should finish");

        let batches = recorder
            .batches
            .lock()
            .expect("batch recorder lock")
            .clone();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0][0].request_id, "normal-wakeup-sentinel");
        assert_eq!(batches[1][0].request_id, "terminal-after-barrier");
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.terminal_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.terminal_barrier_pending.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn flush_commits_pending_streaming_and_terminal_in_distinct_batches() {
        let recorder = Arc::new(RecordingBatchRequestCandidateRepository::default());
        let repository: Arc<dyn RequestCandidateWriteRepository> = recorder.clone();
        let config = RequestCandidateQueueConfig {
            db_batch_size: 512,
            ..RequestCandidateQueueConfig::default()
        };
        let metrics = RequestCandidateQueueMetrics::default();
        metrics.pending_current.store(6, Ordering::Release);
        metrics.priority_pending_current.store(6, Ordering::Release);
        let mut batch = vec![
            record("ordered", 0, 0, RequestCandidateStatus::Pending),
            record("ordered", 0, 0, RequestCandidateStatus::Streaming),
            record("ordered", 0, 0, RequestCandidateStatus::Success),
            record("ordered", 1, 0, RequestCandidateStatus::Pending),
            record("ordered", 1, 0, RequestCandidateStatus::Streaming),
            record("ordered", 1, 0, RequestCandidateStatus::Failed),
        ];

        flush_batch(
            &repository,
            &config,
            &metrics,
            None,
            0,
            RequestCandidateQueueLane::Active,
            &mut batch,
            None,
        )
        .await;

        let batches = recorder.batches();
        assert_eq!(batches.len(), 3);
        assert!(batches[0]
            .iter()
            .all(|candidate| candidate.status == RequestCandidateStatus::Pending));
        assert!(batches[1]
            .iter()
            .all(|candidate| candidate.status == RequestCandidateStatus::Streaming));
        assert!(batches[2]
            .iter()
            .all(|candidate| super::request_candidate_status_is_terminal(candidate.status)));
        assert!(batches
            .iter()
            .all(|batch| super::request_candidate_slots_are_unique(batch)));
        assert!(batch.is_empty());
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.priority_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.flushed_total.load(Ordering::Acquire), 6);
        assert_eq!(metrics.priority_flushed_total.load(Ordering::Acquire), 6);
        assert_eq!(metrics.flush_sql_ops_total.load(Ordering::Acquire), 3);
    }

    #[tokio::test]
    async fn pending_is_visible_before_streaming_batch_starts() {
        let recorder = Arc::new(BlockingStreamingBatchRequestCandidateRepository::default());
        let repository: Arc<dyn RequestCandidateWriteRepository> = recorder.clone();
        let config = RequestCandidateQueueConfig {
            db_batch_size: 512,
            ..RequestCandidateQueueConfig::default()
        };
        let metrics = Arc::new(RequestCandidateQueueMetrics::default());
        metrics.pending_current.store(3, Ordering::Release);
        metrics.priority_pending_current.store(3, Ordering::Release);
        let worker_metrics = Arc::clone(&metrics);
        let worker_config = config.clone();
        let worker_repository = Arc::clone(&repository);
        let worker = tokio::spawn(async move {
            let mut batch = vec![
                record("visible", 0, 0, RequestCandidateStatus::Pending),
                record("visible", 0, 0, RequestCandidateStatus::Streaming),
                record("visible", 0, 0, RequestCandidateStatus::Success),
            ];
            flush_batch(
                &worker_repository,
                &worker_config,
                &worker_metrics,
                None,
                0,
                RequestCandidateQueueLane::Active,
                &mut batch,
                None,
            )
            .await;
        });

        tokio::time::timeout(
            Duration::from_secs(1),
            recorder.streaming_batch_started.notified(),
        )
        .await
        .expect("streaming stage should start after pending commit");
        let pending = recorder
            .inner
            .list_by_request_id("visible")
            .await
            .expect("pending candidate should be readable");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, RequestCandidateStatus::Pending);

        recorder.release_streaming_batch.notify_one();
        worker.await.expect("lifecycle flush should complete");
        let final_candidate = recorder
            .inner
            .list_by_request_id("visible")
            .await
            .expect("final candidate should be readable");
        assert_eq!(final_candidate[0].status, RequestCandidateStatus::Success);
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.priority_pending_current.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn failed_pending_stage_blocks_and_retries_later_lifecycle_stages() {
        let recorder = Arc::new(RecordingBatchRequestCandidateRepository::default());
        recorder.fail_next_batch();
        let repository: Arc<dyn RequestCandidateWriteRepository> = recorder.clone();
        let config = RequestCandidateQueueConfig {
            db_batch_size: 512,
            ..RequestCandidateQueueConfig::default()
        };
        let metrics = RequestCandidateQueueMetrics::default();
        metrics.pending_current.store(3, Ordering::Release);
        metrics.priority_pending_current.store(3, Ordering::Release);
        let mut batch = vec![
            record("retry-order", 0, 0, RequestCandidateStatus::Pending),
            record("retry-order", 0, 0, RequestCandidateStatus::Streaming),
            record("retry-order", 0, 0, RequestCandidateStatus::Success),
        ];

        flush_batch(
            &repository,
            &config,
            &metrics,
            None,
            0,
            RequestCandidateQueueLane::Active,
            &mut batch,
            None,
        )
        .await;

        let first_attempts = recorder.batches();
        assert_eq!(first_attempts.len(), 1);
        assert_eq!(first_attempts[0][0].status, RequestCandidateStatus::Pending);
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[0].status, RequestCandidateStatus::Pending);
        assert_eq!(batch[1].status, RequestCandidateStatus::Streaming);
        assert_eq!(batch[2].status, RequestCandidateStatus::Success);
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 3);
        assert_eq!(metrics.priority_pending_current.load(Ordering::Acquire), 3);
        assert_eq!(metrics.flush_failed_total.load(Ordering::Acquire), 1);

        flush_batch(
            &repository,
            &config,
            &metrics,
            None,
            0,
            RequestCandidateQueueLane::Active,
            &mut batch,
            None,
        )
        .await;

        let attempts = recorder.batches();
        assert_eq!(attempts.len(), 4);
        assert_eq!(attempts[1][0].status, RequestCandidateStatus::Pending);
        assert_eq!(attempts[2][0].status, RequestCandidateStatus::Streaming);
        assert_eq!(attempts[3][0].status, RequestCandidateStatus::Success);
        assert!(batch.is_empty());
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.priority_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.flushed_total.load(Ordering::Acquire), 3);
        assert_eq!(metrics.priority_flushed_total.load(Ordering::Acquire), 3);
    }

    #[tokio::test]
    async fn failed_active_retry_keeps_terminal_barrier_closed() {
        let repository = Arc::new(FailThenBlockPendingRetryRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 16,
                batch_size: 16,
                db_batch_size: 16,
                flush_interval: Duration::from_millis(10),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Sync,
            },
        );

        for status in [
            RequestCandidateStatus::Pending,
            RequestCandidateStatus::Streaming,
            RequestCandidateStatus::Success,
        ] {
            runtime
                .try_enqueue_priority_status(record("barrier-retry", 0, 0, status))
                .expect("lifecycle status should enter its split lane");
        }

        tokio::time::timeout(Duration::from_secs(1), repository.retry_started.notified())
            .await
            .expect("pending retry should start after the injected failure");
        assert_eq!(
            runtime
                .metrics
                .terminal_barrier_pending
                .load(Ordering::Acquire),
            1
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_pending_current
                .load(Ordering::Acquire),
            1
        );
        assert!(repository
            .batches
            .lock()
            .expect("pending retry batch recorder lock")
            .iter()
            .flatten()
            .all(|candidate| candidate.status == RequestCandidateStatus::Pending));

        repository.release_retry.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.metrics.pending_current.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("active retry and terminal lane should drain");

        let statuses = repository
            .batches
            .lock()
            .expect("pending retry batch recorder lock")
            .iter()
            .flatten()
            .map(|candidate| candidate.status)
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                RequestCandidateStatus::Pending,
                RequestCandidateStatus::Pending,
                RequestCandidateStatus::Streaming,
                RequestCandidateStatus::Success,
            ]
        );
        assert_eq!(
            runtime
                .metrics
                .active_pending_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_pending_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_barrier_pending
                .load(Ordering::Acquire),
            0
        );
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
    async fn streaming_status_flushes_ahead_of_normal_backlog() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 64,
                batch_size: 64,
                db_batch_size: 64,
                flush_interval: Duration::from_secs(5),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Drop,
            },
        );

        runtime
            .enqueue_or_fallback(record(
                "priority-target",
                0,
                0,
                RequestCandidateStatus::Pending,
            ))
            .await
            .unwrap();
        for index in 0..32 {
            runtime
                .enqueue_or_fallback(record(
                    &format!("normal-backlog-{index}"),
                    0,
                    0,
                    RequestCandidateStatus::Available,
                ))
                .await
                .unwrap();
        }
        runtime
            .enqueue_or_fallback(record(
                "priority-target",
                0,
                0,
                RequestCandidateStatus::Streaming,
            ))
            .await
            .unwrap();

        let candidate = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = repository
                    .list_by_request_id("priority-target")
                    .await
                    .unwrap();
                if let Some(candidate) = candidates
                    .into_iter()
                    .find(|candidate| candidate.status == RequestCandidateStatus::Streaming)
                    .filter(|_| {
                        runtime
                            .metrics
                            .priority_flushed_total
                            .load(Ordering::Acquire)
                            >= 2
                    })
                {
                    break candidate;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("streaming status should bypass the normal persistence backlog");

        assert_eq!(candidate.status, RequestCandidateStatus::Streaming);
        assert_eq!(
            runtime
                .metrics
                .priority_flushed_total
                .load(Ordering::Acquire),
            2
        );
        assert!(runtime.metrics.pending_current.load(Ordering::Acquire) > 0);
    }

    #[tokio::test]
    async fn active_lifecycle_bypasses_queued_terminal_backlog_after_current_write() {
        let repository = Arc::new(BlockingFirstBatchRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 64,
                batch_size: 64,
                db_batch_size: 64,
                flush_interval: Duration::from_secs(5),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Sync,
            },
        );

        runtime
            .try_enqueue_priority_status(record(
                "terminal-blocker",
                0,
                0,
                RequestCandidateStatus::Success,
            ))
            .expect("terminal blocker should enter the terminal lane");
        tokio::time::timeout(
            Duration::from_secs(1),
            repository.first_batch_started.notified(),
        )
        .await
        .expect("first terminal DB batch should start");

        for index in 0..8 {
            runtime
                .try_enqueue_priority_status(record(
                    &format!("terminal-backlog-{index}"),
                    0,
                    0,
                    RequestCandidateStatus::Success,
                ))
                .expect("terminal backlog should enter the terminal lane");
        }
        for status in [
            RequestCandidateStatus::Pending,
            RequestCandidateStatus::Streaming,
            RequestCandidateStatus::Success,
        ] {
            runtime
                .try_enqueue_priority_status(record("active-target", 0, 0, status))
                .expect("target lifecycle should enter its lane");
        }

        repository.release_first_batch.notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.metrics.pending_current.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("split lifecycle lanes should fully drain");

        let batches = repository
            .batches
            .lock()
            .expect("batch recorder lock")
            .clone();
        let batch_index = |request_id: &str, status: RequestCandidateStatus| {
            batches
                .iter()
                .position(|batch| {
                    batch.iter().any(|candidate| {
                        candidate.request_id == request_id && candidate.status == status
                    })
                })
                .expect("expected lifecycle batch")
        };
        let pending_index = batch_index("active-target", RequestCandidateStatus::Pending);
        let streaming_index = batch_index("active-target", RequestCandidateStatus::Streaming);
        let terminal_index = batch_index("active-target", RequestCandidateStatus::Success);
        let backlog_index = batch_index("terminal-backlog-0", RequestCandidateStatus::Success);
        assert!(pending_index < streaming_index);
        assert!(streaming_index < terminal_index);
        assert!(streaming_index < backlog_index);
        assert_eq!(
            runtime
                .metrics
                .active_queued_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .active_pending_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_queued_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_pending_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_barrier_pending
                .load(Ordering::Acquire),
            0
        );
    }

    #[tokio::test]
    async fn bounded_priority_backpressure_preserves_fifo_order_while_db_is_blocked() {
        let repository = Arc::new(BlockingFirstBatchRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 2,
                batch_size: 1,
                db_batch_size: 1,
                flush_interval: Duration::from_secs(5),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Sync,
            },
        );

        runtime
            .enqueue_or_fallback(record(
                "priority-blocker",
                0,
                0,
                RequestCandidateStatus::Streaming,
            ))
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            repository.first_batch_started.notified(),
        )
        .await
        .expect("priority worker should start its first DB batch");
        runtime
            .try_enqueue_priority_status(record(
                "priority-order",
                0,
                0,
                RequestCandidateStatus::Pending,
            ))
            .expect("pending status should synchronously enter the priority lane");
        let overflow = runtime
            .try_enqueue_priority_status(record(
                "priority-order",
                0,
                0,
                RequestCandidateStatus::Streaming,
            ))
            .expect_err("full lifecycle admission should apply backpressure");
        let runtime_for_tail = Arc::clone(&runtime);
        let tail = tokio::spawn(async move {
            runtime_for_tail
                .enqueue_or_fallback(overflow)
                .await
                .expect("streaming status should wait for bounded admission");
            runtime_for_tail
                .enqueue_or_fallback(record(
                    "priority-order",
                    0,
                    0,
                    RequestCandidateStatus::Success,
                ))
                .await
                .expect("terminal status should retain FIFO order behind streaming");
        });
        tokio::task::yield_now().await;
        assert!(
            !tail.is_finished(),
            "overflow enqueue should be backpressured"
        );
        assert_eq!(
            runtime
                .metrics
                .active_queued_current
                .load(Ordering::Acquire),
            1
        );
        assert_eq!(
            runtime.metrics.priority_max_queued.load(Ordering::Acquire),
            1
        );
        assert_eq!(runtime.priority_admission.available_permits(), 0);
        assert!(
            runtime
                .metrics
                .priority_async_overflow_total
                .load(Ordering::Acquire)
                > 0
        );

        repository.release_first_batch.notify_one();
        tokio::time::timeout(Duration::from_secs(2), tail)
            .await
            .expect("bounded lifecycle tail should resume after DB progress")
            .expect("bounded lifecycle tail task should complete");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if runtime.metrics.pending_current.load(Ordering::Acquire) == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("ordered priority lane should drain after the first DB batch is released");

        let statuses = repository
            .batches
            .lock()
            .expect("batch recorder lock")
            .iter()
            .flatten()
            .filter(|candidate| candidate.request_id == "priority-order")
            .map(|candidate| candidate.status)
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec![
                RequestCandidateStatus::Pending,
                RequestCandidateStatus::Streaming,
                RequestCandidateStatus::Success,
            ]
        );
        assert_eq!(
            runtime
                .metrics
                .active_queued_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .active_pending_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_queued_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_pending_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_barrier_pending
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(runtime.priority_admission.available_permits(), 2);
    }

    #[tokio::test]
    async fn sustained_db_failure_keeps_lifecycle_resident_set_bounded() {
        let repository = Arc::new(FailUntilReleasedRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 2,
                batch_size: 1,
                db_batch_size: 1,
                flush_interval: Duration::from_millis(5),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Drop,
            },
        );

        runtime
            .try_enqueue_priority_status(record(
                "outage-bounded-1",
                0,
                0,
                RequestCandidateStatus::Pending,
            ))
            .expect("first lifecycle record should enter admission");
        tokio::time::timeout(Duration::from_secs(1), repository.first_attempt.notified())
            .await
            .expect("worker should observe the injected DB failure");
        runtime
            .try_enqueue_priority_status(record(
                "outage-bounded-2",
                0,
                0,
                RequestCandidateStatus::Pending,
            ))
            .expect("second lifecycle record should fill admission");
        let overflow = runtime
            .try_enqueue_priority_status(record(
                "outage-bounded-3",
                0,
                0,
                RequestCandidateStatus::Pending,
            ))
            .expect_err("third lifecycle record must observe bounded admission");
        let runtime_for_overflow = Arc::clone(&runtime);
        let overflow_task = tokio::spawn(async move {
            runtime_for_overflow
                .enqueue_or_fallback(overflow)
                .await
                .expect("lossless lifecycle overflow should resume after DB recovery");
        });

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!overflow_task.is_finished());
        assert_eq!(runtime.priority_admission.available_permits(), 0);
        assert_eq!(
            runtime
                .metrics
                .priority_pending_current
                .load(Ordering::Acquire),
            2
        );
        assert_eq!(
            runtime.metrics.enqueued_total.load(Ordering::Acquire),
            2,
            "sustained failures must not admit an unbounded retry resident set"
        );
        assert!(repository.attempts.load(Ordering::Acquire) >= 2);

        repository.failing.store(false, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(2), overflow_task)
            .await
            .expect("overflow should resume after repository recovery")
            .expect("overflow task should complete");
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.metrics.pending_current.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("bounded lifecycle retries should fully drain after recovery");
        assert_eq!(runtime.priority_admission.available_permits(), 2);
        for request_id in ["outage-bounded-1", "outage-bounded-2", "outage-bounded-3"] {
            assert_eq!(
                repository
                    .inner
                    .list_by_request_id(request_id)
                    .await
                    .expect("recovered lifecycle record should be readable")
                    .len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn sustained_db_failure_keeps_normal_resident_set_bounded() {
        let repository = Arc::new(FailUntilReleasedRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 2,
                batch_size: 1,
                db_batch_size: 1,
                flush_interval: Duration::from_millis(5),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Drop,
            },
        );

        runtime
            .enqueue_or_fallback(record(
                "normal-outage-bounded-1",
                0,
                0,
                RequestCandidateStatus::Available,
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), repository.first_attempt.notified())
            .await
            .expect("worker should observe the injected normal-lane DB failure");
        runtime
            .enqueue_or_fallback(record(
                "normal-outage-bounded-2",
                0,
                0,
                RequestCandidateStatus::Available,
            ))
            .await
            .unwrap();
        runtime
            .enqueue_or_fallback(record(
                "normal-outage-dropped",
                0,
                0,
                RequestCandidateStatus::Available,
            ))
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(runtime.normal_admission.available_permits(), 0);
        assert_eq!(runtime.metrics.pending_current.load(Ordering::Acquire), 2);
        assert_eq!(runtime.metrics.enqueued_total.load(Ordering::Acquire), 2);
        assert_eq!(runtime.metrics.dropped_total.load(Ordering::Acquire), 1);
        assert!(repository.attempts.load(Ordering::Acquire) >= 2);

        repository.failing.store(false, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.metrics.pending_current.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("bounded normal retries should fully drain after recovery");
        assert_eq!(runtime.normal_admission.available_permits(), 2);
        for request_id in ["normal-outage-bounded-1", "normal-outage-bounded-2"] {
            assert_eq!(
                repository
                    .inner
                    .list_by_request_id(request_id)
                    .await
                    .expect("recovered normal record should be readable")
                    .len(),
                1
            );
        }
        assert!(repository
            .inner
            .list_by_request_id("normal-outage-dropped")
            .await
            .expect("dropped normal record lookup should succeed")
            .is_empty());
    }

    #[tokio::test]
    async fn terminal_lane_preserves_fifo_last_write_wins_for_same_slot() {
        let repository = Arc::new(RecordingBatchRequestCandidateRepository::default());
        let runtime = RequestCandidateQueueRuntime::spawn(
            repository.clone(),
            RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 16,
                batch_size: 16,
                db_batch_size: 16,
                flush_interval: Duration::from_millis(10),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Sync,
            },
        );

        for status in [
            RequestCandidateStatus::Pending,
            RequestCandidateStatus::Streaming,
            RequestCandidateStatus::Failed,
            RequestCandidateStatus::Success,
        ] {
            runtime
                .try_enqueue_priority_status(record("terminal-last-write", 0, 0, status))
                .expect("lifecycle status should enter its split lane");
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.metrics.pending_current.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal last-write lifecycle should drain");

        let stored = repository
            .inner
            .list_by_request_id("terminal-last-write")
            .await
            .expect("terminal last-write candidate should be readable");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, RequestCandidateStatus::Success);
        let terminal_statuses = repository
            .batches()
            .into_iter()
            .flatten()
            .filter(|candidate| super::request_candidate_status_is_terminal(candidate.status))
            .map(|candidate| candidate.status)
            .collect::<Vec<_>>();
        assert_eq!(terminal_statuses, vec![RequestCandidateStatus::Success]);
        assert_eq!(runtime.metrics.compacted_total.load(Ordering::Acquire), 1);
        assert_eq!(
            runtime
                .metrics
                .terminal_pending_current
                .load(Ordering::Acquire),
            0
        );
        assert_eq!(
            runtime
                .metrics
                .terminal_barrier_pending
                .load(Ordering::Acquire),
            0
        );
    }

    #[tokio::test]
    async fn closed_priority_fast_path_falls_back_without_terminal_regression() {
        let repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let (normal_sender, normal_receiver) = tokio::sync::mpsc::channel(1);
        let (active_sender, active_receiver) = tokio::sync::mpsc::channel(1);
        let (terminal_sender, terminal_receiver) = tokio::sync::mpsc::channel(1);
        drop(normal_receiver);
        drop(active_receiver);
        drop(terminal_receiver);
        let metrics = Arc::new(RequestCandidateQueueMetrics::default());
        let runtime = RequestCandidateQueueRuntime {
            senders: vec![normal_sender],
            active_senders: vec![active_sender],
            terminal_senders: vec![terminal_sender],
            normal_admission: Arc::new(Semaphore::new(1)),
            priority_admission: Arc::new(Semaphore::new(1)),
            priority_capacity: 1,
            repository: repository.clone(),
            config: RequestCandidateQueueConfig {
                mode: super::RequestCandidateWriteMode::Async,
                capacity: 1,
                batch_size: 1,
                db_batch_size: 1,
                flush_interval: Duration::from_secs(1),
                workers: 1,
                db_write_concurrency_limit: None,
                full_policy: super::RequestCandidateQueueFullPolicy::Sync,
            },
            metrics: Arc::clone(&metrics),
            db_write_gate: None,
        };

        for status in [
            RequestCandidateStatus::Pending,
            RequestCandidateStatus::Streaming,
            RequestCandidateStatus::Success,
            RequestCandidateStatus::Streaming,
        ] {
            let fallback = runtime
                .try_enqueue_priority_status(record("closed-priority-fallback", 0, 0, status))
                .expect_err("closed priority lane must return the unpersisted record");
            assert_eq!(fallback.status, status);
            runtime.enqueue_or_fallback(fallback).await.unwrap();
        }

        let rows = repository
            .list_by_request_id("closed-priority-fallback")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, RequestCandidateStatus::Success);
        assert_eq!(metrics.queued_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.priority_queued_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.priority_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.active_queued_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.active_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.terminal_queued_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.terminal_pending_current.load(Ordering::Acquire), 0);
        assert_eq!(metrics.terminal_barrier_pending.load(Ordering::Acquire), 0);
        assert_eq!(metrics.sync_fallback_total.load(Ordering::Acquire), 4);
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
                assert_eq!(
                    runtime
                        .metrics
                        .db_write_max_in_flight
                        .load(Ordering::Acquire),
                    1
                );
                assert_eq!(
                    runtime.metrics.db_write_in_flight.load(Ordering::Acquire),
                    0
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
