use super::snapshot::build_admin_monitoring_resilience_snapshot;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::observability::monitoring::build_admin_monitoring_resilience_status_payload_response;
use axum::{body::Body, response::Response};

pub(in super::super) async fn build_admin_monitoring_resilience_status_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    let snapshot = build_admin_monitoring_resilience_snapshot(state).await?;

    Ok(build_admin_monitoring_resilience_status_payload_response(
        snapshot.timestamp,
        snapshot.health_score,
        snapshot.status,
        snapshot.error_statistics,
        snapshot.recent_errors,
        snapshot.recommendations,
    ))
}
