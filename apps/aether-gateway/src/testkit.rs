use std::sync::Arc;

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use sha2::{Digest, Sha256};

use crate::data::GatewayDataState;
use crate::AppState;

#[derive(Debug, Clone)]
pub struct OpenAiChatPressureTarget {
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct OpenAiChatPressureStateConfig {
    pub client_api_key: String,
    pub api_key_id: String,
    pub user_id: String,
    pub requested_model: String,
    pub provider_model: String,
    pub targets: Vec<OpenAiChatPressureTarget>,
    pub max_in_flight_requests: Option<usize>,
}

impl OpenAiChatPressureStateConfig {
    pub fn new(target_base_urls: Vec<String>) -> Self {
        Self {
            client_api_key: "sk-aether-openai-chat-pressure".to_string(),
            api_key_id: "api-key-openai-chat-pressure".to_string(),
            user_id: "user-openai-chat-pressure".to_string(),
            requested_model: "gpt-5".to_string(),
            provider_model: "gpt-5-upstream".to_string(),
            targets: target_base_urls
                .into_iter()
                .map(|base_url| OpenAiChatPressureTarget { base_url })
                .collect(),
            max_in_flight_requests: None,
        }
    }
}

pub fn build_openai_chat_pressure_state(
    config: OpenAiChatPressureStateConfig,
) -> Result<AppState, String> {
    if config.targets.is_empty() {
        return Err("at least one pressure target is required".to_string());
    }

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(&config.client_api_key)),
        openai_chat_pressure_auth_snapshot(&config),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(
            openai_chat_pressure_candidates(&config),
        ));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![openai_chat_pressure_provider()],
        openai_chat_pressure_endpoints(&config),
        openai_chat_pressure_keys(&config)?,
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    let data_state = GatewayDataState::with_openai_chat_pressure_repositories_for_testkit(
        auth_repository,
        candidate_selection_repository,
        provider_catalog_repository,
        request_candidate_repository,
        usage_repository,
        DEVELOPMENT_ENCRYPTION_KEY,
    );

    let mut state =
        AppState::new().map_err(|err| format!("failed to build pressure gateway state: {err}"))?;
    state.replace_data_state(Arc::new(data_state));
    if let Some(limit) = config.max_in_flight_requests {
        state = state.with_request_concurrency_limit(limit);
    }
    Ok(state)
}

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn openai_chat_pressure_auth_snapshot(
    config: &OpenAiChatPressureStateConfig,
) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        config.user_id.clone(),
        "pressure".to_string(),
        Some("pressure@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!([config.requested_model.clone()])),
        config.api_key_id.clone(),
        Some("pressure".to_string()),
        true,
        false,
        false,
        Some(600_000),
        Some(20_000),
        Some(4_102_444_800),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!([config.requested_model.clone()])),
    )
    .expect("pressure auth snapshot should build")
}

fn openai_chat_pressure_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-openai-chat-pressure".to_string(),
        "openai".to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("pressure provider should build")
    .with_transport_fields(true, false, false, None, None, None, Some(20.0), None, None)
}

fn openai_chat_pressure_endpoints(
    config: &OpenAiChatPressureStateConfig,
) -> Vec<StoredProviderCatalogEndpoint> {
    config
        .targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            StoredProviderCatalogEndpoint::new(
                pressure_endpoint_id(index),
                "provider-openai-chat-pressure".to_string(),
                "openai:chat".to_string(),
                Some("openai".to_string()),
                Some("chat".to_string()),
                true,
            )
            .expect("pressure endpoint should build")
            .with_transport_fields(
                target.base_url.trim_end_matches('/').to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("pressure endpoint transport should build")
        })
        .collect()
}

fn openai_chat_pressure_keys(
    config: &OpenAiChatPressureStateConfig,
) -> Result<Vec<StoredProviderCatalogKey>, String> {
    config
        .targets
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let encrypted = encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                &format!("sk-upstream-openai-chat-pressure-{index}"),
            )
            .map_err(|err| format!("failed to encrypt pressure key: {err}"))?;
            StoredProviderCatalogKey::new(
                pressure_key_id(index),
                "provider-openai-chat-pressure".to_string(),
                format!("pressure-{index}"),
                "api_key".to_string(),
                None,
                true,
            )
            .expect("pressure key should build")
            .with_transport_fields(
                Some(serde_json::json!(["openai:chat"])),
                encrypted,
                None,
                None,
                Some(serde_json::json!({"openai:chat": 1})),
                None,
                None,
                None,
                None,
            )
            .map_err(|err| format!("failed to build pressure key transport: {err}"))
        })
        .collect()
}

fn openai_chat_pressure_candidates(
    config: &OpenAiChatPressureStateConfig,
) -> Vec<StoredMinimalCandidateSelectionRow> {
    config
        .targets
        .iter()
        .enumerate()
        .map(|(index, _)| StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-chat-pressure".to_string(),
            provider_name: "openai".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: pressure_endpoint_id(index),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: pressure_key_id(index),
            key_name: format!("pressure-{index}"),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 1})),
            model_id: pressure_model_id(index),
            global_model_id: "global-model-openai-chat-pressure".to_string(),
            global_model_name: config.requested_model.clone(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: config.provider_model.clone(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: config.provider_model.clone(),
                priority: 1,
                api_formats: Some(vec!["openai:chat".to_string()]),
                endpoint_ids: Some(vec![pressure_endpoint_id(index)]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        })
        .collect()
}

fn pressure_endpoint_id(index: usize) -> String {
    format!("endpoint-openai-chat-pressure-{index}")
}

fn pressure_key_id(index: usize) -> String {
    format!("key-openai-chat-pressure-{index}")
}

fn pressure_model_id(index: usize) -> String {
    format!("model-openai-chat-pressure-{index}")
}
