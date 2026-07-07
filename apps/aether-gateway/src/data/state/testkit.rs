use std::sync::Arc;

use aether_data_contracts::repository::candidate_selection::MinimalCandidateSelectionReadRepository;
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateRepository, RequestCandidateWriteRepository,
};
use aether_data_contracts::repository::provider_catalog::{
    ProviderCatalogReadRepository, ProviderCatalogWriteRepository,
};
use aether_data_contracts::repository::usage::{
    UsageReadRepository, UsageRepository, UsageWriteRepository,
};

use aether_data::repository::auth::AuthApiKeyReadRepository;

use super::{GatewayDataConfig, GatewayDataState};

impl GatewayDataState {
    pub(crate) fn with_openai_chat_pressure_repositories_for_testkit<T, U, V>(
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
            background_task_reader: None,
            background_task_writer: None,
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
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
            system_config_value_cache: Default::default(),
            billing_model_context_cache: Default::default(),
        }
    }
}
