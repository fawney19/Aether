use super::extractors::admin_endpoint_id;
use super::payloads::{
    build_admin_provider_endpoint_response, endpoint_key_counts_by_format,
    AdminProviderEndpointUpdatePatch,
};
use super::support::build_admin_endpoints_data_unavailable_response;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("update_endpoint")
        || request_context.method() != http::Method::PUT
        || !request_context.path().starts_with("/api/admin/endpoints/")
    {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(Some(build_admin_endpoints_data_unavailable_response()));
    }

    let Some(endpoint_id) = admin_endpoint_id(request_context.path()) else {
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
    let patch = match AdminProviderEndpointUpdatePatch::from_object(raw_payload) {
        Ok(patch) => patch,
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
    let updated_record = match state
        .build_admin_update_provider_endpoint_record(&provider, &existing_endpoint, patch)
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
