use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogProvider {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub website: Option<String>,
    pub provider_type: String,
    pub billing_type: Option<String>,
    pub monthly_quota_usd: Option<f64>,
    pub monthly_used_usd: Option<f64>,
    pub quota_reset_day: Option<u64>,
    pub quota_last_reset_at_unix_secs: Option<u64>,
    pub quota_expires_at_unix_secs: Option<u64>,
    pub provider_priority: i32,
    pub is_active: bool,
    pub keep_priority_on_conversion: bool,
    pub enable_format_conversion: bool,
    pub concurrent_limit: Option<i32>,
    pub max_retries: Option<i32>,
    pub proxy: Option<serde_json::Value>,
    pub request_timeout_secs: Option<f64>,
    pub stream_first_byte_timeout_secs: Option<f64>,
    pub config: Option<serde_json::Value>,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl StoredProviderCatalogProvider {
    pub fn new(
        id: String,
        name: String,
        website: Option<String>,
        provider_type: String,
    ) -> Result<Self, crate::DataLayerError> {
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "providers.name is empty".to_string(),
            ));
        }
        if provider_type.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "providers.provider_type is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            name,
            description: None,
            website,
            provider_type,
            billing_type: None,
            monthly_quota_usd: None,
            monthly_used_usd: None,
            quota_reset_day: None,
            quota_last_reset_at_unix_secs: None,
            quota_expires_at_unix_secs: None,
            provider_priority: 0,
            is_active: true,
            keep_priority_on_conversion: false,
            enable_format_conversion: false,
            concurrent_limit: None,
            max_retries: None,
            proxy: None,
            request_timeout_secs: None,
            stream_first_byte_timeout_secs: None,
            config: None,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_transport_fields(
        mut self,
        is_active: bool,
        keep_priority_on_conversion: bool,
        enable_format_conversion: bool,
        concurrent_limit: Option<i32>,
        max_retries: Option<i32>,
        proxy: Option<serde_json::Value>,
        request_timeout_secs: Option<f64>,
        stream_first_byte_timeout_secs: Option<f64>,
        config: Option<serde_json::Value>,
    ) -> Self {
        self.is_active = is_active;
        self.keep_priority_on_conversion = keep_priority_on_conversion;
        self.enable_format_conversion = enable_format_conversion;
        self.concurrent_limit = concurrent_limit;
        self.max_retries = max_retries;
        self.proxy = proxy;
        self.request_timeout_secs = request_timeout_secs;
        self.stream_first_byte_timeout_secs = stream_first_byte_timeout_secs;
        self.config = config;
        self
    }

    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_billing_fields(
        mut self,
        billing_type: Option<String>,
        monthly_quota_usd: Option<f64>,
        monthly_used_usd: Option<f64>,
        quota_reset_day: Option<u64>,
        quota_last_reset_at_unix_secs: Option<u64>,
        quota_expires_at_unix_secs: Option<u64>,
    ) -> Self {
        self.billing_type = billing_type;
        self.monthly_quota_usd = monthly_quota_usd;
        self.monthly_used_usd = monthly_used_usd;
        self.quota_reset_day = quota_reset_day;
        self.quota_last_reset_at_unix_secs = quota_last_reset_at_unix_secs;
        self.quota_expires_at_unix_secs = quota_expires_at_unix_secs;
        self
    }

    pub fn with_routing_fields(mut self, provider_priority: i32) -> Self {
        self.provider_priority = provider_priority;
        self
    }

    pub fn with_timestamps(
        mut self,
        created_at_unix_ms: Option<u64>,
        updated_at_unix_secs: Option<u64>,
    ) -> Self {
        self.created_at_unix_ms = created_at_unix_ms;
        self.updated_at_unix_secs = updated_at_unix_secs;
        self
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogEndpoint {
    pub id: String,
    pub provider_id: String,
    pub api_format: String,
    pub api_family: Option<String>,
    pub endpoint_kind: Option<String>,
    pub is_active: bool,
    pub health_score: f64,
    pub base_url: String,
    pub header_rules: Option<serde_json::Value>,
    pub body_rules: Option<serde_json::Value>,
    pub max_retries: Option<i32>,
    pub custom_path: Option<String>,
    pub config: Option<serde_json::Value>,
    pub format_acceptance_config: Option<serde_json::Value>,
    pub proxy: Option<serde_json::Value>,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
}

impl StoredProviderCatalogEndpoint {
    pub fn new(
        id: String,
        provider_id: String,
        api_format: String,
        api_family: Option<String>,
        endpoint_kind: Option<String>,
        is_active: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if api_format.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_endpoints.api_format is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            provider_id,
            api_format,
            api_family,
            endpoint_kind,
            is_active,
            health_score: 1.0,
            base_url: String::new(),
            header_rules: None,
            body_rules: None,
            max_retries: None,
            custom_path: None,
            config: None,
            format_acceptance_config: None,
            proxy: None,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_transport_fields(
        mut self,
        base_url: String,
        header_rules: Option<serde_json::Value>,
        body_rules: Option<serde_json::Value>,
        max_retries: Option<i32>,
        custom_path: Option<String>,
        config: Option<serde_json::Value>,
        format_acceptance_config: Option<serde_json::Value>,
        proxy: Option<serde_json::Value>,
    ) -> Result<Self, crate::DataLayerError> {
        if base_url.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_endpoints.base_url is empty".to_string(),
            ));
        }

        self.base_url = base_url;
        self.header_rules = header_rules;
        self.body_rules = body_rules;
        self.max_retries = max_retries;
        self.custom_path = custom_path;
        self.config = config;
        self.format_acceptance_config = format_acceptance_config;
        self.proxy = proxy;
        Ok(self)
    }

    pub fn with_health_score(mut self, health_score: f64) -> Self {
        self.health_score = health_score;
        self
    }

    pub fn with_timestamps(
        mut self,
        created_at_unix_ms: Option<u64>,
        updated_at_unix_secs: Option<u64>,
    ) -> Self {
        self.created_at_unix_ms = created_at_unix_ms;
        self.updated_at_unix_secs = updated_at_unix_secs;
        self
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogKey {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub auth_type: String,
    pub capabilities: Option<serde_json::Value>,
    pub is_active: bool,
    pub api_formats: Option<serde_json::Value>,
    pub auth_type_by_format: Option<serde_json::Value>,
    pub allow_auth_channel_mismatch_formats: Option<serde_json::Value>,
    pub encrypted_api_key: Option<String>,
    pub encrypted_auth_config: Option<String>,
    pub note: Option<String>,
    pub internal_priority: i32,
    pub rate_multipliers: Option<serde_json::Value>,
    pub global_priority_by_format: Option<serde_json::Value>,
    pub allowed_models: Option<serde_json::Value>,
    pub expires_at_unix_secs: Option<u64>,
    pub cache_ttl_minutes: i32,
    pub max_probe_interval_minutes: i32,
    pub proxy: Option<serde_json::Value>,
    pub fingerprint: Option<serde_json::Value>,
    pub rpm_limit: Option<u32>,
    pub concurrent_limit: Option<i32>,
    pub learned_rpm_limit: Option<u32>,
    pub concurrent_429_count: Option<u32>,
    pub rpm_429_count: Option<u32>,
    pub last_429_at_unix_secs: Option<u64>,
    pub last_429_type: Option<String>,
    pub adjustment_history: Option<serde_json::Value>,
    pub utilization_samples: Option<serde_json::Value>,
    pub last_probe_increase_at_unix_secs: Option<u64>,
    pub last_rpm_peak: Option<u32>,
    pub request_count: Option<u32>,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub success_count: Option<u32>,
    pub error_count: Option<u32>,
    pub total_response_time_ms: Option<u32>,
    pub last_used_at_unix_secs: Option<u64>,
    pub auto_fetch_models: bool,
    pub last_models_fetch_at_unix_secs: Option<u64>,
    pub last_models_fetch_error: Option<String>,
    pub locked_models: Option<serde_json::Value>,
    pub model_include_patterns: Option<serde_json::Value>,
    pub model_exclude_patterns: Option<serde_json::Value>,
    pub upstream_metadata: Option<serde_json::Value>,
    pub oauth_invalid_at_unix_secs: Option<u64>,
    pub oauth_invalid_reason: Option<String>,
    pub status_snapshot: Option<serde_json::Value>,
    pub created_at_unix_ms: Option<u64>,
    pub updated_at_unix_secs: Option<u64>,
    pub health_by_format: Option<serde_json::Value>,
    pub circuit_breaker_by_format: Option<serde_json::Value>,
}

impl StoredProviderCatalogKey {
    pub fn new(
        id: String,
        provider_id: String,
        name: String,
        auth_type: String,
        capabilities: Option<serde_json::Value>,
        is_active: bool,
    ) -> Result<Self, crate::DataLayerError> {
        if name.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_api_keys.name is empty".to_string(),
            ));
        }
        if auth_type.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_api_keys.auth_type is empty".to_string(),
            ));
        }

        Ok(Self {
            id,
            provider_id,
            name,
            auth_type,
            capabilities,
            is_active,
            api_formats: None,
            auth_type_by_format: None,
            allow_auth_channel_mismatch_formats: None,
            encrypted_api_key: None,
            encrypted_auth_config: None,
            note: None,
            internal_priority: 50,
            rate_multipliers: None,
            global_priority_by_format: None,
            allowed_models: None,
            expires_at_unix_secs: None,
            cache_ttl_minutes: 5,
            max_probe_interval_minutes: 32,
            proxy: None,
            fingerprint: None,
            rpm_limit: None,
            concurrent_limit: None,
            learned_rpm_limit: None,
            concurrent_429_count: None,
            rpm_429_count: None,
            last_429_at_unix_secs: None,
            last_429_type: None,
            adjustment_history: None,
            utilization_samples: None,
            last_probe_increase_at_unix_secs: None,
            last_rpm_peak: None,
            request_count: None,
            total_tokens: 0,
            total_cost_usd: 0.0,
            success_count: None,
            error_count: None,
            total_response_time_ms: None,
            last_used_at_unix_secs: None,
            auto_fetch_models: false,
            last_models_fetch_at_unix_secs: None,
            last_models_fetch_error: None,
            locked_models: None,
            model_include_patterns: None,
            model_exclude_patterns: None,
            upstream_metadata: None,
            oauth_invalid_at_unix_secs: None,
            oauth_invalid_reason: None,
            status_snapshot: None,
            created_at_unix_ms: None,
            updated_at_unix_secs: None,
            health_by_format: None,
            circuit_breaker_by_format: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_transport_fields(
        mut self,
        api_formats: Option<serde_json::Value>,
        encrypted_api_key: impl Into<Option<String>>,
        encrypted_auth_config: Option<String>,
        rate_multipliers: Option<serde_json::Value>,
        global_priority_by_format: Option<serde_json::Value>,
        allowed_models: Option<serde_json::Value>,
        expires_at_unix_secs: Option<u64>,
        proxy: Option<serde_json::Value>,
        fingerprint: Option<serde_json::Value>,
    ) -> Result<Self, crate::DataLayerError> {
        let encrypted_api_key = encrypted_api_key.into();
        if encrypted_api_key
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider_api_keys.api_key is empty".to_string(),
            ));
        }

        self.api_formats = api_formats;
        self.encrypted_api_key = encrypted_api_key;
        self.encrypted_auth_config = encrypted_auth_config;
        self.rate_multipliers = rate_multipliers;
        self.global_priority_by_format = global_priority_by_format;
        self.allowed_models = allowed_models;
        self.expires_at_unix_secs = expires_at_unix_secs;
        self.proxy = proxy;
        self.fingerprint = fingerprint;
        Ok(self)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_rate_limit_fields(
        mut self,
        rpm_limit: Option<u32>,
        concurrent_limit: Option<i32>,
        learned_rpm_limit: Option<u32>,
        concurrent_429_count: Option<u32>,
        rpm_429_count: Option<u32>,
        last_429_at_unix_secs: Option<u64>,
        adjustment_history: Option<serde_json::Value>,
        request_count: Option<u32>,
        success_count: Option<u32>,
    ) -> Self {
        self.rpm_limit = rpm_limit;
        self.concurrent_limit = concurrent_limit;
        self.learned_rpm_limit = learned_rpm_limit;
        self.concurrent_429_count = concurrent_429_count;
        self.rpm_429_count = rpm_429_count;
        self.last_429_at_unix_secs = last_429_at_unix_secs;
        self.adjustment_history = adjustment_history;
        self.request_count = request_count;
        self.success_count = success_count;
        self
    }

    pub fn with_usage_fields(
        mut self,
        error_count: Option<u32>,
        total_response_time_ms: Option<u32>,
    ) -> Self {
        self.error_count = error_count;
        self.total_response_time_ms = total_response_time_ms;
        self
    }

    pub fn with_usage_totals(mut self, total_tokens: u64, total_cost_usd: f64) -> Self {
        self.total_tokens = total_tokens;
        self.total_cost_usd = if total_cost_usd.is_finite() {
            total_cost_usd
        } else {
            0.0
        };
        self
    }

    pub fn with_health_fields(
        mut self,
        health_by_format: Option<serde_json::Value>,
        circuit_breaker_by_format: Option<serde_json::Value>,
    ) -> Self {
        self.health_by_format = health_by_format;
        self.circuit_breaker_by_format = circuit_breaker_by_format;
        self
    }
}

