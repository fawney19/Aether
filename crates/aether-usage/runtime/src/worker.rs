use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
use aether_data_contracts::DataLayerError;
use aether_runtime_state::{RuntimeQueueEntry, RuntimeQueueStore};
use async_trait::async_trait;
use tokio::sync::{mpsc, Notify};
use tracing::warn;

use crate::event_capture_budget::{shared_capture_memory_budget, EventCaptureMemoryBudget};
use crate::executor::spawn_on_usage_background_runtime;
use crate::keyed_lock::KeyedAsyncLockPool;
use crate::queue::UsageDeadLetterOutcome;
use crate::runtime::{
    UsageBillingEventEnricher, UsageRuntimeAccess, UsageWorkerRecordConcurrencyGate,
};
use crate::settlement::{
    reconcile_usage_policy_cost_for_event_with_result, settle_usage_with_reconciled_cost,
};
use crate::{
    build_upsert_usage_record_from_event, UsageEvent, UsageEventType, UsageQueue,
    UsageRuntimeConfig, UsageSettlementWriter,
};

const USAGE_WORKER_DB_PRESSURE_DEFER_MS: u64 = 10;
const USAGE_WORKER_ACK_CHUNK_SIZE: usize = 100;

enum EntryDisposition {
    NeedsAck,
    Complete,
    Deferred(DataLayerError),
}

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
        enrich_terminal_event(self.data.as_ref(), &mut event).await?;
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
    capture_memory_budget: Arc<EventCaptureMemoryBudget>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageWorkerControl {
    shutdown: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
}

