use super::session::attach_admin_provider_oauth_device_poll_terminal_response;
use crate::control::GatewayAdminPrincipalContext;
use crate::handlers::admin::provider::oauth::dispatch::helpers::admin_provider_oauth_key_name_from_auth_config;
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::provisioning::{
    provider_oauth_active_api_formats, provider_oauth_key_proxy_value,
};
use crate::handlers::admin::provider::oauth::runtime::spawn_provider_oauth_account_state_refresh_after_update;
use crate::handlers::admin::provider::oauth::state::{
    current_unix_secs, generate_provider_oauth_nonce,
};
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_contracts::ProxySnapshot;
use aether_data::repository::provider_oauth::{
    StoredAdminProviderOAuthDeviceSession, KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
};
use aether_oauth::core::OAuthError;
use aether_oauth::provider::providers::{
    XaiDevicePollOutcome, XaiProviderOAuthAdapter, XAI_CLIENT_ID, XAI_DEVICE_CODE_URL,
    XAI_TOKEN_URL,
};
use aether_oauth::provider::ProviderOAuthTransportContext;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

pub(super) async fn handle_admin_provider_oauth_xai_device_authorize(
    state: &AdminAppState<'_>,
    provider_id: &str,
    provider: &StoredProviderCatalogProvider,
    principal: &GatewayAdminPrincipalContext,
    runtime_endpoint: Option<&StoredProviderCatalogEndpoint>,
    request_proxy: Option<ProxySnapshot>,
    proxy_node_id: Option<&str>,
) -> Result<Response<Body>, GatewayError> {
    let device_url = state.provider_oauth_token_url("xai_device", XAI_DEVICE_CODE_URL);
    let token_url = state.provider_oauth_token_url("xai", XAI_TOKEN_URL);
    let adapter =
        XaiProviderOAuthAdapter::default().with_endpoint_overrides(&device_url, &token_url);
    let ctx = ProviderOAuthTransportContext {
        provider_id: provider_id.to_string(),
        provider_type: provider.provider_type.clone(),
        endpoint_id: runtime_endpoint.map(|endpoint| endpoint.id.clone()),
        key_id: None,
        auth_type: Some("oauth".to_string()),
        decrypted_api_key: None,
        decrypted_auth_config: None,
        provider_config: provider.config.clone(),
        endpoint_config: runtime_endpoint.and_then(|endpoint| endpoint.config.clone()),
        key_config: None,
        network: aether_oauth::network::OAuthNetworkContext::provider_operation(
            request_proxy.clone(),
        ),
    };
    let executor = crate::oauth::GatewayOAuthHttpExecutor::new(*state);
    let authorization = match adapter.start_device_flow(&executor, &ctx).await {
        Ok(authorization) => authorization,
        Err(error) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_REQUEST,
                sanitize_xai_oauth_error(&error),
            ));
        }
    };

    let now_unix_secs = current_unix_secs();
    let session_id = generate_provider_oauth_nonce();
    let session = StoredAdminProviderOAuthDeviceSession {
        session_id: session_id.clone(),
        provider_id: provider_id.to_string(),
        initiated_by_user_id: principal.user_id.clone(),
        initiated_by_session_id: principal.session_id.clone(),
        initiated_by_management_token_id: principal.management_token_id.clone(),
        region: String::new(),
        client_id: XAI_CLIENT_ID.to_string(),
        client_secret: String::new(),
        device_code: authorization.device_code.clone(),
        auth_type: Some("device".to_string()),
        social_provider: None,
        code_verifier: None,
        redirect_uri: Some(token_url),
        machine_id: None,
        interval: authorization.interval,
        expires_at_unix_secs: now_unix_secs.saturating_add(authorization.expires_in),
        status: "pending".to_string(),
        proxy_node_id: proxy_node_id
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
            authorization
                .expires_in
                .saturating_add(KIRO_DEVICE_AUTH_SESSION_TTL_BUFFER_SECS),
        )
        .await
    {
        return Ok(response);
    }

    Ok(Json(json!({
        "session_id": session_id,
        "user_code": authorization.user_code,
        "verification_uri": authorization.verification_uri,
        "verification_uri_complete": authorization.verification_uri_complete,
        "expires_in": authorization.expires_in,
        "interval": authorization.interval,
        "auth_type": "device",
    }))
    .into_response())
}