#[cfg(test)]
mod transport_tests {
    use super::StoredProviderCatalogKey;

    #[test]
    fn provider_catalog_key_defaults_concurrent_limit_to_none() {
        let key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "default".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build");

        assert_eq!(key.concurrent_limit, None);
    }

    #[test]
    fn provider_catalog_key_rate_limit_builder_sets_concurrent_limit() {
        let key = StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "default".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_rate_limit_fields(
            Some(120),
            Some(3),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert_eq!(key.rpm_limit, Some(120));
        assert_eq!(key.concurrent_limit, Some(3));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProviderCatalogKeyListOrder {
    #[default]
    Name,
    CreatedAt,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProviderCatalogKeyListQuery {
    pub provider_id: String,
    pub search: Option<String>,
    pub is_active: Option<bool>,
    pub offset: usize,
    pub limit: usize,
    pub order: ProviderCatalogKeyListOrder,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogKeyPage {
    pub items: Vec<StoredProviderCatalogKey>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredProviderCatalogKeyStats {
    pub provider_id: String,
    pub total_keys: u64,
    pub active_keys: u64,
}

impl StoredProviderCatalogKeyStats {
    pub fn new(
        provider_id: String,
        total_keys: i64,
        active_keys: i64,
    ) -> Result<Self, crate::DataLayerError> {
        if provider_id.trim().is_empty() {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider key stats provider_id is empty".to_string(),
            ));
        }
        if total_keys < 0 || active_keys < 0 {
            return Err(crate::DataLayerError::UnexpectedValue(
                "provider key stats count is negative".to_string(),
            ));
        }

        Ok(Self {
            provider_id,
            total_keys: total_keys as u64,
            active_keys: active_keys as u64,
        })
    }
}

#[async_trait]
pub trait ProviderCatalogReadRepository: Send + Sync {
    async fn list_providers(
        &self,
        active_only: bool,
    ) -> Result<Vec<StoredProviderCatalogProvider>, crate::DataLayerError>;

    async fn list_providers_by_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogProvider>, crate::DataLayerError>;

    async fn list_endpoints_by_ids(
        &self,
        endpoint_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, crate::DataLayerError>;

    async fn list_endpoints_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogEndpoint>, crate::DataLayerError>;

    async fn list_keys_by_ids(
        &self,
        key_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, crate::DataLayerError>;

    async fn list_keys_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, crate::DataLayerError>;

    async fn list_key_summaries_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKey>, crate::DataLayerError>;

    async fn list_keys_page(
        &self,
        query: &ProviderCatalogKeyListQuery,
    ) -> Result<StoredProviderCatalogKeyPage, crate::DataLayerError>;

    async fn list_key_stats_by_provider_ids(
        &self,
        provider_ids: &[String],
    ) -> Result<Vec<StoredProviderCatalogKeyStats>, crate::DataLayerError>;
}

#[async_trait]
pub trait ProviderCatalogWriteRepository: Send + Sync {
    async fn create_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
        shift_existing_priorities_from: Option<i32>,
    ) -> Result<StoredProviderCatalogProvider, crate::DataLayerError>;

    async fn update_provider(
        &self,
        provider: &StoredProviderCatalogProvider,
    ) -> Result<StoredProviderCatalogProvider, crate::DataLayerError>;

    async fn delete_provider(&self, provider_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn cleanup_deleted_provider_refs(
        &self,
        provider_id: &str,
        endpoint_ids: &[String],
        key_ids: &[String],
    ) -> Result<(), crate::DataLayerError>;

    async fn create_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, crate::DataLayerError>;

    async fn update_endpoint(
        &self,
        endpoint: &StoredProviderCatalogEndpoint,
    ) -> Result<StoredProviderCatalogEndpoint, crate::DataLayerError>;

    async fn delete_endpoint(&self, endpoint_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn create_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, crate::DataLayerError>;

    async fn update_key(
        &self,
        key: &StoredProviderCatalogKey,
    ) -> Result<StoredProviderCatalogKey, crate::DataLayerError>;

    async fn update_key_upstream_metadata(
        &self,
        key_id: &str,
        upstream_metadata: Option<&serde_json::Value>,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn delete_key(&self, key_id: &str) -> Result<bool, crate::DataLayerError>;

    async fn clear_key_oauth_invalid_marker(
        &self,
        key_id: &str,
    ) -> Result<bool, crate::DataLayerError>;

    async fn update_key_oauth_credentials(
        &self,
        key_id: &str,
        encrypted_api_key: &str,
        encrypted_auth_config: Option<&str>,
        expires_at_unix_secs: Option<u64>,
    ) -> Result<bool, crate::DataLayerError>;

    async fn update_key_health_state(
        &self,
        key_id: &str,
        is_active: bool,
        health_by_format: Option<&serde_json::Value>,
        circuit_breaker_by_format: Option<&serde_json::Value>,
    ) -> Result<bool, crate::DataLayerError>;
}

#[cfg(test)]
mod tests {
    use super::StoredProviderCatalogKey;

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-1".to_string(),
            "provider-1".to_string(),
            "key".to_string(),
            "service_account".to_string(),
            None,
            true,
        )
        .expect("key should build")
    }

    #[test]
    fn transport_fields_allow_null_encrypted_api_key() {
        let key = sample_key()
            .with_transport_fields(
                None,
                None::<String>,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("null api key should be accepted");

        assert_eq!(key.encrypted_api_key, None);
    }

    #[test]
    fn transport_fields_reject_empty_encrypted_api_key_string() {
        let err = sample_key()
            .with_transport_fields(
                None,
                Some("   ".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect_err("empty api key string should be rejected");

        assert!(err
            .to_string()
            .contains("provider_api_keys.api_key is empty"));
    }
}
