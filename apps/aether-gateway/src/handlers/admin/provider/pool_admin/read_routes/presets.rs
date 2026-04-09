use aether_admin::provider::pool as admin_provider_pool_pure;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};

pub(super) fn build_admin_pool_scheduling_presets_response() -> Response<Body> {
    Json(admin_provider_pool_pure::build_admin_pool_scheduling_presets_payload()).into_response()
}
