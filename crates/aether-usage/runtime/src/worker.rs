use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
use aether_data_contracts::DataLayerError;
use aether_runtime_state::{RuntimeQueueEntry, RuntimeQueueStore};
use async_trait::async_trait;
use tokio::sync::{mpsc, Notify};
use tracing::warn;

use crate::executor::spawn_on_usage_background_runtime;
use crate::keyed_lock::KeyedAsyncLockPool;
use crate::runtime::{
    UsageBillingEventEnricher, UsageRuntimeAccess, UsageWorkerRecordConcurrencyGate,
};
use crate::{
    build_upsert_usage_record_from_event, settle_usage_if_needed, UsageEvent, UsageEventType,
    UsageQueue, UsageRuntimeConfig, UsageSettlementWriter,
};

const USAGE_WORKER_DB_PRESSURE_DEFER_MS: u64 = 10;
const USAGE_WORKER_ACK_CHUNK_SIZE: usize = 100;

#[async_trait]
pub trait UsageEventRecorder: Send + Sync {
    async fn record_usage_event(&self, event: &UsageEvent) -> Result<(), DataLayerError>;
}

#[async_trait]
pub trait ManualProxyNodeCounter: Send + Sync {
    async fn increment_manual_proxy_node_requests(
        &self,
        node_id: &str,
        total_delta: i64,
        failed_delta: i64,
        latency_ms: Option<i64>,
    ) -> Result<(), DataLayerError>;
}

#[async_trait]
pub trait UsageRecordWriter: Send + Sync {
    /// Native batch support is opt-in; the default preserves one-row writes for other backends.
    fn supports_first_byte_usage_batch(&self) -> bool {
        false
    }

    /// Stable identity for the underlying first-byte writer. Implementations that opt into
    /// batching must return the same value for clones backed by the same repository.
    fn first_byte_usage_writer_identity(&self) -> Option<usize> {
        None
    }

    /// Native pending batching is opt-in because it must retain the complete usage audit write
    /// contract, not just the base lifecycle row.
    fn supports_pending_usage_batch(&self) -> bool {
        false
    }

    /// Stable identity for clones backed by the same pending usage repository.
    fn pending_usage_writer_identity(&self) -> Option<usize> {
        None
    }

    async fn upsert_usage_record(
        &self,
        record: UpsertUsageRecord,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError>;

    async fn upsert_first_byte_usage_record(
        &self,
        record: UpsertUsageRecord,
    ) -> Result<(), DataLayerError> {
        self.upsert_usage_record(record).await.map(|_| ())
    }

    async fn upsert_first_byte_usage_records(
        &self,
        records: Vec<UpsertUsageRecord>,
    ) -> Result<(), DataLayerError> {
        for record in records {
            self.upsert_first_byte_usage_record(record).await?;
        }
        Ok(())
    }

    async fn upsert_pending_usage_records(
        &self,
        records: Vec<UpsertUsageRecord>,
    ) -> Result<(), DataLayerError> {
        for record in records {
            self.upsert_usage_record(record).await?;
        }
        Ok(())
    }
}

pub struct UsageDataEventRecorder<T> {
    data: Arc<T>,
    record_gate: Option<Arc<UsageWorkerRecordConcurrencyGate>>,
    defer_for_database_pressure: bool,
}

impl<T> UsageDataEventRecorder<T> {
    pub fn new(data: Arc<T>) -> Self {
        Self::with_record_gate(data, None)
    }

    pub(crate) fn with_record_gate(
        data: Arc<T>,
        record_gate: Option<Arc<UsageWorkerRecordConcurrencyGate>>,
    ) -> Self {
        Self {
            data,
            record_gate,
            defer_for_database_pressure: false,
        }
    }

    pub(crate) fn with_record_gate_and_database_pressure_defer(
        data: Arc<T>,
        record_gate: Option<Arc<UsageWorkerRecordConcurrencyGate>>,
    ) -> Self {
        Self {
            data,
            record_gate,
            defer_for_database_pressure: true,
        }
    }
}

#[async_trait]
impl<T> UsageEventRecorder for UsageDataEventRecorder<T>
where
    T: UsageRuntimeAccess,
{
    async fn record_usage_event(&self, event: &UsageEvent) -> Result<(), DataLayerError> {
        if self.defer_for_database_pressure
            && self.data.usage_worker_should_defer_for_database_pressure()
        {
            if let Some(gate) = self.record_gate.as_ref() {
                gate.record_deferred();
            }
            tokio::time::sleep(Duration::from_millis(USAGE_WORKER_DB_PRESSURE_DEFER_MS)).await;
        }
        let _record_gate_permit = match self.record_gate.as_ref() {
            Some(gate) => Some(gate.acquire().await),
            None => None,
        };
        let request_lock = usage_request_lock(&event.request_id);
        let _guard = request_lock.lock().await;
        let mut event = event.clone();
        enrich_terminal_event(self.data.as_ref(), &mut event).await;
        write_event_record(self.data.as_ref(), &event).await
    }
}

fn usage_request_lock(request_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<KeyedAsyncLockPool> = OnceLock::new();
    LOCKS
        .get_or_init(KeyedAsyncLockPool::default)
        .lock_for(request_id)
}

pub struct UsageQueueWorker {
    queue: UsageQueue,
    recorder: Arc<dyn UsageEventRecorder>,
    consumer: String,
    worker_index: Option<usize>,
    control: Option<UsageWorkerControl>,
    telemetry: Option<mpsc::Sender<UsageWorkerObservation>>,
    config: UsageRuntimeConfig,
}

#[derive(Clone, Default)]
pub(crate) struct UsageWorkerControl {
    shutdown: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
}

impl UsageWorkerControl {
    pub(crate) fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.shutdown_notify.notify_one();
    }

