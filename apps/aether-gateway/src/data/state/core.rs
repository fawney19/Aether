use aether_data::{DataBackends, DataLayerError, DatabaseDriver};
use aether_data_contracts::repository::candidate_selection::MinimalCandidateSelectionReadRepository;
use aether_data_contracts::repository::candidates::RequestCandidateReadRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use aether_runtime_state::RuntimeQueueStore;
use std::sync::Arc;
use std::time::Duration;

use super::{
    GatewayDataConfig, GatewayDataState, StoredSystemConfigEntry, SystemConfigValueCacheState,
    SystemConfigValueInflightCompletion, SystemConfigValueInflightState,
};

const SYSTEM_CONFIG_VALUE_CACHE_TTL: Duration = Duration::from_secs(30);
const SYSTEM_CONFIG_VALUE_CACHE_MAX_ENTRIES: usize = 512;
const SYSTEM_CONFIG_VALUE_CACHE_MAX_INFLIGHT: usize = 512;

enum SystemConfigValueLoadRegistration<'a> {
    Leader(SystemConfigValueLoadGuard<'a>),
    Follower(Arc<SystemConfigValueInflightState>),
    Saturated,
}

struct SystemConfigValueLoadGuard<'a> {
    cache: &'a SystemConfigValueCacheState,
    key: Option<String>,
    state: Arc<SystemConfigValueInflightState>,
    admission: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl SystemConfigValueInflightState {
    async fn wait(&self) -> SystemConfigValueInflightCompletion {
        loop {
            if let Some(completion) = self.completion.get() {
                return completion.clone();
            }

            let mut notified = Box::pin(self.notify.notified());
            notified.as_mut().enable();
            if let Some(completion) = self.completion.get() {
                return completion.clone();
            }
            notified.await;
        }
    }
}

impl SystemConfigValueCacheState {
    fn get(&self, key: &str) -> Option<Option<serde_json::Value>> {
        self.entries.get_fresh(key, SYSTEM_CONFIG_VALUE_CACHE_TTL)
    }

    fn register(&self, key: &str) -> SystemConfigValueLoadRegistration<'_> {
        {
            let inflight = self
                .inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(state) = inflight.get(key) {
                return SystemConfigValueLoadRegistration::Follower(Arc::clone(state));
            }
        }

        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut inflight = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = inflight.get(key) {
            return SystemConfigValueLoadRegistration::Follower(Arc::clone(state));
        }
        if inflight.len() >= SYSTEM_CONFIG_VALUE_CACHE_MAX_INFLIGHT {
            return SystemConfigValueLoadRegistration::Saturated;
        }
        let Ok(admission) = Arc::clone(&self.admission).try_acquire_owned() else {
            return SystemConfigValueLoadRegistration::Saturated;
        };

        let state = Arc::new(SystemConfigValueInflightState {
            notify: Arc::new(tokio::sync::Notify::new()),
            completion: std::sync::OnceLock::new(),
        });
        inflight.insert(key.to_string(), Arc::clone(&state));
        SystemConfigValueLoadRegistration::Leader(SystemConfigValueLoadGuard {
            cache: self,
            key: Some(key.to_string()),
            state,
            admission: Some(admission),
        })
    }

    fn finish_loaded(
        &self,
        key: &str,
        state: &Arc<SystemConfigValueInflightState>,
        admission: tokio::sync::OwnedSemaphorePermit,
        value: Option<serde_json::Value>,
    ) {
        self.finish_current(
            key,
            state,
            admission,
            SystemConfigValueInflightCompletion::Loaded,
            Some(value),
        );
    }

    fn finish_current(
        &self,
        key: &str,
        state: &Arc<SystemConfigValueInflightState>,
        admission: tokio::sync::OwnedSemaphorePermit,
        completion: SystemConfigValueInflightCompletion,
        cache_value: Option<Option<serde_json::Value>>,
    ) {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut inflight = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(admission);
        debug_assert!(self.admission.available_permits() > 0);
        if !inflight
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, state))
        {
            return;
        }
        if let Some(value) = cache_value {
            self.entries.insert(
                key.to_string(),
                value,
                SYSTEM_CONFIG_VALUE_CACHE_TTL,
                SYSTEM_CONFIG_VALUE_CACHE_MAX_ENTRIES,
            );
        }
        let completed = state.completion.set(completion).is_ok();
        inflight.remove(key);
        drop(inflight);
        drop(_mutation);
        if completed {
            state.notify.notify_waiters();
        }
    }

    fn invalidate(&self, key: &str) {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.entries.remove(key);
        let state = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
        let completed = state.as_ref().is_some_and(|state| {
            state
                .completion
                .set(SystemConfigValueInflightCompletion::Invalidated)
                .is_ok()
        });
        drop(_mutation);
        if completed {
            state
                .expect("completed state should exist")
                .notify
                .notify_waiters();
        }
    }

    fn clear(&self) {
        let _mutation = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.entries.clear();
        let states = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|(_, state)| state)
            .collect::<Vec<_>>();
        let completed = states
            .iter()
            .filter(|state| {
                state
                    .completion
                    .set(SystemConfigValueInflightCompletion::Invalidated)
                    .is_ok()
            })
            .cloned()
            .collect::<Vec<_>>();
        drop(_mutation);
        for state in completed {
            state.notify.notify_waiters();
        }
    }
}

