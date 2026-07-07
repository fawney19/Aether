use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_contracts::ExecutionTelemetry;
use aether_data_contracts::DataLayerError;
use aether_runtime_state::{RuntimeQueueStats, RuntimeQueueStore};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::executor::spawn_on_usage_background_runtime;
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

pub const DEFAULT_USAGE_REQUEST_BODY_CAPTURE_LIMIT_BYTES: usize = 5 * 1024 * 1024;
pub const DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageBodyCapturePolicy {
    pub record_level: UsageRequestRecordLevel,
    pub max_request_body_bytes: Option<usize>,
    pub max_response_body_bytes: Option<usize>,
}

impl Default for UsageBodyCapturePolicy {
    fn default() -> Self {
        Self {
            record_level: UsageRequestRecordLevel::Full,
            max_request_body_bytes: Some(DEFAULT_USAGE_REQUEST_BODY_CAPTURE_LIMIT_BYTES),
            max_response_body_bytes: Some(DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES),
        }
    }
}

#[async_trait]
pub trait UsageRuntimeAccess:
    UsageRecordWriter
    + UsageSettlementWriter
    + UsageBillingEventEnricher
    + crate::worker::ManualProxyNodeCounter
    + Send
    + Sync
{
    fn has_usage_writer(&self) -> bool;
    fn has_usage_worker_queue(&self) -> bool;
    fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>>;
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
    terminal_enqueue_state: Arc<LifecycleEnqueueState>,
    lifecycle_enqueue_state: Arc<LifecycleEnqueueState>,
    lifecycle_coalescer: Arc<LifecycleEventCoalescer>,
    lifecycle_delay: Arc<LifecycleDelayDispatcher>,
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
    pub terminal_enqueue_in_flight: u64,
    pub terminal_enqueue_deferred_total: u64,
    pub terminal_enqueue_deferred_retry_total: u64,
    pub terminal_enqueue_failed_total: u64,
    pub lifecycle_enqueue_in_flight: u64,
    pub lifecycle_enqueue_deferred_total: u64,
    pub lifecycle_enqueue_deferred_dropped_total: u64,
    pub lifecycle_enqueue_deferred_retry_total: u64,
    pub lifecycle_enqueue_failed_total: u64,
    pub enqueue_retry_scheduled_total: u64,
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
const LIFECYCLE_TERMINAL_CANCEL_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
struct LifecycleEventCoalescer {
    entries: tokio::sync::Mutex<HashMap<String, LifecycleEventCoalescerEntry>>,
}

#[derive(Debug, Clone, Copy)]
struct LifecycleEventCoalescerEntry {
    generation: u64,
    terminal_seen_at: Option<Instant>,
}

impl LifecycleEventCoalescer {
    async fn register(&self, request_id: String) -> Option<u64> {
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        Self::compact_locked(&mut entries, now);
        let entry = entries
            .entry(request_id)
            .or_insert(LifecycleEventCoalescerEntry {
                generation: 0,
                terminal_seen_at: None,
            });
        if terminal_cancel_is_active(entry, now) {
            return None;
        }
        entry.generation = entry.generation.saturating_add(1);
        entry.terminal_seen_at = None;
        Some(entry.generation)
    }

    async fn cancel(&self, request_id: &str) {
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        Self::compact_locked(&mut entries, now);
        let entry = entries
            .entry(request_id.to_string())
            .or_insert(LifecycleEventCoalescerEntry {
                generation: 0,
                terminal_seen_at: None,
            });
        entry.generation = entry.generation.saturating_add(1);
        entry.terminal_seen_at = Some(now);
    }

    async fn should_emit(&self, request_id: &str, generation: u64) -> bool {
        let now = Instant::now();
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.get(request_id).copied() else {
            return false;
        };
        let should_emit = entry.generation == generation && !terminal_cancel_is_active(&entry, now);
        if should_emit || !terminal_cancel_is_active(&entry, now) {
            entries.remove(request_id);
        }
        Self::compact_locked(&mut entries, now);
        should_emit
    }

    fn compact_locked(entries: &mut HashMap<String, LifecycleEventCoalescerEntry>, now: Instant) {
        entries.retain(|_, entry| {
            entry.terminal_seen_at.is_none() || terminal_cancel_is_active(entry, now)
        });
    }
}

fn terminal_cancel_is_active(entry: &LifecycleEventCoalescerEntry, now: Instant) -> bool {
    entry
        .terminal_seen_at
        .is_some_and(|seen_at| now.duration_since(seen_at) <= LIFECYCLE_TERMINAL_CANCEL_TTL)
}

impl Default for UsageRuntime {
    fn default() -> Self {
        Self::disabled()
    }
}

impl UsageRuntime {
    pub fn disabled() -> Self {
        Self {
            config: UsageRuntimeConfig::disabled(),
            body_policy_cache: Arc::new(tokio::sync::Mutex::new(None)),
            enqueue_retry: UsageEnqueueRetryDispatcher::disabled(),
            worker_supervisor_state: Arc::new(UsageWorkerSupervisorState::default()),
            worker_record_gate: None,
            terminal_enqueue_state: Arc::new(LifecycleEnqueueState::default()),
            lifecycle_enqueue_state: Arc::new(LifecycleEnqueueState::default()),
            lifecycle_coalescer: Arc::new(LifecycleEventCoalescer::default()),
            lifecycle_delay: LifecycleDelayDispatcher::disabled(),
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
        let lifecycle_coalescer = Arc::new(LifecycleEventCoalescer::default());
        let lifecycle_delay = LifecycleDelayDispatcher::spawn(
            config.clone(),
            Arc::clone(&lifecycle_coalescer),
            Arc::clone(&lifecycle_enqueue_state),
            Arc::clone(&enqueue_retry),
        );
        Ok(Self {
            config,
            body_policy_cache: Arc::new(tokio::sync::Mutex::new(None)),
            enqueue_retry,
            worker_supervisor_state: Arc::new(UsageWorkerSupervisorState::default()),
            worker_record_gate,
            terminal_enqueue_state: Arc::new(LifecycleEnqueueState::default()),
            lifecycle_enqueue_state,
            lifecycle_coalescer,
            lifecycle_delay,
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
            terminal_enqueue_in_flight: self.terminal_enqueue_state.in_flight(),
            terminal_enqueue_deferred_total: self.terminal_enqueue_state.deferred_total(),
            terminal_enqueue_deferred_retry_total: self
                .terminal_enqueue_state
                .deferred_retry_total(),
            terminal_enqueue_failed_total: self.terminal_enqueue_state.failed_total(),
            lifecycle_enqueue_in_flight: self.lifecycle_enqueue_state.in_flight(),
            lifecycle_enqueue_deferred_total: self.lifecycle_enqueue_state.deferred_total(),
            lifecycle_enqueue_deferred_dropped_total: self
                .lifecycle_enqueue_state
                .deferred_dropped_total(),
            lifecycle_enqueue_deferred_retry_total: self
                .lifecycle_enqueue_state
                .deferred_retry_total(),
            lifecycle_enqueue_failed_total: self.lifecycle_enqueue_state.failed_total(),
            enqueue_retry_scheduled_total: self.enqueue_retry.scheduled_total(),
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
        let runtime = self.clone();
        let data = T::clone(data);
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            runtime.record_pending_direct(&data, seed).await;
        }));
    }

    pub async fn record_pending_direct<T>(&self, data: &T, seed: LifecycleUsageSeed)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let request_id = seed.request_id.clone();
        let now_unix_secs = now_unix_secs();
        match build_pending_usage_event_offthread(seed, now_unix_secs).await {
            Ok(mut event) => {
                self.apply_body_capture_policy_from_data(data, &mut event)
                    .await;
                self.enqueue_or_write_lifecycle(data, event).await;
            }
            Err(err) => {
                warn!(
                    event_name = "usage_pending_event_build_failed",
                    log_type = "event",
                    request_id = %request_id,
                    error = %err,
                    "usage runtime failed to build sync pending usage event"
                )
            }
        }
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
        let runtime = self.clone();
        let data = T::clone(data);
        let seed = seed.clone();
        let telemetry = telemetry.cloned();
        let request_id = seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            let now_unix_secs = now_unix_secs();
            match build_streaming_usage_event_offthread(seed, status_code, telemetry, now_unix_secs)
                .await
            {
                Ok(mut event) => {
                    runtime
                        .apply_body_capture_policy_from_data(&data, &mut event)
                        .await;
                    runtime.enqueue_or_write_lifecycle(&data, event).await;
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_stream_event_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build stream usage event"
                    )
                }
            }
        }));
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
        let seed = seed.clone();
        let telemetry = telemetry.cloned();
        let request_id = seed.request_id.clone();
        let now_unix_secs = now_unix_secs();
        match build_streaming_usage_event_offthread(seed, status_code, telemetry, now_unix_secs)
            .await
        {
            Ok(mut event) => {
                self.apply_body_capture_policy_from_data(data, &mut event)
                    .await;
                self.enqueue_or_write_lifecycle(data, event).await;
            }
            Err(err) => {
                warn!(
                    event_name = "usage_stream_event_build_failed",
                    log_type = "event",
                    request_id = %request_id,
                    error = %err,
                    "usage runtime failed to build stream usage event"
                )
            }
        }
    }

    pub fn record_sync_active_immediate_async<T>(&self, data: &T, seed: LifecycleUsageSeed)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let runtime = self.clone();
        let data = T::clone(data);
        let request_id = seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            let now_unix_secs = now_unix_secs();
            match build_active_usage_event_offthread(seed, now_unix_secs).await {
                Ok(mut event) => {
                    runtime
                        .apply_body_capture_policy_from_data(&data, &mut event)
                        .await;
                    runtime.write_event_direct(&data, &event).await;
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_active_event_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build active usage event"
                    )
                }
            }
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
        let runtime = self.clone();
        let data = T::clone(data);
        let request_id = seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            let now_unix_secs = now_unix_secs();
            match build_streaming_usage_event_offthread(seed, status_code, telemetry, now_unix_secs)
                .await
            {
                Ok(mut event) => {
                    runtime
                        .apply_body_capture_policy_from_data(&data, &mut event)
                        .await;
                    runtime.write_event_direct(&data, &event).await;
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_stream_event_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build stream usage event"
                    )
                }
            }
        }));
    }

    pub fn record_sync_terminal<T>(
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
        let runtime = self.clone();
        let data = T::clone(data);
        let request_id = context_seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            match build_sync_terminal_usage_event_offthread(context_seed, payload_seed).await {
                Ok(mut event) => {
                    runtime
                        .apply_body_capture_policy_from_data(&data, &mut event)
                        .await;
                    runtime.enqueue_or_write_terminal(&data, event).await
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_sync_terminal_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build sync terminal usage event"
                    )
                }
            }
        }));
    }

    pub fn record_stream_terminal<T>(
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
        let runtime = self.clone();
        let data = T::clone(data);
        let request_id = context_seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            match build_stream_terminal_usage_event_offthread(context_seed, payload_seed, cancelled)
                .await
            {
                Ok(mut event) => {
                    runtime
                        .apply_body_capture_policy_from_data(&data, &mut event)
                        .await;
                    runtime.enqueue_or_write_terminal(&data, event).await
                }
                Err(err) => {
                    warn!(
                        event_name = "usage_stream_terminal_build_failed",
                        log_type = "event",
                        request_id = %request_id,
                        error = %err,
                        "usage runtime failed to build stream terminal usage event"
                    )
                }
            }
        }));
    }

    pub fn submit_terminal_event<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if !self.is_enabled() {
            return;
        }
        let runtime = self.clone();
        let data = T::clone(data);
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            runtime.record_terminal_event(&data, event).await;
        }));
    }

    pub async fn record_terminal_event<T>(&self, data: &T, mut event: UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        if !self.is_enabled() {
            return;
        }
        self.apply_body_capture_policy_from_data(data, &mut event)
            .await;
        self.enqueue_or_write_terminal(data, event).await;
    }

    pub async fn record_terminal_event_direct<T>(&self, data: &T, mut event: UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        if !self.is_enabled() {
            return;
        }
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
        self.lifecycle_coalescer.cancel(&event.request_id).await;
        self.write_event_direct(data, &event).await;
    }

    async fn apply_body_capture_policy_from_data<T>(&self, data: &T, event: &mut UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
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

    async fn enqueue_or_write_terminal<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        self.lifecycle_coalescer.cancel(&event.request_id).await;
        self.enqueue_or_write_event(data, event, "terminal", self.config.queue_terminal_events)
            .await;
    }

    async fn enqueue_or_write_lifecycle<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        if self.config.queue_lifecycle_events {
            self.enqueue_lifecycle_event_with_config_delay(data, event)
                .await;
        } else {
            self.write_event_direct(data, &event).await;
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
        if !self.lifecycle_delay.schedule(data, event, generation).await {
            self.lifecycle_coalescer.cancel(&request_id).await;
        }
    }

    async fn enqueue_lifecycle_event<T>(&self, data: &T, event: UsageEvent)
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
            return;
        }
        enqueue_lifecycle_event_now(
            data,
            event,
            self.config.clone(),
            Arc::clone(&self.lifecycle_enqueue_state),
            Arc::clone(&self.enqueue_retry),
        )
        .await;
    }

    async fn enqueue_or_write_event<T>(
        &self,
        data: &T,
        mut event: UsageEvent,
        event_phase: &'static str,
        queue_enabled: bool,
    ) where
        T: UsageRuntimeAccess,
    {
        if queue_enabled {
            if let Some(runner) = data.usage_worker_queue() {
                match UsageQueue::new(runner, self.config.clone()) {
                    Ok(queue) => {
                        if event_phase == "terminal" {
                            self.enqueue_terminal_event_or_schedule_retry(queue, event)
                                .await;
                            return;
                        }
                        match queue.enqueue(&event).await {
                            Ok(_) => return,
                            Err(err) => {
                                self.enqueue_retry
                                    .schedule(queue, event, event_phase, err)
                                    .await;
                                return;
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

        if event_phase == "terminal" {
            enrich_terminal_event(data, &mut event).await;
        }
        self.write_event_direct(data, &event).await;
    }

    async fn enqueue_terminal_event_or_schedule_retry(&self, queue: UsageQueue, event: UsageEvent) {
        let now_ms = now_unix_ms();
        if self.terminal_enqueue_state.is_circuit_open(now_ms) {
            self.terminal_enqueue_state.record_deferred(
                "usage_terminal_event_enqueue_deferred",
                "circuit_open",
                &event,
                true,
            );
            self.enqueue_retry
                .schedule(
                    queue,
                    event,
                    "terminal",
                    DataLayerError::TimedOut("terminal enqueue circuit is open".to_string()),
                )
                .await;
            return;
        }

        let Some(_guard) = self
            .terminal_enqueue_state
            .try_acquire_in_flight(self.config.terminal_enqueue_max_in_flight)
        else {
            self.terminal_enqueue_state.record_deferred(
                "usage_terminal_event_enqueue_deferred",
                "in_flight_limit",
                &event,
                true,
            );
            self.enqueue_retry
                .schedule(
                    queue,
                    event,
                    "terminal",
                    DataLayerError::TimedOut("terminal enqueue in-flight limit".to_string()),
                )
                .await;
            return;
        };

        if let Err(err) = queue.enqueue(&event).await {
            self.terminal_enqueue_state
                .open_circuit(now_unix_ms().saturating_add(LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS));
            let failures = self.terminal_enqueue_state.increment_failed_total();
            if should_log_usage_retry_counter(failures) {
                warn!(
                    event_name = "usage_terminal_event_enqueue_failed",
                    log_type = "event",
                    usage_event_type = ?event.event_type,
                    request_id = %event.request_id,
                    fallback = "local_enqueue_retry",
                    failure_total = failures,
                    circuit_open_ms = LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS,
                    error = %err,
                    "usage runtime failed to enqueue terminal event; terminal enqueue circuit opened"
                );
            }
            self.enqueue_retry
                .schedule(queue, event, "terminal", err)
                .await;
        }
    }

    async fn write_event_direct<T>(&self, data: &T, event: &UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        match build_upsert_usage_record_from_event(event) {
            Ok(record) => match data.upsert_usage_record(record).await {
                Ok(Some(stored)) => {
                    if let Err(err) = settle_usage_if_needed(data, &stored).await {
                        warn!(
                            event_name = "usage_terminal_settlement_failed",
                            log_type = "event",
                            request_id = %event.request_id,
                            error = %err,
                            "usage runtime failed to settle terminal usage directly"
                        );
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        event_name = "usage_event_upsert_failed",
                        log_type = "event",
                        usage_event_type = ?event.event_type,
                        request_id = %event.request_id,
                        error = %err,
                        "usage runtime failed to upsert usage event directly"
                    );
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
                )
            }
        }
    }
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
    dropped_total: AtomicU64,
    retry_total: AtomicU64,
    failed_total: AtomicU64,
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
        event: &UsageEvent,
        retry_enabled: bool,
    ) {
        let skipped = self.skipped_total.fetch_add(1, Ordering::AcqRel) + 1;
        let fallback = if retry_enabled {
            self.retry_total.fetch_add(1, Ordering::AcqRel);
            "local_enqueue_retry"
        } else {
            self.dropped_total.fetch_add(1, Ordering::AcqRel);
            "drop"
        };
        if should_log_usage_retry_counter(skipped) {
            warn!(
                event_name,
                log_type = "event",
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
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
}

#[derive(Debug)]
struct LifecycleDelayDispatcher {
    delay: Duration,
    sender: Option<mpsc::Sender<DelayedLifecycleQueueItem>>,
}

impl LifecycleDelayDispatcher {
    fn disabled() -> Arc<Self> {
        Arc::new(Self {
            delay: Duration::ZERO,
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

        let capacity = config.enqueue_retry_buffer_capacity.clamp(1024, 1_048_576);
        let delay = Duration::from_millis(config.lifecycle_enqueue_delay_ms.max(1));
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
            sender: Some(sender),
        })
    }

    async fn schedule<T>(&self, data: T, event: UsageEvent, generation: u64) -> bool
    where
        T: UsageRuntimeAccess + Clone + 'static,
    {
        let Some(sender) = &self.sender else {
            return false;
        };
        sender
            .try_send(DelayedLifecycleQueueItem {
                due_at: tokio::time::Instant::now() + self.delay,
                item: Box::new(DelayedLifecycleEventItem {
                    data,
                    event,
                    generation,
                }),
            })
            .is_ok()
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
) where
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
        return;
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
            return;
        }
    };

    let now_ms = now_unix_ms();
    if enqueue_state.is_circuit_open(now_ms) {
        let retry_enabled = config.retry_deferred_lifecycle_events;
        enqueue_state.record_deferred(
            "usage_lifecycle_event_enqueue_deferred",
            "circuit_open",
            &event,
            retry_enabled,
        );
        if retry_enabled {
            enqueue_retry
                .schedule(
                    queue,
                    event,
                    "lifecycle",
                    DataLayerError::TimedOut("lifecycle enqueue circuit is open".to_string()),
                )
                .await;
        }
        return;
    }

    let Some(_guard) = enqueue_state.try_acquire_in_flight(config.lifecycle_enqueue_max_in_flight)
    else {
        let retry_enabled = config.retry_deferred_lifecycle_events;
        enqueue_state.record_deferred(
            "usage_lifecycle_event_enqueue_deferred",
            "in_flight_limit",
            &event,
            retry_enabled,
        );
        if retry_enabled {
            enqueue_retry
                .schedule(
                    queue,
                    event,
                    "lifecycle",
                    DataLayerError::TimedOut("lifecycle enqueue in-flight limit".to_string()),
                )
                .await;
        }
        return;
    };

    if let Err(err) = queue.enqueue(&event).await {
        enqueue_state.open_circuit(now_unix_ms().saturating_add(LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS));
        let failures = enqueue_state.increment_failed_total();
        if should_log_usage_retry_counter(failures) {
            warn!(
                event_name = "usage_lifecycle_event_enqueue_failed",
                log_type = "event",
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
                fallback = "none",
                failure_total = failures,
                circuit_open_ms = LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS,
                error = %err,
                "usage runtime failed to enqueue lifecycle event; lifecycle enqueue circuit opened"
            );
        }
    }
}

#[derive(Debug)]
struct UsageEnqueueRetryDispatcher {
    senders: Vec<mpsc::Sender<UsageEnqueueRetryItem>>,
    scheduled_total: Arc<AtomicU64>,
}

struct UsageEnqueueRetryItem {
    queue: UsageQueue,
    event: UsageEvent,
    event_phase: &'static str,
    attempts: u64,
}

impl UsageEnqueueRetryDispatcher {
    fn disabled() -> Arc<Self> {
        Arc::new(Self {
            senders: Vec::new(),
            scheduled_total: Arc::new(AtomicU64::new(0)),
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
        let scheduled_total = Arc::new(AtomicU64::new(0));
        let recovered_total = Arc::new(AtomicU64::new(0));
        for worker_index in 0..workers {
            let capacity =
                retry_worker_capacity(config.enqueue_retry_buffer_capacity, workers, worker_index);
            let (sender, receiver) = mpsc::channel(capacity);
            senders.push(sender);
            let worker_config = config.clone();
            let worker_recovered_total = Arc::clone(&recovered_total);
            spawn_on_usage_background_runtime(async move {
                run_usage_enqueue_retry_worker(
                    worker_index,
                    worker_config,
                    receiver,
                    worker_recovered_total,
                )
                .await;
            });
        }

        Arc::new(Self {
            senders,
            scheduled_total,
        })
    }

    async fn schedule(
        &self,
        queue: UsageQueue,
        event: UsageEvent,
        event_phase: &'static str,
        cause: DataLayerError,
    ) {
        let scheduled = self.scheduled_total.fetch_add(1, Ordering::AcqRel) + 1;
        if should_log_usage_retry_counter(scheduled) {
            warn!(
                event_name = "usage_event_enqueue_failed_retry_scheduled",
                log_type = "event",
                event_phase,
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
                retry_scheduled_total = scheduled,
                fallback = "local_enqueue_retry",
                error = %cause,
                "usage runtime failed to enqueue usage event; scheduled local retry"
            );
        }

        let worker_index = retry_worker_index(&event.request_id, self.senders.len());
        let Some(sender) = self.senders.get(worker_index) else {
            warn!(
                event_name = "usage_event_enqueue_retry_unavailable",
                log_type = "event",
                event_phase,
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
                error = %cause,
                "usage runtime local enqueue retry dispatcher is unavailable"
            );
            return;
        };

        let item = UsageEnqueueRetryItem {
            queue,
            event,
            event_phase,
            attempts: 0,
        };
        match sender.try_send(item) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(item)) => {
                warn!(
                    event_name = "usage_event_enqueue_retry_buffer_full",
                    log_type = "event",
                    event_phase = item.event_phase,
                    usage_event_type = ?item.event.event_type,
                    request_id = %item.event.request_id,
                    worker_index,
                    "usage runtime local enqueue retry buffer is full; waiting for capacity"
                );
                if let Err(err) = sender.send(item).await {
                    let item = err.0;
                    warn!(
                        event_name = "usage_event_enqueue_retry_closed",
                        log_type = "event",
                        event_phase = item.event_phase,
                        usage_event_type = ?item.event.event_type,
                        request_id = %item.event.request_id,
                        worker_index,
                        "usage runtime local enqueue retry dispatcher closed before accepting event"
                    );
                }
            }
            Err(mpsc::error::TrySendError::Closed(item)) => {
                warn!(
                    event_name = "usage_event_enqueue_retry_closed",
                    log_type = "event",
                    event_phase = item.event_phase,
                    usage_event_type = ?item.event.event_type,
                    request_id = %item.event.request_id,
                    worker_index,
                    "usage runtime local enqueue retry dispatcher is closed"
                );
            }
        }
    }

    fn scheduled_total(&self) -> u64 {
        self.scheduled_total.load(Ordering::Acquire)
    }
}

async fn run_usage_enqueue_retry_worker(
    worker_index: usize,
    config: UsageRuntimeConfig,
    mut receiver: mpsc::Receiver<UsageEnqueueRetryItem>,
    recovered_total: Arc<AtomicU64>,
) {
    let mut initial_retry_delay_applied = false;
    while let Some(mut item) = receiver.recv().await {
        if !initial_retry_delay_applied {
            initial_retry_delay_applied = true;
            tokio::time::sleep(usage_enqueue_retry_delay(&config, 1)).await;
        }
        loop {
            match item.queue.enqueue(&item.event).await {
                Ok(_) => {
                    let recovered = recovered_total.fetch_add(1, Ordering::AcqRel) + 1;
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

async fn build_pending_usage_event_offthread(
    seed: LifecycleUsageSeed,
    now_unix_secs: u64,
) -> Result<UsageEvent, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        crate::write::build_pending_usage_event_from_owned_seed(seed, now_unix_secs)
    })
    .await
    .map_err(join_error_to_data_layer)?
}

async fn build_active_usage_event_offthread(
    seed: LifecycleUsageSeed,
    now_unix_secs: u64,
) -> Result<UsageEvent, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        crate::write::build_active_usage_event_from_owned_seed(seed, now_unix_secs)
    })
    .await
    .map_err(join_error_to_data_layer)?
}

async fn build_streaming_usage_event_offthread(
    seed: LifecycleUsageSeed,
    status_code: u16,
    telemetry: Option<ExecutionTelemetry>,
    now_unix_secs: u64,
) -> Result<UsageEvent, DataLayerError> {
    tokio::task::spawn_blocking(move || {
        crate::write::build_streaming_usage_event_from_owned_seed(
            seed,
            status_code,
            telemetry,
            now_unix_secs,
        )
    })
    .await
    .map_err(join_error_to_data_layer)?
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

fn boxed_usage_task<F>(task: F) -> Pin<Box<dyn Future<Output = ()> + Send>>
where
    F: Future<Output = ()> + Send + 'static,
{
    Box::pin(task)
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

    use aether_contracts::{ExecutionPlan, ExecutionTelemetry, RequestBody};
    use aether_data_contracts::repository::settlement::{
        StoredUsageSettlement, UsageSettlementInput,
    };
    use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
    use aether_data_contracts::DataLayerError;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeQueueStore, RuntimeState};
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::time::{sleep, Duration};

    use super::{
        UsageBillingEventEnricher, UsageBodyCapturePolicy, UsageRequestRecordLevel,
        UsageRuntimeAccess, UsageWorkerObservation, UsageWorkerSupervisorState,
    };
    use crate::worker::ManualProxyNodeCounter;
    use crate::{
        apply_usage_body_capture_policy_to_event, build_lifecycle_usage_seed, UsageEvent,
        UsageEventData, UsageEventType, UsageQueue, UsageRecordWriter, UsageRuntime,
        UsageRuntimeConfig, UsageSettlementWriter,
    };

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

    struct PanicOnceQueueConfiguredUsageStore {
        inner: CloneQueueConfiguredUsageStore,
        remaining_panics: AtomicUsize,
    }

    struct EnrichmentCountingQueueStore {
        records: Mutex<Vec<UpsertUsageRecord>>,
        queue: Arc<dyn RuntimeQueueStore>,
        enrich_calls: AtomicUsize,
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

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            Some(Arc::clone(&self.queue))
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
            let failed = self
                .remaining_failures
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    (current > 0).then(|| current - 1)
                })
                .is_ok();
            if failed {
                return Err(DataLayerError::Redis("forced append failure".to_string()));
            }
            self.inner
                .append_fields_with_maxlen(stream, fields, maxlen)
                .await
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
    async fn pending_usage_uses_lifecycle_queue_when_enabled() {
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

        for _ in 0..50 {
            let entries = queue
                .read_group("usage-test-consumer")
                .await
                .expect("queue read should succeed");
            if let Some(entry) = entries.into_iter().next() {
                let event = UsageEvent::from_stream_fields(&entry.fields)
                    .expect("queued usage event should parse");
                assert_eq!(event.event_type, UsageEventType::Pending);
                assert_eq!(event.request_id, "req-lifecycle-queue-pending-1");
                assert!(
                    store.records.lock().expect("records lock").is_empty(),
                    "pending lifecycle event should not write directly when queue succeeds"
                );
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }

        panic!("pending lifecycle usage event was not enqueued");
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
    async fn stream_started_direct_lifecycle_enqueue_can_be_delayed() {
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

        let immediate = queue
            .read_group("usage-test-consumer-stream-direct-delay")
            .await
            .expect("queue read should succeed");
        assert!(
            immediate.is_empty(),
            "stream started direct lifecycle event should not enqueue before the delay"
        );
        assert!(
            store.records.lock().expect("records lock").is_empty(),
            "stream started direct lifecycle event should not write directly when queue succeeds"
        );

        sleep(Duration::from_millis(60)).await;
        let entries = queue
            .read_group("usage-test-consumer-stream-direct-delay")
            .await
            .expect("queue read should succeed");
        assert_eq!(entries.len(), 1);
        let event = UsageEvent::from_stream_fields(&entries[0].fields)
            .expect("queued usage event should parse");
        assert_eq!(event.request_id, "req-stream-direct-delay");
        assert_eq!(event.event_type, UsageEventType::Streaming);
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
    async fn lifecycle_queue_append_failure_does_not_write_directly() {
        let config = UsageRuntimeConfig {
            enabled: true,
            queue_lifecycle_events: true,
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
        let flaky_queue: Arc<dyn RuntimeQueueStore> = Arc::new(FlakyAppendQueueStore {
            inner: Arc::clone(&inner_queue),
            remaining_failures: AtomicUsize::new(usize::MAX),
            append_attempts: AtomicUsize::new(0),
        });
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

        assert!(
            store.records.lock().expect("records lock").is_empty(),
            "lifecycle enqueue failure must not fall back to direct DB writes"
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
        let flaky_queue = Arc::new(FlakyAppendQueueStore {
            inner: Arc::clone(&inner_queue),
            remaining_failures: AtomicUsize::new(usize::MAX),
            append_attempts: AtomicUsize::new(0),
        });
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
            snapshot.lifecycle_enqueue_deferred_dropped_total, 9,
            "deferred lifecycle events should be dropped when retry is disabled"
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
        let flaky_queue = Arc::new(FlakyAppendQueueStore {
            inner: Arc::clone(&inner_queue),
            remaining_failures: AtomicUsize::new(1),
            append_attempts: AtomicUsize::new(0),
        });
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
        assert_eq!(snapshot.lifecycle_enqueue_deferred_retry_total, 1);
        assert_eq!(snapshot.lifecycle_enqueue_deferred_dropped_total, 0);
        assert_eq!(snapshot.enqueue_retry_scheduled_total, 1);
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
    async fn terminal_enqueue_failure_opens_short_circuit_and_retries() {
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
        let flaky_queue: Arc<dyn RuntimeQueueStore> = Arc::new(FlakyAppendQueueStore {
            inner: Arc::clone(&inner_queue),
            remaining_failures: AtomicUsize::new(usize::MAX),
            append_attempts: AtomicUsize::new(0),
        });
        let store = CloneQueueConfiguredUsageStore {
            records: Arc::new(Mutex::new(Vec::new())),
            queue: flaky_queue,
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
            "only the first terminal event should attempt Redis before opening the circuit"
        );
        assert_eq!(
            snapshot.terminal_enqueue_deferred_retry_total, 1,
            "second terminal event should be scheduled for retry through the open circuit"
        );
        assert_eq!(
            snapshot.enqueue_retry_scheduled_total, 2,
            "both terminal events should remain reliable via local retry"
        );
        assert!(
            store.records.lock().expect("records lock").is_empty(),
            "terminal queue failure must not fall back to direct writes"
        );
    }

    #[tokio::test]
    async fn queue_append_failure_retries_locally_without_direct_write() {
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
        let flaky_queue: Arc<dyn RuntimeQueueStore> = Arc::new(FlakyAppendQueueStore {
            inner: Arc::clone(&inner_queue),
            remaining_failures: AtomicUsize::new(1),
            append_attempts: AtomicUsize::new(0),
        });
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

        assert_eq!(store.enrich_calls.load(Ordering::Acquire), 0);
        assert!(store.records.lock().expect("records lock").is_empty());
        for _ in 0..50 {
            let entries = queue
                .read_group("usage-test-retry-consumer")
                .await
                .expect("queue read should succeed");
            if let Some(entry) = entries.into_iter().next() {
                let event = UsageEvent::from_stream_fields(&entry.fields)
                    .expect("queued retry event should parse");
                assert_eq!(event.request_id, "req-terminal-retry-1");
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }

        panic!("terminal usage event was not retried into queue");
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
    fn basic_request_record_level_strips_body_capture_but_preserves_derived_fields() {
        let mut event = UsageEvent::new(
            UsageEventType::Failed,
            "req-basic-1",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                total_tokens: Some(42),
                error_message: Some("upstream failed".to_string()),
                request_body: Some(json!({"messages":[{"role":"user","content":"hello"}]})),
                request_body_ref: Some("usage://request/req-basic-1/request_body".to_string()),
                provider_request_body: Some(json!({"model":"gpt-5"})),
                provider_request_body_ref: Some(
                    "usage://request/req-basic-1/provider_request_body".to_string(),
                ),
                response_body: Some(json!({"error":{"message":"bad gateway"}})),
                response_body_ref: Some("usage://request/req-basic-1/response_body".to_string()),
                client_response_body: Some(json!({"detail":"bad gateway"})),
                client_response_body_ref: Some(
                    "usage://request/req-basic-1/client_response_body".to_string(),
                ),
                ..UsageEventData::default()
            },
        );

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
    }
}
