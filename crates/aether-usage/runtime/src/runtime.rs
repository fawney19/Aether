use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant};

use aether_contracts::ExecutionTelemetry;
use aether_data_contracts::repository::usage::UpsertUsageRecord;
use aether_data_contracts::DataLayerError;
use aether_runtime_state::{RuntimeQueueStats, RuntimeQueueStore};
use async_trait::async_trait;
use futures_util::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::executor::spawn_on_usage_background_runtime;
use crate::request_metadata::{
    attach_client_request_body_metadata, attach_provider_request_body_metadata,
    attach_provider_response_body_metadata, clear_client_request_body_metadata,
    clear_provider_request_body_metadata, request_body_derived_facts_action,
    retain_first_byte_request_metadata, RequestBodyDerivedFactsAction,
};
use crate::worker::{
    build_usage_queue_worker_with_record_gate, UsageWorkerControl, UsageWorkerObservation,
};
use crate::{
    apply_usage_body_capture_policy_to_event, build_stream_terminal_usage_seed,
    build_sync_terminal_usage_seed, build_terminal_usage_event_from_seed,
    build_upsert_usage_record_from_event, settle_usage_if_needed, LifecycleUsageSeed,
    StreamTerminalUsagePayloadSeed, SyncTerminalUsagePayloadSeed, TerminalUsageContextSeed,
    UsageEvent, UsageQueue, UsageRecordWriter, UsageRuntimeConfig, UsageSettlementWriter,
};

#[async_trait]
pub trait UsageBillingEventEnricher: Send + Sync {
    async fn enrich_usage_event(&self, event: &mut UsageEvent) -> Result<(), DataLayerError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageRequestRecordLevel {
    Basic,
    #[default]
    Full,
}

pub const DEFAULT_USAGE_REQUEST_BODY_CAPTURE_LIMIT_BYTES: usize = usize::MAX;
pub const DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES: usize = usize::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageBodyCapturePolicy {
    pub record_level: UsageRequestRecordLevel,
    pub compact_base64_images: bool,
    pub max_body_bytes: Option<usize>,
}

impl Default for UsageBodyCapturePolicy {
    fn default() -> Self {
        Self {
            record_level: UsageRequestRecordLevel::Full,
            compact_base64_images: false,
            max_body_bytes: None,
        }
    }
}

#[async_trait]
pub trait UsageRuntimeAccess:
    UsageRecordWriter + UsageSettlementWriter + UsageBillingEventEnricher + Send + Sync
{
    fn has_usage_writer(&self) -> bool;
    fn has_usage_worker_queue(&self) -> bool;
    fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>>;
    fn supports_first_byte_usage_fast_path(&self) -> bool {
        false
    }
    fn usage_worker_should_defer_for_database_pressure(&self) -> bool {
        false
    }

    async fn body_capture_policy(&self) -> Result<UsageBodyCapturePolicy, DataLayerError> {
        Ok(UsageBodyCapturePolicy::default())
    }

    async fn request_record_level(&self) -> Result<UsageRequestRecordLevel, DataLayerError> {
        Ok(self.body_capture_policy().await?.record_level)
    }
}

#[derive(Debug, Clone)]
pub struct UsageRuntime {
    config: UsageRuntimeConfig,
    body_policy_cache: Arc<tokio::sync::Mutex<Option<UsageBodyCapturePolicyCacheEntry>>>,
    enqueue_retry: Arc<UsageEnqueueRetryDispatcher>,
    worker_supervisor_state: Arc<UsageWorkerSupervisorState>,
    worker_record_gate: Option<Arc<UsageWorkerRecordConcurrencyGate>>,
    terminal_submission_state: Arc<TerminalSubmissionState>,
    terminal_execution: Arc<TerminalExecutionDispatcher>,
    terminal_enqueue_state: Arc<LifecycleEnqueueState>,
    terminal_direct_fallback_state: Arc<TerminalDirectFallbackState>,
    lifecycle_enqueue_state: Arc<LifecycleEnqueueState>,
    lifecycle_coalescer: Arc<LifecycleEventCoalescer>,
    lifecycle_delay: Arc<LifecycleDelayDispatcher>,
    lifecycle_submission: Arc<LifecycleSubmissionDispatcher>,
    ordered_lifecycle: Arc<OrderedLifecycleDispatcher>,
    pending_persistence: Arc<PendingPersistenceDispatcher>,
    first_byte_persistence: Arc<FirstBytePersistenceDispatcher>,
}

#[derive(Debug, Default)]
struct UsageWorkerSupervisorState {
    active_count: AtomicUsize,
    desired_count: AtomicUsize,
    read_batches_total: AtomicU64,
    read_entries_total: AtomicU64,
    reclaimed_entries_total: AtomicU64,
    acked_entries_total: AtomicU64,
    dead_lettered_entries_total: AtomicU64,
    process_failures_total: AtomicU64,
    read_failures_total: AtomicU64,
    reclaim_failures_total: AtomicU64,
}

#[derive(Debug)]
pub(crate) struct UsageWorkerRecordConcurrencyGate {
    semaphore: tokio::sync::Semaphore,
    limit: usize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    wait_total: AtomicU64,
    deferred_total: AtomicU64,
}

impl UsageWorkerRecordConcurrencyGate {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(limit.max(1)),
            limit: limit.max(1),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            wait_total: AtomicU64::new(0),
            deferred_total: AtomicU64::new(0),
        }
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    pub(crate) fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::Acquire)
    }

    pub(crate) fn wait_total(&self) -> u64 {
        self.wait_total.load(Ordering::Acquire)
    }

    pub(crate) fn deferred_total(&self) -> u64 {
        self.deferred_total.load(Ordering::Acquire)
    }

    pub(crate) fn record_deferred(&self) {
        self.deferred_total.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) async fn acquire(&self) -> UsageWorkerRecordConcurrencyPermit<'_> {
        if self.semaphore.available_permits() == 0 {
            self.wait_total.fetch_add(1, Ordering::AcqRel);
        }
        let permit = self
            .semaphore
            .acquire()
            .await
            .expect("usage worker record gate semaphore should not be closed");
        let active = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_in_flight.fetch_max(active, Ordering::AcqRel);
        UsageWorkerRecordConcurrencyPermit {
            gate: self,
            _permit: permit,
        }
    }

    fn try_acquire(&self) -> Option<UsageWorkerRecordConcurrencyPermit<'_>> {
        let permit = self.semaphore.try_acquire().ok()?;
        let active = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_in_flight.fetch_max(active, Ordering::AcqRel);
        Some(UsageWorkerRecordConcurrencyPermit {
            gate: self,
            _permit: permit,
        })
    }
}

pub(crate) struct UsageWorkerRecordConcurrencyPermit<'a> {
    gate: &'a UsageWorkerRecordConcurrencyGate,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl Drop for UsageWorkerRecordConcurrencyPermit<'_> {
    fn drop(&mut self) {
        self.gate.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct TerminalSubmissionState {
    semaphore: Arc<tokio::sync::Semaphore>,
    limit: usize,
    pending: AtomicUsize,
    max_pending: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    rejected_total: AtomicU64,
}

impl TerminalSubmissionState {
    fn new(limit: usize) -> Self {
        let limit = limit.clamp(1, TERMINAL_SUBMISSION_MAX_LIMIT);
        Self {
            semaphore: Arc::new(tokio::sync::Semaphore::new(limit)),
            limit,
            pending: AtomicUsize::new(0),
            max_pending: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            rejected_total: AtomicU64::new(0),
        }
    }

    fn register_pending(self: &Arc<Self>) -> TerminalSubmissionPendingGuard {
        let pending = self.pending.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_pending.fetch_max(pending, Ordering::AcqRel);
        TerminalSubmissionPendingGuard {
            state: Arc::clone(self),
        }
    }

    async fn acquire(self: &Arc<Self>) -> Option<TerminalSubmissionPermit> {
        let pending_guard = self.register_pending();
        self.acquire_registered(pending_guard).await
    }

    async fn acquire_registered(
        self: &Arc<Self>,
        pending_guard: TerminalSubmissionPendingGuard,
    ) -> Option<TerminalSubmissionPermit> {
        let permit = match Arc::clone(&self.semaphore).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                self.rejected_total.fetch_add(1, Ordering::AcqRel);
                return None;
            }
        };
        let active = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_in_flight.fetch_max(active, Ordering::AcqRel);
        Some(TerminalSubmissionPermit {
            state: Arc::clone(self),
            _permit: permit,
            _pending_guard: pending_guard,
        })
    }

    fn limit(&self) -> usize {
        self.limit
    }

    fn pending(&self) -> usize {
        self.pending.load(Ordering::Acquire)
    }

    fn max_pending(&self) -> usize {
        self.max_pending.load(Ordering::Acquire)
    }

    fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::Acquire)
    }

    fn rejected_total(&self) -> u64 {
        self.rejected_total.load(Ordering::Acquire)
    }
}

struct TerminalSubmissionPendingGuard {
    state: Arc<TerminalSubmissionState>,
}

impl Drop for TerminalSubmissionPendingGuard {
    fn drop(&mut self) {
        self.state.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

struct TerminalSubmissionPermit {
    state: Arc<TerminalSubmissionState>,
    _permit: tokio::sync::OwnedSemaphorePermit,
    _pending_guard: TerminalSubmissionPendingGuard,
}

impl Drop for TerminalSubmissionPermit {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::AcqRel);
        // `_pending_guard` is dropped after this method, keeping pending
        // non-zero until the active submission has released its permit.
    }
}

fn terminal_submission_limit(config: &UsageRuntimeConfig) -> usize {
    usize::try_from(config.terminal_submission_max_in_flight)
        .unwrap_or(TERMINAL_SUBMISSION_MAX_LIMIT)
        .clamp(1, TERMINAL_SUBMISSION_MAX_LIMIT)
}

#[derive(Debug)]
struct TerminalDirectFallbackState {
    semaphore: tokio::sync::Semaphore,
    limit: usize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    succeeded_total: AtomicU64,
    failed_total: AtomicU64,
    rejected_total: AtomicU64,
}

impl TerminalDirectFallbackState {
    fn new(limit: usize) -> Self {
        let limit = limit.max(1);
        Self {
            semaphore: tokio::sync::Semaphore::new(limit),
            limit,
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            succeeded_total: AtomicU64::new(0),
            failed_total: AtomicU64::new(0),
            rejected_total: AtomicU64::new(0),
        }
    }

    fn try_acquire(&self) -> Option<TerminalDirectFallbackPermit<'_>> {
        let permit = match self.semaphore.try_acquire() {
            Ok(permit) => permit,
            Err(_) => {
                self.record_rejected();
                return None;
            }
        };
        let active = self.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_in_flight.fetch_max(active, Ordering::AcqRel);
        Some(TerminalDirectFallbackPermit {
            state: self,
            _permit: permit,
        })
    }

    fn record_succeeded(&self) -> u64 {
        self.succeeded_total.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn record_failed(&self) -> u64 {
        self.failed_total.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn record_rejected(&self) -> u64 {
        self.rejected_total.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn limit(&self) -> usize {
        self.limit
    }

    fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }

    fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::Acquire)
    }

    fn succeeded_total(&self) -> u64 {
        self.succeeded_total.load(Ordering::Acquire)
    }

    fn failed_total(&self) -> u64 {
        self.failed_total.load(Ordering::Acquire)
    }

    fn rejected_total(&self) -> u64 {
        self.rejected_total.load(Ordering::Acquire)
    }
}

struct TerminalDirectFallbackPermit<'a> {
    state: &'a TerminalDirectFallbackState,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

impl Drop for TerminalDirectFallbackPermit<'_> {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

fn terminal_direct_fallback_limit(config: &UsageRuntimeConfig) -> usize {
    config
        .worker_record_concurrency_limit
        .unwrap_or(TERMINAL_DIRECT_FALLBACK_DEFAULT_MAX_IN_FLIGHT)
        .max(1)
}

impl UsageWorkerSupervisorState {
    fn record_observation(&self, observation: UsageWorkerObservation) {
        if observation.entries_read > 0 || observation.batch_size > 0 {
            self.read_batches_total.fetch_add(1, Ordering::AcqRel);
            self.read_entries_total.fetch_add(
                u64::try_from(observation.entries_read).unwrap_or(u64::MAX),
                Ordering::AcqRel,
            );
        }
        self.reclaimed_entries_total.fetch_add(
            u64::try_from(observation.reclaimed_entries).unwrap_or(u64::MAX),
            Ordering::AcqRel,
        );
        self.acked_entries_total.fetch_add(
            u64::try_from(observation.acked_entries).unwrap_or(u64::MAX),
            Ordering::AcqRel,
        );
        self.dead_lettered_entries_total.fetch_add(
            u64::try_from(observation.dead_lettered_entries).unwrap_or(u64::MAX),
            Ordering::AcqRel,
        );
        self.process_failures_total.fetch_add(
            u64::try_from(observation.process_failures).unwrap_or(u64::MAX),
            Ordering::AcqRel,
        );
        self.read_failures_total.fetch_add(
            u64::try_from(observation.read_failures).unwrap_or(u64::MAX),
            Ordering::AcqRel,
        );
        self.reclaim_failures_total.fetch_add(
            u64::try_from(observation.reclaim_failures).unwrap_or(u64::MAX),
            Ordering::AcqRel,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageRuntimeMetricsSnapshot {
    pub enabled: bool,
    pub queue_terminal_events: bool,
    pub queue_lifecycle_events: bool,
    pub worker_count: usize,
    pub worker_autoscale_enabled: bool,
    pub worker_max_count: usize,
    pub worker_record_concurrency_limit: Option<usize>,
    pub worker_record_concurrency_in_flight: usize,
    pub worker_record_concurrency_max_in_flight: usize,
    pub worker_record_concurrency_wait_total: u64,
    pub worker_record_deferred_total: u64,
    pub worker_active_count: usize,
    pub worker_desired_count: usize,
    pub worker_read_batches_total: u64,
    pub worker_read_entries_total: u64,
    pub worker_reclaimed_entries_total: u64,
    pub worker_acked_entries_total: u64,
    pub worker_dead_lettered_entries_total: u64,
    pub worker_process_failures_total: u64,
    pub worker_read_failures_total: u64,
    pub worker_reclaim_failures_total: u64,
    pub retry_deferred_lifecycle_events: bool,
    pub terminal_submission_limit: usize,
    pub terminal_submission_pending: usize,
    pub terminal_submission_max_pending: usize,
    pub terminal_submission_in_flight: usize,
    pub terminal_submission_max_in_flight: usize,
    pub terminal_submission_rejected_total: u64,
    pub terminal_enqueue_in_flight: u64,
    pub terminal_enqueue_deferred_total: u64,
    pub terminal_enqueue_deferred_direct_write_total: u64,
    pub terminal_enqueue_deferred_dropped_total: u64,
    pub terminal_enqueue_deferred_retry_total: u64,
    pub terminal_enqueue_failed_total: u64,
    pub terminal_direct_fallback_limit: usize,
    pub terminal_direct_fallback_in_flight: usize,
    pub terminal_direct_fallback_max_in_flight: usize,
    pub terminal_direct_fallback_succeeded_total: u64,
    pub terminal_direct_fallback_failed_total: u64,
    pub terminal_direct_fallback_rejected_total: u64,
    pub lifecycle_enqueue_in_flight: u64,
    pub lifecycle_enqueue_deferred_total: u64,
    pub lifecycle_enqueue_deferred_dropped_total: u64,
    pub lifecycle_enqueue_deferred_retry_total: u64,
    pub lifecycle_enqueue_failed_total: u64,
    pub lifecycle_submission_capacity: usize,
    pub lifecycle_submission_workers: usize,
    pub lifecycle_submission_pending: usize,
    pub lifecycle_submission_max_pending: usize,
    pub lifecycle_submission_enqueued_total: u64,
    pub lifecycle_submission_coalesced_total: u64,
    pub lifecycle_submission_overflow_total: u64,
    pub lifecycle_submission_processed_total: u64,
    pub ordered_lifecycle_pending: usize,
    pub ordered_lifecycle_max_pending: usize,
    pub pending_persistence_capacity: usize,
    pub pending_persistence_pending: usize,
    pub pending_persistence_max_pending: usize,
    pub pending_persistence_batch_flush_total: u64,
    pub pending_persistence_batch_records_total: u64,
    pub pending_persistence_max_batch_size: usize,
    pub pending_persistence_batch_failed_total: u64,
    pub pending_persistence_retried_total: u64,
    pub pending_persistence_overflow_total: u64,
    pub lifecycle_coalescer_entries: usize,
    pub lifecycle_coalescer_compact_total: u64,
    pub lifecycle_coalescer_compact_entries_scanned_total: u64,
    pub first_byte_persistence_capacity: usize,
    pub first_byte_persistence_pending: usize,
    pub first_byte_persistence_max_pending: usize,
    pub first_byte_persistence_dispatched_total: u64,
    pub first_byte_persistence_overflow_total: u64,
    pub first_byte_persistence_cancelled_total: u64,
    pub first_byte_persistence_direct_succeeded_total: u64,
    pub first_byte_persistence_direct_failed_total: u64,
    pub first_byte_persistence_batch_flush_total: u64,
    pub first_byte_persistence_batch_records_total: u64,
    pub first_byte_persistence_max_batch_size: usize,
    pub first_byte_persistence_batch_failed_total: u64,
    pub first_byte_persistence_fallback_accepted_total: u64,
    pub first_byte_persistence_fallback_failed_total: u64,
    pub enqueue_retry_scheduled_total: u64,
    pub enqueue_retry_recovered_total: u64,
    pub enqueue_retry_pending: u64,
    pub enqueue_retry_failed_total: u64,
    pub enqueue_retry_closed_or_unavailable_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageQueueHealthSnapshot {
    pub enabled: bool,
    pub configured: bool,
    pub stream_key: String,
    pub consumer_group: String,
    pub dlq_stream_key: String,
    pub stream_length: u64,
    pub group_pending: u64,
    pub group_lag: Option<u64>,
    pub oldest_pending_idle_ms: Option<u64>,
    pub dlq_length: u64,
}

const USAGE_BODY_CAPTURE_POLICY_CACHE_TTL: Duration = Duration::from_secs(30);
const USAGE_BODY_CAPTURE_POLICY_ERROR_CACHE_TTL: Duration = Duration::from_secs(1);
const LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS: u64 = 1_000;
const LIFECYCLE_COALESCER_CLOSE_TTL: Duration = Duration::from_secs(30);
const LIFECYCLE_COALESCER_COMPACT_INTERVAL: Duration = Duration::from_secs(1);
const TERMINAL_SUBMISSION_MAX_LIMIT: usize = 1_048_576;
const TERMINAL_DIRECT_FALLBACK_DEFAULT_MAX_IN_FLIGHT: usize = 32;
const LIFECYCLE_SUBMISSION_MAX_BUFFER: usize = 1_048_576;
const LIFECYCLE_SUBMISSION_MAX_WORKERS: usize = 32;
const PENDING_PERSISTENCE_MAX_BUFFER: usize = 65_536;
const PENDING_PERSISTENCE_BATCH_SIZE: usize = 512;
const PENDING_PERSISTENCE_BATCH_FLUSH_INTERVAL: Duration = Duration::from_millis(1);
const PENDING_PERSISTENCE_MAX_BATCH_CONCURRENCY: usize = 4;
const PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY: usize = 32;
// A single-write retry future keeps its slot across backoff. Combined with the
// four outer batch tasks, this bounds non-native writer retries at 32 globally.
const PENDING_PERSISTENCE_SINGLE_WRITE_CONCURRENCY_PER_BATCH: usize =
    PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY / PENDING_PERSISTENCE_MAX_BATCH_CONCURRENCY;
const PENDING_PERSISTENCE_BATCH_RETRIES_BEFORE_ISOLATION: u64 = 2;
const PENDING_PERSISTENCE_SINGLE_RETRIES_BEFORE_DEGRADE: u64 = 3;
const ORDERED_INTERMEDIATE_RETRIES_BEFORE_DEGRADE: u64 = 3;
const FIRST_BYTE_PERSISTENCE_DEFAULT_CONCURRENCY: usize = 32;
const FIRST_BYTE_PERSISTENCE_MAX_BUFFER: usize = 32_768;
// Keep the first-byte path off the request task while amortizing the single
// transaction used by the PostgreSQL batch writer. The adapter still chunks
// the statement at 128 rows, so this increases rows per commit without
// exceeding PostgreSQL's bind/statement limits.
const FIRST_BYTE_PERSISTENCE_BATCH_SIZE: usize = 512;
const FIRST_BYTE_PERSISTENCE_BATCH_FLUSH_INTERVAL: Duration = Duration::from_millis(1);
const FIRST_BYTE_PERSISTENCE_MAX_BATCH_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalPersistenceOutcome {
    Queued,
    PersistedDirectly,
    BufferedForRetry,
    Failed,
}

const LIFECYCLE_COALESCER_SHARD_COUNT: usize = 64;

#[derive(Debug)]
struct LifecycleEventCoalescerShard {
    entries: tokio::sync::Mutex<HashMap<String, LifecycleEventCoalescerEntry>>,
    next_compaction_at: StdMutex<Option<Instant>>,
}

impl Default for LifecycleEventCoalescerShard {
    fn default() -> Self {
        Self {
            entries: tokio::sync::Mutex::new(HashMap::new()),
            next_compaction_at: StdMutex::new(None),
        }
    }
}

#[derive(Debug)]
struct LifecycleEventCoalescer {
    shards: Vec<LifecycleEventCoalescerShard>,
    admission: Arc<tokio::sync::Semaphore>,
    generation_counter: AtomicU64,
    entry_count: AtomicUsize,
    rejected_total: AtomicU64,
    compact_total: AtomicU64,
    compact_entries_scanned_total: AtomicU64,
}

impl Default for LifecycleEventCoalescer {
    fn default() -> Self {
        Self::new(UsageRuntimeConfig::default().enqueue_retry_buffer_capacity)
    }
}

impl LifecycleEventCoalescer {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.clamp(1, LIFECYCLE_SUBMISSION_MAX_BUFFER);
        Self {
            shards: (0..LIFECYCLE_COALESCER_SHARD_COUNT)
                .map(|_| LifecycleEventCoalescerShard::default())
                .collect(),
            admission: Arc::new(tokio::sync::Semaphore::new(capacity)),
            generation_counter: AtomicU64::new(0),
            entry_count: AtomicUsize::new(0),
            rejected_total: AtomicU64::new(0),
            compact_total: AtomicU64::new(0),
            compact_entries_scanned_total: AtomicU64::new(0),
        }
    }
}

#[derive(Debug)]
struct LifecycleEventCoalescerEntry {
    generation: u64,
    terminal_seen_at: Option<Instant>,
    terminal_cancels_first_byte: bool,
    first_byte_seen_at: Option<Instant>,
    first_byte_persistence_pending: bool,
    _admission: tokio::sync::OwnedSemaphorePermit,
}

impl LifecycleEventCoalescer {
    fn try_new_entry(&self) -> Option<LifecycleEventCoalescerEntry> {
        let admission = Arc::clone(&self.admission).try_acquire_owned().ok()?;
        Some(LifecycleEventCoalescerEntry {
            generation: 0,
            terminal_seen_at: None,
            terminal_cancels_first_byte: false,
            first_byte_seen_at: None,
            first_byte_persistence_pending: false,
            _admission: admission,
        })
    }

    fn record_rejected(&self) {
        self.rejected_total.fetch_add(1, Ordering::AcqRel);
    }

    async fn register(&self, request_id: String) -> Option<u64> {
        loop {
            let now = Instant::now();
            let shard_index = self.shard_index(&request_id);
            let shard = &self.shards[shard_index];
            let mut entries = shard.entries.lock().await;
            self.compact_if_due(shard_index, &mut entries, now);
            let before = entries.len();
            if !entries.contains_key(&request_id) {
                let Some(entry) = self.try_new_entry() else {
                    drop(entries);
                    if self.evict_completed_first_byte_entry(&request_id).await {
                        continue;
                    }
                    self.record_rejected();
                    return None;
                };
                entries.insert(request_id.clone(), entry);
            }
            let entry = entries
                .get_mut(&request_id)
                .expect("coalescer entry should exist after admission");
            if lifecycle_event_is_closed(entry, now) {
                self.adjust_entry_count(before, entries.len());
                return None;
            }
            entry.generation = self.next_generation();
            entry.terminal_seen_at = None;
            entry.terminal_cancels_first_byte = false;
            entry.first_byte_seen_at = None;
            entry.first_byte_persistence_pending = false;
            let generation = entry.generation;
            self.adjust_entry_count(before, entries.len());
            return Some(generation);
        }
    }

    #[cfg(test)]
    async fn cancel(&self, request_id: &str) {
        loop {
            if !self.ensure_terminal_entry(request_id).await {
                return;
            }
            let now = Instant::now();
            let shard = &self.shards[self.shard_index(request_id)];
            let mut entries = shard.entries.lock().await;
            let Some(entry) = entries.get_mut(request_id) else {
                continue;
            };
            entry.generation = self.next_generation();
            entry.terminal_seen_at = Some(now);
            entry.terminal_cancels_first_byte = true;
            entry.first_byte_persistence_pending = false;
            return;
        }
    }

    async fn cancel_delayed_for_queued_terminal(&self, request_id: &str) {
        loop {
            if !self.ensure_terminal_entry(request_id).await {
                return;
            }
            let now = Instant::now();
            let shard = &self.shards[self.shard_index(request_id)];
            let mut entries = shard.entries.lock().await;
            let Some(entry) = entries.get_mut(request_id) else {
                continue;
            };
            if hard_terminal_cancel_is_active(entry, now) {
                return;
            }
            if !entry.first_byte_persistence_pending {
                entry.generation = self.next_generation();
            }
            entry.terminal_seen_at = Some(now);
            entry.terminal_cancels_first_byte = false;
            return;
        }
    }

    async fn mark_first_byte(&self, request_id: &str) -> Option<u64> {
        loop {
            let now = Instant::now();
            let shard_index = self.shard_index(request_id);
            let shard = &self.shards[shard_index];
            let mut entries = shard.entries.lock().await;
            self.compact_if_due(shard_index, &mut entries, now);
            let before = entries.len();
            if !entries.contains_key(request_id) {
                let Some(entry) = self.try_new_entry() else {
                    drop(entries);
                    if self.evict_completed_first_byte_entry(request_id).await {
                        continue;
                    }
                    self.record_rejected();
                    return None;
                };
                entries.insert(request_id.to_string(), entry);
            }
            let entry = entries
                .get_mut(request_id)
                .expect("coalescer first-byte entry should exist after admission");
            if hard_terminal_cancel_is_active(entry, now) || first_byte_marker_is_active(entry, now)
            {
                self.adjust_entry_count(before, entries.len());
                return None;
            }
            entry.generation = self.next_generation();
            entry.first_byte_seen_at = Some(now);
            entry.first_byte_persistence_pending = true;
            let generation = entry.generation;
            self.adjust_entry_count(before, entries.len());
            return Some(generation);
        }
    }

    async fn complete_first_byte(&self, request_id: &str, generation: u64) {
        let shard = &self.shards[self.shard_index(request_id)];
        let mut entries = shard.entries.lock().await;
        if let Some(entry) = entries.get_mut(request_id).filter(|entry| {
            entry.generation == generation
                && entry.first_byte_persistence_pending
                && !entry.terminal_cancels_first_byte
        }) {
            entry.first_byte_seen_at = Some(Instant::now());
            entry.first_byte_persistence_pending = false;
        }
    }

    async fn rollback_first_byte(&self, request_id: &str, generation: u64) {
        let now = Instant::now();
        let shard = &self.shards[self.shard_index(request_id)];
        let mut entries = shard.entries.lock().await;
        let before = entries.len();
        if let Some(entry) = entries
            .get_mut(request_id)
            .filter(|entry| entry.generation == generation && entry.first_byte_persistence_pending)
        {
            if terminal_cancel_is_active(entry, now) {
                entry.generation = self.next_generation();
                entry.first_byte_seen_at = None;
                entry.first_byte_persistence_pending = false;
            } else {
                entries.remove(request_id);
            }
        }
        self.adjust_entry_count(before, entries.len());
    }

    async fn first_byte_is_current(&self, request_id: &str, generation: u64) -> bool {
        let now = Instant::now();
        self.shards[self.shard_index(request_id)]
            .entries
            .lock()
            .await
            .get(request_id)
            .is_some_and(|entry| {
                entry.generation == generation
                    && entry.first_byte_persistence_pending
                    && !hard_terminal_cancel_is_active(entry, now)
            })
    }

    async fn abandon(&self, request_id: &str, generation: u64) {
        let shard = &self.shards[self.shard_index(request_id)];
        let mut entries = shard.entries.lock().await;
        let before = entries.len();
        if entries
            .get(request_id)
            .is_some_and(|entry| entry.generation == generation)
        {
            entries.remove(request_id);
        }
        self.adjust_entry_count(before, entries.len());
    }

    async fn should_emit(&self, request_id: &str, generation: u64) -> bool {
        let now = Instant::now();
        let shard_index = self.shard_index(request_id);
        let shard = &self.shards[shard_index];
        let mut entries = shard.entries.lock().await;
        let before = entries.len();
        let Some(entry) = entries.get(request_id) else {
            return false;
        };
        let closed = lifecycle_event_is_closed(entry, now);
        let should_emit = entry.generation == generation && !closed;
        if should_emit {
            entries.remove(request_id);
        }
        self.adjust_entry_count(before, entries.len());
        self.compact_if_due(shard_index, &mut entries, now);
        should_emit
    }

    async fn ensure_terminal_entry(&self, request_id: &str) -> bool {
        loop {
            let shard_index = self.shard_index(request_id);
            let shard = &self.shards[shard_index];
            let mut entries = shard.entries.lock().await;
            self.compact_if_due(shard_index, &mut entries, Instant::now());
            if entries.contains_key(request_id) {
                return true;
            }
            if let Some(entry) = self.try_new_entry() {
                let before = entries.len();
                entries.insert(request_id.to_string(), entry);
                self.adjust_entry_count(before, entries.len());
                return true;
            }
            drop(entries);

            if !self.evict_for_terminal_entry(request_id).await {
                self.record_rejected();
                return false;
            }
        }
    }

    async fn evict_for_terminal_entry(&self, excluded_request_id: &str) -> bool {
        let now = Instant::now();
        let start = self.shard_index(excluded_request_id);
        for offset in 0..self.shards.len() {
            let shard = &self.shards[(start + offset) % self.shards.len()];
            let mut entries = shard.entries.lock().await;
            let candidate = entries
                .iter()
                .find(|(request_id, entry)| {
                    // A terminal supersedes a delayed intermediate, but never another active
                    // terminal marker or an in-flight first-byte persistence operation.
                    request_id.as_str() != excluded_request_id
                        && !entry.first_byte_persistence_pending
                        && !terminal_cancel_is_active(entry, now)
                })
                .map(|(request_id, _)| request_id.clone());
            if let Some(request_id) = candidate {
                let before = entries.len();
                entries.remove(&request_id);
                self.adjust_entry_count(before, entries.len());
                return true;
            }
        }
        false
    }

    async fn evict_completed_first_byte_entry(&self, excluded_request_id: &str) -> bool {
        let now = Instant::now();
        let start = self.shard_index(excluded_request_id);
        for offset in 0..self.shards.len() {
            let shard = &self.shards[(start + offset) % self.shards.len()];
            let mut entries = shard.entries.lock().await;
            let candidate = entries
                .iter()
                .find(|(request_id, entry)| {
                    request_id.as_str() != excluded_request_id
                        && !entry.first_byte_persistence_pending
                        && entry.first_byte_seen_at.is_some()
                        && !terminal_cancel_is_active(entry, now)
                })
                .map(|(request_id, _)| request_id.clone());
            if let Some(request_id) = candidate {
                let before = entries.len();
                entries.remove(&request_id);
                self.adjust_entry_count(before, entries.len());
                return true;
            }
        }
        false
    }

    fn next_generation(&self) -> u64 {
        self.generation_counter
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    fn shard_index(&self, request_id: &str) -> usize {
        retry_worker_index(request_id, self.shards.len())
    }

    fn adjust_entry_count(&self, before: usize, after: usize) {
        match after.cmp(&before) {
            std::cmp::Ordering::Greater => {
                self.entry_count
                    .fetch_add(after.saturating_sub(before), Ordering::AcqRel);
            }
            std::cmp::Ordering::Less => {
                self.entry_count
                    .fetch_sub(before.saturating_sub(after), Ordering::AcqRel);
            }
            std::cmp::Ordering::Equal => {}
        }
    }

    fn compact_if_due(
        &self,
        shard_index: usize,
        entries: &mut HashMap<String, LifecycleEventCoalescerEntry>,
        now: Instant,
    ) {
        let shard = &self.shards[shard_index];
        let Ok(mut next_compaction_at) = shard.next_compaction_at.lock() else {
            return;
        };
        if next_compaction_at.is_some_and(|deadline| now < deadline) {
            return;
        }
        *next_compaction_at = Some(now + LIFECYCLE_COALESCER_COMPACT_INTERVAL);
        if entries.is_empty() {
            return;
        }
        let before = entries.len();
        self.compact_total.fetch_add(1, Ordering::AcqRel);
        self.compact_entries_scanned_total.fetch_add(
            u64::try_from(entries.len()).unwrap_or(u64::MAX),
            Ordering::AcqRel,
        );
        Self::compact_locked(entries, now);
        self.adjust_entry_count(before, entries.len());
    }

    fn compact_locked(entries: &mut HashMap<String, LifecycleEventCoalescerEntry>, now: Instant) {
        entries.retain(|_, entry| {
            let has_close_marker =
                entry.terminal_seen_at.is_some() || entry.first_byte_seen_at.is_some();
            !has_close_marker || lifecycle_event_is_closed(entry, now)
        });
    }
}

async fn run_lifecycle_coalescer_compactor(coalescer: Weak<LifecycleEventCoalescer>) {
    let mut interval = tokio::time::interval(LIFECYCLE_COALESCER_COMPACT_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        interval.tick().await;
        let Some(coalescer) = coalescer.upgrade() else {
            break;
        };
        let now = Instant::now();
        for shard_index in 0..coalescer.shards.len() {
            let shard = &coalescer.shards[shard_index];
            let mut entries = shard.entries.lock().await;
            coalescer.compact_if_due(shard_index, &mut entries, now);
        }
    }
}

fn terminal_cancel_is_active(entry: &LifecycleEventCoalescerEntry, now: Instant) -> bool {
    entry
        .terminal_seen_at
        .is_some_and(|seen_at| now.duration_since(seen_at) <= LIFECYCLE_COALESCER_CLOSE_TTL)
}

fn hard_terminal_cancel_is_active(entry: &LifecycleEventCoalescerEntry, now: Instant) -> bool {
    entry.terminal_cancels_first_byte && terminal_cancel_is_active(entry, now)
}

fn first_byte_marker_is_active(entry: &LifecycleEventCoalescerEntry, now: Instant) -> bool {
    entry.first_byte_persistence_pending
        || entry
            .first_byte_seen_at
            .is_some_and(|seen_at| now.duration_since(seen_at) <= LIFECYCLE_COALESCER_CLOSE_TTL)
}

fn lifecycle_event_is_closed(entry: &LifecycleEventCoalescerEntry, now: Instant) -> bool {
    terminal_cancel_is_active(entry, now) || first_byte_marker_is_active(entry, now)
}

fn make_first_byte_event_lightweight(mut event: UsageEvent) -> UsageEvent {
    let data = &mut event.data;
    data.request_headers = None;
    data.request_body = None;
    data.provider_request_headers = None;
    data.provider_request_body = None;
    data.response_headers = None;
    data.response_body = None;
    data.client_response_headers = None;
    data.client_response_body = None;
    data.request_metadata = retain_first_byte_request_metadata(data.request_metadata.take());
    event
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LifecycleSubmissionPriority {
    Pending,
    Streaming,
    FirstByte,
    Terminal,
}

type LifecycleAdmissionPermit = Arc<tokio::sync::OwnedSemaphorePermit>;

#[derive(Debug)]
struct LifecycleSubmissionState {
    capacity: usize,
    workers: usize,
    admission: Arc<tokio::sync::Semaphore>,
    pending: AtomicUsize,
    max_pending: AtomicUsize,
    enqueued_total: AtomicU64,
    coalesced_total: AtomicU64,
    overflow_total: AtomicU64,
    processed_total: AtomicU64,
}

impl LifecycleSubmissionState {
    fn new(capacity: usize, workers: usize) -> Self {
        Self {
            capacity,
            workers,
            admission: Arc::new(tokio::sync::Semaphore::new(capacity)),
            ..Self::default()
        }
    }

    fn try_admit(&self) -> Option<LifecycleAdmissionPermit> {
        Arc::clone(&self.admission)
            .try_acquire_owned()
            .ok()
            .map(Arc::new)
    }

    async fn admit(&self) -> Option<LifecycleAdmissionPermit> {
        Arc::clone(&self.admission)
            .acquire_owned()
            .await
            .ok()
            .map(Arc::new)
    }

    fn record_enqueued(&self) -> usize {
        self.enqueued_total.fetch_add(1, Ordering::AcqRel);
        let pending = self.pending.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_pending.fetch_max(pending, Ordering::AcqRel);
        pending
    }

    fn record_processed(&self) {
        self.pending.fetch_sub(1, Ordering::AcqRel);
        self.processed_total.fetch_add(1, Ordering::AcqRel);
    }

    fn record_coalesced(&self) {
        self.coalesced_total.fetch_add(1, Ordering::AcqRel);
    }
}

impl Default for LifecycleSubmissionState {
    fn default() -> Self {
        Self {
            capacity: 0,
            workers: 0,
            admission: Arc::new(tokio::sync::Semaphore::new(0)),
            pending: AtomicUsize::new(0),
            max_pending: AtomicUsize::new(0),
            enqueued_total: AtomicU64::new(0),
            coalesced_total: AtomicU64::new(0),
            overflow_total: AtomicU64::new(0),
            processed_total: AtomicU64::new(0),
        }
    }
}

struct LifecycleSubmissionEnvelope {
    item: Box<dyn LifecycleSubmissionItem>,
    admission: LifecycleAdmissionPermit,
}

#[async_trait]
trait LifecycleSubmissionItem: Send {
    fn request_id(&self) -> &str;
    fn priority(&self) -> LifecycleSubmissionPriority;
    async fn execute(self: Box<Self>, admission: LifecycleAdmissionPermit);
}

struct LifecycleSubmissionBarrierItem {
    request_id: String,
    ordered_lifecycle: Arc<OrderedLifecycleDispatcher>,
    completion: tokio::sync::oneshot::Sender<OrderedLifecycleCompletion>,
}

#[async_trait]
impl LifecycleSubmissionItem for LifecycleSubmissionBarrierItem {
    fn request_id(&self) -> &str {
        &self.request_id
    }

    fn priority(&self) -> LifecycleSubmissionPriority {
        LifecycleSubmissionPriority::Terminal
    }

    async fn execute(self: Box<Self>, admission: LifecycleAdmissionPermit) {
        self.ordered_lifecycle.dispatch(
            Box::new(OrderedLifecycleBarrierItem {
                request_id: self.request_id,
                completion: self.completion,
            }),
            admission,
        );
    }
}

enum LifecycleSubmissionPayload {
    Pending {
        seed: LifecycleUsageSeed,
        observed_at_unix_secs: u64,
    },
    Streaming {
        seed: LifecycleUsageSeed,
        status_code: u16,
        telemetry: Option<ExecutionTelemetry>,
        observed_at_unix_secs: u64,
    },
    Active {
        seed: LifecycleUsageSeed,
        observed_at_unix_secs: u64,
    },
    TerminalSeed {
        seed: LifecycleTerminalUsageSeed,
        observed_at_unix_ms: u64,
    },
    Terminal {
        event: UsageEvent,
        direct: bool,
        completion: Option<tokio::sync::oneshot::Sender<()>>,
    },
}

enum LifecycleTerminalUsageSeed {
    Sync {
        context: TerminalUsageContextSeed,
        payload: SyncTerminalUsagePayloadSeed,
    },
    Stream {
        context: TerminalUsageContextSeed,
        payload: StreamTerminalUsagePayloadSeed,
        cancelled: bool,
    },
    #[cfg(test)]
    BlockedBuild {
        event: UsageEvent,
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    },
}

impl LifecycleTerminalUsageSeed {
    async fn build(self, request_id: &str) -> Result<UsageEvent, DataLayerError> {
        match self {
            Self::Sync { context, payload } => {
                let result = build_sync_terminal_usage_event_offthread(context, payload).await;
                if let Err(err) = &result {
                    warn!(
                        event_name = "usage_sync_terminal_build_failed",
                        log_type = "event",
                        request_id,
                        error = %err,
                        "usage runtime failed to build sync terminal usage event"
                    );
                }
                result
            }
            Self::Stream {
                context,
                payload,
                cancelled,
            } => {
                let result =
                    build_stream_terminal_usage_event_offthread(context, payload, cancelled).await;
                if let Err(err) = &result {
                    warn!(
                        event_name = "usage_stream_terminal_build_failed",
                        log_type = "event",
                        request_id,
                        error = %err,
                        "usage runtime failed to build stream terminal usage event"
                    );
                }
                result
            }
            #[cfg(test)]
            Self::BlockedBuild {
                event,
                started,
                release,
            } => {
                started.notify_one();
                release.notified().await;
                Ok(event)
            }
        }
    }
}

enum TerminalExecutionPayload {
    Seed {
        seed: LifecycleTerminalUsageSeed,
        observed_at_unix_ms: u64,
    },
    Event {
        event: UsageEvent,
        direct: bool,
        completion: Option<tokio::sync::oneshot::Sender<()>>,
    },
}

#[async_trait]
trait TerminalExecutionItem: Send {
    fn request_id(&self) -> &str;
    async fn execute(self: Box<Self>);
}

struct TerminalExecutionBarrierItem {
    request_id: String,
    completion: tokio::sync::oneshot::Sender<OrderedLifecycleCompletion>,
    ordered_completion: OrderedLifecycleCompletion,
}

#[async_trait]
impl TerminalExecutionItem for TerminalExecutionBarrierItem {
    fn request_id(&self) -> &str {
        &self.request_id
    }

    async fn execute(self: Box<Self>) {
        let _ = self.completion.send(self.ordered_completion);
    }
}

struct TerminalExecutionItemImpl<T> {
    runtime: UsageRuntime,
    data: T,
    request_id: String,
    payload: TerminalExecutionPayload,
    pending_guard: TerminalSubmissionPendingGuard,
    ordered_completion: OrderedLifecycleCompletion,
}

#[async_trait]
impl<T> TerminalExecutionItem for TerminalExecutionItemImpl<T>
where
    T: UsageRuntimeAccess + Clone + 'static,
{
    fn request_id(&self) -> &str {
        &self.request_id
    }

    async fn execute(self: Box<Self>) {
        let Self {
            runtime,
            data,
            request_id,
            payload,
            pending_guard,
            ordered_completion,
        } = *self;
        let Some(submission_permit) = runtime
            .begin_registered_terminal_submission(&request_id, pending_guard)
            .await
        else {
            return;
        };

        let (event, direct, completion) = match payload {
            TerminalExecutionPayload::Seed {
                seed,
                observed_at_unix_ms,
            } => {
                let Ok(mut event) = seed.build(&request_id).await else {
                    return;
                };
                event.timestamp_ms = observed_at_unix_ms;
                (event, false, None)
            }
            TerminalExecutionPayload::Event {
                event,
                direct,
                completion,
            } => (event, direct, completion),
        };

        let mut event = event;
        runtime
            .apply_body_capture_policy_from_data(&data, &mut event)
            .await;
        let persistence_outcome = runtime
            .persist_ordered_terminal_event(&data, event, direct)
            .await;
        drop(submission_permit);
        if persistence_outcome == TerminalPersistenceOutcome::Failed {
            warn!(
                event_name = "usage_ordered_terminal_persistence_failed",
                log_type = "ops",
                request_id,
                fallback = "advance_after_failed_terminal",
                "ordered terminal persistence failed; releasing its admission so later phases can advance"
            );
            return;
        }
        ordered_completion.complete();
        if let Some(completion) = completion {
            let _ = completion.send(());
        }
    }
}

struct TerminalExecutionDispatcher {
    enabled: bool,
}

impl std::fmt::Debug for TerminalExecutionDispatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalExecutionDispatcher")
            .field("enabled", &self.enabled)
            .finish()
    }
}

impl TerminalExecutionDispatcher {
    fn disabled() -> Arc<Self> {
        Arc::new(Self { enabled: false })
    }

    fn spawn(config: &UsageRuntimeConfig) -> Arc<Self> {
        Arc::new(Self {
            enabled: config.enabled,
        })
    }

    /// Per-request order is already enforced by `OrderedLifecycleDispatcher`. Dispatch each
    /// admitted item independently so unrelated request IDs can use the configured terminal
    /// semaphore concurrently without hash-shard head-of-line blocking.
    fn dispatch_ordered_now(
        &self,
        item: Box<dyn TerminalExecutionItem>,
    ) -> Result<(), Box<dyn TerminalExecutionItem>> {
        if !self.enabled {
            return Err(item);
        }
        spawn_on_usage_background_runtime(async move {
            execute_terminal_item_isolated(item, "ordered_task").await;
        });
        Ok(())
    }
}

#[cfg(test)]
async fn run_terminal_execution_worker(
    mut receiver: mpsc::UnboundedReceiver<Box<dyn TerminalExecutionItem>>,
) {
    while let Some(item) = receiver.recv().await {
        execute_terminal_item_isolated(item, "shard_worker").await;
    }
}

async fn execute_terminal_item_isolated(
    item: Box<dyn TerminalExecutionItem>,
    execution_path: &'static str,
) {
    let request_id = item.request_id().to_string();
    if AssertUnwindSafe(item.execute())
        .catch_unwind()
        .await
        .is_err()
    {
        warn!(
            event_name = "usage_terminal_execution_panicked",
            log_type = "ops",
            request_id,
            execution_path,
            fallback = "advance_after_panicked_terminal",
            "usage terminal execution panicked; releasing its admission while the worker continues"
        );
    }
}

struct LifecycleSubmissionItemImpl<T> {
    runtime: UsageRuntime,
    data: T,
    request_id: String,
    payload: LifecycleSubmissionPayload,
}

#[async_trait]
impl<T> LifecycleSubmissionItem for LifecycleSubmissionItemImpl<T>
where
    T: UsageRuntimeAccess + Clone + 'static,
{
    fn request_id(&self) -> &str {
        &self.request_id
    }

    fn priority(&self) -> LifecycleSubmissionPriority {
        match &self.payload {
            LifecycleSubmissionPayload::Pending { .. } => LifecycleSubmissionPriority::Pending,
            LifecycleSubmissionPayload::Streaming { telemetry, .. }
                if telemetry.as_ref().and_then(|value| value.ttfb_ms).is_some() =>
            {
                LifecycleSubmissionPriority::FirstByte
            }
            LifecycleSubmissionPayload::Streaming { .. } => LifecycleSubmissionPriority::Streaming,
            LifecycleSubmissionPayload::Active { .. } => LifecycleSubmissionPriority::Streaming,
            LifecycleSubmissionPayload::TerminalSeed { .. } => {
                LifecycleSubmissionPriority::Terminal
            }
            LifecycleSubmissionPayload::Terminal { .. } => LifecycleSubmissionPriority::Terminal,
        }
    }

    async fn execute(self: Box<Self>, admission: LifecycleAdmissionPermit) {
        let Self {
            runtime,
            data,
            request_id,
            payload,
        } = *self;
        let (phase, result) = match payload {
            LifecycleSubmissionPayload::Pending {
                seed,
                observed_at_unix_secs,
            } => (
                "pending",
                crate::write::build_pending_usage_event_from_owned_seed(
                    seed,
                    observed_at_unix_secs,
                ),
            ),
            LifecycleSubmissionPayload::Streaming {
                seed,
                status_code,
                telemetry,
                observed_at_unix_secs,
            } => (
                "streaming",
                crate::write::build_streaming_usage_event_from_owned_seed(
                    seed,
                    status_code,
                    telemetry,
                    observed_at_unix_secs,
                ),
            ),
            LifecycleSubmissionPayload::Active {
                seed,
                observed_at_unix_secs,
            } => (
                "streaming",
                crate::write::build_active_usage_event_from_owned_seed(seed, observed_at_unix_secs),
            ),
            LifecycleSubmissionPayload::TerminalSeed {
                seed,
                observed_at_unix_ms,
            } => {
                let ordered_lifecycle = Arc::clone(&runtime.ordered_lifecycle);
                ordered_lifecycle.dispatch(
                    Box::new(OrderedLifecycleItemImpl {
                        runtime,
                        data,
                        request_id,
                        payload: OrderedLifecyclePayload::TerminalSeed {
                            seed,
                            observed_at_unix_ms,
                        },
                    }),
                    admission,
                );
                return;
            }
            LifecycleSubmissionPayload::Terminal {
                event,
                direct,
                completion,
            } => {
                let ordered_lifecycle = Arc::clone(&runtime.ordered_lifecycle);
                ordered_lifecycle.dispatch(
                    Box::new(OrderedLifecycleItemImpl {
                        runtime,
                        data,
                        request_id,
                        payload: OrderedLifecyclePayload::Terminal {
                            event,
                            direct,
                            completion,
                        },
                    }),
                    admission,
                );
                return;
            }
        };
        match result {
            Ok(mut event) => {
                runtime
                    .apply_body_capture_policy_from_data(&data, &mut event)
                    .await;
                let payload = if event.event_type == crate::UsageEventType::Pending {
                    OrderedLifecyclePayload::Pending { event }
                } else {
                    OrderedLifecyclePayload::Streaming { event }
                };
                let ordered_lifecycle = Arc::clone(&runtime.ordered_lifecycle);
                ordered_lifecycle.dispatch(
                    Box::new(OrderedLifecycleItemImpl {
                        runtime,
                        data,
                        request_id,
                        payload,
                    }),
                    admission,
                );
            }
            Err(err) => {
                warn!(
                    event_name = "usage_lifecycle_submission_build_failed",
                    log_type = "event",
                    request_id = %request_id,
                    lifecycle_phase = phase,
                    error = %err,
                    "usage runtime failed to build a lifecycle event"
                );
                // Preserve ordering through the dispatcher, then degrade this malformed phase so
                // it cannot retain admission or poison the request permanently.
                runtime.ordered_lifecycle.dispatch(
                    Box::new(OrderedLifecycleDegradedItem { request_id }),
                    admission,
                );
            }
        }
    }
}

struct LifecycleSubmissionShard {
    sender: mpsc::UnboundedSender<String>,
    slots: Arc<StdMutex<HashMap<String, LifecycleSubmissionSlot>>>,
    fallback_running: Arc<AtomicBool>,
}

#[derive(Default)]
struct LifecycleSubmissionSlot {
    pending: Option<LifecycleSubmissionEnvelope>,
    streaming: Option<(LifecycleSubmissionPriority, LifecycleSubmissionEnvelope)>,
    terminal: Vec<LifecycleSubmissionEnvelope>,
}

impl LifecycleSubmissionSlot {
    fn insert(&mut self, envelope: LifecycleSubmissionEnvelope) -> bool {
        let priority = envelope.item.priority();
        match priority {
            LifecycleSubmissionPriority::Pending => self.pending.replace(envelope).is_some(),
            LifecycleSubmissionPriority::Streaming | LifecycleSubmissionPriority::FirstByte => {
                if self
                    .streaming
                    .as_ref()
                    .is_some_and(|(current, _)| *current > priority)
                {
                    return true;
                }
                self.streaming.replace((priority, envelope)).is_some()
            }
            LifecycleSubmissionPriority::Terminal => {
                self.terminal.push(envelope);
                false
            }
        }
    }

    async fn execute(self) {
        if let Some(envelope) = self.pending {
            envelope.item.execute(envelope.admission).await;
        }
        if let Some((_, envelope)) = self.streaming {
            envelope.item.execute(envelope.admission).await;
        }
        for envelope in self.terminal {
            envelope.item.execute(envelope.admission).await;
        }
    }
}

struct LifecycleSubmissionDispatcher {
    shards: Vec<LifecycleSubmissionShard>,
    state: Arc<LifecycleSubmissionState>,
}

impl std::fmt::Debug for LifecycleSubmissionDispatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LifecycleSubmissionDispatcher")
            .field("capacity", &self.state.capacity)
            .field("workers", &self.state.workers)
            .field("pending", &self.state.pending.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl LifecycleSubmissionDispatcher {
    fn disabled() -> Arc<Self> {
        Arc::new(Self {
            shards: Vec::new(),
            state: Arc::new(LifecycleSubmissionState::default()),
        })
    }

    fn spawn(config: &UsageRuntimeConfig) -> Arc<Self> {
        if !config.enabled {
            return Self::disabled();
        }
        let capacity = config
            .enqueue_retry_buffer_capacity
            .clamp(1, LIFECYCLE_SUBMISSION_MAX_BUFFER);
        let workers = config
            .worker_record_concurrency_limit
            .unwrap_or(config.enqueue_retry_workers)
            .clamp(1, LIFECYCLE_SUBMISSION_MAX_WORKERS)
            .min(capacity);
        let state = Arc::new(LifecycleSubmissionState::new(capacity, workers));
        let mut shards = Vec::with_capacity(workers);
        for _ in 0..workers {
            let (sender, receiver) = mpsc::unbounded_channel();
            let slots = Arc::new(StdMutex::new(HashMap::new()));
            spawn_on_usage_background_runtime(run_lifecycle_submission_worker(
                receiver,
                Arc::clone(&slots),
                Arc::clone(&state),
            ));
            shards.push(LifecycleSubmissionShard {
                sender,
                slots,
                fallback_running: Arc::new(AtomicBool::new(false)),
            });
        }
        Arc::new(Self { shards, state })
    }

    fn dispatch(&self, item: Box<dyn LifecycleSubmissionItem>) -> bool {
        if self.shards.is_empty() {
            return false;
        }
        let Some(admission) = self.state.try_admit() else {
            let overflow = self.state.overflow_total.fetch_add(1, Ordering::AcqRel) + 1;
            if should_log_usage_retry_counter(overflow) {
                warn!(
                    event_name = "usage_lifecycle_submission_overflow",
                    log_type = "event",
                    request_id = item.request_id(),
                    dispatcher_capacity = self.state.capacity,
                    dispatcher_workers = self.state.workers,
                    overflow_total = overflow,
                    fallback = "drop_intermediate_only",
                    "usage lifecycle handoff reached its hard capacity"
                );
            }
            return false;
        };
        self.dispatch_admitted(item, admission);
        true
    }

    async fn dispatch_terminal(&self, item: Box<dyn LifecycleSubmissionItem>) -> bool {
        if self.shards.is_empty() {
            return false;
        }
        let Some(admission) = self.state.admit().await else {
            return false;
        };
        self.dispatch_admitted(item, admission);
        true
    }

    fn dispatch_admitted(
        &self,
        item: Box<dyn LifecycleSubmissionItem>,
        admission: LifecycleAdmissionPermit,
    ) {
        let request_id = item.request_id().to_string();
        let shard_index = retry_worker_index(&request_id, self.shards.len());
        let shard = &self.shards[shard_index];
        let envelope = LifecycleSubmissionEnvelope { item, admission };
        let mut slots = shard
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(slot) = slots.get_mut(&request_id) {
            if slot.insert(envelope) {
                self.state.record_coalesced();
            }
            return;
        }
        let mut slot = LifecycleSubmissionSlot::default();
        slot.insert(envelope);
        slots.insert(request_id.clone(), slot);
        drop(slots);

        self.state.record_enqueued();

        if shard.sender.send(request_id.clone()).is_err() {
            let overflow = self.state.overflow_total.fetch_add(1, Ordering::AcqRel) + 1;
            warn!(
                event_name = "usage_lifecycle_submission_worker_unavailable",
                log_type = "event",
                request_id,
                dispatcher_workers = self.state.workers,
                overflow_total = overflow,
                fallback = "single_ordered_shard_drainer",
                "usage lifecycle ordered worker is unavailable; draining preserved events serially"
            );
            schedule_lifecycle_submission_fallback(
                Arc::clone(&shard.slots),
                Arc::clone(&shard.fallback_running),
                Arc::clone(&self.state),
            );
        }
    }
}

async fn run_lifecycle_submission_worker(
    mut receiver: mpsc::UnboundedReceiver<String>,
    slots: Arc<StdMutex<HashMap<String, LifecycleSubmissionSlot>>>,
    state: Arc<LifecycleSubmissionState>,
) {
    while let Some(request_id) = receiver.recv().await {
        let slot = slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&request_id);
        if let Some(slot) = slot {
            let result = spawn_on_usage_background_runtime(slot.execute()).await;
            if let Err(err) = result {
                warn!(
                    event_name = "usage_lifecycle_submission_item_panicked",
                    log_type = "ops",
                    request_id,
                    error = %err,
                    fallback = "drop_current_slot",
                    "usage lifecycle submission item panicked; dropping the current slot"
                );
                state.record_processed();
                continue;
            }
        }
        state.record_processed();
    }
}

fn schedule_lifecycle_submission_fallback(
    slots: Arc<StdMutex<HashMap<String, LifecycleSubmissionSlot>>>,
    running: Arc<AtomicBool>,
    state: Arc<LifecycleSubmissionState>,
) {
    if running.swap(true, Ordering::AcqRel) {
        return;
    }
    spawn_on_usage_background_runtime(async move {
        loop {
            let next = {
                let mut slots = slots
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let request_id = slots.keys().next().cloned();
                request_id
                    .and_then(|request_id| slots.remove(&request_id).map(|slot| (request_id, slot)))
            };
            let Some((request_id, slot)) = next else {
                running.store(false, Ordering::Release);
                let has_more = !slots
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .is_empty();
                if has_more
                    && running
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    continue;
                }
                return;
            };
            match spawn_on_usage_background_runtime(slot.execute()).await {
                Ok(()) => state.record_processed(),
                Err(err) => {
                    warn!(
                        event_name = "usage_lifecycle_submission_fallback_item_panicked",
                        log_type = "ops",
                        request_id,
                        error = %err,
                        fallback = "drop_current_slot",
                        "usage lifecycle fallback item panicked; dropping the current slot"
                    );
                    state.record_processed();
                }
            }
        }
    });
}

enum OrderedLifecyclePayload {
    Pending {
        event: UsageEvent,
    },
    Streaming {
        event: UsageEvent,
    },
    TerminalSeed {
        seed: LifecycleTerminalUsageSeed,
        observed_at_unix_ms: u64,
    },
    Terminal {
        event: UsageEvent,
        direct: bool,
        completion: Option<tokio::sync::oneshot::Sender<()>>,
    },
}

trait OrderedLifecycleItem: Send {
    fn request_id(&self) -> &str;
    fn start(self: Box<Self>, completion: OrderedLifecycleCompletion);
}

struct OrderedLifecycleItemImpl<T> {
    runtime: UsageRuntime,
    data: T,
    request_id: String,
    payload: OrderedLifecyclePayload,
}

impl<T> OrderedLifecycleItem for OrderedLifecycleItemImpl<T>
where
    T: UsageRuntimeAccess + Clone + 'static,
{
    fn request_id(&self) -> &str {
        &self.request_id
    }

    fn start(self: Box<Self>, ordered_completion: OrderedLifecycleCompletion) {
        let Self {
            runtime,
            data,
            request_id,
            payload,
        } = *self;
        match payload {
            OrderedLifecyclePayload::Pending { event } => {
                if data.has_usage_writer() {
                    let pending_persistence = Arc::clone(&runtime.pending_persistence);
                    pending_persistence.dispatch(Box::new(PendingPersistenceItemImpl {
                        runtime,
                        data,
                        event,
                        request_id,
                        ordered_completion,
                    }));
                } else {
                    spawn_on_usage_background_runtime(async move {
                        runtime.enqueue_or_write_lifecycle(&data, event).await;
                        ordered_completion.complete();
                    });
                }
            }
            OrderedLifecyclePayload::Streaming { event }
                if event.data.first_byte_time_ms.is_some() =>
            {
                spawn_on_usage_background_runtime(async move {
                    runtime
                        .enqueue_first_byte_lifecycle_event_inner(
                            &data,
                            event,
                            false,
                            Some(ordered_completion),
                        )
                        .await;
                });
            }
            OrderedLifecyclePayload::Streaming { event } => {
                spawn_on_usage_background_runtime(async move {
                    if data.has_usage_writer() {
                        let _ = runtime
                            .persist_ordered_lifecycle_event(&data, &event, "streaming")
                            .await;
                    } else {
                        runtime.enqueue_or_write_lifecycle(&data, event).await;
                    }
                    ordered_completion.complete();
                });
            }
            OrderedLifecyclePayload::TerminalSeed {
                seed,
                observed_at_unix_ms,
            } => {
                let pending_guard = runtime.terminal_submission_state.register_pending();
                let terminal_execution = Arc::clone(&runtime.terminal_execution);
                let item = Box::new(TerminalExecutionItemImpl {
                    runtime,
                    data,
                    request_id,
                    payload: TerminalExecutionPayload::Seed {
                        seed,
                        observed_at_unix_ms,
                    },
                    pending_guard,
                    ordered_completion,
                });
                match terminal_execution.dispatch_ordered_now(item) {
                    Ok(()) => {}
                    Err(item) => {
                        spawn_on_usage_background_runtime(async move {
                            execute_terminal_item_isolated(item, "ordered_direct_fallback").await;
                        });
                    }
                }
            }
            OrderedLifecyclePayload::Terminal {
                event,
                direct,
                completion,
            } => {
                let pending_guard = runtime.terminal_submission_state.register_pending();
                let terminal_execution = Arc::clone(&runtime.terminal_execution);
                let item = Box::new(TerminalExecutionItemImpl {
                    runtime,
                    data,
                    request_id,
                    payload: TerminalExecutionPayload::Event {
                        event,
                        direct,
                        completion,
                    },
                    pending_guard,
                    ordered_completion,
                });
                match terminal_execution.dispatch_ordered_now(item) {
                    Ok(()) => {}
                    Err(item) => {
                        spawn_on_usage_background_runtime(async move {
                            execute_terminal_item_isolated(item, "ordered_direct_fallback").await;
                        });
                    }
                }
            }
        }
    }
}

struct OrderedLifecycleBarrierItem {
    request_id: String,
    completion: tokio::sync::oneshot::Sender<OrderedLifecycleCompletion>,
}

impl OrderedLifecycleItem for OrderedLifecycleBarrierItem {
    fn request_id(&self) -> &str {
        &self.request_id
    }

    fn start(self: Box<Self>, ordered_completion: OrderedLifecycleCompletion) {
        let Self {
            request_id,
            completion,
        } = *self;
        let dispatcher = ordered_completion.terminal_execution();
        let item = Box::new(TerminalExecutionBarrierItem {
            request_id,
            completion,
            ordered_completion,
        });
        match dispatcher.dispatch_ordered_now(item) {
            Ok(()) => {}
            Err(item) => {
                spawn_on_usage_background_runtime(async move {
                    execute_terminal_item_isolated(item, "barrier_direct_fallback").await;
                });
            }
        }
    }
}

/// A deterministic build failure still enters the ordered dispatcher, then releases its
/// admission through the completion guard without persisting a malformed phase.
struct OrderedLifecycleDegradedItem {
    request_id: String,
}

impl OrderedLifecycleItem for OrderedLifecycleDegradedItem {
    fn request_id(&self) -> &str {
        &self.request_id
    }

    fn start(self: Box<Self>, _completion: OrderedLifecycleCompletion) {}
}

#[derive(Debug, Default)]
struct OrderedLifecycleState {
    pending: AtomicUsize,
    max_pending: AtomicUsize,
}

impl OrderedLifecycleState {
    fn record_dispatched(&self) {
        let pending = self.pending.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_pending.fetch_max(pending, Ordering::AcqRel);
    }

    fn record_finished(&self) {
        self.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct OrderedLifecycleQueues {
    active: HashMap<String, LifecycleAdmissionPermit>,
    queued: HashMap<String, VecDeque<OrderedLifecycleEnvelope>>,
    ready: VecDeque<OrderedLifecycleEnvelope>,
}

struct OrderedLifecycleEnvelope {
    item: Box<dyn OrderedLifecycleItem>,
    admission: LifecycleAdmissionPermit,
}

struct OrderedLifecycleCore {
    queues: StdMutex<OrderedLifecycleQueues>,
    notify: tokio::sync::Notify,
    state: Arc<OrderedLifecycleState>,
    terminal_execution: Arc<TerminalExecutionDispatcher>,
}

struct OrderedLifecycleCompletion {
    request_id: String,
    core: Arc<OrderedLifecycleCore>,
    completed: bool,
}

impl OrderedLifecycleCompletion {
    fn terminal_execution(&self) -> Arc<TerminalExecutionDispatcher> {
        Arc::clone(&self.core.terminal_execution)
    }

    fn complete(mut self) {
        self.finish();
        self.completed = true;
    }

    fn finish(&mut self) {
        let mut queues = self
            .core
            .queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if queues.active.remove(&self.request_id).is_none() {
            warn!(
                event_name = "usage_ordered_lifecycle_duplicate_completion",
                log_type = "ops",
                request_id = %self.request_id,
                "usage ordered lifecycle received an unexpected duplicate completion"
            );
            return;
        }
        self.core.state.record_finished();
        let next = queues
            .queued
            .get_mut(&self.request_id)
            .and_then(VecDeque::pop_front);
        if let Some(next) = next {
            queues
                .active
                .insert(self.request_id.clone(), Arc::clone(&next.admission));
            queues.ready.push_back(next);
        } else {
            queues.queued.remove(&self.request_id);
        }
        drop(queues);
        self.core.notify.notify_one();
    }
}

impl Drop for OrderedLifecycleCompletion {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        warn!(
            event_name = "usage_ordered_lifecycle_degraded_completion",
            log_type = "ops",
            request_id = %self.request_id,
            fallback = "advance_after_failed_intermediate",
            "usage ordered lifecycle item ended without explicit completion"
        );
        self.finish();
    }
}

struct OrderedLifecycleDispatcher {
    core: Arc<OrderedLifecycleCore>,
}

impl std::fmt::Debug for OrderedLifecycleDispatcher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OrderedLifecycleDispatcher")
            .field("pending", &self.core.state.pending.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl OrderedLifecycleDispatcher {
    fn disabled(terminal_execution: Arc<TerminalExecutionDispatcher>) -> Arc<Self> {
        Arc::new(Self {
            core: Arc::new(OrderedLifecycleCore {
                queues: StdMutex::new(OrderedLifecycleQueues::default()),
                notify: tokio::sync::Notify::new(),
                state: Arc::new(OrderedLifecycleState::default()),
                terminal_execution,
            }),
        })
    }

    fn spawn(
        config: &UsageRuntimeConfig,
        terminal_execution: Arc<TerminalExecutionDispatcher>,
    ) -> Arc<Self> {
        let dispatcher = Self::disabled(terminal_execution);
        if config.enabled {
            spawn_on_usage_background_runtime(run_ordered_lifecycle_dispatcher(Arc::downgrade(
                &dispatcher.core,
            )));
        }
        dispatcher
    }

    fn dispatch(&self, item: Box<dyn OrderedLifecycleItem>, admission: LifecycleAdmissionPermit) {
        let request_id = item.request_id().to_string();
        let envelope = OrderedLifecycleEnvelope { item, admission };
        self.core.state.record_dispatched();
        let mut queues = self
            .core
            .queues
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !queues.active.contains_key(&request_id) {
            queues
                .active
                .insert(request_id.clone(), Arc::clone(&envelope.admission));
            queues.ready.push_back(envelope);
        } else {
            queues
                .queued
                .entry(request_id)
                .or_default()
                .push_back(envelope);
        }
        drop(queues);
        self.core.notify.notify_one();
    }
}

async fn run_ordered_lifecycle_dispatcher(core: Weak<OrderedLifecycleCore>) {
    loop {
        let Some(core) = core.upgrade() else {
            return;
        };
        let ready = {
            let mut queues = core
                .queues
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            queues.ready.drain(..).collect::<Vec<_>>()
        };
        if ready.is_empty() {
            if Arc::strong_count(&core) == 1 {
                return;
            }
            let _ = tokio::time::timeout(Duration::from_secs(1), core.notify.notified()).await;
            continue;
        }
        for envelope in ready {
            let request_id = envelope.item.request_id().to_string();
            let completion = OrderedLifecycleCompletion {
                request_id: request_id.clone(),
                core: Arc::clone(&core),
                completed: false,
            };
            if catch_unwind(AssertUnwindSafe(|| envelope.item.start(completion))).is_err() {
                warn!(
                    event_name = "usage_ordered_lifecycle_start_panicked",
                    log_type = "ops",
                    request_id,
                    fallback = "advance_after_panicked_handoff",
                    "usage ordered lifecycle item panicked during handoff; releasing its admission"
                );
            }
        }
    }
}

#[derive(Debug, Default)]
struct PendingPersistenceState {
    capacity: usize,
    pending: AtomicUsize,
    max_pending: AtomicUsize,
    batch_flush_total: AtomicU64,
    batch_records_total: AtomicU64,
    max_batch_size: AtomicUsize,
    batch_failed_total: AtomicU64,
    retried_total: AtomicU64,
    overflow_total: AtomicU64,
}

impl PendingPersistenceState {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    fn record_dispatched(&self) -> usize {
        let pending = self.pending.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_pending.fetch_max(pending, Ordering::AcqRel);
        pending
    }

    fn record_finished(&self) {
        self.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

struct PendingPersistencePendingGuard {
    state: Arc<PendingPersistenceState>,
}

impl Drop for PendingPersistencePendingGuard {
    fn drop(&mut self) {
        self.state.record_finished();
    }
}

#[async_trait]
trait PendingPersistenceItem: Send + Sync {
    fn request_id(&self) -> &str;
    fn batch_identity(&self) -> Option<usize>;
    fn build_record(&self) -> Result<UpsertUsageRecord, DataLayerError>;
    fn retry_delay(&self, attempts: u64) -> Duration;
    async fn write_batch(&self, records: Vec<UpsertUsageRecord>) -> Result<(), DataLayerError>;
    async fn complete_success(self: Box<Self>);
    fn complete_degraded(self: Box<Self>);

    async fn persist_reliably(
        self: Box<Self>,
        mut record: Option<UpsertUsageRecord>,
        state: Arc<PendingPersistenceState>,
        mut attempts: u64,
    ) {
        loop {
            if record.is_none() {
                match self.build_record() {
                    Ok(built) => record = Some(built),
                    Err(err) => {
                        attempts = attempts.saturating_add(1);
                        state.retried_total.fetch_add(1, Ordering::AcqRel);
                        if attempts >= PENDING_PERSISTENCE_SINGLE_RETRIES_BEFORE_DEGRADE {
                            state.overflow_total.fetch_add(1, Ordering::AcqRel);
                            warn!(
                                event_name = "usage_pending_record_build_degraded",
                                log_type = "ops",
                                request_id = %self.request_id(),
                                retry_attempts = attempts,
                                error = %err,
                                fallback = "drop_pending_only",
                                "usage pending record build exhausted its bounded retry budget"
                            );
                            self.complete_degraded();
                            return;
                        }
                        let delay = self.retry_delay(attempts);
                        warn!(
                            event_name = "usage_pending_record_build_retry",
                            log_type = "ops",
                            request_id = %self.request_id(),
                            retry_attempt = attempts,
                            retry_delay_ms = delay.as_millis() as u64,
                            error = %err,
                            fallback = "fail_closed_retry",
                            "usage pending record build failed; keeping later phases blocked"
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
            }
            let current = record
                .as_ref()
                .expect("pending retry record should be available")
                .clone();
            match catch_usage_writer_panic(
                "pending single usage write",
                self.write_batch(vec![current]),
            )
            .await
            {
                Ok(()) => {
                    self.complete_success().await;
                    return;
                }
                Err(err) => {
                    attempts = attempts.saturating_add(1);
                    state.batch_failed_total.fetch_add(1, Ordering::AcqRel);
                    state.retried_total.fetch_add(1, Ordering::AcqRel);
                    if attempts >= PENDING_PERSISTENCE_SINGLE_RETRIES_BEFORE_DEGRADE {
                        state.overflow_total.fetch_add(1, Ordering::AcqRel);
                        warn!(
                            event_name = "usage_pending_single_degraded",
                            log_type = "ops",
                            request_id = %self.request_id(),
                            retry_attempts = attempts,
                            error = %err,
                            fallback = "drop_pending_only",
                            "usage pending persistence exhausted its bounded retry budget"
                        );
                        self.complete_degraded();
                        return;
                    }
                    let delay = self.retry_delay(attempts);
                    if should_log_usage_retry_counter(attempts) {
                        warn!(
                            event_name = "usage_pending_single_retry",
                            log_type = "ops",
                            request_id = %self.request_id(),
                            retry_attempt = attempts,
                            retry_delay_ms = delay.as_millis() as u64,
                            error = %err,
                            fallback = "isolated_reliable_retry",
                            "usage pending write failed; retrying without releasing later phases"
                        );
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

struct PendingPersistenceItemImpl<T> {
    runtime: UsageRuntime,
    data: T,
    event: UsageEvent,
    request_id: String,
    ordered_completion: OrderedLifecycleCompletion,
}

#[async_trait]
impl<T> PendingPersistenceItem for PendingPersistenceItemImpl<T>
where
    T: UsageRuntimeAccess + Clone + 'static,
{
    fn request_id(&self) -> &str {
        &self.request_id
    }

    fn batch_identity(&self) -> Option<usize> {
        (self.data.has_usage_writer()
            && UsageRecordWriter::supports_pending_usage_batch(&self.data))
        .then(|| UsageRecordWriter::pending_usage_writer_identity(&self.data))
        .flatten()
    }

    fn build_record(&self) -> Result<UpsertUsageRecord, DataLayerError> {
        build_upsert_usage_record_from_event(&self.event)
    }

    fn retry_delay(&self, attempts: u64) -> Duration {
        usage_enqueue_retry_delay(&self.runtime.config, attempts)
    }

    async fn write_batch(&self, records: Vec<UpsertUsageRecord>) -> Result<(), DataLayerError> {
        if let Some(gate) = &self.runtime.worker_record_gate {
            let _permit = gate.acquire().await;
            self.data.upsert_pending_usage_records(records).await
        } else {
            self.data.upsert_pending_usage_records(records).await
        }
    }

    async fn complete_success(self: Box<Self>) {
        self.ordered_completion.complete();
    }

    fn complete_degraded(self: Box<Self>) {
        self.ordered_completion.complete();
    }
}

struct PendingPersistenceEnvelope {
    item: Box<dyn PendingPersistenceItem>,
    _pending_guard: PendingPersistencePendingGuard,
}

#[derive(Debug)]
struct PendingPersistenceDispatcher {
    sender: Option<mpsc::Sender<PendingPersistenceEnvelope>>,
    state: Arc<PendingPersistenceState>,
}

impl PendingPersistenceDispatcher {
    fn disabled() -> Arc<Self> {
        Arc::new(Self {
            sender: None,
            state: Arc::new(PendingPersistenceState::default()),
        })
    }

    fn spawn(config: &UsageRuntimeConfig) -> Arc<Self> {
        if !config.enabled {
            return Self::disabled();
        }
        let capacity = config
            .enqueue_retry_buffer_capacity
            .clamp(1_024, PENDING_PERSISTENCE_MAX_BUFFER);
        let state = Arc::new(PendingPersistenceState::new(capacity));
        let (sender, receiver) = mpsc::channel(capacity);
        spawn_on_usage_background_runtime(run_pending_persistence_dispatcher(
            receiver,
            Arc::clone(&state),
        ));
        Arc::new(Self {
            sender: Some(sender),
            state,
        })
    }

    fn dispatch(&self, item: Box<dyn PendingPersistenceItem>) {
        let pending = self.state.record_dispatched();
        let envelope = PendingPersistenceEnvelope {
            item,
            _pending_guard: PendingPersistencePendingGuard {
                state: Arc::clone(&self.state),
            },
        };
        let Some(sender) = &self.sender else {
            complete_degraded_pending_envelope(envelope);
            return;
        };
        match sender.try_send(envelope) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(envelope)) => {
                let overflow = self.state.overflow_total.fetch_add(1, Ordering::AcqRel) + 1;
                if should_log_usage_retry_counter(overflow) {
                    warn!(
                        event_name = "usage_pending_persistence_overflow",
                        log_type = "ops",
                        pending,
                        capacity = self.state.capacity,
                        overflow_total = overflow,
                        fallback = "drop_pending_only",
                        "usage pending persistence reached capacity; allowing later lifecycle phases to create the row"
                    );
                }
                complete_degraded_pending_envelope(envelope);
            }
            Err(mpsc::error::TrySendError::Closed(envelope)) => {
                self.state.overflow_total.fetch_add(1, Ordering::AcqRel);
                complete_degraded_pending_envelope(envelope);
            }
        }
    }
}

fn complete_degraded_pending_envelope(envelope: PendingPersistenceEnvelope) {
    let PendingPersistenceEnvelope {
        item,
        _pending_guard,
    } = envelope;
    item.complete_degraded();
    drop(_pending_guard);
}

async fn run_pending_persistence_dispatcher(
    mut receiver: mpsc::Receiver<PendingPersistenceEnvelope>,
    state: Arc<PendingPersistenceState>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        while tasks.len() >= PENDING_PERSISTENCE_MAX_BATCH_CONCURRENCY {
            report_pending_persistence_join(tasks.join_next().await);
        }
        let Some(first) = receiver.recv().await else {
            break;
        };
        let mut batch = vec![first];
        while batch.len() < PENDING_PERSISTENCE_BATCH_SIZE {
            while batch.len() < PENDING_PERSISTENCE_BATCH_SIZE {
                match receiver.try_recv() {
                    Ok(item) => batch.push(item),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                }
            }
            if batch.len() >= PENDING_PERSISTENCE_BATCH_SIZE {
                break;
            }
            match tokio::time::timeout(PENDING_PERSISTENCE_BATCH_FLUSH_INTERVAL, receiver.recv())
                .await
            {
                Ok(Some(item)) => batch.push(item),
                Ok(None) | Err(_) => break,
            }
        }
        state
            .max_batch_size
            .fetch_max(batch.len(), Ordering::AcqRel);
        let task_state = Arc::clone(&state);
        tasks.spawn(async move {
            process_pending_persistence_batch(batch, task_state).await;
        });
        while let Some(result) = tasks.try_join_next() {
            report_pending_persistence_join(Some(result));
        }
    }
    while let Some(result) = tasks.join_next().await {
        report_pending_persistence_join(Some(result));
    }
}

async fn process_pending_persistence_batch(
    batch: Vec<PendingPersistenceEnvelope>,
    state: Arc<PendingPersistenceState>,
) {
    if batch.is_empty() {
        return;
    }
    state.batch_flush_total.fetch_add(1, Ordering::AcqRel);
    state.batch_records_total.fetch_add(
        u64::try_from(batch.len()).unwrap_or(u64::MAX),
        Ordering::AcqRel,
    );
    let mut items = batch.into_iter().map(Some).collect::<Vec<_>>();
    let mut groups = HashMap::<usize, Vec<(usize, UpsertUsageRecord)>>::new();
    let mut single_writes = Vec::<(PendingPersistenceEnvelope, Option<UpsertUsageRecord>)>::new();

    for (index, item) in items.iter_mut().enumerate() {
        let Some(envelope) = item.as_ref() else {
            continue;
        };
        let batch_identity = envelope.item.batch_identity();
        let record = match envelope.item.build_record() {
            Ok(record) => record,
            Err(err) => {
                warn!(
                    event_name = "usage_pending_upsert_build_failed",
                    log_type = "event",
                    request_id = %envelope.item.request_id(),
                    error = %err,
                    fallback = "isolated_reliable_retry",
                    "usage runtime failed to build a pending batch record"
                );
                if let Some(envelope) = item.take() {
                    single_writes.push((envelope, None));
                }
                continue;
            }
        };
        if let Some(identity) = batch_identity {
            groups.entry(identity).or_default().push((index, record));
        } else if let Some(envelope) = item.take() {
            single_writes.push((envelope, Some(record)));
        }
    }

    for (_, grouped) in groups {
        let Some(&(leader_index, _)) = grouped.first() else {
            continue;
        };
        let record_count = grouped.len();
        let records = grouped
            .iter()
            .map(|(_, record)| record.clone())
            .collect::<Vec<_>>();
        let mut attempts = 0_u64;
        let write_result = loop {
            let result = catch_usage_writer_panic(
                "pending batch usage write",
                items[leader_index]
                    .as_ref()
                    .expect("pending batch leader should remain available")
                    .item
                    .write_batch(records.clone()),
            )
            .await;
            match result {
                Ok(()) => break Ok(()),
                Err(err) => {
                    attempts = attempts.saturating_add(1);
                    state.batch_failed_total.fetch_add(1, Ordering::AcqRel);
                    state.retried_total.fetch_add(
                        u64::try_from(record_count).unwrap_or(u64::MAX),
                        Ordering::AcqRel,
                    );
                    if attempts >= PENDING_PERSISTENCE_BATCH_RETRIES_BEFORE_ISOLATION {
                        break Err(err);
                    }
                    let delay = items[leader_index]
                        .as_ref()
                        .expect("pending batch leader should remain available")
                        .item
                        .retry_delay(attempts);
                    warn!(
                        event_name = "usage_pending_upsert_batch_retry",
                        log_type = "ops",
                        records = record_count,
                        retry_attempt = attempts,
                        retry_delay_ms = delay.as_millis() as u64,
                        error = %err,
                        "usage pending batch failed; retrying without releasing later phases"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        };

        match write_result {
            Ok(()) => {
                for (index, _) in grouped {
                    if let Some(envelope) = items[index].take() {
                        envelope.item.complete_success().await;
                        drop(envelope._pending_guard);
                    }
                }
            }
            Err(err) => {
                warn!(
                    event_name = "usage_pending_upsert_batch_isolated",
                    log_type = "ops",
                    records = record_count,
                    retry_attempts = attempts,
                    error = %err,
                    fallback = "bounded_per_request_reliable_retry",
                    "usage pending batch repeatedly failed; isolating requests for reliable retry"
                );
                for (index, record) in grouped {
                    if let Some(envelope) = items[index].take() {
                        single_writes.push((envelope, Some(record)));
                    }
                }
            }
        }
    }

    futures_util::stream::iter(single_writes)
        .for_each_concurrent(
            PENDING_PERSISTENCE_SINGLE_WRITE_CONCURRENCY_PER_BATCH,
            |(envelope, record)| {
                let retry_state = Arc::clone(&state);
                async move {
                    let PendingPersistenceEnvelope {
                        item,
                        _pending_guard,
                    } = envelope;
                    item.persist_reliably(record, retry_state, 0).await;
                    drop(_pending_guard);
                }
            },
        )
        .await;
}

fn report_pending_persistence_join(result: Option<Result<(), tokio::task::JoinError>>) {
    if let Some(Err(err)) = result {
        warn!(
            event_name = "usage_pending_persistence_task_failed",
            log_type = "ops",
            error = %err,
            fallback = "advance_after_panicked_pending",
            "usage pending persistence task failed; releasing affected ordered admissions"
        );
    }
}

#[derive(Debug, Default)]
struct FirstBytePersistenceState {
    capacity: usize,
    pending: AtomicUsize,
    max_pending: AtomicUsize,
    dispatched_total: AtomicU64,
    overflow_total: AtomicU64,
    cancelled_total: AtomicU64,
    direct_succeeded_total: AtomicU64,
    direct_failed_total: AtomicU64,
    batch_flush_total: AtomicU64,
    batch_records_total: AtomicU64,
    max_batch_size: AtomicUsize,
    batch_failed_total: AtomicU64,
    fallback_accepted_total: AtomicU64,
    fallback_failed_total: AtomicU64,
}

impl FirstBytePersistenceState {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            ..Self::default()
        }
    }

    fn record_dispatched(&self) {
        self.dispatched_total.fetch_add(1, Ordering::AcqRel);
        let pending = self.pending.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_pending.fetch_max(pending, Ordering::AcqRel);
    }

    fn record_finished(&self) {
        self.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

struct FirstBytePersistencePendingGuard {
    state: Arc<FirstBytePersistenceState>,
}

impl Drop for FirstBytePersistencePendingGuard {
    fn drop(&mut self) {
        self.state.record_finished();
    }
}

struct FirstByteMarkerCleanup {
    coalescer: Arc<LifecycleEventCoalescer>,
    request_id: String,
    generation: u64,
}

impl FirstByteMarkerCleanup {
    async fn rollback(self) {
        self.coalescer
            .rollback_first_byte(&self.request_id, self.generation)
            .await;
    }
}

struct FirstByteMarkerGuard {
    cleanup: Option<FirstByteMarkerCleanup>,
}

impl FirstByteMarkerGuard {
    fn new(coalescer: Arc<LifecycleEventCoalescer>, request_id: String, generation: u64) -> Self {
        Self {
            cleanup: Some(FirstByteMarkerCleanup {
                coalescer,
                request_id,
                generation,
            }),
        }
    }

    fn disarm(&mut self) {
        self.cleanup.take();
    }
}

impl Drop for FirstByteMarkerGuard {
    fn drop(&mut self) {
        let Some(cleanup) = self.cleanup.take() else {
            return;
        };
        spawn_on_usage_background_runtime(async move {
            cleanup.rollback().await;
        });
    }
}

#[async_trait]
trait FirstBytePersistenceItem: Send + Sync {
    fn batch_identity(&self) -> Option<usize>;
    async fn is_current(&self) -> bool;
    fn build_record(&self) -> Result<UpsertUsageRecord, DataLayerError>;
    async fn write_batch(&self, records: Vec<UpsertUsageRecord>) -> Result<(), DataLayerError>;
    async fn complete_success(self: Box<Self>);
    async fn complete_cancelled(self: Box<Self>);
    async fn enqueue_fallback(self: Box<Self>);
}

struct FirstBytePersistenceItemImpl<T> {
    runtime: UsageRuntime,
    data: T,
    direct_event: UsageEvent,
    fallback_event: UsageEvent,
    request_id: String,
    generation: u64,
    marker_guard: FirstByteMarkerGuard,
    completion: Option<tokio::sync::oneshot::Sender<()>>,
    ordered_completion: Option<OrderedLifecycleCompletion>,
}

#[async_trait]
impl<T> FirstBytePersistenceItem for FirstBytePersistenceItemImpl<T>
where
    T: UsageRuntimeAccess + Clone + 'static,
{
    fn batch_identity(&self) -> Option<usize> {
        (self.data.has_usage_writer()
            && self.data.supports_first_byte_usage_fast_path()
            && UsageRecordWriter::supports_first_byte_usage_batch(&self.data))
        .then(|| UsageRecordWriter::first_byte_usage_writer_identity(&self.data))
        .flatten()
    }

    async fn is_current(&self) -> bool {
        self.runtime
            .lifecycle_coalescer
            .first_byte_is_current(&self.request_id, self.generation)
            .await
    }

    fn build_record(&self) -> Result<UpsertUsageRecord, DataLayerError> {
        build_upsert_usage_record_from_event(&self.direct_event)
    }

    async fn write_batch(&self, records: Vec<UpsertUsageRecord>) -> Result<(), DataLayerError> {
        if let Some(gate) = &self.runtime.worker_record_gate {
            let _permit = gate.acquire().await;
            self.data.upsert_first_byte_usage_records(records).await
        } else {
            self.data.upsert_first_byte_usage_records(records).await
        }
    }

    async fn complete_success(self: Box<Self>) {
        let Self {
            runtime,
            request_id,
            generation,
            mut marker_guard,
            completion,
            ordered_completion,
            ..
        } = *self;
        runtime
            .first_byte_persistence
            .state
            .direct_succeeded_total
            .fetch_add(1, Ordering::AcqRel);
        runtime
            .lifecycle_coalescer
            .complete_first_byte(&request_id, generation)
            .await;
        marker_guard.disarm();
        if let Some(completion) = completion {
            let _ = completion.send(());
        }
        if let Some(completion) = ordered_completion {
            completion.complete();
        }
    }

    async fn complete_cancelled(self: Box<Self>) {
        let Self {
            runtime,
            request_id,
            generation,
            mut marker_guard,
            completion,
            ordered_completion,
            ..
        } = *self;
        runtime
            .first_byte_persistence
            .state
            .cancelled_total
            .fetch_add(1, Ordering::AcqRel);
        // Keep the coalescer marker intact when a terminal event won the race. A later first-byte
        // event is allowed to retry only after the existing generation is explicitly rolled back.
        let _ = generation;
        marker_guard.disarm();
        if let Some(completion) = completion {
            let _ = completion.send(());
        }
        if let Some(ordered_completion) = ordered_completion {
            warn!(
                event_name = "usage_ordered_first_byte_cancelled",
                log_type = "ops",
                request_id,
                fallback = "drop_first_byte_only",
                "an ordered first-byte transition was cancelled; allowing its terminal barrier to advance"
            );
            ordered_completion.complete();
        }
    }

    async fn enqueue_fallback(self: Box<Self>) {
        let Self {
            runtime,
            data,
            direct_event: _,
            fallback_event,
            request_id,
            generation,
            mut marker_guard,
            completion,
            ordered_completion,
            ..
        } = *self;
        if let Some(ordered_completion) = ordered_completion {
            let degraded = runtime
                .first_byte_persistence
                .state
                .fallback_failed_total
                .fetch_add(1, Ordering::AcqRel)
                + 1;
            if should_log_usage_retry_counter(degraded) {
                warn!(
                    event_name = "usage_ordered_first_byte_degraded",
                    log_type = "ops",
                    request_id,
                    degraded_total = degraded,
                    fallback = "drop_first_byte_only",
                    "ordered first-byte persistence degraded so terminal usage can continue"
                );
            }
            runtime
                .lifecycle_coalescer
                .complete_first_byte(&request_id, generation)
                .await;
            marker_guard.disarm();
            if let Some(completion) = completion {
                let _ = completion.send(());
            }
            ordered_completion.complete();
            return;
        }
        enqueue_first_byte_fallback(runtime, data, fallback_event, request_id, generation).await;
        marker_guard.disarm();
        if let Some(completion) = completion {
            let _ = completion.send(());
        }
    }
}

async fn enqueue_first_byte_fallback<T>(
    runtime: UsageRuntime,
    data: T,
    event: UsageEvent,
    request_id: String,
    generation: u64,
) where
    T: UsageRuntimeAccess,
{
    if !runtime
        .lifecycle_coalescer
        .first_byte_is_current(&request_id, generation)
        .await
    {
        runtime
            .first_byte_persistence
            .state
            .cancelled_total
            .fetch_add(1, Ordering::AcqRel);
        return;
    }
    if runtime.enqueue_lifecycle_event(&data, event).await {
        runtime
            .first_byte_persistence
            .state
            .fallback_accepted_total
            .fetch_add(1, Ordering::AcqRel);
        runtime
            .lifecycle_coalescer
            .complete_first_byte(&request_id, generation)
            .await;
    } else {
        runtime
            .first_byte_persistence
            .state
            .fallback_failed_total
            .fetch_add(1, Ordering::AcqRel);
        runtime
            .lifecycle_coalescer
            .rollback_first_byte(&request_id, generation)
            .await;
    }
}

#[derive(Debug)]
struct FirstBytePersistenceDispatcher {
    sender: Option<mpsc::Sender<Box<dyn FirstBytePersistenceItem>>>,
    state: Arc<FirstBytePersistenceState>,
}

impl FirstBytePersistenceDispatcher {
    fn disabled() -> Arc<Self> {
        Arc::new(Self {
            sender: None,
            state: Arc::new(FirstBytePersistenceState::default()),
        })
    }

    fn spawn(config: &UsageRuntimeConfig) -> Arc<Self> {
        if !config.enabled || !config.queue_lifecycle_events {
            return Self::disabled();
        }
        let capacity = config
            .enqueue_retry_buffer_capacity
            .clamp(1_024, FIRST_BYTE_PERSISTENCE_MAX_BUFFER);
        let concurrency = config
            .worker_record_concurrency_limit
            .unwrap_or(FIRST_BYTE_PERSISTENCE_DEFAULT_CONCURRENCY)
            .clamp(1, 256);
        let state = Arc::new(FirstBytePersistenceState::new(capacity));
        let (sender, receiver) = mpsc::channel(capacity);
        spawn_on_usage_background_runtime(run_first_byte_persistence_dispatcher(
            receiver,
            concurrency,
            Arc::clone(&state),
        ));
        Arc::new(Self {
            sender: Some(sender),
            state,
        })
    }

    async fn dispatch(&self, item: Box<dyn FirstBytePersistenceItem>) {
        let Some(sender) = &self.sender else {
            item.enqueue_fallback().await;
            return;
        };
        self.state.record_dispatched();
        if let Err(err) = sender.try_send(item) {
            self.state.record_finished();
            self.state.overflow_total.fetch_add(1, Ordering::AcqRel);
            err.into_inner().enqueue_fallback().await;
        }
    }
}

async fn run_first_byte_persistence_dispatcher(
    mut receiver: mpsc::Receiver<Box<dyn FirstBytePersistenceItem>>,
    concurrency: usize,
    state: Arc<FirstBytePersistenceState>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    let batch_concurrency = concurrency.clamp(1, FIRST_BYTE_PERSISTENCE_MAX_BATCH_CONCURRENCY);
    let write_admission = Arc::new(tokio::sync::Semaphore::new(concurrency));
    while let Some(first) = receiver.recv().await {
        let mut batch = vec![first];
        while batch.len() < FIRST_BYTE_PERSISTENCE_BATCH_SIZE {
            while batch.len() < FIRST_BYTE_PERSISTENCE_BATCH_SIZE {
                match receiver.try_recv() {
                    Ok(item) => batch.push(item),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                }
            }
            if batch.len() >= FIRST_BYTE_PERSISTENCE_BATCH_SIZE {
                break;
            }
            match tokio::time::timeout(FIRST_BYTE_PERSISTENCE_BATCH_FLUSH_INTERVAL, receiver.recv())
                .await
            {
                Ok(Some(item)) => batch.push(item),
                Ok(None) | Err(_) => break,
            }
        }
        while tasks.len() >= batch_concurrency {
            report_first_byte_persistence_join(tasks.join_next().await);
        }
        state
            .max_batch_size
            .fetch_max(batch.len(), Ordering::AcqRel);
        let task_state = Arc::clone(&state);
        let batch_write_admission = Arc::clone(&write_admission);
        tasks.spawn(async move {
            process_first_byte_persistence_batch(
                batch,
                task_state,
                concurrency,
                batch_write_admission,
            )
            .await;
        });
        while let Some(result) = tasks.try_join_next() {
            report_first_byte_persistence_join(Some(result));
        }
    }
    while let Some(result) = tasks.join_next().await {
        report_first_byte_persistence_join(Some(result));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FirstByteBatchKey {
    Shared(usize),
    Single(usize),
}

async fn process_first_byte_persistence_batch(
    batch: Vec<Box<dyn FirstBytePersistenceItem>>,
    state: Arc<FirstBytePersistenceState>,
    single_write_concurrency: usize,
    write_admission: Arc<tokio::sync::Semaphore>,
) {
    if batch.is_empty() {
        return;
    }
    state.batch_flush_total.fetch_add(1, Ordering::AcqRel);
    state.batch_records_total.fetch_add(
        u64::try_from(batch.len()).unwrap_or(u64::MAX),
        Ordering::AcqRel,
    );
    let _pending_guards = batch
        .iter()
        .map(|_| FirstBytePersistencePendingGuard {
            state: Arc::clone(&state),
        })
        .collect::<Vec<_>>();

    let mut groups = HashMap::<
        FirstByteBatchKey,
        Vec<(Box<dyn FirstBytePersistenceItem>, UpsertUsageRecord)>,
    >::new();

    for (index, item) in batch.into_iter().enumerate() {
        if !item.is_current().await {
            item.complete_cancelled().await;
            continue;
        }
        let record = match item.build_record() {
            Ok(record) => record,
            Err(err) => {
                warn!(
                    event_name = "usage_first_byte_upsert_build_failed",
                    log_type = "event",
                    error = %err,
                    "usage runtime failed to build a first-byte batch record"
                );
                state.direct_failed_total.fetch_add(1, Ordering::AcqRel);
                item.enqueue_fallback().await;
                continue;
            }
        };
        let key = item
            .batch_identity()
            .map(FirstByteBatchKey::Shared)
            .unwrap_or(FirstByteBatchKey::Single(index));
        groups.entry(key).or_default().push((item, record));
    }

    futures_util::stream::iter(groups.into_values())
        .for_each_concurrent(single_write_concurrency.max(1), |grouped| {
            let group_state = Arc::clone(&state);
            let group_write_admission = Arc::clone(&write_admission);
            async move {
                process_first_byte_persistence_group(grouped, group_state, group_write_admission)
                    .await;
            }
        })
        .await;
}

async fn process_first_byte_persistence_group(
    grouped: Vec<(Box<dyn FirstBytePersistenceItem>, UpsertUsageRecord)>,
    state: Arc<FirstBytePersistenceState>,
    write_admission: Arc<tokio::sync::Semaphore>,
) {
    let Some((leader, _)) = grouped.first() else {
        return;
    };
    // Repository batch contracts preserve input order for duplicate request IDs, matching
    // repeated single-row first-byte upserts and allowing later records to merge other fields.
    let records = grouped
        .iter()
        .map(|(_, record)| record.clone())
        .collect::<Vec<_>>();
    let record_count = records.len();
    let write_permit = write_admission
        .acquire_owned()
        .await
        .expect("first-byte write admission should remain open");
    let write_result =
        catch_usage_writer_panic("first-byte batch usage write", leader.write_batch(records)).await;
    drop(write_permit);
    match write_result {
        Ok(()) => {
            for (item, _) in grouped {
                item.complete_success().await;
            }
        }
        Err(err) => {
            state.batch_failed_total.fetch_add(1, Ordering::AcqRel);
            warn!(
                event_name = "usage_first_byte_upsert_batch_failed",
                log_type = "ops",
                records = record_count,
                error = %err,
                "usage runtime failed to persist a first-byte batch; falling back per item"
            );
            for (item, _) in grouped {
                state.direct_failed_total.fetch_add(1, Ordering::AcqRel);
                item.enqueue_fallback().await;
            }
        }
    }
}

fn report_first_byte_persistence_join(result: Option<Result<(), tokio::task::JoinError>>) {
    if let Some(Err(err)) = result {
        warn!(
            event_name = "usage_first_byte_persistence_task_failed",
            log_type = "ops",
            error = %err,
            "usage first-byte persistence task failed"
        );
    }
}

impl Default for UsageRuntime {
    fn default() -> Self {
        Self::disabled()
    }
}

impl UsageRuntime {
    pub fn disabled() -> Self {
        let terminal_execution = TerminalExecutionDispatcher::disabled();
        Self {
            config: UsageRuntimeConfig::disabled(),
            body_policy_cache: Arc::new(tokio::sync::Mutex::new(None)),
            enqueue_retry: UsageEnqueueRetryDispatcher::disabled(),
            worker_supervisor_state: Arc::new(UsageWorkerSupervisorState::default()),
            worker_record_gate: None,
            terminal_submission_state: Arc::new(TerminalSubmissionState::new(1)),
            terminal_execution: Arc::clone(&terminal_execution),
            terminal_enqueue_state: Arc::new(LifecycleEnqueueState::default()),
            terminal_direct_fallback_state: Arc::new(TerminalDirectFallbackState::new(
                TERMINAL_DIRECT_FALLBACK_DEFAULT_MAX_IN_FLIGHT,
            )),
            lifecycle_enqueue_state: Arc::new(LifecycleEnqueueState::default()),
            lifecycle_coalescer: Arc::new(LifecycleEventCoalescer::default()),
            lifecycle_delay: LifecycleDelayDispatcher::disabled(),
            lifecycle_submission: LifecycleSubmissionDispatcher::disabled(),
            ordered_lifecycle: OrderedLifecycleDispatcher::disabled(terminal_execution),
            pending_persistence: PendingPersistenceDispatcher::disabled(),
            first_byte_persistence: FirstBytePersistenceDispatcher::disabled(),
        }
    }

    pub fn new(config: UsageRuntimeConfig) -> Result<Self, DataLayerError> {
        config.validate()?;
        let enqueue_retry = UsageEnqueueRetryDispatcher::spawn(config.clone());
        let worker_record_gate = config
            .worker_record_concurrency_limit
            .map(UsageWorkerRecordConcurrencyGate::new)
            .map(Arc::new);
        let lifecycle_enqueue_state = Arc::new(LifecycleEnqueueState::default());
        let lifecycle_coalescer = Arc::new(LifecycleEventCoalescer::new(
            config.enqueue_retry_buffer_capacity,
        ));
        if config.enabled {
            spawn_on_usage_background_runtime(run_lifecycle_coalescer_compactor(Arc::downgrade(
                &lifecycle_coalescer,
            )));
        }
        let terminal_submission_state = Arc::new(TerminalSubmissionState::new(
            terminal_submission_limit(&config),
        ));
        let terminal_execution = TerminalExecutionDispatcher::spawn(&config);
        let ordered_lifecycle =
            OrderedLifecycleDispatcher::spawn(&config, Arc::clone(&terminal_execution));
        let pending_persistence = PendingPersistenceDispatcher::spawn(&config);
        let terminal_direct_fallback_state = Arc::new(TerminalDirectFallbackState::new(
            terminal_direct_fallback_limit(&config),
        ));
        let lifecycle_delay = LifecycleDelayDispatcher::spawn(
            config.clone(),
            Arc::clone(&lifecycle_coalescer),
            Arc::clone(&lifecycle_enqueue_state),
            Arc::clone(&enqueue_retry),
        );
        let lifecycle_submission = LifecycleSubmissionDispatcher::spawn(&config);
        let first_byte_persistence = FirstBytePersistenceDispatcher::spawn(&config);
        Ok(Self {
            config,
            body_policy_cache: Arc::new(tokio::sync::Mutex::new(None)),
            enqueue_retry,
            worker_supervisor_state: Arc::new(UsageWorkerSupervisorState::default()),
            worker_record_gate,
            terminal_submission_state,
            terminal_execution,
            terminal_enqueue_state: Arc::new(LifecycleEnqueueState::default()),
            terminal_direct_fallback_state,
            lifecycle_enqueue_state,
            lifecycle_coalescer,
            lifecycle_delay,
            lifecycle_submission,
            ordered_lifecycle,
            pending_persistence,
            first_byte_persistence,
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn metrics_snapshot(&self) -> UsageRuntimeMetricsSnapshot {
        UsageRuntimeMetricsSnapshot {
            enabled: self.config.enabled,
            queue_terminal_events: self.config.queue_terminal_events,
            queue_lifecycle_events: self.config.queue_lifecycle_events,
            worker_count: self.config.worker_count,
            worker_autoscale_enabled: self.config.worker_autoscale_enabled,
            worker_max_count: self.config.worker_max_count,
            worker_record_concurrency_limit: self
                .worker_record_gate
                .as_ref()
                .map(|gate| gate.limit()),
            worker_record_concurrency_in_flight: self
                .worker_record_gate
                .as_ref()
                .map(|gate| gate.in_flight())
                .unwrap_or_default(),
            worker_record_concurrency_max_in_flight: self
                .worker_record_gate
                .as_ref()
                .map(|gate| gate.max_in_flight())
                .unwrap_or_default(),
            worker_record_concurrency_wait_total: self
                .worker_record_gate
                .as_ref()
                .map(|gate| gate.wait_total())
                .unwrap_or_default(),
            worker_record_deferred_total: self
                .worker_record_gate
                .as_ref()
                .map(|gate| gate.deferred_total())
                .unwrap_or_default(),
            worker_active_count: self
                .worker_supervisor_state
                .active_count
                .load(Ordering::Acquire),
            worker_desired_count: self
                .worker_supervisor_state
                .desired_count
                .load(Ordering::Acquire),
            worker_read_batches_total: self
                .worker_supervisor_state
                .read_batches_total
                .load(Ordering::Acquire),
            worker_read_entries_total: self
                .worker_supervisor_state
                .read_entries_total
                .load(Ordering::Acquire),
            worker_reclaimed_entries_total: self
                .worker_supervisor_state
                .reclaimed_entries_total
                .load(Ordering::Acquire),
            worker_acked_entries_total: self
                .worker_supervisor_state
                .acked_entries_total
                .load(Ordering::Acquire),
            worker_dead_lettered_entries_total: self
                .worker_supervisor_state
                .dead_lettered_entries_total
                .load(Ordering::Acquire),
            worker_process_failures_total: self
                .worker_supervisor_state
                .process_failures_total
                .load(Ordering::Acquire),
            worker_read_failures_total: self
                .worker_supervisor_state
                .read_failures_total
                .load(Ordering::Acquire),
            worker_reclaim_failures_total: self
                .worker_supervisor_state
                .reclaim_failures_total
                .load(Ordering::Acquire),
            retry_deferred_lifecycle_events: self.config.retry_deferred_lifecycle_events,
            terminal_submission_limit: self.terminal_submission_state.limit(),
            terminal_submission_pending: self.terminal_submission_state.pending(),
            terminal_submission_max_pending: self.terminal_submission_state.max_pending(),
            terminal_submission_in_flight: self.terminal_submission_state.in_flight(),
            terminal_submission_max_in_flight: self.terminal_submission_state.max_in_flight(),
            terminal_submission_rejected_total: self.terminal_submission_state.rejected_total(),
            terminal_enqueue_in_flight: self.terminal_enqueue_state.in_flight(),
            terminal_enqueue_deferred_total: self.terminal_enqueue_state.deferred_total(),
            terminal_enqueue_deferred_direct_write_total: self
                .terminal_enqueue_state
                .deferred_direct_write_total(),
            terminal_enqueue_deferred_dropped_total: self
                .terminal_enqueue_state
                .deferred_dropped_total(),
            terminal_enqueue_deferred_retry_total: self
                .terminal_enqueue_state
                .deferred_retry_total(),
            terminal_enqueue_failed_total: self.terminal_enqueue_state.failed_total(),
            terminal_direct_fallback_limit: self.terminal_direct_fallback_state.limit(),
            terminal_direct_fallback_in_flight: self.terminal_direct_fallback_state.in_flight(),
            terminal_direct_fallback_max_in_flight: self
                .terminal_direct_fallback_state
                .max_in_flight(),
            terminal_direct_fallback_succeeded_total: self
                .terminal_direct_fallback_state
                .succeeded_total(),
            terminal_direct_fallback_failed_total: self
                .terminal_direct_fallback_state
                .failed_total(),
            terminal_direct_fallback_rejected_total: self
                .terminal_direct_fallback_state
                .rejected_total(),
            lifecycle_enqueue_in_flight: self.lifecycle_enqueue_state.in_flight(),
            lifecycle_enqueue_deferred_total: self.lifecycle_enqueue_state.deferred_total(),
            lifecycle_enqueue_deferred_dropped_total: self
                .lifecycle_enqueue_state
                .deferred_dropped_total(),
            lifecycle_enqueue_deferred_retry_total: self
                .lifecycle_enqueue_state
                .deferred_retry_total(),
            lifecycle_enqueue_failed_total: self.lifecycle_enqueue_state.failed_total(),
            lifecycle_submission_capacity: self.lifecycle_submission.state.capacity,
            lifecycle_submission_workers: self.lifecycle_submission.state.workers,
            lifecycle_submission_pending: self
                .lifecycle_submission
                .state
                .pending
                .load(Ordering::Acquire),
            lifecycle_submission_max_pending: self
                .lifecycle_submission
                .state
                .max_pending
                .load(Ordering::Acquire),
            lifecycle_submission_enqueued_total: self
                .lifecycle_submission
                .state
                .enqueued_total
                .load(Ordering::Acquire),
            lifecycle_submission_coalesced_total: self
                .lifecycle_submission
                .state
                .coalesced_total
                .load(Ordering::Acquire),
            lifecycle_submission_overflow_total: self
                .lifecycle_submission
                .state
                .overflow_total
                .load(Ordering::Acquire),
            lifecycle_submission_processed_total: self
                .lifecycle_submission
                .state
                .processed_total
                .load(Ordering::Acquire),
            ordered_lifecycle_pending: self
                .ordered_lifecycle
                .core
                .state
                .pending
                .load(Ordering::Acquire),
            ordered_lifecycle_max_pending: self
                .ordered_lifecycle
                .core
                .state
                .max_pending
                .load(Ordering::Acquire),
            pending_persistence_capacity: self.pending_persistence.state.capacity,
            pending_persistence_pending: self
                .pending_persistence
                .state
                .pending
                .load(Ordering::Acquire),
            pending_persistence_max_pending: self
                .pending_persistence
                .state
                .max_pending
                .load(Ordering::Acquire),
            pending_persistence_batch_flush_total: self
                .pending_persistence
                .state
                .batch_flush_total
                .load(Ordering::Acquire),
            pending_persistence_batch_records_total: self
                .pending_persistence
                .state
                .batch_records_total
                .load(Ordering::Acquire),
            pending_persistence_max_batch_size: self
                .pending_persistence
                .state
                .max_batch_size
                .load(Ordering::Acquire),
            pending_persistence_batch_failed_total: self
                .pending_persistence
                .state
                .batch_failed_total
                .load(Ordering::Acquire),
            pending_persistence_retried_total: self
                .pending_persistence
                .state
                .retried_total
                .load(Ordering::Acquire),
            pending_persistence_overflow_total: self
                .pending_persistence
                .state
                .overflow_total
                .load(Ordering::Acquire),
            lifecycle_coalescer_entries: self
                .lifecycle_coalescer
                .entry_count
                .load(Ordering::Acquire),
            lifecycle_coalescer_compact_total: self
                .lifecycle_coalescer
                .compact_total
                .load(Ordering::Acquire),
            lifecycle_coalescer_compact_entries_scanned_total: self
                .lifecycle_coalescer
                .compact_entries_scanned_total
                .load(Ordering::Acquire),
            first_byte_persistence_capacity: self.first_byte_persistence.state.capacity,
            first_byte_persistence_pending: self
                .first_byte_persistence
                .state
                .pending
                .load(Ordering::Acquire),
            first_byte_persistence_max_pending: self
                .first_byte_persistence
                .state
                .max_pending
                .load(Ordering::Acquire),
            first_byte_persistence_dispatched_total: self
                .first_byte_persistence
                .state
                .dispatched_total
                .load(Ordering::Acquire),
            first_byte_persistence_overflow_total: self
                .first_byte_persistence
                .state
                .overflow_total
                .load(Ordering::Acquire),
            first_byte_persistence_cancelled_total: self
                .first_byte_persistence
                .state
                .cancelled_total
                .load(Ordering::Acquire),
            first_byte_persistence_direct_succeeded_total: self
                .first_byte_persistence
                .state
                .direct_succeeded_total
                .load(Ordering::Acquire),
            first_byte_persistence_direct_failed_total: self
                .first_byte_persistence
                .state
                .direct_failed_total
                .load(Ordering::Acquire),
            first_byte_persistence_batch_flush_total: self
                .first_byte_persistence
                .state
                .batch_flush_total
                .load(Ordering::Acquire),
            first_byte_persistence_batch_records_total: self
                .first_byte_persistence
                .state
                .batch_records_total
                .load(Ordering::Acquire),
            first_byte_persistence_max_batch_size: self
                .first_byte_persistence
                .state
                .max_batch_size
                .load(Ordering::Acquire),
            first_byte_persistence_batch_failed_total: self
                .first_byte_persistence
                .state
                .batch_failed_total
                .load(Ordering::Acquire),
            first_byte_persistence_fallback_accepted_total: self
                .first_byte_persistence
                .state
                .fallback_accepted_total
                .load(Ordering::Acquire),
            first_byte_persistence_fallback_failed_total: self
                .first_byte_persistence
                .state
                .fallback_failed_total
                .load(Ordering::Acquire),
            enqueue_retry_scheduled_total: self.enqueue_retry.scheduled_total(),
            enqueue_retry_recovered_total: self.enqueue_retry.recovered_total(),
            enqueue_retry_pending: self.enqueue_retry.pending(),
            enqueue_retry_failed_total: self.enqueue_retry.retry_failed_total(),
            enqueue_retry_closed_or_unavailable_total: self
                .enqueue_retry
                .closed_or_unavailable_total(),
        }
    }

    pub async fn queue_health_snapshot<T>(
        &self,
        data: &T,
    ) -> Result<UsageQueueHealthSnapshot, DataLayerError>
    where
        T: UsageRuntimeAccess,
    {
        let mut snapshot = UsageQueueHealthSnapshot {
            enabled: self.config.enabled,
            configured: false,
            stream_key: self.config.stream_key.clone(),
            consumer_group: self.config.consumer_group.clone(),
            dlq_stream_key: self.config.dlq_stream_key.clone(),
            stream_length: 0,
            group_pending: 0,
            group_lag: None,
            oldest_pending_idle_ms: None,
            dlq_length: 0,
        };
        if !self.config.enabled {
            return Ok(snapshot);
        }
        let Some(runner) = data.usage_worker_queue() else {
            return Ok(snapshot);
        };
        snapshot.configured = true;
        let stream_stats = runner
            .stats(&self.config.stream_key, Some(&self.config.consumer_group))
            .await?;
        let dlq_stats = runner.stats(&self.config.dlq_stream_key, None).await?;
        snapshot.apply_stream_stats(stream_stats);
        snapshot.dlq_length = dlq_stats.stream_length;
        Ok(snapshot)
    }

    pub fn can_spawn_worker<T>(&self, data: &T) -> bool
    where
        T: UsageRuntimeAccess,
    {
        self.is_enabled()
            && (self.config.queue_terminal_events || self.config.queue_lifecycle_events)
            && data.has_usage_writer()
            && data.has_usage_worker_queue()
    }

    pub fn spawn_worker<T>(&self, data: Arc<T>) -> Option<tokio::task::JoinHandle<()>>
    where
        T: UsageRuntimeAccess + 'static,
    {
        if !self.can_spawn_worker(data.as_ref()) {
            return None;
        }
        let runner = data.usage_worker_queue()?;
        let worker = build_usage_queue_worker_with_record_gate(
            runner,
            data,
            self.config.clone(),
            self.worker_record_gate.clone(),
            None,
        )
        .ok()?;
        Some(worker.spawn())
    }

    pub fn spawn_workers<T>(&self, data: Arc<T>) -> Vec<tokio::task::JoinHandle<()>>
    where
        T: UsageRuntimeAccess + 'static,
    {
        if !self.can_spawn_worker(data.as_ref()) {
            return Vec::new();
        }
        let Some(runner) = data.usage_worker_queue() else {
            return Vec::new();
        };

        let worker_count = self.config.worker_count.max(1);
        let mut handles = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let Ok(worker) = build_usage_queue_worker_with_record_gate(
                Arc::clone(&runner),
                Arc::clone(&data),
                self.config.clone(),
                self.worker_record_gate.clone(),
                Some(worker_index),
            ) else {
                warn!(
                    event_name = "usage_worker_build_failed",
                    log_type = "ops",
                    worker_index,
                    "usage runtime failed to build usage queue worker"
                );
                continue;
            };
            handles.push(worker.spawn());
        }
        handles
    }

    pub fn spawn_worker_supervisor<T>(&self, data: Arc<T>) -> Option<tokio::task::JoinHandle<()>>
    where
        T: UsageRuntimeAccess + 'static,
    {
        if !self.can_spawn_worker(data.as_ref()) {
            return None;
        }
        let runner = data.usage_worker_queue()?;
        Some(spawn_on_usage_background_runtime(
            run_usage_worker_supervisor(
                runner,
                data,
                self.config.clone(),
                self.worker_record_gate.clone(),
                Arc::clone(&self.worker_supervisor_state),
            ),
        ))
    }

    pub fn record_pending<T>(&self, data: &T, seed: LifecycleUsageSeed)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let request_id = seed.request_id.clone();
        self.lifecycle_submission
            .dispatch(Box::new(LifecycleSubmissionItemImpl {
                runtime: self.clone(),
                data: T::clone(data),
                request_id,
                payload: LifecycleSubmissionPayload::Pending {
                    seed,
                    observed_at_unix_secs: now_unix_secs(),
                },
            }));
    }

    pub async fn record_pending_direct<T>(&self, data: &T, seed: LifecycleUsageSeed)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        // Keep the direct API non-blocking as well. The ordered dispatcher builds the lightweight
        // pending event off the response task and commits it before this request's streaming item.
        self.record_pending(data, seed);
    }

    pub fn record_stream_started<T>(
        &self,
        data: &T,
        seed: &LifecycleUsageSeed,
        status_code: u16,
        telemetry: Option<&ExecutionTelemetry>,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let seed = seed.clone();
        let telemetry = telemetry.cloned();
        let request_id = seed.request_id.clone();
        self.lifecycle_submission
            .dispatch(Box::new(LifecycleSubmissionItemImpl {
                runtime: self.clone(),
                data: T::clone(data),
                request_id,
                payload: LifecycleSubmissionPayload::Streaming {
                    seed,
                    status_code,
                    telemetry,
                    observed_at_unix_secs: now_unix_secs(),
                },
            }));
    }

    async fn dispatch_terminal<T>(
        &self,
        data: &T,
        event: UsageEvent,
        direct: bool,
        completion: Option<tokio::sync::oneshot::Sender<()>>,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        let request_id = event.request_id.clone();
        self.lifecycle_submission
            .dispatch_terminal(Box::new(LifecycleSubmissionItemImpl {
                runtime: self.clone(),
                data: T::clone(data),
                request_id,
                payload: LifecycleSubmissionPayload::Terminal {
                    event,
                    direct,
                    completion,
                },
            }))
            .await;
    }

    async fn dispatch_terminal_seed<T>(
        &self,
        data: &T,
        request_id: String,
        seed: LifecycleTerminalUsageSeed,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        self.lifecycle_submission
            .dispatch_terminal(Box::new(LifecycleSubmissionItemImpl {
                runtime: self.clone(),
                data: T::clone(data),
                request_id,
                payload: LifecycleSubmissionPayload::TerminalSeed {
                    seed,
                    observed_at_unix_ms: now_unix_ms(),
                },
            }))
            .await;
    }

    async fn await_lifecycle_submission_turn(
        &self,
        request_id: &str,
    ) -> Option<OrderedLifecycleCompletion> {
        let (completion, completed) = tokio::sync::oneshot::channel();
        let accepted = self
            .lifecycle_submission
            .dispatch_terminal(Box::new(LifecycleSubmissionBarrierItem {
                request_id: request_id.to_string(),
                ordered_lifecycle: Arc::clone(&self.ordered_lifecycle),
                completion,
            }))
            .await;
        if !accepted {
            return None;
        }
        match completed.await {
            Ok(ordered_completion) => Some(ordered_completion),
            Err(_) => {
                warn!(
                    event_name = "usage_lifecycle_barrier_completion_dropped",
                    log_type = "ops",
                    request_id,
                    fallback = "continue_direct_terminal",
                    "usage lifecycle barrier completion was dropped before success"
                );
                None
            }
        }
    }

    pub async fn record_stream_started_direct<T>(
        &self,
        data: &T,
        seed: &LifecycleUsageSeed,
        status_code: u16,
        telemetry: Option<&ExecutionTelemetry>,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        // Match `record_stream_started`: the caller must not wait for event construction or DB I/O.
        self.record_stream_started(data, seed, status_code, telemetry);
    }

    pub fn record_sync_active_immediate_async<T>(&self, data: &T, seed: LifecycleUsageSeed)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let request_id = seed.request_id.clone();
        self.lifecycle_submission
            .dispatch(Box::new(LifecycleSubmissionItemImpl {
                runtime: self.clone(),
                data: T::clone(data),
                request_id,
                payload: LifecycleSubmissionPayload::Active {
                    seed,
                    observed_at_unix_secs: now_unix_secs(),
                },
            }));
    }

    pub fn record_stream_started_immediate_async<T>(
        &self,
        data: &T,
        seed: LifecycleUsageSeed,
        status_code: u16,
        telemetry: Option<ExecutionTelemetry>,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        self.record_stream_started(data, &seed, status_code, telemetry.as_ref());
    }

    async fn begin_terminal_submission(
        &self,
        request_id: &str,
    ) -> Option<TerminalSubmissionPermit> {
        let permit = self.terminal_submission_state.acquire().await;
        self.report_terminal_submission_rejection(request_id, permit.is_none());
        permit
    }

    async fn begin_registered_terminal_submission(
        &self,
        request_id: &str,
        pending_guard: TerminalSubmissionPendingGuard,
    ) -> Option<TerminalSubmissionPermit> {
        let permit = self
            .terminal_submission_state
            .acquire_registered(pending_guard)
            .await;
        self.report_terminal_submission_rejection(request_id, permit.is_none());
        permit
    }

    fn report_terminal_submission_rejection(&self, request_id: &str, rejected: bool) {
        if !rejected {
            return;
        }
        let rejected_total = self.terminal_submission_state.rejected_total();
        if should_log_usage_retry_counter(rejected_total) {
            warn!(
                event_name = "usage_terminal_submission_rejected",
                log_type = "event",
                request_id,
                submission_limit = self.terminal_submission_state.limit(),
                rejected_total,
                fallback = "drop",
                "usage runtime terminal submission admission closed"
            );
        }
    }

    pub async fn record_sync_terminal<T>(
        &self,
        data: &T,
        context_seed: TerminalUsageContextSeed,
        payload_seed: SyncTerminalUsagePayloadSeed,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let request_id = context_seed.request_id.clone();
        self.dispatch_terminal_seed(
            data,
            request_id,
            LifecycleTerminalUsageSeed::Sync {
                context: context_seed,
                payload: payload_seed,
            },
        )
        .await;
    }

    pub async fn record_stream_terminal<T>(
        &self,
        data: &T,
        context_seed: TerminalUsageContextSeed,
        payload_seed: StreamTerminalUsagePayloadSeed,
        cancelled: bool,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let request_id = context_seed.request_id.clone();
        self.dispatch_terminal_seed(
            data,
            request_id,
            LifecycleTerminalUsageSeed::Stream {
                context: context_seed,
                payload: payload_seed,
                cancelled,
            },
        )
        .await;
    }

    pub async fn submit_terminal_event<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        self.dispatch_terminal(data, event, false, None).await;
    }

    pub async fn record_terminal_event<T>(&self, data: &T, mut event: UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        if !self.is_enabled() {
            return;
        }
        let ordered_completion = self
            .await_lifecycle_submission_turn(&event.request_id)
            .await;
        let Some(_submission_permit) = self.begin_terminal_submission(&event.request_id).await
        else {
            return;
        };
        self.apply_body_capture_policy_from_data(data, &mut event)
            .await;
        let persistence_outcome = self.enqueue_or_write_terminal(data, event).await;
        if persistence_outcome != TerminalPersistenceOutcome::Failed {
            if let Some(completion) = ordered_completion {
                completion.complete();
            }
        }
    }

    pub async fn record_terminal_event_direct<T>(&self, data: &T, mut event: UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        if !self.is_enabled() {
            return;
        }
        let ordered_completion = self
            .await_lifecycle_submission_turn(&event.request_id)
            .await;
        let Some(_submission_permit) = self.begin_terminal_submission(&event.request_id).await
        else {
            return;
        };
        self.apply_body_capture_policy_from_data(data, &mut event)
            .await;
        if let Err(err) = data.enrich_usage_event(&mut event).await {
            warn!(
                event_name = "usage_terminal_billing_enrichment_failed",
                log_type = "event",
                request_id = %event.request_id,
                error = %err,
                "usage runtime failed to enrich terminal usage event with billing"
            );
        }
        let request_id = event.request_id.clone();
        if self.write_event_direct(data, &event).await {
            self.lifecycle_coalescer
                .cancel_delayed_for_queued_terminal(&request_id)
                .await;
            if let Some(completion) = ordered_completion {
                completion.complete();
            }
        }
    }

    async fn apply_body_capture_policy_from_data<T>(&self, data: &T, event: &mut UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        preserve_request_facts(event);
        preserve_provider_response_facts(event);
        match self.cached_body_capture_policy(data).await {
            Ok(policy) => apply_usage_body_capture_policy_to_event(policy, event),
            Err(err) => {
                warn!(
                    event_name = "usage_body_capture_policy_read_failed",
                    log_type = "event",
                    request_id = %event.request_id,
                    fallback = "default",
                    error = %err,
                    "usage runtime failed to read body capture policy; keeping default capture"
                );
                apply_usage_body_capture_policy_to_event(UsageBodyCapturePolicy::default(), event);
            }
        }
    }

    pub async fn body_capture_policy_for<T>(
        &self,
        data: &T,
    ) -> Result<UsageBodyCapturePolicy, DataLayerError>
    where
        T: UsageRuntimeAccess,
    {
        self.cached_body_capture_policy(data).await
    }

    async fn cached_body_capture_policy<T>(
        &self,
        data: &T,
    ) -> Result<UsageBodyCapturePolicy, DataLayerError>
    where
        T: UsageRuntimeAccess,
    {
        let mut cache = self.body_policy_cache.lock().await;
        if let Some(entry) = cache.as_ref() {
            if entry.cached_at.elapsed() <= entry.ttl {
                return match entry.source {
                    UsageBodyCapturePolicyCacheSource::Loaded => Ok(entry.policy),
                    UsageBodyCapturePolicyCacheSource::FallbackAfterError => {
                        Ok(UsageBodyCapturePolicy::default())
                    }
                };
            }
        }
        match data.body_capture_policy().await {
            Ok(policy) => {
                *cache = Some(UsageBodyCapturePolicyCacheEntry::loaded(policy));
                Ok(policy)
            }
            Err(err) => {
                *cache = Some(UsageBodyCapturePolicyCacheEntry::fallback_after_error());
                Err(err)
            }
        }
    }

    async fn enqueue_or_write_terminal<T>(
        &self,
        data: &T,
        event: UsageEvent,
    ) -> TerminalPersistenceOutcome
    where
        T: UsageRuntimeAccess,
    {
        let request_id = event.request_id.clone();
        let outcome = self
            .enqueue_or_write_event(data, event, "terminal", self.config.queue_terminal_events)
            .await;
        // A direct terminal fallback can race the already-dispatched first-byte
        // write. Keep the delayed-terminal marker used by the queue path so an
        // in-flight `pending -> streaming` transition is not cancelled before it
        // reaches the database.
        if matches!(
            outcome,
            TerminalPersistenceOutcome::PersistedDirectly | TerminalPersistenceOutcome::Queued
        ) {
            self.lifecycle_coalescer
                .cancel_delayed_for_queued_terminal(&request_id)
                .await;
        }
        outcome
    }

    async fn enqueue_or_write_lifecycle<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if self.config.queue_lifecycle_events {
            // Keep pre-first-byte churn debounced, but publish the observed first byte as a single
            // background queue event so the live usage row can enter `streaming` without
            // putting Redis or database I/O back on the response-body critical path.
            if event.event_type == crate::UsageEventType::Streaming
                && event.data.first_byte_time_ms.is_some()
            {
                self.enqueue_first_byte_lifecycle_event(data, event).await;
            } else {
                self.enqueue_lifecycle_event_with_config_delay(data, event)
                    .await;
            }
        } else {
            self.write_event_direct(data, &event).await;
        }
    }

    /// Persist a lifecycle phase before this request's later phase is executed.
    ///
    /// A Redis append is not a persistence barrier: a first-byte or terminal write can otherwise
    /// reach the database before the queued pending event is consumed. This ordered handoff runs
    /// on the usage runtime, so callers only enqueue a small seed and never wait on the database.
    async fn persist_ordered_lifecycle_event<T>(
        &self,
        data: &T,
        event: &UsageEvent,
        phase: &'static str,
    ) -> bool
    where
        T: UsageRuntimeAccess,
    {
        let mut attempts = 0_u64;
        loop {
            let write_succeeded = if let Some(gate) = &self.worker_record_gate {
                let _permit = gate.acquire().await;
                self.write_event_direct(data, event).await
            } else {
                self.write_event_direct(data, event).await
            };
            if write_succeeded {
                if attempts > 0 {
                    info!(
                        event_name = "usage_pending_ordered_persistence_recovered",
                        log_type = "ops",
                        request_id = %event.request_id,
                        retry_attempts = attempts,
                        lifecycle_phase = phase,
                        "ordered lifecycle persistence recovered before the next transition"
                    );
                }
                return true;
            }

            attempts = attempts.saturating_add(1);
            if attempts >= ORDERED_INTERMEDIATE_RETRIES_BEFORE_DEGRADE {
                warn!(
                    event_name = "usage_ordered_lifecycle_persistence_degraded",
                    log_type = "ops",
                    request_id = %event.request_id,
                    lifecycle_phase = phase,
                    retry_attempts = attempts,
                    fallback = "drop_intermediate_only",
                    "ordered intermediate persistence exhausted its bounded retry budget"
                );
                return false;
            }
            let delay = usage_enqueue_retry_delay(&self.config, attempts);
            warn!(
                event_name = "usage_ordered_lifecycle_persistence_retry",
                log_type = "ops",
                request_id = %event.request_id,
                lifecycle_phase = phase,
                retry_attempt = attempts,
                retry_delay_ms = delay.as_millis() as u64,
                "ordered lifecycle persistence failed; keeping the request transition behind a retry"
            );
            tokio::time::sleep(delay).await;
        }
    }

    async fn persist_ordered_terminal_event<T>(
        &self,
        data: &T,
        mut event: UsageEvent,
        direct: bool,
    ) -> TerminalPersistenceOutcome
    where
        T: UsageRuntimeAccess,
    {
        if !direct {
            // The normal queue path remains available for queue-only nodes and preserves the
            // existing terminal fallback policy. It executes only after earlier ordered phases.
            return self.enqueue_or_write_terminal(data, event).await;
        }

        if let Err(err) = data.enrich_usage_event(&mut event).await {
            warn!(
                event_name = "usage_terminal_billing_enrichment_failed",
                log_type = "event",
                request_id = %event.request_id,
                error = %err,
                "usage runtime failed to enrich ordered direct terminal usage event"
            );
        }
        let request_id = event.request_id.clone();
        if self.write_event_direct(data, &event).await {
            self.lifecycle_coalescer
                .cancel_delayed_for_queued_terminal(&request_id)
                .await;
            TerminalPersistenceOutcome::PersistedDirectly
        } else {
            TerminalPersistenceOutcome::Failed
        }
    }

    async fn enqueue_first_byte_lifecycle_event<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        self.enqueue_first_byte_lifecycle_event_inner(data, event, false, None)
            .await;
    }

    async fn enqueue_first_byte_lifecycle_event_inner<T>(
        &self,
        data: &T,
        event: UsageEvent,
        wait_for_completion: bool,
        ordered_completion: Option<OrderedLifecycleCompletion>,
    ) where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        let request_id = event.request_id.clone();
        let Some(generation) = self.lifecycle_coalescer.mark_first_byte(&request_id).await else {
            if let Some(ordered_completion) = ordered_completion {
                warn!(
                    event_name = "usage_ordered_first_byte_not_current",
                    log_type = "ops",
                    request_id,
                    fallback = "drop_duplicate_first_byte",
                    "ordered first-byte transition was already superseded; allowing later phases to advance"
                );
                ordered_completion.complete();
            }
            return;
        };
        let mut marker_guard = FirstByteMarkerGuard::new(
            Arc::clone(&self.lifecycle_coalescer),
            request_id.clone(),
            generation,
        );
        let runtime = self.clone();
        let data = T::clone(data);
        if !data.has_usage_writer() || !data.supports_first_byte_usage_fast_path() {
            if let Some(ordered_completion) = ordered_completion {
                if data.has_usage_writer() {
                    let direct_event = make_first_byte_event_lightweight(event);
                    let _ = runtime
                        .persist_ordered_lifecycle_event(&data, &direct_event, "streaming")
                        .await;
                    runtime
                        .lifecycle_coalescer
                        .complete_first_byte(&request_id, generation)
                        .await;
                } else {
                    enqueue_first_byte_fallback(runtime, data, event, request_id, generation).await;
                }
                marker_guard.disarm();
                ordered_completion.complete();
                return;
            }
            enqueue_first_byte_fallback(runtime, data, event, request_id, generation).await;
            marker_guard.disarm();
            return;
        }
        // Keep the complete event for a queue/worker retry. Only the direct database write
        // receives the slim payload so a transient direct-write failure cannot overwrite the
        // pending row's body-capture and audit metadata with a partial snapshot.
        let direct_event = make_first_byte_event_lightweight(event.clone());
        let dispatcher = Arc::clone(&runtime.first_byte_persistence);
        let (completion, completed) = if wait_for_completion {
            let (completion, completed) = tokio::sync::oneshot::channel();
            (Some(completion), Some(completed))
        } else {
            (None, None)
        };
        dispatcher
            .dispatch(Box::new(FirstBytePersistenceItemImpl {
                runtime,
                data,
                direct_event,
                fallback_event: event,
                request_id,
                generation,
                marker_guard,
                completion,
                ordered_completion,
            }))
            .await;
        if let Some(completed) = completed {
            let _ = completed.await;
        }
    }

    async fn enqueue_lifecycle_event_with_config_delay<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        let delay = Duration::from_millis(self.config.lifecycle_enqueue_delay_ms);
        if delay.is_zero() {
            self.enqueue_lifecycle_event(data, event).await;
            return;
        }

        let request_id = event.request_id.clone();
        let Some(generation) = self.lifecycle_coalescer.register(request_id.clone()).await else {
            return;
        };
        let data = T::clone(data);
        if let Err(event) = self.lifecycle_delay.schedule(data, event, generation).await {
            self.lifecycle_coalescer
                .abandon(&request_id, generation)
                .await;
            self.lifecycle_enqueue_state.record_deferred(
                "usage_lifecycle_delay_buffer_deferred",
                "delay_buffer_unavailable",
                event.event_type,
                &event.request_id,
                DeferredEnqueueFallback::Drop,
            );
        }
    }

    async fn enqueue_lifecycle_event<T>(&self, data: &T, event: UsageEvent) -> bool
    where
        T: UsageRuntimeAccess,
    {
        if !self.config.queue_lifecycle_events {
            warn!(
                event_name = "usage_lifecycle_event_not_queued",
                log_type = "event",
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
                fallback = "none",
                "usage runtime lifecycle queue is disabled; lifecycle event will not be written directly"
            );
            return false;
        }
        enqueue_lifecycle_event_now(
            data,
            event,
            self.config.clone(),
            Arc::clone(&self.lifecycle_enqueue_state),
            Arc::clone(&self.enqueue_retry),
        )
        .await
    }

    async fn enqueue_or_write_event<T>(
        &self,
        data: &T,
        mut event: UsageEvent,
        event_phase: &'static str,
        queue_enabled: bool,
    ) -> TerminalPersistenceOutcome
    where
        T: UsageRuntimeAccess,
    {
        if queue_enabled {
            if let Some(runner) = data.usage_worker_queue() {
                match UsageQueue::new(runner, self.config.clone()) {
                    Ok(queue) => {
                        if event_phase == "terminal" {
                            return self
                                .enqueue_terminal_event_or_fallback(data, queue, event)
                                .await;
                        }
                        match queue.enqueue(&event).await {
                            Ok(_) => return TerminalPersistenceOutcome::Queued,
                            Err(err) => {
                                return if self.enqueue_retry.schedule(
                                    queue,
                                    event,
                                    event_phase,
                                    err,
                                ) {
                                    TerminalPersistenceOutcome::BufferedForRetry
                                } else {
                                    TerminalPersistenceOutcome::Failed
                                };
                            }
                        }
                    }
                    Err(err) => {
                        warn!(
                            event_name = "usage_event_queue_init_failed",
                            log_type = "event",
                            event_phase,
                            usage_event_type = ?event.event_type,
                            request_id = %event.request_id,
                            fallback = "direct_write",
                            error = %err,
                            "usage runtime failed to build queue; falling back to direct write"
                        )
                    }
                }
            }
        }

        if event_phase == "terminal" && queue_enabled {
            let usage_event_type = event.event_type;
            let request_id = event.request_id.clone();
            let direct_write_succeeded = self
                .try_write_terminal_direct_fallback(data, &mut event)
                .await;
            let deferred_fallback = if direct_write_succeeded {
                DeferredEnqueueFallback::DirectWrite
            } else {
                DeferredEnqueueFallback::Drop
            };
            self.terminal_enqueue_state.record_deferred(
                "usage_terminal_event_enqueue_deferred",
                "queue_unavailable",
                usage_event_type,
                &request_id,
                deferred_fallback,
            );
            return if direct_write_succeeded {
                TerminalPersistenceOutcome::PersistedDirectly
            } else {
                TerminalPersistenceOutcome::Failed
            };
        }
        if event_phase == "terminal" {
            enrich_terminal_event(data, &mut event).await;
        }
        if self.write_event_direct(data, &event).await {
            TerminalPersistenceOutcome::PersistedDirectly
        } else {
            TerminalPersistenceOutcome::Failed
        }
    }

    async fn enqueue_terminal_event_or_fallback<T>(
        &self,
        data: &T,
        queue: UsageQueue,
        event: UsageEvent,
    ) -> TerminalPersistenceOutcome
    where
        T: UsageRuntimeAccess,
    {
        let now_ms = now_unix_ms();
        if self.terminal_enqueue_state.is_circuit_open(now_ms) {
            return self
                .defer_terminal_event(
                    data,
                    queue,
                    event,
                    "circuit_open",
                    DataLayerError::TimedOut("terminal enqueue circuit is open".to_string()),
                )
                .await;
        }

        let Some(_guard) = self
            .terminal_enqueue_state
            .try_acquire_in_flight(self.config.terminal_enqueue_max_in_flight)
        else {
            return self
                .defer_terminal_event(
                    data,
                    queue,
                    event,
                    "in_flight_limit",
                    DataLayerError::TimedOut("terminal enqueue in-flight limit".to_string()),
                )
                .await;
        };

        if let Err(err) = queue.enqueue(&event).await {
            drop(_guard);
            self.terminal_enqueue_state
                .open_circuit(now_unix_ms().saturating_add(LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS));
            let failures = self.terminal_enqueue_state.increment_failed_total();
            if should_log_usage_retry_counter(failures) {
                warn!(
                    event_name = "usage_terminal_event_enqueue_failed",
                    log_type = "event",
                    usage_event_type = ?event.event_type,
                    request_id = %event.request_id,
                    fallback = "direct_write_or_bounded_retry",
                    failure_total = failures,
                    circuit_open_ms = LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS,
                    error = %err,
                    "usage runtime failed to enqueue terminal event; terminal enqueue circuit opened"
                );
            }
            return self
                .defer_terminal_event(data, queue, event, "primary_enqueue_failed", err)
                .await;
        }
        TerminalPersistenceOutcome::Queued
    }

    async fn defer_terminal_event<T>(
        &self,
        data: &T,
        queue: UsageQueue,
        mut event: UsageEvent,
        reason: &'static str,
        cause: DataLayerError,
    ) -> TerminalPersistenceOutcome
    where
        T: UsageRuntimeAccess,
    {
        let usage_event_type = event.event_type;
        let request_id = event.request_id.clone();
        let direct_write_succeeded = self
            .try_write_terminal_direct_fallback(data, &mut event)
            .await;
        let (deferred_fallback, outcome) = if direct_write_succeeded {
            (
                DeferredEnqueueFallback::DirectWrite,
                TerminalPersistenceOutcome::PersistedDirectly,
            )
        } else if self.enqueue_retry.schedule(queue, event, "terminal", cause) {
            (
                DeferredEnqueueFallback::LocalRetry,
                TerminalPersistenceOutcome::BufferedForRetry,
            )
        } else {
            (
                DeferredEnqueueFallback::Drop,
                TerminalPersistenceOutcome::Failed,
            )
        };
        self.terminal_enqueue_state.record_deferred(
            "usage_terminal_event_enqueue_deferred",
            reason,
            usage_event_type,
            &request_id,
            deferred_fallback,
        );
        outcome
    }

    async fn try_write_terminal_direct_fallback<T>(&self, data: &T, event: &mut UsageEvent) -> bool
    where
        T: UsageRuntimeAccess,
    {
        if !data.has_usage_writer() || data.usage_worker_should_defer_for_database_pressure() {
            let rejected = self.terminal_direct_fallback_state.record_rejected();
            if should_log_usage_retry_counter(rejected) {
                warn!(
                    event_name = "usage_terminal_direct_fallback_rejected",
                    log_type = "event",
                    usage_event_type = ?event.event_type,
                    request_id = %event.request_id,
                    rejected_total = rejected,
                    fallback = "bounded_local_enqueue_retry",
                    "usage runtime skipped terminal direct fallback because the writer is unavailable or under pressure"
                );
            }
            return false;
        }

        let _worker_record_permit = if let Some(gate) = self.worker_record_gate.as_ref() {
            let Some(permit) = gate.try_acquire() else {
                gate.record_deferred();
                let rejected = self.terminal_direct_fallback_state.record_rejected();
                if should_log_usage_retry_counter(rejected) {
                    warn!(
                        event_name = "usage_terminal_direct_fallback_worker_gate_saturated",
                        log_type = "event",
                        usage_event_type = ?event.event_type,
                        request_id = %event.request_id,
                        worker_record_limit = gate.limit(),
                        rejected_total = rejected,
                        fallback = "bounded_local_enqueue_retry",
                        "usage runtime terminal direct fallback was rejected by the shared database concurrency gate"
                    );
                }
                return false;
            };
            Some(permit)
        } else {
            None
        };

        let Some(_permit) = self.terminal_direct_fallback_state.try_acquire() else {
            let rejected = self.terminal_direct_fallback_state.rejected_total();
            if should_log_usage_retry_counter(rejected) {
                warn!(
                    event_name = "usage_terminal_direct_fallback_saturated",
                    log_type = "event",
                    usage_event_type = ?event.event_type,
                    request_id = %event.request_id,
                    fallback_limit = self.terminal_direct_fallback_state.limit(),
                    rejected_total = rejected,
                    fallback = "bounded_local_enqueue_retry",
                    "usage runtime terminal direct fallback is saturated"
                );
            }
            return false;
        };

        let write_succeeded = if let Err(err) = data.enrich_usage_event(event).await {
            warn!(
                event_name = "usage_terminal_direct_fallback_enrichment_failed",
                log_type = "event",
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
                error = %err,
                fallback = "bounded_local_enqueue_retry",
                "usage runtime could not enrich terminal event for direct fallback"
            );
            false
        } else {
            self.write_event_direct(data, event).await
        };
        if write_succeeded {
            let succeeded = self.terminal_direct_fallback_state.record_succeeded();
            if should_log_usage_retry_counter(succeeded) {
                info!(
                    event_name = "usage_terminal_direct_fallback_succeeded",
                    log_type = "event",
                    usage_event_type = ?event.event_type,
                    request_id = %event.request_id,
                    succeeded_total = succeeded,
                    "usage runtime persisted terminal event through bounded direct fallback"
                );
            }
            true
        } else {
            let failed = self.terminal_direct_fallback_state.record_failed();
            if should_log_usage_retry_counter(failed) {
                warn!(
                    event_name = "usage_terminal_direct_fallback_failed",
                    log_type = "event",
                    usage_event_type = ?event.event_type,
                    request_id = %event.request_id,
                    failed_total = failed,
                    fallback = "bounded_local_enqueue_retry",
                    "usage runtime terminal direct fallback failed"
                );
            }
            false
        }
    }

    async fn write_event_direct<T>(&self, data: &T, event: &UsageEvent) -> bool
    where
        T: UsageRuntimeAccess,
    {
        match build_upsert_usage_record_from_event(event) {
            Ok(record) => match catch_usage_writer_panic(
                "direct usage upsert",
                data.upsert_usage_record(record),
            )
            .await
            {
                Ok(Some(stored)) => {
                    if let Err(err) = settle_usage_if_needed(data, &stored).await {
                        warn!(
                            event_name = "usage_terminal_settlement_failed",
                            log_type = "event",
                            request_id = %event.request_id,
                            error = %err,
                            "usage runtime failed to settle terminal usage directly"
                        );
                        return false;
                    }
                    true
                }
                Ok(None) => true,
                Err(err) => {
                    warn!(
                        event_name = "usage_event_upsert_failed",
                        log_type = "event",
                        usage_event_type = ?event.event_type,
                        request_id = %event.request_id,
                        error = %err,
                        "usage runtime failed to upsert usage event directly"
                    );
                    false
                }
            },
            Err(err) => {
                warn!(
                    event_name = "usage_event_upsert_build_failed",
                    log_type = "event",
                    usage_event_type = ?event.event_type,
                    request_id = %event.request_id,
                    error = %err,
                    "usage runtime failed to build usage event upsert"
                );
                false
            }
        }
    }
}

fn preserve_request_facts(event: &mut UsageEvent) {
    let data = &mut event.data;
    match request_body_derived_facts_action(data.request_body.as_ref(), data.request_body_state) {
        RequestBodyDerivedFactsAction::Refresh => {
            data.request_metadata = attach_client_request_body_metadata(
                data.request_metadata.take(),
                data.request_body.as_ref(),
            );
        }
        RequestBodyDerivedFactsAction::Clear => {
            data.request_metadata =
                clear_client_request_body_metadata(data.request_metadata.take());
        }
        RequestBodyDerivedFactsAction::Preserve => {}
    }
    match request_body_derived_facts_action(
        data.provider_request_body.as_ref(),
        data.provider_request_body_state,
    ) {
        RequestBodyDerivedFactsAction::Refresh => {
            data.request_metadata = attach_provider_request_body_metadata(
                data.request_metadata.take(),
                data.endpoint_api_format
                    .as_deref()
                    .or(data.api_format.as_deref()),
                data.target_model.as_deref().or(Some(data.model.as_str())),
                Some(data.model.as_str()),
                data.provider_request_body.as_ref(),
            );
        }
        RequestBodyDerivedFactsAction::Clear => {
            data.request_metadata =
                clear_provider_request_body_metadata(data.request_metadata.take());
        }
        RequestBodyDerivedFactsAction::Preserve => {}
    }
}

fn preserve_provider_response_facts(event: &mut UsageEvent) {
    let metadata = event.data.request_metadata.take();
    event.data.request_metadata =
        attach_provider_response_body_metadata(metadata, event.data.response_body.as_ref());
}

impl UsageQueueHealthSnapshot {
    fn apply_stream_stats(&mut self, stats: RuntimeQueueStats) {
        self.stream_length = stats.stream_length;
        self.group_pending = stats.group_pending;
        self.group_lag = stats.group_lag;
        self.oldest_pending_idle_ms = stats.oldest_pending_idle_ms;
    }
}

async fn enrich_terminal_event<T>(data: &T, event: &mut UsageEvent)
where
    T: UsageBillingEventEnricher + Send + Sync,
{
    if let Err(err) = data.enrich_usage_event(event).await {
        warn!(
            event_name = "usage_terminal_billing_enrichment_failed",
            log_type = "event",
            request_id = %event.request_id,
            error = %err,
            "usage runtime failed to enrich terminal usage event with billing"
        );
    }
}

struct ManagedUsageWorker {
    control: UsageWorkerControl,
    stopping: bool,
}

struct UsageWorkerReconcileInputs<'a, T> {
    runner: &'a Arc<dyn RuntimeQueueStore>,
    data: &'a Arc<T>,
    config: &'a UsageRuntimeConfig,
    worker_record_gate: &'a Option<Arc<UsageWorkerRecordConcurrencyGate>>,
    telemetry_tx: &'a mpsc::Sender<UsageWorkerObservation>,
}

struct UsageWorkerReconcileState<'a> {
    join_set: &'a mut tokio::task::JoinSet<usize>,
    worker_task_indexes: &'a mut BTreeMap<tokio::task::Id, usize>,
    workers: &'a mut BTreeMap<usize, ManagedUsageWorker>,
    next_worker_index: &'a mut usize,
}

async fn run_usage_worker_supervisor<T>(
    runner: Arc<dyn RuntimeQueueStore>,
    data: Arc<T>,
    config: UsageRuntimeConfig,
    worker_record_gate: Option<Arc<UsageWorkerRecordConcurrencyGate>>,
    state: Arc<UsageWorkerSupervisorState>,
) where
    T: UsageRuntimeAccess + 'static,
{
    let min_workers = config.worker_count.max(1);
    let max_workers = if config.worker_autoscale_enabled {
        config.worker_max_count.max(min_workers)
    } else {
        min_workers
    };
    let mut desired_workers = min_workers;
    let mut next_worker_index = 0usize;
    let mut workers = BTreeMap::<usize, ManagedUsageWorker>::new();
    let mut worker_task_indexes = BTreeMap::<tokio::task::Id, usize>::new();
    let mut join_set = tokio::task::JoinSet::<usize>::new();
    let (telemetry_tx, mut telemetry_rx) =
        mpsc::channel::<UsageWorkerObservation>(max_workers.saturating_mul(4).clamp(16, 1024));
    let mut scale_interval = tokio::time::interval(Duration::from_millis(
        config.worker_scale_interval_ms.max(1),
    ));
    scale_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut full_reads = 0usize;
    let mut busy_reads = 0usize;
    let mut idle_reads = 0usize;
    let mut idle_ticks = 0u64;

    state
        .desired_count
        .store(desired_workers, Ordering::Release);
    let reconcile_inputs = UsageWorkerReconcileInputs {
        runner: &runner,
        data: &data,
        config: &config,
        worker_record_gate: &worker_record_gate,
        telemetry_tx: &telemetry_tx,
    };
    reconcile_usage_workers(
        &reconcile_inputs,
        UsageWorkerReconcileState {
            join_set: &mut join_set,
            worker_task_indexes: &mut worker_task_indexes,
            workers: &mut workers,
            next_worker_index: &mut next_worker_index,
        },
        desired_workers,
    );
    state.active_count.store(workers.len(), Ordering::Release);

    loop {
        tokio::select! {
            Some(observation) = telemetry_rx.recv() => {
                state.record_observation(observation);
                if observation.entries_read == 0 {
                    idle_reads = idle_reads.saturating_add(1);
                } else {
                    busy_reads = busy_reads.saturating_add(1);
                    if observation.entries_read >= observation.batch_size {
                        full_reads = full_reads.saturating_add(1);
                    }
                }
            }
            _ = scale_interval.tick() => {
                drain_finished_usage_workers(
                    &mut join_set,
                    &mut worker_task_indexes,
                    &mut workers,
                );
                if config.worker_autoscale_enabled {
                    let active_workers = workers.len().max(1);
                    let high_pressure = full_reads > 0
                        || (busy_reads >= active_workers.saturating_mul(2) && idle_reads == 0);
                    if high_pressure && desired_workers < max_workers {
                        let grow_by = active_workers.div_ceil(2);
                        let next = desired_workers
                            .saturating_add(grow_by.max(1))
                            .clamp(min_workers, max_workers);
                        if next > desired_workers {
                            info!(
                                event_name = "usage_worker_autoscale_up",
                                log_type = "ops",
                                desired_workers = next,
                                previous_desired_workers = desired_workers,
                                active_workers = workers.len(),
                                max_workers,
                                full_reads,
                                busy_reads,
                                idle_reads,
                                "usage worker supervisor scaled up"
                            );
                            desired_workers = next;
                            idle_ticks = 0;
                        }
                    } else if desired_workers > min_workers
                        && busy_reads == 0
                        && full_reads == 0
                        && idle_reads >= active_workers
                    {
                        idle_ticks = idle_ticks.saturating_add(1);
                        if idle_ticks >= config.worker_idle_scale_down_ticks {
                            let next = desired_workers
                                .saturating_sub(desired_workers.div_ceil(2))
                                .max(min_workers);
                            if next < desired_workers {
                                info!(
                                    event_name = "usage_worker_autoscale_down",
                                    log_type = "ops",
                                    desired_workers = next,
                                    previous_desired_workers = desired_workers,
                                    active_workers = workers.len(),
                                    min_workers,
                                    idle_ticks,
                                    "usage worker supervisor scaled down"
                                );
                                desired_workers = next;
                            }
                            idle_ticks = 0;
                        }
                    } else if busy_reads > 0 || full_reads > 0 {
                        idle_ticks = 0;
                    }
                }

                state.desired_count.store(desired_workers, Ordering::Release);
                reconcile_usage_workers(
                    &reconcile_inputs,
                    UsageWorkerReconcileState {
                        join_set: &mut join_set,
                        worker_task_indexes: &mut worker_task_indexes,
                        workers: &mut workers,
                        next_worker_index: &mut next_worker_index,
                    },
                    desired_workers,
                );
                state
                    .active_count
                    .store(workers.len(), Ordering::Release);
                full_reads = 0;
                busy_reads = 0;
                idle_reads = 0;
            }
        }
    }
}

fn reconcile_usage_workers<T>(
    inputs: &UsageWorkerReconcileInputs<'_, T>,
    state: UsageWorkerReconcileState<'_>,
    desired_workers: usize,
) where
    T: UsageRuntimeAccess + 'static,
{
    let UsageWorkerReconcileState {
        join_set,
        worker_task_indexes,
        workers,
        next_worker_index,
    } = state;

    while workers.len() < desired_workers {
        let worker_index = *next_worker_index;
        *next_worker_index = (*next_worker_index).saturating_add(1);
        let control = UsageWorkerControl::default();
        let Ok(worker) = build_usage_queue_worker_with_record_gate(
            Arc::clone(inputs.runner),
            Arc::clone(inputs.data),
            inputs.config.clone(),
            inputs.worker_record_gate.clone(),
            Some(worker_index),
        ) else {
            warn!(
                event_name = "usage_worker_build_failed",
                log_type = "ops",
                worker_index,
                "usage runtime failed to build elastic usage queue worker"
            );
            break;
        };
        let worker = worker.with_supervisor(control.clone(), inputs.telemetry_tx.clone());
        let handle = join_set.spawn(async move {
            worker.run().await;
            worker_index
        });
        worker_task_indexes.insert(handle.id(), worker_index);
        workers.insert(
            worker_index,
            ManagedUsageWorker {
                control,
                stopping: false,
            },
        );
    }

    let mut excess = workers.len().saturating_sub(desired_workers);
    for worker in workers.values_mut().rev() {
        if excess == 0 {
            break;
        }
        if worker.stopping {
            continue;
        }
        worker.control.request_shutdown();
        worker.stopping = true;
        excess -= 1;
    }
}

fn drain_finished_usage_workers(
    join_set: &mut tokio::task::JoinSet<usize>,
    worker_task_indexes: &mut BTreeMap<tokio::task::Id, usize>,
    workers: &mut BTreeMap<usize, ManagedUsageWorker>,
) {
    while let Some(result) = join_set.try_join_next_with_id() {
        match result {
            Ok((task_id, worker_index)) => {
                worker_task_indexes.remove(&task_id);
                let stopping = workers
                    .remove(&worker_index)
                    .is_some_and(|worker| worker.stopping);
                if !stopping {
                    warn!(
                        event_name = "usage_worker_unexpected_exit",
                        log_type = "ops",
                        worker_index,
                        "usage worker exited before supervisor requested shutdown"
                    );
                }
            }
            Err(err) => {
                let worker_index = worker_task_indexes.remove(&err.id());
                if let Some(worker_index) = worker_index {
                    workers.remove(&worker_index);
                    warn!(
                        event_name = "usage_worker_join_failed",
                        log_type = "ops",
                        worker_index,
                        error = %err,
                        "usage worker task failed"
                    );
                    continue;
                }
                warn!(
                    event_name = "usage_worker_join_failed",
                    log_type = "ops",
                    error = %err,
                    "usage worker task failed"
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct UsageBodyCapturePolicyCacheEntry {
    cached_at: Instant,
    ttl: Duration,
    policy: UsageBodyCapturePolicy,
    source: UsageBodyCapturePolicyCacheSource,
}

impl UsageBodyCapturePolicyCacheEntry {
    fn loaded(policy: UsageBodyCapturePolicy) -> Self {
        Self {
            cached_at: Instant::now(),
            ttl: USAGE_BODY_CAPTURE_POLICY_CACHE_TTL,
            policy,
            source: UsageBodyCapturePolicyCacheSource::Loaded,
        }
    }

    fn fallback_after_error() -> Self {
        Self {
            cached_at: Instant::now(),
            ttl: USAGE_BODY_CAPTURE_POLICY_ERROR_CACHE_TTL,
            policy: UsageBodyCapturePolicy::default(),
            source: UsageBodyCapturePolicyCacheSource::FallbackAfterError,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageBodyCapturePolicyCacheSource {
    Loaded,
    FallbackAfterError,
}

#[derive(Debug, Default)]
struct LifecycleEnqueueState {
    in_flight: AtomicU64,
    circuit_open_until_unix_ms: AtomicU64,
    skipped_total: AtomicU64,
    direct_write_total: AtomicU64,
    dropped_total: AtomicU64,
    retry_total: AtomicU64,
    failed_total: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeferredEnqueueFallback {
    DirectWrite,
    LocalRetry,
    Drop,
}

impl LifecycleEnqueueState {
    fn in_flight(&self) -> u64 {
        self.in_flight.load(Ordering::Acquire)
    }

    fn deferred_total(&self) -> u64 {
        self.skipped_total.load(Ordering::Acquire)
    }

    fn deferred_dropped_total(&self) -> u64 {
        self.dropped_total.load(Ordering::Acquire)
    }

    fn deferred_direct_write_total(&self) -> u64 {
        self.direct_write_total.load(Ordering::Acquire)
    }

    fn deferred_retry_total(&self) -> u64 {
        self.retry_total.load(Ordering::Acquire)
    }

    fn failed_total(&self) -> u64 {
        self.failed_total.load(Ordering::Acquire)
    }

    fn increment_failed_total(&self) -> u64 {
        self.failed_total.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn is_circuit_open(&self, now_unix_ms: u64) -> bool {
        self.circuit_open_until_unix_ms.load(Ordering::Acquire) > now_unix_ms
    }

    fn open_circuit(&self, open_until_unix_ms: u64) {
        let mut current = self.circuit_open_until_unix_ms.load(Ordering::Acquire);
        while open_until_unix_ms > current {
            match self.circuit_open_until_unix_ms.compare_exchange(
                current,
                open_until_unix_ms,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
    }

    fn try_acquire_in_flight<'a>(
        &'a self,
        max_in_flight: u64,
    ) -> Option<LifecycleEnqueueInFlightGuard<'a>> {
        let mut current = self.in_flight.load(Ordering::Acquire);
        loop {
            if current >= max_in_flight {
                return None;
            }
            match self.in_flight.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(LifecycleEnqueueInFlightGuard { state: self });
                }
                Err(next) => current = next,
            }
        }
    }

    fn record_deferred(
        &self,
        event_name: &'static str,
        reason: &'static str,
        usage_event_type: crate::UsageEventType,
        request_id: &str,
        deferred_fallback: DeferredEnqueueFallback,
    ) {
        let skipped = self.skipped_total.fetch_add(1, Ordering::AcqRel) + 1;
        let fallback = match deferred_fallback {
            DeferredEnqueueFallback::DirectWrite => {
                self.direct_write_total.fetch_add(1, Ordering::AcqRel);
                "direct_write"
            }
            DeferredEnqueueFallback::LocalRetry => {
                self.retry_total.fetch_add(1, Ordering::AcqRel);
                "local_enqueue_retry"
            }
            DeferredEnqueueFallback::Drop => {
                self.dropped_total.fetch_add(1, Ordering::AcqRel);
                "drop"
            }
        };
        if should_log_usage_retry_counter(skipped) {
            warn!(
                event_name,
                log_type = "event",
                usage_event_type = ?usage_event_type,
                request_id,
                reason,
                deferred_total = skipped,
                fallback,
                "usage runtime deferred usage enqueue"
            );
        }
    }
}

struct LifecycleEnqueueInFlightGuard<'a> {
    state: &'a LifecycleEnqueueState,
}

impl Drop for LifecycleEnqueueInFlightGuard<'_> {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

#[async_trait]
trait DelayedLifecycleEvent: Send {
    async fn enqueue(
        self: Box<Self>,
        config: UsageRuntimeConfig,
        coalescer: Arc<LifecycleEventCoalescer>,
        enqueue_state: Arc<LifecycleEnqueueState>,
        enqueue_retry: Arc<UsageEnqueueRetryDispatcher>,
    );
}

struct DelayedLifecycleEventItem<T> {
    data: T,
    event: UsageEvent,
    generation: u64,
}

#[async_trait]
impl<T> DelayedLifecycleEvent for DelayedLifecycleEventItem<T>
where
    T: UsageRuntimeAccess + Clone + 'static,
{
    async fn enqueue(
        self: Box<Self>,
        config: UsageRuntimeConfig,
        coalescer: Arc<LifecycleEventCoalescer>,
        enqueue_state: Arc<LifecycleEnqueueState>,
        enqueue_retry: Arc<UsageEnqueueRetryDispatcher>,
    ) {
        if !coalescer
            .should_emit(&self.event.request_id, self.generation)
            .await
        {
            return;
        }
        enqueue_lifecycle_event_now(&self.data, self.event, config, enqueue_state, enqueue_retry)
            .await;
    }
}

struct DelayedLifecycleQueueItem {
    due_at: tokio::time::Instant,
    item: Box<dyn DelayedLifecycleEvent>,
    _admission: tokio::sync::OwnedSemaphorePermit,
}

#[derive(Debug)]
struct LifecycleDelayDispatcher {
    delay: Duration,
    admission: Arc<tokio::sync::Semaphore>,
    sender: Option<mpsc::Sender<DelayedLifecycleQueueItem>>,
}

impl LifecycleDelayDispatcher {
    fn disabled() -> Arc<Self> {
        Arc::new(Self {
            delay: Duration::ZERO,
            admission: Arc::new(tokio::sync::Semaphore::new(0)),
            sender: None,
        })
    }

    fn spawn(
        config: UsageRuntimeConfig,
        coalescer: Arc<LifecycleEventCoalescer>,
        enqueue_state: Arc<LifecycleEnqueueState>,
        enqueue_retry: Arc<UsageEnqueueRetryDispatcher>,
    ) -> Arc<Self> {
        if !config.enabled
            || !config.queue_lifecycle_events
            || config.lifecycle_enqueue_delay_ms == 0
        {
            return Self::disabled();
        }

        let capacity = config.enqueue_retry_buffer_capacity.clamp(1, 1_048_576);
        let delay = Duration::from_millis(config.lifecycle_enqueue_delay_ms.max(1));
        let admission = Arc::new(tokio::sync::Semaphore::new(capacity));
        let (sender, receiver) = mpsc::channel(capacity);
        spawn_on_usage_background_runtime(run_lifecycle_delay_worker(
            config,
            coalescer,
            enqueue_state,
            enqueue_retry,
            receiver,
        ));
        Arc::new(Self {
            delay,
            admission,
            sender: Some(sender),
        })
    }

    async fn schedule<T>(
        &self,
        data: T,
        event: UsageEvent,
        generation: u64,
    ) -> Result<(), UsageEvent>
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        let Some(sender) = &self.sender else {
            return Err(event);
        };
        let Ok(admission) = Arc::clone(&self.admission).try_acquire_owned() else {
            return Err(event);
        };
        let Ok(permit) = sender.try_reserve() else {
            return Err(event);
        };
        permit.send(DelayedLifecycleQueueItem {
            due_at: tokio::time::Instant::now() + self.delay,
            item: Box::new(DelayedLifecycleEventItem {
                data,
                event,
                generation,
            }),
            _admission: admission,
        });
        Ok(())
    }
}

async fn run_lifecycle_delay_worker(
    config: UsageRuntimeConfig,
    coalescer: Arc<LifecycleEventCoalescer>,
    enqueue_state: Arc<LifecycleEnqueueState>,
    enqueue_retry: Arc<UsageEnqueueRetryDispatcher>,
    mut receiver: mpsc::Receiver<DelayedLifecycleQueueItem>,
) {
    let mut pending = BTreeMap::<tokio::time::Instant, Vec<DelayedLifecycleQueueItem>>::new();
    let mut receiver_open = true;

    loop {
        if !pending.is_empty() {
            enqueue_due_lifecycle_items(
                &mut pending,
                tokio::time::Instant::now(),
                &config,
                &coalescer,
                &enqueue_state,
                &enqueue_retry,
            )
            .await;
        }

        if pending.is_empty() {
            if !receiver_open {
                break;
            }
            match receiver.recv().await {
                Some(item) => {
                    pending.entry(item.due_at).or_default().push(item);
                    continue;
                }
                None => break,
            }
        }

        let next_due_at = *pending
            .first_key_value()
            .map(|(due_at, _)| due_at)
            .expect("pending lifecycle delay item should exist");

        if receiver_open {
            tokio::select! {
                maybe_item = receiver.recv() => {
                    match maybe_item {
                        Some(item) => {
                            pending.entry(item.due_at).or_default().push(item);
                        }
                        None => {
                            receiver_open = false;
                        }
                    }
                }
                _ = tokio::time::sleep_until(next_due_at) => {
                    enqueue_due_lifecycle_items(
                        &mut pending,
                        tokio::time::Instant::now(),
                        &config,
                        &coalescer,
                        &enqueue_state,
                        &enqueue_retry,
                    ).await;
                }
            }
        } else {
            tokio::time::sleep_until(next_due_at).await;
            enqueue_due_lifecycle_items(
                &mut pending,
                tokio::time::Instant::now(),
                &config,
                &coalescer,
                &enqueue_state,
                &enqueue_retry,
            )
            .await;
        }
    }
}

async fn enqueue_due_lifecycle_items(
    pending: &mut BTreeMap<tokio::time::Instant, Vec<DelayedLifecycleQueueItem>>,
    now: tokio::time::Instant,
    config: &UsageRuntimeConfig,
    coalescer: &Arc<LifecycleEventCoalescer>,
    enqueue_state: &Arc<LifecycleEnqueueState>,
    enqueue_retry: &Arc<UsageEnqueueRetryDispatcher>,
) {
    let mut ready = Vec::new();
    while let Some((&due_at, _)) = pending.first_key_value() {
        if due_at > now {
            break;
        }
        if let Some(mut items) = pending.remove(&due_at) {
            ready.append(&mut items);
        }
    }

    for item in ready {
        item.item
            .enqueue(
                config.clone(),
                Arc::clone(coalescer),
                Arc::clone(enqueue_state),
                Arc::clone(enqueue_retry),
            )
            .await;
    }
}

async fn enqueue_lifecycle_event_now<T>(
    data: &T,
    event: UsageEvent,
    config: UsageRuntimeConfig,
    enqueue_state: Arc<LifecycleEnqueueState>,
    enqueue_retry: Arc<UsageEnqueueRetryDispatcher>,
) -> bool
where
    T: UsageRuntimeAccess,
{
    let Some(runner) = data.usage_worker_queue() else {
        warn!(
            event_name = "usage_lifecycle_event_queue_unavailable",
            log_type = "event",
            usage_event_type = ?event.event_type,
            request_id = %event.request_id,
            fallback = "none",
            "usage runtime lifecycle queue is unavailable; lifecycle event will not be written directly"
        );
        return false;
    };

    let queue = match UsageQueue::new(runner, config.clone()) {
        Ok(queue) => queue,
        Err(err) => {
            warn!(
                event_name = "usage_lifecycle_event_queue_init_failed",
                log_type = "event",
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
                fallback = "none",
                error = %err,
                "usage runtime failed to build lifecycle queue; lifecycle event will not be written directly"
            );
            return false;
        }
    };

    let now_ms = now_unix_ms();
    if enqueue_state.is_circuit_open(now_ms) {
        let retry_enabled = config.retry_deferred_lifecycle_events;
        if retry_enabled {
            let usage_event_type = event.event_type;
            let request_id = event.request_id.clone();
            let accepted = enqueue_retry.schedule(
                queue,
                event,
                "lifecycle",
                DataLayerError::TimedOut("lifecycle enqueue circuit is open".to_string()),
            );
            enqueue_state.record_deferred(
                "usage_lifecycle_event_enqueue_deferred",
                "circuit_open",
                usage_event_type,
                &request_id,
                if accepted {
                    DeferredEnqueueFallback::LocalRetry
                } else {
                    DeferredEnqueueFallback::Drop
                },
            );
            return accepted;
        }
        enqueue_state.record_deferred(
            "usage_lifecycle_event_enqueue_deferred",
            "circuit_open",
            event.event_type,
            &event.request_id,
            DeferredEnqueueFallback::Drop,
        );
        return false;
    }

    let Some(_guard) = enqueue_state.try_acquire_in_flight(config.lifecycle_enqueue_max_in_flight)
    else {
        let retry_enabled = config.retry_deferred_lifecycle_events;
        if retry_enabled {
            let usage_event_type = event.event_type;
            let request_id = event.request_id.clone();
            let accepted = enqueue_retry.schedule(
                queue,
                event,
                "lifecycle",
                DataLayerError::TimedOut("lifecycle enqueue in-flight limit".to_string()),
            );
            enqueue_state.record_deferred(
                "usage_lifecycle_event_enqueue_deferred",
                "in_flight_limit",
                usage_event_type,
                &request_id,
                if accepted {
                    DeferredEnqueueFallback::LocalRetry
                } else {
                    DeferredEnqueueFallback::Drop
                },
            );
            return accepted;
        }
        enqueue_state.record_deferred(
            "usage_lifecycle_event_enqueue_deferred",
            "in_flight_limit",
            event.event_type,
            &event.request_id,
            DeferredEnqueueFallback::Drop,
        );
        return false;
    };

    let Err(err) = queue.enqueue(&event).await else {
        return true;
    };

    enqueue_state.open_circuit(now_unix_ms().saturating_add(LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS));
    let failures = enqueue_state.increment_failed_total();
    let retry_enabled = config.retry_deferred_lifecycle_events;
    if should_log_usage_retry_counter(failures) {
        warn!(
            event_name = "usage_lifecycle_event_enqueue_failed",
            log_type = "event",
            usage_event_type = ?event.event_type,
            request_id = %event.request_id,
            fallback = if retry_enabled { "local_enqueue_retry" } else { "none" },
            failure_total = failures,
            circuit_open_ms = LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS,
            error = %err,
            "usage runtime failed to enqueue lifecycle event; lifecycle enqueue circuit opened"
        );
    }
    let usage_event_type = event.event_type;
    let request_id = event.request_id.clone();
    let accepted = retry_enabled && enqueue_retry.schedule(queue, event, "lifecycle", err);
    enqueue_state.record_deferred(
        "usage_lifecycle_event_enqueue_deferred",
        "primary_enqueue_failed",
        usage_event_type,
        &request_id,
        if accepted {
            DeferredEnqueueFallback::LocalRetry
        } else {
            DeferredEnqueueFallback::Drop
        },
    );
    accepted
}

#[derive(Debug)]
struct UsageEnqueueRetryDispatcher {
    senders: Vec<mpsc::Sender<UsageEnqueueRetryItem>>,
    metrics: Arc<UsageEnqueueDispatcherMetrics>,
}

#[derive(Debug, Default)]
struct UsageEnqueueDispatcherMetrics {
    scheduled_total: AtomicU64,
    recovered_total: AtomicU64,
    pending: AtomicU64,
    retry_failed_total: AtomicU64,
    closed_or_unavailable_total: AtomicU64,
}

struct UsageEnqueueRetryItem {
    queue: UsageQueue,
    event: UsageEvent,
    event_phase: &'static str,
    attempts: u64,
    delay_before_first_attempt: bool,
}

impl UsageEnqueueRetryDispatcher {
    fn disabled() -> Arc<Self> {
        Arc::new(Self {
            senders: Vec::new(),
            metrics: Arc::new(UsageEnqueueDispatcherMetrics::default()),
        })
    }

    fn spawn(config: UsageRuntimeConfig) -> Arc<Self> {
        let lifecycle_retry_enabled =
            config.queue_lifecycle_events && config.retry_deferred_lifecycle_events;
        if !config.enabled || !(config.queue_terminal_events || lifecycle_retry_enabled) {
            return Self::disabled();
        }

        let workers = config
            .enqueue_retry_workers
            .min(config.enqueue_retry_buffer_capacity)
            .max(1);
        let mut senders = Vec::with_capacity(workers);
        let metrics = Arc::new(UsageEnqueueDispatcherMetrics::default());
        for worker_index in 0..workers {
            let capacity =
                retry_worker_capacity(config.enqueue_retry_buffer_capacity, workers, worker_index);
            let (sender, receiver) = mpsc::channel(capacity);
            senders.push(sender);
            let worker_config = config.clone();
            let worker_metrics = Arc::clone(&metrics);
            spawn_on_usage_background_runtime(async move {
                run_usage_enqueue_retry_worker(
                    worker_index,
                    worker_config,
                    receiver,
                    worker_metrics,
                )
                .await;
            });
        }

        Arc::new(Self { senders, metrics })
    }

    fn schedule(
        &self,
        queue: UsageQueue,
        event: UsageEvent,
        event_phase: &'static str,
        cause: DataLayerError,
    ) -> bool {
        let event_type = event.event_type;
        let request_id = event.request_id.clone();
        let cause_message = cause.to_string();
        let scheduled = self.schedule_item(queue, event, event_phase, Some(cause_message.as_str()));
        if let Some(scheduled) = scheduled {
            if should_log_usage_retry_counter(scheduled) {
                warn!(
                    event_name = "usage_event_enqueue_failed_retry_scheduled",
                    log_type = "event",
                    event_phase,
                    usage_event_type = ?event_type,
                    request_id,
                    retry_scheduled_total = scheduled,
                    fallback = "local_enqueue_retry",
                    error = %cause_message,
                    "usage runtime failed to enqueue usage event; scheduled local retry"
                );
            }
            true
        } else {
            false
        }
    }

    fn schedule_item(
        &self,
        queue: UsageQueue,
        event: UsageEvent,
        event_phase: &'static str,
        cause: Option<&str>,
    ) -> Option<u64> {
        let worker_index = retry_worker_index(&event.request_id, self.senders.len());
        let Some(sender) = self.senders.get(worker_index) else {
            self.metrics.record_closed_or_unavailable();
            warn!(
                event_name = "usage_event_enqueue_retry_unavailable",
                log_type = "event",
                event_phase,
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
                error = ?cause,
                fallback = "drop",
                "usage runtime local enqueue retry dispatcher is unavailable"
            );
            return None;
        };

        match sender.try_reserve() {
            Ok(permit) => {
                let scheduled = self.metrics.record_scheduled();
                permit.send(UsageEnqueueRetryItem {
                    queue,
                    event,
                    event_phase,
                    attempts: 0,
                    delay_before_first_attempt: true,
                });
                Some(scheduled)
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                let dropped = self.metrics.record_closed_or_unavailable();
                if should_log_usage_retry_counter(dropped) {
                    warn!(
                        event_name = "usage_event_enqueue_retry_buffer_full",
                        log_type = "event",
                        event_phase,
                        usage_event_type = ?event.event_type,
                        request_id = %event.request_id,
                        worker_index,
                        retry_dropped_total = dropped,
                        fallback = "drop",
                        "usage runtime local enqueue retry buffer is full; dropped usage event"
                    );
                }
                None
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.metrics.record_closed_or_unavailable();
                warn!(
                    event_name = "usage_event_enqueue_retry_closed",
                    log_type = "event",
                    event_phase,
                    usage_event_type = ?event.event_type,
                    request_id = %event.request_id,
                    worker_index,
                    fallback = "drop",
                    "usage runtime local enqueue retry dispatcher is closed"
                );
                None
            }
        }
    }

    fn scheduled_total(&self) -> u64 {
        self.metrics.scheduled_total.load(Ordering::Acquire)
    }

    fn recovered_total(&self) -> u64 {
        self.metrics.recovered_total.load(Ordering::Acquire)
    }

    fn pending(&self) -> u64 {
        self.metrics.pending.load(Ordering::Acquire)
    }

    fn retry_failed_total(&self) -> u64 {
        self.metrics.retry_failed_total.load(Ordering::Acquire)
    }

    fn closed_or_unavailable_total(&self) -> u64 {
        self.metrics
            .closed_or_unavailable_total
            .load(Ordering::Acquire)
    }
}

impl UsageEnqueueDispatcherMetrics {
    fn record_scheduled(&self) -> u64 {
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.scheduled_total.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn record_recovered(&self) -> u64 {
        let recovered = self.recovered_total.fetch_add(1, Ordering::AcqRel) + 1;
        self.pending.fetch_sub(1, Ordering::AcqRel);
        recovered
    }

    fn record_retry_failed(&self) {
        self.retry_failed_total.fetch_add(1, Ordering::AcqRel);
    }

    fn record_closed_or_unavailable(&self) -> u64 {
        self.closed_or_unavailable_total
            .fetch_add(1, Ordering::AcqRel)
            + 1
    }
}

async fn run_usage_enqueue_retry_worker(
    worker_index: usize,
    config: UsageRuntimeConfig,
    mut receiver: mpsc::Receiver<UsageEnqueueRetryItem>,
    metrics: Arc<UsageEnqueueDispatcherMetrics>,
) {
    let mut initial_retry_delay_applied = false;
    while let Some(mut item) = receiver.recv().await {
        if item.delay_before_first_attempt && !initial_retry_delay_applied {
            initial_retry_delay_applied = true;
            tokio::time::sleep(usage_enqueue_retry_delay(&config, 1)).await;
        }
        loop {
            match item.queue.enqueue(&item.event).await {
                Ok(_) => {
                    let recovered = metrics.record_recovered();
                    if item.attempts > 0 && should_log_usage_retry_counter(recovered) {
                        warn!(
                            event_name = "usage_event_enqueue_retry_recovered",
                            log_type = "event",
                            event_phase = item.event_phase,
                            usage_event_type = ?item.event.event_type,
                            request_id = %item.event.request_id,
                            worker_index,
                            retry_attempts = item.attempts,
                            retry_recovered_total = recovered,
                            "usage runtime local enqueue retry recovered"
                        );
                    }
                    break;
                }
                Err(err) => {
                    item.attempts = item.attempts.saturating_add(1);
                    metrics.record_retry_failed();
                    let delay = usage_enqueue_retry_delay(&config, item.attempts);
                    if should_log_usage_retry_counter(item.attempts) {
                        warn!(
                            event_name = "usage_event_enqueue_retry_failed",
                            log_type = "event",
                            event_phase = item.event_phase,
                            usage_event_type = ?item.event.event_type,
                            request_id = %item.event.request_id,
                            worker_index,
                            retry_attempt = item.attempts,
                            retry_delay_ms = delay.as_millis() as u64,
                            error = %err,
                            "usage runtime local enqueue retry failed; will retry"
                        );
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

fn usage_enqueue_retry_delay(config: &UsageRuntimeConfig, attempts: u64) -> Duration {
    let exponent = attempts.saturating_sub(1).min(16);
    let multiplier = 1_u64.checked_shl(exponent as u32).unwrap_or(u64::MAX);
    let delay_ms = config
        .enqueue_retry_initial_backoff_ms
        .saturating_mul(multiplier)
        .min(config.enqueue_retry_max_backoff_ms);
    Duration::from_millis(delay_ms.max(1))
}

async fn catch_usage_writer_panic<T, F>(
    operation: &'static str,
    future: F,
) -> Result<T, DataLayerError>
where
    F: Future<Output = Result<T, DataLayerError>>,
{
    match AssertUnwindSafe(future).catch_unwind().await {
        Ok(result) => result,
        Err(_) => Err(DataLayerError::UnexpectedValue(format!(
            "{operation} panicked"
        ))),
    }
}

fn retry_worker_capacity(total_capacity: usize, workers: usize, worker_index: usize) -> usize {
    let workers = workers.max(1);
    let base = total_capacity / workers;
    let remainder = total_capacity % workers;
    (base + usize::from(worker_index < remainder)).max(1)
}

fn retry_worker_index(request_id: &str, worker_count: usize) -> usize {
    if worker_count <= 1 {
        return 0;
    }
    (fnv_hash(request_id.as_bytes()) % worker_count as u64) as usize
}

fn fnv_hash(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 14_695_981_039_346_656_037;
    const FNV_PRIME: u64 = 1_099_511_628_211;

    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn should_log_usage_retry_counter(value: u64) -> bool {
    value <= 8 || value.is_power_of_two() || value.is_multiple_of(1_000)
}

async fn build_sync_terminal_usage_event_offthread(
    context_seed: TerminalUsageContextSeed,
    payload_seed: SyncTerminalUsagePayloadSeed,
) -> Result<UsageEvent, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        build_terminal_usage_event_from_seed(build_sync_terminal_usage_seed(
            context_seed,
            payload_seed,
        ))
    })
    .await
    .map_err(join_error_to_data_layer)?
}

async fn build_stream_terminal_usage_event_offthread(
    context_seed: TerminalUsageContextSeed,
    payload_seed: StreamTerminalUsagePayloadSeed,
    cancelled: bool,
) -> Result<UsageEvent, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        build_terminal_usage_event_from_seed(build_stream_terminal_usage_seed(
            context_seed,
            payload_seed,
            cancelled,
        ))
    })
    .await
    .map_err(join_error_to_data_layer)?
}

fn join_error_to_data_layer(err: tokio::task::JoinError) -> DataLayerError {
    DataLayerError::UnexpectedValue(format!("usage builder task join failed: {err}"))
}

fn now_unix_secs() -> u64 {
    now_unix_ms() / 1_000
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use aether_contracts::{ExecutionPlan, ExecutionTelemetry, RequestBody};
    use aether_data_contracts::repository::settlement::{
        StoredUsageSettlement, UsageSettlementInput,
    };
    use aether_data_contracts::repository::usage::{
        StoredRequestUsageAudit, UpsertUsageRecord, UsageBodyCaptureState,
    };
    use aether_data_contracts::DataLayerError;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeQueueStore, RuntimeState};
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio::time::{sleep, timeout, Duration};

    use super::{
        preserve_provider_response_facts, preserve_request_facts, LifecycleAdmissionPermit,
        LifecycleEventCoalescer, LifecycleSubmissionDispatcher, LifecycleSubmissionItem,
        LifecycleSubmissionPriority, LifecycleTerminalUsageSeed, UsageBillingEventEnricher,
        UsageBodyCapturePolicy, UsageEnqueueRetryDispatcher, UsageRequestRecordLevel,
        UsageRuntimeAccess, UsageWorkerObservation, UsageWorkerSupervisorState,
    };
    use crate::worker::ManualProxyNodeCounter;
    use crate::{
        apply_usage_body_capture_policy_to_event, build_lifecycle_usage_seed,
        build_terminal_usage_context_seed, SyncTerminalUsagePayloadSeed, TerminalUsageContextSeed,
        UsageEvent, UsageEventData, UsageEventType, UsageQueue, UsageRecordWriter, UsageRuntime,
        UsageRuntimeConfig, UsageSettlementWriter,
    };

    fn terminal_test_plan(request_id: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: request_id.to_string(),
            candidate_id: Some(format!("candidate-{request_id}")),
            provider_name: Some("openai".to_string()),
            provider_id: "provider-terminal-test".to_string(),
            endpoint_id: "endpoint-terminal-test".to_string(),
            key_id: "key-terminal-test".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn sync_terminal_test_seeds(
        request_id: &str,
    ) -> (TerminalUsageContextSeed, SyncTerminalUsagePayloadSeed) {
        let plan = terminal_test_plan(request_id);
        (
            build_terminal_usage_context_seed(&plan, None),
            SyncTerminalUsagePayloadSeed {
                report_kind: "sync_completed".to_string(),
                status_code: 200,
                response_time_ms: Some(12),
                first_byte_time_ms: None,
                provider_response_headers: None,
                client_response_headers: None,
                provider_response_full: Some(json!({"id": request_id})),
                provider_response_body_state: None,
                client_response: Some(json!({"id": request_id})),
                client_response_body_state: None,
                standardized_usage: None,
                capture_metadata: None,
            },
        )
    }

    struct TestLifecycleSubmissionItem {
        request_id: String,
        priority: LifecycleSubmissionPriority,
        started: Option<Arc<tokio::sync::Notify>>,
        release: Option<Arc<tokio::sync::Notify>>,
        seen: Arc<Mutex<Vec<LifecycleSubmissionPriority>>>,
    }

    struct PanickingLifecycleSubmissionItem {
        request_id: String,
    }

    #[async_trait]
    impl LifecycleSubmissionItem for PanickingLifecycleSubmissionItem {
        fn request_id(&self) -> &str {
            &self.request_id
        }

        fn priority(&self) -> LifecycleSubmissionPriority {
            LifecycleSubmissionPriority::Pending
        }

        async fn execute(self: Box<Self>, _admission: LifecycleAdmissionPermit) {
            panic!("forced lifecycle submission panic");
        }
    }

    struct PanickingTerminalExecutionItem {
        request_id: String,
    }

    #[async_trait]
    impl super::TerminalExecutionItem for PanickingTerminalExecutionItem {
        fn request_id(&self) -> &str {
            &self.request_id
        }

        async fn execute(self: Box<Self>) {
            panic!("forced terminal execution panic");
        }
    }

    struct CompletingTerminalExecutionItem {
        request_id: String,
        completion: tokio::sync::oneshot::Sender<()>,
    }

    #[async_trait]
    impl super::TerminalExecutionItem for CompletingTerminalExecutionItem {
        fn request_id(&self) -> &str {
            &self.request_id
        }

        async fn execute(self: Box<Self>) {
            let _ = self.completion.send(());
        }
    }

    struct PanickingFirstBytePersistenceItem {
        _marker_guard: super::FirstByteMarkerGuard,
        record: UpsertUsageRecord,
    }

    #[async_trait]
    impl super::FirstBytePersistenceItem for PanickingFirstBytePersistenceItem {
        fn batch_identity(&self) -> Option<usize> {
            panic!("forced first-byte identity panic");
        }

        async fn is_current(&self) -> bool {
            true
        }

        fn build_record(&self) -> Result<UpsertUsageRecord, DataLayerError> {
            Ok(self.record.clone())
        }

        async fn write_batch(
            &self,
            _records: Vec<UpsertUsageRecord>,
        ) -> Result<(), DataLayerError> {
            unreachable!("identity panic should precede the write")
        }

        async fn complete_success(self: Box<Self>) {
            unreachable!("identity panic should precede completion")
        }

        async fn complete_cancelled(self: Box<Self>) {
            unreachable!("test item is always current")
        }

        async fn enqueue_fallback(self: Box<Self>) {
            unreachable!("identity panic should precede fallback")
        }
    }

    #[async_trait]
    impl LifecycleSubmissionItem for TestLifecycleSubmissionItem {
        fn request_id(&self) -> &str {
            &self.request_id
        }

        fn priority(&self) -> LifecycleSubmissionPriority {
            self.priority
        }

        async fn execute(self: Box<Self>, _admission: LifecycleAdmissionPermit) {
            if let Some(started) = self.started {
                started.notify_one();
            }
            if let Some(release) = self.release {
                release.notified().await;
            }
            self.seen
                .lock()
                .expect("lifecycle test seen lock")
                .push(self.priority);
        }
    }

    #[derive(Default)]
    struct NoRedisUsageStore {
        records: Mutex<Vec<UpsertUsageRecord>>,
    }

    struct QueueConfiguredUsageStore {
        inner: NoRedisUsageStore,
        queue: Arc<dyn RuntimeQueueStore>,
    }

    #[derive(Clone)]
    struct CloneQueueConfiguredUsageStore {
        records: Arc<Mutex<Vec<UpsertUsageRecord>>>,
        queue: Arc<dyn RuntimeQueueStore>,
    }

    #[derive(Clone)]
    struct BatchingQueueUsageStore {
        records: Arc<Mutex<Vec<UpsertUsageRecord>>>,
        queue: Arc<dyn RuntimeQueueStore>,
    }

    #[derive(Clone)]
    struct OrderedBatchUsageStore {
        records: Arc<Mutex<Vec<UpsertUsageRecord>>>,
        pending_batch_sizes: Arc<Mutex<Vec<usize>>>,
        pending_attempts: Arc<Mutex<BTreeMap<String, usize>>>,
        permanent_pending_failure_request_id: Option<Arc<str>>,
        remaining_pending_failures: Arc<AtomicUsize>,
        remaining_pending_panics: Arc<AtomicUsize>,
        remaining_first_byte_panics: Arc<AtomicUsize>,
        queue: Arc<dyn RuntimeQueueStore>,
    }

    #[derive(Clone)]
    struct BlockingNonBatchPendingStore {
        release_writes: Arc<tokio::sync::Semaphore>,
        writes_in_flight: Arc<AtomicUsize>,
        max_writes_in_flight: Arc<AtomicUsize>,
        writes_completed: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct AdmissionBlockedUsageStore {
        release_writes: Arc<tokio::sync::Semaphore>,
        writes_in_flight: Arc<AtomicUsize>,
        max_writes_in_flight: Arc<AtomicUsize>,
        writes_completed: Arc<AtomicUsize>,
    }

    struct BlockingNonBatchPendingItem {
        request_id: String,
        record: UpsertUsageRecord,
        store: BlockingNonBatchPendingStore,
        record_gate: Arc<super::UsageWorkerRecordConcurrencyGate>,
        completed: Arc<AtomicUsize>,
        degraded: Arc<AtomicUsize>,
    }

    struct PanicOnceQueueConfiguredUsageStore {
        inner: CloneQueueConfiguredUsageStore,
        remaining_panics: AtomicUsize,
    }

    #[derive(Clone)]
    struct PanicOncePolicyQueueConfiguredUsageStore {
        inner: CloneQueueConfiguredUsageStore,
        remaining_policy_panics: Arc<AtomicUsize>,
        policy_reads: Arc<AtomicUsize>,
    }

    struct EnrichmentCountingQueueStore {
        records: Mutex<Vec<UpsertUsageRecord>>,
        queue: Arc<dyn RuntimeQueueStore>,
        enrich_calls: AtomicUsize,
    }

    #[derive(Clone)]
    struct FailingWriteQueueConfiguredUsageStore {
        queue: Arc<dyn RuntimeQueueStore>,
        upsert_attempts: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct QueueOnlyUsageStore {
        queue: Arc<dyn RuntimeQueueStore>,
        upsert_attempts: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct BlockingWriteQueueConfiguredUsageStore {
        records: Arc<Mutex<Vec<UpsertUsageRecord>>>,
        queue: Arc<dyn RuntimeQueueStore>,
        write_started: Arc<tokio::sync::Notify>,
        release_writes: Arc<tokio::sync::Notify>,
        writes_completed: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    struct BlockingPolicyQueueConfiguredUsageStore {
        queue: Arc<dyn RuntimeQueueStore>,
        policy_started: Arc<tokio::sync::Notify>,
        release_policy: Arc<tokio::sync::Notify>,
        policy_reads: Arc<AtomicUsize>,
    }

    #[derive(Default)]
    struct FailingPolicyUsageStore {
        inner: NoRedisUsageStore,
        policy_reads: AtomicUsize,
    }

    struct FlakyAppendQueueStore {
        inner: Arc<dyn RuntimeQueueStore>,
        remaining_failures: AtomicUsize,
        append_attempts: AtomicUsize,
        successful_appends: AtomicUsize,
        active_appends: AtomicUsize,
        max_active_appends: AtomicUsize,
        append_delay_ms: u64,
    }

    impl FlakyAppendQueueStore {
        fn new(inner: Arc<dyn RuntimeQueueStore>, remaining_failures: usize) -> Self {
            Self {
                inner,
                remaining_failures: AtomicUsize::new(remaining_failures),
                append_attempts: AtomicUsize::new(0),
                successful_appends: AtomicUsize::new(0),
                active_appends: AtomicUsize::new(0),
                max_active_appends: AtomicUsize::new(0),
                append_delay_ms: 0,
            }
        }

        fn with_append_delay_ms(mut self, append_delay_ms: u64) -> Self {
            self.append_delay_ms = append_delay_ms;
            self
        }
    }

    async fn wait_for_enqueue_dispatcher_to_drain(runtime: &UsageRuntime, expected_recovered: u64) {
        timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if snapshot.enqueue_retry_recovered_total >= expected_recovered
                    && snapshot.enqueue_retry_pending == 0
                {
                    return;
                }
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("local usage enqueue dispatcher should drain");
    }

    #[async_trait]
    impl UsageRecordWriter for NoRedisUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.records.lock().expect("records lock").push(record);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for NoRedisUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for NoRedisUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for NoRedisUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for NoRedisUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            false
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            None
        }
    }

    #[async_trait]
    impl UsageRecordWriter for QueueConfiguredUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.inner.upsert_usage_record(record).await
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for QueueConfiguredUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for QueueConfiguredUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for QueueConfiguredUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for QueueConfiguredUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }
    }

    #[async_trait]
    impl UsageRecordWriter for CloneQueueConfiguredUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.records.lock().expect("records lock").push(record);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageRecordWriter for BlockingNonBatchPendingStore {
        async fn upsert_usage_record(
            &self,
            _record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            let active = self.writes_in_flight.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_writes_in_flight
                .fetch_max(active, Ordering::AcqRel);
            let permit = self
                .release_writes
                .acquire()
                .await
                .expect("blocking pending store semaphore should remain open");
            permit.forget();
            self.writes_in_flight.fetch_sub(1, Ordering::AcqRel);
            self.writes_completed.fetch_add(1, Ordering::AcqRel);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageRecordWriter for AdmissionBlockedUsageStore {
        async fn upsert_usage_record(
            &self,
            _record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            let active = self.writes_in_flight.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_writes_in_flight
                .fetch_max(active, Ordering::AcqRel);
            let permit = self
                .release_writes
                .acquire()
                .await
                .expect("admission test semaphore should remain open");
            permit.forget();
            self.writes_in_flight.fetch_sub(1, Ordering::AcqRel);
            self.writes_completed.fetch_add(1, Ordering::AcqRel);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for AdmissionBlockedUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for AdmissionBlockedUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for AdmissionBlockedUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn supports_first_byte_usage_fast_path(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            false
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            None
        }
    }

    #[async_trait]
    impl super::PendingPersistenceItem for BlockingNonBatchPendingItem {
        fn request_id(&self) -> &str {
            &self.request_id
        }

        fn batch_identity(&self) -> Option<usize> {
            None
        }

        fn build_record(&self) -> Result<UpsertUsageRecord, DataLayerError> {
            Ok(self.record.clone())
        }

        fn retry_delay(&self, _attempts: u64) -> Duration {
            Duration::from_millis(1)
        }

        async fn write_batch(&self, records: Vec<UpsertUsageRecord>) -> Result<(), DataLayerError> {
            let _permit = self.record_gate.acquire().await;
            self.store.upsert_pending_usage_records(records).await
        }

        async fn complete_success(self: Box<Self>) {
            self.completed.fetch_add(1, Ordering::AcqRel);
        }

        fn complete_degraded(self: Box<Self>) {
            self.degraded.fetch_add(1, Ordering::AcqRel);
        }
    }

    #[async_trait]
    impl UsageRecordWriter for BatchingQueueUsageStore {
        fn supports_first_byte_usage_batch(&self) -> bool {
            true
        }

        fn first_byte_usage_writer_identity(&self) -> Option<usize> {
            Some(Arc::as_ptr(&self.records) as usize)
        }

        fn supports_pending_usage_batch(&self) -> bool {
            true
        }

        fn pending_usage_writer_identity(&self) -> Option<usize> {
            Some(Arc::as_ptr(&self.records) as usize)
        }

        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.records.lock().expect("records lock").push(record);
            Ok(None)
        }

        async fn upsert_first_byte_usage_records(
            &self,
            records: Vec<UpsertUsageRecord>,
        ) -> Result<(), DataLayerError> {
            self.records.lock().expect("records lock").extend(records);
            Ok(())
        }

        async fn upsert_pending_usage_records(
            &self,
            records: Vec<UpsertUsageRecord>,
        ) -> Result<(), DataLayerError> {
            self.records.lock().expect("records lock").extend(records);
            Ok(())
        }
    }

    #[async_trait]
    impl UsageRecordWriter for OrderedBatchUsageStore {
        fn supports_first_byte_usage_batch(&self) -> bool {
            true
        }

        fn first_byte_usage_writer_identity(&self) -> Option<usize> {
            Some(Arc::as_ptr(&self.records) as usize)
        }

        fn supports_pending_usage_batch(&self) -> bool {
            true
        }

        fn pending_usage_writer_identity(&self) -> Option<usize> {
            Some(Arc::as_ptr(&self.records) as usize)
        }

        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.records.lock().expect("records lock").push(record);
            Ok(None)
        }

        async fn upsert_first_byte_usage_records(
            &self,
            records: Vec<UpsertUsageRecord>,
        ) -> Result<(), DataLayerError> {
            if self
                .remaining_first_byte_panics
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    (remaining > 0).then(|| remaining - 1)
                })
                .is_ok()
            {
                panic!("forced first-byte batch panic");
            }
            self.records.lock().expect("records lock").extend(records);
            Ok(())
        }

        async fn upsert_pending_usage_records(
            &self,
            records: Vec<UpsertUsageRecord>,
        ) -> Result<(), DataLayerError> {
            self.pending_batch_sizes
                .lock()
                .expect("pending batch sizes lock")
                .push(records.len());
            {
                let mut attempts = self.pending_attempts.lock().expect("pending attempts lock");
                for record in &records {
                    *attempts.entry(record.request_id.clone()).or_default() += 1;
                }
            }
            if self
                .permanent_pending_failure_request_id
                .as_deref()
                .is_some_and(|request_id| {
                    records.iter().any(|record| record.request_id == request_id)
                })
            {
                return Err(DataLayerError::Postgres(
                    "forced permanent pending batch failure".to_string(),
                ));
            }
            if self
                .remaining_pending_panics
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    (remaining > 0).then(|| remaining - 1)
                })
                .is_ok()
            {
                panic!("forced pending batch panic");
            }
            if self
                .remaining_pending_failures
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    (remaining > 0).then(|| remaining - 1)
                })
                .is_ok()
            {
                return Err(DataLayerError::Postgres(
                    "forced pending batch failure".to_string(),
                ));
            }
            self.records.lock().expect("records lock").extend(records);
            Ok(())
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for OrderedBatchUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for OrderedBatchUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for OrderedBatchUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn supports_first_byte_usage_fast_path(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for BatchingQueueUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for BatchingQueueUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for BatchingQueueUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn supports_first_byte_usage_fast_path(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for CloneQueueConfiguredUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for CloneQueueConfiguredUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for CloneQueueConfiguredUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for CloneQueueConfiguredUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn supports_first_byte_usage_fast_path(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }
    }

    #[async_trait]
    impl UsageRecordWriter for PanicOncePolicyQueueConfiguredUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.inner.upsert_usage_record(record).await
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for PanicOncePolicyQueueConfiguredUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for PanicOncePolicyQueueConfiguredUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl UsageRuntimeAccess for PanicOncePolicyQueueConfiguredUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.inner.queue))
        }

        async fn body_capture_policy(&self) -> Result<UsageBodyCapturePolicy, DataLayerError> {
            self.policy_reads.fetch_add(1, Ordering::AcqRel);
            if self
                .remaining_policy_panics
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                    (remaining > 0).then(|| remaining - 1)
                })
                .is_ok()
            {
                panic!("forced body capture policy panic");
            }
            Ok(UsageBodyCapturePolicy::default())
        }
    }

    #[async_trait]
    impl UsageRecordWriter for PanicOnceQueueConfiguredUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            let should_panic = self
                .remaining_panics
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    (current > 0).then(|| current - 1)
                })
                .is_ok();
            if should_panic {
                panic!("forced usage writer panic");
            }
            self.inner.upsert_usage_record(record).await
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for PanicOnceQueueConfiguredUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for PanicOnceQueueConfiguredUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for PanicOnceQueueConfiguredUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for PanicOnceQueueConfiguredUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.inner.queue))
        }
    }

    #[async_trait]
    impl UsageRecordWriter for EnrichmentCountingQueueStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.records.lock().expect("records lock").push(record);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for EnrichmentCountingQueueStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for EnrichmentCountingQueueStore {
        async fn enrich_usage_event(&self, event: &mut UsageEvent) -> Result<(), DataLayerError> {
            self.enrich_calls.fetch_add(1, Ordering::AcqRel);
            event.data.total_cost_usd = Some(0.123);
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for EnrichmentCountingQueueStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for EnrichmentCountingQueueStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }
    }

    #[async_trait]
    impl UsageRecordWriter for FailingWriteQueueConfiguredUsageStore {
        fn supports_first_byte_usage_batch(&self) -> bool {
            true
        }

        fn first_byte_usage_writer_identity(&self) -> Option<usize> {
            Some(Arc::as_ptr(&self.upsert_attempts) as usize)
        }

        async fn upsert_usage_record(
            &self,
            _record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.upsert_attempts.fetch_add(1, Ordering::AcqRel);
            Err(DataLayerError::Postgres(
                "forced direct fallback write failure".to_string(),
            ))
        }

        async fn upsert_first_byte_usage_records(
            &self,
            _records: Vec<UpsertUsageRecord>,
        ) -> Result<(), DataLayerError> {
            self.upsert_attempts.fetch_add(1, Ordering::AcqRel);
            Err(DataLayerError::Postgres(
                "forced batch first-byte write failure".to_string(),
            ))
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for FailingWriteQueueConfiguredUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for FailingWriteQueueConfiguredUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for FailingWriteQueueConfiguredUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for FailingWriteQueueConfiguredUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn supports_first_byte_usage_fast_path(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }
    }

    #[async_trait]
    impl UsageRecordWriter for QueueOnlyUsageStore {
        async fn upsert_usage_record(
            &self,
            _record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.upsert_attempts.fetch_add(1, Ordering::AcqRel);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for QueueOnlyUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for QueueOnlyUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for QueueOnlyUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for QueueOnlyUsageStore {
        fn has_usage_writer(&self) -> bool {
            false
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }
    }

    #[async_trait]
    impl UsageRecordWriter for BlockingWriteQueueConfiguredUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.write_started.notify_one();
            self.release_writes.notified().await;
            self.records.lock().expect("records lock").push(record);
            self.writes_completed.fetch_add(1, Ordering::AcqRel);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for BlockingWriteQueueConfiguredUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for BlockingWriteQueueConfiguredUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for BlockingWriteQueueConfiguredUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl UsageRuntimeAccess for BlockingWriteQueueConfiguredUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn supports_first_byte_usage_fast_path(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }
    }

    #[async_trait]
    impl UsageRecordWriter for BlockingPolicyQueueConfiguredUsageStore {
        async fn upsert_usage_record(
            &self,
            _record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for BlockingPolicyQueueConfiguredUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for BlockingPolicyQueueConfiguredUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for BlockingPolicyQueueConfiguredUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl UsageRuntimeAccess for BlockingPolicyQueueConfiguredUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
        }

        async fn body_capture_policy(&self) -> Result<UsageBodyCapturePolicy, DataLayerError> {
            self.policy_reads.fetch_add(1, Ordering::AcqRel);
            self.policy_started.notify_one();
            self.release_policy.notified().await;
            Ok(UsageBodyCapturePolicy::default())
        }
    }

    #[async_trait]
    impl UsageRecordWriter for FailingPolicyUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            self.inner.upsert_usage_record(record).await
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for FailingPolicyUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            false
        }

        async fn settle_usage(
            &self,
            _input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, DataLayerError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for FailingPolicyUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for FailingPolicyUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    #[async_trait]
    impl UsageRuntimeAccess for FailingPolicyUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            false
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            None
        }

        async fn body_capture_policy(&self) -> Result<UsageBodyCapturePolicy, DataLayerError> {
            self.policy_reads.fetch_add(1, Ordering::AcqRel);
            Err(DataLayerError::Postgres(
                "forced policy read failure".to_string(),
            ))
        }
    }

    #[async_trait]
    impl RuntimeQueueStore for FlakyAppendQueueStore {
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
            self.append_attempts.fetch_add(1, Ordering::AcqRel);
            let active = self.active_appends.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_active_appends.fetch_max(active, Ordering::AcqRel);
            if self.append_delay_ms > 0 {
                sleep(Duration::from_millis(self.append_delay_ms)).await;
            }
            let failed = self
                .remaining_failures
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    (current > 0).then(|| current - 1)
                })
                .is_ok();
            let result = if failed {
                Err(DataLayerError::Redis("forced append failure".to_string()))
            } else {
                self.inner
                    .append_fields_with_maxlen(stream, fields, maxlen)
                    .await
            };
            if result.is_ok() {
                self.successful_appends.fetch_add(1, Ordering::AcqRel);
            }
            self.active_appends.fetch_sub(1, Ordering::AcqRel);
            result
        }

        async fn read_group(
            &self,
            stream: &str,
            group: &str,
            consumer: &str,
            count: usize,
            block_ms: Option<u64>,
        ) -> Result<Vec<aether_runtime_state::RuntimeQueueEntry>, DataLayerError> {
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
            config: aether_runtime_state::RuntimeQueueReclaimConfig,
        ) -> Result<Vec<aether_runtime_state::RuntimeQueueEntry>, DataLayerError> {
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
            self.inner.ack(stream, group, ids).await
        }

        async fn delete(&self, stream: &str, ids: &[String]) -> Result<usize, DataLayerError> {
            self.inner.delete(stream, ids).await
        }

        async fn stats(
            &self,
            stream: &str,
            group: Option<&str>,
        ) -> Result<aether_runtime_state::RuntimeQueueStats, DataLayerError> {
            self.inner.stats(stream, group).await
        }
    }

    #[tokio::test]
    async fn terminal_usage_without_redis_writes_directly_to_usage_repository() {
        let runtime = UsageRuntime::new(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        })
        .expect("usage runtime should build");
        let store = NoRedisUsageStore::default();
        let event = UsageEvent::new(
            UsageEventType::Completed,
            "req-no-redis-1",
            UsageEventData {
                user_id: Some("user-no-redis-1".to_string()),
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                input_tokens: Some(4),
                output_tokens: Some(8),
                total_tokens: Some(12),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        runtime.record_terminal_event(&store, event).await;

        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request_id, "req-no-redis-1");
        assert_eq!(records[0].status, "completed");
        assert_eq!(records[0].total_tokens, Some(12));
    }

    #[tokio::test]
    async fn direct_terminal_usage_bypasses_redis_queue_and_writes_repository() {
        let runtime = UsageRuntime::new(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        })
        .expect("usage runtime should build");
        let store = QueueConfiguredUsageStore {
            inner: NoRedisUsageStore::default(),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let event = UsageEvent::new(
            UsageEventType::Failed,
            "req-direct-terminal-1",
            UsageEventData {
                user_id: Some("user-direct-terminal-1".to_string()),
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                status_code: Some(503),
                error_message: Some("upstream failed".to_string()),
                ..UsageEventData::default()
            },
        );

        runtime.record_terminal_event_direct(&store, event).await;

        let records = store.inner.records.lock().expect("records lock");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request_id, "req-direct-terminal-1");
        assert_eq!(records[0].status, "failed");
        assert_eq!(records[0].billing_status, "void");
        assert_eq!(records[0].status_code, Some(503));
    }

    #[tokio::test]
    async fn pending_usage_is_persisted_before_later_lifecycle_events() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            stream_key: "usage:events:test:pending".to_string(),
            consumer_group: "usage_consumers_test_pending".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let plan = ExecutionPlan {
            request_id: "req-lifecycle-queue-pending-1".to_string(),
            candidate_id: Some("cand-lifecycle-queue-pending-1".to_string()),
            provider_name: Some("openai".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        runtime.record_pending(&store, build_lifecycle_usage_seed(&plan, None));

        timeout(Duration::from_secs(1), async {
            while store.records.lock().expect("records lock").is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending lifecycle usage should be persisted");

        {
            let records = store.records.lock().expect("records lock");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].request_id, "req-lifecycle-queue-pending-1");
            assert_eq!(records[0].status, "pending");
        }
        assert!(
            queue
                .read_group("usage-test-consumer")
                .await
                .expect("queue read should succeed")
                .is_empty(),
            "ordered pending persistence should not leave a duplicate queue event"
        );
    }

    #[tokio::test]
    async fn lifecycle_submission_persists_pending_before_first_byte_streaming() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(1),
            stream_key: "usage:events:test:ordered-pending-streaming".to_string(),
            consumer_group: "usage_consumers_test_ordered_pending_streaming".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let plan = ExecutionPlan {
            request_id: "req-ordered-pending-streaming".to_string(),
            candidate_id: Some("cand-ordered-pending-streaming".to_string()),
            provider_name: Some("openai".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: true,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let seed = build_lifecycle_usage_seed(&plan, None);

        runtime.record_pending(&store, seed.clone());
        runtime.record_stream_started(
            &store,
            &seed,
            200,
            Some(&ExecutionTelemetry {
                ttfb_ms: Some(12),
                elapsed_ms: Some(34),
                upstream_bytes: Some(56),
            }),
        );

        timeout(Duration::from_secs(1), async {
            while store.records.lock().expect("records lock").len() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending and streaming lifecycle writes should complete");

        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, "pending");
        assert_eq!(records[1].status, "streaming");
        assert_eq!(records[1].first_byte_time_ms, Some(12));
    }

    #[tokio::test]
    async fn duplicate_first_byte_releases_ordered_barrier_for_terminal() {
        const CAPACITY: usize = 8;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(1),
            enqueue_retry_buffer_capacity: CAPACITY,
            ..UsageRuntimeConfig::default()
        };
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_id = "req-duplicate-first-byte-terminal";
        let seed = build_lifecycle_usage_seed(&terminal_test_plan(request_id), None);
        let telemetry = ExecutionTelemetry {
            ttfb_ms: Some(12),
            elapsed_ms: Some(20),
            upstream_bytes: Some(1),
        };

        runtime.record_stream_started(&store, &seed, 200, Some(&telemetry));
        timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if store.records.lock().expect("records lock").len() == 1
                    && snapshot.lifecycle_submission_pending == 0
                    && snapshot.first_byte_persistence_pending == 0
                    && snapshot.ordered_lifecycle_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first first-byte transition should complete");

        runtime.record_stream_started(&store, &seed, 200, Some(&telemetry));
        timeout(
            Duration::from_secs(1),
            runtime.record_terminal_event_direct(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            ),
        )
        .await
        .expect("a duplicate first-byte marker must release the terminal barrier");

        let records = store.records.lock().expect("records lock");
        assert_eq!(
            records.len(),
            2,
            "the duplicate first byte must be coalesced"
        );
        assert_eq!(records[0].status, "streaming");
        assert_eq!(records[1].status, "completed");
        drop(records);

        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.lifecycle_submission_pending, 0);
        assert_eq!(snapshot.first_byte_persistence_pending, 0);
        assert_eq!(snapshot.ordered_lifecycle_pending, 0);
        assert_eq!(
            runtime
                .lifecycle_submission
                .state
                .admission
                .available_permits(),
            CAPACITY
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn direct_terminal_holds_ordered_slot_until_persistence_finishes() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(2),
            terminal_submission_max_in_flight: 2,
            enqueue_retry_buffer_capacity: 8,
            ..UsageRuntimeConfig::default()
        };
        let store = AdmissionBlockedUsageStore {
            release_writes: Arc::new(tokio::sync::Semaphore::new(0)),
            writes_in_flight: Arc::new(AtomicUsize::new(0)),
            max_writes_in_flight: Arc::new(AtomicUsize::new(0)),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_id = "req-direct-terminal-ordering";
        let direct_runtime = runtime.clone();
        let direct_store = store.clone();
        let direct_terminal = tokio::spawn(async move {
            direct_runtime
                .record_terminal_event_direct(
                    &direct_store,
                    UsageEvent::new(
                        UsageEventType::Completed,
                        request_id,
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            status_code: Some(200),
                            ..UsageEventData::default()
                        },
                    ),
                )
                .await;
        });
        timeout(Duration::from_secs(1), async {
            while store.writes_in_flight.load(Ordering::Acquire) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the direct terminal write should start");

        runtime.record_pending(
            &store,
            build_lifecycle_usage_seed(&terminal_test_plan(request_id), None),
        );
        timeout(Duration::from_secs(1), async {
            while runtime.metrics_snapshot().ordered_lifecycle_pending != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the later pending phase should queue behind the direct terminal");
        sleep(Duration::from_millis(25)).await;
        assert_eq!(store.writes_in_flight.load(Ordering::Acquire), 1);
        assert_eq!(store.max_writes_in_flight.load(Ordering::Acquire), 1);
        assert_eq!(store.writes_completed.load(Ordering::Acquire), 0);

        store.release_writes.add_permits(1);
        timeout(Duration::from_secs(1), direct_terminal)
            .await
            .expect("the direct terminal caller should finish after persistence")
            .expect("the direct terminal task should not panic");
        timeout(Duration::from_secs(1), async {
            while store.writes_completed.load(Ordering::Acquire) != 1
                || store.writes_in_flight.load(Ordering::Acquire) != 1
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the queued pending write should start only after terminal persistence");

        store.release_writes.add_permits(1);
        timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if store.writes_completed.load(Ordering::Acquire) == 2
                    && snapshot.ordered_lifecycle_pending == 0
                    && snapshot.lifecycle_submission_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the queued pending phase should drain after its write resumes");
        assert_eq!(store.max_writes_in_flight.load(Ordering::Acquire), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn terminal_lifecycle_admission_bounds_resident_work_and_backpressures_excess() {
        const CAPACITY: usize = 40;
        const TOTAL: usize = CAPACITY + 8;

        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(CAPACITY),
            terminal_submission_max_in_flight: CAPACITY as u64,
            enqueue_retry_buffer_capacity: CAPACITY,
            ..UsageRuntimeConfig::default()
        };
        let store = AdmissionBlockedUsageStore {
            release_writes: Arc::new(tokio::sync::Semaphore::new(0)),
            writes_in_flight: Arc::new(AtomicUsize::new(0)),
            max_writes_in_flight: Arc::new(AtomicUsize::new(0)),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let mut submissions = Vec::with_capacity(TOTAL);

        for index in 0..TOTAL {
            let runtime = runtime.clone();
            let store = store.clone();
            submissions.push(tokio::spawn(async move {
                runtime
                    .submit_terminal_event(
                        &store,
                        UsageEvent::new(
                            UsageEventType::Completed,
                            format!("req-terminal-admission-{index}"),
                            UsageEventData {
                                provider_name: "openai".to_string(),
                                model: "gpt-5".to_string(),
                                status_code: Some(200),
                                ..UsageEventData::default()
                            },
                        ),
                    )
                    .await;
            }));
        }

        timeout(Duration::from_secs(2), async {
            loop {
                let finished = submissions
                    .iter()
                    .filter(|submission| submission.is_finished())
                    .count();
                if finished == CAPACITY
                    && store.writes_in_flight.load(Ordering::Acquire) == CAPACITY
                    && runtime
                        .lifecycle_submission
                        .state
                        .admission
                        .available_permits()
                        == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the hard lifecycle bound should backpressure excess terminal calls");

        let saturated = runtime.metrics_snapshot();
        assert_eq!(
            submissions
                .iter()
                .filter(|submission| submission.is_finished())
                .count(),
            CAPACITY,
            "only admitted callers may return while persistence is blocked"
        );
        assert!(saturated.lifecycle_submission_pending <= CAPACITY);
        assert!(saturated.ordered_lifecycle_pending <= CAPACITY);
        assert!(saturated.terminal_submission_pending <= CAPACITY);
        assert!(saturated.terminal_submission_in_flight <= CAPACITY);
        assert_eq!(saturated.ordered_lifecycle_pending, CAPACITY);
        assert_eq!(saturated.terminal_submission_pending, CAPACITY);
        assert_eq!(
            store.max_writes_in_flight.load(Ordering::Acquire),
            CAPACITY,
            "configured workers should still execute independently up to the hard bound"
        );

        store.release_writes.add_permits(TOTAL);
        timeout(Duration::from_secs(2), async {
            for submission in submissions {
                submission
                    .await
                    .expect("terminal admission task should not panic");
            }
        })
        .await
        .expect("all backpressured terminal callers should recover after persistence resumes");
        timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if store.writes_completed.load(Ordering::Acquire) == TOTAL
                    && snapshot.lifecycle_submission_pending == 0
                    && snapshot.ordered_lifecycle_pending == 0
                    && snapshot.terminal_submission_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all admitted lifecycle work should drain after recovery");

        let recovered = runtime.metrics_snapshot();
        assert!(recovered.lifecycle_submission_max_pending <= CAPACITY);
        assert!(recovered.ordered_lifecycle_max_pending <= CAPACITY);
        assert!(recovered.terminal_submission_max_pending <= CAPACITY);
        assert_eq!(recovered.terminal_submission_in_flight, 0);
        assert_eq!(
            runtime
                .lifecycle_submission
                .state
                .admission
                .available_permits(),
            CAPACITY
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn non_batch_pending_single_writes_are_concurrent_and_globally_bounded() {
        const ITEMS_PER_BATCH: usize =
            super::PENDING_PERSISTENCE_SINGLE_WRITE_CONCURRENCY_PER_BATCH * 2;
        const TOTAL_ITEMS: usize =
            ITEMS_PER_BATCH * super::PENDING_PERSISTENCE_MAX_BATCH_CONCURRENCY;

        assert_eq!(
            super::PENDING_PERSISTENCE_SINGLE_WRITE_CONCURRENCY_PER_BATCH
                * super::PENDING_PERSISTENCE_MAX_BATCH_CONCURRENCY,
            super::PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY
        );

        let store = BlockingNonBatchPendingStore {
            release_writes: Arc::new(tokio::sync::Semaphore::new(0)),
            writes_in_flight: Arc::new(AtomicUsize::new(0)),
            max_writes_in_flight: Arc::new(AtomicUsize::new(0)),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        assert!(!UsageRecordWriter::supports_pending_usage_batch(&store));
        let record_gate = Arc::new(super::UsageWorkerRecordConcurrencyGate::new(
            super::PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY * 2,
        ));
        let completed = Arc::new(AtomicUsize::new(0));
        let degraded = Arc::new(AtomicUsize::new(0));
        let state = Arc::new(super::PendingPersistenceState::new(TOTAL_ITEMS));
        let mut tasks = tokio::task::JoinSet::new();

        for batch_index in 0..super::PENDING_PERSISTENCE_MAX_BATCH_CONCURRENCY {
            let mut batch = Vec::with_capacity(ITEMS_PER_BATCH);
            for item_index in 0..ITEMS_PER_BATCH {
                let request_id = format!("req-blocking-non-batch-{batch_index}-{item_index}");
                let record = super::build_upsert_usage_record_from_event(&UsageEvent::new(
                    UsageEventType::Pending,
                    request_id.clone(),
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ))
                .expect("pending record should build");
                state.record_dispatched();
                batch.push(super::PendingPersistenceEnvelope {
                    item: Box::new(BlockingNonBatchPendingItem {
                        request_id,
                        record,
                        store: store.clone(),
                        record_gate: Arc::clone(&record_gate),
                        completed: Arc::clone(&completed),
                        degraded: Arc::clone(&degraded),
                    }),
                    _pending_guard: super::PendingPersistencePendingGuard {
                        state: Arc::clone(&state),
                    },
                });
            }
            let task_state = Arc::clone(&state);
            tasks.spawn(async move {
                super::process_pending_persistence_batch(batch, task_state).await;
            });
        }

        timeout(Duration::from_secs(2), async {
            while store.max_writes_in_flight.load(Ordering::Acquire)
                < super::PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("non-batch pending writes should fill the bounded concurrency window");
        sleep(Duration::from_millis(25)).await;

        let max_in_flight = store.max_writes_in_flight.load(Ordering::Acquire);
        assert!(max_in_flight > super::PENDING_PERSISTENCE_MAX_BATCH_CONCURRENCY);
        assert_eq!(
            max_in_flight,
            super::PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY
        );
        assert_eq!(
            store.writes_in_flight.load(Ordering::Acquire),
            super::PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY
        );
        assert_eq!(
            record_gate.max_in_flight(),
            super::PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY
        );

        store.release_writes.add_permits(TOTAL_ITEMS);
        timeout(Duration::from_secs(2), async {
            while let Some(result) = tasks.join_next().await {
                result.expect("pending batch task should complete");
            }
        })
        .await
        .expect("bounded non-batch pending writes should drain after release");

        assert_eq!(store.writes_completed.load(Ordering::Acquire), TOTAL_ITEMS);
        assert_eq!(completed.load(Ordering::Acquire), TOTAL_ITEMS);
        assert_eq!(degraded.load(Ordering::Acquire), 0);
        assert_eq!(state.pending.load(Ordering::Acquire), 0);
        assert_eq!(store.writes_in_flight.load(Ordering::Acquire), 0);
        assert_eq!(record_gate.in_flight(), 0);
    }

    #[tokio::test]
    async fn saturated_pending_persistence_drops_overflow_and_stays_bounded() {
        const RESIDENT_BOUND: usize = 1_024
            + super::PENDING_PERSISTENCE_BATCH_SIZE
                * super::PENDING_PERSISTENCE_MAX_BATCH_CONCURRENCY
            + 1;
        const TOTAL_ITEMS: usize = RESIDENT_BOUND + 128;

        let config = UsageRuntimeConfig {
            enabled: true,
            enqueue_retry_buffer_capacity: 1_024,
            ..UsageRuntimeConfig::default()
        };
        let dispatcher = super::PendingPersistenceDispatcher::spawn(&config);
        let store = BlockingNonBatchPendingStore {
            release_writes: Arc::new(tokio::sync::Semaphore::new(0)),
            writes_in_flight: Arc::new(AtomicUsize::new(0)),
            max_writes_in_flight: Arc::new(AtomicUsize::new(0)),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        let record_gate = Arc::new(super::UsageWorkerRecordConcurrencyGate::new(
            super::PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY * 2,
        ));
        let completed = Arc::new(AtomicUsize::new(0));
        let degraded = Arc::new(AtomicUsize::new(0));
        let dispatch_dispatcher = Arc::clone(&dispatcher);
        let dispatch_store = store.clone();
        let dispatch_gate = Arc::clone(&record_gate);
        let dispatch_completed = Arc::clone(&completed);
        let dispatch_degraded = Arc::clone(&degraded);
        let dispatch = tokio::spawn(async move {
            for index in 0..TOTAL_ITEMS {
                let request_id = format!("req-bounded-pending-{index}");
                let record = super::build_upsert_usage_record_from_event(&UsageEvent::new(
                    UsageEventType::Pending,
                    request_id.clone(),
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ))
                .expect("pending record should build");
                dispatch_dispatcher.dispatch(Box::new(BlockingNonBatchPendingItem {
                    request_id,
                    record,
                    store: dispatch_store.clone(),
                    record_gate: Arc::clone(&dispatch_gate),
                    completed: Arc::clone(&dispatch_completed),
                    degraded: Arc::clone(&dispatch_degraded),
                }));
            }
        });

        timeout(Duration::from_secs(5), dispatch)
            .await
            .expect("overflow dispatch must remain non-blocking")
            .expect("overflow dispatch task should not panic");
        let overflow = dispatcher.state.overflow_total.load(Ordering::Acquire);
        assert!(overflow > 0, "the test must overflow the bounded channel");
        assert_eq!(degraded.load(Ordering::Acquire) as u64, overflow);
        assert!(
            dispatcher.state.max_pending.load(Ordering::Acquire) <= RESIDENT_BOUND,
            "resident pending records exceeded the fixed channel and worker bound"
        );
        store.release_writes.add_permits(TOTAL_ITEMS);
        timeout(Duration::from_secs(5), async {
            while dispatcher.state.pending.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all accepted pending records should drain after recovery");
        let writes_completed = store.writes_completed.load(Ordering::Acquire);
        assert_eq!(completed.load(Ordering::Acquire), writes_completed);
        assert_eq!(writes_completed as u64 + overflow, TOTAL_ITEMS as u64);
        assert!(
            store.max_writes_in_flight.load(Ordering::Acquire)
                <= super::PENDING_PERSISTENCE_SINGLE_WRITE_TARGET_CONCURRENCY,
            "pending persistence exceeded its fixed worker window"
        );
    }

    #[tokio::test]
    async fn ordered_lifecycle_batches_beyond_submission_worker_count() {
        const REQUESTS: usize = 96;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(1),
            enqueue_retry_workers: 1,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 2,
            ..UsageRuntimeConfig::default()
        };
        let store = OrderedBatchUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            pending_batch_sizes: Arc::new(Mutex::new(Vec::new())),
            pending_attempts: Arc::new(Mutex::new(BTreeMap::new())),
            permanent_pending_failure_request_id: None,
            remaining_pending_failures: Arc::new(AtomicUsize::new(0)),
            remaining_pending_panics: Arc::new(AtomicUsize::new(0)),
            remaining_first_byte_panics: Arc::new(AtomicUsize::new(0)),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let mut request_ids = Vec::with_capacity(REQUESTS);

        for index in 0..REQUESTS {
            let request_id = format!("req-ordered-batch-{index}");
            let seed = build_lifecycle_usage_seed(&terminal_test_plan(&request_id), None);
            runtime.record_pending(&store, seed.clone());
            runtime.record_stream_started(
                &store,
                &seed,
                200,
                Some(&ExecutionTelemetry {
                    ttfb_ms: Some(10),
                    elapsed_ms: Some(10),
                    upstream_bytes: Some(1),
                }),
            );
            request_ids.push(request_id);
        }

        timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if store.records.lock().expect("records lock").len() == REQUESTS * 2
                    && snapshot.lifecycle_submission_pending == 0
                    && snapshot.ordered_lifecycle_pending == 0
                    && snapshot.pending_persistence_pending == 0
                    && snapshot.first_byte_persistence_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("ordered pending and first-byte batches should drain");

        let snapshot = runtime.metrics_snapshot();
        assert!(
            snapshot.pending_persistence_max_batch_size > 7,
            "pending max batch was {}",
            snapshot.pending_persistence_max_batch_size
        );
        assert!(
            snapshot.first_byte_persistence_max_batch_size > 7,
            "first-byte max batch was {}",
            snapshot.first_byte_persistence_max_batch_size
        );
        assert_eq!(
            snapshot.pending_persistence_batch_records_total,
            REQUESTS as u64
        );
        assert_eq!(
            snapshot.first_byte_persistence_batch_records_total,
            REQUESTS as u64
        );

        let records = store.records.lock().expect("records lock");
        for request_id in request_ids {
            let statuses = records
                .iter()
                .filter(|record| record.request_id == request_id)
                .map(|record| record.status.as_str())
                .collect::<Vec<_>>();
            assert_eq!(statuses, vec!["pending", "streaming"]);
        }
    }

    #[tokio::test]
    async fn ordered_lifecycle_retries_pending_and_degrades_first_byte_before_terminal_barrier() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(1),
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 2,
            ..UsageRuntimeConfig::default()
        };
        let store = OrderedBatchUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            pending_batch_sizes: Arc::new(Mutex::new(Vec::new())),
            pending_attempts: Arc::new(Mutex::new(BTreeMap::new())),
            permanent_pending_failure_request_id: None,
            remaining_pending_failures: Arc::new(AtomicUsize::new(1)),
            remaining_pending_panics: Arc::new(AtomicUsize::new(1)),
            remaining_first_byte_panics: Arc::new(AtomicUsize::new(1)),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_id = "req-ordered-retry-barrier";
        let seed = build_lifecycle_usage_seed(&terminal_test_plan(request_id), None);

        runtime.record_pending(&store, seed.clone());
        runtime.record_stream_started(
            &store,
            &seed,
            200,
            Some(&ExecutionTelemetry {
                ttfb_ms: Some(9),
                elapsed_ms: Some(9),
                upstream_bytes: Some(1),
            }),
        );
        runtime
            .record_terminal_event_direct(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        let records = store.records.lock().expect("records lock");
        let statuses = records
            .iter()
            .filter(|record| record.request_id == request_id)
            .map(|record| record.status.as_str())
            .collect::<Vec<_>>();
        assert_eq!(statuses, vec!["pending", "completed"]);
        drop(records);

        let snapshot = runtime.metrics_snapshot();
        assert!(snapshot.pending_persistence_batch_failed_total >= 2);
        assert!(snapshot.pending_persistence_retried_total >= 2);
        assert_eq!(snapshot.first_byte_persistence_batch_failed_total, 1);
        assert_eq!(snapshot.first_byte_persistence_direct_failed_total, 1);
        assert_eq!(snapshot.first_byte_persistence_fallback_failed_total, 1);
        assert_eq!(snapshot.first_byte_persistence_pending, 0);
        assert_eq!(snapshot.ordered_lifecycle_pending, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn permanent_pending_failure_isolated_without_blocking_healthy_or_terminal_writes() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(4),
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let poison_request_id = "req-permanent-pending-failure";
        let healthy_request_id = "req-after-permanent-pending-failure";
        let pending_attempts = Arc::new(Mutex::new(BTreeMap::new()));
        let pending_batch_sizes = Arc::new(Mutex::new(Vec::new()));
        let store = OrderedBatchUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            pending_batch_sizes: Arc::clone(&pending_batch_sizes),
            pending_attempts: Arc::clone(&pending_attempts),
            permanent_pending_failure_request_id: Some(Arc::from(poison_request_id)),
            remaining_pending_failures: Arc::new(AtomicUsize::new(0)),
            remaining_pending_panics: Arc::new(AtomicUsize::new(0)),
            remaining_first_byte_panics: Arc::new(AtomicUsize::new(0)),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        runtime.record_pending(
            &store,
            build_lifecycle_usage_seed(&terminal_test_plan(poison_request_id), None),
        );
        runtime.record_pending(
            &store,
            build_lifecycle_usage_seed(&terminal_test_plan(healthy_request_id), None),
        );

        let poison_runtime = runtime.clone();
        let poison_store = store.clone();
        let poison_terminal = tokio::spawn(async move {
            poison_runtime
                .record_terminal_event_direct(
                    &poison_store,
                    UsageEvent::new(
                        UsageEventType::Completed,
                        poison_request_id,
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            status_code: Some(200),
                            ..UsageEventData::default()
                        },
                    ),
                )
                .await;
        });
        let healthy_runtime = runtime.clone();
        let healthy_store = store.clone();
        let healthy_terminal = tokio::spawn(async move {
            healthy_runtime
                .record_terminal_event_direct(
                    &healthy_store,
                    UsageEvent::new(
                        UsageEventType::Completed,
                        healthy_request_id,
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            status_code: Some(200),
                            ..UsageEventData::default()
                        },
                    ),
                )
                .await;
        });

        timeout(Duration::from_secs(2), async {
            poison_terminal
                .await
                .expect("poison terminal task should not panic");
            healthy_terminal
                .await
                .expect("healthy terminal task should not panic");
        })
        .await
        .expect("bounded pending retries must release both terminal barriers");
        timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if snapshot.lifecycle_submission_pending == 0
                    && snapshot.pending_persistence_pending == 0
                    && snapshot.ordered_lifecycle_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending persistence and ordered lifecycle gauges should drain");

        let records = store.records.lock().expect("records lock");
        let poison_statuses = records
            .iter()
            .filter(|record| record.request_id == poison_request_id)
            .map(|record| record.status.as_str())
            .collect::<Vec<_>>();
        let healthy_statuses = records
            .iter()
            .filter(|record| record.request_id == healthy_request_id)
            .map(|record| record.status.as_str())
            .collect::<Vec<_>>();
        assert_eq!(poison_statuses, vec!["completed"]);
        assert_eq!(healthy_statuses, vec!["pending", "completed"]);
        drop(records);

        let attempts = pending_attempts.lock().expect("pending attempts lock");
        assert_eq!(
            attempts.get(poison_request_id).copied(),
            Some(
                (super::PENDING_PERSISTENCE_BATCH_RETRIES_BEFORE_ISOLATION
                    + super::PENDING_PERSISTENCE_SINGLE_RETRIES_BEFORE_DEGRADE)
                    as usize
            )
        );
        assert!(
            (1..=3).contains(
                &attempts
                    .get(healthy_request_id)
                    .copied()
                    .expect("healthy pending record should be attempted")
            ),
            "a healthy record may share the failed batch but must need only one isolated write"
        );
        drop(attempts);
        assert_eq!(
            pending_batch_sizes
                .lock()
                .expect("pending batch sizes lock")
                .len(),
            6,
            "batch isolation plus bounded single retries must have fixed write cost"
        );

        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.pending_persistence_batch_failed_total, 5);
        assert_eq!(snapshot.pending_persistence_overflow_total, 1);
        assert!((5..=7).contains(&snapshot.pending_persistence_retried_total));
        assert_eq!(snapshot.pending_persistence_pending, 0);
        assert_eq!(snapshot.ordered_lifecycle_pending, 0);
    }

    #[tokio::test]
    async fn permanent_ordered_first_byte_failure_degrades_and_allows_terminal_progress() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(1),
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let request_id = "req-permanent-ordered-first-byte-failure";
        let remaining_first_byte_panics = Arc::new(AtomicUsize::new(usize::MAX));
        let store = OrderedBatchUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            pending_batch_sizes: Arc::new(Mutex::new(Vec::new())),
            pending_attempts: Arc::new(Mutex::new(BTreeMap::new())),
            permanent_pending_failure_request_id: None,
            remaining_pending_failures: Arc::new(AtomicUsize::new(0)),
            remaining_pending_panics: Arc::new(AtomicUsize::new(0)),
            remaining_first_byte_panics: Arc::clone(&remaining_first_byte_panics),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let seed = build_lifecycle_usage_seed(&terminal_test_plan(request_id), None);

        runtime.record_pending(&store, seed.clone());
        runtime.record_stream_started(
            &store,
            &seed,
            200,
            Some(&ExecutionTelemetry {
                ttfb_ms: Some(9),
                elapsed_ms: Some(12),
                upstream_bytes: Some(1),
            }),
        );
        timeout(
            Duration::from_secs(2),
            runtime.record_terminal_event_direct(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            ),
        )
        .await
        .expect("terminal should pass the degraded first-byte barrier");
        timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if snapshot.first_byte_persistence_pending == 0
                    && snapshot.ordered_lifecycle_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first-byte and ordered lifecycle gauges should drain");

        let records = store.records.lock().expect("records lock");
        let statuses = records
            .iter()
            .filter(|record| record.request_id == request_id)
            .map(|record| record.status.as_str())
            .collect::<Vec<_>>();
        assert_eq!(statuses, vec!["pending", "completed"]);
        drop(records);

        assert_eq!(
            remaining_first_byte_panics.load(Ordering::Acquire),
            usize::MAX - 1,
            "an ordered first-byte failure must not spawn an unbounded fallback task"
        );
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.first_byte_persistence_batch_failed_total, 1);
        assert_eq!(snapshot.first_byte_persistence_direct_failed_total, 1);
        assert_eq!(snapshot.first_byte_persistence_fallback_failed_total, 1);
        assert_eq!(snapshot.first_byte_persistence_pending, 0);
        assert_eq!(snapshot.ordered_lifecycle_pending, 0);
    }

    #[tokio::test]
    async fn terminal_worker_catches_panic_and_processes_the_next_item() {
        let (sender, receiver) = mpsc::unbounded_channel::<Box<dyn super::TerminalExecutionItem>>();
        let worker = tokio::spawn(super::run_terminal_execution_worker(receiver));
        assert!(
            sender
                .send(Box::new(PanickingTerminalExecutionItem {
                    request_id: "req-terminal-worker-direct-panic".to_string(),
                }))
                .is_ok(),
            "the panic item should enter the worker FIFO"
        );

        let (completion, completed) = tokio::sync::oneshot::channel();
        assert!(
            sender
                .send(Box::new(CompletingTerminalExecutionItem {
                    request_id: "req-terminal-worker-direct-next".to_string(),
                    completion,
                }))
                .is_ok(),
            "the healthy item should enter the same worker FIFO"
        );
        timeout(Duration::from_secs(1), completed)
            .await
            .expect("the worker should continue after the preceding panic")
            .expect("the healthy item should report completion");

        drop(sender);
        timeout(Duration::from_secs(1), worker)
            .await
            .expect("the terminal worker should stop after its sender closes")
            .expect("the terminal worker should exit normally");
    }

    #[tokio::test]
    async fn failed_terminal_persistence_releases_admission_and_allows_later_attempts() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(1),
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let upsert_attempts = Arc::new(AtomicUsize::new(0));
        let store = FailingWriteQueueConfiguredUsageStore {
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
            upsert_attempts: Arc::clone(&upsert_attempts),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_id = "req-terminal-persistence-fail-closed";

        runtime
            .submit_terminal_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        timeout(Duration::from_secs(1), async {
            while upsert_attempts.load(Ordering::Acquire) != 1
                || runtime.metrics_snapshot().terminal_submission_pending != 0
                || runtime.metrics_snapshot().ordered_lifecycle_pending != 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the failed terminal persistence attempt should release admission");

        timeout(
            Duration::from_secs(1),
            runtime.record_terminal_event_direct(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            ),
        )
        .await
        .expect("a later terminal attempt must not remain behind a failed write");
        assert_eq!(
            upsert_attempts.load(Ordering::Acquire),
            2,
            "the later direct caller should receive its own bounded write attempt"
        );
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.terminal_submission_pending, 0);
        assert_eq!(snapshot.ordered_lifecycle_pending, 0);
        assert_eq!(snapshot.lifecycle_submission_pending, 0);
    }

    #[tokio::test]
    async fn terminal_worker_panic_releases_admission_without_stopping_the_shard() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(1),
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let records = Arc::new(Mutex::new(Vec::new()));
        let policy_reads = Arc::new(AtomicUsize::new(0));
        let remaining_policy_panics = Arc::new(AtomicUsize::new(1));
        let store = PanicOncePolicyQueueConfiguredUsageStore {
            inner: CloneQueueConfiguredUsageStore {
                records: Arc::clone(&records),
                queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
            },
            remaining_policy_panics: Arc::clone(&remaining_policy_panics),
            policy_reads: Arc::clone(&policy_reads),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let failed_request_id = "req-terminal-worker-panic-fail-closed";

        runtime
            .submit_terminal_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    failed_request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        timeout(Duration::from_secs(1), async {
            while policy_reads.load(Ordering::Acquire) == 0
                || runtime.metrics_snapshot().terminal_submission_pending != 0
                || runtime.metrics_snapshot().ordered_lifecycle_pending != 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first terminal item should panic and release admission");

        timeout(
            Duration::from_secs(1),
            runtime.record_terminal_event_direct(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    failed_request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            ),
        )
        .await
        .expect("a later attempt for the panicked request should make progress");

        let healthy_request_id = "req-terminal-worker-panic-healthy";
        runtime
            .submit_terminal_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    healthy_request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        runtime
            .record_terminal_event_direct(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    healthy_request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        assert_eq!(remaining_policy_panics.load(Ordering::Acquire), 0);
        let records = records.lock().expect("records lock");
        assert_eq!(
            records
                .iter()
                .filter(|record| record.request_id == healthy_request_id)
                .count(),
            2,
            "the same terminal shard should continue processing healthy requests"
        );
        assert!(
            records
                .iter()
                .filter(|record| record.request_id == failed_request_id)
                .count()
                == 1,
            "only the later healthy attempt should persist for the panicked request"
        );
        drop(records);
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.terminal_submission_pending, 0);
        assert_eq!(snapshot.ordered_lifecycle_pending, 0);
        assert_eq!(snapshot.lifecycle_submission_pending, 0);
    }

    #[tokio::test]
    async fn stale_lifecycle_generation_keeps_the_current_generation_registered() {
        let coalescer = super::LifecycleEventCoalescer::default();
        let request_id = "req-lifecycle-generation";
        let pending_generation = coalescer
            .register(request_id.to_string())
            .await
            .expect("pending generation should register");
        let streaming_generation = coalescer
            .register(request_id.to_string())
            .await
            .expect("streaming generation should register");

        assert!(!coalescer.should_emit(request_id, pending_generation).await);
        assert!(
            coalescer
                .should_emit(request_id, streaming_generation)
                .await
        );
    }

    #[tokio::test]
    async fn lifecycle_submission_panic_releases_admission_and_allows_later_slots() {
        let config = UsageRuntimeConfig {
            enabled: true,
            enqueue_retry_buffer_capacity: 8,
            enqueue_retry_workers: 1,
            worker_record_concurrency_limit: Some(1),
            ..UsageRuntimeConfig::default()
        };
        let dispatcher = LifecycleSubmissionDispatcher::spawn(&config);
        let request_id = "req-lifecycle-submission-panic";
        dispatcher.dispatch(Box::new(PanickingLifecycleSubmissionItem {
            request_id: request_id.to_string(),
        }));

        timeout(Duration::from_secs(1), async {
            while dispatcher.state.pending.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the panicked lifecycle slot should release its pending gauge");

        let seen = Arc::new(Mutex::new(Vec::new()));
        dispatcher.dispatch(Box::new(TestLifecycleSubmissionItem {
            request_id: request_id.to_string(),
            priority: LifecycleSubmissionPriority::Terminal,
            started: None,
            release: None,
            seen: Arc::clone(&seen),
        }));
        timeout(Duration::from_secs(1), async {
            while dispatcher.state.processed_total.load(Ordering::Acquire) < 2
                || dispatcher.state.pending.load(Ordering::Acquire) != 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a later slot should run after the panicked slot releases admission");

        assert_eq!(
            seen.lock().expect("lifecycle test seen lock").as_slice(),
            [LifecycleSubmissionPriority::Terminal]
        );
        assert_eq!(dispatcher.state.enqueued_total.load(Ordering::Acquire), 2);
        assert_eq!(dispatcher.state.processed_total.load(Ordering::Acquire), 2);
    }

    #[tokio::test]
    async fn lifecycle_submission_preserves_pending_before_coalesced_first_byte() {
        let config = UsageRuntimeConfig {
            enabled: true,
            enqueue_retry_buffer_capacity: 8,
            enqueue_retry_workers: 1,
            worker_record_concurrency_limit: Some(1),
            ..UsageRuntimeConfig::default()
        };
        let dispatcher = LifecycleSubmissionDispatcher::spawn(&config);
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let seen = Arc::new(Mutex::new(Vec::new()));

        dispatcher.dispatch(Box::new(TestLifecycleSubmissionItem {
            request_id: "blocker".to_string(),
            priority: LifecycleSubmissionPriority::Pending,
            started: Some(Arc::clone(&started)),
            release: Some(Arc::clone(&release)),
            seen: Arc::clone(&seen),
        }));
        timeout(Duration::from_secs(1), started.notified())
            .await
            .expect("lifecycle worker should start the blocker");

        dispatcher.dispatch(Box::new(TestLifecycleSubmissionItem {
            request_id: "target".to_string(),
            priority: LifecycleSubmissionPriority::Pending,
            started: None,
            release: None,
            seen: Arc::clone(&seen),
        }));
        dispatcher.dispatch(Box::new(TestLifecycleSubmissionItem {
            request_id: "target".to_string(),
            priority: LifecycleSubmissionPriority::Streaming,
            started: None,
            release: None,
            seen: Arc::clone(&seen),
        }));
        dispatcher.dispatch(Box::new(TestLifecycleSubmissionItem {
            request_id: "target".to_string(),
            priority: LifecycleSubmissionPriority::FirstByte,
            started: None,
            release: None,
            seen: Arc::clone(&seen),
        }));

        assert_eq!(dispatcher.state.pending.load(Ordering::Acquire), 2);
        release.notify_one();
        timeout(Duration::from_secs(1), async {
            loop {
                if seen.lock().expect("lifecycle test seen lock").len() == 3 {
                    break;
                }
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("coalesced lifecycle items should drain");

        let seen = seen.lock().expect("lifecycle test seen lock").clone();
        assert_eq!(
            seen,
            vec![
                LifecycleSubmissionPriority::Pending,
                LifecycleSubmissionPriority::Pending,
                LifecycleSubmissionPriority::FirstByte
            ]
        );
        assert_eq!(dispatcher.state.coalesced_total.load(Ordering::Acquire), 1);
        assert_eq!(dispatcher.state.overflow_total.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn streaming_lifecycle_event_survives_a_superseded_pending_event() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 40,
            stream_key: "usage:events:test:pending-streaming-coalescing".to_string(),
            consumer_group: "usage_consumers_test_pending_streaming_coalescing".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Pending,
                    "req-pending-streaming-coalescing",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5.6-sol".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        sleep(Duration::from_millis(10)).await;
        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Streaming,
                    "req-pending-streaming-coalescing",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5.6-sol".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        sleep(Duration::from_millis(90)).await;
        let entries = queue
            .read_group("usage-test-consumer-pending-streaming-coalescing")
            .await
            .expect("queue read should succeed");
        assert_eq!(entries.len(), 1);
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued usage event should parse");
        assert_eq!(event.event_type, UsageEventType::Streaming);
    }

    #[tokio::test]
    async fn pending_lifecycle_enqueue_can_be_delayed() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 30,
            stream_key: "usage:events:test:pending-delay".to_string(),
            consumer_group: "usage_consumers_test_pending_delay".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let event = UsageEvent::new(
            UsageEventType::Pending,
            "req-lifecycle-delay",
            UsageEventData {
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                ..UsageEventData::default()
            },
        );

        runtime.enqueue_or_write_lifecycle(&store, event).await;

        let immediate = queue
            .read_group("usage-test-consumer-delay")
            .await
            .expect("queue read should succeed");
        assert!(
            immediate.is_empty(),
            "lifecycle event should not enqueue before the delay"
        );

        sleep(Duration::from_millis(60)).await;
        let entries = queue
            .read_group("usage-test-consumer-delay")
            .await
            .expect("queue read should succeed");
        assert_eq!(entries.len(), 1);
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued usage event should parse");
        assert_eq!(event.request_id, "req-lifecycle-delay");
        assert_eq!(event.event_type, UsageEventType::Pending);
    }

    #[tokio::test]
    async fn lifecycle_delay_worker_respects_each_item_due_time() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 60,
            stream_key: "usage:events:test:per-item-delay".to_string(),
            consumer_group: "usage_consumers_test_per_item_delay".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Pending,
                    "req-per-item-delay-1",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        sleep(Duration::from_millis(40)).await;
        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Pending,
                    "req-per-item-delay-2",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        sleep(Duration::from_millis(50)).await;
        runtime
            .lifecycle_coalescer
            .cancel("req-per-item-delay-2")
            .await;

        let entries = queue
            .read_group("usage-test-consumer-per-item-delay-first")
            .await
            .expect("queue read should succeed");
        assert_eq!(
            entries.len(),
            1,
            "only the first lifecycle event should be due"
        );
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued usage event should parse");
        assert_eq!(event.request_id, "req-per-item-delay-1");

        let premature_second = queue
            .read_group("usage-test-consumer-per-item-delay-premature-second")
            .await
            .expect("queue read should succeed");
        assert!(
            premature_second.is_empty(),
            "second lifecycle event should not be emitted before its own delay"
        );

        sleep(Duration::from_millis(50)).await;
        let entries = queue
            .read_group("usage-test-consumer-per-item-delay-second")
            .await
            .expect("queue read should succeed");
        assert!(
            entries.is_empty(),
            "second lifecycle event should still be cancellable until its own due time"
        );
    }

    #[tokio::test]
    async fn lifecycle_delay_retains_admission_while_items_reside_in_timer_heap() {
        const CAPACITY: usize = 3;
        const BURST: usize = CAPACITY * 3;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 60_000,
            enqueue_retry_buffer_capacity: CAPACITY,
            ..UsageRuntimeConfig::default()
        };
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let mut accepted = 0;

        for index in 0..BURST {
            let event = UsageEvent::new(
                UsageEventType::Pending,
                format!("req-delay-heap-bound-{index}"),
                UsageEventData {
                    provider_name: "openai".to_string(),
                    model: "gpt-5".to_string(),
                    ..UsageEventData::default()
                },
            );
            if runtime
                .lifecycle_delay
                .schedule(store.clone(), event, index as u64 + 1)
                .await
                .is_ok()
            {
                accepted += 1;
            }
        }
        assert_eq!(accepted, CAPACITY);

        timeout(Duration::from_secs(1), async {
            while runtime
                .lifecycle_delay
                .sender
                .as_ref()
                .expect("delay sender should be enabled")
                .capacity()
                != CAPACITY
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the delay worker should move accepted items into its timer heap");
        assert_eq!(
            runtime.lifecycle_delay.admission.available_permits(),
            0,
            "moving items out of the channel must not release resident-work admission"
        );

        let overflow = UsageEvent::new(
            UsageEventType::Pending,
            "req-delay-heap-overflow",
            UsageEventData {
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                ..UsageEventData::default()
            },
        );
        assert!(
            runtime
                .lifecycle_delay
                .schedule(store, overflow, u64::MAX)
                .await
                .is_err(),
            "timer-heap residents must continue to enforce the hard capacity"
        );
    }

    #[tokio::test]
    async fn older_delayed_generation_does_not_discard_latest_lifecycle_event() {
        let coalescer = LifecycleEventCoalescer::default();
        let request_id = "req-latest-lifecycle-generation";
        let older = coalescer.register(request_id.to_string()).await;
        let latest = coalescer.register(request_id.to_string()).await;

        assert!(
            !coalescer
                .should_emit(request_id, older.expect("older generation"))
                .await,
            "the superseded generation must not be emitted"
        );
        assert!(
            coalescer
                .should_emit(request_id, latest.expect("latest generation"))
                .await,
            "checking an older generation must preserve the latest generation"
        );
    }

    #[tokio::test]
    async fn lifecycle_cleanup_tokens_preserve_newer_first_byte_and_terminal_markers() {
        let coalescer = LifecycleEventCoalescer::default();
        let request_id = "req-lifecycle-cleanup-token";
        let delayed_generation = coalescer
            .register(request_id.to_string())
            .await
            .expect("delayed generation");
        let first_byte_generation = coalescer
            .mark_first_byte(request_id)
            .await
            .expect("first-byte generation");

        coalescer.abandon(request_id, delayed_generation).await;
        assert!(
            coalescer.mark_first_byte(request_id).await.is_none(),
            "abandoning an older delayed item must preserve the first-byte marker"
        );

        coalescer.cancel(request_id).await;
        coalescer
            .rollback_first_byte(request_id, first_byte_generation)
            .await;
        assert!(
            coalescer.register(request_id.to_string()).await.is_none(),
            "rolling back an older first-byte enqueue must preserve the terminal marker"
        );

        let retry_request_id = "req-first-byte-marker-retry";
        let retry_generation = coalescer
            .mark_first_byte(retry_request_id)
            .await
            .expect("retry first-byte generation");
        coalescer
            .rollback_first_byte(retry_request_id, retry_generation)
            .await;
        assert!(
            coalescer.mark_first_byte(retry_request_id).await.is_some(),
            "a rejected first-byte enqueue should be allowed to retry"
        );
    }

    #[tokio::test]
    async fn accepted_first_byte_remains_current_after_coalescer_ttl() {
        let coalescer = LifecycleEventCoalescer::default();
        let request_id = "req-first-byte-persistence-after-ttl";
        let generation = coalescer
            .mark_first_byte(request_id)
            .await
            .expect("first-byte generation");
        {
            let shard = &coalescer.shards[coalescer.shard_index(request_id)];
            let mut entries = shard.entries.lock().await;
            entries
                .get_mut(request_id)
                .expect("first-byte marker")
                .first_byte_seen_at = Some(Instant::now() - Duration::from_secs(31));
        }
        *coalescer.shards[coalescer.shard_index(request_id)]
            .next_compaction_at
            .lock()
            .expect("coalescer compaction deadline") =
            Some(Instant::now() - Duration::from_secs(1));
        coalescer
            .register("req-trigger-unrelated-compaction".to_string())
            .await
            .expect("unrelated lifecycle generation");

        assert!(
            coalescer
                .first_byte_is_current(request_id, generation)
                .await,
            "the coalescer TTL must not become a persistence deadline"
        );
        coalescer.cancel(request_id).await;
        assert!(
            !coalescer
                .first_byte_is_current(request_id, generation)
                .await,
            "a terminal marker must still cancel the accepted first-byte write"
        );
    }

    #[tokio::test]
    async fn completed_first_byte_marker_expires_after_coalescer_ttl() {
        let coalescer = LifecycleEventCoalescer::default();
        let request_id = "req-first-byte-completed-marker-ttl";
        let generation = coalescer
            .mark_first_byte(request_id)
            .await
            .expect("first-byte generation");
        coalescer.complete_first_byte(request_id, generation).await;
        {
            let shard = &coalescer.shards[coalescer.shard_index(request_id)];
            let mut entries = shard.entries.lock().await;
            entries
                .get_mut(request_id)
                .expect("completed first-byte marker")
                .first_byte_seen_at = Some(Instant::now() - Duration::from_secs(31));
        }

        assert!(
            coalescer.mark_first_byte(request_id).await.is_some(),
            "completed first-byte markers should only deduplicate for the configured TTL"
        );
    }

    #[tokio::test]
    async fn lifecycle_coalescer_hard_bounds_large_burst_without_repeated_compaction() {
        const CAPACITY: usize = 32;
        const BURST: usize = CAPACITY * 4;
        let coalescer = LifecycleEventCoalescer::new(CAPACITY);
        for shard in &coalescer.shards {
            *shard
                .next_compaction_at
                .lock()
                .expect("coalescer compaction deadline") =
                Some(Instant::now() + Duration::from_secs(60));
        }

        let mut accepted = 0;
        for index in 0..BURST {
            if coalescer
                .mark_first_byte(&format!("req-coalescer-burst-{index}"))
                .await
                .is_some()
            {
                accepted += 1;
            }
        }
        assert_eq!(accepted, CAPACITY);
        assert_eq!(coalescer.entry_count.load(Ordering::Acquire), CAPACITY);
        assert_eq!(coalescer.admission.available_permits(), 0);
        assert_eq!(
            coalescer.rejected_total.load(Ordering::Acquire),
            (BURST - CAPACITY) as u64
        );
        assert_eq!(coalescer.compact_total.load(Ordering::Acquire), 0);

        for shard in &coalescer.shards {
            *shard
                .next_compaction_at
                .lock()
                .expect("coalescer compaction deadline") =
                Some(Instant::now() - Duration::from_secs(1));
        }
        let mut compacted_shards = 0_u64;
        let now = Instant::now();
        for (shard_index, shard) in coalescer.shards.iter().enumerate() {
            let mut entries = shard.entries.lock().await;
            compacted_shards += u64::from(!entries.is_empty());
            coalescer.compact_if_due(shard_index, &mut entries, now);
        }

        assert_eq!(
            coalescer.compact_total.load(Ordering::Acquire),
            compacted_shards
        );
        assert_eq!(
            coalescer
                .compact_entries_scanned_total
                .load(Ordering::Acquire),
            CAPACITY as u64
        );
        assert_eq!(
            coalescer.entry_count.load(Ordering::Acquire),
            CAPACITY,
            "in-flight first-byte markers must not be evicted by TTL compaction"
        );
    }

    #[tokio::test]
    async fn terminal_marker_evicts_completed_coalescer_entry_at_capacity() {
        const CAPACITY: usize = 2;
        let coalescer = LifecycleEventCoalescer::new(CAPACITY);
        let first_request_id = "req-coalescer-completed-first";
        let second_request_id = "req-coalescer-completed-second";

        let first_generation = coalescer
            .mark_first_byte(first_request_id)
            .await
            .expect("first completed marker should be admitted");
        coalescer
            .complete_first_byte(first_request_id, first_generation)
            .await;
        let second_generation = coalescer
            .mark_first_byte(second_request_id)
            .await
            .expect("second completed marker should be admitted");
        coalescer
            .complete_first_byte(second_request_id, second_generation)
            .await;
        assert_eq!(coalescer.admission.available_permits(), 0);

        let terminal_request_id = "req-coalescer-terminal-at-capacity";
        coalescer.cancel(terminal_request_id).await;

        let entries = coalescer.shards.iter().fold(0, |count, shard| {
            count
                + shard
                    .entries
                    .try_lock()
                    .expect("coalescer shards should be unlocked after cancellation")
                    .len()
        });
        assert_eq!(entries, CAPACITY);
        assert_eq!(coalescer.entry_count.load(Ordering::Acquire), CAPACITY);
        assert_eq!(coalescer.admission.available_permits(), 0);
        assert!(
            coalescer.shards[coalescer.shard_index(terminal_request_id)]
                .entries
                .lock()
                .await
                .contains_key(terminal_request_id),
            "a terminal marker should replace a completed marker when the cap is full"
        );
    }

    #[tokio::test]
    async fn completed_first_byte_markers_do_not_throttle_new_intermediates_at_capacity() {
        const CAPACITY: usize = 2;
        let coalescer = LifecycleEventCoalescer::new(CAPACITY);
        for request_id in ["req-completed-cap-first", "req-completed-cap-second"] {
            let generation = coalescer
                .mark_first_byte(request_id)
                .await
                .expect("completed first-byte marker should be admitted");
            coalescer.complete_first_byte(request_id, generation).await;
        }
        assert_eq!(coalescer.admission.available_permits(), 0);

        assert!(
            coalescer
                .mark_first_byte("req-new-first-byte-at-capacity")
                .await
                .is_some(),
            "a new first byte should replace a completed tombstone"
        );
        assert!(
            coalescer
                .register("req-new-delayed-at-capacity".to_string())
                .await
                .is_some(),
            "a new delayed intermediate should replace the remaining completed tombstone"
        );
        assert_eq!(coalescer.entry_count.load(Ordering::Acquire), CAPACITY);
        assert_eq!(coalescer.admission.available_permits(), 0);
        assert_eq!(coalescer.rejected_total.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn terminal_marker_evicts_delayed_intermediate_at_capacity() {
        const CAPACITY: usize = 2;
        let coalescer = LifecycleEventCoalescer::new(CAPACITY);
        let delayed_request_ids = ["req-delayed-capacity-first", "req-delayed-capacity-second"];
        for request_id in delayed_request_ids {
            coalescer
                .register(request_id.to_string())
                .await
                .expect("delayed intermediate should be admitted");
        }
        assert_eq!(coalescer.admission.available_permits(), 0);

        let terminal_request_id = "req-terminal-replaces-delayed";
        coalescer.cancel(terminal_request_id).await;

        assert!(
            coalescer.shards[coalescer.shard_index(terminal_request_id)]
                .entries
                .lock()
                .await
                .contains_key(terminal_request_id),
            "terminal admission must displace a delayed intermediate when the cap is full"
        );
        let retained_delayed = async {
            let mut retained = 0;
            for request_id in delayed_request_ids {
                if coalescer.shards[coalescer.shard_index(request_id)]
                    .entries
                    .lock()
                    .await
                    .contains_key(request_id)
                {
                    retained += 1;
                }
            }
            retained
        }
        .await;
        assert_eq!(retained_delayed, CAPACITY - 1);
        assert_eq!(coalescer.entry_count.load(Ordering::Acquire), CAPACITY);
        assert_eq!(coalescer.admission.available_permits(), 0);
    }

    #[tokio::test]
    async fn terminal_marker_does_not_evict_active_terminal_marker() {
        let coalescer = LifecycleEventCoalescer::new(1);
        let active_terminal_request_id = "req-active-terminal-marker";
        coalescer.cancel(active_terminal_request_id).await;

        let rejected_terminal_request_id = "req-terminal-marker-rejected-at-capacity";
        coalescer.cancel(rejected_terminal_request_id).await;

        assert!(
            coalescer.shards[coalescer.shard_index(active_terminal_request_id)]
                .entries
                .lock()
                .await
                .contains_key(active_terminal_request_id),
            "an active terminal marker must retain its TTL protection"
        );
        assert!(
            !coalescer.shards[coalescer.shard_index(rejected_terminal_request_id)]
                .entries
                .lock()
                .await
                .contains_key(rejected_terminal_request_id),
            "a new marker must not replace an active terminal marker"
        );
        assert_eq!(coalescer.entry_count.load(Ordering::Acquire), 1);
        assert_eq!(coalescer.admission.available_permits(), 0);
        assert_eq!(coalescer.rejected_total.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn terminal_event_cancels_delayed_lifecycle_enqueue() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 50,
            stream_key: "usage:events:test:pending-cancel".to_string(),
            consumer_group: "usage_consumers_test_pending_cancel".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Pending,
                    "req-lifecycle-cancel",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        runtime
            .record_terminal_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    "req-lifecycle-cancel",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        total_tokens: Some(12),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        sleep(Duration::from_millis(80)).await;
        let entries = queue
            .read_group("usage-test-consumer-cancel")
            .await
            .expect("queue read should succeed");
        assert_eq!(entries.len(), 1);
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued usage event should parse");
        assert_eq!(event.request_id, "req-lifecycle-cancel");
        assert_eq!(event.event_type, UsageEventType::Completed);
    }

    #[tokio::test]
    async fn first_byte_batch_panic_rolls_back_marker_admission() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(1),
            enqueue_retry_buffer_capacity: 1,
            ..UsageRuntimeConfig::default()
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_id = "req-first-byte-marker-panic";
        let generation = runtime
            .lifecycle_coalescer
            .mark_first_byte(request_id)
            .await
            .expect("first-byte marker should be admitted");
        let record = super::build_upsert_usage_record_from_event(&UsageEvent::new(
            UsageEventType::Streaming,
            request_id,
            UsageEventData {
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                first_byte_time_ms: Some(12),
                ..UsageEventData::default()
            },
        ))
        .expect("first-byte record should build");

        runtime
            .first_byte_persistence
            .dispatch(Box::new(PanickingFirstBytePersistenceItem {
                _marker_guard: super::FirstByteMarkerGuard::new(
                    Arc::clone(&runtime.lifecycle_coalescer),
                    request_id.to_string(),
                    generation,
                ),
                record,
            }))
            .await;

        timeout(Duration::from_secs(1), async {
            while runtime
                .first_byte_persistence
                .state
                .pending
                .load(Ordering::Acquire)
                != 0
                || runtime.lifecycle_coalescer.admission.available_permits() != 1
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("a panicked first-byte batch should release marker admission");
        assert!(!runtime.lifecycle_coalescer.shards
            [runtime.lifecycle_coalescer.shard_index(request_id)]
        .entries
        .lock()
        .await
        .contains_key(request_id));
        assert!(
            runtime
                .lifecycle_coalescer
                .mark_first_byte("req-first-byte-after-panic")
                .await
                .is_some(),
            "new first-byte work should be admitted after panic cleanup"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn non_batch_first_byte_writes_use_configured_bounded_concurrency() {
        const CONCURRENCY: usize = 16;
        const REQUESTS: usize = CONCURRENCY;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(CONCURRENCY),
            enqueue_retry_buffer_capacity: REQUESTS * 2,
            ..UsageRuntimeConfig::default()
        };
        let store = AdmissionBlockedUsageStore {
            release_writes: Arc::new(tokio::sync::Semaphore::new(0)),
            writes_in_flight: Arc::new(AtomicUsize::new(0)),
            max_writes_in_flight: Arc::new(AtomicUsize::new(0)),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        assert!(!UsageRecordWriter::supports_first_byte_usage_batch(&store));
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        for index in 0..REQUESTS {
            let request_id = format!("req-non-batch-first-byte-{index}");
            let seed = build_lifecycle_usage_seed(&terminal_test_plan(&request_id), None);
            runtime.record_stream_started(
                &store,
                &seed,
                200,
                Some(&ExecutionTelemetry {
                    ttfb_ms: Some(12),
                    elapsed_ms: Some(20),
                    upstream_bytes: Some(1),
                }),
            );
        }

        timeout(Duration::from_secs(2), async {
            while store.max_writes_in_flight.load(Ordering::Acquire) != CONCURRENCY {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("non-batch first-byte writes should fill configured concurrency");
        assert_eq!(store.writes_in_flight.load(Ordering::Acquire), CONCURRENCY);
        assert_eq!(
            runtime
                .worker_record_gate
                .as_ref()
                .expect("record gate should be configured")
                .max_in_flight(),
            CONCURRENCY
        );

        store.release_writes.add_permits(REQUESTS);
        timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if store.writes_completed.load(Ordering::Acquire) == REQUESTS
                    && snapshot.lifecycle_submission_pending == 0
                    && snapshot.ordered_lifecycle_pending == 0
                    && snapshot.first_byte_persistence_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("non-batch first-byte writes should drain after release");
        assert_eq!(store.writes_in_flight.load(Ordering::Acquire), 0);
        assert_eq!(
            store.max_writes_in_flight.load(Ordering::Acquire),
            CONCURRENCY
        );
    }

    #[tokio::test]
    async fn first_byte_dispatcher_batches_records_for_one_writer() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(4),
            stream_key: "usage:events:test:first-byte-batch".to_string(),
            consumer_group: "usage_consumers_test_first_byte_batch".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let store = BatchingQueueUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        for index in 0..32 {
            runtime
                .enqueue_or_write_lifecycle(
                    &store,
                    UsageEvent::new(
                        UsageEventType::Streaming,
                        format!("req-first-byte-batch-{index}"),
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            status_code: Some(200),
                            first_byte_time_ms: Some(12 + index as u64),
                            ..UsageEventData::default()
                        },
                    ),
                )
                .await;
        }

        timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if snapshot.first_byte_persistence_pending == 0
                    && snapshot.first_byte_persistence_direct_succeeded_total == 32
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first-byte batch dispatcher should drain");

        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 32);
        let snapshot = runtime.metrics_snapshot();
        assert!(snapshot.first_byte_persistence_batch_flush_total >= 1);
        assert_eq!(snapshot.first_byte_persistence_batch_records_total, 32);
        assert!(snapshot.first_byte_persistence_max_batch_size >= 2);
        assert_eq!(snapshot.first_byte_persistence_batch_failed_total, 0);
    }

    #[tokio::test]
    async fn stream_started_direct_with_first_byte_bypasses_queue_backlog_and_lifecycle_delay() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 30,
            stream_key: "usage:events:test:stream-direct-delay".to_string(),
            consumer_group: "usage_consumers_test_stream_direct_delay".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let plan = ExecutionPlan {
            request_id: "req-stream-direct-delay".to_string(),
            candidate_id: Some("cand-stream-direct-delay".to_string()),
            provider_name: Some("openai".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: true,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let lifecycle_seed = build_lifecycle_usage_seed(&plan, None);

        runtime
            .record_stream_started_direct(
                &store,
                &lifecycle_seed,
                200,
                Some(&ExecutionTelemetry {
                    ttfb_ms: Some(12),
                    elapsed_ms: Some(34),
                    upstream_bytes: Some(56),
                }),
            )
            .await;

        timeout(Duration::from_secs(1), async {
            while store.records.lock().expect("records lock").is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first-byte transition should be persisted directly");
        {
            let records = store.records.lock().expect("records lock");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].request_id, "req-stream-direct-delay");
            assert_eq!(records[0].status, "streaming");
            assert_eq!(records[0].first_byte_time_ms, Some(12));
        }

        sleep(Duration::from_millis(60)).await;
        let queued = queue
            .read_group("usage-test-consumer-stream-direct-delay-after-wait")
            .await
            .expect("queue read should succeed");
        assert!(
            queued.is_empty(),
            "a successful first-byte fast-path write must bypass the ordinary queue"
        );
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.first_byte_persistence_pending, 0);
        assert_eq!(snapshot.first_byte_persistence_direct_succeeded_total, 1);
        assert_eq!(snapshot.first_byte_persistence_direct_failed_total, 0);
    }

    #[tokio::test]
    async fn stream_started_without_first_byte_keeps_lifecycle_delay() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 30,
            stream_key: "usage:events:test:stream-no-first-byte-delay".to_string(),
            consumer_group: "usage_consumers_test_stream_no_first_byte_delay".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let event = UsageEvent::new(
            UsageEventType::Streaming,
            "req-stream-no-first-byte-delay",
            UsageEventData {
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                status_code: Some(200),
                first_byte_time_ms: None,
                ..UsageEventData::default()
            },
        );

        runtime.enqueue_or_write_lifecycle(&store, event).await;

        let immediate = queue
            .read_group("usage-test-consumer-stream-no-first-byte-immediate")
            .await
            .expect("queue read should succeed");
        assert!(
            immediate.is_empty(),
            "pre-first-byte streaming event should still be coalesced"
        );

        sleep(Duration::from_millis(60)).await;
        let entries = queue
            .read_group("usage-test-consumer-stream-no-first-byte-delayed")
            .await
            .expect("queue read should succeed");
        assert_eq!(entries.len(), 1);
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued usage event should parse");
        assert_eq!(event.request_id, "req-stream-no-first-byte-delay");
        assert_eq!(event.event_type, UsageEventType::Streaming);
        assert_eq!(event.data.first_byte_time_ms, None);
    }

    #[tokio::test]
    async fn first_byte_transition_supersedes_delayed_pending_event() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 40,
            stream_key: "usage:events:test:first-byte-supersedes-pending".to_string(),
            consumer_group: "usage_consumers_test_first_byte_supersedes_pending".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Pending,
                    "req-first-byte-supersedes-pending",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Streaming,
                    "req-first-byte-supersedes-pending",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        first_byte_time_ms: Some(12),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        timeout(Duration::from_secs(1), async {
            while store.records.lock().expect("records lock").is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first-byte transition should be persisted directly");
        {
            let records = store.records.lock().expect("records lock");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].status, "streaming");
            assert_eq!(records[0].first_byte_time_ms, Some(12));
        }

        sleep(Duration::from_millis(80)).await;
        let delayed = queue
            .read_group("usage-test-consumer-first-byte-after-delay")
            .await
            .expect("queue read should succeed");
        assert!(
            delayed.is_empty(),
            "the delayed pending event must not overwrite the first-byte transition"
        );
    }

    #[tokio::test]
    async fn first_byte_transition_uses_queue_when_node_has_no_usage_writer() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 1_000,
            stream_key: "usage:events:test:first-byte-queue-only-node".to_string(),
            consumer_group: "usage_consumers_test_first_byte_queue_only_node".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let upsert_attempts = Arc::new(AtomicUsize::new(0));
        let store = QueueOnlyUsageStore {
            queue: queue_runner,
            upsert_attempts: Arc::clone(&upsert_attempts),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Streaming,
                    "req-first-byte-queue-only-node",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        first_byte_time_ms: Some(12),
                        request_metadata: Some(json!({
                            "trace_id": "trace-queue-only-first-byte",
                            "billing_snapshot": {"source": "original-event"}
                        })),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        let entries = timeout(Duration::from_secs(1), async {
            loop {
                let entries = queue
                    .read_group("usage-test-consumer-first-byte-queue-only-node")
                    .await
                    .expect("queue read should succeed");
                if !entries.is_empty() {
                    break entries;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first-byte transition should be queued");
        assert_eq!(entries.len(), 1);
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued usage event should parse");
        assert_eq!(event.event_type, UsageEventType::Streaming);
        assert_eq!(
            event.data.request_metadata,
            Some(json!({
                "trace_id": "trace-queue-only-first-byte",
                "billing_snapshot": {"source": "original-event"}
            })),
            "queue-only nodes must preserve the original lifecycle event"
        );
        assert_eq!(upsert_attempts.load(Ordering::Acquire), 0);
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.first_byte_persistence_pending, 0);
        assert_eq!(snapshot.first_byte_persistence_fallback_accepted_total, 1);
        assert_eq!(snapshot.first_byte_persistence_fallback_failed_total, 0);
    }

    #[tokio::test]
    async fn terminal_cancellation_drops_buffered_first_byte_write() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            worker_record_concurrency_limit: Some(1),
            stream_key: "usage:events:test:first-byte-terminal-cancel".to_string(),
            consumer_group: "usage_consumers_test_first_byte_terminal_cancel".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let write_started = Arc::new(tokio::sync::Notify::new());
        let release_writes = Arc::new(tokio::sync::Notify::new());
        let writes_completed = Arc::new(AtomicUsize::new(0));
        let store = BlockingWriteQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
            write_started: Arc::clone(&write_started),
            release_writes: Arc::clone(&release_writes),
            writes_completed: Arc::clone(&writes_completed),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let first_write_started = write_started.notified();

        let streaming_event = |request_id| {
            UsageEvent::new(
                UsageEventType::Streaming,
                request_id,
                UsageEventData {
                    provider_name: "openai".to_string(),
                    model: "gpt-5".to_string(),
                    status_code: Some(200),
                    first_byte_time_ms: Some(12),
                    ..UsageEventData::default()
                },
            )
        };
        runtime
            .enqueue_or_write_lifecycle(&store, streaming_event("req-first-byte-blocking"))
            .await;
        timeout(Duration::from_secs(1), first_write_started)
            .await
            .expect("first fast-path write should start");
        runtime
            .enqueue_or_write_lifecycle(&store, streaming_event("req-first-byte-cancelled"))
            .await;
        runtime
            .lifecycle_coalescer
            .cancel("req-first-byte-cancelled")
            .await;
        release_writes.notify_one();
        timeout(Duration::from_secs(1), async {
            while writes_completed.load(Ordering::Acquire) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first fast-path write should finish");
        sleep(Duration::from_millis(50)).await;
        assert_eq!(writes_completed.load(Ordering::Acquire), 1);
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.first_byte_persistence_pending, 0);
        assert_eq!(snapshot.first_byte_persistence_cancelled_total, 1);
    }

    #[tokio::test]
    async fn queued_terminal_keeps_inflight_first_byte_persistence_alive() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            queue_lifecycle_events: true,
            stream_key: "usage:events:test:first-byte-queued-terminal".to_string(),
            consumer_group: "usage_consumers_test_first_byte_queued_terminal".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let write_started = Arc::new(tokio::sync::Notify::new());
        let release_writes = Arc::new(tokio::sync::Notify::new());
        let writes_completed = Arc::new(AtomicUsize::new(0));
        let store = BlockingWriteQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
            write_started: Arc::clone(&write_started),
            release_writes: Arc::clone(&release_writes),
            writes_completed: Arc::clone(&writes_completed),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_id = "req-first-byte-queued-terminal";

        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Streaming,
                    request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        first_byte_time_ms: Some(12),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        timeout(Duration::from_secs(1), write_started.notified())
            .await
            .expect("first-byte write should start");

        runtime
            .enqueue_or_write_terminal(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        release_writes.notify_one();

        timeout(Duration::from_secs(1), async {
            while writes_completed.load(Ordering::Acquire) != 1
                || runtime
                    .metrics_snapshot()
                    .first_byte_persistence_direct_succeeded_total
                    != 1
                || runtime.metrics_snapshot().first_byte_persistence_pending != 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("queued terminal must not cancel the in-flight first-byte write");
        assert_eq!(
            runtime
                .metrics_snapshot()
                .first_byte_persistence_direct_succeeded_total,
            1
        );
        let entries = queue
            .read_group("usage-test-first-byte-queued-terminal")
            .await
            .expect("queued terminal should be readable");
        assert_eq!(entries.len(), 1);
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued terminal should parse");
        assert_eq!(event.event_type, UsageEventType::Completed);
    }

    #[tokio::test]
    async fn first_byte_append_failure_is_retried_locally() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 1_000,
            retry_deferred_lifecycle_events: true,
            stream_key: "usage:events:test:first-byte-retry".to_string(),
            consumer_group: "usage_consumers_test_first_byte_retry".to_string(),
            consumer_block_ms: 1,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 5,
            enqueue_retry_buffer_capacity: 16,
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&inner_queue), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let flaky_queue: Arc<dyn RuntimeQueueStore> = Arc::new(
            FlakyAppendQueueStore::new(Arc::clone(&inner_queue), 1).with_append_delay_ms(200),
        );
        let upsert_attempts = Arc::new(AtomicUsize::new(0));
        let store = FailingWriteQueueConfiguredUsageStore {
            queue: flaky_queue,
            upsert_attempts: Arc::clone(&upsert_attempts),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_metadata = json!({
            "trace_id": "trace-first-byte-retry",
            "upstream_is_stream": true,
            "billing_snapshot": {"status": "pending", "source": "full-event"},
            "stage_timings_ms": {"planning": 7}
        });

        timeout(
            Duration::from_millis(100),
            runtime.enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Streaming,
                    "req-first-byte-retry",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        first_byte_time_ms: Some(12),
                        request_headers: Some(json!({"x-request-id": "full-event"})),
                        request_body: Some(json!({"messages": [{"content": "hello"}]})),
                        request_body_state: Some(UsageBodyCaptureState::Inline),
                        provider_request_headers: Some(json!({"x-provider": "full-event"})),
                        provider_request_body: Some(json!({"model": "gpt-5"})),
                        provider_request_body_state: Some(UsageBodyCaptureState::Inline),
                        response_headers: Some(json!({"content-type": "text/event-stream"})),
                        response_body_state: Some(UsageBodyCaptureState::Unavailable),
                        request_metadata: Some(request_metadata.clone()),
                        ..UsageEventData::default()
                    },
                ),
            ),
        )
        .await
        .expect("first-byte submission must not wait for the Redis append");

        wait_for_enqueue_dispatcher_to_drain(&runtime, 1).await;
        assert_eq!(upsert_attempts.load(Ordering::Acquire), 1);
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.first_byte_persistence_batch_failed_total, 1);
        assert_eq!(snapshot.first_byte_persistence_direct_failed_total, 1);
        let entries = queue
            .read_group("usage-test-consumer-first-byte-retry")
            .await
            .expect("queue read should succeed");
        assert_eq!(entries.len(), 1);
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued usage event should parse");
        assert_eq!(event.event_type, UsageEventType::Streaming);
        assert_eq!(event.data.first_byte_time_ms, Some(12));
        assert_eq!(
            event.data.request_headers,
            Some(json!({"x-request-id": "full-event"}))
        );
        assert_eq!(
            event.data.request_body,
            Some(json!({"messages": [{"content": "hello"}]}))
        );
        assert_eq!(
            event.data.request_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
        assert_eq!(
            event.data.provider_request_headers,
            Some(json!({"x-provider": "full-event"}))
        );
        assert_eq!(
            event.data.provider_request_body,
            Some(json!({"model": "gpt-5"}))
        );
        assert_eq!(
            event.data.provider_request_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
        assert_eq!(
            event.data.response_headers,
            Some(json!({"content-type": "text/event-stream"}))
        );
        assert_eq!(
            event.data.response_body_state,
            Some(UsageBodyCaptureState::Unavailable)
        );
        assert_eq!(event.data.request_metadata, Some(request_metadata));
    }

    #[tokio::test]
    async fn direct_terminal_event_cancels_delayed_lifecycle_enqueue() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 50,
            stream_key: "usage:events:test:direct-terminal-cancel".to_string(),
            consumer_group: "usage_consumers_test_direct_terminal_cancel".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        runtime
            .enqueue_or_write_lifecycle(
                &store,
                UsageEvent::new(
                    UsageEventType::Pending,
                    "req-direct-terminal-cancel",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        runtime
            .record_terminal_event_direct(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    "req-direct-terminal-cancel",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        total_tokens: Some(12),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        sleep(Duration::from_millis(80)).await;
        let entries = queue
            .read_group("usage-test-consumer-direct-terminal-cancel")
            .await
            .expect("queue read should succeed");
        assert!(
            entries.is_empty(),
            "direct terminal usage should cancel delayed lifecycle queue writes"
        );
        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request_id, "req-direct-terminal-cancel");
        assert_eq!(records[0].status, "completed");
    }

    #[test]
    fn spawn_workers_uses_configured_worker_count() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            worker_count: 3,
            stream_key: "usage:events:test:worker-count".to_string(),
            consumer_group: "usage_consumers_test_worker_count".to_string(),
            ..UsageRuntimeConfig::default()
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };

        let handles = runtime.spawn_workers(Arc::new(store));
        assert_eq!(handles.len(), 3);
        for handle in handles {
            handle.abort();
        }
    }

    #[test]
    fn worker_supervisor_state_records_worker_observations() {
        let state = UsageWorkerSupervisorState::default();

        state.record_observation(UsageWorkerObservation {
            worker_index: Some(1),
            entries_read: 2,
            batch_size: 16,
            reclaimed_entries: 1,
            acked_entries: 2,
            dead_lettered_entries: 1,
            process_failures: 1,
            read_failures: 1,
            reclaim_failures: 1,
        });

        assert_eq!(state.read_batches_total.load(Ordering::Acquire), 1);
        assert_eq!(state.read_entries_total.load(Ordering::Acquire), 2);
        assert_eq!(state.reclaimed_entries_total.load(Ordering::Acquire), 1);
        assert_eq!(state.acked_entries_total.load(Ordering::Acquire), 2);
        assert_eq!(state.dead_lettered_entries_total.load(Ordering::Acquire), 1);
        assert_eq!(state.process_failures_total.load(Ordering::Acquire), 1);
        assert_eq!(state.read_failures_total.load(Ordering::Acquire), 1);
        assert_eq!(state.reclaim_failures_total.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn worker_supervisor_scales_up_when_reads_stay_full() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            worker_count: 1,
            worker_autoscale_enabled: true,
            worker_max_count: 4,
            worker_scale_interval_ms: 10,
            worker_idle_scale_down_ticks: 100,
            stream_key: "usage:events:test:worker-autoscale-up".to_string(),
            consumer_group: "usage_consumers_test_worker_autoscale_up".to_string(),
            consumer_batch_size: 1,
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        for index in 0..32 {
            queue
                .enqueue(&UsageEvent::new(
                    UsageEventType::Completed,
                    format!("req-worker-autoscale-up-{index}"),
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        total_tokens: Some(12),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ))
                .await
                .expect("usage event should enqueue");
        }
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        let supervisor = runtime
            .spawn_worker_supervisor(Arc::new(store))
            .expect("supervisor should spawn");
        for _ in 0..100 {
            let snapshot = runtime.metrics_snapshot();
            if snapshot.worker_desired_count > 1 {
                supervisor.abort();
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        supervisor.abort();

        let snapshot = runtime.metrics_snapshot();
        assert!(
            snapshot.worker_desired_count > 1,
            "usage worker supervisor should scale up after repeated full reads: {snapshot:?}"
        );
    }

    #[tokio::test]
    async fn worker_supervisor_replaces_worker_after_panic() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            worker_count: 1,
            worker_autoscale_enabled: false,
            worker_max_count: 1,
            worker_scale_interval_ms: 10,
            stream_key: "usage:events:test:worker-panic-recovery".to_string(),
            consumer_group: "usage_consumers_test_worker_panic_recovery".to_string(),
            consumer_batch_size: 1,
            consumer_block_ms: 1,
            reclaim_idle_ms: 60_000,
            reclaim_interval_ms: 60_000,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        queue
            .enqueue(&UsageEvent::new(
                UsageEventType::Completed,
                "req-worker-panic-first",
                UsageEventData {
                    provider_name: "openai".to_string(),
                    model: "gpt-5".to_string(),
                    total_tokens: Some(12),
                    status_code: Some(200),
                    ..UsageEventData::default()
                },
            ))
            .await
            .expect("first usage event should enqueue");

        let records = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(PanicOnceQueueConfiguredUsageStore {
            inner: CloneQueueConfiguredUsageStore {
                records: Arc::clone(&records),
                queue: queue_runner,
            },
            remaining_panics: AtomicUsize::new(1),
        });
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let supervisor = runtime
            .spawn_worker_supervisor(Arc::clone(&store))
            .expect("supervisor should spawn");

        for _ in 0..100 {
            if store.remaining_panics.load(Ordering::Acquire) == 0 {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            store.remaining_panics.load(Ordering::Acquire),
            0,
            "first worker should panic while processing the first event"
        );

        queue
            .enqueue(&UsageEvent::new(
                UsageEventType::Completed,
                "req-worker-panic-second",
                UsageEventData {
                    provider_name: "openai".to_string(),
                    model: "gpt-5".to_string(),
                    total_tokens: Some(24),
                    status_code: Some(200),
                    ..UsageEventData::default()
                },
            ))
            .await
            .expect("second usage event should enqueue");

        for _ in 0..100 {
            let recorded_second = records
                .lock()
                .expect("records lock")
                .iter()
                .any(|record| record.request_id == "req-worker-panic-second");
            if recorded_second {
                supervisor.abort();
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
        supervisor.abort();

        let records = records.lock().expect("records lock");
        assert!(
            records
                .iter()
                .any(|record| record.request_id == "req-worker-panic-second"),
            "replacement worker should consume events after the first worker panics: {records:?}"
        );
    }

    #[tokio::test]
    async fn ordered_pending_bypasses_lifecycle_queue_append_failure() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            lifecycle_enqueue_delay_ms: 0,
            retry_deferred_lifecycle_events: false,
            stream_key: "usage:events:test:lifecycle-failure".to_string(),
            consumer_group: "usage_consumers_test_lifecycle_failure".to_string(),
            consumer_block_ms: 1,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 5,
            enqueue_retry_buffer_capacity: 16,
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let flaky_queue: Arc<dyn RuntimeQueueStore> = Arc::new(FlakyAppendQueueStore::new(
            Arc::clone(&inner_queue),
            usize::MAX,
        ));
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: flaky_queue,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let plan = ExecutionPlan {
            request_id: "req-lifecycle-queue-failure-1".to_string(),
            candidate_id: Some("cand-lifecycle-queue-failure-1".to_string()),
            provider_name: Some("openai".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        runtime.record_pending(&store, build_lifecycle_usage_seed(&plan, None));
        sleep(Duration::from_millis(50)).await;

        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "pending");
        drop(records);
        assert_eq!(
            runtime.metrics_snapshot().lifecycle_enqueue_failed_total,
            0,
            "ordered pending persistence should not attempt the lifecycle queue"
        );
    }

    #[tokio::test]
    async fn lifecycle_enqueue_failure_opens_short_circuit() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            retry_deferred_lifecycle_events: false,
            stream_key: "usage:events:test:lifecycle-circuit".to_string(),
            consumer_group: "usage_consumers_test_lifecycle_circuit".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let flaky_queue = Arc::new(FlakyAppendQueueStore::new(
            Arc::clone(&inner_queue),
            usize::MAX,
        ));
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: flaky_queue.clone(),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        for index in 0..10 {
            let event = UsageEvent::new(
                UsageEventType::Pending,
                format!("req-lifecycle-circuit-{index}"),
                UsageEventData {
                    provider_name: "openai".to_string(),
                    model: "gpt-5".to_string(),
                    ..UsageEventData::default()
                },
            );
            runtime.enqueue_lifecycle_event(&store, event).await;
        }

        assert_eq!(
            flaky_queue.append_attempts.load(Ordering::Acquire),
            1,
            "lifecycle enqueue circuit should prevent repeated Redis appends after a failure"
        );
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(
            snapshot.lifecycle_enqueue_failed_total, 1,
            "first append failure should open the lifecycle enqueue circuit"
        );
        assert_eq!(
            snapshot.lifecycle_enqueue_deferred_dropped_total, 10,
            "the failed primary event and circuit-deferred events should be counted as dropped"
        );
        assert_eq!(
            snapshot.enqueue_retry_scheduled_total, 0,
            "default lifecycle overload policy must not schedule local enqueue retries"
        );
        assert!(
            store.records.lock().expect("records lock").is_empty(),
            "lifecycle enqueue circuit must not fall back to direct DB writes"
        );
    }

    #[tokio::test]
    async fn lifecycle_deferred_retry_can_be_enabled() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            retry_deferred_lifecycle_events: true,
            stream_key: "usage:events:test:lifecycle-deferred-retry".to_string(),
            consumer_group: "usage_consumers_test_lifecycle_deferred_retry".to_string(),
            consumer_block_ms: 1,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 5,
            enqueue_retry_buffer_capacity: 16,
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let flaky_queue = Arc::new(FlakyAppendQueueStore::new(Arc::clone(&inner_queue), 1));
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: flaky_queue,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        runtime
            .enqueue_lifecycle_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Pending,
                    "req-lifecycle-deferred-retry-open",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        runtime
            .enqueue_lifecycle_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Pending,
                    "req-lifecycle-deferred-retry-scheduled",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.lifecycle_enqueue_deferred_retry_total, 2);
        assert_eq!(snapshot.lifecycle_enqueue_deferred_dropped_total, 0);
        assert_eq!(
            snapshot.enqueue_retry_scheduled_total, 2,
            "the initial append failure and the circuit-deferred event should both retry"
        );
    }

    #[tokio::test]
    async fn lifecycle_retry_buffer_full_drops_without_waiting_for_capacity() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
            enqueue_retry_buffer_capacity: 1,
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(runner, config.clone()).expect("usage queue should build");
        let (sender, _receiver) = mpsc::channel(1);
        let dispatcher = UsageEnqueueRetryDispatcher {
            senders: vec![sender],
            metrics: Arc::new(Default::default()),
        };
        let event = |request_id: &str| {
            UsageEvent::new(
                UsageEventType::Streaming,
                request_id,
                UsageEventData {
                    provider_name: "openai".to_string(),
                    model: "gpt-5".to_string(),
                    first_byte_time_ms: Some(12),
                    ..UsageEventData::default()
                },
            )
        };

        assert!(dispatcher.schedule(
            queue.clone(),
            event("req-lifecycle-retry-buffer-fill"),
            "lifecycle",
            DataLayerError::Redis("forced failure".to_string()),
        ));
        let started_at = std::time::Instant::now();
        let accepted = dispatcher.schedule(
            queue,
            event("req-lifecycle-retry-buffer-overflow"),
            "lifecycle",
            DataLayerError::Redis("forced failure".to_string()),
        );
        assert!(
            started_at.elapsed() < Duration::from_millis(50),
            "retry submission must remain non-blocking"
        );
        assert!(!accepted);
        assert_eq!(dispatcher.scheduled_total(), 1);
        assert_eq!(dispatcher.pending(), 1);
        assert_eq!(dispatcher.closed_or_unavailable_total(), 1);
    }

    #[tokio::test]
    async fn disabled_lifecycle_queue_writes_pending_directly() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: false,
            stream_key: "usage:events:test:lifecycle-disabled".to_string(),
            consumer_group: "usage_consumers_test_lifecycle_disabled".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: queue_runner,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let plan = ExecutionPlan {
            request_id: "req-lifecycle-disabled-1".to_string(),
            candidate_id: Some("cand-lifecycle-disabled-1".to_string()),
            provider_name: Some("openai".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        runtime.record_pending(&store, build_lifecycle_usage_seed(&plan, None));
        sleep(Duration::from_millis(50)).await;

        let records = store.records.lock().expect("records lock");
        let record = records
            .first()
            .expect("disabled lifecycle queue should direct-write pending usage");
        assert_eq!(record.request_id, "req-lifecycle-disabled-1");
        assert_eq!(record.status, "pending");
        assert_eq!(record.billing_status, "pending");
    }

    #[tokio::test]
    async fn queued_terminal_usage_does_not_enrich_or_write_before_enqueue() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            stream_key: "usage:events:test:terminal".to_string(),
            consumer_group: "usage_consumers_test_terminal".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue_runner: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue = UsageQueue::new(Arc::clone(&queue_runner), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = EnrichmentCountingQueueStore {
            records: Mutex::new(Vec::new()),
            queue: queue_runner,
            enrich_calls: AtomicUsize::new(0),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let event = UsageEvent::new(
            UsageEventType::Completed,
            "req-terminal-queue-1",
            UsageEventData {
                user_id: Some("user-terminal-queue-1".to_string()),
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                input_tokens: Some(4),
                output_tokens: Some(8),
                total_tokens: Some(12),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        runtime.record_terminal_event(&store, event).await;

        assert_eq!(store.enrich_calls.load(Ordering::Acquire), 0);
        assert!(store.records.lock().expect("records lock").is_empty());
        assert_eq!(
            runtime.metrics_snapshot().enqueue_retry_scheduled_total,
            0,
            "healthy terminal enqueue should be durable before returning"
        );
        let entries = queue
            .read_group("usage-test-terminal-consumer")
            .await
            .expect("queue read should succeed");
        let entry = entries.first().expect("terminal event should be queued");
        let queued = UsageEvent::from_stream_fields(&entry.fields)
            .expect("queued terminal event should parse");
        assert_eq!(queued.request_id, "req-terminal-queue-1");
        assert_eq!(queued.event_type, UsageEventType::Completed);
        assert_eq!(queued.data.total_cost_usd, None);
    }

    #[tokio::test]
    async fn terminal_enqueue_failure_uses_bounded_direct_database_fallback() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            stream_key: "usage:events:test:terminal-circuit".to_string(),
            consumer_group: "usage_consumers_test_terminal_circuit".to_string(),
            consumer_block_ms: 1,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 5,
            enqueue_retry_buffer_capacity: 16,
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let flaky_queue = Arc::new(FlakyAppendQueueStore::new(Arc::clone(&inner_queue), 2));
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: flaky_queue.clone(),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        for index in 0..2 {
            runtime
                .record_terminal_event(
                    &store,
                    UsageEvent::new(
                        UsageEventType::Completed,
                        format!("req-terminal-circuit-{index}"),
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            total_tokens: Some(12),
                            status_code: Some(200),
                            ..UsageEventData::default()
                        },
                    ),
                )
                .await;
        }

        let snapshot = runtime.metrics_snapshot();
        assert_eq!(
            snapshot.terminal_enqueue_failed_total, 1,
            "the first terminal submission should attempt Redis before opening the circuit"
        );
        assert_eq!(
            snapshot.terminal_enqueue_deferred_direct_write_total, 2,
            "the failed primary event and circuit-deferred event should use direct fallback"
        );
        assert_eq!(snapshot.terminal_enqueue_deferred_retry_total, 0);
        assert_eq!(snapshot.terminal_enqueue_deferred_dropped_total, 0);
        assert_eq!(snapshot.terminal_direct_fallback_succeeded_total, 2);
        assert_eq!(snapshot.enqueue_retry_scheduled_total, 0);
        assert_eq!(snapshot.enqueue_retry_recovered_total, 0);
        assert_eq!(snapshot.enqueue_retry_pending, 0);
        assert_eq!(snapshot.enqueue_retry_failed_total, 0);
        assert_eq!(snapshot.enqueue_retry_closed_or_unavailable_total, 0);
        assert_eq!(
            store.records.lock().expect("records lock").len(),
            2,
            "both terminal events should be persisted directly"
        );
        assert_eq!(flaky_queue.successful_appends.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn failed_terminal_persistence_does_not_cancel_buffered_first_byte() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            queue_lifecycle_events: true,
            stream_key: "usage:events:test:terminal-total-failure".to_string(),
            consumer_group: "usage_consumers_test_terminal_total_failure".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(FlakyAppendQueueStore::new(inner_queue, usize::MAX));
        let store = FailingWriteQueueConfiguredUsageStore {
            queue,
            upsert_attempts: Arc::new(AtomicUsize::new(0)),
        };
        let mut runtime = UsageRuntime::new(config).expect("usage runtime should build");
        runtime.enqueue_retry = UsageEnqueueRetryDispatcher::disabled();
        let request_id = "req-terminal-total-failure";
        let generation = runtime
            .lifecycle_coalescer
            .mark_first_byte(request_id)
            .await
            .expect("buffered first-byte generation");

        runtime
            .enqueue_or_write_terminal(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        assert!(
            runtime
                .lifecycle_coalescer
                .first_byte_is_current(request_id, generation)
                .await,
            "terminal loss must not discard the only remaining lifecycle transition"
        );
        assert_eq!(
            runtime
                .metrics_snapshot()
                .terminal_enqueue_deferred_dropped_total,
            1
        );

        let direct_request_id = "req-terminal-direct-total-failure";
        let direct_generation = runtime
            .lifecycle_coalescer
            .mark_first_byte(direct_request_id)
            .await
            .expect("direct-path buffered first-byte generation");
        runtime
            .record_terminal_event_direct(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    direct_request_id,
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        assert!(
            runtime
                .lifecycle_coalescer
                .first_byte_is_current(direct_request_id, direct_generation)
                .await,
            "failed direct terminal persistence must not cancel buffered first-byte state"
        );
    }

    #[tokio::test]
    async fn permanent_redis_and_database_failure_keeps_terminal_retry_tasks_bounded() {
        const TOTAL: usize = 32;
        const RETRY_CAPACITY: u64 = 1;
        const RETRY_WORKERS: u64 = 1;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: TOTAL as u64,
            terminal_enqueue_max_in_flight: 1,
            stream_key: "usage:events:test:terminal-permanent-failure".to_string(),
            consumer_group: "usage_consumers_test_terminal_permanent_failure".to_string(),
            consumer_block_ms: 1,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 5,
            enqueue_retry_buffer_capacity: RETRY_CAPACITY as usize,
            enqueue_retry_workers: RETRY_WORKERS as usize,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let flaky_queue = Arc::new(FlakyAppendQueueStore::new(
            Arc::clone(&inner_queue),
            usize::MAX,
        ));
        let queue: Arc<dyn RuntimeQueueStore> = flaky_queue.clone();
        let store = FailingWriteQueueConfiguredUsageStore {
            queue,
            upsert_attempts: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let mut submissions = tokio::task::JoinSet::new();

        for index in 0..TOTAL {
            let runtime = runtime.clone();
            let store = store.clone();
            submissions.spawn(async move {
                runtime
                    .record_terminal_event(
                        &store,
                        UsageEvent::new(
                            UsageEventType::Completed,
                            format!("req-terminal-permanent-failure-{index}"),
                            UsageEventData {
                                provider_name: "openai".to_string(),
                                model: "gpt-5".to_string(),
                                total_tokens: Some(12),
                                status_code: Some(200),
                                ..UsageEventData::default()
                            },
                        ),
                    )
                    .await;
            });
        }

        let completed_without_waiters = timeout(Duration::from_secs(1), async {
            while let Some(result) = submissions.join_next().await {
                result.expect("terminal submission should not panic");
            }
        })
        .await;
        let saturated_snapshot = runtime.metrics_snapshot();

        flaky_queue.remaining_failures.store(0, Ordering::Release);
        if completed_without_waiters.is_err() {
            submissions.abort_all();
            while submissions.join_next().await.is_some() {}
        }
        wait_for_enqueue_dispatcher_to_drain(
            &runtime,
            saturated_snapshot.enqueue_retry_scheduled_total,
        )
        .await;

        assert!(
            completed_without_waiters.is_ok(),
            "terminal submissions must never wait outside bounded fallback queues"
        );
        assert_eq!(
            saturated_snapshot.terminal_enqueue_deferred_total,
            TOTAL as u64
        );
        assert_eq!(
            saturated_snapshot.terminal_direct_fallback_failed_total,
            TOTAL as u64
        );
        assert_eq!(
            store.upsert_attempts.load(Ordering::Acquire),
            TOTAL,
            "each terminal event should receive one bounded direct write attempt"
        );
        assert_eq!(
            saturated_snapshot.terminal_enqueue_deferred_retry_total
                + saturated_snapshot.terminal_enqueue_deferred_dropped_total,
            TOTAL as u64
        );
        assert_eq!(
            saturated_snapshot.enqueue_retry_scheduled_total,
            saturated_snapshot.terminal_enqueue_deferred_retry_total
        );
        assert!(
            saturated_snapshot.enqueue_retry_pending <= RETRY_CAPACITY + RETRY_WORKERS,
            "retry pending events must stay within active workers plus channel capacity"
        );
        assert!(
            saturated_snapshot.terminal_enqueue_deferred_dropped_total > 0,
            "the test must saturate the bounded retry buffer"
        );
        assert_eq!(
            saturated_snapshot.enqueue_retry_closed_or_unavailable_total,
            saturated_snapshot.terminal_enqueue_deferred_dropped_total
        );
        assert!(saturated_snapshot.terminal_direct_fallback_max_in_flight <= 1);
        assert_eq!(saturated_snapshot.terminal_direct_fallback_in_flight, 0);
        assert_eq!(saturated_snapshot.terminal_enqueue_in_flight, 0);
    }

    #[tokio::test]
    async fn slow_database_fallback_backpressures_excess_at_hard_capacity_and_recovers() {
        const EXCESS_SUBMISSIONS: usize = 16;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: (EXCESS_SUBMISSIONS + 1) as u64,
            terminal_enqueue_max_in_flight: 1,
            stream_key: "usage:events:test:terminal-slow-db-fallback".to_string(),
            consumer_group: "usage_consumers_test_terminal_slow_db_fallback".to_string(),
            consumer_block_ms: 1,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 5,
            enqueue_retry_buffer_capacity: 1,
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let flaky_queue = Arc::new(FlakyAppendQueueStore::new(
            Arc::clone(&inner_queue),
            usize::MAX,
        ));
        let queue: Arc<dyn RuntimeQueueStore> = flaky_queue.clone();
        let store = BlockingWriteQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue,
            write_started: Arc::new(tokio::sync::Notify::new()),
            release_writes: Arc::new(tokio::sync::Notify::new()),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let write_started = store.write_started.notified();
        let first_runtime = runtime.clone();
        let first_store = store.clone();
        let mut first_submission = tokio::spawn(async move {
            first_runtime
                .record_terminal_event(
                    &first_store,
                    UsageEvent::new(
                        UsageEventType::Completed,
                        "req-terminal-slow-db-fallback-first",
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            total_tokens: Some(12),
                            status_code: Some(200),
                            ..UsageEventData::default()
                        },
                    ),
                )
                .await;
        });

        let first_write_started = timeout(Duration::from_secs(1), write_started).await;
        let mut excess = tokio::task::JoinSet::new();
        for index in 0..EXCESS_SUBMISSIONS {
            let runtime = runtime.clone();
            let store = store.clone();
            excess.spawn(async move {
                runtime
                    .record_terminal_event(
                        &store,
                        UsageEvent::new(
                            UsageEventType::Completed,
                            format!("req-terminal-slow-db-fallback-excess-{index}"),
                            UsageEventData {
                                provider_name: "openai".to_string(),
                                model: "gpt-5".to_string(),
                                total_tokens: Some(12),
                                status_code: Some(200),
                                ..UsageEventData::default()
                            },
                        ),
                    )
                    .await;
            });
        }
        let excess_backpressured = timeout(Duration::from_millis(50), async {
            while let Some(result) = excess.join_next().await {
                result.expect("excess terminal submission should not panic");
            }
        })
        .await;
        let saturated_snapshot = runtime.metrics_snapshot();

        flaky_queue.remaining_failures.store(0, Ordering::Release);
        sleep(Duration::from_millis(
            super::LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS + 25,
        ))
        .await;
        store.release_writes.notify_waiters();
        let first_completed = timeout(Duration::from_secs(1), &mut first_submission).await;
        if first_completed.is_err() {
            first_submission.abort();
            let _ = first_submission.await;
        }
        let excess_recovered = timeout(Duration::from_secs(2), async {
            while let Some(result) = excess.join_next().await {
                result.expect("backpressured terminal submission should not panic");
            }
        })
        .await;
        wait_for_enqueue_dispatcher_to_drain(
            &runtime,
            saturated_snapshot.enqueue_retry_scheduled_total,
        )
        .await;
        let recovered_snapshot = runtime.metrics_snapshot();

        assert!(
            first_write_started.is_ok(),
            "first fallback should reach the writer"
        );
        assert!(
            excess_backpressured.is_err(),
            "excess terminal calls should wait outside the hard resident-work bound"
        );
        assert!(first_completed.is_ok(), "released fallback should complete");
        if excess_recovered.is_err() {
            let snapshot = runtime.metrics_snapshot();
            panic!(
                "backpressured terminal calls should recover after persistence resumes: \
                 remaining={} lifecycle_pending={} ordered_pending={} terminal_pending={} \
                 terminal_in_flight={} writes_completed={} successful_appends={}",
                excess.len(),
                snapshot.lifecycle_submission_pending,
                snapshot.ordered_lifecycle_pending,
                snapshot.terminal_submission_pending,
                snapshot.terminal_submission_in_flight,
                store.writes_completed.load(Ordering::Acquire),
                flaky_queue.successful_appends.load(Ordering::Acquire),
            );
        }
        assert_eq!(saturated_snapshot.terminal_direct_fallback_limit, 1);
        assert_eq!(saturated_snapshot.terminal_direct_fallback_in_flight, 1);
        assert_eq!(saturated_snapshot.terminal_direct_fallback_max_in_flight, 1);
        assert_eq!(saturated_snapshot.worker_record_concurrency_in_flight, 1);
        assert_eq!(
            saturated_snapshot.worker_record_concurrency_max_in_flight,
            1
        );
        assert_eq!(saturated_snapshot.worker_record_deferred_total, 0);
        assert_eq!(
            saturated_snapshot.terminal_direct_fallback_rejected_total,
            0
        );
        assert_eq!(
            saturated_snapshot.terminal_enqueue_deferred_retry_total
                + saturated_snapshot.terminal_enqueue_deferred_dropped_total,
            0
        );
        assert_eq!(saturated_snapshot.enqueue_retry_pending, 0);
        assert_eq!(store.writes_completed.load(Ordering::Acquire), 1);
        assert_eq!(
            flaky_queue.successful_appends.load(Ordering::Acquire),
            EXCESS_SUBMISSIONS
        );
        assert_eq!(recovered_snapshot.terminal_direct_fallback_in_flight, 0);
        assert_eq!(recovered_snapshot.worker_record_concurrency_in_flight, 0);
        assert_eq!(recovered_snapshot.lifecycle_submission_pending, 0);
        assert_eq!(recovered_snapshot.ordered_lifecycle_pending, 0);
        assert_eq!(
            recovered_snapshot.terminal_direct_fallback_succeeded_total,
            1
        );
    }

    #[tokio::test]
    async fn terminal_seed_preserves_pending_streaming_terminal_order() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(1),
            ..UsageRuntimeConfig::default()
        };
        let queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_id = "req-terminal-seed-ordered";
        let plan = terminal_test_plan(request_id);
        let lifecycle_seed = build_lifecycle_usage_seed(&plan, None);
        let (context_seed, payload_seed) = sync_terminal_test_seeds(request_id);

        runtime.record_pending(&store, lifecycle_seed.clone());
        runtime.record_stream_started(&store, &lifecycle_seed, 200, None);
        runtime
            .record_sync_terminal(&store, context_seed, payload_seed)
            .await;

        timeout(Duration::from_secs(2), async {
            loop {
                let records = store.records.lock().expect("records lock").len();
                if records == 3 && runtime.metrics_snapshot().lifecycle_submission_pending == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("ordered terminal seed should fully persist");

        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 3);
        assert!(records.iter().all(|record| record.request_id == request_id));
        assert_eq!(records[0].status, "pending");
        assert_eq!(records[1].status, "streaming");
        assert_eq!(records[2].status, "completed");
    }

    #[tokio::test]
    async fn awaited_terminal_does_not_hold_permit_behind_queued_terminal_seed() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let store = BlockingWriteQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue,
            write_started: Arc::new(tokio::sync::Notify::new()),
            release_writes: Arc::new(tokio::sync::Notify::new()),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let blocker_started = Arc::new(tokio::sync::Notify::new());
        let release_blocker = Arc::new(tokio::sync::Notify::new());
        let seen = Arc::new(Mutex::new(Vec::new()));

        runtime
            .lifecycle_submission
            .dispatch(Box::new(TestLifecycleSubmissionItem {
                request_id: "req-terminal-seed-deadlock-blocker".to_string(),
                priority: LifecycleSubmissionPriority::Pending,
                started: Some(Arc::clone(&blocker_started)),
                release: Some(Arc::clone(&release_blocker)),
                seen,
            }));
        timeout(Duration::from_secs(1), blocker_started.notified())
            .await
            .expect("lifecycle blocker should start");

        let (context_seed, payload_seed) =
            sync_terminal_test_seeds("req-terminal-seed-before-awaited");
        runtime
            .record_sync_terminal(&store, context_seed, payload_seed)
            .await;

        let awaited_runtime = runtime.clone();
        let awaited_store = store.clone();
        let awaited_terminal = tokio::spawn(async move {
            awaited_runtime
                .record_terminal_event(
                    &awaited_store,
                    UsageEvent::new(
                        UsageEventType::Completed,
                        "req-awaited-terminal-after-seed",
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            status_code: Some(200),
                            ..UsageEventData::default()
                        },
                    ),
                )
                .await;
        });

        timeout(Duration::from_secs(1), async {
            while runtime.metrics_snapshot().lifecycle_submission_pending < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal seed and awaited barrier should queue behind the blocker");
        assert_eq!(runtime.metrics_snapshot().terminal_submission_pending, 0);

        let first_write_started = store.write_started.notified();
        release_blocker.notify_one();
        timeout(Duration::from_secs(1), first_write_started)
            .await
            .expect("terminal seed write should start");
        timeout(Duration::from_secs(1), async {
            while runtime.metrics_snapshot().terminal_submission_pending != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("awaited terminal should queue behind the terminal seed permit");

        let second_write_started = store.write_started.notified();
        store.release_writes.notify_one();
        timeout(Duration::from_secs(1), second_write_started)
            .await
            .expect("awaited terminal write should start after the seed completes");
        store.release_writes.notify_one();
        timeout(Duration::from_secs(2), awaited_terminal)
            .await
            .expect("awaited terminal must not deadlock behind the queued seed")
            .expect("awaited terminal task should not panic");
        timeout(Duration::from_secs(2), async {
            while store.records.lock().expect("records lock").len() != 2
                || runtime.metrics_snapshot().lifecycle_submission_pending != 0
                || store.writes_completed.load(Ordering::Acquire) != 2
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both terminal records should persist");

        let records = store.records.lock().expect("records lock");
        assert!(records
            .iter()
            .any(|record| record.request_id == "req-terminal-seed-before-awaited"));
        assert!(records
            .iter()
            .any(|record| record.request_id == "req-awaited-terminal-after-seed"));
        assert_eq!(
            runtime.metrics_snapshot().terminal_submission_max_pending,
            2
        );
    }

    #[tokio::test]
    async fn submitted_terminal_does_not_hold_permit_behind_queued_terminal_seed() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: 1,
            ..UsageRuntimeConfig::default()
        };
        let queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let store = BlockingWriteQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue,
            write_started: Arc::new(tokio::sync::Notify::new()),
            release_writes: Arc::new(tokio::sync::Notify::new()),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let blocker_started = Arc::new(tokio::sync::Notify::new());
        let release_blocker = Arc::new(tokio::sync::Notify::new());

        runtime
            .lifecycle_submission
            .dispatch(Box::new(TestLifecycleSubmissionItem {
                request_id: "req-submitted-terminal-deadlock-blocker".to_string(),
                priority: LifecycleSubmissionPriority::Pending,
                started: Some(Arc::clone(&blocker_started)),
                release: Some(Arc::clone(&release_blocker)),
                seen: Arc::new(Mutex::new(Vec::new())),
            }));
        timeout(Duration::from_secs(1), blocker_started.notified())
            .await
            .expect("lifecycle blocker should start");

        let (context_seed, payload_seed) =
            sync_terminal_test_seeds("req-terminal-seed-before-submitted");
        runtime
            .record_sync_terminal(&store, context_seed, payload_seed)
            .await;
        runtime
            .submit_terminal_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    "req-submitted-terminal-after-seed",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        assert_eq!(runtime.metrics_snapshot().terminal_submission_pending, 0);
        assert!(runtime.metrics_snapshot().lifecycle_submission_pending >= 3);

        let first_write_started = store.write_started.notified();
        release_blocker.notify_one();
        timeout(Duration::from_secs(1), first_write_started)
            .await
            .expect("terminal seed write should start");
        timeout(Duration::from_secs(1), async {
            while runtime.metrics_snapshot().terminal_submission_pending != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("submitted terminal should queue behind the terminal seed permit");

        let second_write_started = store.write_started.notified();
        store.release_writes.notify_one();
        timeout(Duration::from_secs(1), second_write_started)
            .await
            .expect("submitted terminal write should start after the seed completes");
        store.release_writes.notify_one();
        timeout(Duration::from_secs(2), async {
            while store.records.lock().expect("records lock").len() != 2
                || runtime.metrics_snapshot().lifecycle_submission_pending != 0
                || store.writes_completed.load(Ordering::Acquire) != 2
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("queued seed and submitted terminal should both persist");

        let records = store.records.lock().expect("records lock");
        assert!(records
            .iter()
            .any(|record| record.request_id == "req-terminal-seed-before-submitted"));
        assert!(records
            .iter()
            .any(|record| record.request_id == "req-submitted-terminal-after-seed"));
        assert_eq!(
            runtime.metrics_snapshot().terminal_submission_max_pending,
            2
        );
    }

    #[tokio::test]
    async fn terminal_seed_waits_for_its_ordered_turn_before_admission() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: 1,
            stream_key: "usage:events:test:terminal-seed-turn".to_string(),
            consumer_group: "usage_consumers_test_terminal_seed_turn".to_string(),
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let tracked_queue = Arc::new(FlakyAppendQueueStore::new(inner_queue, 0));
        let queue: Arc<dyn RuntimeQueueStore> = tracked_queue.clone();
        let store = BlockingPolicyQueueConfiguredUsageStore {
            queue,
            policy_started: Arc::new(tokio::sync::Notify::new()),
            release_policy: Arc::new(tokio::sync::Notify::new()),
            policy_reads: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let request_id = "req-terminal-seed-waits-for-turn";
        let plan = terminal_test_plan(request_id);
        let policy_started = store.policy_started.notified();

        runtime.record_pending(&store, build_lifecycle_usage_seed(&plan, None));
        timeout(Duration::from_secs(1), policy_started)
            .await
            .expect("pending phase should block in body policy");

        let (context_seed, payload_seed) = sync_terminal_test_seeds(request_id);
        runtime
            .record_sync_terminal(&store, context_seed, payload_seed)
            .await;

        let blocked_snapshot = runtime.metrics_snapshot();
        assert_eq!(blocked_snapshot.terminal_submission_pending, 0);
        assert_eq!(blocked_snapshot.terminal_submission_max_pending, 0);
        assert_eq!(blocked_snapshot.terminal_submission_in_flight, 0);
        assert!(blocked_snapshot.lifecycle_submission_pending >= 2);

        store.release_policy.notify_waiters();
        timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if tracked_queue.successful_appends.load(Ordering::Acquire) == 1
                    && snapshot.lifecycle_submission_pending == 0
                    && snapshot.terminal_submission_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal seed should persist after its ordered turn is released");

        let recovered_snapshot = runtime.metrics_snapshot();
        assert_eq!(recovered_snapshot.terminal_submission_max_pending, 1);
        assert_eq!(recovered_snapshot.terminal_submission_max_in_flight, 1);
        assert_eq!(recovered_snapshot.terminal_submission_rejected_total, 0);
        assert_eq!(
            recovered_snapshot.terminal_enqueue_deferred_dropped_total,
            0
        );
    }

    #[tokio::test]
    async fn terminal_seed_backlog_does_not_create_per_request_admission_waiters() {
        const BACKLOG: usize = 16;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: 1,
            stream_key: "usage:events:test:terminal-seed-backlog".to_string(),
            consumer_group: "usage_consumers_test_terminal_seed_backlog".to_string(),
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let tracked_queue = Arc::new(FlakyAppendQueueStore::new(inner_queue, 0));
        let queue: Arc<dyn RuntimeQueueStore> = tracked_queue.clone();
        let store = BlockingPolicyQueueConfiguredUsageStore {
            queue,
            policy_started: Arc::new(tokio::sync::Notify::new()),
            release_policy: Arc::new(tokio::sync::Notify::new()),
            policy_reads: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let policy_started = store.policy_started.notified();

        runtime
            .submit_terminal_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    "req-terminal-seed-backlog-blocker",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;
        timeout(Duration::from_secs(1), policy_started)
            .await
            .expect("blocking terminal should hold the only admission permit");

        for index in 0..BACKLOG {
            let request_id = format!("req-terminal-seed-backlog-{index}");
            let (context_seed, payload_seed) = sync_terminal_test_seeds(&request_id);
            runtime
                .record_sync_terminal(&store, context_seed, payload_seed)
                .await;
        }

        timeout(Duration::from_secs(1), async {
            while runtime.metrics_snapshot().terminal_submission_pending < BACKLOG + 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal execution queue should receive the full backlog");
        let blocked_snapshot = runtime.metrics_snapshot();
        assert_eq!(blocked_snapshot.terminal_submission_pending, BACKLOG + 1);
        assert_eq!(
            blocked_snapshot.terminal_submission_max_pending,
            BACKLOG + 1
        );
        assert_eq!(blocked_snapshot.terminal_submission_in_flight, 1);
        assert!(blocked_snapshot.lifecycle_submission_pending <= BACKLOG + 1);

        store.release_policy.notify_waiters();
        timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                if tracked_queue.successful_appends.load(Ordering::Acquire) == BACKLOG + 1
                    && snapshot.lifecycle_submission_pending == 0
                    && snapshot.terminal_submission_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all terminal seeds should recover from admission backpressure");

        let recovered_snapshot = runtime.metrics_snapshot();
        assert_eq!(
            recovered_snapshot.terminal_submission_max_pending,
            BACKLOG + 1
        );
        assert_eq!(recovered_snapshot.terminal_submission_max_in_flight, 1);
        assert_eq!(recovered_snapshot.terminal_submission_rejected_total, 0);
        assert_eq!(recovered_snapshot.terminal_enqueue_failed_total, 0);
        assert_eq!(
            recovered_snapshot.terminal_enqueue_deferred_dropped_total,
            0
        );
    }

    #[tokio::test]
    async fn terminal_permit_wait_does_not_block_unrelated_lifecycle_on_same_shard() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: 1,
            ..UsageRuntimeConfig::default()
        };
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        let held_permit = runtime
            .terminal_submission_state
            .acquire()
            .await
            .expect("test should hold the only terminal permit");
        let blocked_request_id = "req-terminal-permit-hol-blocked";
        let (context_seed, payload_seed) = sync_terminal_test_seeds(blocked_request_id);
        runtime
            .record_sync_terminal(&store, context_seed, payload_seed)
            .await;
        timeout(Duration::from_secs(1), async {
            while runtime.metrics_snapshot().terminal_submission_pending < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal work should reach the dedicated execution queue");

        let unrelated_request_id = "req-terminal-permit-hol-unrelated";
        runtime.record_pending(
            &store,
            build_lifecycle_usage_seed(&terminal_test_plan(unrelated_request_id), None),
        );
        timeout(Duration::from_secs(1), async {
            loop {
                let unrelated_written = {
                    let records = store.records.lock().expect("records lock");
                    records.iter().any(|record| {
                        record.request_id == unrelated_request_id && record.status == "pending"
                    })
                };
                if unrelated_written {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("unrelated lifecycle work must bypass terminal permit wait");
        assert!(!store
            .records
            .lock()
            .expect("records lock")
            .iter()
            .any(|record| record.request_id == blocked_request_id));

        drop(held_permit);
        timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                let terminal_written = store
                    .records
                    .lock()
                    .expect("records lock")
                    .iter()
                    .any(|record| record.request_id == blocked_request_id);
                if terminal_written
                    && snapshot.terminal_submission_pending == 0
                    && snapshot.lifecycle_submission_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal work should drain after the permit is released");
    }

    #[tokio::test]
    async fn terminal_build_does_not_block_unrelated_lifecycle_on_same_shard() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: 1,
            ..UsageRuntimeConfig::default()
        };
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let blocked_request_id = "req-terminal-build-hol-blocked";
        let build_started = Arc::new(tokio::sync::Notify::new());
        let release_build = Arc::new(tokio::sync::Notify::new());
        runtime
            .dispatch_terminal_seed(
                &store,
                blocked_request_id.to_string(),
                LifecycleTerminalUsageSeed::BlockedBuild {
                    event: UsageEvent::new(
                        UsageEventType::Completed,
                        blocked_request_id,
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            status_code: Some(200),
                            ..UsageEventData::default()
                        },
                    ),
                    started: Arc::clone(&build_started),
                    release: Arc::clone(&release_build),
                },
            )
            .await;
        timeout(Duration::from_secs(1), build_started.notified())
            .await
            .expect("terminal build should start on the dedicated worker");

        let unrelated_request_id = "req-terminal-build-hol-unrelated";
        runtime.record_pending(
            &store,
            build_lifecycle_usage_seed(&terminal_test_plan(unrelated_request_id), None),
        );
        timeout(Duration::from_secs(1), async {
            loop {
                let unrelated_written = {
                    let records = store.records.lock().expect("records lock");
                    records.iter().any(|record| {
                        record.request_id == unrelated_request_id && record.status == "pending"
                    })
                };
                if unrelated_written {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("unrelated lifecycle work must bypass terminal build");
        assert!(!store
            .records
            .lock()
            .expect("records lock")
            .iter()
            .any(|record| record.request_id == blocked_request_id));

        release_build.notify_waiters();
        timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = runtime.metrics_snapshot();
                let terminal_written = store
                    .records
                    .lock()
                    .expect("records lock")
                    .iter()
                    .any(|record| record.request_id == blocked_request_id);
                if terminal_written
                    && snapshot.terminal_submission_pending == 0
                    && snapshot.lifecycle_submission_pending == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal build should drain after release");
    }

    #[tokio::test]
    async fn terminal_submission_pending_decrements_when_waiter_is_cancelled() {
        let state = Arc::new(super::TerminalSubmissionState::new(1));
        let first = state
            .acquire()
            .await
            .expect("first terminal submission should acquire");

        let waiter_state = Arc::clone(&state);
        let waiter = tokio::spawn(async move { waiter_state.acquire().await });
        timeout(Duration::from_secs(1), async {
            while state.pending() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled waiter should be counted while it is queued");

        waiter.abort();
        assert!(
            matches!(waiter.await, Err(err) if err.is_cancelled()),
            "admission waiter task should report cancellation"
        );
        assert_eq!(state.pending(), 1);

        drop(first);
        assert_eq!(state.pending(), 0);
        assert_eq!(state.max_pending(), 2);
    }

    #[tokio::test]
    async fn direct_terminal_write_is_visible_to_submission_metrics_until_completion() {
        let config = UsageRuntimeConfig {
            enabled: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: 1,
            ..UsageRuntimeConfig::default()
        };
        let store = BlockingWriteQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default())),
            write_started: Arc::new(tokio::sync::Notify::new()),
            release_writes: Arc::new(tokio::sync::Notify::new()),
            writes_completed: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let write_started = store.write_started.notified();
        let write_runtime = runtime.clone();
        let write_store = store.clone();
        let write = tokio::spawn(async move {
            write_runtime
                .record_terminal_event_direct(
                    &write_store,
                    UsageEvent::new(
                        UsageEventType::Completed,
                        "req-direct-terminal-submission-metrics",
                        UsageEventData {
                            provider_name: "openai".to_string(),
                            model: "gpt-5".to_string(),
                            status_code: Some(200),
                            ..UsageEventData::default()
                        },
                    ),
                )
                .await;
        });

        timeout(Duration::from_secs(1), write_started)
            .await
            .expect("the direct terminal write should start");
        let in_flight = runtime.metrics_snapshot();
        assert_eq!(in_flight.terminal_submission_pending, 1);
        assert_eq!(in_flight.terminal_submission_in_flight, 1);

        store.release_writes.notify_one();
        timeout(Duration::from_secs(1), write)
            .await
            .expect("the direct terminal write should finish")
            .expect("the direct terminal write task should not panic");
        let completed = runtime.metrics_snapshot();
        assert_eq!(completed.terminal_submission_pending, 0);
        assert_eq!(completed.terminal_submission_in_flight, 0);
        assert_eq!(store.writes_completed.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn terminal_submission_admission_backpressures_and_recovers_all_events() {
        const EXCESS_SUBMISSIONS: usize = 16;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            worker_record_concurrency_limit: Some(1),
            terminal_submission_max_in_flight: 1,
            terminal_enqueue_max_in_flight: 1,
            stream_key: "usage:events:test:terminal-submission-admission".to_string(),
            consumer_group: "usage_consumers_test_terminal_submission_admission".to_string(),
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let tracked_queue = Arc::new(FlakyAppendQueueStore::new(inner_queue, 0));
        let queue: Arc<dyn RuntimeQueueStore> = tracked_queue.clone();
        let store = BlockingPolicyQueueConfiguredUsageStore {
            queue,
            policy_started: Arc::new(tokio::sync::Notify::new()),
            release_policy: Arc::new(tokio::sync::Notify::new()),
            policy_reads: Arc::new(AtomicUsize::new(0)),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let policy_started = store.policy_started.notified();
        runtime
            .submit_terminal_event(
                &store,
                UsageEvent::new(
                    UsageEventType::Completed,
                    "req-terminal-submission-admission-first",
                    UsageEventData {
                        provider_name: "openai".to_string(),
                        model: "gpt-5".to_string(),
                        total_tokens: Some(12),
                        status_code: Some(200),
                        ..UsageEventData::default()
                    },
                ),
            )
            .await;

        let first_policy_started = timeout(Duration::from_secs(1), policy_started).await;
        let mut pending_submissions = Vec::with_capacity(EXCESS_SUBMISSIONS);
        for index in 0..EXCESS_SUBMISSIONS {
            let runtime = runtime.clone();
            let store = store.clone();
            pending_submissions.push(tokio::spawn(async move {
                runtime
                    .submit_terminal_event(
                        &store,
                        UsageEvent::new(
                            UsageEventType::Completed,
                            format!("req-terminal-submission-admission-excess-{index}"),
                            UsageEventData {
                                provider_name: "openai".to_string(),
                                model: "gpt-5".to_string(),
                                total_tokens: Some(12),
                                status_code: Some(200),
                                ..UsageEventData::default()
                            },
                        ),
                    )
                    .await;
            }));
        }
        for submission in pending_submissions {
            timeout(Duration::from_secs(1), submission)
                .await
                .expect("terminal ownership handoff should not wait for admission")
                .expect("terminal submission handoff task should complete");
        }
        timeout(Duration::from_secs(1), async {
            while runtime.metrics_snapshot().terminal_submission_pending < EXCESS_SUBMISSIONS + 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal submissions should reach the execution backlog");
        let saturated_snapshot = runtime.metrics_snapshot();

        store.release_policy.notify_waiters();
        let all_completed = timeout(Duration::from_secs(2), async {
            loop {
                store.release_policy.notify_waiters();
                if tracked_queue.successful_appends.load(Ordering::Acquire)
                    == EXCESS_SUBMISSIONS + 1
                    && runtime.metrics_snapshot().terminal_submission_in_flight == 0
                    && runtime.metrics_snapshot().lifecycle_submission_pending == 0
                {
                    break;
                }
                sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        let recovered_snapshot = runtime.metrics_snapshot();

        assert!(
            first_policy_started.is_ok(),
            "first terminal submission should reach policy I/O"
        );
        assert!(all_completed.is_ok(), "all submissions should complete");
        assert_eq!(
            tracked_queue.successful_appends.load(Ordering::Acquire),
            EXCESS_SUBMISSIONS + 1,
            "every admitted terminal event should reach the queue"
        );
        assert_eq!(saturated_snapshot.terminal_submission_limit, 1);
        assert_eq!(
            saturated_snapshot.terminal_submission_pending,
            EXCESS_SUBMISSIONS + 1
        );
        assert_eq!(
            saturated_snapshot.terminal_submission_max_pending,
            EXCESS_SUBMISSIONS + 1
        );
        assert_eq!(saturated_snapshot.terminal_submission_in_flight, 1);
        assert_eq!(saturated_snapshot.terminal_submission_max_in_flight, 1);
        assert_eq!(saturated_snapshot.terminal_submission_rejected_total, 0);
        assert_eq!(store.policy_reads.load(Ordering::Acquire), 1);
        assert_eq!(recovered_snapshot.terminal_submission_in_flight, 0);
    }

    #[tokio::test]
    async fn terminal_enqueue_limit_bounds_primary_burst_and_recovers_all_events() {
        const WORKERS: usize = 4;
        const PRIMARY_LIMIT: usize = 4;
        const BURST: usize = 64;
        const FORCED_FAILURES: usize = 8;

        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            stream_key: "usage:events:test:terminal-dispatch-burst".to_string(),
            consumer_group: "usage_consumers_test_terminal_dispatch_burst".to_string(),
            consumer_block_ms: 1,
            terminal_enqueue_max_in_flight: PRIMARY_LIMIT as u64,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 5,
            enqueue_retry_buffer_capacity: BURST,
            enqueue_retry_workers: WORKERS,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let tracked_queue = Arc::new(
            FlakyAppendQueueStore::new(Arc::clone(&inner_queue), FORCED_FAILURES)
                .with_append_delay_ms(2),
        );
        let queue: Arc<dyn RuntimeQueueStore> = tracked_queue.clone();
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue,
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");

        let mut submissions = Vec::with_capacity(BURST);
        for index in 0..BURST {
            let runtime = runtime.clone();
            let store = store.clone();
            submissions.push(tokio::spawn(async move {
                runtime
                    .record_terminal_event(
                        &store,
                        UsageEvent::new(
                            UsageEventType::Completed,
                            format!("req-terminal-dispatch-burst-{index}"),
                            UsageEventData {
                                provider_name: "openai".to_string(),
                                model: "gpt-5".to_string(),
                                total_tokens: Some(12),
                                status_code: Some(200),
                                ..UsageEventData::default()
                            },
                        ),
                    )
                    .await;
            }));
        }
        for submission in submissions {
            submission
                .await
                .expect("terminal submission should not panic");
        }
        let scheduled = runtime.metrics_snapshot().enqueue_retry_scheduled_total;
        wait_for_enqueue_dispatcher_to_drain(&runtime, scheduled).await;

        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.enqueue_retry_scheduled_total, scheduled);
        assert_eq!(snapshot.enqueue_retry_recovered_total, scheduled);
        assert_eq!(snapshot.enqueue_retry_pending, 0);
        assert_eq!(snapshot.enqueue_retry_closed_or_unavailable_total, 0);
        assert_eq!(
            tracked_queue.successful_appends.load(Ordering::Acquire)
                + store.records.lock().expect("records lock").len(),
            BURST,
            "every terminal event should reach Redis or bounded direct fallback"
        );
        assert_eq!(
            tracked_queue.append_attempts.load(Ordering::Acquire),
            tracked_queue.successful_appends.load(Ordering::Acquire)
                + snapshot.terminal_enqueue_failed_total as usize
                + snapshot.enqueue_retry_failed_total as usize
        );
        assert!(
            tracked_queue.max_active_appends.load(Ordering::Acquire) <= PRIMARY_LIMIT + WORKERS,
            "Redis queue enqueue concurrency must be bounded by primary and retry limits"
        );
        assert_eq!(snapshot.terminal_enqueue_in_flight, 0);
        assert!(
            snapshot.terminal_enqueue_deferred_direct_write_total
                + snapshot.terminal_enqueue_deferred_retry_total
                > 0,
            "burst submissions above the primary limit should use a bounded fallback"
        );
        assert_eq!(snapshot.terminal_enqueue_deferred_dropped_total, 0);
        assert!(snapshot.terminal_direct_fallback_max_in_flight <= 32);
    }

    #[tokio::test]
    async fn queue_append_failure_prefers_bounded_direct_write_before_retry() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_terminal_events: true,
            stream_key: "usage:events:test:retry".to_string(),
            consumer_group: "usage_consumers_test_retry".to_string(),
            consumer_block_ms: 1,
            enqueue_retry_initial_backoff_ms: 1,
            enqueue_retry_max_backoff_ms: 5,
            enqueue_retry_buffer_capacity: 16,
            enqueue_retry_workers: 1,
            ..UsageRuntimeConfig::default()
        };
        let inner_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let flaky_queue: Arc<dyn RuntimeQueueStore> =
            Arc::new(FlakyAppendQueueStore::new(Arc::clone(&inner_queue), 1));
        let queue = UsageQueue::new(Arc::clone(&inner_queue), config.clone())
            .expect("usage queue should build");
        queue
            .ensure_consumer_group()
            .await
            .expect("consumer group should initialize");
        let store = EnrichmentCountingQueueStore {
            records: Mutex::new(Vec::new()),
            queue: flaky_queue,
            enrich_calls: AtomicUsize::new(0),
        };
        let runtime = UsageRuntime::new(config).expect("usage runtime should build");
        let event = UsageEvent::new(
            UsageEventType::Completed,
            "req-terminal-retry-1",
            UsageEventData {
                user_id: Some("user-terminal-retry-1".to_string()),
                provider_name: "openai".to_string(),
                model: "gpt-5".to_string(),
                total_tokens: Some(12),
                status_code: Some(200),
                ..UsageEventData::default()
            },
        );

        runtime.record_terminal_event(&store, event).await;

        assert_eq!(store.enrich_calls.load(Ordering::Acquire), 1);
        assert_eq!(store.records.lock().expect("records lock").len(), 1);
        assert!(
            queue
                .read_group("usage-test-retry-consumer")
                .await
                .expect("queue read should succeed")
                .is_empty(),
            "successful direct fallback should not also schedule Redis retry"
        );
        let snapshot = runtime.metrics_snapshot();
        assert_eq!(snapshot.terminal_direct_fallback_succeeded_total, 1);
        assert_eq!(snapshot.enqueue_retry_scheduled_total, 0);
    }

    #[tokio::test]
    async fn body_capture_policy_read_failure_is_short_cached_as_default() {
        let runtime = UsageRuntime::new(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        })
        .expect("usage runtime should build");
        let store = FailingPolicyUsageStore::default();

        let first = runtime.body_capture_policy_for(&store).await;
        assert!(first.is_err(), "first failed read should be surfaced");

        let second = runtime
            .body_capture_policy_for(&store)
            .await
            .expect("short cached fallback should be returned");
        let third = runtime
            .body_capture_policy_for(&store)
            .await
            .expect("short cached fallback should be reused");

        assert_eq!(second, UsageBodyCapturePolicy::default());
        assert_eq!(third, UsageBodyCapturePolicy::default());
        assert_eq!(
            store.policy_reads.load(Ordering::Acquire),
            1,
            "policy read failures should be short cached to avoid concurrent DB storms"
        );
    }

    #[test]
    fn preserve_request_facts_ignores_post_capture_truncation_placeholders() {
        let truncated = json!({
            "truncated": true,
            "reason": "body_capture_limit_exceeded"
        });
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-truncated-preserve",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                request_body: Some(truncated.clone()),
                request_body_state: Some(UsageBodyCaptureState::Truncated),
                provider_request_body: Some(truncated),
                provider_request_body_state: Some(UsageBodyCaptureState::Truncated),
                request_metadata: Some(json!({
                    "requested_reasoning_effort": "xhigh",
                    "provider_reasoning_effort": "max",
                    "provider_service_tier": "priority"
                })),
                ..UsageEventData::default()
            },
        );

        preserve_request_facts(&mut event);

        let metadata = event
            .data
            .request_metadata
            .as_ref()
            .expect("metadata remains");
        assert_eq!(metadata["requested_reasoning_effort"], "xhigh");
        assert_eq!(metadata["provider_reasoning_effort"], "max");
        assert_eq!(metadata["provider_service_tier"], "priority");
    }

    #[test]
    fn preserve_request_facts_clears_stale_facts_when_final_bodies_are_missing() {
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-missing-final-body",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                request_body_state: Some(UsageBodyCaptureState::None),
                provider_request_body_state: Some(UsageBodyCaptureState::None),
                request_metadata: Some(json!({
                    "requested_reasoning_effort": "xhigh",
                    "provider_reasoning_effort": "max",
                    "provider_service_tier": "priority",
                    "provider_actual_service_tier": "priority"
                })),
                ..UsageEventData::default()
            },
        );

        preserve_request_facts(&mut event);

        let metadata = event
            .data
            .request_metadata
            .as_ref()
            .expect("response audit fact remains");
        assert!(metadata.get("requested_reasoning_effort").is_none());
        assert!(metadata.get("provider_reasoning_effort").is_none());
        assert!(metadata.get("provider_service_tier").is_none());
        assert_eq!(metadata["provider_actual_service_tier"], "priority");
    }

    #[test]
    fn basic_request_record_level_strips_body_capture_but_preserves_derived_fields() {
        let mut event = UsageEvent::new(
            UsageEventType::Failed,
            "req-basic-1",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                total_tokens: Some(42),
                error_message: Some("upstream failed".to_string()),
                request_body: Some(json!({
                    "messages":[{"role":"user","content":"hello"}],
                    "reasoning": {"effort": "xhigh"}
                })),
                request_body_ref: Some("usage://request/req-basic-1/request_body".to_string()),
                provider_request_body: Some(json!({
                    "model":"gpt-5",
                    "reasoning": {"effort": "max"},
                    "service_tier": "priority"
                })),
                provider_request_body_ref: Some(
                    "usage://request/req-basic-1/provider_request_body".to_string(),
                ),
                response_body: Some(json!({
                    "error":{"message":"bad gateway"},
                    "service_tier": "Default"
                })),
                response_body_ref: Some("usage://request/req-basic-1/response_body".to_string()),
                client_response_body: Some(json!({"detail":"bad gateway"})),
                client_response_body_ref: Some(
                    "usage://request/req-basic-1/client_response_body".to_string(),
                ),
                request_metadata: Some(json!({
                    "requested_reasoning_effort": "low",
                    "provider_reasoning_effort": "medium",
                    "provider_service_tier": "standard"
                })),
                ..UsageEventData::default()
            },
        );

        preserve_request_facts(&mut event);
        preserve_provider_response_facts(&mut event);
        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Basic,
                ..UsageBodyCapturePolicy::default()
            },
            &mut event,
        );

        assert_eq!(event.data.total_tokens, Some(42));
        assert_eq!(event.data.error_message.as_deref(), Some("upstream failed"));
        assert!(event.data.request_body.is_none());
        assert!(event.data.request_body_ref.is_none());
        assert!(event.data.provider_request_body.is_none());
        assert!(event.data.provider_request_body_ref.is_none());
        assert!(event.data.response_body.is_none());
        assert!(event.data.response_body_ref.is_none());
        assert!(event.data.client_response_body.is_none());
        assert!(event.data.client_response_body_ref.is_none());
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("requested_reasoning_effort"))
                .and_then(serde_json::Value::as_str),
            Some("xhigh")
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("provider_reasoning_effort"))
                .and_then(serde_json::Value::as_str),
            Some("max")
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("provider_service_tier"))
                .and_then(serde_json::Value::as_str),
            Some("priority")
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("provider_actual_service_tier"))
                .and_then(serde_json::Value::as_str),
            Some("default")
        );
    }
}
