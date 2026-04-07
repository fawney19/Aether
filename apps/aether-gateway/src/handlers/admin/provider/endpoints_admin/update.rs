use super::builders::build_admin_update_provider_endpoint_record;
use super::extractors::admin_endpoint_id;
use super::payloads::{
    build_admin_provider_endpoint_response, endpoint_key_counts_by_format,
    AdminProviderEndpointUpdateRequest,
};
use super::support::build_admin_endpoints_data_unavailable_response;
use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_handle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("update_endpoint")
        || request_context.request_method != http::Method::PUT
        || !request_context
            .request_path
            .starts_with("/api/admin/endpoints/")
    {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(Some(build_admin_endpoints_data_unavailable_response()));
    }

    let Some(endpoint_id) = admin_endpoint_id(&request_context.request_path) else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "Endpoint 不存在" })),
            )
                .into_response(),
        ));
    };
    let Some(request_body) = request_body else {
        return Ok(Some(
            (
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "请求体不能为空" })),
            )
                .into_response(),
        ));
    };
    let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
        Ok(value) => value,
        Err(_) => {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                )
                    .into_response(),
            ));
        }
    };
    let Some(raw_payload) = raw_value.as_object().cloned() else {
        return Ok(Some(
            (
                http::StatusCode::BAD_REQUEST,
                Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
            )
                .into_response(),
        ));
    };
    let payload = match serde_json::from_value::<AdminProviderEndpointUpdateRequest>(raw_value) {
        Ok(payload) => payload,
        Err(_) => {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                )
                    .into_response(),
            ));
        }
    };
    let Some(existing_endpoint) = state
        .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(&endpoint_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Endpoint {endpoint_id} 不存在") })),
            )
                .into_response(),
        ));
    };
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(
            &existing_endpoint.provider_id,
        ))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Provider {} 不存在", existing_endpoint.provider_id) })),
            )
                .into_response(),
        ));
    };
    let updated_record = match build_admin_update_provider_endpoint_record(
        state,
        &provider,
        &existing_endpoint,
        &raw_payload,
        payload,
    )
    .await
    {
        Ok(record) => record,
        Err(detail) => {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": detail })),
                )
                    .into_response(),
            ));
        }
    };
    let Some(updated) = state
        .update_provider_catalog_endpoint(&updated_record)
        .await?
    else {
        return Ok(Some(build_admin_endpoints_data_unavailable_response()));
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await
        .unwrap_or_default();
    let (total_keys_by_format, active_keys_by_format) = endpoint_key_counts_by_format(&keys);

    Ok(Some(
        Json(build_admin_provider_endpoint_response(
            &updated,
            &provider.name,
            total_keys_by_format
                .get(updated.api_format.as_str())
                .copied()
                .unwrap_or(0),
            active_keys_by_format
                .get(updated.api_format.as_str())
                .copied()
                .unwrap_or(0),
            now_unix_secs,
        ))
        .into_response(),
    ))
}
