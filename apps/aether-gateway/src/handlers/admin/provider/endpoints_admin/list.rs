use super::extractors::admin_provider_id_for_endpoints;
use super::reads::build_admin_provider_endpoints_payload;
use super::support::build_admin_endpoints_data_unavailable_response;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::query_param_value;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    _request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("list_provider_endpoints")
        || !request_context
            .path()
            .starts_with("/api/admin/endpoints/providers/")
        || !request_context.path().ends_with("/endpoints")
    {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() {
        return Ok(Some(build_admin_endpoints_data_unavailable_response()));
    }

    let Some(provider_id) = admin_provider_id_for_endpoints(request_context.path()) else {
        return Ok(Some(
            (
                http::StatusCode::NOT_FOUND,
                Json(json!({ "detail": "Provider 不存在" })),
            )
                .into_response(),
        ));
    };
    let skip = query_param_value(request_context.query_string(), "skip")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let limit = query_param_value(request_context.query_string(), "limit")
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
