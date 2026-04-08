use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use chrono::{DateTime, Utc};
use hmac::Mac;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{
    RequestCandidateStatus, StoredAdminGlobalModel, StoredAdminProviderModel,
    StoredAuthApiKeySnapshot, StoredLdapModuleConfig, StoredManagementToken,
    StoredManagementTokenUserSummary, StoredManagementTokenWithUser, StoredOAuthProviderConfig,
    StoredOAuthProviderModuleConfig, StoredProviderActiveGlobalModel,
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
    StoredProviderModelStats, StoredProviderQuotaSnapshot, StoredProxyNode,
    StoredPublicGlobalModel, StoredRequestCandidate,
};
use crate::AppState;

pub(super) fn sample_currently_usable_auth_snapshot(
    api_key_id: &str,
    user_id: &str,
) -> StoredAuthApiKeySnapshot {
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
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
    )
    .expect("auth snapshot should build")
}

pub(super) fn sample_expired_auth_snapshot(
    api_key_id: &str,
    user_id: &str,
) -> StoredAuthApiKeySnapshot {
    let mut snapshot = sample_currently_usable_auth_snapshot(api_key_id, user_id);
    snapshot.api_key_expires_at_unix_secs = Some(1);
    snapshot
}

pub(super) fn sample_locked_auth_snapshot(
    api_key_id: &str,
    user_id: &str,
) -> StoredAuthApiKeySnapshot {
    let mut snapshot = sample_currently_usable_auth_snapshot(api_key_id, user_id);
    snapshot.api_key_is_locked = true;
    snapshot
}

pub(super) fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub(super) fn test_auth_secret() -> String {
    std::env::var("JWT_SECRET_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "aether-rust-dev-jwt-secret".to_string())
}

pub(super) fn build_test_auth_token(
    token_type: &str,
    mut payload: serde_json::Map<String, serde_json::Value>,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> String {
    use base64::Engine as _;

    let header = json!({ "alg": "HS256", "typ": "JWT" });
    payload.insert("exp".to_string(), json!(expires_at.timestamp()));
    payload.insert("type".to_string(), json!(token_type));
    let header_segment = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&header)
            .expect("jwt header should serialize")
            .as_slice(),
    );
    let payload_segment = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&payload)
            .expect("jwt payload should serialize")
            .as_slice(),
    );
    let signing_input = format!("{header_segment}.{payload_segment}");
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(test_auth_secret().as_bytes())
        .expect("jwt secret should build");
    mac.update(signing_input.as_bytes());
    let signature = mac.finalize().into_bytes();
    format!(
        "{header_segment}.{payload_segment}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_slice())
    )
}

