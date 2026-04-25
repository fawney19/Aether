use super::super::kiro::{
    admin_provider_oauth_kiro_refresh_base_url_override, fetch_admin_provider_oauth_kiro_email,
    refresh_admin_provider_oauth_kiro_auth_config,
};
use super::session::{
    attach_admin_provider_oauth_device_poll_terminal_response, AdminProviderOAuthDevicePollPayload,
};
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::provisioning::{
    provider_oauth_active_api_formats, provider_oauth_key_proxy_value,
};
use crate::handlers::admin::provider::oauth::runtime::{
    provider_oauth_runtime_endpoint_for_provider,
    spawn_provider_oauth_account_state_refresh_after_update,
};
use crate::handlers::admin::provider::oauth::state::{
    build_admin_provider_oauth_backend_unavailable_response, build_kiro_device_key_name,
    current_unix_secs, decode_jwt_claims, json_non_empty_string, json_u64_value,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_device_poll_provider_id;
use crate::handlers::admin::request::{AdminAppState, AdminKiroAuthConfig, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn handle_admin_provider_oauth_device_poll(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) = admin_provider_oauth_device_poll_provider_id(request_context.path())
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

    let Some(mut session) = state.read_provider_oauth_device_session(session_id).await? else {
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
        let _ = state
            .save_provider_oauth_device_session(session_id, &session, 30)
            .await;
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
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider_id))
        .await?;
    let runtime_endpoint = provider_oauth_runtime_endpoint_for_provider("kiro", &endpoints);
    let request_proxy = state
        .resolve_admin_provider_oauth_operation_proxy_snapshot(
            session.proxy_node_id.as_deref(),
            &[
                runtime_endpoint
                    .as_ref()
                    .and_then(|endpoint| endpoint.proxy.as_ref()),
                provider.proxy.as_ref(),
            ],
        )
        .await;

    let token_result = match state
        .poll_admin_kiro_device_token(
            &session.region,
            &session.client_id,
            &session.client_secret,
            &session.device_code,
            request_proxy.clone(),
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
            let _ = state
                .save_provider_oauth_device_session(session_id, &session, 30)
                .await;
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
            let _ = state
                .save_provider_oauth_device_session(session_id, &session, 30)
                .await;
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

    let Some(access_token) = json_non_empty_string(
        token_result
            .get("accessToken")
            .or_else(|| token_result.get("access_token")),
    ) else {
        return Ok(Json(json!({
            "status": "error",
            "error": "token 响应缺少 accessToken 或 refreshToken",
            "replaced": false,
        }))
        .into_response());
    };
    let Some(refresh_token) = json_non_empty_string(
        token_result
            .get("refreshToken")
            .or_else(|| token_result.get("refresh_token")),
    ) else {
        return Ok(Json(json!({
            "status": "error",
            "error": "token 响应缺少 accessToken 或 refreshToken",
            "replaced": false,
        }))
        .into_response());
    };
    let initial_expires_at = json_u64_value(
        token_result
            .get("expiresIn")
            .or_else(|| token_result.get("expires_in")),
    )
    .map(|expires_in| current_unix_secs().saturating_add(expires_in))
    .unwrap_or_else(|| current_unix_secs().saturating_add(3600));
    let social_refresh_base_url =
        admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_social_refresh");
    let idc_refresh_base_url =
        admin_provider_oauth_kiro_refresh_base_url_override(state, "kiro_idc_refresh");
    let mut refreshed_auth_config = match refresh_admin_provider_oauth_kiro_auth_config(
        state,
        &AdminKiroAuthConfig {
            auth_method: Some("idc".to_string()),
            refresh_token: Some(refresh_token.clone()),
            expires_at: Some(initial_expires_at),
            profile_arn: None,
            region: Some(session.region.clone()),
            auth_region: Some(session.region.clone()),
            api_region: None,
            client_id: Some(session.client_id.clone()),
            client_secret: Some(session.client_secret.clone()),
            machine_id: None,
            kiro_version: None,
            system_version: None,
            node_version: None,
            access_token: Some(access_token.clone()),
        },
        request_proxy.clone(),
        social_refresh_base_url.as_deref(),
        idc_refresh_base_url.as_deref(),
    )
    .await
    {
        Ok(config) => config,
        Err(detail) => {
            return Ok(Json(json!({
                "status": "error",
                "error": format!("token 验证失败: {detail}"),
                "replaced": false,
            }))
            .into_response());
        }
    };
    if refreshed_auth_config.auth_method.is_none() {
        refreshed_auth_config.auth_method = Some("idc".to_string());
    }
    let Some(access_token) = refreshed_auth_config
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(Json(json!({
            "status": "error",
            "error": "token 验证失败: accessToken 为空",
            "replaced": false,
        }))
        .into_response());
    };
    let expires_at = refreshed_auth_config
        .expires_at
        .unwrap_or_else(|| current_unix_secs().saturating_add(3600));
    let mut email = decode_jwt_claims(&access_token)
        .and_then(|claims| claims.get("email").cloned())
        .and_then(|value| value.as_str().map(ToOwned::to_owned));
    if email.is_none() {
        email = fetch_admin_provider_oauth_kiro_email(
            state,
            &refreshed_auth_config,
            request_proxy.clone(),
        )
        .await;
    }

    let mut auth_config = refreshed_auth_config
        .to_json_value()
        .as_object()
        .cloned()
        .unwrap_or_default();
    auth_config.insert("provider_type".to_string(), json!("kiro"));
    if let Some(email) = email.as_ref() {
        auth_config.insert("email".to_string(), json!(email));
    }

    let duplicate = match state
        .find_duplicate_provider_oauth_key(&provider_id, &auth_config, None)
        .await
    {
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

    let api_formats = provider_oauth_active_api_formats(&endpoints);
    let key_proxy = provider_oauth_key_proxy_value(session.proxy_node_id.as_deref());
    let mut replaced = false;
    let persisted_key = if let Some(existing_key) = duplicate {
        replaced = true;
        match state
            .update_existing_provider_oauth_catalog_key(
                &existing_key,
                &provider.provider_type,
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
    } else {
        let key_name = build_kiro_device_key_name(
            email.as_deref(),
            refreshed_auth_config.refresh_token.as_deref(),
        );
        match state
            .create_provider_oauth_catalog_key(
                &provider_id,
                &provider.provider_type,
                &key_name,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy,
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

    spawn_provider_oauth_account_state_refresh_after_update(
        state.cloned_app(),
        provider.clone(),
        persisted_key.id.clone(),
        request_proxy.clone(),
    );

    session.status = "authorized".to_string();
    session.key_id = Some(persisted_key.id.clone());
    session.email = email.clone();
    session.replaced = replaced;
    session.error_msg = None;
    let _ = state
        .save_provider_oauth_device_session(session_id, &session, 60)
        .await;

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
