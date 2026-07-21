use std::time::{SystemTime, UNIX_EPOCH};

use super::json;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::announcements::{
    InMemoryAnnouncementReadRepository, StoredAnnouncement,
};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::auth_modules::{
    InMemoryAuthModuleReadRepository, StoredLdapModuleConfig, StoredOAuthProviderModuleConfig,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::global_models::InMemoryGlobalModelReadRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::global_models::{
    StoredPublicCatalogModel, StoredPublicGlobalModel,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskLookupKey, VideoTaskReadRepository, VideoTaskStatus,
    VideoTaskWriteRepository,
};
use base64::Engine as _;
use sha2::{Digest, Sha256};

fn run_frontdoor_async_test<F>(name: &'static str, future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("frontdoor test runtime should build")
                .block_on(future);
        })
        .expect("large-stack frontdoor test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn explicit_user_limit_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(10),
        Some(5),
        Some(4_102_444_800),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
    )
    .expect("snapshot should build")
    .with_user_rate_limit(Some(1))
}

fn system_default_user_limit_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(10),
        Some(5),
        Some(4_102_444_800),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
    )
    .expect("snapshot should build")
}

fn unrestricted_models_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        None,
        None,
        None,
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(10),
        Some(5),
        Some(4_102_444_800),
        None,
        None,
        None,
    )
    .expect("snapshot should build")
}

fn sample_provider(id: &str, name: &str, priority: i32) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        id.to_string(),
        name.to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_routing_fields(priority)
}

fn sample_endpoint(
    id: &str,
    provider_id: &str,
    api_format: &str,
    base_url: &str,
) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        id.to_string(),
        provider_id.to_string(),
        api_format.to_string(),
        None,
        None,
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        base_url.to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("endpoint transport should build")
}

fn sample_key(
    id: &str,
    provider_id: &str,
    api_format: &str,
    secret: &str,
) -> StoredProviderCatalogKey {
    let encrypted_api_key = encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, secret)
        .expect("api key ciphertext should build");
    StoredProviderCatalogKey::new(
        id.to_string(),
        provider_id.to_string(),
        "default".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!([api_format])),
        encrypted_api_key,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build")
}

fn sample_request_candidate(
    id: &str,
    request_id: &str,
    endpoint_id: &str,
    status: RequestCandidateStatus,
    created_at_unix_secs: i64,
    finished_at_unix_secs: Option<i64>,
) -> StoredRequestCandidate {
    let created_at_unix_ms = created_at_unix_secs * 1_000;
    let finished_at_unix_ms = finished_at_unix_secs.map(|v| v * 1_000);
    StoredRequestCandidate::new(
        id.to_string(),
        request_id.to_string(),
        Some("user-1".to_string()),
        Some("api-key-1".to_string()),
        Some("alice".to_string()),
        Some("default".to_string()),
        0,
        0,
        Some("provider-1".to_string()),
        Some(endpoint_id.to_string()),
        Some("key-1".to_string()),
        status,
        None,
        false,
        Some(200),
        matches!(status, RequestCandidateStatus::Failed).then_some("rate_limit".to_string()),
        None,
        Some(120),
        Some(1),
        None,
        None,
        created_at_unix_ms,
        Some(created_at_unix_ms),
        finished_at_unix_ms,
    )
    .expect("request candidate should build")
}

fn sample_models_candidate_row(
    provider_id: &str,
    provider_name: &str,
    api_format: &str,
    global_model_name: &str,
    provider_priority: i32,
) -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: provider_id.to_string(),
        provider_name: provider_name.to_string(),
        provider_type: "custom".to_string(),
        provider_priority,
        provider_is_active: true,
        endpoint_id: format!("endpoint-{provider_id}"),
        endpoint_api_format: api_format.to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_is_active: true,
        key_id: format!("key-{provider_id}"),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec![api_format.to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 50,
        key_global_priority_by_format: None,
        model_id: format!("model-{provider_id}-{global_model_name}"),
        global_model_id: format!("global-{global_model_name}"),
        global_model_name: global_model_name.to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: global_model_name.to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: global_model_name.to_string(),
            priority: 1,
            api_formats: Some(vec![api_format.to_string()]),
            endpoint_ids: None,
            operations: None,
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn sample_public_global_model(
    id: &str,
    name: &str,
    display_name: &str,
    is_active: bool,
) -> StoredPublicGlobalModel {
    StoredPublicGlobalModel::new(
        id.to_string(),
        name.to_string(),
        Some(display_name.to_string()),
        is_active,
        Some(0.02),
        Some(json!({"tiers":[{"up_to": null, "input_price_per_1m": 3.0, "output_price_per_1m": 15.0}]})),
        Some(json!(["vision"])),
        Some(json!({"family": "test"})),
        0,
    )
    .expect("global model should build")
}

fn sample_public_global_model_with_capabilities(
    id: &str,
    name: &str,
    display_name: &str,
    supported_capabilities: serde_json::Value,
) -> StoredPublicGlobalModel {
    StoredPublicGlobalModel::new(
        id.to_string(),
        name.to_string(),
        Some(display_name.to_string()),
        true,
        Some(0.02),
        Some(json!({"tiers":[{"up_to": null, "input_price_per_1m": 3.0, "output_price_per_1m": 15.0}]})),
        Some(supported_capabilities),
        Some(json!({"family": "test"})),
        0,
    )
    .expect("global model should build")
}

fn sample_public_catalog_model(
    id: &str,
    provider_id: &str,
    provider_name: &str,
    provider_model_name: &str,
    name: &str,
    display_name: &str,
) -> StoredPublicCatalogModel {
    StoredPublicCatalogModel::new(
        id.to_string(),
        provider_id.to_string(),
        provider_name.to_string(),
        provider_model_name.to_string(),
        name.to_string(),
        display_name.to_string(),
        Some(format!("{display_name} description")),
        Some(format!("https://cdn.example/{name}.png")),
        Some(3.0),
        Some(15.0),
        Some(1.5),
        Some(0.3),
        Some(true),
        Some(true),
        Some(true),
        Some(false),
        true,
    )
    .expect("public catalog model should build")
}

mod ai;
mod core;
mod internal;
mod oauth;
mod ops;
mod public_support;
