use super::builders::build_admin_create_provider_endpoint_record;
use super::extractors::admin_provider_id_for_endpoints;
use super::payloads::{build_admin_provider_endpoint_response, AdminProviderEndpointCreateRequest};
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
        || decision.route_kind.as_deref() != Some("create_endpoint")
        || request_context.request_method != http::Method::POST
        || !request_context
            .request_path
            .starts_with("/api/admin/endpoints/providers/")
        || !request_context.request_path.ends_with("/endpoints")
    {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
        return Ok(Some(build_admin_endpoints_data_unavailable_response()));
    }

    let Some(provider_id) = admin_provider_id_for_endpoints(&request_context.request_path) else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "Provider 不存在" })),
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
    let payload = match serde_json::from_slice::<AdminProviderEndpointCreateRequest>(request_body) {
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
    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
            )
                .into_response(),
        ));
    };
    let record = match build_admin_create_provider_endpoint_record(state, &provider, payload).await
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
    let Some(created) = state.create_provider_catalog_endpoint(&record).await? else {
        return Ok(Some(build_admin_endpoints_data_unavailable_response()));
    };
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    Ok(Some(
        Json(build_admin_provider_endpoint_response(
            &created,
            &provider.name,
            0,
            0,
            now_unix_secs,
        ))
        .into_response(),
    ))
}
