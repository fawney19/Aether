use super::request::{
    admin_provider_ops_execute_json_request, admin_provider_ops_headers_with_transport_controls,
    admin_provider_ops_verify_execution_error_message, AdminProviderOpsExecuteJsonError,
};
use crate::handlers::admin::provider::ops::providers::config::persist_admin_provider_ops_runtime_credentials;
use crate::handlers::admin::request::AdminAppState;
use crate::handlers::shared::{
    canonicalize_provider_ops_base_url, resolve_provider_ops_same_origin_url,
};
use aether_admin::provider::ops::{
    admin_provider_ops_frontend_updated_credentials, admin_provider_ops_verify_failure,
    parse_verify_payload, ADMIN_PROVIDER_OPS_USER_AGENT,
};
use aether_contracts::ProxySnapshot;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogProvider;
use serde_json::{json, Map, Value};
use tracing::warn;

pub(super) async fn admin_provider_ops_local_sub2api_verify_response(
    state: &AdminAppState<'_>,
    provider: Option<&StoredProviderCatalogProvider>,
    base_url: &str,
    verify_endpoint: &str,
    credentials: &Map<String, Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Value {
    let (access_token, updated_credentials, frontend_updated_credentials) =
        match admin_provider_ops_sub2api_exchange_token(
            state,
            base_url,
            credentials,
            proxy_snapshot,
        )
        .await
        {
            Ok(value) => value,
            Err(message) => return admin_provider_ops_verify_failure(message),
        };
    if let Some(provider) = provider.filter(|_| !updated_credentials.is_empty()) {
        if let Err(err) =
            persist_admin_provider_ops_runtime_credentials(state, provider, &updated_credentials)
                .await
        {
            warn!(
                provider_id = %provider.id,
                error = ?err,
                "failed to persist sub2api verify runtime credentials"
            );
        }
    }

    let verify_url = match admin_provider_ops_sub2api_request_url(base_url, verify_endpoint) {
        Ok(url) => url,
        Err(message) => return admin_provider_ops_verify_failure(message),
    };
    let auth_value = match reqwest::header::HeaderValue::from_str(&format!("Bearer {access_token}"))
    {
        Ok(value) => value,
        Err(_) => return admin_provider_ops_verify_failure("访问令牌格式无效"),
    };
    let auth_headers = reqwest::header::HeaderMap::from_iter([
        (reqwest::header::AUTHORIZATION, auth_value),
        (
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static(ADMIN_PROVIDER_OPS_USER_AGENT),
        ),
        (
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("*/*"),
        ),
    ]);
    let auth_headers =
        admin_provider_ops_headers_with_transport_controls(&auth_headers, None, true);
    let (status, response_json) = match admin_provider_ops_execute_json_request(
        state,
        "provider-ops-verify:sub2api",
        reqwest::Method::GET,
        &verify_url,
        &auth_headers,
        None,
        proxy_snapshot,
    )
    .await
    {
        Ok(result) => result,
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(message))
        | Err(AdminProviderOpsExecuteJsonError::Transport(message)) => {
            return admin_provider_ops_verify_failure(
                admin_provider_ops_verify_execution_error_message(&message),
            );
        }
    };

    parse_verify_payload(
        "sub2api",
        status,
        &response_json,
        frontend_updated_credentials,
    )
}

// 对齐 Python httpx.AsyncClient(base_url=...) 的行为:
// 以 "/" 开头的端点始终相对站点根路径解析，而不是简单字符串拼接。
pub(in super::super) fn admin_provider_ops_sub2api_request_url(
    base_url: &str,
    endpoint: &str,
) -> Result<String, String> {
    let destination = canonicalize_provider_ops_base_url(base_url).map_err(ToString::to_string)?;
    resolve_provider_ops_same_origin_url(&destination, endpoint).map_err(ToString::to_string)
}

fn admin_provider_ops_sub2api_updated_credentials(
    token_data: &Map<String, Value>,
    previous_refresh_token: Option<&str>,
) -> Map<String, Value> {
    let mut updated_credentials = Map::new();
    if let Some(new_refresh_token) = token_data
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if previous_refresh_token != Some(new_refresh_token) {
            updated_credentials.insert(
                "refresh_token".to_string(),
                Value::String(new_refresh_token.to_string()),
            );
        }
    }
    if let Some(access_token) = token_data
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        updated_credentials.insert(
            "_cached_access_token".to_string(),
            Value::String(access_token.to_string()),
        );
        updated_credentials.insert(
            "_cached_token_expires_at".to_string(),
            Value::from(admin_provider_ops_sub2api_cached_token_expires_at(
                token_data,
            )),
        );
    }
    updated_credentials
}

