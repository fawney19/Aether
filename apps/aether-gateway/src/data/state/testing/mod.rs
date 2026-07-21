use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use aether_data_contracts::repository::candidates::RequestCandidateRepository;
use aether_data_contracts::repository::pool_scores::PoolMemberScoreRepository;
use aether_data_contracts::repository::quota::ProviderQuotaRepository;
use aether_data_contracts::repository::usage::UsageRepository;

use super::{
    AnnouncementReadRepository, AnnouncementWriteRepository, AuthApiKeyReadRepository,
    AuthApiKeyWriteRepository, AuthModuleReadRepository, AuthModuleWriteRepository,
    BackgroundTaskReadRepository, BackgroundTaskWriteRepository, BillingReadRepository,
    GatewayDataConfig, GatewayDataState, GeminiFileMappingReadRepository,
    GeminiFileMappingWriteRepository, GlobalModelReadRepository, GlobalModelWriteRepository,
    ManagementTokenReadRepository, ManagementTokenWriteRepository,
    MinimalCandidateSelectionReadRepository, OAuthProviderReadRepository,
    OAuthProviderWriteRepository, PoolMemberScoreWriteRepository, PoolScoreReadRepository,
    ProviderCatalogReadRepository, ProviderCatalogWriteRepository, ProviderQuotaReadRepository,
    ProviderQuotaWriteRepository, ProxyNodeReadRepository, ProxyNodeWriteRepository,
    RequestCandidateReadRepository, RequestCandidateWriteRepository, RoutingGroupReadRepository,
    RoutingGroupWriteRepository, SettlementWriteRepository, StoredSystemConfigEntry,
    StoredUserPreferenceRecord, UsageReadRepository, UsageWriteRepository, UserReadRepository,
    VideoTaskReadRepository, VideoTaskWriteRepository, WalletReadRepository, WalletWriteRepository,
};

mod announcements;
mod video_tasks;

