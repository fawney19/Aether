use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::stats::{
    admin_stats_provider_quota_usage_empty_response,
    build_admin_stats_provider_quota_usage_response,
};
use axum::{body::Body, http, response::Response};
use chrono::Utc;

pub(super) async fn maybe_build_local_admin_stats_provider_quota_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Option<Response<Body>>, GatewayError> {
    if request_context.route_kind() != Some("provider_quota_usage")
        || request_context.method() != http::Method::GET
        || !matches!(
            request_context.path(),
            "/api/admin/stats/providers/quota-usage" | "/api/admin/stats/providers/quota-usage/"
        )
    {
        return Ok(None);
    }

    if !state.has_provider_catalog_data_reader() {
        return Ok(Some(admin_stats_provider_quota_usage_empty_response()));
    }

    let providers = state.list_provider_catalog_providers(false).await?;
    Ok(Some(build_admin_stats_provider_quota_usage_response(
        &providers,
        Utc::now(),
    )))
}
