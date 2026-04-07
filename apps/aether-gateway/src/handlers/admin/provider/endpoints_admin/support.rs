use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) const ADMIN_ENDPOINTS_DATA_UNAVAILABLE_DETAIL: &str = "Admin endpoint data unavailable";

pub(super) fn build_admin_endpoints_data_unavailable_response() -> Response<Body> {
    (
        http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "detail": ADMIN_ENDPOINTS_DATA_UNAVAILABLE_DETAIL })),
    )
        .into_response()
}