fn admin_provider_ops_sub2api_cached_token_expires_at(token_data: &Map<String, Value>) -> f64 {
    if let Some(token_expires_at) = token_data
        .get("token_expires_at")
        .and_then(admin_provider_ops_sub2api_json_number)
    {
        return token_expires_at / 1000.0 - 60.0;
    }
    let expires_in = token_data
        .get("expires_in")
        .and_then(admin_provider_ops_sub2api_json_number)
        .unwrap_or(900.0);
    admin_provider_ops_sub2api_unix_timestamp_secs() + expires_in - 60.0
}

fn admin_provider_ops_sub2api_json_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))
        .or_else(|| {
            value
                .as_str()
                .and_then(|value| value.trim().parse::<f64>().ok())
        })
}

fn admin_provider_ops_sub2api_unix_timestamp_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs_f64())
        .unwrap_or_default()
}

fn admin_provider_ops_sub2api_cached_access_token(
    credentials: &Map<String, Value>,
) -> Option<String> {
    let cached_access_token = credentials
        .get("_cached_access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let cached_expires_at = credentials
        .get("_cached_token_expires_at")
        .and_then(admin_provider_ops_sub2api_json_number)
        .unwrap_or_default();
    if admin_provider_ops_sub2api_unix_timestamp_secs() >= cached_expires_at {
        return None;
    }
    Some(cached_access_token.to_string())
}

async fn admin_provider_ops_sub2api_token_request(
    state: &AdminAppState<'_>,
    base_url: &str,
    path: &str,
    body: Value,
    default_error: &str,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Result<Map<String, Value>, String> {
    let url = admin_provider_ops_sub2api_request_url(base_url, path)?;
    let default_headers = reqwest::header::HeaderMap::from_iter([
        (
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static(ADMIN_PROVIDER_OPS_USER_AGENT),
        ),
        (
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("*/*"),
        ),
    ]);
    let default_headers =
        admin_provider_ops_headers_with_transport_controls(&default_headers, None, true);
    let (status, response_json) = match admin_provider_ops_execute_json_request(
        state,
        &format!("provider-ops-sub2api:{path}"),
        reqwest::Method::POST,
        &url,
        &default_headers,
        Some(body),
        proxy_snapshot,
    )
    .await
    {
        Ok(result) => result,
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(message))
        | Err(AdminProviderOpsExecuteJsonError::Transport(message)) => {
            return Err(admin_provider_ops_verify_execution_error_message(&message));
        }
    };
    let payload = response_json.as_object().cloned().unwrap_or_default();
    if status != http::StatusCode::OK
        || payload.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0
    {
        // Upstream messages are untrusted response data and may echo credentials or
        // request headers.  Callers only need a stable local classification here;
        // never reflect the upstream text into verify responses, logs, or caches.
        return Err(default_error.to_string());
    }
    payload
        .get("data")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| "响应格式无效".to_string())
}

pub(in super::super) async fn admin_provider_ops_sub2api_exchange_token(
    state: &AdminAppState<'_>,
    base_url: &str,
    credentials: &Map<String, Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Result<(String, Map<String, Value>, Option<Map<String, Value>>), String> {
    if let Some(cached_access_token) = admin_provider_ops_sub2api_cached_access_token(credentials) {
        return Ok((cached_access_token, Map::new(), None));
    }

    let email = credentials
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let password = credentials
        .get("password")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let refresh_token = credentials
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();

    let mut refresh_error = None::<String>;
    let token_data = if !refresh_token.is_empty() {
        match admin_provider_ops_sub2api_token_request(
            state,
            base_url,
            "/api/v1/auth/refresh",
            json!({ "refresh_token": refresh_token }),
            "Refresh Token 无效或已过期",
            proxy_snapshot,
        )
        .await
        {
            Ok(token_data) => token_data,
            Err(err) => {
                if email.is_empty() || password.is_empty() {
                    return Err(err);
                }
                refresh_error = Some(err);
                admin_provider_ops_sub2api_token_request(
                    state,
                    base_url,
                    "/api/v1/auth/login",
                    json!({ "email": email, "password": password }),
                    "登录失败",
                    proxy_snapshot,
                )
                .await?
            }
        }
    } else if !email.is_empty() && !password.is_empty() {
        admin_provider_ops_sub2api_token_request(
            state,
            base_url,
            "/api/v1/auth/login",
            json!({ "email": email, "password": password }),
            "登录失败",
            proxy_snapshot,
        )
        .await?
    } else {
        return Err(refresh_error.unwrap_or_else(|| "请填写账号密码或 Refresh Token".to_string()));
    };

    let access_token = token_data
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "响应格式无效".to_string())?;

    let updated_credentials =
        admin_provider_ops_sub2api_updated_credentials(&token_data, Some(refresh_token));

    Ok((
        access_token.to_string(),
        updated_credentials.clone(),
        admin_provider_ops_frontend_updated_credentials(updated_credentials),
    ))
}
