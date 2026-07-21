use aether_data_contracts::DataLayerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRuntimeConfig {
    pub enabled: bool,
    pub queue_terminal_events: bool,
    pub queue_lifecycle_events: bool,
    pub worker_count: usize,
    pub worker_autoscale_enabled: bool,
    pub worker_max_count: usize,
    pub worker_record_concurrency_limit: Option<usize>,
    pub worker_scale_interval_ms: u64,
    pub worker_idle_scale_down_ticks: u64,
    pub stream_key: String,
    pub consumer_group: String,
    pub dlq_stream_key: String,
    pub stream_maxlen: usize,
    pub consumer_batch_size: usize,
    pub consumer_block_ms: u64,
    pub reclaim_idle_ms: u64,
    pub reclaim_count: usize,
    pub reclaim_interval_ms: u64,
    pub terminal_submission_max_in_flight: u64,
    pub terminal_enqueue_max_in_flight: u64,
    pub lifecycle_enqueue_max_in_flight: u64,
    pub lifecycle_enqueue_delay_ms: u64,
    pub retry_deferred_lifecycle_events: bool,
    pub enqueue_retry_buffer_capacity: usize,
    pub enqueue_retry_workers: usize,
    pub enqueue_retry_initial_backoff_ms: u64,
    pub enqueue_retry_max_backoff_ms: u64,
}

impl Default for UsageRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            queue_terminal_events: false,
            queue_lifecycle_events: false,
            worker_count: 4,
            worker_autoscale_enabled: true,
            worker_max_count: 32,
            worker_record_concurrency_limit: Some(32),
            worker_scale_interval_ms: 1_000,
            worker_idle_scale_down_ticks: 30,
            stream_key: "usage:events".to_string(),
            consumer_group: "usage_consumers".to_string(),
            dlq_stream_key: "usage:events:dlq".to_string(),
            stream_maxlen: 200_000,
            consumer_batch_size: 128,
            consumer_block_ms: 500,
            reclaim_idle_ms: 60_000,
            reclaim_count: 128,
            reclaim_interval_ms: 5_000,
            terminal_submission_max_in_flight: 1_024,
            terminal_enqueue_max_in_flight: 1_024,
            lifecycle_enqueue_max_in_flight: 512,
            lifecycle_enqueue_delay_ms: 1_000,
            retry_deferred_lifecycle_events: true,
            enqueue_retry_buffer_capacity: 131_072,
            enqueue_retry_workers: 8,
            enqueue_retry_initial_backoff_ms: 3_000,
            enqueue_retry_max_backoff_ms: 10_000,
        }
    }
}

impl UsageRuntimeConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn validate(&self) -> Result<(), DataLayerError> {
        if !self.enabled {
            return Ok(());
        }

        if self.stream_key.trim().is_empty() {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime stream_key cannot be empty".to_string(),
            ));
        }
        if self.consumer_group.trim().is_empty() {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime consumer_group cannot be empty".to_string(),
            ));
        }
        if self.dlq_stream_key.trim().is_empty() {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime dlq_stream_key cannot be empty".to_string(),
            ));
        }
        if self.worker_count == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime worker_count must be positive".to_string(),
            ));
        }
        if self.worker_max_count == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime worker_max_count must be positive".to_string(),
            ));
        }
        if self
            .worker_record_concurrency_limit
            .is_some_and(|limit| limit == 0)
        {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime worker_record_concurrency_limit must be positive when set"
                    .to_string(),
            ));
        }
        if self.worker_scale_interval_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime worker_scale_interval_ms must be positive".to_string(),
            ));
        }
        if self.worker_idle_scale_down_ticks == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime worker_idle_scale_down_ticks must be positive".to_string(),
            ));
        }
        if self.stream_maxlen == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime stream_maxlen must be positive".to_string(),
            ));
        }
        if self.consumer_batch_size == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime consumer_batch_size must be positive".to_string(),
            ));
        }
        if self.consumer_block_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime consumer_block_ms must be positive".to_string(),
            ));
        }
        if self.reclaim_idle_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime reclaim_idle_ms must be positive".to_string(),
            ));
        }
        if self.reclaim_count == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime reclaim_count must be positive".to_string(),
            ));
        }
        if self.reclaim_interval_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime reclaim_interval_ms must be positive".to_string(),
            ));
        }
        if self.terminal_submission_max_in_flight == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime terminal_submission_max_in_flight must be positive".to_string(),
            ));
        }
        if self.terminal_enqueue_max_in_flight == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime terminal_enqueue_max_in_flight must be positive".to_string(),
            ));
        }
        if self.lifecycle_enqueue_max_in_flight == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime lifecycle_enqueue_max_in_flight must be positive".to_string(),
            ));
        }
        if self.enqueue_retry_buffer_capacity == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime enqueue_retry_buffer_capacity must be positive".to_string(),
            ));
        }
        if self.enqueue_retry_workers == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime enqueue_retry_workers must be positive".to_string(),
            ));
        }
        if self.enqueue_retry_initial_backoff_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime enqueue_retry_initial_backoff_ms must be positive".to_string(),
            ));
        }
        if self.enqueue_retry_max_backoff_ms == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime enqueue_retry_max_backoff_ms must be positive".to_string(),
            ));
        }
        if self.enqueue_retry_initial_backoff_ms > self.enqueue_retry_max_backoff_ms {
            return Err(DataLayerError::InvalidConfiguration(
                "usage runtime enqueue retry initial backoff cannot exceed max backoff".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::UsageRuntimeConfig;

    #[test]
    fn disabled_config_is_valid() {
        assert!(UsageRuntimeConfig::disabled().validate().is_ok());
    }

    #[test]
    fn enabled_config_rejects_empty_stream_key() {
        let config = UsageRuntimeConfig {
            enabled: true,
            stream_key: String::new(),
            ..UsageRuntimeConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn enabled_config_rejects_zero_terminal_submission_limit() {
        let config = UsageRuntimeConfig {
            enabled: true,
            terminal_submission_max_in_flight: 0,
            ..UsageRuntimeConfig::default()
        };
        assert!(config.validate().is_err());
    }
}