    fn should_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    async fn wait_for_shutdown(&self) {
        while !self.should_shutdown() {
            self.shutdown_notify.notified().await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UsageWorkerObservation {
    pub worker_index: Option<usize>,
    pub entries_read: usize,
    pub batch_size: usize,
    pub reclaimed_entries: usize,
    pub acked_entries: usize,
    pub dead_lettered_entries: usize,
    pub process_failures: usize,
    pub read_failures: usize,
    pub reclaim_failures: usize,
}

impl UsageWorkerObservation {
    fn read(worker_index: Option<usize>, entries_read: usize, batch_size: usize) -> Self {
        Self {
            worker_index,
            entries_read,
            batch_size,
            reclaimed_entries: 0,
            acked_entries: 0,
            dead_lettered_entries: 0,
            process_failures: 0,
            read_failures: 0,
            reclaim_failures: 0,
        }
    }

    fn reclaimed(worker_index: Option<usize>, reclaimed_entries: usize) -> Self {
        Self {
            worker_index,
            entries_read: 0,
            batch_size: 0,
            reclaimed_entries,
            acked_entries: 0,
            dead_lettered_entries: 0,
            process_failures: 0,
            read_failures: 0,
            reclaim_failures: 0,
        }
    }

    fn acked(worker_index: Option<usize>, acked_entries: usize) -> Self {
        Self {
            worker_index,
            entries_read: 0,
            batch_size: 0,
            reclaimed_entries: 0,
            acked_entries,
            dead_lettered_entries: 0,
            process_failures: 0,
            read_failures: 0,
            reclaim_failures: 0,
        }
    }

    fn dead_lettered(worker_index: Option<usize>, dead_lettered_entries: usize) -> Self {
        Self {
            worker_index,
            entries_read: 0,
            batch_size: 0,
            reclaimed_entries: 0,
            acked_entries: 0,
            dead_lettered_entries,
            process_failures: 0,
            read_failures: 0,
            reclaim_failures: 0,
        }
    }

    fn process_failed(worker_index: Option<usize>) -> Self {
        Self {
            worker_index,
            entries_read: 0,
            batch_size: 0,
            reclaimed_entries: 0,
            acked_entries: 0,
            dead_lettered_entries: 0,
            process_failures: 1,
            read_failures: 0,
            reclaim_failures: 0,
        }
    }

    fn read_failed(worker_index: Option<usize>) -> Self {
        Self {
            worker_index,
            entries_read: 0,
            batch_size: 0,
            reclaimed_entries: 0,
            acked_entries: 0,
            dead_lettered_entries: 0,
            process_failures: 0,
            read_failures: 1,
            reclaim_failures: 0,
        }
    }

    fn reclaim_failed(worker_index: Option<usize>) -> Self {
        Self {
            worker_index,
            entries_read: 0,
            batch_size: 0,
            reclaimed_entries: 0,
            acked_entries: 0,
            dead_lettered_entries: 0,
            process_failures: 0,
            read_failures: 0,
            reclaim_failures: 1,
        }
    }
}

impl UsageQueueWorker {
    pub fn new(
        runner: Arc<dyn RuntimeQueueStore>,
        recorder: Arc<dyn UsageEventRecorder>,
        config: UsageRuntimeConfig,
        worker_index: Option<usize>,
    ) -> Result<Self, DataLayerError> {
        let queue = UsageQueue::new(runner, config.clone())?;
        let consumer = consumer_name(worker_index);
        Ok(Self {
            queue,
            recorder,
            consumer,
            worker_index,
            control: None,
            telemetry: None,
            config,
        })
    }

    pub(crate) fn with_supervisor(
        mut self,
        control: UsageWorkerControl,
        telemetry: mpsc::Sender<UsageWorkerObservation>,
    ) -> Self {
        self.control = Some(control);
        self.telemetry = Some(telemetry);
        self
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        spawn_on_usage_background_runtime(async move { self.run_forever().await })
    }

    pub(crate) async fn run(self) {
        self.run_forever().await;
    }

    async fn run_forever(self) {
        if let Err(err) = self.queue.ensure_consumer_group().await {
            warn!(
                event_name = "usage_worker_consumer_group_failed",
                log_type = "ops",
                worker_consumer = %self.consumer,
                worker_group = %self.config.consumer_group,
                error = %err,
                "usage worker failed to ensure consumer group"
            );
            return;
        }

        let mut reclaim_interval =
            tokio::time::interval(Duration::from_millis(self.config.reclaim_interval_ms));
        reclaim_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        reclaim_interval.tick().await;

        let mut reclaim_due = false;

        loop {
            if self.should_shutdown() {
                break;
            }

            let result = {
                let mut read_future = Box::pin(self.queue.read_group(&self.consumer));
                loop {
                    tokio::select! {
                        biased;
                        // A command already delivered by Redis remains in the PEL and is recovered
                        // by a subsequent worker reclaim after the idle period.
                        _ = self.wait_for_shutdown() => return,
                        _ = reclaim_interval.tick(), if !reclaim_due => {
                            // Do not reclaim while XREADGROUP is in flight. Redis can add an entry
                            // to this consumer's PEL before delivering the response; claiming it in
                            // that window would make both paths process the same stream entry.
                            reclaim_due = true;
                        }
                        result = &mut read_future => break result,
                    }
                }
            };

            match result {
                Ok(entries) => {
                    self.report_read(entries.len());
                    if let Err(err) = self.process_entries(entries).await {
                        self.report_process_failed();
                        warn!(
                            event_name = "usage_worker_process_failed",
                            log_type = "ops",
                            worker_consumer = %self.consumer,
                            worker_group = %self.config.consumer_group,
                            error = %err,
                            "usage worker failed to process queue entries"
                        );
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
                Err(err) => {
                    self.report_read_failed();
                    warn!(
                        event_name = "usage_worker_read_failed",
                        log_type = "ops",
                        worker_consumer = %self.consumer,
                        worker_group = %self.config.consumer_group,
                        error = %err,
                        "usage worker failed to read queue"
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }

            if self.should_shutdown() {
                break;
            }

            if reclaim_due {
                reclaim_due = false;
                self.reclaim_stale_entries().await;
            }
        }
    }

    async fn wait_for_shutdown(&self) {
        match self.control.as_ref() {
            Some(control) => control.wait_for_shutdown().await,
            None => std::future::pending().await,
        }
    }

    async fn reclaim_stale_entries(&self) {
        match self.queue.claim_stale(&self.consumer, "0-0").await {
            Ok(entries) => {
                self.report_reclaimed(entries.len());
                if let Err(err) = self.process_entries(entries).await {
                    self.report_process_failed();
                    warn!(
                        event_name = "usage_worker_reclaim_process_failed",
                        log_type = "ops",
                        worker_consumer = %self.consumer,
                        worker_group = %self.config.consumer_group,
                        error = %err,
                        "usage worker failed while reclaiming stale entries"
                    );
                }
            }
            Err(err) => {
                self.report_reclaim_failed();
                warn!(
                    event_name = "usage_worker_reclaim_failed",
                    log_type = "ops",
                    worker_consumer = %self.consumer,
                    worker_group = %self.config.consumer_group,
                    error = %err,
                    "usage worker failed to reclaim stale entries"
                );
            }
        }
    }

    fn should_shutdown(&self) -> bool {
        self.control
            .as_ref()
            .is_some_and(UsageWorkerControl::should_shutdown)
    }

    fn report_read(&self, entries_read: usize) {
        self.report(UsageWorkerObservation::read(
            self.worker_index,
            entries_read,
            self.config.consumer_batch_size.max(1),
        ));
    }

    fn report_reclaimed(&self, reclaimed_entries: usize) {
        self.report(UsageWorkerObservation::reclaimed(
            self.worker_index,
            reclaimed_entries,
        ));
    }

    fn report_acked(&self, acked_entries: usize) {
        self.report(UsageWorkerObservation::acked(
            self.worker_index,
            acked_entries,
        ));
    }

    fn report_dead_lettered(&self, dead_lettered_entries: usize) {
        self.report(UsageWorkerObservation::dead_lettered(
            self.worker_index,
            dead_lettered_entries,
        ));
    }

    fn report_process_failed(&self) {
        self.report(UsageWorkerObservation::process_failed(self.worker_index));
    }

    fn report_read_failed(&self) {
        self.report(UsageWorkerObservation::read_failed(self.worker_index));
    }

    fn report_reclaim_failed(&self) {
        self.report(UsageWorkerObservation::reclaim_failed(self.worker_index));
    }

    fn report(&self, observation: UsageWorkerObservation) {
        let Some(telemetry) = &self.telemetry else {
            return;
        };
        let _ = telemetry.try_send(observation);
    }

    async fn process_entries(&self, entries: Vec<RuntimeQueueEntry>) -> Result<(), DataLayerError> {
        if entries.is_empty() {
            return Ok(());
        }

        let mut ack_ids = Vec::new();
        for entry in entries {
            match self.process_entry(&entry).await {
                Ok(should_ack) => {
                    if should_ack {
                        ack_ids.push(entry.id.clone());
                        if ack_ids.len() >= USAGE_WORKER_ACK_CHUNK_SIZE {
                            self.queue.ack_and_delete(&ack_ids).await?;
                            self.report_acked(ack_ids.len());
                            ack_ids.clear();
                        }
                    }
                }
                Err(err) => {
                    if !ack_ids.is_empty() {
                        let _ = self.queue.ack_and_delete(&ack_ids).await;
                    }
                    return Err(err);
                }
            }
        }

        if !ack_ids.is_empty() {
            self.queue.ack_and_delete(&ack_ids).await?;
            self.report_acked(ack_ids.len());
        }

        Ok(())
    }

    async fn process_entry(&self, entry: &RuntimeQueueEntry) -> Result<bool, DataLayerError> {
        let event = match UsageEvent::from_stream_fields(&entry.fields) {
            Ok(event) => event,
            Err(err) => {
                warn!(
                    event_name = "usage_worker_entry_decode_dead_lettered",
                    log_type = "ops",
                    worker_consumer = %self.consumer,
                    worker_group = %self.config.consumer_group,
                    entry_id = %entry.id,
                    error = %err,
                    "usage worker moved malformed queue entry to dead letter"
                );
                self.queue.push_dead_letter(entry, &err.to_string()).await?;
                self.report_dead_lettered(1);
                return Ok(true);
            }
        };

        match self.recorder.record_usage_event(&event).await {
            Ok(()) => Ok(true),
            Err(err) if usage_event_record_error_is_permanent(&err) => {
                warn!(
                    event_name = "usage_worker_entry_record_dead_lettered",
                    log_type = "ops",
                    worker_consumer = %self.consumer,
                    worker_group = %self.config.consumer_group,
                    entry_id = %entry.id,
                    request_id = %event.request_id,
                    event_type = ?event.event_type,
                    provider_name = %event.data.provider_name,
                    model = %event.data.model,
                    api_format = event.data.api_format.as_deref().unwrap_or(""),
                    provider_id = event.data.provider_id.as_deref().unwrap_or(""),
                    provider_endpoint_id = event.data.provider_endpoint_id.as_deref().unwrap_or(""),
                    provider_api_key_id = event.data.provider_api_key_id.as_deref().unwrap_or(""),
                    error = %err,
                    "usage worker moved non-retryable usage event to dead letter"
                );
                self.queue.push_dead_letter(entry, &err.to_string()).await?;
                self.report_dead_lettered(1);
                Ok(true)
            }
            Err(err) => {
                warn!(
                    event_name = "usage_worker_entry_record_retryable_failed",
                    log_type = "ops",
                    worker_consumer = %self.consumer,
                    worker_group = %self.config.consumer_group,
                    entry_id = %entry.id,
                    request_id = %event.request_id,
                    event_type = ?event.event_type,
                    provider_name = %event.data.provider_name,
                    model = %event.data.model,
                    api_format = event.data.api_format.as_deref().unwrap_or(""),
                    provider_id = event.data.provider_id.as_deref().unwrap_or(""),
                    provider_endpoint_id = event.data.provider_endpoint_id.as_deref().unwrap_or(""),
                    provider_api_key_id = event.data.provider_api_key_id.as_deref().unwrap_or(""),
                    error = %err,
                    "usage worker will retry usage event after record failure"
                );
                Err(err)
            }
        }
    }
}

fn usage_event_record_error_is_permanent(err: &DataLayerError) -> bool {
    match err {
        DataLayerError::InvalidConfiguration(_)
        | DataLayerError::InvalidInput(_)
        | DataLayerError::UnexpectedValue(_) => true,
        DataLayerError::Postgres(message) | DataLayerError::Sql(message) => {
            database_error_is_known_permanent(message)
        }
        DataLayerError::Redis(_) | DataLayerError::TimedOut(_) => false,
    }
}

fn database_error_is_known_permanent(message: &str) -> bool {
    message.contains("SQLSTATE 23503") || message.contains("violates foreign key constraint")
}

pub fn build_usage_queue_worker<T>(
    runner: Arc<dyn RuntimeQueueStore>,
    data: Arc<T>,
    config: UsageRuntimeConfig,
    worker_index: Option<usize>,
) -> Result<UsageQueueWorker, DataLayerError>
where
    T: UsageRuntimeAccess + 'static,
{
    build_usage_queue_worker_with_record_gate(runner, data, config, None, worker_index)
}

pub(crate) fn build_usage_queue_worker_with_record_gate<T>(
    runner: Arc<dyn RuntimeQueueStore>,
    data: Arc<T>,
    config: UsageRuntimeConfig,
    record_gate: Option<Arc<UsageWorkerRecordConcurrencyGate>>,
    worker_index: Option<usize>,
) -> Result<UsageQueueWorker, DataLayerError>
where
    T: UsageRuntimeAccess + 'static,
{
    UsageQueueWorker::new(
        runner,
        Arc::new(
            UsageDataEventRecorder::with_record_gate_and_database_pressure_defer(data, record_gate),
        ),
        config,
        worker_index,
    )
}

pub async fn write_event_record<T>(data: &T, event: &UsageEvent) -> Result<(), DataLayerError>
where
    T: UsageRecordWriter + UsageSettlementWriter + Send + Sync,
{
    let record = build_upsert_usage_record_from_event(event)?;
    if let Some(stored) = data.upsert_usage_record(record).await? {
        settle_usage_if_needed(data, &stored).await?;
    }
    // Manual proxy traffic is counted at the actual transport-attempt boundary. Usage events are
    // replayable, so emitting that side effect here would count normal requests and reclaims twice.
    Ok(())
}

async fn enrich_terminal_event<T>(data: &T, event: &mut UsageEvent)
where
    T: UsageBillingEventEnricher + Send + Sync,
{
    if !matches!(
        event.event_type,
        UsageEventType::Completed | UsageEventType::Failed | UsageEventType::Cancelled
    ) {
        return;
    }

    if let Err(err) = data.enrich_usage_event(event).await {
        warn!(
            event_name = "usage_worker_billing_enrichment_failed",
            log_type = "event",
            request_id = %event.request_id,
            event_type = ?event.event_type,
            error = %err,
            "usage worker failed to enrich terminal usage event with billing"
        );
    }
}

fn consumer_name(worker_index: Option<usize>) -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "aether-gateway".to_string());
    match worker_index {
        Some(worker_index) => format!("{host}:{}:{worker_index}", std::process::id()),
        None => format!("{host}:{}", std::process::id()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use aether_data_contracts::repository::settlement::{
        StoredUsageSettlement, UsageSettlementInput,
    };
    use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
    use aether_data_contracts::DataLayerError;
    use aether_runtime_state::{
        MemoryRuntimeStateConfig, RuntimeQueueEntry, RuntimeQueueReclaimConfig, RuntimeQueueStats,
        RuntimeQueueStore, RuntimeState,
    };
    use async_trait::async_trait;
    use tokio::sync::Notify;

    use super::{
        build_usage_queue_worker_with_record_gate, usage_event_record_error_is_permanent,
        write_event_record, ManualProxyNodeCounter, UsageEventRecorder, UsageQueueWorker,
        UsageRecordWriter, UsageWorkerControl,
    };
    use crate::runtime::UsageWorkerRecordConcurrencyGate;
    use crate::UsageBillingEventEnricher;
    use crate::{
        UsageEvent, UsageEventData, UsageEventType, UsageQueue, UsageRuntimeConfig,
        UsageSettlementWriter,
    };

    #[derive(Default)]
    struct TestUsageStore {
        records: Mutex<Vec<UpsertUsageRecord>>,
        settlements: Mutex<Vec<UsageSettlementInput>>,
        enrich_calls: Mutex<Vec<String>>,
        manual_proxy_counter_calls: AtomicUsize,
    }

    #[derive(Default)]
    struct SelectiveFailingRecorder {
        calls: Mutex<Vec<String>>,
    }

    #[derive(Default)]
    struct SlowUsageStore {
        active: std::sync::atomic::AtomicUsize,
        max_active: std::sync::atomic::AtomicUsize,
        db_pressure: AtomicBool,
        records: Mutex<Vec<String>>,
    }

    struct ReadReclaimRaceProbeQueue {
        entry: RuntimeQueueEntry,
        read_calls: AtomicUsize,
        first_read_cancelled: AtomicBool,
        read_completed: AtomicUsize,
        release_read: Notify,
        reclaim_calls: AtomicUsize,
        acked: AtomicBool,
    }

    impl ReadReclaimRaceProbeQueue {
        fn new(entry: RuntimeQueueEntry) -> Self {
            Self {
                entry,
                read_calls: AtomicUsize::new(0),
                first_read_cancelled: AtomicBool::new(false),
                read_completed: AtomicUsize::new(0),
                release_read: Notify::new(),
                reclaim_calls: AtomicUsize::new(0),
                acked: AtomicBool::new(false),
            }
        }
    }

    struct FirstReadDropGuard<'a> {
        cancelled: &'a AtomicBool,
        completed: bool,
    }

    impl Drop for FirstReadDropGuard<'_> {
        fn drop(&mut self) {
            if !self.completed {
                self.cancelled.store(true, Ordering::Release);
            }
        }
    }

    #[async_trait]
    impl RuntimeQueueStore for ReadReclaimRaceProbeQueue {
        async fn ensure_consumer_group(
            &self,
            _stream: &str,
            _group: &str,
            _start_id: &str,
        ) -> Result<(), DataLayerError> {
            Ok(())
        }

        async fn append_fields_with_maxlen(
            &self,
            _stream: &str,
            _fields: &BTreeMap<String, String>,
            _maxlen: Option<usize>,
        ) -> Result<String, DataLayerError> {
            Ok("0-0".to_string())
        }

        async fn read_group(
            &self,
            _stream: &str,
            _group: &str,
            _consumer: &str,
            _count: usize,
            _block_ms: Option<u64>,
        ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
            let call_index = self.read_calls.fetch_add(1, Ordering::AcqRel);
            let mut first_read_guard = (call_index == 0).then(|| FirstReadDropGuard {
                cancelled: &self.first_read_cancelled,
                completed: false,
            });
            self.release_read.notified().await;
            if let Some(guard) = first_read_guard.as_mut() {
                guard.completed = true;
            }
            self.read_completed.fetch_add(1, Ordering::AcqRel);
            Ok((call_index == 0)
                .then(|| self.entry.clone())
                .into_iter()
                .collect())
        }

        async fn claim_stale(
            &self,
            _stream: &str,
            _group: &str,
            _consumer: &str,
            _start_id: &str,
            _config: RuntimeQueueReclaimConfig,
        ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
            self.reclaim_calls.fetch_add(1, Ordering::AcqRel);
            Ok((!self.acked.load(Ordering::Acquire))
                .then(|| self.entry.clone())
                .into_iter()
                .collect())
        }

        async fn ack(
            &self,
            _stream: &str,
            _group: &str,
            ids: &[String],
        ) -> Result<usize, DataLayerError> {
            if ids.iter().any(|id| id == &self.entry.id) {
                self.acked.store(true, Ordering::Release);
                Ok(1)
            } else {
                Ok(0)
            }
        }

        async fn delete(&self, _stream: &str, ids: &[String]) -> Result<usize, DataLayerError> {
            Ok(ids.len())
        }

        async fn stats(
            &self,
            _stream: &str,
            _group: Option<&str>,
        ) -> Result<RuntimeQueueStats, DataLayerError> {
            Ok(RuntimeQueueStats::default())
        }
    }

    #[async_trait]
    impl UsageRecordWriter for TestUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, aether_data_contracts::DataLayerError>
        {
            self.records
                .lock()
                .expect("records lock")
                .push(record.clone());
            Ok(Some(
                StoredRequestUsageAudit::new(
                    "usage-1".to_string(),
                    record.request_id,
                    record.user_id,
                    record.api_key_id,
                    record.username,
                    record.api_key_name,
                    record.provider_name,
                    record.model,
                    record.target_model,
                    record.provider_id,
                    record.provider_endpoint_id,
                    record.provider_api_key_id,
                    record.request_type,
                    record.api_format,
                    record.api_family,
                    record.endpoint_kind,
                    record.endpoint_api_format,
                    record.provider_api_family,
                    record.provider_endpoint_kind,
                    record.has_format_conversion.unwrap_or(false),
                    record.is_stream.unwrap_or(false),
                    record.input_tokens.unwrap_or_default() as i32,
                    record.output_tokens.unwrap_or_default() as i32,
                    record.total_tokens.unwrap_or_default() as i32,
                    record.total_cost_usd.unwrap_or_default(),
                    record.actual_total_cost_usd.unwrap_or_default(),
                    record.status_code.map(i32::from),
                    record.error_message,
                    record.error_category,
                    record.response_time_ms.map(|value| value as i32),
                    record.first_byte_time_ms.map(|value| value as i32),
                    record.status,
                    record.billing_status,
                    record
                        .created_at_unix_ms
                        .unwrap_or(record.updated_at_unix_secs) as i64,
                    record.updated_at_unix_secs as i64,
                    record.finalized_at_unix_secs.map(|value| value as i64),
                )
                .expect("stored usage should build"),
            ))
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for TestUsageStore {
        fn has_usage_settlement_writer(&self) -> bool {
            true
        }

        async fn settle_usage(
            &self,
            input: UsageSettlementInput,
        ) -> Result<Option<StoredUsageSettlement>, aether_data_contracts::DataLayerError> {
            self.settlements
                .lock()
                .expect("settlements lock")
                .push(input);
            Ok(None)
        }
    }

    #[async_trait]
    impl ManualProxyNodeCounter for TestUsageStore {
        async fn increment_manual_proxy_node_requests(
            &self,
            _node_id: &str,
            _total_delta: i64,
            _failed_delta: i64,
            _latency_ms: Option<i64>,
        ) -> Result<(), aether_data_contracts::DataLayerError> {
            self.manual_proxy_counter_calls
                .fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[async_trait]
    impl UsageBillingEventEnricher for TestUsageStore {
        async fn enrich_usage_event(&self, event: &mut UsageEvent) -> Result<(), DataLayerError> {
            self.enrich_calls
                .lock()
                .expect("enrich calls lock")
                .push(event.request_id.clone());
            event.data.total_cost_usd = Some(0.456);
            Ok(())
        }
    }

    impl crate::runtime::UsageRuntimeAccess for TestUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            None
        }
    }

    #[async_trait]
    impl UsageRecordWriter for SlowUsageStore {
        async fn upsert_usage_record(
            &self,
            record: UpsertUsageRecord,
        ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError> {
            let active = self
                .active
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                + 1;
            self.max_active
                .fetch_max(active, std::sync::atomic::Ordering::AcqRel);
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.records
                .lock()
                .expect("records lock")
                .push(record.request_id.clone());
            self.active
                .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            Ok(None)
        }
    }

    #[async_trait]
    impl UsageSettlementWriter for SlowUsageStore {
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
    impl ManualProxyNodeCounter for SlowUsageStore {
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
    impl UsageBillingEventEnricher for SlowUsageStore {
        async fn enrich_usage_event(&self, _event: &mut UsageEvent) -> Result<(), DataLayerError> {
            Ok(())
        }
    }

    impl crate::runtime::UsageRuntimeAccess for SlowUsageStore {
        fn has_usage_writer(&self) -> bool {
            true
        }

        fn has_usage_worker_queue(&self) -> bool {
            true
        }

        fn usage_worker_queue(&self) -> Option<Arc<dyn RuntimeQueueStore>> {
            None
        }

        fn usage_worker_should_defer_for_database_pressure(&self) -> bool {
            self.db_pressure.load(Ordering::Acquire)
        }
    }

    #[async_trait]
    impl UsageEventRecorder for SelectiveFailingRecorder {
        async fn record_usage_event(&self, event: &UsageEvent) -> Result<(), DataLayerError> {
            self.calls
                .lock()
                .expect("calls lock")
                .push(event.request_id.clone());
            if event.request_id == "req-worker-poison" {
                return Err(DataLayerError::UnexpectedValue(
                    "permanent test error".to_string(),
                ));
            }
            Ok(())
        }
    }

    fn sample_event() -> UsageEvent {
        UsageEvent::new(
            UsageEventType::Completed,
            "req-worker-123".to_string(),
            UsageEventData {
                user_id: Some("user-worker-123".to_string()),
                api_key_id: Some("api-key-worker-123".to_string()),
                provider_name: "openai".to_string(),
                provider_id: Some("provider-worker-123".to_string()),
                provider_endpoint_id: Some("endpoint-worker-123".to_string()),
                provider_api_key_id: Some("provider-key-worker-123".to_string()),
                model: "gpt-5".to_string(),
                api_format: Some("openai:chat".to_string()),
                endpoint_api_format: Some("openai:chat".to_string()),
                is_stream: Some(false),
                status_code: Some(200),
                input_tokens: Some(4),
                output_tokens: Some(6),
                total_tokens: Some(10),
                response_time_ms: Some(52),
                ..UsageEventData::default()
            },
        )
    }

    #[tokio::test]
    async fn write_event_record_persists_usage_and_triggers_settlement() {
        let store = TestUsageStore::default();
        let event = sample_event();

        write_event_record(&store, &event)
            .await
            .expect("worker should write usage record");

        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request_id, "req-worker-123");
        assert_eq!(records[0].status, "completed");
        drop(records);

        let settlements = store.settlements.lock().expect("settlements lock");
        assert_eq!(settlements.len(), 1);
        assert_eq!(settlements[0].request_id, "req-worker-123");
    }

    #[tokio::test]
    async fn replayable_usage_write_does_not_duplicate_transport_owned_proxy_counter() {
        let store = TestUsageStore::default();
        let mut event = sample_event();
        event.data.request_metadata = Some(serde_json::json!({
            "proxy": {"mode": "manual", "node_id": "manual-node-1"}
        }));

        write_event_record(&store, &event)
            .await
            .expect("first usage write should succeed");
        write_event_record(&store, &event)
            .await
            .expect("replayed usage write should succeed");

        assert_eq!(
            store.manual_proxy_counter_calls.load(Ordering::Acquire),
            0,
            "proxy traffic belongs to the transport attempt, not the replayable usage worker"
        );
    }

    #[tokio::test]
    async fn data_event_recorder_enriches_terminal_event_before_write() {
        let store = Arc::new(TestUsageStore::default());
        let recorder = super::UsageDataEventRecorder::new(Arc::clone(&store));
        let event = sample_event();

        recorder
            .record_usage_event(&event)
            .await
            .expect("recorder should enrich and write usage");

        assert_eq!(
            store
                .enrich_calls
                .lock()
                .expect("enrich calls lock")
                .as_slice(),
            ["req-worker-123"]
        );
        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].total_cost_usd, Some(0.456));
    }

    #[tokio::test]
    async fn data_event_recorder_skips_enrichment_for_lifecycle_event() {
        let store = Arc::new(TestUsageStore::default());
        let recorder = super::UsageDataEventRecorder::new(Arc::clone(&store));
        let mut event = sample_event();
        event.event_type = UsageEventType::Pending;

        recorder
            .record_usage_event(&event)
            .await
            .expect("recorder should write lifecycle usage");

        assert!(store
            .enrich_calls
            .lock()
            .expect("enrich calls lock")
            .is_empty());
        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].total_cost_usd, None);
    }

    #[tokio::test]
    async fn data_event_recorder_serializes_same_request_id_writes() {
        let store = Arc::new(SlowUsageStore::default());
        let recorder = Arc::new(super::UsageDataEventRecorder::new(Arc::clone(&store)));
        let mut first = sample_event();
        first.request_id = "req-same".to_string();
        first.event_type = UsageEventType::Pending;
        let mut second = sample_event();
        second.request_id = "req-same".to_string();
        second.event_type = UsageEventType::Completed;

        let first_recorder = Arc::clone(&recorder);
        let second_recorder = Arc::clone(&recorder);
        tokio::try_join!(
            async move { first_recorder.record_usage_event(&first).await },
            async move { second_recorder.record_usage_event(&second).await }
        )
        .expect("same request writes should both succeed");

        assert_eq!(
            store.max_active.load(std::sync::atomic::Ordering::Acquire),
            1
        );
        assert_eq!(store.records.lock().expect("records lock").len(), 2);
    }

    #[tokio::test]
    async fn data_event_recorder_defers_when_database_pool_is_under_pressure() {
        let store = Arc::new(SlowUsageStore::default());
        store.db_pressure.store(true, Ordering::Release);
        let gate = Arc::new(UsageWorkerRecordConcurrencyGate::new(1));
        let recorder = super::UsageDataEventRecorder::with_record_gate_and_database_pressure_defer(
            Arc::clone(&store),
            Some(Arc::clone(&gate)),
        );

        recorder
            .record_usage_event(&sample_event())
            .await
            .expect("recorder should write after brief defer");

        assert_eq!(gate.deferred_total(), 1);
        assert_eq!(store.records.lock().expect("records lock").len(), 1);
    }

    #[tokio::test]
    async fn usage_worker_record_gate_limits_concurrent_record_writes() {
        let runner = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue_runner: Arc<dyn RuntimeQueueStore> = runner.clone();
        let store = Arc::new(SlowUsageStore::default());
        let gate = Arc::new(UsageWorkerRecordConcurrencyGate::new(2));
        let config = UsageRuntimeConfig {
            enabled: true,
            stream_key: "usage:test:worker:record-gate".to_string(),
            consumer_group: "usage:test:worker:record-gate-group".to_string(),
            dlq_stream_key: "usage:test:worker:record-gate-dlq".to_string(),
            consumer_batch_size: 1,
            consumer_block_ms: 1,
            worker_record_concurrency_limit: Some(2),
            ..UsageRuntimeConfig::default()
        };
        let mut handles = Vec::new();
        for worker_index in 0..4 {
            let worker = build_usage_queue_worker_with_record_gate(
                Arc::clone(&queue_runner),
                Arc::clone(&store),
                config.clone(),
                Some(Arc::clone(&gate)),
                Some(worker_index),
            )
            .expect("worker should build");
            worker
                .queue
                .ensure_consumer_group()
                .await
                .expect("group should initialize");
            handles.push(tokio::spawn(async move {
                let entries = worker
                    .queue
                    .read_group(&worker.consumer)
                    .await
                    .expect("event should read");
                worker
                    .process_entries(entries)
                    .await
                    .expect("event should process");
            }));
        }

        for index in 0..4 {
            let mut event = sample_event();
            event.request_id = format!("req-record-gate-{index}");
            UsageQueue::new(queue_runner.clone(), config.clone())
                .expect("queue should build")
                .enqueue(&event)
                .await
                .expect("event should enqueue");
        }

        for handle in handles {
            handle.await.expect("worker should complete");
        }

        assert_eq!(
            store.max_active.load(std::sync::atomic::Ordering::Acquire),
            2
        );
        assert_eq!(gate.max_in_flight(), 2);
        assert!(gate.wait_total() > 0);
        assert_eq!(store.records.lock().expect("records lock").len(), 4);
    }

    #[tokio::test]
    async fn usage_worker_defers_reclaim_until_inflight_read_is_processed() {
        let event = sample_event();
        let queue = Arc::new(ReadReclaimRaceProbeQueue::new(RuntimeQueueEntry {
            id: "1-0".to_string(),
            fields: event
                .to_stream_fields()
                .expect("usage event should serialize"),
        }));
        let queue_runner: Arc<dyn RuntimeQueueStore> = queue.clone();
        let config = UsageRuntimeConfig {
            enabled: true,
            stream_key: "usage:test:worker:read-reclaim-race".to_string(),
            consumer_group: "usage:test:worker:read-reclaim-race-group".to_string(),
            dlq_stream_key: "usage:test:worker:read-reclaim-race-dlq".to_string(),
            consumer_batch_size: 1,
            consumer_block_ms: 1_000,
            reclaim_interval_ms: 10,
            reclaim_idle_ms: 1,
            reclaim_count: 1,
            ..UsageRuntimeConfig::default()
        };
        let recorder = Arc::new(SelectiveFailingRecorder::default());
        let worker_recorder: Arc<dyn UsageEventRecorder> = recorder.clone();
        let control = UsageWorkerControl::default();
        let (telemetry_tx, _telemetry_rx) = tokio::sync::mpsc::channel(8);
        let worker = UsageQueueWorker::new(queue_runner, worker_recorder, config, None)
            .expect("worker should build")
            .with_supervisor(control.clone(), telemetry_tx);
        let handle = tokio::spawn(worker.run());

        tokio::time::timeout(Duration::from_secs(1), async {
            while queue.read_calls.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker should start the blocking read");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            queue.reclaim_calls.load(Ordering::Acquire),
            0,
            "reclaim must not run while XREADGROUP can still return the same PEL entry"
        );
        assert!(recorder.calls.lock().expect("calls lock").is_empty());

        queue.release_read.notify_one();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !queue.acked.load(Ordering::Acquire)
                || queue.reclaim_calls.load(Ordering::Acquire) == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("read entry should be processed before deferred reclaim runs");

        assert_eq!(
            recorder.calls.lock().expect("calls lock").as_slice(),
            [event.request_id.as_str()],
            "the stream entry must be recorded exactly once"
        );
        assert_eq!(queue.read_completed.load(Ordering::Acquire), 1);
        assert!(!queue.first_read_cancelled.load(Ordering::Acquire));

        control.request_shutdown();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("worker should stop promptly")
            .expect("worker task should not panic");
    }

    #[tokio::test]
    async fn usage_worker_shutdown_cancels_blocking_read_promptly() {
        let event = sample_event();
        let queue = Arc::new(ReadReclaimRaceProbeQueue::new(RuntimeQueueEntry {
            id: "2-0".to_string(),
            fields: event
                .to_stream_fields()
                .expect("usage event should serialize"),
        }));
        let queue_runner: Arc<dyn RuntimeQueueStore> = queue.clone();
        let config = UsageRuntimeConfig {
            enabled: true,
            stream_key: "usage:test:worker:shutdown-read".to_string(),
            consumer_group: "usage:test:worker:shutdown-read-group".to_string(),
            dlq_stream_key: "usage:test:worker:shutdown-read-dlq".to_string(),
            consumer_batch_size: 1,
            consumer_block_ms: 60_000,
            reclaim_interval_ms: 10,
            reclaim_idle_ms: 1,
            reclaim_count: 1,
            ..UsageRuntimeConfig::default()
        };
        let recorder: Arc<dyn UsageEventRecorder> = Arc::new(SelectiveFailingRecorder::default());
        let control = UsageWorkerControl::default();
        let (telemetry_tx, _telemetry_rx) = tokio::sync::mpsc::channel(8);
        let worker = UsageQueueWorker::new(queue_runner, recorder, config, None)
            .expect("worker should build")
            .with_supervisor(control.clone(), telemetry_tx);
        let handle = tokio::spawn(worker.run());

        tokio::time::timeout(Duration::from_secs(1), async {
            while queue.read_calls.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker should start the blocking read");

        control.request_shutdown();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("shutdown should interrupt the blocking read")
            .expect("worker task should not panic");

        assert!(queue.first_read_cancelled.load(Ordering::Acquire));
        assert_eq!(queue.read_completed.load(Ordering::Acquire), 0);
        assert_eq!(queue.reclaim_calls.load(Ordering::Acquire), 0);
    }

    #[test]
    fn usage_event_record_error_classifies_permanent_failures() {
        assert!(usage_event_record_error_is_permanent(
            &DataLayerError::UnexpectedValue("bad payload".to_string())
        ));
        assert!(usage_event_record_error_is_permanent(
            &DataLayerError::Postgres(
                "error returned from database: violates foreign key constraint (SQLSTATE 23503)"
                    .to_string()
            )
        ));
        assert!(!usage_event_record_error_is_permanent(
            &DataLayerError::Redis("connection refused".to_string())
        ));
        assert!(!usage_event_record_error_is_permanent(
            &DataLayerError::TimedOut("postgres acquire".to_string())
        ));
    }

    #[tokio::test]
    async fn process_entries_dead_letters_permanent_record_error_and_continues() {
        let runner = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue_runner: Arc<dyn RuntimeQueueStore> = runner.clone();
        let recorder = Arc::new(SelectiveFailingRecorder::default());
        let config = UsageRuntimeConfig {
            enabled: true,
            stream_key: "usage:test:worker:events".to_string(),
            consumer_group: "usage:test:worker:group".to_string(),
            dlq_stream_key: "usage:test:worker:dlq".to_string(),
            consumer_batch_size: 10,
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let worker = UsageQueueWorker::new(queue_runner, recorder.clone(), config, None)
            .expect("worker should build");
        worker
            .queue
            .ensure_consumer_group()
            .await
            .expect("group should initialize");

        let mut poison = sample_event();
        poison.request_id = "req-worker-poison".to_string();
        let mut ok = sample_event();
        ok.request_id = "req-worker-ok".to_string();
        worker
            .queue
            .enqueue(&poison)
            .await
            .expect("poison event should enqueue");
        worker
            .queue
            .enqueue(&ok)
            .await
            .expect("ok event should enqueue");

        let entries = worker
            .queue
            .read_group(&worker.consumer)
            .await
            .expect("events should read");
        assert_eq!(entries.len(), 2);

        worker
            .process_entries(entries)
            .await
            .expect("permanent failure should not block batch");

        assert_eq!(
            recorder.calls.lock().expect("calls lock").as_slice(),
            ["req-worker-poison", "req-worker-ok"]
        );

        runner
            .ensure_consumer_group(
                "usage:test:worker:dlq",
                "usage:test:worker:dlq-group",
                "0-0",
            )
            .await
            .expect("dlq group should initialize");
        let dlq_entries = runner
            .read_group(
                "usage:test:worker:dlq",
                "usage:test:worker:dlq-group",
                "usage-test-dlq-consumer",
                10,
                Some(1),
            )
            .await
            .expect("dlq should read");
        assert_eq!(dlq_entries.len(), 1);
        let payload = dlq_entries[0]
            .fields
            .get("payload")
            .expect("dlq payload should exist");
        assert!(payload.contains("req-worker-poison"));
        assert!(payload.contains("permanent test error"));
    }
}
