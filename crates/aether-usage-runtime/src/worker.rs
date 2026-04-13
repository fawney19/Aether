use std::sync::Arc;
use std::time::Duration;

use aether_data::redis::{RedisConsumerName, RedisStreamEntry, RedisStreamRunner};
use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
use aether_data_contracts::DataLayerError;
use async_trait::async_trait;
use tracing::warn;

use crate::executor::spawn_on_usage_background_runtime;
use crate::{
    build_upsert_usage_record_from_event, settle_usage_if_needed, UsageEvent, UsageQueue,
    UsageRuntimeConfig, UsageSettlementWriter,
};

#[async_trait]
pub trait UsageEventRecorder: Send + Sync {
    async fn record_usage_event(&self, event: &UsageEvent) -> Result<(), DataLayerError>;
}

#[async_trait]
pub trait UsageRecordWriter: Send + Sync {
    async fn upsert_usage_record(
        &self,
        record: UpsertUsageRecord,
    ) -> Result<Option<StoredRequestUsageAudit>, DataLayerError>;
}

pub struct UsageDataEventRecorder<T> {
    data: Arc<T>,
}

impl<T> UsageDataEventRecorder<T> {
    pub fn new(data: Arc<T>) -> Self {
        Self { data }
    }
}

#[async_trait]
impl<T> UsageEventRecorder for UsageDataEventRecorder<T>
where
    T: UsageRecordWriter + UsageSettlementWriter + Send + Sync,
{
    async fn record_usage_event(&self, event: &UsageEvent) -> Result<(), DataLayerError> {
        write_event_record(self.data.as_ref(), event).await
    }
}

pub struct UsageQueueWorker {
    queue: UsageQueue,
    recorder: Arc<dyn UsageEventRecorder>,
    consumer: RedisConsumerName,
    config: UsageRuntimeConfig,
}

impl UsageQueueWorker {
    pub fn new(
        runner: RedisStreamRunner,
        recorder: Arc<dyn UsageEventRecorder>,
        config: UsageRuntimeConfig,
    ) -> Result<Self, DataLayerError> {
        let queue = UsageQueue::new(runner, config.clone())?;
        let consumer = RedisConsumerName(consumer_name());
        Ok(Self {
            queue,
            recorder,
            consumer,
            config,
        })
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        spawn_on_usage_background_runtime(async move { self.run_forever().await })
    }

    async fn run_forever(self) {
        if let Err(err) = self.queue.ensure_consumer_group().await {
            warn!(
                event_name = "usage_worker_consumer_group_failed",
                log_type = "ops",
                worker_consumer = %self.consumer.0,
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

        loop {
            tokio::select! {
                _ = reclaim_interval.tick() => {
                    match self.queue.claim_stale(&self.consumer, "0-0").await {
                        Ok(entries) => {
                            if let Err(err) = self.process_entries(entries).await {
                                warn!(
                                    event_name = "usage_worker_reclaim_process_failed",
                                    log_type = "ops",
                                    worker_consumer = %self.consumer.0,
                                    worker_group = %self.config.consumer_group,
                                    error = %err,
                                    "usage worker failed while reclaiming stale entries"
                                );
                            }
                        }
                        Err(err) => warn!(
                            event_name = "usage_worker_reclaim_failed",
                            log_type = "ops",
                            worker_consumer = %self.consumer.0,
                            worker_group = %self.config.consumer_group,
                            error = %err,
                            "usage worker failed to reclaim stale entries"
                        ),
                    }
                }
                result = self.queue.read_group(&self.consumer) => {
                    match result {
                        Ok(entries) => {
                            if let Err(err) = self.process_entries(entries).await {
                                warn!(
                                    event_name = "usage_worker_process_failed",
                                    log_type = "ops",
                                    worker_consumer = %self.consumer.0,
                                    worker_group = %self.config.consumer_group,
                                    error = %err,
                                    "usage worker failed to process queue entries"
                                );
                                tokio::time::sleep(Duration::from_millis(250)).await;
                            }
                        }
                        Err(err) => {
                            warn!(
                                event_name = "usage_worker_read_failed",
                                log_type = "ops",
                                worker_consumer = %self.consumer.0,
                                worker_group = %self.config.consumer_group,
                                error = %err,
                                "usage worker failed to read queue"
                            );
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                    }
                }
            }
        }
    }

    async fn process_entries(&self, entries: Vec<RedisStreamEntry>) -> Result<(), DataLayerError> {
        if entries.is_empty() {
            return Ok(());
        }

        let mut ack_ids = Vec::new();
        for entry in entries {
            match self.process_entry(&entry).await {
                Ok(should_ack) => {
                    if should_ack {
                        ack_ids.push(entry.id.clone());
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
        }

        Ok(())
    }

    async fn process_entry(&self, entry: &RedisStreamEntry) -> Result<bool, DataLayerError> {
        let event = match UsageEvent::from_stream_fields(&entry.fields) {
            Ok(event) => event,
            Err(err) => {
                self.queue.push_dead_letter(entry, &err.to_string()).await?;
                return Ok(true);
            }
        };

        self.recorder.record_usage_event(&event).await?;
        Ok(true)
    }
}

pub fn build_usage_queue_worker<T>(
    runner: RedisStreamRunner,
    data: Arc<T>,
    config: UsageRuntimeConfig,
) -> Result<UsageQueueWorker, DataLayerError>
where
    T: UsageRecordWriter + UsageSettlementWriter + Send + Sync + 'static,
{
    UsageQueueWorker::new(runner, Arc::new(UsageDataEventRecorder::new(data)), config)
}

pub async fn write_event_record<T>(data: &T, event: &UsageEvent) -> Result<(), DataLayerError>
where
    T: UsageRecordWriter + UsageSettlementWriter + Send + Sync,
{
    let record = build_upsert_usage_record_from_event(event)?;
    if let Some(stored) = data.upsert_usage_record(record).await? {
        settle_usage_if_needed(data, &stored).await?;
    }
    Ok(())
}

fn consumer_name() -> String {
    let host = std::env::var("HOSTNAME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "aether-gateway".to_string());
    format!("{host}:{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aether_data_contracts::repository::settlement::{
        StoredUsageSettlement, UsageSettlementInput,
    };
    use aether_data_contracts::repository::usage::{StoredRequestUsageAudit, UpsertUsageRecord};
    use async_trait::async_trait;

    use super::{write_event_record, UsageRecordWriter};
    use crate::{UsageEvent, UsageEventData, UsageEventType, UsageSettlementWriter};

    #[derive(Default)]
    struct TestUsageStore {
        records: Mutex<Vec<UpsertUsageRecord>>,
        settlements: Mutex<Vec<UsageSettlementInput>>,
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
}
