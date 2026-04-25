use super::super::duplicates::find_duplicate_provider_oauth_key;
use super::super::errors::build_internal_control_error_response;
use super::super::provisioning::{
    build_provider_oauth_auth_config_from_token_payload, create_provider_oauth_catalog_key,
    provider_oauth_active_api_formats, update_existing_provider_oauth_catalog_key,
};
use super::super::runtime::{
    provider_oauth_runtime_endpoint_for_provider,
    spawn_provider_oauth_account_state_refresh_after_update,
};
use super::super::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    exchange_admin_provider_oauth_refresh_token, is_fixed_provider_type_for_provider_oauth,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_import_provider_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn handle_admin_provider_oauth_import_refresh_token(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&axum::body::Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) = admin_provider_oauth_import_provider_id(request_context.path()) else {
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
    let Some(refresh_token_input) = raw_payload
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Refresh Token 不能为空",
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
            "Kiro 不支持单条 Refresh Token 导入，请使用批量导入或设备授权。",
        ));
    }
    let Some(template) = admin_provider_oauth_template(&provider_type) else {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    };
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?;
    let runtime_endpoint = provider_oauth_runtime_endpoint_for_provider(&provider_type, &endpoints);
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            proxy_node_id.as_deref(),
            &[
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;

    let token_payload = match state
        .exchange_admin_provider_oauth_refresh_token(
            template,
            refresh_token_input,
            request_proxy.clone(),
        )
        .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let (mut auth_config, access_token, returned_refresh_token, expires_at) =
        build_provider_oauth_auth_config_from_token_payload(&provider_type, &token_payload);
    let Some(access_token) = access_token else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "token refresh 返回缺少 access_token",
        ));
    };
    let refresh_token = returned_refresh_token
        .or_else(|| Some(refresh_token_input.to_string()))
        .filter(|value| !value.trim().is_empty());
    if let Some(refresh_token) = refresh_token.as_ref() {
        auth_config.insert("refresh_token".to_string(), json!(refresh_token));
    }

    let api_formats = provider_oauth_active_api_formats(&endpoints);
    let duplicate = match state
        .find_duplicate_provider_oauth_key(&provider_id, &auth_config, None)
        .await
    {
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
        match state
            .update_existing_provider_oauth_catalog_key(
                &existing_key,
                &provider_type,
                &access_token,
                &auth_config,
                &api_formats,
                None,
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
        match state
            .create_provider_oauth_catalog_key(
                &provider_id,
                &provider_type,
                &name,
                &access_token,
                &auth_config,
                &api_formats,
                None,
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

    spawn_provider_oauth_account_state_refresh_after_update(
        state.cloned_app(),
        provider.clone(),
        persisted_key.id.clone(),
        request_proxy.clone(),
    );

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
