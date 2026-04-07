use super::builders::build_admin_endpoint_payload;
use super::extractors::admin_endpoint_id;
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

pub(super) async fn maybe_handle(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("get_endpoint")
        || !request_context
            .request_path
            .starts_with("/api/admin/endpoints/")
    {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() {
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

    Ok(Some(
        match build_admin_endpoint_payload(state, &endpoint_id).await {
            Some(payload) => Json(payload).into_response(),
            None => (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Endpoint {endpoint_id} 不存在") })),
            )
                .into_response(),
        },
    ))
}
