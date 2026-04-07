use super::quota::antigravity::refresh_antigravity_provider_quota_locally;
use super::quota::codex::refresh_codex_provider_quota_locally;
use super::quota::kiro::refresh_kiro_provider_quota_locally;
use super::quota::shared::persist_provider_quota_refresh_state;
use super::state::{
    enrich_admin_provider_oauth_auth_config, json_non_empty_string, json_u64_value,
};
use crate::handlers::admin::provider::shared::payloads::{
    OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_EXPIRED_PREFIX, OAUTH_REFRESH_FAILED_PREFIX,
    OAUTH_REQUEST_FAILED_PREFIX,
};
use crate::handlers::admin::shared::{
    decrypt_catalog_secret_with_fallbacks, encrypt_catalog_secret_with_fallbacks,
    parse_catalog_auth_config_json,
};
use crate::{AppState, GatewayError};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) fn build_internal_control_error_response(
    status: http::StatusCode,
    message: impl Into<String>,
) -> Response<Body> {
    (status, Json(json!({ "detail": message.into() }))).into_response()
}

pub(crate) fn normalize_provider_oauth_refresh_error_message(
    status_code: Option<u16>,
    body_excerpt: Option<&str>,
) -> String {
    let mut message = None::<String>;
    let mut error_code = None::<String>;
    let mut error_type = None::<String>;

    if let Some(body_excerpt) = body_excerpt {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body_excerpt) {
            if let Some(object) = value.as_object() {
                if let Some(error_object) =
                    object.get("error").and_then(serde_json::Value::as_object)
                {
                    message = error_object
                        .get("message")
                        .or_else(|| error_object.get("error_description"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned);
                    error_code = error_object
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                    error_type = error_object
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
                if message.is_none() {
                    message = object
                        .get("message")
                        .or_else(|| object.get("error_description"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned);
                }
                if error_code.is_none() {
                    error_code = object
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
                if error_type.is_none() {
                    error_type = object
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| value.to_ascii_lowercase());
                }
            }
        }
    }

    let message = message
        .or_else(|| {
            body_excerpt
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(300).collect::<String>())
        })
        .unwrap_or_default();
    let lowered = message.to_ascii_lowercase();
    let error_code = error_code.unwrap_or_default();
    let error_type = error_type.unwrap_or_default();

    if error_code == "refresh_token_reused"
        || lowered.contains("already been used to generate a new access token")
    {
        return "refresh_token 已被使用并轮换，请重新登录授权".to_string();
    }
    if error_code == "invalid_grant"
        || error_code == "invalid_refresh_token"
        || (lowered.contains("refresh token")
            && ["expired", "revoked", "invalid"]
                .iter()
                .any(|keyword| lowered.contains(keyword)))
    {
        return "refresh_token 无效、已过期或已撤销，请重新登录授权".to_string();
    }
    if error_type == "invalid_request_error" && !message.is_empty() {
        return message;
    }
    if !message.is_empty() {
        return message;
    }
    status_code
        .map(|status_code| format!("HTTP {status_code}"))
        .unwrap_or_else(|| "未知错误".to_string())
}

pub(crate) fn merge_provider_oauth_refresh_failure_reason(
    current_reason: Option<&str>,
    refresh_reason: &str,
) -> Option<String> {
    let current_reason = current_reason.map(str::trim).unwrap_or_default();
    let refresh_reason = refresh_reason.trim();
    if refresh_reason.is_empty() {
        return (!current_reason.is_empty()).then(|| current_reason.to_string());
    }
    if current_reason.is_empty() {
        return Some(refresh_reason.to_string());
    }
    if current_reason.starts_with(OAUTH_EXPIRED_PREFIX) {
        return None;
    }
    if current_reason.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX) {
        if let Some((head, _)) = current_reason.split_once("[REFRESH_FAILED]") {
            return Some(
                format!("{}\n{}", head.trim_end(), refresh_reason)
                    .trim()
                    .to_string(),
            );
        }
        return Some(format!("{current_reason}\n{refresh_reason}"));
    }
    Some(refresh_reason.to_string())
}

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
    let mut formats = Vec::new();
    let mut seen = BTreeSet::new();
    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        let api_format = endpoint.api_format.trim();
        if api_format.is_empty() || !seen.insert(api_format.to_string()) {
            continue;
        }
        formats.push(api_format.to_string());
    }
    formats
}

