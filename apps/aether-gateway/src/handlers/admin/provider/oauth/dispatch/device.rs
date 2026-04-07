use super::super::refresh::{
    build_internal_control_error_response, build_provider_oauth_auth_config_from_token_payload,
    create_provider_oauth_catalog_key, find_duplicate_provider_oauth_key,
    provider_oauth_active_api_formats, provider_oauth_key_proxy_value,
    refresh_provider_oauth_account_state_after_update, update_existing_provider_oauth_catalog_key,
};
use super::super::state::{
    build_admin_provider_oauth_backend_unavailable_response, build_kiro_device_key_name,
    current_unix_secs, decode_jwt_claims, default_kiro_device_region,
    default_kiro_device_start_url, generate_provider_oauth_nonce, json_non_empty_string,
    json_u64_value, normalize_kiro_device_region, poll_admin_kiro_device_token,
    read_provider_oauth_device_session, register_admin_kiro_device_oidc_client,
    save_provider_oauth_device_session, start_admin_kiro_device_authorization,
};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_oauth_device_authorize_provider_id, admin_provider_oauth_device_poll_provider_id,
};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::{AppState, GatewayError};
use aether_data::repository::provider_oauth::{
    StoredAdminProviderOAuthDeviceSession, KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct AdminProviderOAuthDeviceAuthorizePayload {
    #[serde(default = "default_kiro_device_start_url")]
    start_url: String,
    #[serde(default = "default_kiro_device_region")]
    region: String,
    proxy_node_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AdminProviderOAuthDevicePollPayload {
    session_id: String,
}

pub(super) async fn handle_admin_provider_oauth_device_authorize(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) =
        admin_provider_oauth_device_authorize_provider_id(&request_context.request_path)
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

    let client_registration =
        match register_admin_kiro_device_oidc_client(state, &region, &start_url).await {
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

    let device_authorization = match start_admin_kiro_device_authorization(
        state,
        &region,
        &client_id,
        &client_secret,
        &start_url,
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
        created_at_unix_secs: now_unix_secs,
        key_id: None,
        email: None,
        replaced: false,
        error_msg: None,
    };
    if let Err(response) = save_provider_oauth_device_session(
        state,
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

pub(super) async fn handle_admin_provider_oauth_device_poll(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) =
        admin_provider_oauth_device_poll_provider_id(&request_context.request_path)
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
    let payload = match serde_json::from_slice::<AdminProviderOAuthDevicePollPayload>(request_body)
    {
        Ok(payload) => payload,
        Err(_) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须是合法的 JSON 对象",
            ));
        }
    };
    let session_id = payload.session_id.trim();
    if session_id.is_empty() {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "session_id 不能为空",
        ));
    }

    let Some(mut session) = read_provider_oauth_device_session(state, session_id).await? else {
        return Ok(Json(json!({
            "status": "expired",
            "error": "会话不存在或已过期",
            "replaced": false,
        }))
        .into_response());
    };
    if session.provider_id != provider_id {
        return Ok(Json(json!({
            "status": "error",
            "error": "会话与 Provider 不匹配",
            "replaced": false,
        }))
        .into_response());
    }
    if session.status == "authorized" {
        return Ok(Json(json!({
            "status": "authorized",
            "key_id": session.key_id,
            "email": session.email,
            "replaced": session.replaced,
        }))
        .into_response());
    }
    if matches!(session.status.as_str(), "expired" | "error") {
        return Ok(Json(json!({
            "status": session.status,
            "error": session.error_msg,
            "replaced": session.replaced,
        }))
        .into_response());
    }

    if current_unix_secs() > session.expires_at_unix_secs {
        session.status = "expired".to_string();
        session.error_msg = Some("设备码已过期".to_string());
        let _ = save_provider_oauth_device_session(state, session_id, &session, 30).await;
        return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
            session_id,
            "expired",
            Json(json!({
                "status": "expired",
                "error": "设备码已过期",
                "replaced": false,
            }))
            .into_response(),
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

    let token_result = match poll_admin_kiro_device_token(
        state,
        &session.region,
        &session.client_id,
        &session.client_secret,
        &session.device_code,
    )
    .await
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    if token_result
        .get("_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        let error_code = json_non_empty_string(token_result.get("error")).unwrap_or_default();
        if error_code == "authorization_pending" {
            return Ok(Json(json!({"status": "pending", "replaced": false})).into_response());
        }
        if error_code == "slow_down" {
            return Ok(Json(json!({"status": "slow_down", "replaced": false})).into_response());
        }
        if error_code == "expired_token" {
            session.status = "expired".to_string();
            session.error_msg = Some("设备码已过期".to_string());
            let _ = save_provider_oauth_device_session(state, session_id, &session, 30).await;
            return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
                session_id,
                "expired",
                Json(json!({
                    "status": "expired",
                    "error": "设备码已过期",
                    "replaced": false,
                }))
                .into_response(),
            ));
        }
        if error_code == "access_denied" {
            session.status = "error".to_string();
            session.error_msg = Some("用户拒绝授权".to_string());
            let _ = save_provider_oauth_device_session(state, session_id, &session, 30).await;
            return Ok(attach_admin_provider_oauth_device_poll_terminal_response(
                session_id,
                "error",
                Json(json!({
                    "status": "error",
                    "error": "用户拒绝授权",
                    "replaced": false,
                }))
                .into_response(),
            ));
        }
        let error_message = json_non_empty_string(token_result.get("error_description"))
            .or_else(|| (!error_code.is_empty()).then_some(error_code.clone()))
            .unwrap_or_else(|| "未知错误".to_string());
        return Ok(Json(json!({
            "status": "error",
            "error": error_message,
            "replaced": false,
        }))
        .into_response());
    }

    let Some(access_token) = json_non_empty_string(token_result.get("accessToken")) else {
        return Ok(Json(json!({
            "status": "error",
            "error": "token 响应缺少 accessToken 或 refreshToken",
            "replaced": false,
        }))
        .into_response());
    };
    let Some(refresh_token) = json_non_empty_string(token_result.get("refreshToken")) else {
        return Ok(Json(json!({
            "status": "error",
            "error": "token 响应缺少 accessToken 或 refreshToken",
            "replaced": false,
        }))
        .into_response());
    };
    let expires_at = json_u64_value(token_result.get("expiresIn"))
        .map(|expires_in| current_unix_secs().saturating_add(expires_in))
        .unwrap_or_else(|| current_unix_secs().saturating_add(3600));
    let email = decode_jwt_claims(&access_token)
        .and_then(|claims| claims.get("email").cloned())
        .and_then(|value| value.as_str().map(ToOwned::to_owned));

    let mut auth_config = serde_json::Map::new();
    auth_config.insert("provider_type".to_string(), json!("kiro"));
    auth_config.insert("auth_method".to_string(), json!("idc"));
    auth_config.insert("refresh_token".to_string(), json!(refresh_token.clone()));
    auth_config.insert("client_id".to_string(), json!(session.client_id.clone()));
    auth_config.insert(
        "client_secret".to_string(),
        json!(session.client_secret.clone()),
    );
    auth_config.insert("region".to_string(), json!(session.region.clone()));
    auth_config.insert("auth_region".to_string(), json!(session.region.clone()));
    auth_config.insert("access_token".to_string(), json!(access_token.clone()));
    auth_config.insert("expires_at".to_string(), json!(expires_at));
    if let Some(email) = email.as_ref() {
        auth_config.insert("email".to_string(), json!(email));
    }

    let duplicate =
        match find_duplicate_provider_oauth_key(state, &provider_id, &auth_config, None).await {
            Ok(duplicate) => duplicate,
            Err(detail) => {
                return Ok(Json(json!({
                    "status": "error",
                    "error": detail,
                    "replaced": false,
                }))
                .into_response());
            }
        };

    let key_proxy = provider_oauth_key_proxy_value(session.proxy_node_id.as_deref());
    let api_formats = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .filter(|endpoint| endpoint.is_active)
        .map(|endpoint| endpoint.api_format)
        .collect::<Vec<_>>();
    let mut replaced = false;
    let persisted_key = if let Some(existing_key) = duplicate {
        replaced = true;
        match update_existing_provider_oauth_catalog_key(
            state,
            &existing_key,
            &access_token,
            &auth_config,
            key_proxy.clone(),
            Some(expires_at),
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
        let key_name = build_kiro_device_key_name(email.as_deref(), Some(&refresh_token));
        match create_provider_oauth_catalog_key(
            state,
            &provider_id,
            &key_name,
            &access_token,
            &auth_config,
            &api_formats,
            key_proxy.clone(),
            Some(expires_at),
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

    session.status = "authorized".to_string();
    session.key_id = Some(persisted_key.id.clone());
    session.email = email.clone();
    session.replaced = replaced;
    session.error_msg = None;
    let _ = save_provider_oauth_device_session(state, session_id, &session, 60).await;

    Ok(attach_admin_provider_oauth_device_poll_terminal_response(
        session_id,
        "authorized",
        Json(json!({
            "status": "authorized",
            "key_id": persisted_key.id,
            "email": email,
            "replaced": replaced,
        }))
        .into_response(),
    ))
}

fn attach_admin_provider_oauth_device_poll_terminal_response(
    session_id: &str,
    status: &str,
    response: Response<Body>,
) -> Response<Body> {
    match status {
        "authorized" => attach_admin_audit_response(
            response,
            "admin_provider_oauth_device_authorization_completed",
            "poll_provider_oauth_device_authorization_terminal_state",
            "provider_oauth_device_session",
            session_id,
        ),
        "expired" => attach_admin_audit_response(
            response,
            "admin_provider_oauth_device_authorization_expired",
            "poll_provider_oauth_device_authorization_terminal_state",
            "provider_oauth_device_session",
            session_id,
        ),
        "error" => attach_admin_audit_response(
            response,
            "admin_provider_oauth_device_authorization_failed",
            "poll_provider_oauth_device_authorization_terminal_state",
            "provider_oauth_device_session",
            session_id,
        ),
        _ => response,
    }
}
