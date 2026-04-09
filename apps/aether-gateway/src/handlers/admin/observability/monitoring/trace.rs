use super::route_filters::parse_admin_monitoring_limit;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::log_ids::short_request_id;
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response, admin_monitoring_trace_not_found_response,
    admin_monitoring_trace_provider_id_from_path, admin_monitoring_trace_request_id_from_path,
    build_admin_monitoring_trace_provider_stats_payload_response,
    build_admin_monitoring_trace_request_payload_response, parse_admin_monitoring_attempted_only,
};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
};
use tracing::warn;

pub(super) async fn build_admin_monitoring_trace_request_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let state = state.as_ref();
    let Some(request_id) =
        admin_monitoring_trace_request_id_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response("缺少 request_id"));
    };
    let attempted_only = match parse_admin_monitoring_attempted_only(
        request_context.request_query_string.as_deref(),
    ) {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };

    let Some(trace) = state
        .data
        .read_decision_trace(&request_id, attempted_only)
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))?
    else {
        warn!(
            event_name = "admin_monitoring_request_trace_not_found",
            log_type = "admin_monitoring",
            request_id = %short_request_id(request_id.as_str()),
            attempted_only,
            path = %request_context.request_path,
            "admin monitoring request trace not found"
        );
        return Ok(admin_monitoring_trace_not_found_response(
            &request_id,
            attempted_only,
        ));
    };

    Ok(build_admin_monitoring_trace_request_payload_response(
        &trace,
    ))
}

pub(super) async fn build_admin_monitoring_trace_provider_stats_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let state = state.as_ref();
    let Some(provider_id) =
        admin_monitoring_trace_provider_id_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response("缺少 provider_id"));
    };
    let limit = match parse_admin_monitoring_limit(request_context.request_query_string.as_deref())
    {
        Ok(value) => value,
        Err(detail) => return Ok(admin_monitoring_bad_request_response(detail)),
    };

    let candidates = state
        .read_request_candidates_by_provider_id(&provider_id, limit)
        .await?;
    let total_attempts = candidates.len();
    let success_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Success)
        .count();
    let failed_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Failed)
        .count();
    let cancelled_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Cancelled)
        .count();
    let skipped_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Skipped)
        .count();
    let pending_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Pending)
        .count();
    let available_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Available)
        .count();
    let unused_count = candidates
        .iter()
        .filter(|item| item.status == RequestCandidateStatus::Unused)
        .count();
    let completed_count = success_count + failed_count;
    let failure_rate = if completed_count == 0 {
        0.0
    } else {
        ((failed_count as f64 / completed_count as f64) * 10000.0).round() / 100.0
    };
    let latency_values = candidates
        .iter()
        .filter_map(|item| item.latency_ms.map(|value| value as f64))
        .collect::<Vec<_>>();
    let avg_latency_ms = if latency_values.is_empty() {
        0.0
    } else {
        let total = latency_values.iter().sum::<f64>();
        ((total / latency_values.len() as f64) * 100.0).round() / 100.0
    };

    Ok(
        build_admin_monitoring_trace_provider_stats_payload_response(
            provider_id,
            total_attempts,
            success_count,
            failed_count,
            cancelled_count,
            skipped_count,
            pending_count,
            available_count,
            unused_count,
            failure_rate,
            avg_latency_ms,
        ),
    )
}