impl Default for SystemConfigValueCacheState {
    fn default() -> Self {
        Self {
            entries: aether_cache::ExpiringMap::default(),
            inflight: std::sync::Mutex::new(std::collections::HashMap::new()),
            mutation: std::sync::Mutex::new(()),
            admission: Arc::new(tokio::sync::Semaphore::new(
                SYSTEM_CONFIG_VALUE_CACHE_MAX_INFLIGHT,
            )),
        }
    }
}

impl SystemConfigValueLoadGuard<'_> {
    fn finish_loaded(&mut self, value: Option<serde_json::Value>) {
        if let Some(key) = self.key.take() {
            let admission = self
                .admission
                .take()
                .expect("active system config leader must own admission");
            self.cache
                .finish_loaded(&key, &self.state, admission, value);
        }
    }

    fn finish(&mut self, completion: SystemConfigValueInflightCompletion) {
        if let Some(key) = self.key.take() {
            let admission = self
                .admission
                .take()
                .expect("active system config leader must own admission");
            self.cache
                .finish_current(&key, &self.state, admission, completion, None);
        }
    }
}

impl Drop for SystemConfigValueLoadGuard<'_> {
    fn drop(&mut self) {
        self.finish(SystemConfigValueInflightCompletion::Cancelled);
    }
}

