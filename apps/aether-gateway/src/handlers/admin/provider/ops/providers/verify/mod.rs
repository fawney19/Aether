mod headers;
mod helpers;
mod payload;

use crate::handlers::admin::provider::ops::providers::support::ADMIN_PROVIDER_OPS_VERIFY_RUST_ONLY_MESSAGE;
use crate::handlers::admin::request::AdminAppState;
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
};
use serde_json::json;

pub(super) fn admin_provider_ops_verify_headers(
    architecture_id: &str,
    config: &serde_json::Map<String, serde_json::Value>,
    credentials: &serde_json::Map<String, serde_json::Value>,
) -> Result<reqwest::header::HeaderMap, String> {
    headers::admin_provider_ops_verify_headers(architecture_id, config, credentials)
}

pub(super) fn admin_provider_ops_normalized_verify_architecture_id(architecture_id: &str) -> &str {
    helpers::admin_provider_ops_normalized_verify_architecture_id(architecture_id)
}

pub(super) fn admin_provider_ops_verify_failure(message: impl Into<String>) -> serde_json::Value {
    helpers::admin_provider_ops_verify_failure(message)
}

pub(super) fn admin_provider_ops_value_as_f64(value: Option<&serde_json::Value>) -> Option<f64> {
    helpers::admin_provider_ops_value_as_f64(value)
}

pub(super) async fn admin_provider_ops_local_verify_response(
    state: &AdminAppState<'_>,
    base_url: &str,
    architecture_id: &str,
    config: &serde_json::Map<String, serde_json::Value>,
    credentials: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    if architecture_id == "sub2api" {
        return payload::admin_provider_ops_local_sub2api_verify_response(
            state,
            base_url,
            credentials,
        )
        .await;
    }

    let mut resolved_config = config.clone();
    if architecture_id == "anyrouter" {
        if let Some(acw_cookie) =
            helpers::admin_provider_ops_anyrouter_acw_cookie(state, base_url).await
        {
            resolved_config.insert(
                "acw_cookie".to_string(),
                serde_json::Value::String(acw_cookie),
            );
        }
    }

    let verify_path = match architecture_id {
        "anyrouter" => "/api/user/self",
        "cubence" => "/api/v1/dashboard/overview",
        "yescode" => "/api/v1/auth/profile",
        "nekocode" => "/api/user/self",
        "new_api" | "generic_api" => "/api/user/self",
        _ => {
            return helpers::admin_provider_ops_verify_failure(
                ADMIN_PROVIDER_OPS_VERIFY_RUST_ONLY_MESSAGE,
            )
        }
    };
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return helpers::admin_provider_ops_verify_failure("请提供 API 地址");
    }

    let headers = match headers::admin_provider_ops_verify_headers(
        architecture_id,
        &resolved_config,
        credentials,
    ) {
        Ok(headers) => headers,
        Err(message) => return helpers::admin_provider_ops_verify_failure(message),
    };

    let response = match state
        .http_client()
        .get(format!("{base_url}{verify_path}"))
        .headers(headers)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) if err.is_timeout() => {
            return helpers::admin_provider_ops_verify_failure("连接超时")
        }
        Err(err) if err.is_connect() => {
            return helpers::admin_provider_ops_verify_failure(format!("连接失败: {err}"))
        }
        Err(err) => return helpers::admin_provider_ops_verify_failure(format!("验证失败: {err}")),
    };

    let status = response.status();
    let response_json = match response.bytes().await {
        Ok(bytes) => {
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_else(|_| json!({}))
        }
        Err(_) => json!({}),
    };

    match architecture_id {
        "cubence" => payload::admin_provider_ops_cubence_verify_payload(status, &response_json),
        "yescode" => payload::admin_provider_ops_yescode_verify_payload(status, &response_json),
        "nekocode" => payload::admin_provider_ops_nekocode_verify_payload(status, &response_json),
        _ => payload::admin_provider_ops_generic_verify_payload(status, &response_json),
    }
}
