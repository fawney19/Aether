use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) const ADMIN_PROVIDER_QUERY_INVALID_JSON_DETAIL: &str = "Invalid JSON request body";
pub(super) const ADMIN_PROVIDER_QUERY_PROVIDER_ID_REQUIRED_DETAIL: &str = "provider_id is required";
pub(super) const ADMIN_PROVIDER_QUERY_MODEL_REQUIRED_DETAIL: &str = "model is required";
pub(super) const ADMIN_PROVIDER_QUERY_FAILOVER_MODELS_REQUIRED_DETAIL: &str =
    "failover_models should not be empty";
pub(super) const ADMIN_PROVIDER_QUERY_PROVIDER_NOT_FOUND_DETAIL: &str = "Provider not found";
pub(super) const ADMIN_PROVIDER_QUERY_API_KEY_NOT_FOUND_DETAIL: &str = "API Key not found";
pub(super) const ADMIN_PROVIDER_QUERY_NO_ACTIVE_API_KEY_DETAIL: &str =
    "No active API Key found for this provider";
pub(super) const ADMIN_PROVIDER_QUERY_NO_LOCAL_MODELS_DETAIL: &str =
    "No models available from local provider catalog";

pub(super) fn build_admin_provider_query_bad_request_response(
    detail: &'static str,
) -> Response<Body> {
    (
        http::StatusCode::BAD_REQUEST,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}

pub(super) fn build_admin_provider_query_not_found_response(
    detail: &'static str,
) -> Response<Body> {
    (
        http::StatusCode::NOT_FOUND,
        Json(json!({ "detail": detail })),
    )
        .into_response()
}