pub(super) async fn issue_test_admin_access_token(
    state: &AppState,
    client_device_id: &str,
) -> String {
    let user = state
        .create_local_auth_user_with_settings(
            Some("admin@example.com".to_string()),
            true,
            "admin".to_string(),
            "hash".to_string(),
            "admin".to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("admin user should be created")
        .expect("admin user should exist");
    let now = chrono::Utc::now();
    let session_id = "session-admin-token".to_string();
    let refresh_token = "refresh-session-admin-token".to_string();
    let session = crate::data::state::StoredUserSessionRecord::new(
        session_id.clone(),
        user.id.clone(),
        client_device_id.to_string(),
        None,
        crate::data::state::StoredUserSessionRecord::hash_refresh_token(&refresh_token),
        None,
        None,
        Some(now),
        Some(now + chrono::Duration::days(7)),
        None,
        None,
        Some("127.0.0.1".to_string()),
        Some("admin-test".to_string()),
        Some(now),
        Some(now),
    )
    .expect("session should build");
    state
        .create_user_session(session)
        .await
        .expect("session should persist")
        .expect("session should exist");
    build_test_auth_token(
        "access",
        serde_json::Map::from_iter([
            ("user_id".to_string(), json!(user.id)),
            ("role".to_string(), json!("admin")),
            (
                "created_at".to_string(),
                json!(user.created_at.map(|value| value.to_rfc3339())),
            ),
            ("session_id".to_string(), json!(session_id)),
        ]),
        now + chrono::Duration::hours(12),
    )
}

pub(super) fn sample_provider(
    id: &str,
    name: &str,
    priority: i32,
) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        id.to_string(),
        name.to_string(),
        Some("https://example.com".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_routing_fields(priority)
}

pub(super) fn sample_proxy_node(node_id: &str) -> StoredProxyNode {
    StoredProxyNode::new(
        node_id.to_string(),
        "proxy-node-1".to_string(),
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
        7,
    )
    .expect("proxy node should build")
    .with_runtime_fields(
        Some("test".to_string()),
        Some("system".to_string()),
        Some(1_710_000_000),
        None,
        None,
        None,
        None,
        Some(1_710_000_010),
        Some(json!({
            "upgrade_to": "1.2.3",
            "allowed_ports": [443],
        })),
        Some(1_709_000_000),
        Some(1_710_000_100),
    )
}

pub(super) fn sample_provider_quota(provider_id: &str) -> StoredProviderQuotaSnapshot {
    StoredProviderQuotaSnapshot::new(
        provider_id.to_string(),
        "monthly_quota".to_string(),
        Some(100.0),
        12.5,
        Some(30),
        Some(1_711_000_000),
        Some(1_711_000_000 + 30 * 24 * 60 * 60),
        true,
    )
    .expect("provider quota should build")
}

pub(super) fn sample_provider_model_stats(
    provider_id: &str,
    total_models: i64,
    active_models: i64,
) -> StoredProviderModelStats {
    StoredProviderModelStats::new(provider_id.to_string(), total_models, active_models)
        .expect("provider model stats should build")
}

pub(super) fn sample_provider_active_global_model(
    provider_id: &str,
    global_model_id: &str,
) -> StoredProviderActiveGlobalModel {
    StoredProviderActiveGlobalModel::new(provider_id.to_string(), global_model_id.to_string())
        .expect("provider active global model should build")
}

pub(super) fn sample_management_token(
    token_id: &str,
    user_id: &str,
    username: &str,
    is_active: bool,
) -> StoredManagementTokenWithUser {
    let token = StoredManagementToken::new(
        token_id.to_string(),
        user_id.to_string(),
        format!("{username}-token"),
    )
    .expect("management token should build")
    .with_display_fields(
        Some(format!("{username} token")),
        Some("ae_test".to_string()),
        Some(json!(["127.0.0.1"])),
    )
    .with_runtime_fields(
        Some(4_102_444_800),
        Some(1_711_000_000),
        Some("127.0.0.1".to_string()),
        7,
        is_active,
    )
    .with_timestamps(Some(1_710_000_000), Some(1_711_000_100));
    let user = StoredManagementTokenUserSummary::new(
        user_id.to_string(),
        Some(format!("{username}@example.com")),
        username.to_string(),
        "admin".to_string(),
    )
    .expect("management token user should build");
    StoredManagementTokenWithUser::new(token, user)
}

pub(super) fn sample_admin_provider_model(
    id: &str,
    provider_id: &str,
    global_model_id: &str,
    provider_model_name: &str,
) -> StoredAdminProviderModel {
    StoredAdminProviderModel::new(
        id.to_string(),
        provider_id.to_string(),
        global_model_id.to_string(),
        provider_model_name.to_string(),
        Some(json!([{"name": format!("{provider_model_name}-alias"), "priority": 1}])),
        Some(0.02),
        Some(json!({
            "tiers": [{
                "up_to": null,
                "input_price_per_1m": 3.0,
                "output_price_per_1m": 15.0,
            }]
        })),
        Some(true),
        Some(true),
        None,
        Some(false),
        Some(false),
        true,
        true,
        Some(json!({"billing": {"mode": "local"}, "provider_hint": provider_model_name})),
        Some(1_711_000_000),
        Some(1_711_000_100),
        Some("gpt-5".to_string()),
        Some("GPT 5".to_string()),
        Some(0.03),
        Some(json!({
            "tiers": [{
                "up_to": null,
                "input_price_per_1m": 4.0,
                "output_price_per_1m": 20.0,
            }]
        })),
        Some(json!({"streaming": true, "vision": false, "billing": {"currency": "USD"}})),
    )
    .expect("admin provider model should build")
}

pub(super) fn sample_admin_global_model(
    id: &str,
    name: &str,
    display_name: &str,
) -> StoredAdminGlobalModel {
    StoredAdminGlobalModel::new(
        id.to_string(),
        name.to_string(),
        display_name.to_string(),
        true,
        Some(0.03),
        Some(json!({
            "tiers": [{
                "up_to": null,
                "input_price_per_1m": 4.0,
                "output_price_per_1m": 20.0,
            }]
        })),
        Some(json!(["streaming", "vision"])),
        Some(json!({"streaming": true, "vision": false, "billing": {"currency": "USD"}})),
        0,
        0,
        0,
        Some(1_711_000_000),
        Some(1_711_000_100),
    )
    .expect("admin global model should build")
}

pub(super) fn sample_public_global_model_with_mappings(
    id: &str,
    name: &str,
    display_name: &str,
    mappings: &[&str],
) -> StoredPublicGlobalModel {
    StoredPublicGlobalModel::new(
        id.to_string(),
        name.to_string(),
        Some(display_name.to_string()),
        true,
        None,
        None,
        None,
        Some(json!({ "model_mappings": mappings })),
        0,
    )
    .expect("public global model should build")
}

pub(super) fn sample_oauth_module_provider(
    provider_type: &str,
    display_name: &str,
) -> StoredOAuthProviderModuleConfig {
    StoredOAuthProviderModuleConfig::new(
        provider_type.to_string(),
        display_name.to_string(),
        "client-id".to_string(),
        Some("encrypted-secret".to_string()),
        "https://example.com/oauth/callback".to_string(),
    )
    .expect("oauth module provider should build")
}

pub(super) fn sample_ldap_module_config() -> StoredLdapModuleConfig {
    StoredLdapModuleConfig {
        server_url: "ldaps://ldap.example.com".to_string(),
        bind_dn: "cn=admin,dc=example,dc=com".to_string(),
        bind_password_encrypted: Some("encrypted-password".to_string()),
        base_dn: "dc=example,dc=com".to_string(),
        user_search_filter: Some("(uid={username})".to_string()),
        username_attr: Some("uid".to_string()),
        email_attr: Some("mail".to_string()),
        display_name_attr: Some("displayName".to_string()),
        is_enabled: true,
        is_exclusive: false,
        use_starttls: true,
        connect_timeout: Some(10),
    }
}

pub(super) fn sample_oauth_provider_config(provider_type: &str) -> StoredOAuthProviderConfig {
    StoredOAuthProviderConfig::new(
        provider_type.to_string(),
        "Linux Do".to_string(),
        "client-id".to_string(),
        "https://backend.example.com/oauth/callback".to_string(),
        "https://frontend.example.com/auth/callback".to_string(),
    )
    .expect("oauth provider config should build")
    .with_config_fields(
        Some(
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "secret-value")
                .expect("secret should encrypt"),
        ),
        Some("https://connect.linux.do/oauth2/authorize".to_string()),
        Some("https://connect.linux.do/oauth2/token".to_string()),
        Some("https://connect.linux.do/api/user".to_string()),
        Some(vec!["openid".to_string()]),
        Some(json!({"email": "email"})),
        Some(json!({"team": true})),
        true,
    )
}

pub(super) fn sample_endpoint(
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
    .with_health_score(0.9)
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

pub(super) fn sample_key(
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

pub(super) fn sample_request_candidate(
    id: &str,
    request_id: &str,
    endpoint_id: &str,
    status: RequestCandidateStatus,
    created_at_unix_secs: i64,
    finished_at_unix_secs: Option<i64>,
) -> StoredRequestCandidate {
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
        created_at_unix_secs,
        Some(created_at_unix_secs),
        finished_at_unix_secs,
    )
    .expect("request candidate should build")
}

pub(super) fn sample_recent_key_rpm_candidate(
    id: &str,
    request_id: &str,
    endpoint_id: &str,
    key_id: &str,
    now_unix_secs: i64,
    concurrent_requests: i32,
) -> StoredRequestCandidate {
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
        Some(key_id.to_string()),
        RequestCandidateStatus::Success,
        None,
        false,
        Some(200),
        None,
        None,
        Some(120),
        Some(concurrent_requests),
        None,
        None,
        now_unix_secs - 10,
        Some(now_unix_secs - 10),
        Some(now_unix_secs - 8),
    )
    .expect("request candidate should build")
}