fn normalize_codex_plan_group_for_provider_oauth(
    plan_type: Option<&serde_json::Value>,
) -> Option<String> {
    let normalized = plan_type
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_ascii_lowercase();
    match normalized.as_str() {
        "free" => Some("free".to_string()),
        "team" | "plus" | "enterprise" => Some("team_plus_enterprise".to_string()),
        _ => None,
    }
}

fn normalize_provider_oauth_identity_value(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn is_codex_provider_oauth_provider_type(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|provider_type| provider_type.eq_ignore_ascii_case("codex"))
}

fn match_codex_provider_oauth_identity(
    new_auth_config: &serde_json::Map<String, serde_json::Value>,
    existing_auth_config: &serde_json::Map<String, serde_json::Value>,
) -> Option<bool> {
    let new_provider_type = new_auth_config.get("provider_type");
    let existing_provider_type = existing_auth_config.get("provider_type");
    if !is_codex_provider_oauth_provider_type(new_provider_type)
        && !is_codex_provider_oauth_provider_type(existing_provider_type)
    {
        return None;
    }

    let new_account_user_id =
        normalize_provider_oauth_identity_value(new_auth_config.get("account_user_id"));
    let existing_account_user_id =
        normalize_provider_oauth_identity_value(existing_auth_config.get("account_user_id"));
    if let (Some(new_account_user_id), Some(existing_account_user_id)) =
        (new_account_user_id, existing_account_user_id)
    {
        return Some(new_account_user_id == existing_account_user_id);
    }

    let new_account_id = normalize_provider_oauth_identity_value(new_auth_config.get("account_id"));
    let existing_account_id =
        normalize_provider_oauth_identity_value(existing_auth_config.get("account_id"));
    let new_user_id = normalize_provider_oauth_identity_value(new_auth_config.get("user_id"));
    let existing_user_id =
        normalize_provider_oauth_identity_value(existing_auth_config.get("user_id"));
    let new_email = normalize_provider_oauth_identity_value(new_auth_config.get("email"));
    let existing_email = normalize_provider_oauth_identity_value(existing_auth_config.get("email"));

    if let (Some(new_account_id), Some(existing_account_id)) =
        (new_account_id.as_deref(), existing_account_id.as_deref())
    {
        if new_account_id != existing_account_id {
            return Some(false);
        }
    }

    if let (
        Some(new_account_id),
        Some(existing_account_id),
        Some(new_user_id),
        Some(existing_user_id),
    ) = (
        new_account_id.as_deref(),
        existing_account_id.as_deref(),
        new_user_id.as_deref(),
        existing_user_id.as_deref(),
    ) {
        return Some(new_account_id == existing_account_id && new_user_id == existing_user_id);
    }

    if let (
        Some(new_account_id),
        Some(existing_account_id),
        Some(new_email),
        Some(existing_email),
    ) = (
        new_account_id.as_deref(),
        existing_account_id.as_deref(),
        new_email.as_deref(),
        existing_email.as_deref(),
    ) {
        return Some(new_account_id == existing_account_id && new_email == existing_email);
    }

    None
}

fn is_codex_cross_plan_group_non_duplicate(
    new_auth_config: &serde_json::Map<String, serde_json::Value>,
    existing_auth_config: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let new_provider_type = new_auth_config.get("provider_type");
    let existing_provider_type = existing_auth_config.get("provider_type");
    if !is_codex_provider_oauth_provider_type(new_provider_type)
        && !is_codex_provider_oauth_provider_type(existing_provider_type)
    {
        return false;
    }

    let new_group = normalize_codex_plan_group_for_provider_oauth(new_auth_config.get("plan_type"));
    let existing_group =
        normalize_codex_plan_group_for_provider_oauth(existing_auth_config.get("plan_type"));
    matches!(
        (new_group.as_deref(), existing_group.as_deref()),
        (Some(left), Some(right)) if left != right
    )
}

