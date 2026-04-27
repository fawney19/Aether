use super::super::super::errors::build_internal_control_error_response;
use super::super::super::quota::codex::refresh_codex_provider_quota_locally;
use super::super::super::runtime::provider_oauth_runtime_endpoint_for_provider;
use super::super::super::state::{
    admin_provider_oauth_template, enrich_admin_provider_oauth_auth_config,
    is_fixed_provider_type_for_provider_oauth, json_non_empty_string, json_u64_value,
};
use super::shared::{
    parse_admin_provider_oauth_complete_callback, parse_admin_provider_oauth_complete_request_body,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_complete_key_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn handle_admin_provider_oauth_complete_key(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(key_id) = admin_provider_oauth_complete_key_id(request_context.path()) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
    let payload = match parse_admin_provider_oauth_complete_request_body(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let callback = match parse_admin_provider_oauth_complete_callback(&payload.callback_url) {
        Ok(callback) => callback,
        Err(response) => return Ok(response),
    };

    let state_data = match state
        .consume_provider_oauth_state(&callback.state_nonce)
        .await
    {
        Ok(Some(state_data)) => state_data,
        Ok(None) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "state 无效或已过期",
            ));
        }
        Err(_) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth redis unavailable",
            ));
        }
    };
    if state_data.key_id != key_id {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "state 无效或已过期",
        ));
    }

    let key = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next();
    let Some(key) = key else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
    if !state_data.provider_id.trim().is_empty() && state_data.provider_id != key.provider_id {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "state 无效或已过期",
        ));
    }

    let provider_id = key.provider_id.clone();
    let provider = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next();
    let Some(provider) = provider else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !provider_key_is_oauth_managed(&key, provider_type.as_str()) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Key 不是 OAuth 管理账号",
        ));
    }
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }
    if !state_data.provider_type.trim().is_empty()
        && !state_data
            .provider_type
            .eq_ignore_ascii_case(&provider_type)
    {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "state 无效或已过期",
        ));
    }
    let Some(template) = admin_provider_oauth_template(&provider_type) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不支持 OAuth 授权",
        ));
    };
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?;
    let runtime_endpoint = provider_oauth_runtime_endpoint_for_provider(&provider_type, &endpoints);
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            payload.proxy_node_id.as_deref(),
            &[
                key.proxy.as_ref(),
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;

    let token_payload = match state
        .exchange_admin_provider_oauth_code(
            template,
            &callback.code,
            &callback.state_nonce,
            state_data.pkce_verifier.as_deref(),
            request_proxy.clone(),
        )
        .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let Some(access_token) = json_non_empty_string(token_payload.get("access_token")) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 返回缺少 access_token",
        ));
    };
    let refresh_token = json_non_empty_string(token_payload.get("refresh_token"));
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let expires_at = json_u64_value(token_payload.get("expires_in"))
        .map(|expires_in| now_unix_secs.saturating_add(expires_in));

    let mut auth_config = serde_json::Map::new();
    auth_config.insert("provider_type".to_string(), json!(provider_type.clone()));
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
    enrich_admin_provider_oauth_auth_config(&provider_type, &mut auth_config, &token_payload);

    let Some(encrypted_api_key) = state.encrypt_catalog_secret_with_fallbacks(&access_token) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth encryption unavailable",
        ));
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(encrypted_auth_config) =
        state.encrypt_catalog_secret_with_fallbacks(&auth_config_json)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth encryption unavailable",
        ));
    };
    let updated = state
        .update_provider_catalog_key_oauth_credentials(
            &key_id,
            &encrypted_api_key,
            Some(&encrypted_auth_config),
            expires_at,
        )
        .await?;
    if !updated {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    }

    let mut account_state_recheck_attempted = false;
    let mut account_state_recheck_error = None::<String>;
    if provider_type == "codex" {
        let endpoints = state
            .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
            .await?;
        if let Some(endpoint) = endpoints.into_iter().find(|endpoint| {
            endpoint.is_active
                && crate::ai_pipeline::is_openai_responses_format(&endpoint.api_format)
        }) {
            let refreshed_key = state
                .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
                .await?
                .into_iter()
                .next()
                .unwrap_or_else(|| key.clone());
            if let Some(result) = refresh_codex_provider_quota_locally(
                state,
                &provider,
                &endpoint,
                vec![refreshed_key],
                request_proxy.clone(),
            )
            .await?
            {
                account_state_recheck_attempted = true;
                let success = result
                    .get("success")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                if success == 0 {
                    account_state_recheck_error = result
                        .get("results")
                        .and_then(serde_json::Value::as_array)
                        .and_then(|results| results.first())
                        .and_then(|value| value.get("message"))
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned);
                }
            }
        }
    }

    Ok(Json(json!({
        "provider_type": provider_type,
        "expires_at": expires_at,
        "has_refresh_token": refresh_token.is_some(),
        "email": auth_config.get("email").cloned().unwrap_or(serde_json::Value::Null),
        "account_state_recheck_attempted": account_state_recheck_attempted,
        "account_state_recheck_error": account_state_recheck_error,
    }))
    .into_response())
}
