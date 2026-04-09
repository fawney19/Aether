use super::{
    admin_pool_provider_id_from_path, build_admin_pool_error_response, pool_selection,
    AdminPoolResolveSelectionRequest, ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::provider::pool as admin_provider_pool_pure;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};

pub(super) async fn build_admin_pool_resolve_selection_response(
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

    let Some(provider_id) = admin_pool_provider_id_from_path(request_context.path()) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "provider_id 无效",
        ));
    };

    let payload = match request_body {
        None => AdminPoolResolveSelectionRequest::default(),
        Some(body) if body.is_empty() => AdminPoolResolveSelectionRequest::default(),
        Some(body) => match serde_json::from_slice::<AdminPoolResolveSelectionRequest>(body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "Invalid JSON request body",
                ));
            }
        },
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            format!("Provider {provider_id} 不存在"),
        ));
    };

    let provider_type = provider.provider_type.clone();
    let search = payload.search.trim();
    let quick_selectors =
        admin_provider_pool_pure::admin_pool_sanitize_quick_selectors(payload.quick_selectors);

    let mut keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?
        .into_iter()
        .filter(|key| {
            pool_selection::admin_pool_matches_search(state, key, &provider_type, Some(search))
        })
        .filter(|key| {
            quick_selectors.is_empty()
                || quick_selectors.iter().all(|selector| {
                    pool_selection::admin_pool_matches_quick_selector(
                        state,
                        key,
                        &provider_type,
                        selector,
                    )
                })
        })
        .collect::<Vec<_>>();

    keys.sort_by(|left, right| {
        left.internal_priority
            .cmp(&right.internal_priority)
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(Json(admin_provider_pool_pure::build_admin_pool_selection_payload(&keys)).into_response())
}
