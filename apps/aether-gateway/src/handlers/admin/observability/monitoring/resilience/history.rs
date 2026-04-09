use super::snapshot::build_admin_monitoring_provider_name_by_id_and_keys;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response, build_admin_monitoring_circuit_history_items,
    build_admin_monitoring_circuit_history_payload_response,
    parse_admin_monitoring_circuit_history_limit,
};
use axum::{body::Body, response::Response};

pub(in super::super) async fn build_admin_monitoring_resilience_circuit_history_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let limit = match parse_admin_monitoring_circuit_history_limit(
        request_context.request_query_string.as_deref(),
    ) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };

    let (provider_name_by_id, keys) =
        build_admin_monitoring_provider_name_by_id_and_keys(state).await?;
    let items = build_admin_monitoring_circuit_history_items(&keys, &provider_name_by_id, limit);
    Ok(build_admin_monitoring_circuit_history_payload_response(
        items,
    ))
}
