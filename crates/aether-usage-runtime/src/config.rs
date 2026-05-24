use aether_data_contracts::DataLayerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRuntimeConfig {
    pub enabled: bool,
    pub queue_terminal_events: bool,
    pub stream_key: String,
    pub consumer_group: String,
    pub dlq_stream_key: String,
    pub stream_maxlen: usize,
    pub consumer_batch_size: usize,
    pub consumer_block_ms: u64,
    pub reclaim_idle_ms: u64,
    pub reclaim_count: usize,
    pub reclaim_interval_ms: u64,
}

impl Default for UsageRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            queue_terminal_events: false,
            stream_key: "usage:events".to_string(),
            consumer_group: "usage_consumers".to_string(),
            dlq_stream_key: "usage:events:dlq".to_string(),
            stream_maxlen: 2_000,
            consumer_batch_size: 200,
            consumer_block_ms: 500,
            reclaim_idle_ms: 30_000,
            reclaim_count: 200,
            reclaim_interval_ms: 5_000,
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
}
