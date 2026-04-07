use super::super::refresh::build_internal_control_error_response;
use super::super::state::{
    admin_provider_oauth_template, build_provider_oauth_start_response,
    generate_provider_oauth_pkce_verifier, is_fixed_provider_type_for_provider_oauth,
    provider_oauth_pkce_s256, save_provider_oauth_state,
};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_oauth_start_key_id, admin_provider_oauth_start_provider_id,
};
use crate::{AppState, GatewayError};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn handle_admin_provider_oauth_start_key(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Result<Response<Body>, GatewayError> {
    let Some(key_id) = admin_provider_oauth_start_key_id(&request_context.request_path) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Key 不存在",
        ));
    };
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
    let Some(template) = admin_provider_oauth_template(&provider_type) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不支持 OAuth 授权",
        ));
    };

    let pkce_verifier = template
        .use_pkce
        .then(generate_provider_oauth_pkce_verifier);
    let code_challenge = pkce_verifier.as_deref().map(provider_oauth_pkce_s256);
    let nonce = match save_provider_oauth_state(
        state,
        &key_id,
        &provider_id,
        &provider_type,
        pkce_verifier.as_deref(),
    )
    .await
    {
        Ok(nonce) => nonce,
        Err(_) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth redis unavailable",
            ));
        }
    };

    Ok(Json(build_provider_oauth_start_response(
        template,
        &nonce,
        code_challenge.as_deref(),
    ))
    .into_response())
}

pub(super) async fn handle_admin_provider_oauth_start_provider(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Result<Response<Body>, GatewayError> {
    let Some(provider_id) = admin_provider_oauth_start_provider_id(&request_context.request_path)
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
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
    if provider_type == "kiro" {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "Kiro 不支持 OAuth 授权，请使用导入授权。",
        ));
    }
    let Some(template) = admin_provider_oauth_template(&provider_type) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不支持 OAuth 授权",
        ));
    };

    let pkce_verifier = template
        .use_pkce
        .then(generate_provider_oauth_pkce_verifier);
    let code_challenge = pkce_verifier.as_deref().map(provider_oauth_pkce_s256);
    let nonce = match save_provider_oauth_state(
        state,
        "",
        &provider_id,
        &provider_type,
        pkce_verifier.as_deref(),
    )
    .await
    {
        Ok(nonce) => nonce,
        Err(_) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                "provider oauth redis unavailable",
            ));
        }
    };

    Ok(Json(build_provider_oauth_start_response(
        template,
        &nonce,
        code_challenge.as_deref(),
    ))
    .into_response())
}
