pub(crate) use crate::handlers::admin::{
    admin_provider_ops_local_action_response, admin_provider_pool_config,
    build_internal_control_error_response, maybe_build_local_admin_pool_response,
    maybe_build_local_admin_response, provider_oauth_runtime_endpoint_for_provider,
    refresh_antigravity_provider_quota_locally, refresh_codex_provider_quota_locally,
    refresh_kiro_provider_quota_locally, AdminAppState, AdminRequestContext, AdminRouteRequest,
    AdminRouteResponse, AdminRouteResult, AdminStatsTimeRange, AdminStatsUsageFilter,
};

use crate::handlers::admin::{
    admin_stats_bad_request_response as admin_stats_bad_request_response_impl,
    parse_bounded_u32 as parse_bounded_u32_impl, round_to as round_to_impl,
};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    response::Response,
};

pub(crate) async fn maybe_build_local_admin_security_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    crate::handlers::admin::maybe_build_local_admin_security_response(
        state,
        request_context,
        request_body,
    )
    .await
}

pub(crate) async fn build_admin_endpoint_health_status_payload(
    state: &AdminAppState<'_>,
    lookback_hours: u64,
) -> Option<serde_json::Value> {
    crate::handlers::admin::build_admin_endpoint_health_status_payload(state, lookback_hours).await
}

pub(crate) async fn maybe_build_local_admin_video_tasks_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    crate::handlers::admin::maybe_build_local_admin_video_tasks_response(state, request_context)
        .await
}

pub(crate) async fn maybe_build_local_admin_usage_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    crate::handlers::admin::maybe_build_local_admin_usage_response(
        state,
        request_context,
        request_body,
    )
    .await
}

pub(crate) fn admin_stats_bad_request_response(detail: String) -> Response<Body> {
    admin_stats_bad_request_response_impl(detail)
}

pub(crate) fn parse_bounded_u32(
    field: &str,
    value: &str,
    min: u32,
    max: u32,
) -> Result<u32, String> {
    parse_bounded_u32_impl(field, value, min, max)
}

pub(crate) fn round_to(value: f64, decimals: u32) -> f64 {
    round_to_impl(value, decimals)
}

pub(crate) async fn maybe_build_local_admin_provider_oauth_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    crate::handlers::admin::maybe_build_local_admin_provider_oauth_response(
        state,
        request_context,
        request_body,
    )
    .await
}

pub(crate) async fn maybe_build_local_admin_providers_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    crate::handlers::admin::maybe_build_local_admin_providers_response(
        state,
        request_context,
        request_body,
    )
    .await
}
