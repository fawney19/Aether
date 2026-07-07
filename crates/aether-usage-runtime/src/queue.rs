use std::sync::Arc;

use serde_json::json;

use aether_data_contracts::DataLayerError;
use aether_runtime_state::{
    RuntimeQueueEntry, RuntimeQueueReclaimConfig, RuntimeQueueStats, RuntimeQueueStore,
};

use super::config::UsageRuntimeConfig;
use super::event::UsageEvent;

#[derive(Clone)]
pub struct UsageQueue {
    runner: Arc<dyn RuntimeQueueStore>,
    config: UsageRuntimeConfig,
    stream: String,
    group: String,
    dlq_stream: String,
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
        })
    }

    pub async fn ensure_consumer_group(&self) -> Result<(), DataLayerError> {
        self.runner
            .ensure_consumer_group(&self.stream, &self.group, "0-0")
            .await
    }

    pub async fn enqueue(&self, event: &UsageEvent) -> Result<String, DataLayerError> {
        let fields = event.to_stream_fields()?;
        self.runner
            .append_fields_with_maxlen(&self.stream, &fields, Some(self.config.stream_maxlen))
            .await
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

    pub async fn claim_stale(
        &self,
        consumer: &str,
        start_id: &str,
    ) -> Result<Vec<RuntimeQueueEntry>, DataLayerError> {
        self.runner
            .claim_stale(
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

    pub async fn ack_and_delete(&self, ids: &[String]) -> Result<(), DataLayerError> {
        self.runner.ack(&self.stream, &self.group, ids).await?;
        self.runner.delete(&self.stream, ids).await?;
        Ok(())
    }

    pub async fn push_dead_letter(
        &self,
        entry: &RuntimeQueueEntry,
        error: &str,
    ) -> Result<String, DataLayerError> {
        let fields = std::collections::BTreeMap::from([(
            "payload".to_string(),
            serde_json::to_string(&json!({
                "entry_id": entry.id,
                "fields": entry.fields,
                "error": error,
            }))
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        )]);
        self.runner
            .append_fields_with_maxlen(&self.dlq_stream, &fields, None)
            .await
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
    use super::{usage_queue_runtime_settings, UsageQueue, UsageQueueRuntimeSettings};
    use crate::UsageRuntimeConfig;
    use aether_runtime_state::{MemoryRuntimeStateConfig, RuntimeState};
    use std::sync::Arc;

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
}
