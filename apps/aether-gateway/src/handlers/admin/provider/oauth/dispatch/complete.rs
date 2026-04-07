use super::super::quota::codex::refresh_codex_provider_quota_locally;
use super::super::refresh::{
    build_internal_control_error_response, build_provider_oauth_auth_config_from_token_payload,
    create_provider_oauth_catalog_key, find_duplicate_provider_oauth_key,
    provider_oauth_active_api_formats, provider_oauth_key_proxy_value,
    refresh_provider_oauth_account_state_after_update, update_existing_provider_oauth_catalog_key,
};
use super::super::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    consume_provider_oauth_state, enrich_admin_provider_oauth_auth_config,
    exchange_admin_provider_oauth_code, is_fixed_provider_type_for_provider_oauth,
    json_non_empty_string, json_u64_value, parse_provider_oauth_callback_params,
};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_oauth_complete_key_id, admin_provider_oauth_complete_provider_id,
};
use crate::handlers::admin::shared::encrypt_catalog_secret_with_fallbacks;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn handle_admin_provider_oauth_complete_key(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(key_id) = admin_provider_oauth_complete_key_id(&request_context.request_path) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
    let Some(request_body) = request_body else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "请求体必须是合法的 JSON 对象",
        ));
    };
    let raw_payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ));
        }
    };
    let callback_url = raw_payload
        .get("callback_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "callback_url 缺少 code/state",
            )
        });
    let callback_url = match callback_url {
        Ok(callback_url) => callback_url,
        Err(response) => return Ok(response),
    };
    let params = parse_provider_oauth_callback_params(callback_url);
    let Some(code) = params
        .get("code")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "callback_url 缺少 code/state",
        ));
    };
    let Some(state_nonce) = params
        .get("state")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "callback_url 缺少 code/state",
        ));
    };

    let state_data = match consume_provider_oauth_state(state, state_nonce).await {
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
    if !key.auth_type.eq_ignore_ascii_case("oauth") {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Key 不是 oauth 认证类型",
        ));
    }
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

    let token_payload = match exchange_admin_provider_oauth_code(
        state,
        template,
        code,
        state_nonce,
        state_data.pkce_verifier.as_deref(),
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

    let Some(encrypted_api_key) = encrypt_catalog_secret_with_fallbacks(state, &access_token)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth encryption unavailable",
        ));
    };
    let auth_config_json = serde_json::to_string(&serde_json::Value::Object(auth_config.clone()))
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let Some(encrypted_auth_config) =
        encrypt_catalog_secret_with_fallbacks(state, &auth_config_json)
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
                && endpoint
                    .api_format
                    .trim()
                    .eq_ignore_ascii_case("openai:cli")
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

pub(super) async fn handle_admin_provider_oauth_complete_provider(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(provider_id) =
        admin_provider_oauth_complete_provider_id(&request_context.request_path)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let Some(request_body) = request_body else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "请求体必须是合法的 JSON 对象",
        ));
    };
    let raw_payload = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ));
        }
    };
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(callback_url) = raw_payload
        .get("callback_url")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "callback_url 缺少 code/state",
        ));
    };
    let name = raw_payload
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let proxy_node_id = raw_payload
        .get("proxy_node_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let params = parse_provider_oauth_callback_params(callback_url);
    let Some(code) = params
        .get("code")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "callback_url 缺少 code/state",
        ));
    };
    let Some(state_nonce) = params
        .get("state")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "callback_url 缺少 code/state",
        ));
    };

    let state_data = match consume_provider_oauth_state(state, state_nonce).await {
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
    if !state_data.key_id.trim().is_empty() || state_data.provider_id != provider_id {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "state 无效或已过期",
        ));
    }

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
    if provider_type == "kiro" {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Kiro 不支持 OAuth 授权，请使用导入授权。",
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

    let token_payload = match exchange_admin_provider_oauth_code(
        state,
        template,
        code,
        state_nonce,
        state_data.pkce_verifier.as_deref(),
    )
    .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let (auth_config, access_token, refresh_token, expires_at) =
        build_provider_oauth_auth_config_from_token_payload(&provider_type, &token_payload);
    let Some(access_token) = access_token else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token exchange 返回缺少 access_token",
        ));
    };

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?;
    let api_formats = provider_oauth_active_api_formats(&endpoints);
    let key_proxy = provider_oauth_key_proxy_value(proxy_node_id.as_deref());
    let duplicate =
        match find_duplicate_provider_oauth_key(state, &provider_id, &auth_config, None).await {
            Ok(duplicate) => duplicate,
            Err(detail) => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    detail,
                ));
            }
        };

    let replaced = duplicate.is_some();
    let persisted_key = if let Some(existing_key) = duplicate {
        match update_existing_provider_oauth_catalog_key(
            state,
            &existing_key,
            &access_token,
            &auth_config,
            key_proxy.clone(),
            expires_at,
        )
        .await?
        {
            Some(key) => key,
            None => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    } else {
        let name = name
            .or_else(|| {
                auth_config
                    .get("email")
                    .and_then(serde_json::Value::as_str)
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
        match create_provider_oauth_catalog_key(
            state,
            &provider_id,
            &name,
            &access_token,
            &auth_config,
            &api_formats,
            key_proxy.clone(),
            expires_at,
        )
        .await?
        {
            Some(key) => key,
            None => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    "provider oauth write unavailable",
                ));
            }
        }
    };

    let _ = refresh_provider_oauth_account_state_after_update(state, &provider, &persisted_key.id)
        .await;

    Ok(Json(json!({
        "key_id": persisted_key.id,
        "provider_type": provider_type,
        "expires_at": expires_at,
        "has_refresh_token": refresh_token.is_some(),
        "email": auth_config.get("email").cloned().unwrap_or(serde_json::Value::Null),
        "replaced": replaced,
    }))
    .into_response())
}