pub(super) async fn handle_admin_provider_oauth_xai_device_poll(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    request_proxy: Option<ProxySnapshot>,
    session_id: &str,
    mut session: StoredAdminProviderOAuthDeviceSession,
) -> Result<Response<Body>, GatewayError> {
    let token_url = session
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.provider_oauth_token_url("xai", XAI_TOKEN_URL));
    let adapter =
        XaiProviderOAuthAdapter::default().with_endpoint_overrides(XAI_DEVICE_CODE_URL, token_url);
    let ctx = ProviderOAuthTransportContext {
        provider_id: provider.id.clone(),
        provider_type: provider.provider_type.clone(),
        endpoint_id: None,
        key_id: None,
        auth_type: Some("oauth".to_string()),
        decrypted_api_key: None,
        decrypted_auth_config: None,
        provider_config: provider.config.clone(),
        endpoint_config: None,
        key_config: None,
        network: aether_oauth::network::OAuthNetworkContext::provider_operation(
            request_proxy.clone(),
        ),
    };
    let executor = crate::oauth::GatewayOAuthHttpExecutor::new(*state);
    let outcome = match adapter
        .poll_device_token(&executor, &ctx, &session.device_code)
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            return Ok(xai_device_poll_terminal_from_error(
                state,
                session_id,
                &mut session,
                &error,
            )
            .await);
        }
    };

    match outcome {
        XaiDevicePollOutcome::Pending => {
            Ok(Json(json!({"status": "pending", "replaced": false})).into_response())
        }
        XaiDevicePollOutcome::SlowDown => {
            Ok(Json(json!({"status": "slow_down", "replaced": false})).into_response())
        }
        XaiDevicePollOutcome::Authorized(result) => {
            persist_xai_device_authorization(
                state,
                provider,
                endpoints,
                request_proxy,
                session_id,
                session,
                *result,
            )
            .await
        }
    }
}

async fn persist_xai_device_authorization(
    state: &AdminAppState<'_>,
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    request_proxy: Option<ProxySnapshot>,
    session_id: &str,
    mut session: StoredAdminProviderOAuthDeviceSession,
    result: aether_oauth::provider::ProviderOAuthTokenSet,
) -> Result<Response<Body>, GatewayError> {
    let access_token = result.token_set.access_token.trim().to_string();
    if access_token.is_empty() {
        return Ok(Json(json!({
            "status": "error",
            "error": "xAI token 响应缺少 access_token",
            "replaced": false,
        }))
        .into_response());
    }
    let mut auth_config = result.auth_config.as_object().cloned().unwrap_or_default();
    auth_config.insert("provider_type".to_string(), json!("xai"));
    auth_config.insert("auth_method".to_string(), json!("oauth"));
    auth_config.insert("using_api".to_string(), json!(false));

    let duplicate = match state
        .find_duplicate_provider_oauth_key(&provider.id, &auth_config, None)
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

    let api_formats = provider_oauth_active_api_formats(endpoints);
    let key_proxy = provider_oauth_key_proxy_value(session.proxy_node_id.as_deref());
    let expires_at = result.token_set.expires_at_unix_secs;
    let email = auth_config
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
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
        let key_name = admin_provider_oauth_key_name_from_auth_config(
            &provider.provider_type,
            &auth_config,
            None,
        );
        match state
            .create_provider_oauth_catalog_key(
                &provider.id,
                &provider.provider_type,
                &key_name,
                &access_token,
                &auth_config,
                &api_formats,
                key_proxy,
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

async fn xai_device_poll_terminal_from_error(
    state: &AdminAppState<'_>,
    session_id: &str,
    session: &mut StoredAdminProviderOAuthDeviceSession,
    error: &OAuthError,
) -> Response<Body> {
    let (status, message) = match error {
        OAuthError::InvalidRequest(detail) if detail.to_ascii_lowercase().contains("expired") => {
            ("expired", "设备码已过期".to_string())
        }
        OAuthError::InvalidRequest(detail) if detail.to_ascii_lowercase().contains("denied") => {
            ("error", "用户拒绝授权".to_string())
        }
        _ => ("error", sanitize_xai_oauth_error(error)),
    };
    session.status = status.to_string();
    session.error_msg = Some(message.clone());
    let _ = state
        .save_provider_oauth_device_session(session_id, session, 30)
        .await;
    attach_admin_provider_oauth_device_poll_terminal_response(
        session_id,
        status,
        Json(json!({
            "status": status,
            "error": message,
            "replaced": false,
        }))
        .into_response(),
    )
}

fn sanitize_xai_oauth_error(error: &OAuthError) -> String {
    match error {
        OAuthError::InvalidRequest(_) => "xAI 设备授权失败: 请求参数无效".to_string(),
        OAuthError::HttpStatus { status_code, .. } => {
            format!("xAI 设备授权失败: HTTP {status_code}")
        }
        _ => "xAI 设备授权失败".to_string(),
    }
}
