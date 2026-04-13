use aether_data::redis::{RedisKvRunner, RedisKvRunnerConfig, RedisLockRunner};
use aether_data::{DataBackends, DataLayerError};

use super::{GatewayDataConfig, GatewayDataState, StoredSystemConfigEntry};

fn current_system_config_updated_at_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl GatewayDataState {
    pub(crate) fn disabled() -> Self {
        Self::default()
    }

    pub(crate) fn from_config(config: GatewayDataConfig) -> Result<Self, DataLayerError> {
        if !config.is_enabled() {
            return Ok(Self {
                config,
                backends: None,
                auth_api_key_reader: None,
                auth_api_key_writer: None,
                auth_module_reader: None,
                auth_module_writer: None,
                announcement_reader: None,
                announcement_writer: None,
                management_token_reader: None,
                management_token_writer: None,
                oauth_provider_reader: None,
                oauth_provider_writer: None,
                proxy_node_reader: None,
                proxy_node_writer: None,
                billing_reader: None,
                gemini_file_mapping_reader: None,
                gemini_file_mapping_writer: None,
                global_model_reader: None,
                global_model_writer: None,
                minimal_candidate_selection_reader: None,
                request_candidate_reader: None,
                request_candidate_writer: None,
                provider_catalog_reader: None,
                provider_catalog_writer: None,
                provider_quota_reader: None,
                provider_quota_writer: None,
                usage_reader: None,
                usage_writer: None,
                user_reader: None,
                user_preferences: None,
                usage_worker_runner: None,
                video_task_reader: None,
                video_task_writer: None,
                wallet_reader: None,
                wallet_writer: None,
                settlement_writer: None,
                shadow_result_reader: None,
                shadow_result_writer: None,
                system_config_values: None,
            });
        }

        let backends = DataBackends::from_config(config.to_data_layer_config())?;
        let auth_api_key_reader = backends.read().auth_api_keys();
        let auth_api_key_writer = backends.write().auth_api_keys();
        let auth_module_reader = backends.read().auth_modules();
        let auth_module_writer = backends.write().auth_modules();
        let announcement_reader = backends.read().announcements();
        let announcement_writer = backends.write().announcements();
        let management_token_reader = backends.read().management_tokens();
        let management_token_writer = backends.write().management_tokens();
        let oauth_provider_reader = backends.read().oauth_providers();
        let oauth_provider_writer = backends.write().oauth_providers();
        let proxy_node_reader = backends.read().proxy_nodes();
        let proxy_node_writer = backends.write().proxy_nodes();
        let billing_reader = backends.read().billing();
        let gemini_file_mapping_reader = backends.read().gemini_file_mappings();
        let global_model_reader = backends.read().global_models();
        let global_model_writer = backends.write().global_models();
        let minimal_candidate_selection_reader = backends.read().minimal_candidate_selection();
        let request_candidate_reader = backends.read().request_candidates();
        let request_candidate_writer = backends.write().request_candidates();
        let gemini_file_mapping_writer = backends.write().gemini_file_mappings();
        let provider_catalog_reader = backends.read().provider_catalog();
        let provider_catalog_writer = backends.write().provider_catalog();
        let provider_quota_reader = backends.read().provider_quotas();
        let provider_quota_writer = backends.write().provider_quotas();
        let usage_reader = backends.read().usage();
        let usage_writer = backends.write().usage();
        let user_reader = backends.read().users();
        let usage_worker_runner = backends.workers().redis();
        let video_task_reader = backends.read().video_tasks();
        let video_task_writer = backends.write().video_tasks();
        let wallet_reader = backends.read().wallets();
        let wallet_writer = backends.write().wallets();
        let settlement_writer = backends.write().settlement();
        let shadow_result_reader = backends.read().shadow_results();
        let shadow_result_writer = backends.write().shadow_results();

        Ok(Self {
            config,
            backends: Some(backends),
            auth_api_key_reader,
            auth_api_key_writer,
            auth_module_reader,
            auth_module_writer,
            announcement_reader,
            announcement_writer,
            management_token_reader,
            management_token_writer,
            oauth_provider_reader,
            oauth_provider_writer,
            proxy_node_reader,
            proxy_node_writer,
            billing_reader,
            gemini_file_mapping_reader,
            gemini_file_mapping_writer,
            global_model_reader,
            global_model_writer,
            minimal_candidate_selection_reader,
            request_candidate_reader,
            request_candidate_writer,
            provider_catalog_reader,
            provider_catalog_writer,
            provider_quota_reader,
            provider_quota_writer,
            usage_reader,
            usage_writer,
            user_reader,
            user_preferences: None,
            usage_worker_runner,
            video_task_reader,
            video_task_writer,
            wallet_reader,
            wallet_writer,
            settlement_writer,
            shadow_result_reader,
            shadow_result_writer,
            system_config_values: None,
        })
    }

    pub(crate) fn has_backends(&self) -> bool {
        self.backends.is_some()
    }

    pub(crate) fn has_auth_api_key_reader(&self) -> bool {
        self.auth_api_key_reader.is_some()
    }

    pub(crate) fn has_auth_api_key_writer(&self) -> bool {
        self.auth_api_key_writer.is_some()
    }

    pub(crate) fn has_auth_module_writer(&self) -> bool {
        self.auth_module_writer.is_some()
    }

    pub(crate) fn has_announcement_reader(&self) -> bool {
        self.announcement_reader.is_some()
    }

    pub(crate) fn has_announcement_writer(&self) -> bool {
        self.announcement_writer.is_some()
    }

    pub(crate) fn has_management_token_reader(&self) -> bool {
        self.management_token_reader.is_some()
    }

    pub(crate) fn has_management_token_writer(&self) -> bool {
        self.management_token_writer.is_some()
    }

    pub(crate) fn has_gemini_file_mapping_reader(&self) -> bool {
        self.gemini_file_mapping_reader.is_some()
    }

    pub(crate) fn has_gemini_file_mapping_writer(&self) -> bool {
        self.gemini_file_mapping_writer.is_some()
    }

    pub(crate) fn has_global_model_reader(&self) -> bool {
        self.global_model_reader.is_some()
    }

    pub(crate) fn has_global_model_writer(&self) -> bool {
        self.global_model_writer.is_some()
    }

    pub(crate) fn has_redis_backend(&self) -> bool {
        self.backends
            .as_ref()
            .and_then(|backends| backends.redis())
            .is_some()
    }

    #[allow(dead_code)]
    pub(crate) fn has_minimal_candidate_selection_reader(&self) -> bool {
        self.minimal_candidate_selection_reader.is_some()
    }

    pub(crate) fn has_request_candidate_reader(&self) -> bool {
        self.request_candidate_reader.is_some()
    }

    pub(crate) fn has_request_candidate_writer(&self) -> bool {
        self.request_candidate_writer.is_some()
    }

    pub(crate) fn has_provider_catalog_reader(&self) -> bool {
        self.provider_catalog_reader.is_some()
    }

    pub(crate) fn has_provider_catalog_writer(&self) -> bool {
        self.provider_catalog_writer.is_some()
    }

    pub(crate) fn has_proxy_node_reader(&self) -> bool {
        self.proxy_node_reader.is_some()
    }

    pub(crate) fn has_proxy_node_writer(&self) -> bool {
        self.proxy_node_writer.is_some()
    }

    pub(crate) fn has_system_config_store(&self) -> bool {
        self.system_config_values.is_some()
            || self
                .backends
                .as_ref()
                .and_then(|backends| backends.postgres())
                .is_some()
    }

    pub(crate) fn oauth_refresh_lock_runner(&self) -> Option<RedisLockRunner> {
        self.backends
            .as_ref()
            .and_then(|backends| backends.locks().redis())
    }

    pub(crate) fn kv_runner(&self) -> Option<RedisKvRunner> {
        self.backends
            .as_ref()
            .and_then(|backends| backends.redis())
            .and_then(|backend| backend.kv_runner(RedisKvRunnerConfig::default()).ok())
    }

    pub(crate) fn postgres_pool(&self) -> Option<aether_data::postgres::PostgresPool> {
        self.backends
            .as_ref()
            .and_then(|backends| backends.postgres())
            .map(|backend| backend.pool_clone())
    }

    pub(crate) fn postgres_max_connections(&self) -> Option<u32> {
        self.config.postgres().map(|config| config.max_connections)
    }

    pub(crate) fn has_provider_quota_writer(&self) -> bool {
        self.provider_quota_writer.is_some()
    }

    pub(crate) fn has_usage_reader(&self) -> bool {
        self.usage_reader.is_some()
    }

    pub(crate) fn has_user_reader(&self) -> bool {
        self.user_reader.is_some()
    }

    pub(crate) fn has_usage_writer(&self) -> bool {
        self.usage_writer.is_some()
    }

    pub(crate) fn has_usage_worker_runner(&self) -> bool {
        self.usage_worker_runner.is_some()
    }

    pub(crate) fn has_video_task_reader(&self) -> bool {
        self.video_task_reader.is_some()
    }

    pub(crate) fn has_video_task_writer(&self) -> bool {
        self.video_task_writer.is_some()
    }

    pub(crate) fn has_wallet_reader(&self) -> bool {
        self.wallet_reader.is_some()
    }

    pub(crate) fn has_wallet_writer(&self) -> bool {
        self.wallet_writer.is_some()
    }

    pub(crate) fn has_settlement_writer(&self) -> bool {
        self.settlement_writer.is_some()
    }

    pub(crate) fn has_shadow_result_writer(&self) -> bool {
        self.shadow_result_writer.is_some()
    }

    pub(crate) fn has_shadow_result_reader(&self) -> bool {
        self.shadow_result_reader.is_some()
    }

    #[allow(dead_code)]
    pub(crate) fn encryption_key(&self) -> Option<&str> {
        self.config.encryption_key()
    }

    pub(crate) async fn find_system_config_value(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        if let Some(values) = &self.system_config_values {
            return Ok(values
                .read()
                .expect("system config values lock")
                .get(key)
                .map(|entry| entry.value.clone()));
        }
        match self
            .backends
            .as_ref()
            .and_then(|backends| backends.postgres())
        {
            Some(backend) => backend.find_system_config_value(key).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn upsert_system_config_value(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<serde_json::Value, DataLayerError> {
        Ok(self
            .upsert_system_config_entry(key, value, description)
            .await?
            .value)
    }

    pub(crate) async fn list_system_config_entries(
        &self,
    ) -> Result<Vec<StoredSystemConfigEntry>, DataLayerError> {
        if let Some(values) = &self.system_config_values {
            return Ok(values
                .read()
                .expect("system config values lock")
                .values()
                .cloned()
                .collect());
        }
        match self
            .backends
            .as_ref()
            .and_then(|backends| backends.postgres())
        {
            Some(backend) => backend.list_system_config_entries().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn upsert_system_config_entry(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<StoredSystemConfigEntry, DataLayerError> {
        if let Some(values) = &self.system_config_values {
            let mut values = values.write().expect("system config values lock");
            let description = description
                .map(ToOwned::to_owned)
                .or_else(|| values.get(key).and_then(|entry| entry.description.clone()));
            let entry = StoredSystemConfigEntry {
                key: key.to_string(),
                value: value.clone(),
                description,
                updated_at_unix_secs: Some(current_system_config_updated_at_unix_secs()),
            };
            values.insert(key.to_string(), entry.clone());
            return Ok(entry);
        }
        match self
            .backends
            .as_ref()
            .and_then(|backends| backends.postgres())
        {
            Some(backend) => {
                backend
                    .upsert_system_config_entry(key, value, description)
                    .await
            }
            None => Ok(StoredSystemConfigEntry {
                key: key.to_string(),
                value: value.clone(),
                description: description.map(ToOwned::to_owned),
                updated_at_unix_secs: Some(current_system_config_updated_at_unix_secs()),
            }),
        }
    }

    pub(crate) async fn delete_system_config_value(
        &self,
        key: &str,
    ) -> Result<bool, DataLayerError> {
        if let Some(values) = &self.system_config_values {
            return Ok(values
                .write()
                .expect("system config values lock")
                .remove(key)
                .is_some());
        }
        match self
            .backends
            .as_ref()
            .and_then(|backends| backends.postgres())
        {
            Some(backend) => backend.delete_system_config_value(key).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn read_admin_system_stats(
        &self,
    ) -> Result<super::AdminSystemStats, DataLayerError> {
        match self
            .backends
            .as_ref()
            .and_then(|backends| backends.postgres())
        {
            Some(backend) => backend.read_admin_system_stats().await,
            None => Ok(super::AdminSystemStats::default()),
        }
    }
}
