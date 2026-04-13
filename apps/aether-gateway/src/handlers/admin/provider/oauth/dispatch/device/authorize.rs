use super::session::AdminProviderOAuthDeviceAuthorizePayload;
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::state::{
    build_admin_provider_oauth_backend_unavailable_response, current_unix_secs,
    default_kiro_device_start_url, generate_provider_oauth_nonce, json_non_empty_string,
    json_u64_value, normalize_kiro_device_region,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_device_authorize_provider_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_data::repository::provider_oauth::{
    StoredAdminProviderOAuthDeviceSession, KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn handle_admin_provider_oauth_device_authorize(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) =
        admin_provider_oauth_device_authorize_provider_id(request_context.path())
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
    let payload =
        match serde_json::from_slice::<AdminProviderOAuthDeviceAuthorizePayload>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(build_internal_control_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "请求体必须是合法的 JSON 对象",
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
    if provider_type != "kiro" {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "设备授权仅支持 Kiro provider",
        ));
    }

    let region = normalize_kiro_device_region(Some(payload.region.as_str())).ok_or_else(|| {
        build_internal_control_error_response(http::StatusCode::BAD_REQUEST, "region 格式无效")
    });
    let region = match region {
        Ok(region) => region,
        Err(response) => return Ok(response),
    };
    let start_url = payload.start_url.trim();
    let start_url = if start_url.is_empty() {
        default_kiro_device_start_url()
    } else {
        start_url.to_string()
    };

    let client_registration = match state
        .register_admin_kiro_device_oidc_client(
            &region,
            &start_url,
            payload.proxy_node_id.as_deref(),
        )
        .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let Some(client_id) = json_non_empty_string(client_registration.get("clientId")) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "注册 OIDC 客户端失败: unknown",
        ));
    };
    let Some(client_secret) = json_non_empty_string(client_registration.get("clientSecret")) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "注册 OIDC 客户端失败: unknown",
        ));
    };

    let device_authorization = match state
        .start_admin_kiro_device_authorization(
            &region,
            &client_id,
            &client_secret,
            &start_url,
            payload.proxy_node_id.as_deref(),
        )
        .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let Some(device_code) = json_non_empty_string(
        device_authorization
            .get("deviceCode")
            .or_else(|| device_authorization.get("device_code")),
    ) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "发起设备授权失败: unknown",
        ));
    };
    let user_code = json_non_empty_string(
        device_authorization
            .get("userCode")
            .or_else(|| device_authorization.get("user_code")),
    )
    .unwrap_or_default();
    let verification_uri = json_non_empty_string(
        device_authorization
            .get("verificationUri")
            .or_else(|| device_authorization.get("verification_uri"))
            .or_else(|| device_authorization.get("verificationUrl")),
    )
    .unwrap_or_default();
    let verification_uri_complete = json_non_empty_string(
        device_authorization
            .get("verificationUriComplete")
            .or_else(|| device_authorization.get("verification_uri_complete"))
            .or_else(|| device_authorization.get("verificationUrlComplete")),
    )
    .unwrap_or_else(|| verification_uri.clone());
    let expires_in = json_u64_value(
        device_authorization
            .get("expiresIn")
            .or_else(|| device_authorization.get("expires_in")),
    )
    .unwrap_or(600);
    let interval = json_u64_value(device_authorization.get("interval")).unwrap_or(5);
    let now_unix_secs = current_unix_secs();
    let session_id = generate_provider_oauth_nonce();
    let session = StoredAdminProviderOAuthDeviceSession {
        provider_id: provider_id.clone(),
        region,
        client_id,
        client_secret,
        device_code,
        interval,
        expires_at_unix_secs: now_unix_secs.saturating_add(expires_in),
        status: "pending".to_string(),
        proxy_node_id: payload
            .proxy_node_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        created_at_unix_ms: now_unix_secs,
        key_id: None,
        email: None,
        replaced: false,
        error_msg: None,
    };
    if let Err(response) = state
        .save_provider_oauth_device_session(
            &session_id,
            &session,
            expires_in.saturating_add(KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS),
        )
        .await
    {
        return Ok(response);
    }

    Ok(Json(json!({
        "session_id": session_id,
        "user_code": user_code,
        "verification_uri": verification_uri,
        "verification_uri_complete": verification_uri_complete,
        "expires_in": expires_in,
        "interval": interval,
    }))
    .into_response())
}
