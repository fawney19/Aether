use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_contracts::ExecutionTelemetry;
use aether_data_contracts::DataLayerError;
use aether_runtime_state::RuntimeQueueStore;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::warn;

use crate::executor::spawn_on_usage_background_runtime;
use crate::{
    apply_usage_body_capture_policy_to_event, build_stream_terminal_usage_seed,
    build_sync_terminal_usage_seed, build_terminal_usage_event_from_seed,
    build_upsert_usage_record_from_event, build_usage_queue_worker, settle_usage_if_needed,
    LifecycleUsageSeed, StreamTerminalUsagePayloadSeed, SyncTerminalUsagePayloadSeed,
    TerminalUsageContextSeed, UsageEvent, UsageQueue, UsageRecordWriter, UsageRuntimeConfig,
    UsageSettlementWriter,
};

#[async_trait]
pub trait UsageBillingEventEnricher: Send + Sync {
    async fn enrich_usage_event(&self, event: &mut UsageEvent) -> Result<(), DataLayerError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageRequestRecordLevel {
    #[default]
    Basic,
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
            record_level: UsageRequestRecordLevel::Basic,
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
    lifecycle_enqueue_state: Arc<LifecycleEnqueueState>,
}

const USAGE_BODY_CAPTURE_POLICY_CACHE_TTL: Duration = Duration::from_secs(30);
const USAGE_BODY_CAPTURE_POLICY_ERROR_CACHE_TTL: Duration = Duration::from_secs(1);
const LIFECYCLE_ENQUEUE_MAX_IN_FLIGHT: u64 = 128;
const LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS: u64 = 1_000;

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
            lifecycle_enqueue_state: Arc::new(LifecycleEnqueueState::default()),
        }
    }

    pub fn new(config: UsageRuntimeConfig) -> Result<Self, DataLayerError> {
        config.validate()?;
        let enqueue_retry = UsageEnqueueRetryDispatcher::spawn(config.clone());
        Ok(Self {
            config,
            body_policy_cache: Arc::new(tokio::sync::Mutex::new(None)),
            enqueue_retry,
            lifecycle_enqueue_state: Arc::new(LifecycleEnqueueState::default()),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
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
        let worker = build_usage_queue_worker(runner, data, self.config.clone()).ok()?;
        Some(worker.spawn())
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
        let request_id = seed.request_id.clone();
        spawn_on_usage_background_runtime(boxed_usage_task(async move {
            let now_unix_secs = now_unix_secs();
            match build_pending_usage_event_offthread(seed, now_unix_secs).await {
                Ok(mut event) => {
                    runtime
                        .apply_body_capture_policy_from_data(&data, &mut event)
                        .await;
                    runtime.enqueue_or_write_lifecycle(&data, event).await;
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
        }));
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
        self.enqueue_or_write_event(data, event, "terminal", self.config.queue_terminal_events)
            .await;
    }

    async fn enqueue_or_write_lifecycle<T>(&self, data: &T, event: UsageEvent)
    where
        T: UsageRuntimeAccess,
    {
        self.enqueue_lifecycle_event(data, event).await;
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

        let now_ms = now_unix_ms();
        if self.lifecycle_enqueue_state.is_circuit_open(now_ms) {
            self.lifecycle_enqueue_state
                .record_skip("circuit_open", &event);
            return;
        }

        let Some(_guard) = self.lifecycle_enqueue_state.try_acquire_in_flight(&event) else {
            return;
        };

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

        let queue = match UsageQueue::new(runner, self.config.clone()) {
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

        if let Err(err) = queue.enqueue(&event).await {
            self.lifecycle_enqueue_state
                .open_circuit(now_unix_ms().saturating_add(LIFECYCLE_ENQUEUE_CIRCUIT_OPEN_MS));
            let failures = self
                .lifecycle_enqueue_state
                .failed_total
                .fetch_add(1, Ordering::AcqRel)
                + 1;
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
                    Ok(queue) => match queue.enqueue(&event).await {
                        Ok(_) => return,
                        Err(err) => {
                            self.enqueue_retry
                                .schedule(queue, event, event_phase, err)
                                .await;
                            return;
                        }
                    },
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
    failed_total: AtomicU64,
}

impl LifecycleEnqueueState {
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
        event: &UsageEvent,
    ) -> Option<LifecycleEnqueueInFlightGuard<'a>> {
        let mut current = self.in_flight.load(Ordering::Acquire);
        loop {
            if current >= LIFECYCLE_ENQUEUE_MAX_IN_FLIGHT {
                self.record_skip("in_flight_limit", event);
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

    fn record_skip(&self, reason: &'static str, event: &UsageEvent) {
        let skipped = self.skipped_total.fetch_add(1, Ordering::AcqRel) + 1;
        if should_log_usage_retry_counter(skipped) {
            warn!(
                event_name = "usage_lifecycle_event_enqueue_skipped",
                log_type = "event",
                usage_event_type = ?event.event_type,
                request_id = %event.request_id,
                reason,
                skipped_total = skipped,
                fallback = "none",
                "usage runtime skipped lifecycle enqueue"
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
        if !config.enabled || !(config.queue_terminal_events || config.queue_lifecycle_events) {
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
}

async fn run_usage_enqueue_retry_worker(
    worker_index: usize,
    config: UsageRuntimeConfig,
    mut receiver: mpsc::Receiver<UsageEnqueueRetryItem>,
    recovered_total: Arc<AtomicU64>,
) {
    while let Some(mut item) = receiver.recv().await {
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
    value <= 8 || value.is_power_of_two() || value % 1_000 == 0
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

    use aether_contracts::{ExecutionPlan, RequestBody};
    use aether_data_contracts::repository::settlement::{
        StoredUsageSettlement, UsageSettlementInput,
    };
    use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
    use aether_data_contracts::DataLayerError;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeQueueStore, RuntimeState};
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::time::{sleep, Duration};

    use super::{UsageBillingEventEnricher, UsageBodyCapturePolicy, UsageRuntimeAccess};
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
        assert!(
            store.records.lock().expect("records lock").is_empty(),
            "lifecycle enqueue circuit must not fall back to direct DB writes"
        );
    }

    #[tokio::test]
    async fn disabled_lifecycle_queue_does_not_write_directly() {
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

        assert!(
            store.records.lock().expect("records lock").is_empty(),
            "disabled lifecycle queue must not fall back to direct DB writes"
        );
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
    fn default_body_capture_policy_strips_body_capture_but_preserves_derived_fields() {
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

        apply_usage_body_capture_policy_to_event(UsageBodyCapturePolicy::default(), &mut event);

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
