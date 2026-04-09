use crate::handlers::admin::shared::build_proxy_error_response;
use axum::{body::Body, http, response::Response};
use serde_json::json;

pub(crate) const ADMIN_PROVIDER_STRATEGY_DATA_UNAVAILABLE_DETAIL: &str =
    "Admin provider strategy data unavailable";
pub(crate) const ADMIN_PROVIDER_STRATEGY_STATS_DATA_UNAVAILABLE_DETAIL: &str =
    "Admin provider strategy stats data unavailable";

pub(crate) fn admin_provider_strategy_data_unavailable_response(detail: &str) -> Response<Body> {
    build_proxy_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        "data_unavailable",
        detail,
        Some(json!({ "error": detail })),
    )
}
