use super::extractors::admin_endpoint_id;
use super::support::build_admin_endpoints_data_unavailable_response;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::{
    attach_admin_audit_response, mark_sensitive_admin_response_no_store,
};
use crate::GatewayError;
use axum::{
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_handle(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };
    if decision.route_family.as_deref() != Some("endpoints_manage")
        || decision.route_kind.as_deref() != Some("reveal_endpoint_rules")
    {
        return Ok(None);
    }
    if !state.has_provider_catalog_data_reader() {
        return Ok(Some(build_admin_endpoints_data_unavailable_response()));
    }
    let Some(endpoint_id) = request_context
        .path()
        .strip_suffix("/rules/reveal")
        .and_then(admin_endpoint_id)
    else {
        return Ok(Some(
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "detail": "Endpoint 不存在" })),
            )
                .into_response(),
        ));
    };
    let Some(endpoint) = state
        .read_provider_catalog_endpoints_by_ids(std::slice::from_ref(&endpoint_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(Some(
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "detail": "Endpoint 不存在" })),
            )
                .into_response(),
        ));
    };
    let payload = json!({
        "header_rules": endpoint.header_rules.as_ref().and_then(|value| value.as_array()).cloned().unwrap_or_default(),
        "body_rules": endpoint.body_rules.as_ref().and_then(|value| value.as_array()).cloned().unwrap_or_default(),
        "response_header_rules": endpoint.config.as_ref().and_then(|config| config.get("response_header_rules")).and_then(|value| value.as_array()).cloned().unwrap_or_default(),
    });
    Ok(Some(mark_sensitive_admin_response_no_store(
        attach_admin_audit_response(
            Json(payload).into_response(),
            "admin_endpoint_rules_revealed",
            "reveal_endpoint_rules",
            "provider_endpoint",
            &endpoint_id,
        ),
    )))
}
