use super::super::quota::shared::persist_provider_quota_refresh_state;
use super::super::refresh::{
    build_internal_control_error_response, merge_provider_oauth_refresh_failure_reason,
    normalize_provider_oauth_refresh_error_message, provider_oauth_runtime_endpoint_for_provider,
    refresh_provider_oauth_account_state_after_update,
};
use super::super::state::is_fixed_provider_type_for_provider_oauth;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_refresh_key_id;
use crate::handlers::admin::provider::shared::payloads::{
    OAUTH_ACCOUNT_BLOCK_PREFIX, OAUTH_REFRESH_FAILED_PREFIX,
};
use crate::handlers::admin::shared::decrypt_catalog_secret_with_fallbacks;
use crate::{AppState, GatewayError};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn handle_admin_provider_oauth_refresh_key(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Result<Response<Body>, GatewayError> {
    let Some(key_id) = admin_provider_oauth_refresh_key_id(&request_context.request_path) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
    let Some(key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
    if !key.auth_type.eq_ignore_ascii_case("oauth") {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Key 不是 oauth 认证类型",
        ));
    }

    let Some(encrypted_auth_config) = key.encrypted_auth_config.as_deref() else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "缺少 auth_config，无法 refresh",
        ));
    };
    let Some(decrypted_auth_config) =
        decrypt_catalog_secret_with_fallbacks(state.encryption_key(), encrypted_auth_config)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth encryption unavailable",
        ));
    };
    let parsed_auth_config = serde_json::from_str::<serde_json::Value>(&decrypted_auth_config)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let has_refresh_token = parsed_auth_config
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if !has_refresh_token {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "缺少 refresh_token，需要重新授权",
        ));
    }

    let provider_id = key.provider_id.clone();
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?;
    let Some(endpoint) = provider_oauth_runtime_endpoint_for_provider(&provider_type, endpoints)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "找不到有效端点，无法 refresh",
        ));
    };
    let Some(transport) = state
        .read_provider_transport_snapshot(&provider_id, &endpoint.id, &key_id)
        .await?
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Provider transport snapshot unavailable",
        ));
    };

    match state.force_local_oauth_refresh_entry(&transport).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "缺少 refresh_token，需要重新授权",
            ));
        }
        Err(crate::provider_transport::LocalOAuthRefreshError::HttpStatus {
            status_code,
            body_excerpt,
            ..
        }) => {
            let error_reason = normalize_provider_oauth_refresh_error_message(
                Some(status_code),
                Some(body_excerpt.as_str()),
            );
            if matches!(status_code, 400 | 401 | 403) {
                let merged_reason = merge_provider_oauth_refresh_failure_reason(
                    key.oauth_invalid_reason.as_deref(),
                    format!(
                        "{OAUTH_REFRESH_FAILED_PREFIX}Token 续期失败 ({status_code}): {error_reason}"
                    )
                    .as_str(),
                );
                if let Some(merged_reason) = merged_reason {
                    let now_unix_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    let _ = persist_provider_quota_refresh_state(
                        state,
                        &key_id,
                        None,
                        Some(now_unix_secs),
                        Some(merged_reason),
                        None,
                    )
                    .await?;
                }
            }
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                format!("Token 刷新失败：{error_reason}"),
            ));
        }
        Err(crate::provider_transport::LocalOAuthRefreshError::Transport { source, .. }) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                format!("Token 刷新失败：{}", source),
            ));
        }
        Err(crate::provider_transport::LocalOAuthRefreshError::InvalidResponse {
            message, ..
        }) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                format!("Token 刷新失败：{message}"),
            ));
        }
    }

    if !key
        .oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| value.starts_with(OAUTH_ACCOUNT_BLOCK_PREFIX))
    {
        let _ = state
            .clear_provider_catalog_key_oauth_invalid_marker(&key_id)
            .await?;
    }

    let refreshed_key = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
        .unwrap_or(key);
    let refreshed_auth_config = refreshed_key
        .encrypted_auth_config
        .as_deref()
        .and_then(|ciphertext| {
            decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
        })
        .and_then(|plaintext| serde_json::from_str::<serde_json::Value>(&plaintext).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let (account_state_recheck_attempted, account_state_recheck_error) =
        refresh_provider_oauth_account_state_after_update(state, &provider, &key_id).await?;

    Ok(Json(json!({
        "provider_type": provider_type,
        "expires_at": refreshed_auth_config.get("expires_at").cloned().unwrap_or(serde_json::Value::Null),
        "has_refresh_token": refreshed_auth_config
            .get("refresh_token")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()),
        "email": refreshed_auth_config.get("email").cloned().unwrap_or(serde_json::Value::Null),
        "account_state_recheck_attempted": account_state_recheck_attempted,
        "account_state_recheck_error": account_state_recheck_error,
    }))
    .into_response())
}
