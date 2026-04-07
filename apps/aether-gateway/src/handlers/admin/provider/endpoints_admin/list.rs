use super::builders::build_admin_provider_endpoints_payload;
use super::extractors::admin_provider_id_for_endpoints;
use super::support::build_admin_endpoints_data_unavailable_response;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::shared::query_param_value;
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
        || decision.route_kind.as_deref() != Some("list_provider_endpoints")
        || !request_context
            .request_path
            .starts_with("/api/admin/endpoints/providers/")
        || !request_context.request_path.ends_with("/endpoints")
    {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() {
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
    let skip = query_param_value(request_context.request_query_string.as_deref(), "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(request_context.request_query_string.as_deref(), "limit")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(100);

    Ok(Some(
        match build_admin_provider_endpoints_payload(state, &provider_id, skip, limit).await {
            Some(payload) => Json(payload).into_response(),
            None => (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
            )
                .into_response(),
        },
    ))
}