fn current_system_config_updated_at_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod system_config_value_cache_tests {
    use super::*;

    fn leader<'a>(
        cache: &'a SystemConfigValueCacheState,
        key: &str,
    ) -> SystemConfigValueLoadGuard<'a> {
        match cache.register(key) {
            SystemConfigValueLoadRegistration::Leader(guard) => guard,
            _ => panic!("first registration should lead"),
        }
    }

    #[tokio::test]
    async fn completion_before_first_poll_releases_system_config_follower() {
        let cache = SystemConfigValueCacheState::default();
        let mut leader = leader(&cache, "key-a");
        let follower = match cache.register("key-a") {
            SystemConfigValueLoadRegistration::Follower(state) => state,
            _ => panic!("second registration should follow"),
        };

        leader.finish_loaded(Some(serde_json::json!({"version": 1})));
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(100), follower.wait())
                .await
                .expect("completed follower must not miss the notification"),
            SystemConfigValueInflightCompletion::Loaded
        ));
        assert_eq!(
            cache.get("key-a"),
            Some(Some(serde_json::json!({"version": 1})))
        );
    }

    #[tokio::test]
    async fn system_config_follower_receives_leader_failure() {
        let cache = SystemConfigValueCacheState::default();
        let mut leader = leader(&cache, "key-a");
        let follower = match cache.register("key-a") {
            SystemConfigValueLoadRegistration::Follower(state) => state,
            _ => panic!("second registration should follow"),
        };

        leader.finish(SystemConfigValueInflightCompletion::Failed(
            DataLayerError::Sql("forced system config failure".to_string()),
        ));
        let completion = tokio::time::timeout(Duration::from_millis(100), follower.wait())
            .await
            .expect("failed load should release its follower");
        let SystemConfigValueInflightCompletion::Failed(error) = completion else {
            panic!("follower should observe the leader failure");
        };
        assert_eq!(error.to_string(), "sql error: forced system config failure");
    }

    #[tokio::test]
    async fn invalidation_wins_and_old_guard_preserves_replacement() {
        let cache = SystemConfigValueCacheState::default();
        let mut old_leader = leader(&cache, "key-a");
        let old_follower = match cache.register("key-a") {
            SystemConfigValueLoadRegistration::Follower(state) => state,
            _ => panic!("second registration should follow"),
        };

        cache.invalidate("key-a");
        let replacement = leader(&cache, "key-a");
        old_leader.finish_loaded(Some(serde_json::json!({"stale": true})));
        assert!(matches!(
            old_follower.wait().await,
            SystemConfigValueInflightCompletion::Invalidated
        ));
        assert_eq!(cache.get("key-a"), None);
        assert!(matches!(
            cache.register("key-a"),
            SystemConfigValueLoadRegistration::Follower(_)
        ));
        drop(replacement);
    }

    #[test]
    fn system_config_states_are_independent_and_inflight_is_bounded() {
        let first = SystemConfigValueCacheState::default();
        let second = SystemConfigValueCacheState::default();
        let first_guard = leader(&first, "shared-key");
        let second_guard = leader(&second, "shared-key");
        drop(first_guard);
        drop(second_guard);

        let mut guards = Vec::with_capacity(SYSTEM_CONFIG_VALUE_CACHE_MAX_INFLIGHT);
        for index in 0..SYSTEM_CONFIG_VALUE_CACHE_MAX_INFLIGHT {
            let key = format!("bounded-{index}");
            guards.push(leader(&first, &key));
        }
        assert!(matches!(
            first.register("over-capacity"),
            SystemConfigValueLoadRegistration::Saturated
        ));
        drop(guards);
    }

    #[test]
    fn capacity_full_cancelled_system_config_follower_can_retry() {
        let cache = SystemConfigValueCacheState::default();
        let mut active = Vec::with_capacity(SYSTEM_CONFIG_VALUE_CACHE_MAX_INFLIGHT - 1);
        for index in 0..SYSTEM_CONFIG_VALUE_CACHE_MAX_INFLIGHT - 1 {
            active.push(leader(&cache, &format!("active-{index}")));
        }

        let current = leader(&cache, "retry-key");
        let follower = match cache.register("retry-key") {
            SystemConfigValueLoadRegistration::Follower(state) => state,
            _ => panic!("same-key request should follow at full capacity"),
        };
        assert_eq!(cache.admission.available_permits(), 0);
        assert!(matches!(
            cache.register("over-capacity"),
            SystemConfigValueLoadRegistration::Saturated
        ));

        drop(current);
        assert!(matches!(
            follower.completion.get(),
            Some(SystemConfigValueInflightCompletion::Cancelled)
        ));
        let mut replacement = leader(&cache, "retry-key");
        replacement.finish(SystemConfigValueInflightCompletion::Cancelled);
        assert_eq!(cache.admission.available_permits(), 1);
        drop(active);
    }
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

    #[cfg(test)]
    pub(crate) fn without_wallet_reader_for_tests(mut self) -> Self {
        self.wallet_reader = None;
        self
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
        if let Some(value) = self.system_config_value_cache.get(key) {
            return Ok(value);
        }

        loop {
            match self.system_config_value_cache.register(key) {
                SystemConfigValueLoadRegistration::Saturated => {
                    return Err(DataLayerError::TimedOut(format!(
                        "system config cache admission saturated for key '{key}'"
                    )));
                }
                SystemConfigValueLoadRegistration::Follower(state) => match state.wait().await {
                    SystemConfigValueInflightCompletion::Failed(error) => {
                        return Err(error);
                    }
                    SystemConfigValueInflightCompletion::Loaded => {
                        if let Some(value) = self.system_config_value_cache.get(key) {
                            return Ok(value);
                        }
                    }
                    SystemConfigValueInflightCompletion::Cancelled
                    | SystemConfigValueInflightCompletion::Invalidated => {}
                },
                SystemConfigValueLoadRegistration::Leader(mut guard) => {
                    if let Some(value) = self.system_config_value_cache.get(key) {
                        guard.finish(SystemConfigValueInflightCompletion::Loaded);
                        return Ok(value);
                    }
                    match self.load_system_config_value_uncached(key).await {
                        Ok(value) => {
                            guard.finish_loaded(value.clone());
                            return Ok(value);
                        }
                        Err(error) => {
                            guard
                                .finish(SystemConfigValueInflightCompletion::Failed(error.clone()));
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    async fn load_system_config_value_uncached(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        crate::request_diagnostics::observe_db_operation(
            "system_config_value",
            self.database_pool_summary(),
            backends.find_system_config_value(key),
        )
        .await
    }

    pub(crate) async fn find_system_config_value_strong(
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
        let Some(backends) = self.backends.as_ref() else {
            return Ok(None);
        };
        crate::request_diagnostics::observe_db_operation(
            "system_config_value_strong",
            self.database_pool_summary(),
            backends.find_system_config_value(key),
        )
        .await
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
        self.system_config_value_cache.invalidate(key);
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
        let purges_config = matches!(
            &target,
            aether_data::repository::system::AdminSystemPurgeTarget::Config
        );
        if purges_config {
            if let Some(values) = &self.system_config_values {
                let mut values = values.write().expect("system config values lock");
                let deleted = values.len() as u64;
                values.clear();
                self.system_config_value_cache.clear();
                let mut summary =
                    aether_data::repository::system::AdminSystemPurgeSummary::default();
                summary.add("system_configs", deleted);
                return Ok(summary);
            }
        }
        let result = match self.backends.as_ref() {
            Some(backends) => backends.purge_admin_system_data(target).await,
            None => Ok(aether_data::repository::system::AdminSystemPurgeSummary::default()),
        };
        if purges_config && result.is_ok() {
            self.system_config_value_cache.clear();
        }
        result
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
