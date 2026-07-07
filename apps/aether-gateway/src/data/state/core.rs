use aether_data::{DataBackends, DataLayerError, DatabaseDriver};
use aether_data_contracts::repository::candidate_selection::MinimalCandidateSelectionReadRepository;
use aether_data_contracts::repository::candidates::RequestCandidateReadRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use aether_runtime_state::RuntimeQueueStore;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

use super::{GatewayDataConfig, GatewayDataState, StoredSystemConfigEntry};

const SYSTEM_CONFIG_VALUE_CACHE_TTL: Duration = Duration::from_secs(30);

fn system_config_value_load_state() -> &'static SystemConfigValueLoadState {
    static STATE: std::sync::OnceLock<SystemConfigValueLoadState> = std::sync::OnceLock::new();
    STATE.get_or_init(SystemConfigValueLoadState::default)
}

#[derive(Debug, Default)]
struct SystemConfigValueLoadState {
    inflight: std::sync::Mutex<HashSet<String>>,
    notify: Notify,
}

enum SystemConfigValueLoadRegistration<'a> {
    Leader(SystemConfigValueLoadGuard<'a>),
    Follower,
    Bypass,
}

struct SystemConfigValueLoadGuard<'a> {
    state: &'a SystemConfigValueLoadState,
    key: Option<String>,
}

impl Drop for SystemConfigValueLoadGuard<'_> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.state.finish(&key);
        }
    }
}

