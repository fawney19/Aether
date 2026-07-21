use super::state::{
    decode_jwt_claims, enrich_admin_provider_oauth_auth_config, json_non_empty_string,
    json_u64_value,
};
use crate::ai_serving::{
    build_provider_key_pool_score_upsert, provider_key_pool_score_id, provider_key_pool_score_scope,
};
use crate::handlers::admin::admin_provider_pool_config;
use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_active_api_formats;
use crate::GatewayError;
use aether_data_contracts::repository::pool_scores::{
    GetPoolMemberScoresByIdsQuery, PoolMemberIdentity,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use aether_provider_transport::{
    grok_browser_transport_fingerprint_from_auth_config, provider_types::provider_type_is_fixed,
};
use serde_json::{json, Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) fn provider_oauth_key_proxy_value(
    proxy_node_id: Option<&str>,
) -> Option<serde_json::Value> {
    proxy_node_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| json!({ "node_id": value, "enabled": true }))
}

pub(crate) fn provider_oauth_active_api_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
) -> Vec<String> {
    provider_active_api_formats(endpoints)
}

pub(crate) fn provider_oauth_token_payload_expires_at_unix_secs(
    token_payload: &serde_json::Value,
    now_unix_secs: u64,
) -> Option<u64> {
    json_u64_value(
        token_payload
            .get("expires_in")
            .or_else(|| token_payload.get("expiresIn")),
    )
    .map(|expires_in| now_unix_secs.saturating_add(expires_in))
    .or_else(|| {
        json_u64_value(
            token_payload
                .get("expires_at")
                .or_else(|| token_payload.get("expiresAt"))
                .or_else(|| token_payload.get("expiry"))
                .or_else(|| token_payload.get("exp")),
        )
    })
    .or_else(|| {
        let access_token = json_non_empty_string(token_payload.get("access_token"))?;
        let claims = decode_jwt_claims(&access_token)?;
        json_u64_value(claims.get("exp"))
    })
}

pub(crate) fn build_provider_oauth_auth_config_from_token_payload(
    provider_type: &str,
    token_payload: &serde_json::Value,
) -> (
    serde_json::Map<String, serde_json::Value>,
    Option<String>,
    Option<String>,
    Option<u64>,
) {
    let access_token = json_non_empty_string(token_payload.get("access_token"));
    let refresh_token = json_non_empty_string(token_payload.get("refresh_token"));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let expires_at =
        provider_oauth_token_payload_expires_at_unix_secs(token_payload, now_unix_secs);

    let mut auth_config = serde_json::Map::new();
    auth_config.insert("provider_type".to_string(), json!(provider_type));
    auth_config.insert("updated_at".to_string(), json!(now_unix_secs));
    if let Some(token_type) = token_payload.get("token_type").cloned() {
        auth_config.insert("token_type".to_string(), token_type);
    }
    if let Some(refresh_token) = refresh_token.as_ref() {
        auth_config.insert("refresh_token".to_string(), json!(refresh_token));
    }
    if let Some(expires_at) = expires_at {
        auth_config.insert("expires_at".to_string(), json!(expires_at));
    }
    if let Some(scope) = token_payload.get("scope").cloned() {
        auth_config.insert("scope".to_string(), scope);
    }
    enrich_admin_provider_oauth_auth_config(provider_type, &mut auth_config, token_payload);
    (auth_config, access_token, refresh_token, expires_at)
}

fn grok_oauth_catalog_key_fingerprint(
    provider_type: &str,
    auth_config: &Map<String, Value>,
) -> Option<Value> {
    if !provider_type.trim().eq_ignore_ascii_case("grok") {
        return None;
    }
    grok_browser_transport_fingerprint_from_auth_config(auth_config)
}