impl GatewayDataState {
    #[cfg(test)]
    pub(crate) fn with_user_preferences_for_tests(
        mut self,
        preferences: impl IntoIterator<Item = StoredUserPreferenceRecord>,
    ) -> Self {
        self.user_preferences = Some(Arc::new(RwLock::new(
            preferences
                .into_iter()
                .map(|record| (record.user_id.clone(), record))
                .collect(),
        )));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_request_candidate_reader_for_tests(
        repository: Arc<dyn RequestCandidateReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            request_candidate_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_request_candidate_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: RequestCandidateRepository + 'static,
    {
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> = repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> = repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            request_candidate_reader: Some(request_candidate_reader),
            request_candidate_writer: Some(request_candidate_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_catalog_reader_for_tests(
        repository: Arc<dyn ProviderCatalogReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            provider_catalog_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_catalog_reader(
        mut self,
        repository: Arc<dyn ProviderCatalogReadRepository>,
    ) -> Self {
        self.provider_catalog_reader = Some(repository);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_request_candidate_reader(
        mut self,
        repository: Arc<dyn RequestCandidateReadRepository>,
    ) -> Self {
        self.request_candidate_reader = Some(repository);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_settlement_writer_for_tests(
        mut self,
        repository: Arc<dyn SettlementWriteRepository>,
    ) -> Self {
        self.settlement_writer = Some(repository);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_encryption_key_for_tests(
        mut self,
        encryption_key: impl Into<String>,
    ) -> Self {
        self.config = self.config.with_encryption_key(encryption_key);
        if self.system_config_values.is_none() {
            self.system_config_values = Some(Arc::new(RwLock::new(BTreeMap::new())));
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn with_system_config_values_for_tests<I>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = (String, serde_json::Value)>,
    {
        let now_unix_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let values = entries
            .into_iter()
            .map(|(key, value)| {
                let entry = StoredSystemConfigEntry {
                    key: key.clone(),
                    value,
                    description: None,
                    updated_at_unix_secs: Some(now_unix_secs),
                };
                (key, entry)
            })
            .collect();
        self.system_config_values = Some(Arc::new(RwLock::new(values)));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_background_task_repository_for_tests<T>(mut self, repository: Arc<T>) -> Self
    where
        T: BackgroundTaskReadRepository + BackgroundTaskWriteRepository + 'static,
    {
        let background_task_reader: Arc<dyn BackgroundTaskReadRepository> = repository.clone();
        let background_task_writer: Arc<dyn BackgroundTaskWriteRepository> = repository;
        self.background_task_reader = Some(background_task_reader);
        self.background_task_writer = Some(background_task_writer);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_global_model_repository_for_tests<T>(mut self, repository: Arc<T>) -> Self
    where
        T: GlobalModelReadRepository + GlobalModelWriteRepository + 'static,
    {
        let global_model_reader: Arc<dyn GlobalModelReadRepository> = repository.clone();
        let global_model_writer: Arc<dyn GlobalModelWriteRepository> = repository;
        self.global_model_reader = Some(global_model_reader);
        self.global_model_writer = Some(global_model_writer);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_global_model_reader(
        mut self,
        repository: Arc<dyn GlobalModelReadRepository>,
    ) -> Self {
        self.global_model_reader = Some(repository);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_provider_catalog_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: ProviderCatalogReadRepository + ProviderCatalogWriteRepository + 'static,
    {
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> = repository.clone();
        let provider_catalog_writer: Arc<dyn ProviderCatalogWriteRepository> = repository;
        Self {
            config: GatewayDataConfig::disabled(),
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
            provider_catalog_reader: Some(provider_catalog_reader),
            provider_catalog_writer: Some(provider_catalog_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn attach_provider_catalog_repository_for_tests<T>(
        mut self,
        repository: Arc<T>,
    ) -> Self
    where
        T: ProviderCatalogReadRepository + ProviderCatalogWriteRepository + 'static,
    {
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> = repository.clone();
        let provider_catalog_writer: Arc<dyn ProviderCatalogWriteRepository> = repository;
        self.provider_catalog_reader = Some(provider_catalog_reader);
        self.provider_catalog_writer = Some(provider_catalog_writer);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_pool_score_repository_for_tests<T>(mut self, repository: Arc<T>) -> Self
    where
        T: PoolMemberScoreRepository + 'static,
    {
        let pool_score_reader: Arc<dyn PoolScoreReadRepository> = repository.clone();
        let pool_score_writer: Arc<dyn PoolMemberScoreWriteRepository> = repository;
        self.pool_score_reader = Some(pool_score_reader);
        self.pool_score_writer = Some(pool_score_writer);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_provider_catalog_and_request_candidate_reader_for_tests(
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        request_candidate_repository: Arc<dyn RequestCandidateReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            request_candidate_reader: Some(request_candidate_repository),
            request_candidate_writer: None,
            provider_catalog_reader: Some(provider_catalog_repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_catalog_global_model_and_quota_readers_for_tests<T>(
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        global_model_repository: Arc<dyn GlobalModelReadRepository>,
        provider_quota_repository: Arc<T>,
    ) -> Self
    where
        T: ProviderQuotaRepository + 'static,
    {
        let provider_quota_reader: Arc<dyn ProviderQuotaReadRepository> =
            provider_quota_repository.clone();
        let provider_quota_writer: Arc<dyn ProviderQuotaWriteRepository> =
            provider_quota_repository;
        Self {
            config: GatewayDataConfig::disabled(),
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
            global_model_reader: Some(global_model_repository),
            global_model_writer: None,
            minimal_candidate_selection_reader: None,
            request_candidate_reader: None,
            request_candidate_writer: None,
            provider_catalog_reader: Some(provider_catalog_repository),
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: Some(provider_quota_reader),
            provider_quota_writer: Some(provider_quota_writer),
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_catalog_global_model_and_quota_repositories_for_tests<T, U, V>(
        provider_catalog_repository: Arc<U>,
        global_model_repository: Arc<V>,
        provider_quota_repository: Arc<T>,
    ) -> Self
    where
        T: ProviderQuotaRepository + 'static,
        U: ProviderCatalogReadRepository + ProviderCatalogWriteRepository + 'static,
        V: GlobalModelReadRepository + GlobalModelWriteRepository + 'static,
    {
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> =
            provider_catalog_repository.clone();
        let provider_catalog_writer: Arc<dyn ProviderCatalogWriteRepository> =
            provider_catalog_repository;
        let global_model_reader: Arc<dyn GlobalModelReadRepository> =
            global_model_repository.clone();
        let global_model_writer: Arc<dyn GlobalModelWriteRepository> = global_model_repository;
        let provider_quota_reader: Arc<dyn ProviderQuotaReadRepository> =
            provider_quota_repository.clone();
        let provider_quota_writer: Arc<dyn ProviderQuotaWriteRepository> =
            provider_quota_repository;
        Self {
            config: GatewayDataConfig::disabled(),
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
            global_model_reader: Some(global_model_reader),
            global_model_writer: Some(global_model_writer),
            minimal_candidate_selection_reader: None,
            request_candidate_reader: None,
            request_candidate_writer: None,
            provider_catalog_reader: Some(provider_catalog_reader),
            provider_catalog_writer: Some(provider_catalog_writer),
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: Some(provider_quota_reader),
            provider_quota_writer: Some(provider_quota_writer),
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_global_model_reader_for_tests(
        repository: Arc<dyn GlobalModelReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            global_model_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_catalog_and_minimal_candidate_selection_for_tests(
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: None,
            request_candidate_writer: None,
            provider_catalog_reader: Some(provider_catalog_repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_request_candidate_and_usage_repository_for_tests<T, U>(
        request_candidate_repository: Arc<T>,
        usage_repository: Arc<U>,
    ) -> Self
    where
        T: RequestCandidateRepository + 'static,
        U: UsageRepository + 'static,
    {
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> =
            request_candidate_repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> =
            request_candidate_repository;
        let usage_reader: Arc<dyn UsageReadRepository> = usage_repository.clone();
        let usage_writer: Arc<dyn UsageWriteRepository> = usage_repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            request_candidate_reader: Some(request_candidate_reader),
            request_candidate_writer: Some(request_candidate_writer),
            provider_catalog_reader: None,
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(usage_reader),
            usage_writer: Some(usage_writer),
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_request_candidate_and_gemini_file_mapping_repository_for_tests<T, U>(
        request_candidate_repository: Arc<T>,
        gemini_file_mapping_repository: Arc<U>,
    ) -> Self
    where
        T: RequestCandidateRepository + 'static,
        U: aether_data::repository::gemini_file_mappings::GeminiFileMappingRepository + 'static,
    {
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> =
            request_candidate_repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> =
            request_candidate_repository;
        let gemini_file_mapping_reader: Arc<dyn GeminiFileMappingReadRepository> =
            gemini_file_mapping_repository.clone();
        let gemini_file_mapping_writer: Arc<dyn GeminiFileMappingWriteRepository> =
            gemini_file_mapping_repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            gemini_file_mapping_reader: Some(gemini_file_mapping_reader),
            gemini_file_mapping_writer: Some(gemini_file_mapping_writer),
            global_model_reader: None,
            global_model_writer: None,
            minimal_candidate_selection_reader: None,
            request_candidate_reader: Some(request_candidate_reader),
            request_candidate_writer: Some(request_candidate_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_usage_reader_for_tests(repository: Arc<dyn UsageReadRepository>) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(repository),
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_user_reader(mut self, repository: Arc<dyn UserReadRepository>) -> Self {
        self.user_reader = Some(repository);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_routing_group_repository_for_tests<T>(mut self, repository: Arc<T>) -> Self
    where
        T: RoutingGroupReadRepository + RoutingGroupWriteRepository + 'static,
    {
        self.routing_group_reader = Some(repository.clone());
        self.routing_group_writer = Some(repository);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_auth_api_key_reader(
        mut self,
        repository: Arc<dyn AuthApiKeyReadRepository>,
    ) -> Self {
        self.auth_api_key_reader = Some(repository);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_user_reader_for_tests(repository: Arc<dyn UserReadRepository>) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: Some(repository),
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_user_and_wallet_for_tests<T>(
        user_repository: Arc<dyn UserReadRepository>,
        wallet_repository: Arc<T>,
    ) -> Self
    where
        T: aether_data::repository::wallet::WalletRepository + 'static,
    {
        let wallet_reader: Arc<dyn WalletReadRepository> = wallet_repository.clone();
        let wallet_writer: Arc<dyn WalletWriteRepository> = wallet_repository;
        Self {
            config: GatewayDataConfig::disabled(),
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
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: Some(user_repository),
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: Some(wallet_reader),
            wallet_writer: Some(wallet_writer),
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_user_billing_and_wallet_for_tests<T>(
        user_repository: Arc<dyn UserReadRepository>,
        billing_repository: Arc<dyn BillingReadRepository>,
        wallet_repository: Arc<T>,
    ) -> Self
    where
        T: aether_data::repository::wallet::WalletRepository + 'static,
    {
        let wallet_reader: Arc<dyn WalletReadRepository> = wallet_repository.clone();
        let wallet_writer: Arc<dyn WalletWriteRepository> = wallet_repository;
        Self {
            config: GatewayDataConfig::disabled(),
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
            billing_reader: Some(billing_repository),
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
            user_reader: Some(user_repository),
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: Some(wallet_reader),
            wallet_writer: Some(wallet_writer),
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_user_wallet_and_usage_for_tests<TUsage, TWallet>(
        user_repository: Arc<dyn UserReadRepository>,
        wallet_repository: Arc<TWallet>,
        usage_repository: Arc<TUsage>,
    ) -> Self
    where
        TUsage: UsageRepository + 'static,
        TWallet: aether_data::repository::wallet::WalletRepository + 'static,
    {
        let wallet_reader: Arc<dyn WalletReadRepository> = wallet_repository.clone();
        let wallet_writer: Arc<dyn WalletWriteRepository> = wallet_repository;
        let usage_reader: Arc<dyn UsageReadRepository> = usage_repository.clone();
        let usage_writer: Arc<dyn UsageWriteRepository> = usage_repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(usage_reader),
            usage_writer: Some(usage_writer),
            user_reader: Some(user_repository),
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: Some(wallet_reader),
            wallet_writer: Some(wallet_writer),
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_announcement_user_and_wallet_for_tests<T, U>(
        announcement_repository: Arc<T>,
        user_repository: Arc<dyn UserReadRepository>,
        wallet_repository: Arc<U>,
    ) -> Self
    where
        T: AnnouncementReadRepository + AnnouncementWriteRepository + 'static,
        U: aether_data::repository::wallet::WalletRepository + 'static,
    {
        let announcement_reader: Arc<dyn AnnouncementReadRepository> =
            announcement_repository.clone();
        let announcement_writer: Arc<dyn AnnouncementWriteRepository> = announcement_repository;
        let wallet_reader: Arc<dyn WalletReadRepository> = wallet_repository.clone();
        let wallet_writer: Arc<dyn WalletWriteRepository> = wallet_repository;

        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: None,
            auth_api_key_writer: None,
            auth_module_reader: None,
            auth_module_writer: None,
            announcement_reader: Some(announcement_reader),
            announcement_writer: Some(announcement_writer),
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
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: Some(user_repository),
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: Some(wallet_reader),
            wallet_writer: Some(wallet_writer),
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_catalog_and_usage_reader_for_tests<T, U>(
        provider_catalog_repository: Arc<T>,
        usage_repository: Arc<U>,
    ) -> Self
    where
        T: ProviderCatalogReadRepository + ProviderCatalogWriteRepository + 'static,
        U: UsageReadRepository + 'static,
    {
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> =
            provider_catalog_repository.clone();
        let provider_catalog_writer: Arc<dyn ProviderCatalogWriteRepository> =
            provider_catalog_repository;
        let usage_reader: Arc<dyn UsageReadRepository> = usage_repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            provider_catalog_reader: Some(provider_catalog_reader),
            provider_catalog_writer: Some(provider_catalog_writer),
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(usage_reader),
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_api_key_reader_for_tests(
        repository: Arc<dyn AuthApiKeyReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_module_reader_for_tests(
        repository: Arc<dyn AuthModuleReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: None,
            auth_api_key_writer: None,
            auth_module_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn attach_auth_module_reader_for_tests(
        mut self,
        repository: Arc<dyn AuthModuleReadRepository>,
    ) -> Self {
        self.auth_module_reader = Some(repository);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_auth_module_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: AuthModuleReadRepository + AuthModuleWriteRepository + 'static,
    {
        let auth_module_reader: Arc<dyn AuthModuleReadRepository> = repository.clone();
        let auth_module_writer: Arc<dyn AuthModuleWriteRepository> = repository;
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: None,
            auth_api_key_writer: None,
            auth_module_reader: Some(auth_module_reader),
            auth_module_writer: Some(auth_module_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn attach_auth_module_repository_for_tests<T>(mut self, repository: Arc<T>) -> Self
    where
        T: AuthModuleReadRepository + AuthModuleWriteRepository + 'static,
    {
        let auth_module_reader: Arc<dyn AuthModuleReadRepository> = repository.clone();
        let auth_module_writer: Arc<dyn AuthModuleWriteRepository> = repository;
        self.auth_module_reader = Some(auth_module_reader);
        self.auth_module_writer = Some(auth_module_writer);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_management_token_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: aether_data::repository::management_tokens::ManagementTokenReadRepository
            + aether_data::repository::management_tokens::ManagementTokenWriteRepository
            + 'static,
    {
        let management_token_reader: Arc<dyn ManagementTokenReadRepository> = repository.clone();
        let management_token_writer: Arc<dyn ManagementTokenWriteRepository> = repository;
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: None,
            auth_api_key_writer: None,
            auth_module_reader: None,
            auth_module_writer: None,
            announcement_reader: None,
            announcement_writer: None,
            management_token_reader: Some(management_token_reader),
            management_token_writer: Some(management_token_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_management_token_reader_for_tests(
        repository: Arc<dyn ManagementTokenReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: None,
            auth_api_key_writer: None,
            auth_module_reader: None,
            auth_module_writer: None,
            announcement_reader: None,
            announcement_writer: None,
            management_token_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_oauth_provider_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: aether_data::repository::oauth_providers::OAuthProviderReadRepository
            + aether_data::repository::oauth_providers::OAuthProviderWriteRepository
            + 'static,
    {
        let oauth_provider_reader: Arc<dyn OAuthProviderReadRepository> = repository.clone();
        let oauth_provider_writer: Arc<dyn OAuthProviderWriteRepository> = repository;
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: None,
            auth_api_key_writer: None,
            auth_module_reader: None,
            auth_module_writer: None,
            announcement_reader: None,
            announcement_writer: None,
            management_token_reader: None,
            management_token_writer: None,
            oauth_provider_reader: Some(oauth_provider_reader),
            oauth_provider_writer: Some(oauth_provider_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn attach_oauth_provider_repository_for_tests<T>(
        mut self,
        repository: Arc<T>,
    ) -> Self
    where
        T: aether_data::repository::oauth_providers::OAuthProviderReadRepository
            + aether_data::repository::oauth_providers::OAuthProviderWriteRepository
            + 'static,
    {
        let oauth_provider_reader: Arc<dyn OAuthProviderReadRepository> = repository.clone();
        let oauth_provider_writer: Arc<dyn OAuthProviderWriteRepository> = repository;
        self.oauth_provider_reader = Some(oauth_provider_reader);
        self.oauth_provider_writer = Some(oauth_provider_writer);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_proxy_node_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: aether_data::repository::proxy_nodes::ProxyNodeReadRepository
            + aether_data::repository::proxy_nodes::ProxyNodeWriteRepository
            + 'static,
    {
        let proxy_node_reader: Arc<dyn ProxyNodeReadRepository> = repository.clone();
        let proxy_node_writer: Arc<dyn ProxyNodeWriteRepository> = repository;
        Self {
            config: GatewayDataConfig::disabled(),
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
            proxy_node_reader: Some(proxy_node_reader),
            proxy_node_writer: Some(proxy_node_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn attach_proxy_node_repository_for_tests<T>(mut self, repository: Arc<T>) -> Self
    where
        T: aether_data::repository::proxy_nodes::ProxyNodeReadRepository
            + aether_data::repository::proxy_nodes::ProxyNodeWriteRepository
            + 'static,
    {
        let proxy_node_reader: Arc<dyn ProxyNodeReadRepository> = repository.clone();
        let proxy_node_writer: Arc<dyn ProxyNodeWriteRepository> = repository;
        self.proxy_node_reader = Some(proxy_node_reader);
        self.proxy_node_writer = Some(proxy_node_writer);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_auth_api_key_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: aether_data::repository::auth::AuthRepository + 'static,
    {
        let auth_api_key_reader: Arc<dyn AuthApiKeyReadRepository> = repository.clone();
        let auth_api_key_writer: Arc<dyn AuthApiKeyWriteRepository> = repository;
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_reader),
            auth_api_key_writer: Some(auth_api_key_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_decision_trace_readers_for_tests(
        request_candidate_repository: Arc<dyn RequestCandidateReadRepository>,
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            request_candidate_reader: Some(request_candidate_repository),
            request_candidate_writer: None,
            provider_catalog_reader: Some(provider_catalog_repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_transport_reader_for_tests(
        repository: Arc<dyn ProviderCatalogReadRepository>,
        encryption_key: impl Into<String>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled().with_encryption_key(encryption_key),
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
            provider_catalog_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_request_audit_readers_for_tests(
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
        request_candidate_repository: Arc<dyn RequestCandidateReadRepository>,
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        usage_repository: Arc<dyn UsageReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            request_candidate_reader: Some(request_candidate_repository),
            request_candidate_writer: None,
            provider_catalog_reader: Some(provider_catalog_repository),
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(usage_repository),
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn with_minimal_candidate_selection_reader_for_tests(
        repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            minimal_candidate_selection_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_minimal_candidate_selection_and_billing_for_tests(
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        billing_repository: Arc<dyn BillingReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            billing_reader: Some(billing_repository),
            gemini_file_mapping_reader: None,
            gemini_file_mapping_writer: None,
            global_model_reader: None,
            global_model_writer: None,
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_minimal_candidate_selection_and_auth_for_tests(
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_candidate_selection_and_quota_for_tests<T>(
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        provider_quota_repository: Arc<T>,
    ) -> Self
    where
        T: ProviderQuotaRepository + 'static,
    {
        let provider_quota_reader: Arc<dyn ProviderQuotaReadRepository> =
            provider_quota_repository.clone();
        let provider_quota_writer: Arc<dyn ProviderQuotaWriteRepository> =
            provider_quota_repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: None,
            request_candidate_writer: None,
            provider_catalog_reader: None,
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: Some(provider_quota_reader),
            provider_quota_writer: Some(provider_quota_writer),
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_candidate_selection_quota_and_request_candidates_for_tests<T>(
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        provider_quota_repository: Arc<T>,
        request_candidate_repository: Arc<dyn RequestCandidateReadRepository>,
    ) -> Self
    where
        T: ProviderQuotaRepository + 'static,
    {
        let provider_quota_reader: Arc<dyn ProviderQuotaReadRepository> =
            provider_quota_repository.clone();
        let provider_quota_writer: Arc<dyn ProviderQuotaWriteRepository> =
            provider_quota_repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: Some(request_candidate_repository),
            request_candidate_writer: None,
            provider_catalog_reader: None,
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: Some(provider_quota_reader),
            provider_quota_writer: Some(provider_quota_writer),
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_candidate_selection_provider_catalog_quota_and_request_candidates_for_tests<
        T,
    >(
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        provider_quota_repository: Arc<T>,
        request_candidate_repository: Arc<dyn RequestCandidateReadRepository>,
    ) -> Self
    where
        T: ProviderQuotaRepository + 'static,
    {
        let provider_quota_reader: Arc<dyn ProviderQuotaReadRepository> =
            provider_quota_repository.clone();
        let provider_quota_writer: Arc<dyn ProviderQuotaWriteRepository> =
            provider_quota_repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: Some(request_candidate_repository),
            request_candidate_writer: None,
            provider_catalog_reader: Some(provider_catalog_repository),
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: Some(provider_quota_reader),
            provider_quota_writer: Some(provider_quota_writer),
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests<
        T,
        U,
    >(
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        provider_catalog_repository: Arc<U>,
        request_candidate_repository: Arc<T>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: RequestCandidateRepository + 'static,
        U: ProviderCatalogReadRepository + ProviderCatalogWriteRepository + 'static,
    {
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> =
            request_candidate_repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> =
            request_candidate_repository;
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> =
            provider_catalog_repository.clone();
        let provider_catalog_writer: Arc<dyn ProviderCatalogWriteRepository> =
            provider_catalog_repository;
        Self {
            config: GatewayDataConfig::disabled().with_encryption_key(encryption_key),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: Some(request_candidate_reader),
            request_candidate_writer: Some(request_candidate_writer),
            provider_catalog_reader: Some(provider_catalog_reader),
            provider_catalog_writer: Some(provider_catalog_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_candidate_selection_provider_catalog_request_candidates_for_tests<
        T,
        U,
    >(
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        provider_catalog_repository: Arc<U>,
        request_candidate_repository: Arc<T>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: RequestCandidateRepository + 'static,
        U: ProviderCatalogReadRepository + ProviderCatalogWriteRepository + 'static,
    {
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> =
            request_candidate_repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> =
            request_candidate_repository;
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> =
            provider_catalog_repository.clone();
        let provider_catalog_writer: Arc<dyn ProviderCatalogWriteRepository> =
            provider_catalog_repository;

        Self {
            config: GatewayDataConfig::disabled().with_encryption_key(encryption_key),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: Some(request_candidate_reader),
            request_candidate_writer: Some(request_candidate_writer),
            provider_catalog_reader: Some(provider_catalog_reader),
            provider_catalog_writer: Some(provider_catalog_writer),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests<
        T,
        U,
        V,
    >(
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        provider_catalog_repository: Arc<U>,
        request_candidate_repository: Arc<T>,
        usage_repository: Arc<V>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: RequestCandidateRepository + 'static,
        U: ProviderCatalogReadRepository + ProviderCatalogWriteRepository + 'static,
        V: UsageRepository + 'static,
    {
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> =
            request_candidate_repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> =
            request_candidate_repository;
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> =
            provider_catalog_repository.clone();
        let provider_catalog_writer: Arc<dyn ProviderCatalogWriteRepository> =
            provider_catalog_repository;
        let usage_reader: Arc<dyn UsageReadRepository> = usage_repository.clone();
        let usage_writer: Arc<dyn UsageWriteRepository> = usage_repository;
        Self {
            config: GatewayDataConfig::disabled().with_encryption_key(encryption_key),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: Some(request_candidate_reader),
            request_candidate_writer: Some(request_candidate_writer),
            provider_catalog_reader: Some(provider_catalog_reader),
            provider_catalog_writer: Some(provider_catalog_writer),
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(usage_reader),
            usage_writer: Some(usage_writer),
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_candidate_selection_provider_catalog_request_candidates_usage_billing_and_wallet_for_tests<
        T,
        U,
        V,
        W,
    >(
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        provider_catalog_repository: Arc<U>,
        request_candidate_repository: Arc<T>,
        usage_repository: Arc<V>,
        billing_repository: Arc<dyn BillingReadRepository>,
        wallet_repository: Arc<W>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: RequestCandidateRepository + 'static,
        U: ProviderCatalogReadRepository + ProviderCatalogWriteRepository + 'static,
        V: UsageRepository + 'static,
        W: aether_data::repository::wallet::WalletRepository + 'static,
    {
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> =
            request_candidate_repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> =
            request_candidate_repository;
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> =
            provider_catalog_repository.clone();
        let provider_catalog_writer: Arc<dyn ProviderCatalogWriteRepository> =
            provider_catalog_repository;
        let usage_reader: Arc<dyn UsageReadRepository> = usage_repository.clone();
        let usage_writer: Arc<dyn UsageWriteRepository> = usage_repository;
        let wallet_reader: Arc<dyn WalletReadRepository> = wallet_repository.clone();
        let wallet_writer: Arc<dyn WalletWriteRepository> = wallet_repository;

        Self {
            config: GatewayDataConfig::disabled().with_encryption_key(encryption_key),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            billing_reader: Some(billing_repository),
            gemini_file_mapping_reader: None,
            gemini_file_mapping_writer: None,
            global_model_reader: None,
            global_model_writer: None,
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: Some(request_candidate_reader),
            request_candidate_writer: Some(request_candidate_writer),
            provider_catalog_reader: Some(provider_catalog_reader),
            provider_catalog_writer: Some(provider_catalog_writer),
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(usage_reader),
            usage_writer: Some(usage_writer),
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: Some(wallet_reader),
            wallet_writer: Some(wallet_writer),
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn with_auth_candidate_selection_provider_catalog_and_quota_for_tests<T>(
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
        candidate_selection_repository: Arc<dyn MinimalCandidateSelectionReadRepository>,
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        provider_quota_repository: Arc<T>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: ProviderQuotaRepository + 'static,
    {
        let provider_quota_reader: Arc<dyn ProviderQuotaReadRepository> =
            provider_quota_repository.clone();
        let provider_quota_writer: Arc<dyn ProviderQuotaWriteRepository> =
            provider_quota_repository;

        Self {
            config: GatewayDataConfig::disabled().with_encryption_key(encryption_key),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            minimal_candidate_selection_reader: Some(candidate_selection_repository),
            request_candidate_reader: None,
            request_candidate_writer: None,
            provider_catalog_reader: Some(provider_catalog_repository),
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: Some(provider_quota_reader),
            provider_quota_writer: Some(provider_quota_writer),
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_usage_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: UsageRepository + 'static,
    {
        let usage_reader: Arc<dyn UsageReadRepository> = repository.clone();
        let usage_writer: Arc<dyn UsageWriteRepository> = repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(usage_reader),
            usage_writer: Some(usage_writer),
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_billing_reader_for_tests(
        repository: Arc<dyn BillingReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
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
            billing_reader: Some(repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_and_wallet_for_tests<T>(
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
        wallet_repository: Arc<T>,
    ) -> Self
    where
        T: aether_data::repository::wallet::WalletRepository + 'static,
    {
        let wallet_reader: Arc<dyn WalletReadRepository> = wallet_repository.clone();
        let wallet_writer: Arc<dyn WalletWriteRepository> = wallet_repository;
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: Some(wallet_reader),
            wallet_writer: Some(wallet_writer),
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_wallet_and_usage_for_tests<TWallet, TUsage>(
        auth_api_key_repository: Arc<dyn AuthApiKeyReadRepository>,
        wallet_repository: Arc<TWallet>,
        usage_repository: Arc<TUsage>,
    ) -> Self
    where
        TWallet: aether_data::repository::wallet::WalletRepository + 'static,
        TUsage: UsageRepository + 'static,
    {
        let wallet_reader: Arc<dyn WalletReadRepository> = wallet_repository.clone();
        let wallet_writer: Arc<dyn WalletWriteRepository> = wallet_repository;
        let usage_reader: Arc<dyn UsageReadRepository> = usage_repository.clone();
        let usage_writer: Arc<dyn UsageWriteRepository> = usage_repository;
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: Some(auth_api_key_repository),
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
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: Some(usage_reader),
            usage_writer: Some(usage_writer),
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: Some(wallet_reader),
            wallet_writer: Some(wallet_writer),
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_usage_billing_and_wallet_for_tests<TUsage, TWallet>(
        usage_repository: Arc<TUsage>,
        billing_repository: Arc<dyn BillingReadRepository>,
        wallet_repository: Arc<TWallet>,
    ) -> Self
    where
        TUsage: UsageRepository + 'static,
        TWallet: aether_data::repository::wallet::WalletRepository + 'static,
    {
        let usage_reader: Arc<dyn UsageReadRepository> = usage_repository.clone();
        let usage_writer: Arc<dyn UsageWriteRepository> = usage_repository;
        let wallet_reader: Arc<dyn WalletReadRepository> = wallet_repository.clone();
        let wallet_writer: Arc<dyn WalletWriteRepository> = wallet_repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            billing_reader: Some(billing_repository),
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
            usage_reader: Some(usage_reader),
            usage_writer: Some(usage_writer),
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: Some(wallet_reader),
            wallet_writer: Some(wallet_writer),
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_provider_quota_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: ProviderQuotaRepository + 'static,
    {
        let provider_quota_reader: Arc<dyn ProviderQuotaReadRepository> = repository.clone();
        let provider_quota_writer: Arc<dyn ProviderQuotaWriteRepository> = repository;

        Self {
            config: GatewayDataConfig::disabled(),
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
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: Some(provider_quota_reader),
            provider_quota_writer: Some(provider_quota_writer),
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: None,
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }
}
