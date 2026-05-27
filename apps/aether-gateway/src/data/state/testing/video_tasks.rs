use std::sync::Arc;

use aether_data_contracts::repository::candidates::RequestCandidateRepository;
use aether_data_contracts::repository::video_tasks::VideoTaskRepository;

use super::{
    AuthApiKeyReadRepository, GatewayDataConfig, GatewayDataState, ProviderCatalogReadRepository,
    RequestCandidateReadRepository, RequestCandidateWriteRepository, VideoTaskReadRepository,
    VideoTaskWriteRepository,
};

impl GatewayDataState {
    pub(crate) fn with_video_task_reader_for_tests(
        repository: Arc<dyn VideoTaskReadRepository>,
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
            provider_catalog_reader: None,
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            risk_control_reader: None,
            risk_control_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: Some(repository),
            video_task_writer: None,
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_auth_and_video_task_repository_for_tests<T>(
        auth_repository: Arc<dyn AuthApiKeyReadRepository>,
        repository: Arc<T>,
    ) -> Self
    where
        T: VideoTaskRepository + 'static,
    {
        let video_task_reader: Arc<dyn VideoTaskReadRepository> = repository.clone();
        let video_task_writer: Arc<dyn VideoTaskWriteRepository> = repository;

        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: Some(auth_repository),
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
            risk_control_reader: None,
            risk_control_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: Some(video_task_reader),
            video_task_writer: Some(video_task_writer),
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_video_task_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: VideoTaskRepository + 'static,
    {
        let video_task_reader: Arc<dyn VideoTaskReadRepository> = repository.clone();
        let video_task_writer: Arc<dyn VideoTaskWriteRepository> = repository;

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
            risk_control_reader: None,
            risk_control_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: Some(video_task_reader),
            video_task_writer: Some(video_task_writer),
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_video_task_repository_and_provider_transport_for_tests<T>(
        repository: Arc<T>,
        provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: VideoTaskRepository + 'static,
    {
        let video_task_reader: Arc<dyn VideoTaskReadRepository> = repository.clone();
        let video_task_writer: Arc<dyn VideoTaskWriteRepository> = repository;

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
            provider_catalog_reader: Some(provider_catalog_repository),
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            risk_control_reader: None,
            risk_control_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: Some(video_task_reader),
            video_task_writer: Some(video_task_writer),
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_video_task_and_request_candidate_repository_for_tests<T, U>(
        repository: Arc<T>,
        request_candidate_repository: Arc<U>,
    ) -> Self
    where
        T: VideoTaskRepository + 'static,
        U: RequestCandidateRepository + 'static,
    {
        let video_task_reader: Arc<dyn VideoTaskReadRepository> = repository.clone();
        let video_task_writer: Arc<dyn VideoTaskWriteRepository> = repository;
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> =
            request_candidate_repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> =
            request_candidate_repository;

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
            risk_control_reader: None,
            risk_control_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: Some(video_task_reader),
            video_task_writer: Some(video_task_writer),
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_video_task_provider_transport_and_request_candidate_repository_for_tests<
        T,
        U,
        V,
    >(
        repository: Arc<T>,
        provider_catalog_repository: Arc<U>,
        request_candidate_repository: Arc<V>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: VideoTaskRepository + 'static,
        U: ProviderCatalogReadRepository + 'static,
        V: RequestCandidateRepository + 'static,
    {
        let video_task_reader: Arc<dyn VideoTaskReadRepository> = repository.clone();
        let video_task_writer: Arc<dyn VideoTaskWriteRepository> = repository;
        let request_candidate_reader: Arc<dyn RequestCandidateReadRepository> =
            request_candidate_repository.clone();
        let request_candidate_writer: Arc<dyn RequestCandidateWriteRepository> =
            request_candidate_repository;
        let provider_catalog_reader: Arc<dyn ProviderCatalogReadRepository> =
            provider_catalog_repository;

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
            request_candidate_reader: Some(request_candidate_reader),
            request_candidate_writer: Some(request_candidate_writer),
            provider_catalog_reader: Some(provider_catalog_reader),
            provider_catalog_writer: None,
            pool_score_reader: None,
            pool_score_writer: None,
            provider_quota_reader: None,
            provider_quota_writer: None,
            risk_control_reader: None,
            risk_control_writer: None,
            routing_group_reader: None,
            routing_group_writer: None,
            usage_reader: None,
            usage_writer: None,
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            video_task_reader: Some(video_task_reader),
            video_task_writer: Some(video_task_writer),
            background_task_reader: None,
            background_task_writer: None,
            wallet_reader: None,
            wallet_writer: None,
            settlement_writer: None,
            system_config_values: None,
        }
    }
}
