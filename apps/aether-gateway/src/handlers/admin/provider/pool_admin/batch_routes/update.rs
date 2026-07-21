use super::{
    admin_pool_provider_id_from_path, build_admin_pool_error_response,
    ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
    ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
};
use crate::handlers::admin::provider::shared::payloads::AdminProviderKeyBatchUpdateRequest;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};

pub(super) async fn build_admin_pool_batch_update_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
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

    let Some(provider_id) = admin_pool_provider_id_from_path(request_context.path()) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let payload = match request_body.filter(|body| !body.is_empty()) {
        Some(body) => match serde_json::from_slice::<AdminProviderKeyBatchUpdateRequest>(body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "请求体必须包含 key_ids 与 patch",
                ));
            }
        },
        None => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                "请求体必须包含 key_ids 与 patch",
            ));
        }
    };

    state
        .build_admin_pool_batch_update_response(&provider_id, payload)
        .await
}