pub(crate) async fn create_provider_oauth_catalog_key(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider_type: &str,
    name: &str,
    access_token: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    api_formats: &[String],
    proxy: Option<serde_json::Value>,
    expires_at_unix_secs: Option<u64>,
) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
    let Some(encrypted_api_key) = state.encrypt_catalog_secret_with_fallbacks(access_token) else {
        return Ok(None);
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(encrypted_auth_config) =
        state.encrypt_catalog_secret_with_fallbacks(&auth_config_json)
    else {
        return Ok(None);
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut record = StoredProviderCatalogKey::new(
        Uuid::new_v4().to_string(),
        provider_id.to_string(),
        name.to_string(),
        "oauth".to_string(),
        None,
        true,
    )
    .map_err(|err| GatewayError::Internal(err.to_string()))?
    .with_transport_fields(
        provider_oauth_catalog_key_api_formats(provider_type, api_formats),
        encrypted_api_key,
        Some(encrypted_auth_config),
        None,
        None,
        None,
        expires_at_unix_secs,
        proxy,
        grok_oauth_catalog_key_fingerprint(provider_type, auth_config),
    )
    .map_err(|err| GatewayError::Internal(err.to_string()))?;
    record.internal_priority = 50;
    record.cache_ttl_minutes = 5;
    record.max_probe_interval_minutes = 32;
    record.request_count = Some(0);
    record.success_count = Some(0);
    record.error_count = Some(0);
    record.total_response_time_ms = Some(0);
    record.health_by_format = Some(json!({}));
    record.circuit_breaker_by_format = Some(json!({}));
    record.created_at_unix_ms = Some(now_unix_secs);
    record.updated_at_unix_secs = Some(now_unix_secs);
    let created = state.create_provider_catalog_key(&record).await?;
    if let Some(key) = created.as_ref() {
        let _ = state
            .app()
            .invalidate_local_oauth_refresh_entry(&key.id)
            .await;
        seed_provider_oauth_pool_score(state, provider_id, key, now_unix_secs).await;
    }
    Ok(created)
}

pub(crate) async fn update_existing_provider_oauth_catalog_key(
    state: &AdminAppState<'_>,
    existing_key: &StoredProviderCatalogKey,
    provider_type: &str,
    access_token: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    api_formats: &[String],
    proxy: Option<serde_json::Value>,
    expires_at_unix_secs: Option<u64>,
) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
    let Some(encrypted_api_key) = state.encrypt_catalog_secret_with_fallbacks(access_token) else {
        return Ok(None);
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(encrypted_auth_config) =
        state.encrypt_catalog_secret_with_fallbacks(&auth_config_json)
    else {
        return Ok(None);
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut updated = existing_key.clone();
    updated.is_active = true;
    updated.encrypted_api_key = Some(encrypted_api_key);
    updated.encrypted_auth_config = Some(encrypted_auth_config);
    updated.api_formats = provider_oauth_catalog_key_api_formats(provider_type, api_formats);
    updated.expires_at_unix_secs = expires_at_unix_secs;
    updated.oauth_invalid_at_unix_secs = None;
    updated.oauth_invalid_reason = None;
    if updated.fingerprint.is_none() {
        updated.fingerprint = grok_oauth_catalog_key_fingerprint(provider_type, auth_config);
    }
    if let Some(proxy) = proxy {
        updated.proxy = Some(proxy);
    }
    updated.updated_at_unix_secs = Some(now_unix_secs);
    if state.update_provider_catalog_key(&updated).await?.is_none() {
        return Ok(None);
    }
    if !state
        .clear_provider_catalog_key_oauth_invalid_marker(&updated.id)
        .await?
    {
        return Ok(None);
    }
    let persisted = state
        .reset_provider_catalog_key_recovery_state(&updated.id)
        .await?;
    if let Some(key) = persisted.as_ref() {
        let _ = state
            .app()
            .invalidate_local_oauth_refresh_entry(&key.id)
            .await;
        seed_provider_oauth_pool_score(state, &existing_key.provider_id, key, now_unix_secs).await;
    }
    Ok(persisted)
}

pub(super) async fn seed_provider_oauth_pool_score(
    state: &AdminAppState<'_>,
    provider_id: &str,
    key: &StoredProviderCatalogKey,
    now_unix_secs: u64,
) {
    let provider_id = provider_id.to_string();
    let provider = match state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await
    {
        Ok(mut providers) => providers.pop(),
        Err(err) => {
            tracing::debug!(
                provider_id = %provider_id,
                key_id = %key.id,
                error = ?err,
                "gateway provider oauth provisioning: failed to read provider for pool score seed"
            );
            return;
        }
    };
    let Some(provider) = provider else {
        return;
    };
    let Some(pool_config) = admin_provider_pool_config(&provider) else {
        return;
    };
    if !key.is_active || key.provider_id != provider.id {
        return;
    }

    let identity = PoolMemberIdentity::provider_api_key(provider.id.clone(), key.id.clone());
    let scope = provider_key_pool_score_scope();
    let score_id = provider_key_pool_score_id(&identity, &scope);
    let existing = match state
        .app()
        .data
        .get_pool_member_scores_by_ids(&GetPoolMemberScoresByIdsQuery {
            ids: vec![score_id],
        })
        .await
    {
        Ok(mut scores) => scores.pop(),
        Err(err) => {
            tracing::debug!(
                provider_id = %provider_id,
                key_id = %key.id,
                error = ?err,
                "gateway provider oauth provisioning: failed to read existing pool score"
            );
            return;
        }
    };
    let upsert = build_provider_key_pool_score_upsert(
        key,
        provider.provider_type.as_str(),
        existing.as_ref(),
        now_unix_secs,
        pool_config.score_rules,
    );
    if let Err(err) = state.app().data.upsert_pool_member_score(upsert).await {
        tracing::debug!(
            provider_id = %provider_id,
            key_id = %key.id,
            error = ?err,
            "gateway provider oauth provisioning: failed to refresh pool score row"
        );
    }
}

fn provider_oauth_catalog_key_api_formats(
    provider_type: &str,
    api_formats: &[String],
) -> Option<serde_json::Value> {
    if provider_type_is_fixed(provider_type) {
        None
    } else {
        Some(json!(api_formats))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        grok_oauth_catalog_key_fingerprint, provider_oauth_token_payload_expires_at_unix_secs,
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn sample_unsigned_jwt(payload: serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn token_payload_expiry_uses_relative_expires_in_aliases() {
        let payload = json!({
            "access_token": "opaque-token",
            "expiresIn": 120,
        });

        assert_eq!(
            provider_oauth_token_payload_expires_at_unix_secs(&payload, 1_000),
            Some(1_120)
        );
    }

    #[test]
    fn token_payload_expiry_uses_absolute_expires_at_aliases() {
        let payload = json!({
            "access_token": "opaque-token",
            "expiresAt": 4_102_444_800u64,
        });

        assert_eq!(
            provider_oauth_token_payload_expires_at_unix_secs(&payload, 1_000),
            Some(4_102_444_800)
        );
    }

    #[test]
    fn token_payload_expiry_falls_back_to_access_token_exp_claim() {
        let access_token = sample_unsigned_jwt(json!({
            "exp": 2_000_000_000u64,
        }));
        let payload = json!({
            "access_token": access_token,
        });

        assert_eq!(
            provider_oauth_token_payload_expires_at_unix_secs(&payload, 1_000),
            Some(2_000_000_000)
        );
    }

    #[test]
    fn grok_oauth_catalog_key_fingerprint_uses_browser_wreq_profile() {
        let auth_config = json!({
            "sso_token": "abc",
            "browser_profile": "chrome-137",
        });
        let auth_config = auth_config.as_object().expect("object");

        let fingerprint = grok_oauth_catalog_key_fingerprint("grok", auth_config)
            .expect("fingerprint should resolve");

        assert_eq!(
            fingerprint["transport_profile"]["profile_id"],
            json!("chrome137")
        );
        assert_eq!(
            fingerprint["transport_profile"]["backend"],
            json!("browser_wreq")
        );
        assert_eq!(
            fingerprint["transport_profile"]["extra"]["browser_profile"],
            json!("chrome137")
        );
    }

    #[test]
    fn grok_oauth_catalog_key_fingerprint_infers_profile_from_user_agent() {
        let auth_config = json!({
            "sso_token": "abc",
            "user_agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36",
        });
        let auth_config = auth_config.as_object().expect("object");

        let fingerprint = grok_oauth_catalog_key_fingerprint("grok", auth_config)
            .expect("fingerprint should resolve");

        assert_eq!(
            fingerprint["transport_profile"]["profile_id"],
            json!("chrome137")
        );
        assert_eq!(
            fingerprint["transport_profile"]["extra"]["browser_profile"],
            json!("chrome137")
        );
    }

    #[test]
    fn grok_oauth_catalog_key_fingerprint_ignores_non_grok_providers() {
        let auth_config = json!({
            "browser_profile": "chrome136",
        });
        let auth_config = auth_config.as_object().expect("object");

        assert!(grok_oauth_catalog_key_fingerprint("openai", auth_config).is_none());
    }
}
