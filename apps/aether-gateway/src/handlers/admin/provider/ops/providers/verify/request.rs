use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_contracts::{
    ExecutionPlan, ExecutionResult, ExecutionTimeouts, ProxySnapshot, RequestBody,
    EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER, EXECUTION_REQUEST_HTTP1_ONLY_HEADER,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const ADMIN_PROVIDER_OPS_VERIFY_TIMEOUT_MS: u64 = 30_000;
const ADMIN_PROVIDER_OPS_RESPONSE_BODY_LIMIT_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct AdminProviderOpsTextResponse {
    pub(super) body: String,
}

pub(in super::super) enum AdminProviderOpsExecuteJsonError {
    InvalidJson(String),
    Transport(String),
}

pub(super) async fn admin_provider_ops_execute_get_json(
    state: &AdminAppState<'_>,
    request_id: &str,
    url: &str,
    headers: &reqwest::header::HeaderMap,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Result<(http::StatusCode, Value), String> {
    match admin_provider_ops_execute_json_request(
        state,
        request_id,
        reqwest::Method::GET,
        url,
        headers,
        None,
        proxy_snapshot,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(message))
        | Err(AdminProviderOpsExecuteJsonError::Transport(message)) => Err(message),
    }
}

pub(in super::super) async fn admin_provider_ops_execute_json_request(
    state: &AdminAppState<'_>,
    request_id: &str,
    method: reqwest::Method,
    url: &str,
    headers: &reqwest::header::HeaderMap,
    json_body: Option<Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Result<(http::StatusCode, Value), AdminProviderOpsExecuteJsonError> {
    let result = admin_provider_ops_execute_request(
        state,
        request_id,
        method,
        url,
        headers,
        json_body,
        proxy_snapshot,
    )
    .await
    .map_err(AdminProviderOpsExecuteJsonError::Transport)?;
    admin_provider_ops_execution_json_response(&result)
}

pub(in super::super) async fn admin_provider_ops_execute_proxy_json_request(
    state: &AdminAppState<'_>,
    request_id: &str,
    method: reqwest::Method,
    url: &str,
    headers: &reqwest::header::HeaderMap,
    json_body: Option<Value>,
    proxy_snapshot: &ProxySnapshot,
) -> Result<(http::StatusCode, Value), String> {
    match admin_provider_ops_execute_json_request(
        state,
        request_id,
        method,
        url,
        headers,
        json_body,
        Some(proxy_snapshot),
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(AdminProviderOpsExecuteJsonError::InvalidJson(message))
        | Err(AdminProviderOpsExecuteJsonError::Transport(message)) => Err(message),
    }
}

pub(super) async fn admin_provider_ops_execute_get_text(
    state: &AdminAppState<'_>,
    request_id: &str,
    url: &str,
    headers: &reqwest::header::HeaderMap,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Result<AdminProviderOpsTextResponse, String> {
    let result = admin_provider_ops_execute_request(
        state,
        request_id,
        reqwest::Method::GET,
        url,
        headers,
        None,
        proxy_snapshot,
    )
    .await?;
    let body = result
        .body
        .as_ref()
        .and_then(|body| admin_provider_ops_execution_body_bytes(&result.headers, body))
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
                .and_then(|value| serde_json::to_string(value).ok())
        })
        .unwrap_or_default();
    Ok(AdminProviderOpsTextResponse { body })
}

pub(super) async fn admin_provider_ops_execute_get_text_no_redirect(
    state: &AdminAppState<'_>,
    request_id: &str,
    url: &str,
    headers: &reqwest::header::HeaderMap,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Result<AdminProviderOpsTextResponse, String> {
    admin_provider_ops_execute_get_text(
        state,
        request_id,
        url,
        &admin_provider_ops_headers_with_transport_controls(headers, Some(false), false),
        proxy_snapshot,
    )
    .await
}

async fn admin_provider_ops_execute_request(
    state: &AdminAppState<'_>,
    request_id: &str,
    method: reqwest::Method,
    url: &str,
    headers: &reqwest::header::HeaderMap,
    json_body: Option<Value>,
    proxy_snapshot: Option<&ProxySnapshot>,
) -> Result<ExecutionResult, String> {
    let has_json_body = json_body.is_some();
    let body = json_body
        .map(RequestBody::from_json)
        .unwrap_or(RequestBody {
            json_body: None,
            body_bytes_b64: None,
            body_ref: None,
        });
    let plan = ExecutionPlan {
        request_id: request_id.to_string(),
        candidate_id: None,
        provider_name: Some("provider_ops".to_string()),
        provider_id: String::new(),
        endpoint_id: String::new(),
        key_id: String::new(),
        method: method.as_str().to_string(),
        url: url.to_string(),
        headers: admin_provider_ops_execution_headers(
            &admin_provider_ops_headers_with_transport_controls(headers, Some(false), false),
        ),
        content_type: has_json_body.then(|| "application/json".to_string()),
        content_encoding: None,
        body,
        stream: false,
        client_api_format: "provider_ops:verify".to_string(),
        provider_api_format: "provider_ops:verify".to_string(),
        model_name: Some("verify-auth".to_string()),
        proxy: proxy_snapshot.cloned(),
        transport_profile: None,
        timeouts: Some(ExecutionTimeouts {
            connect_ms: Some(ADMIN_PROVIDER_OPS_VERIFY_TIMEOUT_MS),
            read_ms: Some(ADMIN_PROVIDER_OPS_VERIFY_TIMEOUT_MS),
            write_ms: Some(ADMIN_PROVIDER_OPS_VERIFY_TIMEOUT_MS),
            pool_ms: Some(ADMIN_PROVIDER_OPS_VERIFY_TIMEOUT_MS),
            total_ms: Some(ADMIN_PROVIDER_OPS_VERIFY_TIMEOUT_MS),
            ..ExecutionTimeouts::default()
        }),
    };
    let bounded_plan = crate::execution_runtime::transport::with_upstream_response_body_limit(
        &plan,
        ADMIN_PROVIDER_OPS_RESPONSE_BODY_LIMIT_BYTES,
    );
    state
        .execute_execution_runtime_sync_plan(None, &bounded_plan)
        .await
        .map_err(admin_provider_ops_gateway_error_message)
}

fn admin_provider_ops_execution_headers(
    headers: &reqwest::header::HeaderMap,
) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|text| (name.as_str().to_string(), text.to_string()))
        })
        .collect()
}

