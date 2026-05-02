use super::super::duplicates::find_duplicate_provider_oauth_key;
use super::super::errors::build_internal_control_error_response;
use super::super::provisioning::{
    build_provider_oauth_auth_config_from_token_payload, create_provider_oauth_catalog_key,
    provider_oauth_active_api_formats, provider_oauth_key_proxy_value,
    update_existing_provider_oauth_catalog_key,
};
use super::super::runtime::{
    provider_oauth_runtime_endpoint_for_provider,
    spawn_provider_oauth_account_state_refresh_after_update,
};
use super::super::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    exchange_admin_provider_oauth_refresh_token, is_fixed_provider_type_for_provider_oauth,
    json_u64_value,
};
use super::token_import::{
    build_codex_access_token_import_auth_config, normalize_single_import_tokens,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_import_provider_id;
use crate::handlers::admin::request::{
    AdminAppState, AdminProviderOAuthTemplate, AdminRequestContext,
};
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

struct AdminProviderOAuthSingleImportTokens {
    access_token: String,
    auth_config: serde_json::Map<String, serde_json::Value>,
    expires_at: Option<u64>,
}

fn import_payload_string(
    payload: &serde_json::Map<String, serde_json::Value>,
    snake_case: &str,
    camel_case: &str,
) -> Option<String> {
    payload
        .get(snake_case)
        .or_else(|| payload.get(camel_case))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn import_payload_u64(
    payload: &serde_json::Map<String, serde_json::Value>,
    snake_case: &str,
    camel_case: &str,
) -> Option<u64> {
    json_u64_value(payload.get(snake_case).or_else(|| payload.get(camel_case)))
}

async fn resolve_admin_provider_oauth_single_import_tokens(
    state: &AdminAppState<'_>,
    template: AdminProviderOAuthTemplate,
    provider_type: &str,
    refresh_token: Option<&str>,
    access_token: Option<&str>,
    imported_expires_at: Option<u64>,
    request_proxy: Option<ProxySnapshot>,
) -> Result<AdminProviderOAuthSingleImportTokens, Response<Body>> {
    if let Some(refresh_token) = refresh_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let token_payload = match state
            .exchange_admin_provider_oauth_refresh_token(
                template,
                refresh_token,
                request_proxy.clone(),
            )
            .await
        {
            Ok(payload) => payload,
            Err(response) => {
                if provider_type.eq_ignore_ascii_case("codex") {
                    if let Some(access_token) = access_token
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        let (auth_config, expires_at) = build_codex_access_token_import_auth_config(
                            access_token,
                            Some(refresh_token),
                            imported_expires_at,
                            Some("Refresh Token 验证失败，已回退为 Access Token 导入"),
                        );
                        return Ok(AdminProviderOAuthSingleImportTokens {
                            access_token: access_token.to_string(),
                            auth_config,
                            expires_at,
                        });
                    }
                }
                return Err(response);
            }
        };

        let (mut auth_config, access_token, returned_refresh_token, expires_at) =
            build_provider_oauth_auth_config_from_token_payload(provider_type, &token_payload);
        let Some(access_token) = access_token else {
            return Err(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "token refresh 返回缺少 access_token",
            ));
        };
        let refresh_token = returned_refresh_token
            .or_else(|| Some(refresh_token.to_string()))
            .filter(|value| !value.trim().is_empty());
        if let Some(refresh_token) = refresh_token.as_ref() {
            auth_config.insert("refresh_token".to_string(), json!(refresh_token));
        }
        return Ok(AdminProviderOAuthSingleImportTokens {
            access_token,
            auth_config,
            expires_at,
        });
    }

    let Some(access_token) = access_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Refresh Token 或 Access Token 不能为空",
        ));
    };
    if !provider_type.eq_ignore_ascii_case("codex") {
        return Err(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Access Token 导入仅支持 Codex Provider",
        ));
    }

    let (auth_config, expires_at) =
        build_codex_access_token_import_auth_config(access_token, None, imported_expires_at, None);
    Ok(AdminProviderOAuthSingleImportTokens {
        access_token: access_token.to_string(),
        auth_config,
        expires_at,
    })
}

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
    let refresh_token_input = import_payload_string(&raw_payload, "refresh_token", "refreshToken");
    let access_token_input = import_payload_string(&raw_payload, "access_token", "accessToken");
    let imported_expires_at = import_payload_u64(&raw_payload, "expires_at", "expiresAt");
    let (refresh_token_input, access_token_input) = normalize_single_import_tokens(
        refresh_token_input.as_deref(),
        access_token_input.as_deref(),
    );
    if refresh_token_input.is_none() && access_token_input.is_none() {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Refresh Token 或 Access Token 不能为空",
        ));
    }
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
    let key_proxy = provider_oauth_key_proxy_value(proxy_node_id.as_deref());

    let resolved_import = match resolve_admin_provider_oauth_single_import_tokens(
        state,
        template,
        &provider_type,
        refresh_token_input.as_deref(),
        access_token_input.as_deref(),
        imported_expires_at,
        request_proxy.clone(),
    )
    .await
    {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    let AdminProviderOAuthSingleImportTokens {
        access_token,
        auth_config,
        expires_at,
    } = resolved_import;
    let has_refresh_token = auth_config
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());

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
        match state
            .create_provider_oauth_catalog_key(
                &provider_id,
                &provider_type,
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
        "has_refresh_token": has_refresh_token,
        "temporary": auth_config
            .get("access_token_import_temporary")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        "email": auth_config.get("email").cloned().unwrap_or(serde_json::Value::Null),
        "replaced": replaced,
    }))
    .into_response())
}
