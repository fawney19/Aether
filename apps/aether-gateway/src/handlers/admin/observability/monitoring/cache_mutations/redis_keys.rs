use super::super::cache_config::ADMIN_MONITORING_REDIS_CACHE_CATEGORIES;
use super::super::cache_route_helpers::{
    admin_monitoring_cache_redis_category_from_path, admin_monitoring_redis_unavailable_response,
};
use super::super::cache_store::{
    admin_monitoring_has_test_redis_keys, delete_admin_monitoring_namespaced_keys,
    list_admin_monitoring_namespaced_keys,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response, admin_monitoring_unknown_cache_category_response,
    build_admin_monitoring_redis_keys_delete_success_response,
};
use axum::{body::Body, response::Response};

pub(in super::super) async fn build_admin_monitoring_redis_keys_delete_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(category) =
        admin_monitoring_cache_redis_category_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response("缺少 category"));
    };

    let Some((cat_key, name, pattern, _description)) = ADMIN_MONITORING_REDIS_CACHE_CATEGORIES
        .iter()
        .find(|(cat_key, _, _, _)| *cat_key == category)
    else {
        return Ok(admin_monitoring_unknown_cache_category_response(&category));
    };

    if state.redis_kv_runner().is_none() && !admin_monitoring_has_test_redis_keys(state) {
        return Ok(admin_monitoring_redis_unavailable_response());
    }

    let raw_keys = list_admin_monitoring_namespaced_keys(state, pattern).await?;
    let deleted_count = delete_admin_monitoring_namespaced_keys(state, &raw_keys).await?;

    Ok(build_admin_monitoring_redis_keys_delete_success_response(
        cat_key,
        name,
        deleted_count,
    ))
}
