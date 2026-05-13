use super::super::shared::{
    default_provider_quota_execution_timeouts, execute_provider_quota_plan,
    extract_execution_error_message, persist_provider_quota_refresh_state,
    quota_refresh_success_invalid_state, ProviderQuotaExecutionOutcome,
};
use super::parse::parse_kiro_usage_response;
use super::plan::{build_kiro_usage_headers, execute_kiro_quota_plan};
use super::{
    kiro_auth_from_refreshed_entry, kiro_quota_error_is_account_banned,
    kiro_quota_error_is_token_invalid,
};
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::runtime::provider_oauth_runtime_endpoint_for_provider;
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_kiro_overage_key_id;
use crate::handlers::admin::request::{
    AdminAppState, AdminGatewayProviderTransportSnapshot, AdminKiroRequestAuth, AdminRequestContext,
};
use crate::GatewayError;
use aether_contracts::{ExecutionPlan, ProxySnapshot, RequestBody};
use aether_provider_transport::LocalResolvedOAuthRequestAuth;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

fn build_kiro_overage_url(auth: &AdminKiroRequestAuth) -> String {
    format!(
        "https://q.{}.amazonaws.com/setUserPreference",
        auth.auth_config.effective_api_region()
    )
}

fn build_kiro_overage_body(auth: &AdminKiroRequestAuth, enabled: bool) -> Value {
    let status = if enabled { "ENABLED" } else { "DISABLED" };
    let mut object = serde_json::Map::new();
    object.insert(
        "overageConfiguration".to_string(),
        json!({ "overageStatus": status }),
    );
    if let Some(profile_arn) = auth.auth_config.profile_arn_for_payload() {
        object.insert("profileArn".to_string(), json!(profile_arn));
    }
    Value::Object(object)
}

async fn execute_kiro_overage_plan(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    auth: &AdminKiroRequestAuth,
    enabled: bool,
    proxy_override: Option<&ProxySnapshot>,
) -> Result<ProviderQuotaExecutionOutcome, GatewayError> {
    let proxy = match proxy_override {
        Some(proxy) => Some(proxy.clone()),
        None => {
            state
                .resolve_transport_proxy_snapshot_with_tunnel_affinity(transport)
                .await
        }
    };
    let timeouts = state
        .resolve_transport_execution_timeouts(transport)
        .or(Some(default_provider_quota_execution_timeouts(
            proxy.as_ref(),
        )));
    let body_json = build_kiro_overage_body(auth, enabled);
    let plan = ExecutionPlan {
        request_id: format!("kiro-overage:{}", transport.key.id),
        candidate_id: None,
        provider_name: Some("kiro".to_string()),
        provider_id: transport.provider.id.clone(),
        endpoint_id: transport.endpoint.id.clone(),
        key_id: transport.key.id.clone(),
        method: "POST".to_string(),
        url: build_kiro_overage_url(auth),
        headers: build_kiro_usage_headers(auth),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: RequestBody {
            json_body: Some(body_json),
            body_bytes_b64: None,
            body_ref: None,
        },
        stream: false,
        client_api_format: "claude:messages".to_string(),
        provider_api_format: "kiro:set_user_preference".to_string(),
        model_name: Some("kiro-set-user-preference".to_string()),
        proxy,
        transport_profile: state.resolve_transport_profile(transport),
        timeouts,
    };

    execute_provider_quota_plan(state, transport, plan, "kiro_overage").await
}

async fn resolve_kiro_request_auth(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
) -> Result<(AdminKiroRequestAuth, bool), GatewayError> {
    match state.force_local_oauth_refresh_entry(transport).await {
        Ok(Some(entry)) => match kiro_auth_from_refreshed_entry(&entry) {
            Some(LocalResolvedOAuthRequestAuth::Kiro(auth)) => Ok((auth, true)),
            _ => Err(GatewayError::Internal(
                "Kiro Token 刷新成功但认证信息解析失败".to_string(),
            )),
        },
        Ok(None) => match state
            .resolve_local_oauth_kiro_request_auth(transport)
            .await?
        {
            Some(auth) => Ok((auth, false)),
            None => Err(GatewayError::Internal(
                "缺少 Kiro 认证配置 (auth_config)".to_string(),
            )),
        },
        Err(err) => Err(GatewayError::Internal(format!(
            "Kiro Token 刷新失败: {err}"
        ))),
    }
}

