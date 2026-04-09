use super::super::config::{
    admin_provider_ops_config_object, admin_provider_ops_connector_object,
    admin_provider_ops_decrypted_credentials, resolve_admin_provider_ops_base_url,
};
use super::super::support::{
    AdminProviderOpsConnectRequest, ADMIN_PROVIDER_OPS_CONNECT_RUST_ONLY_MESSAGE,
};
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn handle_admin_provider_ops_connect(
    state: &AdminAppState<'_>,
    provider_id: &str,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let payload = match parse_json_object_payload::<AdminProviderOpsConnectRequest>(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let provider_ids = [provider_id.to_string()];
    let Some(existing_provider) = state
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(bad_request_detail_response("Provider 不存在"));
    };
    let Some(provider_ops_config) = admin_provider_ops_config_object(&existing_provider) else {
        return Ok(bad_request_detail_response("未配置操作设置"));
    };
    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(&provider_ids)
        .await?;
    if resolve_admin_provider_ops_base_url(
        &existing_provider,
        &endpoints,
        Some(provider_ops_config),
    )
    .is_none()
    {
        return Ok(bad_request_detail_response("Provider 未配置 base_url"));
    }

    let actual_credentials = payload
        .credentials
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            admin_provider_ops_decrypted_credentials(
                state,
                admin_provider_ops_config_object(&existing_provider)
                    .and_then(admin_provider_ops_connector_object)
                    .and_then(|connector| connector.get("credentials")),
            )
        });
    if actual_credentials.is_empty() {
        return Ok(bad_request_detail_response("未提供凭据"));
    }

    Ok((
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": ADMIN_PROVIDER_OPS_CONNECT_RUST_ONLY_MESSAGE })),
    )
        .into_response())
}

pub(super) fn handle_admin_provider_ops_disconnect() -> Response<Body> {
    Json(json!({
        "success": true,
        "message": "已断开连接",
    }))
    .into_response()
}

fn parse_json_object_payload<T>(request_body: Option<&Bytes>) -> Result<T, Response<Body>>
where
    T: serde::de::DeserializeOwned,
{
    let Some(request_body) = request_body else {
        return Err(bad_request_detail_response("请求体不能为空"));
    };
    let raw_value = serde_json::from_slice::<serde_json::Value>(request_body)
        .map_err(|_| bad_request_detail_response("请求体必须是合法的 JSON 对象"))?;
    if !raw_value.is_object() {
        return Err(bad_request_detail_response("请求体必须是合法的 JSON 对象"));
    }
    serde_json::from_value::<T>(raw_value)
        .map_err(|_| bad_request_detail_response("请求体必须是合法的 JSON 对象"))
}

fn bad_request_detail_response(detail: &str) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}