impl SystemConfigValueLoadState {
    fn register(&self, key: &str) -> SystemConfigValueLoadRegistration<'_> {
        match self.inflight.lock() {
            Ok(mut inflight) => {
                if inflight.contains(key) {
                    SystemConfigValueLoadRegistration::Follower
                } else {
                    inflight.insert(key.to_string());
                    SystemConfigValueLoadRegistration::Leader(SystemConfigValueLoadGuard {
                        state: self,
                        key: Some(key.to_string()),
                    })
                }
            }
            Err(_) => SystemConfigValueLoadRegistration::Bypass,
        }
    }

    fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }

    fn finish(&self, key: &str) {
        let removed = self
            .inflight
            .lock()
            .map(|mut inflight| inflight.remove(key))
            .unwrap_or(false);
        if removed {
            self.notify.notify_waiters();
        }
    }
}

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
                background_task_reader: None,
                background_task_writer: None,
                gemini_file_mapping_reader: None,
                gemini_file_mapping_writer: None,
                global_model_reader: None,
                global_model_writer: None,
                minimal_candidate_selection_reader: None,
                request_candidate_reader: None,
                request_candidate_writer: None,
                provider_catalog_reader: None,
                provider_catalog_writer: None,
                pool_score_reader: None,
                pool_score_writer: None,
                provider_quota_reader: None,
                provider_quota_writer: None,
                routing_group_reader: None,
                routing_group_writer: None,
                usage_reader: None,
                usage_writer: None,
                user_reader: None,
                user_preferences: None,
                usage_worker_queue: None,
                video_task_reader: None,
                video_task_writer: None,
                wallet_reader: None,
                wallet_writer: None,
                settlement_writer: None,
                system_config_values: None,
                system_config_value_cache: Default::default(),
                billing_model_context_cache: Default::default(),
            });
        }

        let backends = DataBackends::from_config(config.to_data_layer_config())?;
        let auth_api_key_reader = backends.read().auth_api_keys().map(|repository| {
            Arc::new(super::auth_api_key_cache::CachedAuthApiKeyReadRepository::new(repository))
                as Arc<dyn aether_data::repository::auth::AuthApiKeyReadRepository>
        });
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
        let background_task_reader = backends.read().background_tasks();
        let background_task_writer = backends.write().background_tasks();
        let gemini_file_mapping_reader = backends.read().gemini_file_mappings();
        let global_model_reader = backends.read().global_models();
        let global_model_writer = backends.write().global_models();
        let minimal_candidate_selection_reader =
            backends
                .read()
                .minimal_candidate_selection()
                .map(|repository| {
                    Arc::new(
                        super::candidate_cache::CachedMinimalCandidateSelectionReadRepository::new(
                            repository,
                        ),
                    ) as Arc<dyn MinimalCandidateSelectionReadRepository>
                });
        let request_candidate_reader = backends.read().request_candidates().map(|repository| {
            Arc::new(
                super::request_candidate_cache::CachedRequestCandidateReadRepository::new(
                    repository,
                ),
            ) as Arc<dyn RequestCandidateReadRepository>
        });
        let request_candidate_writer = backends.write().request_candidates();
        let gemini_file_mapping_writer = backends.write().gemini_file_mappings();
        let provider_catalog_reader = backends.read().provider_catalog().map(|repository| {
            Arc::new(
                super::provider_catalog_cache::CachedProviderCatalogReadRepository::new(repository),
            ) as Arc<dyn ProviderCatalogReadRepository>
        });
        let provider_catalog_writer = backends.write().provider_catalog();
        let pool_score_reader = backends.read().pool_scores();
        let pool_score_writer = backends.write().pool_scores();
        let provider_quota_reader = backends.read().provider_quotas();
        let provider_quota_writer = backends.write().provider_quotas();
        let routing_group_reader = backends.read().routing_groups().map(|repository| {
            Arc::new(super::routing_group_cache::CachedRoutingGroupReadRepository::new(
                repository,
            )) as Arc<dyn aether_data_contracts::repository::routing_profiles::RoutingGroupReadRepository>
        });
        let routing_group_writer = backends.write().routing_groups();
        let usage_reader = backends.read().usage();
        let usage_writer = backends.write().usage();
        let user_reader = backends.read().users();
        let usage_worker_queue = None;
        let video_task_reader = backends.read().video_tasks();
        let video_task_writer = backends.write().video_tasks();
        let wallet_reader = backends.read().wallets();
        let wallet_writer = backends.write().wallets();
        let settlement_writer = backends.write().settlement();

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
            background_task_reader,
            background_task_writer,
            gemini_file_mapping_reader,
            gemini_file_mapping_writer,
            global_model_reader,
            global_model_writer,
            minimal_candidate_selection_reader,
            request_candidate_reader,
            request_candidate_writer,
            provider_catalog_reader,
            provider_catalog_writer,
            pool_score_reader,
            pool_score_writer,
            provider_quota_reader,
            provider_quota_writer,
            routing_group_reader,
            routing_group_writer,
            usage_reader,
            usage_writer,
            user_reader,
            user_preferences: None,
            usage_worker_queue,
            video_task_reader,
            video_task_writer,
            wallet_reader,
            wallet_writer,
            settlement_writer,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        })
    }

    pub(crate) fn has_backends(&self) -> bool {
        self.backends.is_some()
    }

    pub(crate) fn with_usage_worker_queue(
        mut self,
        queue: Option<Arc<dyn RuntimeQueueStore>>,
    ) -> Self {
        self.usage_worker_queue = queue;
        self
    }

    pub(crate) fn has_database_maintenance_backend(&self) -> bool {
        self.backends
            .as_ref()
            .is_some_and(|backends| backends.has_database_maintenance_backend())
    }

    pub(crate) fn has_database_pool_summary(&self) -> bool {
        self.backends
            .as_ref()
            .is_some_and(|backends| backends.has_database_pool_summary())
    }

    pub(crate) fn has_wallet_daily_usage_aggregation_backend(&self) -> bool {
        self.backends
            .as_ref()
            .is_some_and(|backends| backends.has_wallet_daily_usage_aggregation_backend())
    }

    pub(crate) fn has_stats_hourly_aggregation_backend(&self) -> bool {
        self.backends
            .as_ref()
            .is_some_and(|backends| backends.has_stats_hourly_aggregation_backend())
    }

    pub(crate) fn has_stats_daily_aggregation_backend(&self) -> bool {
        self.backends
            .as_ref()
            .is_some_and(|backends| backends.has_stats_daily_aggregation_backend())
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

    pub(crate) fn has_background_task_reader(&self) -> bool {
        self.background_task_reader.is_some()
    }

    pub(crate) fn has_background_task_writer(&self) -> bool {
        self.background_task_writer.is_some()
    }

    pub(crate) fn has_audit_log_reader(&self) -> bool {
        self.backends
            .as_ref()
            .and_then(|backends| backends.read().audit_logs())
            .is_some()
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

    #[allow(dead_code)]
    pub(crate) fn has_minimal_candidate_selection_reader(&self) -> bool {
        self.minimal_candidate_selection_reader.is_some()
    }

    pub(crate) fn clear_minimal_candidate_selection_cache(&self) {
        if let Some(repository) = &self.minimal_candidate_selection_reader {
            repository.clear_local_cache();
        }
    }

    pub(crate) fn clear_routing_group_cache(&self) {
        if let Some(repository) = &self.routing_group_reader {
            repository.clear_local_cache();
        }
    }

    pub(crate) fn clear_provider_catalog_cache(&self) {
        if let Some(repository) = &self.provider_catalog_reader {
            repository.clear_local_cache();
        }
    }

    pub(crate) fn has_request_candidate_reader(&self) -> bool {
        self.request_candidate_reader.is_some()
    }

    pub(crate) fn has_request_candidate_writer(&self) -> bool {
        self.request_candidate_writer.is_some()
    }

    pub(crate) fn request_candidate_writer(
        &self,
    ) -> Option<
        Arc<dyn aether_data_contracts::repository::candidates::RequestCandidateWriteRepository>,
    > {
        self.request_candidate_writer.clone()
    }

    pub(crate) fn has_routing_group_reader(&self) -> bool {
        self.routing_group_reader.is_some()
    }

    pub(crate) fn has_routing_group_writer(&self) -> bool {
        self.routing_group_writer.is_some()
    }

    pub(crate) fn has_provider_catalog_reader(&self) -> bool {
        self.provider_catalog_reader.is_some()
    }

    pub(crate) fn has_provider_catalog_writer(&self) -> bool {
        self.provider_catalog_writer.is_some()
    }

    pub(crate) fn has_pool_score_reader(&self) -> bool {
        self.pool_score_reader.is_some()
    }

    pub(crate) fn has_pool_score_writer(&self) -> bool {
        self.pool_score_writer.is_some()
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
                .is_some_and(|backends| backends.has_system_config_backend())
    }

    pub(crate) fn database_driver(&self) -> Option<DatabaseDriver> {
        self.backends
            .as_ref()
            .and_then(|backends| backends.database_driver())
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

    pub(crate) fn has_usage_counter_flush_backend(&self) -> bool {
        self.has_usage_writer() && self.database_driver() == Some(DatabaseDriver::Postgres)
    }

    pub(crate) fn has_usage_worker_queue(&self) -> bool {
        self.usage_worker_queue.is_some()
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
        let cached_value = self
            .system_config_value_cache
            .read()
            .expect("system config value cache lock")
            .get(key)
            .cloned();
        if let Some((cached_at, value)) = cached_value {
            if cached_at.elapsed() <= SYSTEM_CONFIG_VALUE_CACHE_TTL {
                return Ok(value);
            }
        }
        let load_state = system_config_value_load_state();
        loop {
            let notified = load_state.notified();
            match load_state.register(key) {
                SystemConfigValueLoadRegistration::Bypass => {
                    let Some(backends) = self.backends.as_ref() else {
                        return Ok(None);
                    };
                    let value = crate::request_diagnostics::observe_db_operation(
                        "system_config_value",
                        self.database_pool_summary(),
                        backends.find_system_config_value(key),
                    )
                    .await?;
                    self.system_config_value_cache
                        .write()
                        .expect("system config value cache lock")
                        .insert(key.to_string(), (Instant::now(), value.clone()));
                    return Ok(value);
                }
                SystemConfigValueLoadRegistration::Follower => {
                    notified.await;
                    let cached_value = self
                        .system_config_value_cache
                        .read()
                        .expect("system config value cache lock")
                        .get(key)
                        .cloned();
                    if let Some((cached_at, value)) = cached_value {
                        if cached_at.elapsed() <= SYSTEM_CONFIG_VALUE_CACHE_TTL {
                            return Ok(value);
                        }
                    }
                }
                SystemConfigValueLoadRegistration::Leader(_guard) => {
                    let cached_value = self
                        .system_config_value_cache
                        .read()
                        .expect("system config value cache lock")
                        .get(key)
                        .cloned();
                    if let Some((cached_at, value)) = cached_value {
                        if cached_at.elapsed() <= SYSTEM_CONFIG_VALUE_CACHE_TTL {
                            return Ok(value);
                        }
                    }
                    let Some(backends) = self.backends.as_ref() else {
                        return Ok(None);
                    };
                    let value = crate::request_diagnostics::observe_db_operation(
                        "system_config_value",
                        self.database_pool_summary(),
                        backends.find_system_config_value(key),
                    )
                    .await?;
                    self.system_config_value_cache
                        .write()
                        .expect("system config value cache lock")
                        .insert(key.to_string(), (Instant::now(), value.clone()));
                    return Ok(value);
                }
            }
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
        let Some(backends) = self.backends.as_ref() else {
            return Ok(Vec::new());
        };
        backends.list_system_config_entries().await
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
            self.clear_cached_system_config_value(key);
            return Ok(entry);
        }
        if let Some(backends) = self.backends.as_ref() {
            if let Some(entry) = backends
                .upsert_system_config_entry(key, value, description)
                .await?
            {
                self.clear_cached_system_config_value(key);
                return Ok(entry);
            }
        }
        self.clear_cached_system_config_value(key);
        Ok(StoredSystemConfigEntry {
            key: key.to_string(),
            value: value.clone(),
            description: description.map(ToOwned::to_owned),
            updated_at_unix_secs: Some(current_system_config_updated_at_unix_secs()),
        })
    }

    pub(crate) async fn delete_system_config_value(
        &self,
        key: &str,
    ) -> Result<bool, DataLayerError> {
        if let Some(values) = &self.system_config_values {
            let deleted = values
                .write()
                .expect("system config values lock")
                .remove(key)
                .is_some();
            self.clear_cached_system_config_value(key);
            return Ok(deleted);
        }
        let Some(backends) = self.backends.as_ref() else {
            return Ok(false);
        };
        let deleted = backends.delete_system_config_value(key).await?;
        self.clear_cached_system_config_value(key);
        Ok(deleted)
    }

    fn clear_cached_system_config_value(&self, key: &str) {
        self.system_config_value_cache
            .write()
            .expect("system config value cache lock")
            .remove(key);
    }

    pub(crate) async fn read_admin_system_stats(
        &self,
    ) -> Result<super::AdminSystemStats, DataLayerError> {
        match self.backends.as_ref() {
            Some(backends) => backends.read_admin_system_stats().await,
            None => Ok(super::AdminSystemStats::default()),
        }
    }

    pub(crate) async fn purge_admin_system_data(
        &self,
        target: aether_data::repository::system::AdminSystemPurgeTarget,
    ) -> Result<aether_data::repository::system::AdminSystemPurgeSummary, DataLayerError> {
        if matches!(
            target,
            aether_data::repository::system::AdminSystemPurgeTarget::Config
        ) {
            if let Some(values) = &self.system_config_values {
                let mut values = values.write().expect("system config values lock");
                let deleted = values.len() as u64;
                values.clear();
                let mut summary =
                    aether_data::repository::system::AdminSystemPurgeSummary::default();
                summary.add("system_configs", deleted);
                return Ok(summary);
            }
        }
        match self.backends.as_ref() {
            Some(backends) => backends.purge_admin_system_data(target).await,
            None => Ok(aether_data::repository::system::AdminSystemPurgeSummary::default()),
        }
    }

    pub(crate) async fn export_admin_system_usage_aggregates(
        &self,
    ) -> Result<aether_data::repository::system::AdminSystemUsageAggregateSnapshot, DataLayerError>
    {
        match self.backends.as_ref() {
            Some(backends) => backends.export_admin_system_usage_aggregates().await,
            None => {
                Ok(aether_data::repository::system::AdminSystemUsageAggregateSnapshot::default())
            }
        }
    }

    pub(crate) async fn import_admin_system_usage_aggregates(
        &self,
        snapshot: &aether_data::repository::system::AdminSystemUsageAggregateSnapshot,
        user_id_map: &std::collections::BTreeMap<String, String>,
        api_key_id_map: &std::collections::BTreeMap<String, String>,
        mode: aether_data::repository::system::AdminSystemUsageAggregateImportMode,
    ) -> Result<
        aether_data::repository::system::AdminSystemUsageAggregateImportSummary,
        DataLayerError,
    > {
        match self.backends.as_ref() {
            Some(backends) => {
                backends
                    .import_admin_system_usage_aggregates(
                        snapshot,
                        user_id_map,
                        api_key_id_map,
                        mode,
                    )
                    .await
            }
            None => Ok(
                aether_data::repository::system::AdminSystemUsageAggregateImportSummary::default(),
            ),
        }
    }

    pub(crate) async fn purge_admin_request_bodies_batch(
        &self,
        batch_size: usize,
    ) -> Result<aether_data::repository::system::AdminSystemPurgeSummary, DataLayerError> {
        match self.backends.as_ref() {
            Some(backends) => backends.purge_admin_request_bodies_batch(batch_size).await,
            None => Ok(aether_data::repository::system::AdminSystemPurgeSummary::default()),
        }
    }
}
