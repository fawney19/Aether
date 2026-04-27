use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogKey, StoredProviderCatalogProvider,
};

use crate::data::auth::GatewayAuthApiKeySnapshot;

pub(super) fn sample_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-1".to_string(),
        provider_name: "OpenAI".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: "endpoint-1".to_string(),
        endpoint_api_format: "openai:chat".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_is_active: true,
        key_id: "key-1".to_string(),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:chat".to_string()]),
        key_allowed_models: None,
        key_capabilities: Some(serde_json::json!({"cache_1h": true})),
        key_internal_priority: 50,
        key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 2})),
        model_id: "model-1".to_string(),
        global_model_id: "global-model-1".to_string(),
        global_model_name: "gpt-4.1".to_string(),
        global_model_mappings: Some(vec!["gpt-4\\.1-.*".to_string()]),
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "gpt-4.1-upstream".to_string(),
        model_provider_model_mappings: Some(vec![
            StoredProviderModelMapping {
                name: "gpt-4.1-canary".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:chat".to_string()]),
            },
            StoredProviderModelMapping {
                name: "gpt-4.1-responses".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:responses".to_string()]),
            },
        ]),
        model_supports_streaming: None,
        model_is_active: true,
        model_is_available: true,
    }
}

pub(super) fn sample_provider(
    id: &str,
    concurrent_limit: Option<i32>,
) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        id.to_string(),
        format!("provider-{id}"),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        false,
        concurrent_limit,
        None,
        None,
        None,
        None,
        None,
    )
}

pub(super) fn sample_key(
    id: &str,
    provider_id: &str,
    rpm_limit: Option<u32>,
) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        id.to_string(),
        provider_id.to_string(),
        format!("key-{id}"),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_rate_limit_fields(
        rpm_limit,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(20),
        Some(20),
    )
}

pub(super) fn sample_auth_snapshot(api_key_id: &str) -> GatewayAuthApiKeySnapshot {
    GatewayAuthApiKeySnapshot {
        user_id: "user-1".to_string(),
        username: "alice".to_string(),
        email: None,
        user_role: "user".to_string(),
        user_auth_source: "local".to_string(),
        user_is_active: true,
        user_is_deleted: false,
        user_rate_limit: None,
        user_allowed_providers: None,
        user_allowed_api_formats: None,
        user_allowed_models: None,
        api_key_id: api_key_id.to_string(),
        api_key_name: Some("default".to_string()),
        api_key_is_active: true,
        api_key_is_locked: false,
        api_key_is_standalone: false,
        api_key_rate_limit: None,
        api_key_concurrent_limit: None,
        api_key_expires_at_unix_secs: None,
        api_key_allowed_providers: None,
        api_key_allowed_api_formats: None,
        api_key_allowed_models: None,
        currently_usable: true,
    }
}