pub(crate) async fn find_duplicate_provider_oauth_key(
    state: &AppState,
    provider_id: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    exclude_key_id: Option<&str>,
) -> Result<Option<StoredProviderCatalogKey>, String> {
    let new_email = normalize_provider_oauth_identity_value(auth_config.get("email"));
    let new_user_id = normalize_provider_oauth_identity_value(auth_config.get("user_id"));
    let new_auth_method = normalize_provider_oauth_identity_value(auth_config.get("auth_method"));

    if new_email.is_none() && new_user_id.is_none() {
        return Ok(None);
    }

    let existing_keys = state
        .list_provider_catalog_keys_by_provider_ids(&[provider_id.to_string()])
        .await
        .map_err(|err| format!("{err:?}"))?;

    for existing_key in existing_keys.into_iter().filter(|key| {
        key.auth_type.trim().eq_ignore_ascii_case("oauth")
            && exclude_key_id.is_none_or(|exclude| key.id != exclude)
    }) {
        let Some(existing_auth_config) = parse_catalog_auth_config_json(state, &existing_key)
        else {
            continue;
        };
        let existing_email =
            normalize_provider_oauth_identity_value(existing_auth_config.get("email"));
        let existing_user_id =
            normalize_provider_oauth_identity_value(existing_auth_config.get("user_id"));
        let existing_auth_method =
            normalize_provider_oauth_identity_value(existing_auth_config.get("auth_method"));

        let mut is_duplicate = false;
        let codex_identity_match =
            match_codex_provider_oauth_identity(auth_config, &existing_auth_config);
        if let Some(codex_identity_match) = codex_identity_match {
            is_duplicate = codex_identity_match;
        }

        if codex_identity_match.is_none()
            && !is_duplicate
            && new_user_id.is_some()
            && existing_user_id.is_some()
            && new_user_id == existing_user_id
            && !is_codex_cross_plan_group_non_duplicate(auth_config, &existing_auth_config)
        {
            is_duplicate = true;
        }

        if codex_identity_match.is_none()
            && !is_duplicate
            && new_email.is_some()
            && existing_email.is_some()
            && new_email == existing_email
        {
            let is_kiro = auth_config
                .get("provider_type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("kiro"))
                || existing_auth_config
                    .get("provider_type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("kiro"));
            if is_kiro {
                if new_auth_method.is_some()
                    && existing_auth_method.is_some()
                    && new_auth_method
                        .as_deref()
                        .zip(existing_auth_method.as_deref())
                        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
                {
                    is_duplicate = true;
                }
            } else if !is_codex_cross_plan_group_non_duplicate(auth_config, &existing_auth_config) {
                is_duplicate = true;
            }
        }

        if !is_duplicate {
            continue;
        }
        if !existing_key.is_active {
            return Ok(Some(existing_key));
        }
        let identifier =
            normalize_provider_oauth_identity_value(auth_config.get("account_user_id"))
                .or_else(|| normalize_provider_oauth_identity_value(auth_config.get("account_id")))
                .or_else(|| new_email.clone())
                .or_else(|| new_user_id.clone())
                .unwrap_or_default();
        return Err(format!(
            "该 OAuth 账号 ({identifier}) 已存在于当前 Provider 中（名称: {}）",
            existing_key.name
        ));
    }

    Ok(None)
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
    let expires_at = json_u64_value(token_payload.get("expires_in"))
        .map(|expires_in| now_unix_secs.saturating_add(expires_in));

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

pub(crate) async fn create_provider_oauth_catalog_key(
    state: &AppState,
    provider_id: &str,
    name: &str,
    access_token: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    api_formats: &[String],
    proxy: Option<serde_json::Value>,
    expires_at_unix_secs: Option<u64>,
) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
    let Some(encrypted_api_key) = encrypt_catalog_secret_with_fallbacks(state, access_token) else {
        return Ok(None);
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(encrypted_auth_config) =
        encrypt_catalog_secret_with_fallbacks(state, &auth_config_json)
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
        Some(json!(api_formats)),
        encrypted_api_key,
        Some(encrypted_auth_config),
        None,
        None,
        None,
        expires_at_unix_secs,
        proxy,
        None,
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
    record.created_at_unix_secs = Some(now_unix_secs);
    record.updated_at_unix_secs = Some(now_unix_secs);
    state.create_provider_catalog_key(&record).await
}

pub(crate) async fn update_existing_provider_oauth_catalog_key(
    state: &AppState,
    existing_key: &StoredProviderCatalogKey,
    access_token: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    proxy: Option<serde_json::Value>,
    expires_at_unix_secs: Option<u64>,
) -> Result<Option<StoredProviderCatalogKey>, GatewayError> {
    let Some(encrypted_api_key) = encrypt_catalog_secret_with_fallbacks(state, access_token) else {
        return Ok(None);
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(encrypted_auth_config) =
        encrypt_catalog_secret_with_fallbacks(state, &auth_config_json)
    else {
        return Ok(None);
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut updated = existing_key.clone();
    updated.encrypted_api_key = encrypted_api_key;
    updated.encrypted_auth_config = Some(encrypted_auth_config);
    updated.is_active = true;
    updated.expires_at_unix_secs = expires_at_unix_secs;
    updated.oauth_invalid_at_unix_secs = None;
    updated.oauth_invalid_reason = None;
    updated.health_by_format = Some(json!({}));
    updated.circuit_breaker_by_format = Some(json!({}));
    updated.error_count = Some(0);
    if let Some(proxy) = proxy {
        updated.proxy = Some(proxy);
    }
    updated.updated_at_unix_secs = Some(now_unix_secs);
    state.update_provider_catalog_key(&updated).await
}

pub(crate) fn provider_oauth_runtime_endpoint_for_provider(
    provider_type: &str,
    endpoints: Vec<StoredProviderCatalogEndpoint>,
) -> Option<StoredProviderCatalogEndpoint> {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    match provider_type.as_str() {
        "codex" => endpoints.into_iter().find(|endpoint| {
            endpoint.is_active
                && endpoint
                    .api_format
                    .trim()
                    .eq_ignore_ascii_case("openai:cli")
        }),
        "antigravity" => endpoints.into_iter().find(|endpoint| {
            endpoint.is_active
                && (endpoint
                    .api_format
                    .trim()
                    .eq_ignore_ascii_case("gemini:chat")
                    || endpoint
                        .api_format
                        .trim()
                        .eq_ignore_ascii_case("gemini:cli"))
        }),
        "kiro" => endpoints
            .iter()
            .find(|endpoint| {
                endpoint.is_active
                    && endpoint
                        .api_format
                        .trim()
                        .eq_ignore_ascii_case("claude:cli")
            })
            .cloned()
            .or_else(|| endpoints.into_iter().find(|endpoint| endpoint.is_active)),
        _ => endpoints.into_iter().find(|endpoint| endpoint.is_active),
    }
}

pub(crate) async fn refresh_provider_oauth_account_state_after_update(
    state: &AppState,
    provider: &StoredProviderCatalogProvider,
    key_id: &str,
) -> Result<(bool, Option<String>), GatewayError> {
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !matches!(provider_type.as_str(), "codex" | "kiro" | "antigravity") {
        return Ok((false, None));
    }

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let Some(endpoint) = provider_oauth_runtime_endpoint_for_provider(&provider_type, endpoints)
    else {
        return Ok((false, None));
    };
    let Some(key) = state
        .read_provider_catalog_keys_by_ids(&[key_id.to_string()])
        .await?
        .into_iter()
        .next()
    else {
        return Ok((false, None));
    };

    let payload = match provider_type.as_str() {
        "codex" => {
            refresh_codex_provider_quota_locally(state, provider, &endpoint, vec![key]).await?
        }
        "kiro" => {
            refresh_kiro_provider_quota_locally(state, provider, &endpoint, vec![key]).await?
        }
        "antigravity" => {
            refresh_antigravity_provider_quota_locally(state, provider, &endpoint, vec![key])
                .await?
        }
        _ => None,
    };
    let Some(payload) = payload else {
        return Ok((false, None));
    };
    let success = payload
        .get("success")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let error = if success == 0 {
        payload
            .get("results")
            .and_then(serde_json::Value::as_array)
            .and_then(|results| results.first())
            .and_then(|value| value.get("message"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    } else {
        None
    };
    Ok((true, error))
}
