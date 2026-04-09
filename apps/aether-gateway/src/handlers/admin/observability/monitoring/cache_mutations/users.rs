use super::super::cache_affinity::{
    clear_admin_monitoring_scheduler_affinity_entries,
    delete_admin_monitoring_cache_affinity_raw_keys,
};
use super::super::cache_identity::{
    admin_monitoring_find_user_summary_by_id, admin_monitoring_list_export_api_key_records_by_ids,
};
use super::super::cache_route_helpers::{
    admin_monitoring_cache_affinity_unavailable_response,
    admin_monitoring_cache_users_not_found_response,
    admin_monitoring_cache_users_user_identifier_from_path,
};
use super::super::cache_store::{
    list_admin_monitoring_cache_affinity_records_by_affinity_keys,
    load_admin_monitoring_cache_affinity_entries_for_tests,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response,
    build_admin_monitoring_cache_users_delete_api_key_success_response,
    build_admin_monitoring_cache_users_delete_user_success_response,
};
use axum::{body::Body, response::Response};

pub(in super::super) async fn build_admin_monitoring_cache_users_delete_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(user_identifier) =
        admin_monitoring_cache_users_user_identifier_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response(
            "缺少 user_identifier",
        ));
    };

    if state.redis_kv_runner().is_none()
        && load_admin_monitoring_cache_affinity_entries_for_tests(state).is_empty()
    {
        return Ok(admin_monitoring_cache_affinity_unavailable_response());
    }

    let direct_api_key_by_id =
        admin_monitoring_list_export_api_key_records_by_ids(state, &[user_identifier.clone()])
            .await?;

    if let Some(api_key) = direct_api_key_by_id.get(&user_identifier) {
        let target_affinity_keys =
            std::iter::once(user_identifier.clone()).collect::<std::collections::BTreeSet<_>>();
        let target_affinities = list_admin_monitoring_cache_affinity_records_by_affinity_keys(
            state,
            &target_affinity_keys,
        )
        .await?;
        let raw_keys = target_affinities
            .iter()
            .map(|item| item.raw_key.clone())
            .collect::<Vec<_>>();
        let _ = delete_admin_monitoring_cache_affinity_raw_keys(state, &raw_keys).await?;
        clear_admin_monitoring_scheduler_affinity_entries(state, &target_affinities);

        let user = admin_monitoring_find_user_summary_by_id(state, &api_key.user_id).await?;
        let api_key_name = api_key
            .name
            .clone()
            .unwrap_or_else(|| user_identifier.clone());
        return Ok(
            build_admin_monitoring_cache_users_delete_api_key_success_response(
                api_key.user_id.clone(),
                user.as_ref().map(|item| item.username.clone()),
                user.and_then(|item| item.email),
                user_identifier,
                api_key.name.clone(),
            ),
        );
    }

    let Some(user) = state.find_user_auth_by_identifier(&user_identifier).await? else {
        return Ok(admin_monitoring_cache_users_not_found_response(
            &user_identifier,
        ));
    };

    let user_api_key_ids = state
        .list_auth_api_key_export_records_by_user_ids(std::slice::from_ref(&user.id))
        .await?
        .into_iter()
        .map(|item| item.api_key_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let target_affinities =
        list_admin_monitoring_cache_affinity_records_by_affinity_keys(state, &user_api_key_ids)
            .await?;
    let raw_keys = target_affinities
        .iter()
        .map(|item| item.raw_key.clone())
        .collect::<Vec<_>>();
    let _ = delete_admin_monitoring_cache_affinity_raw_keys(state, &raw_keys).await?;
    clear_admin_monitoring_scheduler_affinity_entries(state, &target_affinities);

    Ok(
        build_admin_monitoring_cache_users_delete_user_success_response(
            user.id,
            user.username,
            user.email,
        ),
    )
}
