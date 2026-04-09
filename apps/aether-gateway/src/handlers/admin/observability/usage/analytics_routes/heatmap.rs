use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_admin::observability::usage::{
    admin_usage_data_unavailable_response, admin_usage_heatmap_json,
    ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
};
use aether_data_contracts::repository::usage::UsageAuditListQuery;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn build_admin_usage_heatmap_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_usage_data_reader() {
        return Ok(admin_usage_data_unavailable_response(
            ADMIN_USAGE_DATA_UNAVAILABLE_DETAIL,
        ));
    }
    let now_unix_secs = u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default();
    let created_from_unix_secs = now_unix_secs.saturating_sub(365 * 24 * 3600);
    let mut usage = state
        .list_usage_audits(&UsageAuditListQuery {
            created_from_unix_secs: Some(created_from_unix_secs),
            ..Default::default()
        })
        .await?;
    usage.retain(|item| item.status != "pending" && item.status != "streaming");
    Ok(Json(admin_usage_heatmap_json(&usage)).into_response())
}
