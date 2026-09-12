use super::{
    admin_pool_provider_id_from_path, build_admin_pool_error_response, AdminPoolBatchActionRequest,
    ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
    ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::Response,
};

const MAX_ADMIN_POOL_BATCH_ITEMS: usize = 100;

pub(super) fn validate_admin_pool_batch_item_count(item_count: usize) -> Result<(), String> {
    if item_count > MAX_ADMIN_POOL_BATCH_ITEMS {
        return Err(format!("key_ids 最多 {MAX_ADMIN_POOL_BATCH_ITEMS} 个"));
    }
    Ok(())
}

pub(super) async fn build_admin_pool_batch_action_response(
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
    let payload = match request_body {
        Some(body) if !body.is_empty() => {
            match serde_json::from_slice::<AdminPoolBatchActionRequest>(body) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(build_admin_pool_error_response(
                        http::StatusCode::BAD_REQUEST,
                        "Invalid JSON request body",
                    ));
                }
            }
        }
        _ => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                "Invalid JSON request body",
            ));
        }
    };
    if let Err(detail) = validate_admin_pool_batch_item_count(payload.key_ids.len()) {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            detail,
        ));
    }

    state
        .build_admin_pool_batch_action_response(&provider_id, payload)
        .await
}

#[cfg(test)]
mod tests {
    use super::{validate_admin_pool_batch_item_count, MAX_ADMIN_POOL_BATCH_ITEMS};

    #[test]
    fn admin_pool_batch_item_count_has_an_inclusive_boundary() {
        assert!(validate_admin_pool_batch_item_count(MAX_ADMIN_POOL_BATCH_ITEMS).is_ok());
        assert!(validate_admin_pool_batch_item_count(MAX_ADMIN_POOL_BATCH_ITEMS + 1).is_err());
    }
}
