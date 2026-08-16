use std::sync::Arc;

use super::{
    AnnouncementReadRepository, AnnouncementWriteRepository, GatewayDataConfig, GatewayDataState,
};

impl GatewayDataState {
    #[cfg(test)]
    pub(crate) fn with_announcement_reader_for_tests(
        repository: Arc<dyn AnnouncementReadRepository>,
    ) -> Self {
        Self {
            config: GatewayDataConfig::disabled(),
            backends: None,
            auth_api_key_reader: None,
            auth_api_key_writer: None,
            auth_module_reader: None,
            auth_module_writer: None,
            announcement_reader: Some(repository),
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
            daily_usage_runtime_state: None,
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
    pub(crate) fn with_announcement_repository_for_tests<T>(repository: Arc<T>) -> Self
    where
        T: aether_data::repository::announcements::AnnouncementReadRepository
            + aether_data::repository::announcements::AnnouncementWriteRepository
            + 'static,
    {
        let announcement_reader: Arc<dyn AnnouncementReadRepository> = repository.clone();
        let announcement_writer: Arc<dyn AnnouncementWriteRepository> = repository;
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
            user_reader: None,
            user_preferences: None,
            usage_worker_queue: None,
            daily_usage_runtime_state: None,
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
