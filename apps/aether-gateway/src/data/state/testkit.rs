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
use aether_data::repository::management_tokens::{
    InMemoryManagementTokenRepository, ManagementTokenReadRepository,
    ManagementTokenWriteRepository, StoredManagementToken, StoredManagementTokenUserSummary,
    StoredManagementTokenWithUser,
};
use aether_data::repository::proxy_nodes::{InMemoryProxyNodeRepository, StoredProxyNode};
use aether_data::repository::proxy_nodes::{ProxyNodeReadRepository, ProxyNodeWriteRepository};
use aether_data::repository::routing_profiles::InMemoryRoutingGroupRepository;
use aether_data::repository::users::{
    InMemoryUserReadRepository, StoredUserAuthRecord, UserReadRepository,
};
use aether_data_contracts::repository::routing_profiles::{
    StoredRoutingGroup, StoredRoutingGroupBinding, StoredRoutingGroupVersion,
};
use aether_routing_core::RoutingGroupConfig;
use sha2::{Digest, Sha256};

use super::{GatewayDataConfig, GatewayDataState};

impl GatewayDataState {
    pub(crate) fn with_tunnel_management_auth_for_testkit(
        node_id: &str,
        tunnel_generation: &str,
        raw_token: &str,
        encryption_key: impl Into<String>,
    ) -> Result<Self, aether_data::DataLayerError> {
        const TOKEN_ID: &str = "token-tunnel-harness";
        const USER_ID: &str = "user-tunnel-harness";

        let node = StoredProxyNode::new(
            node_id.to_string(),
            "tunnel harness node".to_string(),
            "127.0.0.1".to_string(),
            0,
            false,
            "offline".to_string(),
            30,
            0,
            0,
            0,
            0,
            0,
            true,
            false,
            0,
        )?
        .with_tunnel_generation(tunnel_generation.to_string());
        let proxy_repository = Arc::new(InMemoryProxyNodeRepository::seed([node]));

        let user_summary = StoredManagementTokenUserSummary::new(
            USER_ID.to_string(),
            Some("tunnel-harness@example.com".to_string()),
            "tunnel_harness_admin".to_string(),
            "admin".to_string(),
        )?;
        let token = StoredManagementToken::new(
            TOKEN_ID.to_string(),
            USER_ID.to_string(),
            "tunnel harness token".to_string(),
        )?
        .with_permissions(Some(serde_json::json!(["admin:proxy_nodes:admin"])));
        let token_hash = format!("{:x}", Sha256::digest(raw_token.as_bytes()));
        let token_repository = Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
            [StoredManagementTokenWithUser::new(token, user_summary)],
            [(token_hash, TOKEN_ID.to_string())],
        ));
        let token_reader: Arc<dyn ManagementTokenReadRepository> = token_repository.clone();
        let token_writer: Arc<dyn ManagementTokenWriteRepository> = token_repository;

        let user = StoredUserAuthRecord::new(
            USER_ID.to_string(),
            Some("tunnel-harness@example.com".to_string()),
            true,
            "tunnel_harness_admin".to_string(),
            None,
            "admin".to_string(),
            "local".to_string(),
            None,
            None,
            None,
            true,
            false,
            None,
            None,
        )?;
        let user_reader: Arc<dyn UserReadRepository> =
            Arc::new(InMemoryUserReadRepository::seed_auth_users([user]));

        let mut state =
            Self::with_proxy_node_repository_for_testkit(proxy_repository, encryption_key);
        state.management_token_reader = Some(token_reader);
        state.management_token_writer = Some(token_writer);
        state.user_reader = Some(user_reader);
        Ok(state)
    }

    pub(crate) fn with_proxy_node_repository_for_testkit<T>(
        repository: Arc<T>,
        encryption_key: impl Into<String>,
    ) -> Self
    where
        T: ProxyNodeReadRepository + ProxyNodeWriteRepository + 'static,
    {
        let proxy_node_reader: Arc<dyn ProxyNodeReadRepository> = repository.clone();
        let proxy_node_writer: Arc<dyn ProxyNodeWriteRepository> = repository;
        let mut state = Self::disabled();
        state.config = GatewayDataConfig::disabled().with_encryption_key(encryption_key);
        state.proxy_node_reader = Some(proxy_node_reader);
        state.proxy_node_writer = Some(proxy_node_writer);
        state
    }

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
        let routing_groups = Arc::new(InMemoryRoutingGroupRepository::seed(
            [StoredRoutingGroup {
                id: "system-default".to_string(),
                name: "system-default".to_string(),
                description: Some("pressure harness routing strategy".to_string()),
                enabled: true,
                is_system_default: true,
                sort_order: 0,
                config_json: serde_json::to_value(RoutingGroupConfig::default())
                    .expect("default routing config should serialize"),
                version: 1,
                created_at: 1,
                updated_at: 1,
                published_at: Some(1),
            }],
            std::iter::empty::<StoredRoutingGroupBinding>(),
            std::iter::empty::<StoredRoutingGroupVersion>(),
        ));

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
            routing_group_reader: Some(routing_groups.clone()),
            routing_group_writer: Some(routing_groups),
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
