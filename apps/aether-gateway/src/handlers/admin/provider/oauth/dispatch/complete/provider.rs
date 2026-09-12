use super::super::super::errors::build_internal_control_error_response;
use super::super::super::provisioning::{
    provider_oauth_key_proxy_value, provision_provider_oauth_token_payload_for_provider,
};
use super::super::super::runtime::resolve_provider_oauth_runtime_endpoints;
use super::super::super::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    is_fixed_provider_type_for_provider_oauth,
};
use super::shared::{
    admin_provider_oauth_state_matches_principal, parse_admin_provider_oauth_complete_callback,
    parse_admin_provider_oauth_complete_request_body,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_complete_provider_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};

pub(super) async fn handle_admin_provider_oauth_complete_provider(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(provider_id) = admin_provider_oauth_complete_provider_id(request_context.path())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let payload = match parse_admin_provider_oauth_complete_request_body(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let callback = match parse_admin_provider_oauth_complete_callback(&payload.callback_url) {
        Ok(callback) => callback,
        Err(response) => return Ok(response),
    };

    let preview = match state.load_provider_oauth_state(&callback.state_nonce).await {
        Ok(Some(state_data)) => state_data,
        Ok(None) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "state 无效或已过期",
            ));
        }
        Err(GatewayError::Client {
            status: http::StatusCode::BAD_REQUEST,
            ..
        }) => {
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
    if !preview.key_id.trim().is_empty()
        || preview.provider_id != provider_id
        || !admin_provider_oauth_state_matches_principal(&preview, request_context)
    {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "state 无效或已过期",
        ));
    }
    let state_data = match state
        .consume_provider_oauth_state(&callback.state_nonce)
        .await
    {
        Ok(Some(state_data)) if state_data == preview => state_data,
        Ok(Some(_)) | Ok(None) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "state 无效或已过期",
            ));
        }
        Err(GatewayError::Client {
            status: http::StatusCode::BAD_REQUEST,
            ..
        }) => {
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
    let endpoint_resolution =
        resolve_provider_oauth_runtime_endpoints(state, &provider, &provider_type).await?;
    let endpoints = endpoint_resolution.endpoints;
    let runtime_endpoint = endpoint_resolution.runtime_endpoint;
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            payload.proxy_node_id.as_deref(),
            &[
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;
    let key_proxy = provider_oauth_key_proxy_value(payload.proxy_node_id.as_deref());

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

    provision_provider_oauth_token_payload_for_provider(
        state,
        &provider,
        &endpoints,
        &token_payload,
        payload.name,
        key_proxy,
        request_proxy,
        "provider-complete",
    )
    .await
}
