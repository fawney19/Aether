use super::duplicates::{
    acquire_claude_oauth_account_lock, acquire_codex_oauth_account_locks,
    release_provider_oauth_account_locks,
};
use super::errors::build_internal_control_error_response;
use super::runtime::spawn_provider_oauth_account_state_refresh_after_update;
use super::state::{
    decode_jwt_claims, enrich_admin_provider_oauth_auth_config, json_non_empty_string,
    json_u64_value,
};
use crate::ai_serving::{
    build_provider_key_pool_score_upsert, provider_key_pool_score_id, provider_key_pool_score_scope,
};
use crate::handlers::admin::admin_provider_pool_config;
use crate::handlers::admin::provider::write::keys::build_provider_catalog_key_admin_cas_update;
use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_active_api_formats;
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::pool_scores::{
    GetPoolMemberScoresByIdsQuery, PoolMemberIdentity,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_provider_transport::{
    grok_browser_transport_fingerprint_from_auth_config, provider_types::provider_type_is_fixed,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
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

pub(crate) async fn provision_provider_oauth_token_payload_for_provider(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    token_payload: &Value,
    requested_name: Option<String>,
    key_proxy: Option<Value>,
    request_proxy: Option<ProxySnapshot>,
    lock_operation: &'static str,
) -> Result<Response<Body>, GatewayError> {
    let provider_id = provider.id.clone();
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    let (auth_config, access_token, refresh_token, expires_at) =
        build_provider_oauth_auth_config_from_token_payload(&provider_type, token_payload);
    let Some(access_token) = access_token else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 返回缺少 access_token",
        ));
    };

    let api_formats = provider_oauth_active_api_formats(endpoints);
    let oauth_account_leases = if provider_type == "codex" {
        match acquire_codex_oauth_account_locks(state, &provider_id, &auth_config, lock_operation)
            .await
        {
            Ok(leases) => leases,
            Err(error) => {
                return Ok(build_internal_control_error_response(
                    error.status_code(),
                    error.detail(),
                ));
            }
        }
    } else if provider_type == "claude_code" {
        match acquire_claude_oauth_account_lock(state, &provider_id, &auth_config, lock_operation)
            .await
        {
            Ok(leases) => leases,
            Err(error) => {
                return Ok(build_internal_control_error_response(
                    error.status_code(),
                    error.detail(),
                ));
            }
        }
    } else {
        Vec::new()
    };
    let duplicate = match state
        .find_duplicate_provider_oauth_key(&provider_id, &auth_config, None)
        .await
    {
        Ok(duplicate) => duplicate,
        Err(detail) => {
            release_provider_oauth_account_locks(state, oauth_account_leases).await;
            return Ok(build_internal_control_error_response(
                if provider_type == "codex" {
                    http::StatusCode::CONFLICT
                } else {
                    http::StatusCode::BAD_REQUEST
                },
                detail,
            ));
        }
    };

    let replaced = duplicate.is_some();
    let persisted_key = if let Some(existing_key) = duplicate {
        match state
            .update_existing_provider_oauth_catalog_key(
                &existing_key,
                &provider_type,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy.clone(),
                expires_at,
            )
            .await
        {
            Err(error) => {
                release_provider_oauth_account_locks(state, oauth_account_leases).await;
                return Err(error);
            }
            Ok(Some(key)) => key,
            Ok(None) => {
                release_provider_oauth_account_locks(state, oauth_account_leases).await;
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    } else {
        let name = requested_name
            .or_else(|| {
                auth_config
                    .get("email")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| {
                format!(
                    "账号_{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0)
                )
            });
        match state
            .create_provider_oauth_catalog_key(
                &provider_id,
                &provider_type,
                &name,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy,
                expires_at,
            )
            .await
        {
            Err(error) => {
                release_provider_oauth_account_locks(state, oauth_account_leases).await;
                return Err(error);
            }
            Ok(Some(key)) => key,
            Ok(None) => {
                release_provider_oauth_account_locks(state, oauth_account_leases).await;
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    };
    release_provider_oauth_account_locks(state, oauth_account_leases).await;

    spawn_provider_oauth_account_state_refresh_after_update(
        state.cloned_app(),
        provider.clone(),
        persisted_key.id.clone(),
        request_proxy,
    );

    Ok(Json(json!({
        "key_id": persisted_key.id,
        "provider_type": provider_type,
        "expires_at": expires_at,
        "has_refresh_token": refresh_token.is_some(),
        "temporary": refresh_token.is_none(),
        "email": auth_config.get("email").cloned().unwrap_or(Value::Null),
        "replaced": replaced,
    }))
    .into_response())
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

pub(crate) fn rotate_codex_credential_generation(
    key: &mut StoredProviderCatalogKey,
    provider_type: &str,
) {
    if !provider_type.trim().eq_ignore_ascii_case("codex") {
        return;
    }

    let mut upstream_metadata = key
        .upstream_metadata
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    upstream_metadata.insert(
        "codex".to_string(),
        json!({
            aether_admin::provider::quota::CODEX_CREDENTIAL_GENERATION_KEY:
                Uuid::now_v7().to_string(),
        }),
    );
    key.upstream_metadata = Some(Value::Object(upstream_metadata));

    if let Some(mut status_snapshot) = key
        .status_snapshot
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
    {
        status_snapshot.insert("quota".to_string(), Value::Null);
        key.status_snapshot = Some(Value::Object(status_snapshot));
    }
}

pub(crate) fn ensure_codex_credential_generation_rotated(
    key: &mut StoredProviderCatalogKey,
    provider_type: &str,
    previous_generation: Option<&str>,
) {
    if !provider_type.trim().eq_ignore_ascii_case("codex") {
        return;
    }

    let current_generation = key
        .upstream_metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("codex"))
        .and_then(|codex| aether_admin::provider::quota::codex_credential_generation(Some(codex)));
    let already_rotated = current_generation.is_some() && current_generation != previous_generation;
    if !already_rotated {
        rotate_codex_credential_generation(key, provider_type);
    }
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
    let key_id = Uuid::new_v4().to_string();
    let Ok(encrypted_api_key) =
        state
            .app()
            .seal_provider_catalog_key_api_key(provider_id, &key_id, access_token)
    else {
        return Ok(None);
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Ok(encrypted_auth_config) =
        state
            .app()
            .seal_provider_catalog_key_auth_config(provider_id, &key_id, &auth_config_json)
    else {
        return Ok(None);
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut record = StoredProviderCatalogKey::new(
        key_id,
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
    rotate_codex_credential_generation(&mut record, provider_type);
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
    let Ok(encrypted_api_key) = state.app().seal_provider_catalog_key_api_key(
        &existing_key.provider_id,
        &existing_key.id,
        access_token,
    ) else {
        return Ok(None);
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Ok(encrypted_auth_config) = state.app().seal_provider_catalog_key_auth_config(
        &existing_key.provider_id,
        &existing_key.id,
        &auth_config_json,
    ) else {
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
    rotate_codex_credential_generation(&mut updated, provider_type);
    let admin_update =
        build_provider_catalog_key_admin_cas_update(existing_key, updated.clone(), provider_type);
    if !state
        .compare_and_update_provider_catalog_key_admin_state(&admin_update)
        .await?
    {
        return Ok(None);
    }
    let persisted = state
        .reset_provider_catalog_key_recovery_state_fenced(
            &updated.id,
            updated
                .encrypted_auth_config
                .as_deref()
                .expect("OAuth update always supplies encrypted auth_config"),
        )
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
        Err(_) => {
            tracing::debug!(
                provider_id = %provider_id,
                key_id = %key.id,
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
        Err(_) => {
            tracing::debug!(
                provider_id = %provider_id,
                key_id = %key.id,
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
    if state
        .app()
        .data
        .upsert_pool_member_score(upsert)
        .await
        .is_err()
    {
        tracing::debug!(
            provider_id = %provider_id,
            key_id = %key.id,
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
        ensure_codex_credential_generation_rotated, grok_oauth_catalog_key_fingerprint,
        provider_oauth_token_payload_expires_at_unix_secs, rotate_codex_credential_generation,
    };
    use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::{json, Value};

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

    #[test]
    fn codex_credential_rotation_replaces_quota_namespace_and_preserves_unrelated_state() {
        let mut key = StoredProviderCatalogKey::new(
            "key".to_string(),
            "provider".to_string(),
            "Codex".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.upstream_metadata = Some(json!({
            "codex": {
                "credential_generation": "old-generation",
                "primary_used_percent": 75.0,
            },
            "unrelated": {"preserved": true},
        }));
        key.status_snapshot = Some(json!({
            "oauth": {"status": "valid"},
            "quota": {"used_ratio": 0.75},
        }));

        rotate_codex_credential_generation(&mut key, "codex");

        let codex = key
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("codex"))
            .and_then(Value::as_object)
            .expect("codex namespace should exist");
        assert_eq!(codex.len(), 1);
        assert_ne!(
            codex
                .get(aether_admin::provider::quota::CODEX_CREDENTIAL_GENERATION_KEY)
                .and_then(Value::as_str),
            Some("old-generation")
        );
        assert_eq!(
            key.upstream_metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/unrelated/preserved")),
            Some(&json!(true))
        );
        assert_eq!(
            key.status_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.get("quota")),
            Some(&Value::Null)
        );
        assert_eq!(
            key.status_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pointer("/oauth/status")),
            Some(&json!("valid"))
        );
    }

    #[test]
    fn codex_credential_rotation_ensure_does_not_rotate_twice_in_one_write() {
        let mut key = StoredProviderCatalogKey::new(
            "key".to_string(),
            "provider".to_string(),
            "Codex".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build");
        key.upstream_metadata = Some(json!({
            "codex": {"credential_generation": "generation-before-write"}
        }));

        rotate_codex_credential_generation(&mut key, "codex");
        let builder_generation = key
            .upstream_metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/codex/credential_generation"))
            .and_then(Value::as_str)
            .expect("builder should rotate the generation")
            .to_string();

        ensure_codex_credential_generation_rotated(
            &mut key,
            "codex",
            Some("generation-before-write"),
        );

        assert_eq!(
            key.upstream_metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/codex/credential_generation"))
                .and_then(Value::as_str),
            Some(builder_generation.as_str())
        );
    }
}