pub(in super::super) fn admin_provider_ops_headers_with_transport_controls(
    headers: &reqwest::header::HeaderMap,
    follow_redirects: Option<bool>,
    http1_only: bool,
) -> reqwest::header::HeaderMap {
    let mut headers = headers.clone();
    if let Some(follow_redirects) = follow_redirects {
        let value = if follow_redirects { "true" } else { "false" };
        headers.insert(
            reqwest::header::HeaderName::from_static(EXECUTION_REQUEST_FOLLOW_REDIRECTS_HEADER),
            reqwest::header::HeaderValue::from_static(value),
        );
    }
    if http1_only {
        headers.insert(
            reqwest::header::HeaderName::from_static(EXECUTION_REQUEST_HTTP1_ONLY_HEADER),
            reqwest::header::HeaderValue::from_static("true"),
        );
    }
    headers
}

fn admin_provider_ops_execution_status_code(result: &ExecutionResult) -> http::StatusCode {
    http::StatusCode::from_u16(result.status_code).unwrap_or(http::StatusCode::BAD_GATEWAY)
}

fn admin_provider_ops_execution_json_response(
    result: &ExecutionResult,
) -> Result<(http::StatusCode, Value), AdminProviderOpsExecuteJsonError> {
    let status = admin_provider_ops_execution_status_code(result);
    if let Some(json_body) = result.body.as_ref().and_then(|body| body.json_body.clone()) {
        return Ok((status, json_body));
    }

    let Some(bytes) = result
        .body
        .as_ref()
        .and_then(|body| admin_provider_ops_execution_body_bytes(&result.headers, body))
    else {
        return Ok((status, json!({})));
    };

    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => Ok((status, value)),
        Err(_) if status != http::StatusCode::OK => Ok((status, json!({}))),
        Err(_) => Err(AdminProviderOpsExecuteJsonError::InvalidJson(
            "upstream response is not valid JSON".to_string(),
        )),
    }
}

fn admin_provider_ops_execution_body_bytes(
    headers: &BTreeMap<String, String>,
    body: &aether_contracts::ResponseBody,
) -> Option<Vec<u8>> {
    let bytes = body.body_bytes_b64.as_deref().and_then(|value| {
        crate::execution_runtime::transport::decode_base64_body_with_limit(
            value,
            ADMIN_PROVIDER_OPS_RESPONSE_BODY_LIMIT_BYTES,
        )
        .ok()
    })?;
    crate::execution_runtime::transport::decode_response_body_bytes_with_limit(
        headers,
        &bytes,
        ADMIN_PROVIDER_OPS_RESPONSE_BODY_LIMIT_BYTES,
    )
    .ok()
    .map(std::borrow::Cow::into_owned)
}

fn admin_provider_ops_gateway_error_message(error: GatewayError) -> String {
    admin_provider_ops_verify_execution_error_message(&error.into_message())
}

pub(super) fn admin_provider_ops_verify_execution_error_message(error: &str) -> String {
    let lower = error.trim().to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        return "连接超时".to_string();
    }
    if lower.contains("connect")
        || lower.contains("connection")
        || lower.contains("dns")
        || lower.contains("proxy")
        || lower.contains("relay")
    {
        return "连接失败".to_string();
    }
    "验证失败".to_string()
}

#[cfg(test)]
mod tests {
    use super::admin_provider_ops_verify_execution_error_message;

    #[test]
    fn provider_ops_transport_errors_do_not_echo_urls_or_credentials() {
        let raw = "connection failed for https://user:password@api.example.test/path?token=secret";
        let projected = admin_provider_ops_verify_execution_error_message(raw);
        assert_eq!(projected, "连接失败");
        for secret in ["user", "password", "token", "secret", "api.example.test"] {
            assert!(!projected.contains(secret), "leaked {secret}");
        }
    }
}