impl UsageWorkerControl {
    pub(crate) fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.shutdown_notify.notify_waiters();
        self.shutdown_notify.notify_one();
    }

    fn should_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    pub(crate) async fn wait_for_shutdown(&self) {
        loop {
            let notified = self.shutdown_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.should_shutdown() {
                return;
            }
            notified.await;
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
            capture_memory_budget: shared_capture_memory_budget(),
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

    pub(crate) fn with_shutdown(mut self, control: UsageWorkerControl) -> Self {
        self.control = Some(control);
        self
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
        let mut reclaim_cursor = "0-0".to_string();

        loop {
            if self.should_shutdown() {
                break;
            }

            let result = {
                let mut read_future = Box::pin(self.queue.read_group_reserved(&self.consumer));
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
                Ok(batch) => {
                    self.report_read(batch.entries.len(), batch.requested_count);
                    let reservation = batch.reservation;
                    let result = self.process_entries(batch.entries).await;
                    // The raw fields and decoded event must be gone, and ACK must finish,
                    // before another worker can reuse this batch's receive allowance.
                    drop(reservation);
                    if let Err(err) = result {
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
                self.reclaim_stale_entries(&mut reclaim_cursor).await;
            }
        }
    }

    async fn wait_for_shutdown(&self) {
        match self.control.as_ref() {
            Some(control) => control.wait_for_shutdown().await,
            None => std::future::pending().await,
        }
    }

    async fn reclaim_stale_entries(&self, cursor: &mut String) {
        let result = tokio::select! {
            biased;
            _ = self.wait_for_shutdown() => return,
            result = self.queue.claim_stale_page_reserved(&self.consumer, cursor) => result,
        };
        match result {
            Ok(batch) => {
                let reservation = batch.reservation;
                let page = batch.page;
                // An empty page can still advance past a large active PEL prefix.
                // Failed writes remain pending and are revisited after the scan wraps.
                *cursor = page.next_start_id;
                self.report_reclaimed(page.entries.len());
                let result = self.process_entries(page.entries).await;
                drop(page.deleted_ids);
                drop(reservation);
                if let Err(err) = result {
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

    fn report_read(&self, entries_read: usize, requested_count: usize) {
        self.report(UsageWorkerObservation::read(
            self.worker_index,
            entries_read,
            requested_count,
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
        let mut deferred_error = None;
        for entry in entries {
            let id = entry.id.clone();
            let result = self.process_entry(entry).await;
            match result {
                Ok(EntryDisposition::NeedsAck) => {
                    ack_ids.push(id);
                    if ack_ids.len() >= USAGE_WORKER_ACK_CHUNK_SIZE {
                        self.acknowledge_entries(&ack_ids).await?;
                        ack_ids.clear();
                    }
                }
                Ok(EntryDisposition::Complete) => {}
                Ok(EntryDisposition::Deferred(err)) => {
                    // An entry that cannot fit the DLQ encoder must not indefinitely block
                    // the healthy entries returned with it on every reclaim pass.
                    deferred_error.get_or_insert(err);
                }
                Err(err) => {
                    if !ack_ids.is_empty() {
                        let _ = self.acknowledge_entries(&ack_ids).await;
                    }
                    return Err(err);
                }
            }
        }

        if !ack_ids.is_empty() {
            self.acknowledge_entries(&ack_ids).await?;
        }

        match deferred_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    async fn acknowledge_entries(&self, ids: &[String]) -> Result<(), DataLayerError> {
        let acked = self.queue.ack_and_delete_counted(ids).await?;
        if acked > 0 {
            self.report_acked(acked);
        }
        Ok(())
    }

    async fn dead_letter_entry(
        &self,
        entry: RuntimeQueueEntry,
        error: DataLayerError,
        event_name: &'static str,
    ) -> Result<EntryDisposition, DataLayerError> {
        let id = entry.id.clone();
        let outcome = self
            .queue
            .transfer_dead_letter_owned(entry, error.to_string())
            .await?;
        let (destination_id, disposition) = match outcome {
            UsageDeadLetterOutcome::Transferred {
                destination_id,
                acked,
            } => {
                if acked > 0 {
                    self.report_acked(acked);
                }
                (destination_id, EntryDisposition::Complete)
            }
            UsageDeadLetterOutcome::Appended { destination_id } => {
                (destination_id, EntryDisposition::NeedsAck)
            }
            UsageDeadLetterOutcome::NotPending => {
                warn!(
                    event_name = "usage_worker_dead_letter_source_not_pending",
                    log_type = "ops",
                    worker_consumer = %self.consumer,
                    worker_group = %self.config.consumer_group,
                    entry_id = %id,
                    error = %error,
                    "usage worker skipped dead letter transfer because the source is no longer pending"
                );
                return Ok(EntryDisposition::Complete);
            }
            UsageDeadLetterOutcome::EncodingDeferred { error } => {
                warn!(
                    event_name = "usage_worker_dead_letter_encoding_deferred",
                    log_type = "ops",
                    worker_consumer = %self.consumer,
                    worker_group = %self.config.consumer_group,
                    entry_id = %id,
                    error = %error,
                    "usage worker retained entry pending after dead letter encoding failed"
                );
                return Ok(EntryDisposition::Deferred(error));
            }
        };
        self.report_dead_lettered(1);
        warn!(
            event_name,
            log_type = "ops",
            worker_consumer = %self.consumer,
            worker_group = %self.config.consumer_group,
            entry_id = %id,
            dead_letter_id = %destination_id,
            error = %error,
            "usage worker appended queue entry to dead letter"
        );
        Ok(disposition)
    }

    async fn process_entry(
        &self,
        entry: RuntimeQueueEntry,
    ) -> Result<EntryDisposition, DataLayerError> {
        let event = match UsageEvent::from_stream_fields_with_capture_budget(
            &entry.fields,
            Arc::clone(&self.capture_memory_budget),
        ) {
            Ok(event) => event,
            Err(err) => {
                return self
                    .dead_letter_entry(entry, err, "usage_worker_entry_decode_dead_lettered")
                    .await;
            }
        };

        match self.recorder.record_usage_event(&event).await {
            Ok(()) => Ok(EntryDisposition::NeedsAck),
            Err(err) if usage_event_record_error_is_permanent(&err) => {
                warn!(
                    event_name = "usage_worker_entry_record_permanent_failed",
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
                    "usage worker encountered a non-retryable usage record failure"
                );
                drop(event);
                self.dead_letter_entry(entry, err, "usage_worker_entry_record_dead_lettered")
                    .await
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
    let reconciled = reconcile_usage_policy_cost_for_event_with_result(data, event).await?;
    let record = build_upsert_usage_record_from_event(event)?;
    if let Some(stored) = data.upsert_usage_record(record).await? {
        settle_usage_with_reconciled_cost(data, &stored, reconciled).await?;
    }
    // Manual proxy traffic is counted at the actual transport-attempt boundary. Usage events are
    // replayable, so emitting that side effect here would count normal requests and reclaims twice.
    Ok(())
}

async fn enrich_terminal_event<T>(data: &T, event: &mut UsageEvent) -> Result<(), DataLayerError>
where
    T: UsageBillingEventEnricher + Send + Sync,
{
    if !matches!(
        event.event_type,
        UsageEventType::Completed | UsageEventType::Failed | UsageEventType::Cancelled
    ) {
        return Ok(());
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
        return Err(err);
    }
    Ok(())
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
    mod dead_letter_transfer {
        include!("worker_dead_letter_tests.rs");
    }
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use aether_data_contracts::repository::settlement::{
        ReconcileUsagePolicyCostInput, StoredUsagePolicyCostReservation, StoredUsageSettlement,
        UsageSettlementInput,
    };
    use aether_data_contracts::repository::usage::{
        StoredRequestUsageAudit, UpsertUsageRecord, UsageBodyCaptureState,
    };
    use aether_data_contracts::DataLayerError;
    use aether_runtime_state::{
        MemoryRuntimeStateConfig, RuntimeQueueEntry, RuntimeQueueReclaimConfig,
        RuntimeQueueReclaimPage, RuntimeQueueStats, RuntimeQueueStore, RuntimeState,
    };
    use async_trait::async_trait;
    use tokio::sync::Notify;

    use super::{
        build_usage_queue_worker_with_record_gate, usage_event_record_error_is_permanent,
        write_event_record, ManualProxyNodeCounter, UsageEventRecorder, UsageQueueWorker,
        UsageRecordWriter, UsageWorkerControl,
    };
    use crate::dead_letter_encoding::DeadLetterEncodingBudget;
    use crate::event_capture_budget::EventCaptureMemoryBudget;
    use crate::queue_read_budget::QueueReadBudget;
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
        reconciliations: Mutex<Vec<ReconcileUsagePolicyCostInput>>,
        enrich_calls: Mutex<Vec<String>>,
        enrich_outcomes: Mutex<VecDeque<TestEnrichmentOutcome>>,
        manual_proxy_counter_calls: AtomicUsize,
    }

    enum TestEnrichmentOutcome {
        TimedOut,
        Unpriced,
        Priced { listed: f64, actual: f64 },
    }

    #[derive(Default)]
    struct ControlledRecorder {
        entered: Notify,
        release: Notify,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl UsageEventRecorder for ControlledRecorder {
        async fn record_usage_event(&self, _event: &UsageEvent) -> Result<(), DataLayerError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    #[derive(Default)]
    struct SelectiveFailingRecorder {
        calls: Mutex<Vec<String>>,
    }

    enum CaptureRecordOutcome {
        RetryOnce,
        PermanentFailure,
        Wait,
    }

    struct CaptureBudgetRecorder {
        budget: Arc<EventCaptureMemoryBudget>,
        outcome: CaptureRecordOutcome,
        calls: AtomicUsize,
        entered: Notify,
    }

    #[async_trait]
    impl UsageEventRecorder for CaptureBudgetRecorder {
        async fn record_usage_event(&self, event: &UsageEvent) -> Result<(), DataLayerError> {
            let call = self.calls.fetch_add(1, Ordering::AcqRel);
            assert_eq!(event.data.input_tokens, Some(4));
            assert_eq!(event.data.output_tokens, Some(6));
            assert_eq!(event.data.total_tokens, Some(10));
            assert_eq!(event.data.cache_read_input_tokens, Some(0));
            assert_eq!(event.data.actual_total_cost_usd, Some(0.123));
            let metadata = event.data.request_metadata.as_ref().expect("billing facts");
            assert_eq!(metadata["requested_reasoning_effort"], "high");
            assert_eq!(metadata["provider_service_tier"], "priority");
            assert_eq!(metadata["provider_actual_service_tier"], "default");
            if event.data.response_body.is_some() {
                let retained = self.budget.retained_bytes();
                assert!(retained > 0);
                let recorder_copy = event.clone();
                assert!(recorder_copy.data.response_body.is_some());
                assert!(self.budget.retained_bytes() > retained);
                drop(recorder_copy);
                assert_eq!(self.budget.retained_bytes(), retained);
            } else {
                assert_eq!(self.budget.retained_bytes(), 0);
                assert_eq!(
                    event.data.response_body_state,
                    Some(UsageBodyCaptureState::Truncated)
                );
            }
            self.entered.notify_one();
            match self.outcome {
                CaptureRecordOutcome::RetryOnce if call == 0 => Err(DataLayerError::TimedOut(
                    "retry test database write".to_string(),
                )),
                CaptureRecordOutcome::RetryOnce => Ok(()),
                CaptureRecordOutcome::PermanentFailure => Err(DataLayerError::UnexpectedValue(
                    "permanent capture test error".to_string(),
                )),
                CaptureRecordOutcome::Wait => std::future::pending().await,
            }
        }
    }

    async fn capture_budget_worker(
        budget_bytes: usize,
        outcome: CaptureRecordOutcome,
    ) -> (
        Arc<RuntimeState>,
        UsageQueueWorker,
        Arc<CaptureBudgetRecorder>,
    ) {
        let runner = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let queue_runner: Arc<dyn RuntimeQueueStore> = runner.clone();
        let budget = Arc::new(EventCaptureMemoryBudget::new(budget_bytes));
        let recorder = Arc::new(CaptureBudgetRecorder {
            budget: Arc::clone(&budget),
            outcome,
            calls: AtomicUsize::new(0),
            entered: Notify::new(),
        });
        let config = UsageRuntimeConfig {
            enabled: true,
            stream_key: "usage:test:worker:capture".to_string(),
            consumer_group: "usage:test:worker:capture-group".to_string(),
            dlq_stream_key: "usage:test:worker:capture-dlq".to_string(),
            consumer_batch_size: 1,
            consumer_block_ms: 1,
            ..UsageRuntimeConfig::default()
        };
        let mut worker = UsageQueueWorker::new(queue_runner, recorder.clone(), config, None)
            .expect("worker should build");
        worker.capture_memory_budget = budget;
        worker.queue =
            worker
                .queue
                .with_dead_letter_encoding_budget(Arc::new(DeadLetterEncodingBudget::new(
                    64 * 1024 * 1024,
                    4,
                )));
        worker
            .queue
            .ensure_consumer_group()
            .await
            .expect("consumer group");
        (runner, worker, recorder)
    }

    fn captured_worker_event() -> UsageEvent {
        let mut event = sample_event();
        event.data.model = "gpt-5.6-sol".to_string();
        event.data.endpoint_api_format = Some("openai:responses".to_string());
        event.data.request_body = Some(serde_json::json!({"reasoning": {"effort": "high"}}));
        event.data.provider_request_body = Some(serde_json::json!({
            "model": "gpt-5.6-sol", "service_tier": "priority"
        }));
        event.data.response_body = Some(serde_json::json!({
            "service_tier": "Default", "output": "x".repeat(1024)
        }));
        event.data.cache_read_input_tokens = Some(0);
        event.data.actual_total_cost_usd = Some(0.123);
        event
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
        reclaim_pages: Mutex<Option<VecDeque<Result<RuntimeQueueReclaimPage, DataLayerError>>>>,
        reclaim_cursors: Mutex<Vec<String>>,
        acked: AtomicBool,
        requested_counts: Mutex<Vec<usize>>,
        block_ack: AtomicBool,
        ack_entered: Notify,
        release_ack: Notify,
        block_reclaim: AtomicBool,
        reclaim_cancelled: AtomicBool,
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
                reclaim_pages: Mutex::new(None),
                reclaim_cursors: Mutex::new(Vec::new()),
                acked: AtomicBool::new(false),
                requested_counts: Mutex::new(Vec::new()),
                block_ack: AtomicBool::new(false),
                ack_entered: Notify::new(),
                release_ack: Notify::new(),
                block_reclaim: AtomicBool::new(false),
                reclaim_cancelled: AtomicBool::new(false),
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
            count: usize,
            _block_ms: Option<u64>,
        ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
            let call_index = self.read_calls.fetch_add(1, Ordering::AcqRel);
            self.requested_counts
                .lock()
                .expect("requested counts lock")
                .push(count);
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

        async fn claim_stale_page(
            &self,
            stream: &str,
            group: &str,
            consumer: &str,
            start_id: &str,
            config: RuntimeQueueReclaimConfig,
        ) -> Result<RuntimeQueueReclaimPage, DataLayerError> {
            self.reclaim_cursors
                .lock()
                .expect("reclaim cursors lock")
                .push(start_id.to_string());
            if self.block_reclaim.load(Ordering::Acquire) {
                let _guard = FirstReadDropGuard {
                    cancelled: &self.reclaim_cancelled,
                    completed: false,
                };
                return std::future::pending().await;
            }
            let scripted = self
                .reclaim_pages
                .lock()
                .expect("reclaim pages lock")
                .as_mut()
                .map(|pages| pages.pop_front().expect("scripted reclaim page"));
            if let Some(page) = scripted {
                self.reclaim_calls.fetch_add(1, Ordering::AcqRel);
                return page;
            }
            Ok(RuntimeQueueReclaimPage {
                next_start_id: "0-0".to_string(),
                entries: self
                    .claim_stale(stream, group, consumer, start_id, config)
                    .await?,
                deleted_ids: Vec::new(),
            })
        }

        async fn ack(
            &self,
            _stream: &str,
            _group: &str,
            ids: &[String],
        ) -> Result<usize, DataLayerError> {
            if self.block_ack.load(Ordering::Acquire) {
                self.ack_entered.notify_one();
                self.release_ack.notified().await;
            }
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

        async fn reconcile_usage_policy_cost(
            &self,
            input: ReconcileUsagePolicyCostInput,
        ) -> Result<Option<StoredUsagePolicyCostReservation>, DataLayerError> {
            self.reconciliations
                .lock()
                .expect("reconciliations lock")
                .push(input);
            Ok(None)
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
            match self
                .enrich_outcomes
                .lock()
                .expect("enrich outcomes lock")
                .pop_front()
            {
                Some(TestEnrichmentOutcome::TimedOut) => {
                    return Err(DataLayerError::TimedOut("test pricing lookup".to_string()));
                }
                Some(TestEnrichmentOutcome::Unpriced) => return Ok(()),
                Some(TestEnrichmentOutcome::Priced { listed, actual }) => {
                    event.data.total_cost_usd = Some(listed);
                    event.data.actual_total_cost_usd = Some(actual);
                    return Ok(());
                }
                None => {}
            }
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
    async fn same_request_id_terminal_events_reconcile_each_reservation_token_before_upsert() {
        let store = TestUsageStore::default();
        let mut first = sample_event();
        first.request_id = "shared-client-trace".to_string();
        first.data.actual_total_cost_usd = Some(0.25);
        first.data.request_metadata = Some(serde_json::json!({
            "plan_usage_reservation_token": "server-token-a"
        }));
        let mut second = first.clone();
        second.data.actual_total_cost_usd = Some(0.75);
        second.data.request_metadata = Some(serde_json::json!({
            "plan_usage_reservation_token": "server-token-b"
        }));

        write_event_record(&store, &first)
            .await
            .expect("first terminal event");
        write_event_record(&store, &second)
            .await
            .expect("second terminal event");

        let reconciliations = store.reconciliations.lock().expect("reconciliations lock");
        assert_eq!(reconciliations.len(), 2);
        assert_eq!(reconciliations[0].request_id, "shared-client-trace");
        assert_eq!(reconciliations[0].reservation_token, "server-token-a");
        assert_eq!(reconciliations[0].actual_cost_units, 25_000_000);
        assert_eq!(reconciliations[1].request_id, "shared-client-trace");
        assert_eq!(reconciliations[1].reservation_token, "server-token-b");
        assert_eq!(reconciliations[1].actual_cost_units, 75_000_000);
        assert_eq!(store.records.lock().expect("records lock").len(), 2);
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
    async fn data_event_recorder_pricing_timeout_stays_pending_until_successful_reclaim() {
        let runner = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let store = Arc::new(TestUsageStore::default());
        store
            .enrich_outcomes
            .lock()
            .expect("enrich outcomes lock")
            .extend([
                TestEnrichmentOutcome::TimedOut,
                TestEnrichmentOutcome::Priced {
                    listed: 0.456,
                    actual: 0.123,
                },
            ]);
        let config = UsageRuntimeConfig {
            consumer_batch_size: 1,
            consumer_block_ms: 1,
            reclaim_count: 1,
            reclaim_idle_ms: 1,
            queue_payload_max_bytes: 4096,
            ..UsageRuntimeConfig::default()
        };
        let budget = Arc::new(QueueReadBudget::new(4096, 4096));
        let mut worker =
            build_usage_queue_worker_with_record_gate(runner, store.clone(), config, None, None)
                .expect("worker should build");
        worker.queue = worker.queue.with_read_budget(Arc::clone(&budget));
        worker.queue.ensure_consumer_group().await.expect("group");
        let mut event = sample_event();
        event.data.request_metadata = Some(serde_json::json!({
            "plan_usage_reservation_token": "pricing-retry-reservation"
        }));
        worker.queue.enqueue(&event).await.expect("enqueue");
        let batch = worker
            .queue
            .read_group_reserved(&worker.consumer)
            .await
            .expect("read");
        let error = worker
            .process_entries(batch.entries)
            .await
            .expect_err("pricing timeout must reach the worker");
        drop(batch.reservation);
        assert!(matches!(error, DataLayerError::TimedOut(_)));
        assert!(store.records.lock().expect("records lock").is_empty());
        assert!(store
            .reconciliations
            .lock()
            .expect("reconciliations lock")
            .is_empty());
        assert!(store
            .settlements
            .lock()
            .expect("settlements lock")
            .is_empty());
        let stats = worker.queue.stats().await.expect("pending stats");
        assert_eq!((stats.stream_length, stats.group_pending), (1, 1));
        assert_eq!(
            worker
                .queue
                .dlq_stats()
                .await
                .expect("dlq stats")
                .stream_length,
            0
        );
        assert_eq!(budget.snapshot().reserved_bytes, 0);

        let mut cursor = "0-0".to_string();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                tokio::time::sleep(Duration::from_millis(1)).await;
                worker.reclaim_stale_entries(&mut cursor).await;
                if !store.records.lock().expect("records lock").is_empty() {
                    break;
                }
            }
        })
        .await
        .expect("pending entry should become reclaimable");

        {
            let records = store.records.lock().expect("records lock");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].total_cost_usd, Some(0.456));
            assert_eq!(records[0].actual_total_cost_usd, Some(0.123));
            assert_eq!(records[0].total_tokens, Some(10));
        }
        {
            let reconciliations = store.reconciliations.lock().expect("reconciliations lock");
            assert_eq!(reconciliations.len(), 1);
            assert_eq!(reconciliations[0].actual_cost_units, 12_300_000);
            assert_eq!(
                reconciliations[0].reservation_token,
                "pricing-retry-reservation"
            );
        }
        assert_eq!(store.settlements.lock().expect("settlements lock").len(), 1);
        assert_eq!(
            store.enrich_calls.lock().expect("enrich calls lock").len(),
            2
        );
        let stats = worker.queue.stats().await.expect("acked stats");
        assert_eq!((stats.stream_length, stats.group_pending), (0, 0));
        assert_eq!(
            worker
                .queue
                .dlq_stats()
                .await
                .expect("dlq stats")
                .stream_length,
            0
        );
        assert_eq!(budget.snapshot().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn data_event_recorder_successful_unpriced_enrichment_keeps_existing_write_behavior() {
        let store = Arc::new(TestUsageStore::default());
        store
            .enrich_outcomes
            .lock()
            .expect("enrich outcomes lock")
            .push_back(TestEnrichmentOutcome::Unpriced);
        let recorder = super::UsageDataEventRecorder::new(Arc::clone(&store));
        recorder
            .record_usage_event(&sample_event())
            .await
            .expect("missing pricing is a successful enrichment outcome");
        let records = store.records.lock().expect("records lock");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].total_cost_usd, None);
        assert_eq!(records[0].actual_total_cost_usd, None);
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
    async fn usage_worker_reclaim_cursor_advances_on_empty_pages_and_retries_read_errors() {
        let event = sample_event();
        let entry = RuntimeQueueEntry {
            id: "43-0".to_string(),
            fields: event.to_stream_fields().expect("event fields"),
        };
        let runner = Arc::new(ReadReclaimRaceProbeQueue::new(entry.clone()));
        let page = |next_start_id: &str, entries, deleted_ids| RuntimeQueueReclaimPage {
            next_start_id: next_start_id.to_string(),
            entries,
            deleted_ids,
        };
        *runner.reclaim_pages.lock().expect("reclaim pages lock") = Some(VecDeque::from([
            Ok(page("11-0", Vec::new(), vec!["9-0".to_string()])),
            Err(DataLayerError::Redis("temporary reclaim error".to_string())),
            Ok(page("42-0", Vec::new(), Vec::new())),
            Ok(page("0-0", vec![entry], Vec::new())),
            Ok(page("0-0", Vec::new(), Vec::new())),
        ]));
        let recorder = Arc::new(SelectiveFailingRecorder::default());
        let worker = UsageQueueWorker::new(
            runner.clone(),
            recorder.clone(),
            UsageRuntimeConfig::default(),
            None,
        )
        .expect("cursor worker");
        let mut cursor = "0-0".to_string();
        for expected in ["11-0", "11-0", "42-0", "0-0", "0-0"] {
            worker.reclaim_stale_entries(&mut cursor).await;
            assert_eq!(cursor, expected);
        }
        assert_eq!(
            runner
                .reclaim_cursors
                .lock()
                .expect("reclaim cursors lock")
                .as_slice(),
            ["0-0", "11-0", "11-0", "42-0", "0-0"]
        );
        assert_eq!(
            recorder.calls.lock().expect("calls lock").as_slice(),
            [event.request_id]
        );
        assert!(runner.acked.load(Ordering::Acquire));
        assert_eq!(runner.reclaim_calls.load(Ordering::Acquire), 5);
    }

    #[tokio::test]
    async fn usage_worker_reclaim_cursor_advances_after_write_failure_and_revisits_on_wrap() {
        struct FailOnceRecorder(AtomicUsize);

        #[async_trait]
        impl UsageEventRecorder for FailOnceRecorder {
            async fn record_usage_event(&self, _event: &UsageEvent) -> Result<(), DataLayerError> {
                if self.0.fetch_add(1, Ordering::AcqRel) == 0 {
                    Err(DataLayerError::TimedOut(
                        "temporary write failure".to_string(),
                    ))
                } else {
                    Ok(())
                }
            }
        }

        let entry = RuntimeQueueEntry {
            id: "43-0".to_string(),
            fields: sample_event().to_stream_fields().expect("event fields"),
        };
        let runner = Arc::new(ReadReclaimRaceProbeQueue::new(entry.clone()));
        *runner.reclaim_pages.lock().expect("reclaim pages lock") = Some(VecDeque::from([
            Ok(RuntimeQueueReclaimPage {
                next_start_id: "50-0".to_string(),
                entries: vec![entry.clone()],
                deleted_ids: Vec::new(),
            }),
            Ok(RuntimeQueueReclaimPage {
                next_start_id: "0-0".to_string(),
                entries: Vec::new(),
                deleted_ids: Vec::new(),
            }),
            Ok(RuntimeQueueReclaimPage {
                next_start_id: "0-0".to_string(),
                entries: vec![entry],
                deleted_ids: Vec::new(),
            }),
        ]));
        let recorder = Arc::new(FailOnceRecorder(AtomicUsize::new(0)));
        let worker = UsageQueueWorker::new(
            runner.clone(),
            recorder.clone(),
            UsageRuntimeConfig::default(),
            None,
        )
        .expect("cursor worker");
        let mut cursor = "0-0".to_string();
        worker.reclaim_stale_entries(&mut cursor).await;
        assert_eq!(cursor, "50-0");
        assert!(!runner.acked.load(Ordering::Acquire));
        worker.reclaim_stale_entries(&mut cursor).await;
        worker.reclaim_stale_entries(&mut cursor).await;
        assert_eq!(cursor, "0-0");
        assert!(runner.acked.load(Ordering::Acquire));
        assert_eq!(recorder.0.load(Ordering::Acquire), 2);
        assert_eq!(
            runner
                .reclaim_cursors
                .lock()
                .expect("reclaim cursors lock")
                .as_slice(),
            ["0-0", "50-0", "0-0"]
        );
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

    fn receive_budget_worker_config() -> UsageRuntimeConfig {
        UsageRuntimeConfig {
            consumer_batch_size: 128,
            consumer_block_ms: 60_000,
            reclaim_interval_ms: 60_000,
            queue_payload_max_bytes: 4096,
            ..UsageRuntimeConfig::default()
        }
    }

    #[tokio::test]
    async fn usage_worker_shared_read_budget_waits_for_slow_recorder_and_releases_on_cancel() {
        let runner = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let budget = Arc::new(QueueReadBudget::new(4096, 4096));
        let config = receive_budget_worker_config();
        let first_recorder = Arc::new(ControlledRecorder::default());
        let second_recorder = Arc::new(ControlledRecorder::default());
        let first_control = UsageWorkerControl::default();
        let (telemetry, _observations) = tokio::sync::mpsc::channel(16);
        let mut first = UsageQueueWorker::new(
            runner.clone(),
            first_recorder.clone(),
            config.clone(),
            Some(0),
        )
        .expect("first worker")
        .with_supervisor(first_control.clone(), telemetry);
        first.queue = first.queue.with_read_budget(Arc::clone(&budget));
        let queue = first.queue.clone();
        let mut second = UsageQueueWorker::new(runner, second_recorder.clone(), config, Some(1))
            .expect("second worker");
        second.queue = second.queue.with_read_budget(Arc::clone(&budget));
        for index in 0..2 {
            let mut event = sample_event();
            event.request_id = format!("receive-budget-{index}");
            queue.enqueue(&event).await.expect("enqueue");
        }

        let first_handle = tokio::spawn(first.run());
        tokio::time::timeout(Duration::from_secs(1), first_recorder.entered.notified())
            .await
            .expect("first recorder should hold a batch");
        assert!(budget.snapshot().reserved_bytes > 0);
        let second_handle = tokio::spawn(second.run());
        tokio::time::timeout(Duration::from_secs(1), async {
            while budget.snapshot().waiters == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second worker should wait before reading");
        assert_eq!(second_recorder.calls.load(Ordering::Acquire), 0);
        let stats = queue.stats().await.expect("blocked stats");
        assert_eq!((stats.stream_length, stats.group_pending), (2, 1));

        first_control.request_shutdown();
        first_recorder.release.notify_one();
        tokio::time::timeout(Duration::from_secs(1), first_handle)
            .await
            .expect("first worker should finish its acquired batch")
            .expect("first worker task");
        tokio::time::timeout(Duration::from_secs(1), second_recorder.entered.notified())
            .await
            .expect("second worker should acquire released allowance");
        assert!(budget.snapshot().reserved_bytes > 0);
        second_handle.abort();
        assert!(second_handle
            .await
            .expect_err("cancel worker")
            .is_cancelled());
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert_eq!(budget.snapshot().waiters, 0);
        let stats = queue.stats().await.expect("cancelled stats");
        assert_eq!((stats.stream_length, stats.group_pending), (1, 1));
    }

    #[tokio::test]
    async fn usage_worker_read_budget_survives_ack_and_reports_actual_requested_count() {
        let runner = Arc::new(ReadReclaimRaceProbeQueue::new(RuntimeQueueEntry {
            id: "7-0".to_string(),
            fields: sample_event().to_stream_fields().expect("event fields"),
        }));
        runner.block_ack.store(true, Ordering::Release);
        let budget = Arc::new(QueueReadBudget::new(4096, 4096));
        let control = UsageWorkerControl::default();
        let (telemetry, mut observations) = tokio::sync::mpsc::channel(16);
        let mut worker = UsageQueueWorker::new(
            runner.clone(),
            Arc::new(SelectiveFailingRecorder::default()),
            receive_budget_worker_config(),
            Some(3),
        )
        .expect("worker")
        .with_supervisor(control.clone(), telemetry);
        worker.queue = worker.queue.with_read_budget(Arc::clone(&budget));
        runner.release_read.notify_one();
        let handle = tokio::spawn(worker.run());
        tokio::time::timeout(Duration::from_secs(1), runner.ack_entered.notified())
            .await
            .expect("worker should reach ACK");
        assert!(budget.snapshot().reserved_bytes > 0);
        assert!(!runner.acked.load(Ordering::Acquire));
        let observation = observations.recv().await.expect("read observation");
        assert_eq!(observation.worker_index, Some(3));
        assert_eq!(observation.entries_read, 1);
        assert_eq!(observation.batch_size, 1);
        assert_eq!(
            runner
                .requested_counts
                .lock()
                .expect("requested counts lock")
                .as_slice(),
            [1]
        );

        control.request_shutdown();
        runner.release_ack.notify_one();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("worker should finish ACK before shutdown")
            .expect("worker task");
        assert!(runner.acked.load(Ordering::Acquire));
        assert_eq!(budget.snapshot().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn usage_worker_shutdown_cancels_reclaim_budget_wait_without_reading() {
        use std::future::Future;
        use std::task::Poll;

        let runner = Arc::new(ReadReclaimRaceProbeQueue::new(RuntimeQueueEntry {
            id: "8-0".to_string(),
            fields: sample_event().to_stream_fields().expect("event fields"),
        }));
        let budget = Arc::new(QueueReadBudget::new(4096, 4096));
        let (_, occupied) = budget.reserve(1, 4096).await.expect("occupy budget");
        let control = UsageWorkerControl::default();
        let (telemetry, _observations) = tokio::sync::mpsc::channel(16);
        let mut worker = UsageQueueWorker::new(
            runner.clone(),
            Arc::new(SelectiveFailingRecorder::default()),
            receive_budget_worker_config(),
            None,
        )
        .expect("worker")
        .with_supervisor(control.clone(), telemetry);
        worker.queue = worker.queue.with_read_budget(Arc::clone(&budget));
        let mut cursor = "8-0".to_string();
        let mut reclaim = Box::pin(worker.reclaim_stale_entries(&mut cursor));
        std::future::poll_fn(|cx| {
            assert!(reclaim.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(budget.snapshot().waiters, 1);
        assert!(runner
            .reclaim_cursors
            .lock()
            .expect("cursors lock")
            .is_empty());
        control.request_shutdown();
        tokio::time::timeout(Duration::from_secs(1), &mut reclaim)
            .await
            .expect("shutdown should cancel budget wait");
        drop(reclaim);
        assert_eq!(cursor, "8-0");
        assert_eq!(budget.snapshot().waiters, 0);
        assert_eq!(budget.snapshot().reserved_bytes, 4096);
        drop(occupied);
        assert_eq!(budget.snapshot().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn usage_worker_shutdown_cancels_inflight_reclaim_and_releases_budget() {
        use std::future::Future;
        use std::task::Poll;

        let runner = Arc::new(ReadReclaimRaceProbeQueue::new(RuntimeQueueEntry {
            id: "9-0".to_string(),
            fields: sample_event().to_stream_fields().expect("event fields"),
        }));
        runner.block_reclaim.store(true, Ordering::Release);
        let budget = Arc::new(QueueReadBudget::new(4096, 4096));
        let control = UsageWorkerControl::default();
        let (telemetry, _observations) = tokio::sync::mpsc::channel(16);
        let mut worker = UsageQueueWorker::new(
            runner.clone(),
            Arc::new(SelectiveFailingRecorder::default()),
            receive_budget_worker_config(),
            None,
        )
        .expect("worker")
        .with_supervisor(control.clone(), telemetry);
        worker.queue = worker.queue.with_read_budget(Arc::clone(&budget));
        let mut cursor = "9-0".to_string();
        let mut reclaim = Box::pin(worker.reclaim_stale_entries(&mut cursor));
        std::future::poll_fn(|cx| {
            assert!(reclaim.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(budget.snapshot().reserved_bytes, 4096);
        assert_eq!(
            runner
                .reclaim_cursors
                .lock()
                .expect("cursors lock")
                .as_slice(),
            ["9-0"]
        );
        control.request_shutdown();
        tokio::time::timeout(Duration::from_secs(1), &mut reclaim)
            .await
            .expect("shutdown should cancel reclaim I/O");
        drop(reclaim);
        assert_eq!(cursor, "9-0");
        assert!(runner.reclaim_cancelled.load(Ordering::Acquire));
        assert_eq!(budget.snapshot().reserved_bytes, 0);
        assert!(!runner.acked.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn usage_worker_shutdown_finishes_acquired_reclaim_before_releasing_budget() {
        let runner = Arc::new(ReadReclaimRaceProbeQueue::new(RuntimeQueueEntry {
            id: "10-0".to_string(),
            fields: sample_event().to_stream_fields().expect("event fields"),
        }));
        let budget = Arc::new(QueueReadBudget::new(4096, 4096));
        let recorder = Arc::new(ControlledRecorder::default());
        let control = UsageWorkerControl::default();
        let (telemetry, _observations) = tokio::sync::mpsc::channel(16);
        let mut worker = UsageQueueWorker::new(
            runner.clone(),
            recorder.clone(),
            receive_budget_worker_config(),
            None,
        )
        .expect("worker")
        .with_supervisor(control.clone(), telemetry);
        worker.queue = worker.queue.with_read_budget(Arc::clone(&budget));
        let handle = tokio::spawn(async move {
            let mut cursor = "10-0".to_string();
            worker.reclaim_stale_entries(&mut cursor).await;
            cursor
        });
        tokio::time::timeout(Duration::from_secs(1), recorder.entered.notified())
            .await
            .expect("reclaimed entry should reach recorder");
        control.request_shutdown();
        tokio::task::yield_now().await;
        assert!(!handle.is_finished());
        assert!(budget.snapshot().reserved_bytes > 0);
        assert!(!runner.acked.load(Ordering::Acquire));
        recorder.release.notify_one();
        let cursor = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("acquired page should finish processing")
            .expect("reclaim task");
        assert_eq!(cursor, "0-0");
        assert!(runner.acked.load(Ordering::Acquire));
        assert_eq!(budget.snapshot().reserved_bytes, 0);
    }

    #[tokio::test]
    async fn usage_worker_read_budget_processes_oversized_historical_payload() {
        let runner = Arc::new(RuntimeState::memory(MemoryRuntimeStateConfig::default()));
        let store = Arc::new(TestUsageStore::default());
        let budget = Arc::new(QueueReadBudget::new(4096, 4096));
        let control = UsageWorkerControl::default();
        let (telemetry, mut observations) = tokio::sync::mpsc::channel(16);
        let mut worker = build_usage_queue_worker_with_record_gate(
            runner.clone(),
            store.clone(),
            receive_budget_worker_config(),
            None,
            None,
        )
        .expect("worker")
        .with_supervisor(control.clone(), telemetry);
        worker.queue = worker.queue.with_read_budget(Arc::clone(&budget));
        worker.capture_memory_budget = Arc::new(EventCaptureMemoryBudget::new(128 * 1024));
        let queue = worker.queue.clone();
        let mut event = sample_event();
        event.data.response_body = Some(serde_json::json!({"legacy": "x".repeat(8192)}));
        let fields = event.to_stream_fields().expect("historical envelope");
        assert!(fields.values().map(String::len).sum::<usize>() > 4096);
        runner
            .append_fields_with_maxlen(&worker.config.stream_key, &fields, None)
            .await
            .expect("append pre-limit message");
        let handle = tokio::spawn(worker.run());
        tokio::time::timeout(Duration::from_secs(1), async {
            while observations
                .recv()
                .await
                .expect("worker observation")
                .acked_entries
                == 0
            {}
        })
        .await
        .expect("oversized message should be recorded and ACKed");
        control.request_shutdown();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("worker should stop")
            .expect("worker task");
        {
            let records = store.records.lock().expect("records lock");
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].response_body, event.data.response_body);
            assert_eq!(records[0].total_tokens, Some(10));
        }
        let snapshot = budget.snapshot();
        assert_eq!(snapshot.reserved_bytes, 0);
        assert_eq!(snapshot.oversized_entries_total, 1);
        assert_eq!(snapshot.oversized_batches_total, 1);
        let stats = queue.stats().await.expect("acked stats");
        assert_eq!((stats.stream_length, stats.group_pending), (0, 0));
        assert_eq!(queue.dlq_stats().await.expect("dlq stats").stream_length, 0);
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
    async fn capture_budget_retry_releases_decoded_lease_and_preserves_pending_payload() {
        let (_runner, worker, recorder) =
            capture_budget_worker(64 * 1024, CaptureRecordOutcome::RetryOnce).await;
        worker
            .queue
            .enqueue(&captured_worker_event())
            .await
            .expect("enqueue");
        let entries = worker
            .queue
            .read_group(&worker.consumer)
            .await
            .expect("read event");
        assert_eq!(entries.len(), 1);
        let retry_entries = entries.clone();
        assert!(matches!(
            worker.process_entries(entries).await,
            Err(DataLayerError::TimedOut(_))
        ));
        assert_eq!(recorder.budget.retained_bytes(), 0);
        let stats = worker.queue.stats().await.expect("pending stats");
        assert_eq!(stats.stream_length, 1);
        assert_eq!(stats.group_pending, 1);
        assert_eq!(
            worker
                .queue
                .dlq_stats()
                .await
                .expect("dlq stats")
                .stream_length,
            0
        );

        // Replay the same pending entry, as reclamation does, without a timing-dependent idle wait.
        worker
            .process_entries(retry_entries)
            .await
            .expect("retry should succeed");
        assert_eq!(recorder.calls.load(Ordering::Acquire), 2);
        assert_eq!(recorder.budget.retained_bytes(), 0);
        let stats = worker.queue.stats().await.expect("ack stats");
        assert_eq!(stats.stream_length, 0);
        assert_eq!(stats.group_pending, 0);
    }

    #[tokio::test]
    async fn capture_budget_downgrade_dead_letter_keeps_exact_original_fields() {
        let (runner, worker, recorder) =
            capture_budget_worker(0, CaptureRecordOutcome::PermanentFailure).await;
        let mut original = captured_worker_event()
            .to_stream_fields()
            .expect("wire serialization");
        original.insert(
            "legacy_marker".to_string(),
            "preserve this field".to_string(),
        );
        runner
            .append_fields_with_maxlen(&worker.config.stream_key, &original, None)
            .await
            .expect("enqueue raw fields");
        let entries = worker
            .queue
            .read_group(&worker.consumer)
            .await
            .expect("read event");
        worker
            .process_entries(entries)
            .await
            .expect("permanent failure should dead letter");
        assert_eq!(recorder.calls.load(Ordering::Acquire), 1);
        assert_eq!(recorder.budget.retained_bytes(), 0);
        assert_eq!(recorder.budget.downgraded_total(), 1);
        let stats = worker.queue.stats().await.expect("ack stats");
        assert_eq!(stats.stream_length, 0);
        assert_eq!(stats.group_pending, 0);
        runner
            .ensure_consumer_group(
                &worker.config.dlq_stream_key,
                "capture-dlq-inspection",
                "0-0",
            )
            .await
            .expect("dlq group");
        let dlq = runner
            .read_group(
                &worker.config.dlq_stream_key,
                "capture-dlq-inspection",
                "capture-inspector",
                1,
                Some(1),
            )
            .await
            .expect("read dlq");
        assert_eq!(dlq.len(), 1);
        let payload: serde_json::Value =
            serde_json::from_str(&dlq[0].fields["payload"]).expect("dlq json");
        assert_eq!(
            payload["fields"],
            serde_json::to_value(original).expect("original fields json")
        );
        assert_eq!(
            payload["error"].as_str(),
            Some("unexpected database value: permanent capture test error")
        );
    }

    #[tokio::test]
    async fn capture_budget_malformed_entry_dead_letters_without_recording() {
        let (runner, worker, recorder) =
            capture_budget_worker(1024, CaptureRecordOutcome::PermanentFailure).await;
        let original = BTreeMap::from([(
            "payload".to_string(),
            "malformed legacy payload".to_string(),
        )]);
        runner
            .append_fields_with_maxlen(&worker.config.stream_key, &original, None)
            .await
            .expect("enqueue raw fields");
        let entries = worker
            .queue
            .read_group(&worker.consumer)
            .await
            .expect("read event");
        worker
            .process_entries(entries)
            .await
            .expect("malformed event should dead letter");
        assert_eq!(recorder.calls.load(Ordering::Acquire), 0);
        assert_eq!(recorder.budget.retained_bytes(), 0);
        assert_eq!(recorder.budget.downgraded_total(), 0);
        let stats = worker.queue.stats().await.expect("ack stats");
        assert_eq!(stats.stream_length, 0);
        assert_eq!(stats.group_pending, 0);
        runner
            .ensure_consumer_group(
                &worker.config.dlq_stream_key,
                "capture-dlq-inspection",
                "0-0",
            )
            .await
            .expect("dlq group");
        let dlq = runner
            .read_group(
                &worker.config.dlq_stream_key,
                "capture-dlq-inspection",
                "capture-inspector",
                1,
                Some(1),
            )
            .await
            .expect("read dlq");
        assert_eq!(dlq.len(), 1);
        let payload: serde_json::Value =
            serde_json::from_str(&dlq[0].fields["payload"]).expect("dlq json");
        assert_eq!(
            payload["fields"],
            serde_json::to_value(original).expect("original fields json")
        );
    }

    #[tokio::test]
    async fn capture_budget_cancelled_record_releases_lease_without_acknowledging() {
        let (_runner, worker, recorder) =
            capture_budget_worker(64 * 1024, CaptureRecordOutcome::Wait).await;
        worker
            .queue
            .enqueue(&captured_worker_event())
            .await
            .expect("enqueue");
        let entries = worker
            .queue
            .read_group(&worker.consumer)
            .await
            .expect("read event");
        let worker = Arc::new(worker);
        let task_worker = Arc::clone(&worker);
        let task = tokio::spawn(async move { task_worker.process_entries(entries).await });
        tokio::time::timeout(Duration::from_secs(1), recorder.entered.notified())
            .await
            .expect("recorder should start");
        assert!(recorder.budget.retained_bytes() > 0);
        task.abort();
        assert!(task
            .await
            .expect_err("task should be cancelled")
            .is_cancelled());
        assert_eq!(recorder.budget.retained_bytes(), 0);
        let stats = worker.queue.stats().await.expect("pending stats");
        assert_eq!(stats.stream_length, 1);
        assert_eq!(stats.group_pending, 1);
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
        let mut worker = UsageQueueWorker::new(queue_runner, recorder.clone(), config, None)
            .expect("worker should build");
        worker.queue =
            worker
                .queue
                .with_dead_letter_encoding_budget(Arc::new(DeadLetterEncodingBudget::new(
                    64 * 1024 * 1024,
                    4,
                )));
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
