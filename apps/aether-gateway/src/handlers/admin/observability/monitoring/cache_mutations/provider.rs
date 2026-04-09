use super::super::cache_affinity::{
    clear_admin_monitoring_scheduler_affinity_entries,
    delete_admin_monitoring_cache_affinity_raw_keys,
};
use super::super::cache_route_helpers::{
    admin_monitoring_cache_affinity_unavailable_response,
    admin_monitoring_cache_provider_id_from_path,
};
use super::super::cache_store::list_admin_monitoring_cache_affinity_records;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response, admin_monitoring_cache_provider_not_found_response,
    build_admin_monitoring_cache_provider_delete_success_response,
};
use axum::{body::Body, response::Response};

pub(in super::super) async fn build_admin_monitoring_cache_provider_delete_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(provider_id) =
        admin_monitoring_cache_provider_id_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response("缺少 provider_id"));
    };

    let raw_affinities = list_admin_monitoring_cache_affinity_records(state).await?;
    if state.redis_kv_runner().is_none() && raw_affinities.is_empty() {
        return Ok(admin_monitoring_cache_affinity_unavailable_response());
    }

    let target_affinities = raw_affinities
        .into_iter()
        .filter(|item| item.provider_id.as_deref() == Some(provider_id.as_str()))
        .collect::<Vec<_>>();
    if target_affinities.is_empty() {
        return Ok(admin_monitoring_cache_provider_not_found_response(
            &provider_id,
        ));
    }

    let raw_keys = target_affinities
        .iter()
        .map(|item| item.raw_key.clone())
        .collect::<Vec<_>>();
    let deleted = delete_admin_monitoring_cache_affinity_raw_keys(state, &raw_keys).await?;
    clear_admin_monitoring_scheduler_affinity_entries(state, &target_affinities);

    Ok(build_admin_monitoring_cache_provider_delete_success_response(provider_id, deleted))
}