fn encode_refreshed_auth_config(
    state: &AdminAppState<'_>,
    transport: &AdminGatewayProviderTransportSnapshot,
    auth: &AdminKiroRequestAuth,
) -> Option<String> {
    let mut auth_config_object = transport
        .key
        .decrypted_auth_config
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(refreshed_auth_config) = auth.auth_config.to_json_value().as_object() {
        for (key, value) in refreshed_auth_config {
            auth_config_object.insert(key.clone(), value.clone());
        }
    }
    auth_config_object
        .entry("provider_type".to_string())
        .or_insert_with(|| json!("kiro"));
    let auth_config_json = Value::Object(auth_config_object).to_string();
    state.encrypt_catalog_secret_with_fallbacks(auth_config_json.as_str())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn bad_request(message: impl Into<String>) -> Response<Body> {
    build_internal_control_error_response(http::StatusCode::BAD_REQUEST, message)
}

fn not_found(message: impl Into<String>) -> Response<Body> {
    build_internal_control_error_response(http::StatusCode::NOT_FOUND, message)
}

fn forbidden(message: impl Into<String>) -> Response<Body> {
    build_internal_control_error_response(http::StatusCode::FORBIDDEN, message)
}

fn upstream_failure(status_code: u16, detail: &str) -> Response<Body> {
    build_internal_control_error_response(
        http::StatusCode::BAD_GATEWAY,
        format!("setUserPreference 返回状态码 {status_code}: {detail}"),
    )
}

pub(crate) async fn handle_admin_kiro_overage(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(key_id) = admin_provider_oauth_kiro_overage_key_id(request_context.path()) else {
        return Ok(not_found("Key 不存在"));
    };

    let Some(request_body) = request_body else {
        return Ok(bad_request(
            "请求体必须是合法的 JSON 对象，且包含 enabled 字段",
        ));
    };
    let payload = match serde_json::from_slice::<Value>(request_body) {
        Ok(Value::Object(map)) => map,
        _ => {
            return Ok(bad_request(
                "请求体必须是合法的 JSON 对象，且包含 enabled 字段",
            ));
        }
    };
    let Some(enabled) = payload.get("enabled").and_then(Value::as_bool) else {
        return Ok(bad_request("enabled 字段必须是布尔值"));
    };

    let Some(key) = state
        .read_provider_catalog_keys_by_ids(std::slice::from_ref(&key_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(not_found("Key 不存在"));
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&key.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(not_found("Provider 不存在"));
    };
    if !provider.provider_type.trim().eq_ignore_ascii_case("kiro") {
        return Ok(bad_request("该 Provider 不是 Kiro 类型"));
    }

    let overage_capable = key
        .upstream_metadata
        .as_ref()
        .and_then(|metadata| metadata.get("kiro"))
        .and_then(|kiro| kiro.get("overage_capable"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !overage_capable {
        return Ok(forbidden("该订阅不支持超额"));
    }

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let Some(endpoint) = provider_oauth_runtime_endpoint_for_provider("kiro", &endpoints) else {
        return Ok(bad_request("找不到有效的 Kiro 端点"));
    };

    let Some(transport) = state
        .read_provider_transport_snapshot(&provider.id, &endpoint.id, &key.id)
        .await?
    else {
        return Ok(bad_request("Provider transport snapshot unavailable"));
    };

    let (auth, token_refreshed) = match resolve_kiro_request_auth(state, &transport).await {
        Ok(result) => result,
        Err(GatewayError::Internal(message)) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                message,
            ));
        }
        Err(err) => return Err(err),
    };

    let proxy_override = None::<ProxySnapshot>;
    let toggle_outcome =
        execute_kiro_overage_plan(state, &transport, &auth, enabled, proxy_override.as_ref())
            .await?;

    let toggle_result = match toggle_outcome {
        ProviderQuotaExecutionOutcome::Response(result) => result,
        ProviderQuotaExecutionOutcome::Failure(detail) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_GATEWAY,
                format!("setUserPreference 请求执行失败: {detail}"),
            ));
        }
    };

    if toggle_result.status_code != 200 {
        let now = now_unix_secs();
        let err_msg = extract_execution_error_message(&toggle_result);
        let detail = err_msg
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown error");
        let (mut oauth_invalid_at_unix_secs, mut oauth_invalid_reason) =
            quota_refresh_success_invalid_state(&key);
        let mut metadata_update = None::<Value>;
        match toggle_result.status_code {
            401 => {
                oauth_invalid_at_unix_secs = Some(now);
                oauth_invalid_reason = Some("Kiro Token 无效或已过期".to_string());
            }
            403 | 423 => {
                if kiro_quota_error_is_token_invalid(err_msg.as_deref()) {
                    oauth_invalid_at_unix_secs = Some(now);
                    oauth_invalid_reason = Some("Kiro Token 无效或已过期".to_string());
                } else if kiro_quota_error_is_account_banned(err_msg.as_deref()) {
                    oauth_invalid_at_unix_secs = Some(now);
                    oauth_invalid_reason = Some(format!("账户已封禁: {detail}"));
                    metadata_update = Some(json!({
                        "kiro": {
                            "is_banned": true,
                            "ban_reason": detail,
                            "banned_at": now,
                            "updated_at": now,
                        }
                    }));
                }
            }
            _ => {}
        }
        let _ = persist_provider_quota_refresh_state(
            state,
            &key_id,
            metadata_update.as_ref(),
            oauth_invalid_at_unix_secs,
            oauth_invalid_reason,
            None,
        )
        .await?;
        return Ok(upstream_failure(toggle_result.status_code, detail));
    }

    // Upstream accepted the toggle. Refetch authoritative state via
    // getUsageLimits and persist the merged metadata so any concurrent
    // quota refresh sees consistent data.
    let usage_outcome =
        execute_kiro_quota_plan(state, &transport, &auth, proxy_override.as_ref()).await?;
    let usage_result = match usage_outcome {
        ProviderQuotaExecutionOutcome::Response(result) => result,
        ProviderQuotaExecutionOutcome::Failure(detail) => {
            return Ok(build_internal_control_error_response(
                http::StatusCode::BAD_GATEWAY,
                format!("getUsageLimits 请求执行失败: {detail}"),
            ));
        }
    };
    if usage_result.status_code != 200 {
        let err_msg = extract_execution_error_message(&usage_result);
        return Ok(upstream_failure(
            usage_result.status_code,
            err_msg.as_deref().unwrap_or("unknown error"),
        ));
    }
    let Some(usage_body) = usage_result
        .body
        .as_ref()
        .and_then(|body| body.json_body.as_ref())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_GATEWAY,
            "getUsageLimits 响应中未包含限额信息",
        ));
    };
    let now = now_unix_secs();
    let Some(metadata) = parse_kiro_usage_response(usage_body, now) else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_GATEWAY,
            "getUsageLimits 响应无法解析",
        ));
    };

    let metadata_update = json!({ "kiro": metadata.clone() });
    let encrypted_auth_config = if token_refreshed {
        encode_refreshed_auth_config(state, &transport, &auth)
    } else {
        None
    };
    let (oauth_invalid_at_unix_secs, oauth_invalid_reason) =
        quota_refresh_success_invalid_state(&key);
    let _ = persist_provider_quota_refresh_state(
        state,
        &key_id,
        Some(&metadata_update),
        oauth_invalid_at_unix_secs,
        oauth_invalid_reason,
        encrypted_auth_config,
    )
    .await?;

    Ok(Json(metadata).into_response())
}
