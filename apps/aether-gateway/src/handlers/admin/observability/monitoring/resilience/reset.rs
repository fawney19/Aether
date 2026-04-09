use super::snapshot::build_admin_monitoring_resilience_snapshot;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::monitoring::build_admin_monitoring_reset_error_stats_payload_response;
use axum::{body::Body, response::Response};

pub(in super::super) async fn build_admin_monitoring_reset_error_stats_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let snapshot = build_admin_monitoring_resilience_snapshot(state).await?;
    let reset_at = chrono::Utc::now();
    state.mark_admin_monitoring_error_stats_reset(reset_at.timestamp().max(0) as u64);

    let reset_by = if let Some(user_id) = request_context
        .control_decision
        .as_ref()
        .and_then(|decision| decision.admin_principal.as_ref())
        .map(|principal| principal.user_id.clone())
    {
        state
            .find_user_auth_by_id(&user_id)
            .await?
            .and_then(|user| user.email.or(Some(user.username)))
            .or(Some(user_id))
    } else {
        None
    };

    Ok(build_admin_monitoring_reset_error_stats_payload_response(
        snapshot.previous_stats,
        reset_by,
        reset_at,
    ))
}
