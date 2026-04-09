use super::helpers::{
    admin_provider_ops_frontend_updated_credentials, admin_provider_ops_json_object,
    admin_provider_ops_sub2api_exchange_token, admin_provider_ops_value_as_f64,
    admin_provider_ops_verify_failure, admin_provider_ops_verify_success,
    admin_provider_ops_verify_user_payload,
};
use crate::handlers::admin::request::AdminAppState;
use aether_admin::provider::verify as admin_provider_verify_pure;
use http;
use serde_json::{json, Map, Value};

pub(super) fn admin_provider_ops_generic_verify_payload(
    status: http::StatusCode,
    response_json: &Value,
) -> Value {
    admin_provider_verify_pure::admin_provider_ops_generic_verify_payload(status, response_json)
}

pub(super) fn admin_provider_ops_cubence_verify_payload(
    status: http::StatusCode,
    response_json: &Value,
) -> Value {
    admin_provider_verify_pure::admin_provider_ops_cubence_verify_payload(status, response_json)
}

pub(super) fn admin_provider_ops_yescode_verify_payload(
    status: http::StatusCode,
    response_json: &Value,
) -> Value {
    admin_provider_verify_pure::admin_provider_ops_yescode_verify_payload(status, response_json)
}

pub(super) fn admin_provider_ops_nekocode_verify_payload(
    status: http::StatusCode,
    response_json: &Value,
) -> Value {
    admin_provider_verify_pure::admin_provider_ops_nekocode_verify_payload(status, response_json)
}

pub(super) fn admin_provider_ops_sub2api_verify_payload(
    status: http::StatusCode,
    response_json: &Value,
    updated_credentials: Option<Map<String, Value>>,
) -> Value {
    admin_provider_verify_pure::admin_provider_ops_sub2api_verify_payload(
        status,
        response_json,
        updated_credentials,
    )
}

pub(super) async fn admin_provider_ops_local_sub2api_verify_response(
    state: &AdminAppState<'_>,
    base_url: &str,
    credentials: &Map<String, Value>,
) -> Value {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return admin_provider_ops_verify_failure("请提供 API 地址");
    }

    let (access_token, updated_credentials) =
        match admin_provider_ops_sub2api_exchange_token(state, base_url, credentials).await {
            Ok(value) => value,
            Err(message) => return admin_provider_ops_verify_failure(message),
        };

    let response = match state
        .http_client()
        .get(format!("{base_url}/api/v1/auth/me?timezone=Asia/Shanghai"))
        .bearer_auth(access_token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) if err.is_timeout() => return admin_provider_ops_verify_failure("连接超时"),
        Err(err) if err.is_connect() => {
            return admin_provider_ops_verify_failure(format!("连接失败: {err}"));
        }
        Err(err) => return admin_provider_ops_verify_failure(format!("验证失败: {err}")),
    };

    let status = response.status();
    let response_json = match response.bytes().await {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| json!({})),
        Err(_) => json!({}),
    };
    admin_provider_ops_sub2api_verify_payload(status, &response_json, updated_credentials)
}
