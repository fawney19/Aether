use super::super::super::runtime::provider_oauth_runtime_endpoint_for_provider;
use super::super::super::state::is_fixed_provider_type_for_provider_oauth;
use super::helpers::{self, RefreshDispatch, RefreshRequestContext};
use super::response;
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_refresh_key_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::provider_key_auth::provider_key_is_oauth_managed;
use crate::GatewayError;
use axum::http;

pub(super) async fn parse_admin_provider_oauth_refresh_request(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<RefreshDispatch<RefreshRequestContext>, GatewayError> {
    let Some(key_id) = admin_provider_oauth_refresh_key_id(request_context.path()) else {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        )));
    };
    let Some(key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        )));
    };
    let Some(encrypted_auth_config) = key.encrypted_auth_config.as_deref() else {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::BAD_REQUEST,
            "缺少 auth_config，无法 refresh",
        )));
    };
    let Some(decrypted_auth_config) = helpers::decrypt_auth_config(state, encrypted_auth_config)
    else {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth encryption unavailable",
        )));
    };
    let parsed_auth_config = helpers::parse_auth_config_object(&decrypted_auth_config);
    if !helpers::auth_config_has_refresh_token(&parsed_auth_config) {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::BAD_REQUEST,
            "缺少 refresh_token，需要重新授权",
        )));
    }

    let provider_id = key.provider_id.clone();
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        )));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !provider_key_is_oauth_managed(&key, provider_type.as_str()) {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Key 不是 OAuth 管理账号",
        )));
    }
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        )));
    }

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?;
    let Some(endpoint) = provider_oauth_runtime_endpoint_for_provider(&provider_type, &endpoints)
    else {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::BAD_REQUEST,
            "找不到有效端点，无法 refresh",
        )));
    };
    let Some(transport) = state
        .read_provider_transport_snapshot_uncached(&provider_id, &endpoint.id, &key_id)
        .await?
    else {
        return Ok(RefreshDispatch::Respond(response::control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Provider transport snapshot unavailable",
        )));
    };

    Ok(RefreshDispatch::Continue(RefreshRequestContext {
        key_id,
        key,
        provider,
        provider_type,
        trace_id: request_context.trace_id().to_string(),
        transport,
    }))
}
