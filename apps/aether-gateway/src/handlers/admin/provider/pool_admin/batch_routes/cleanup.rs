use super::{
    build_admin_pool_error_response, ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
    ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
};
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use axum::{body::Body, http, response::Response};

pub(super) async fn build_admin_pool_cleanup_banned_keys_response(
    state: &AdminAppState<'_>,
    provider_id: String,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
        ));
    }
    if !state.has_provider_catalog_data_writer() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
        ));
    }

    state
        .build_admin_pool_cleanup_banned_keys_response(&provider_id)
        .await
}
