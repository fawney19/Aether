use super::super::cache_affinity::{
    clear_admin_monitoring_scheduler_affinity_entries,
    delete_admin_monitoring_cache_affinity_raw_keys,
};
use super::super::cache_identity::admin_monitoring_list_export_api_key_records_by_ids;
use super::super::cache_route_helpers::{
    admin_monitoring_cache_affinity_delete_params_from_path,
    admin_monitoring_cache_affinity_unavailable_response,
};
use super::super::cache_store::{
    list_admin_monitoring_cache_affinity_records_by_affinity_keys,
    load_admin_monitoring_cache_affinity_entries_for_tests,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response, admin_monitoring_not_found_response,
    build_admin_monitoring_cache_affinity_delete_success_response,
};
use axum::{body::Body, response::Response};

pub(in super::super) async fn build_admin_monitoring_cache_affinity_delete_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((affinity_key, endpoint_id, model_id, api_format)) =
        admin_monitoring_cache_affinity_delete_params_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response(
            "缺少 affinity_key、endpoint_id、model_id 或 api_format",
        ));
    };

    if state.redis_kv_runner().is_none()
        && load_admin_monitoring_cache_affinity_entries_for_tests(state).is_empty()
    {
        return Ok(admin_monitoring_cache_affinity_unavailable_response());
    }

    let target_affinity_keys =
        std::iter::once(affinity_key.clone()).collect::<std::collections::BTreeSet<_>>();
    let target_affinity =
        list_admin_monitoring_cache_affinity_records_by_affinity_keys(state, &target_affinity_keys)
            .await?
            .into_iter()
            .find(|item| {
                item.affinity_key == affinity_key
                    && item.endpoint_id.as_deref() == Some(endpoint_id.as_str())
                    && item.model_name == model_id
                    && item.api_format.eq_ignore_ascii_case(&api_format)
            });
    let Some(target_affinity) = target_affinity else {
        return Ok(admin_monitoring_not_found_response(
            "未找到指定的缓存亲和性记录",
        ));
    };

    let _ = delete_admin_monitoring_cache_affinity_raw_keys(
        state,
        std::slice::from_ref(&target_affinity.raw_key),
    )
    .await?;
    clear_admin_monitoring_scheduler_affinity_entries(
        state,
        std::slice::from_ref(&target_affinity),
    );

    let mut api_key_by_id = admin_monitoring_list_export_api_key_records_by_ids(
        state,
        std::slice::from_ref(&affinity_key),
    )
    .await?;
    let api_key_name = api_key_by_id
        .remove(&affinity_key)
        .and_then(|item| item.name)
        .unwrap_or_else(|| affinity_key.chars().take(8).collect::<String>());

    Ok(
        build_admin_monitoring_cache_affinity_delete_success_response(
            affinity_key,
            endpoint_id,
            model_id,
            api_key_name,
        ),
    )
}
