use super::super::cache_route_helpers::{
    admin_monitoring_cache_model_mapping_provider_params_from_path,
    admin_monitoring_cache_model_name_from_path, admin_monitoring_redis_unavailable_response,
};
use super::super::cache_store::{
    admin_monitoring_has_test_redis_keys, delete_admin_monitoring_namespaced_keys,
    list_admin_monitoring_namespaced_keys,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use aether_admin::observability::monitoring::{
    admin_monitoring_bad_request_response,
    build_admin_monitoring_model_mapping_delete_model_success_response,
    build_admin_monitoring_model_mapping_delete_provider_success_response,
    build_admin_monitoring_model_mapping_delete_success_response,
};
use axum::{body::Body, response::Response};

pub(in super::super) async fn build_admin_monitoring_model_mapping_delete_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    if state.redis_kv_runner().is_none() && !admin_monitoring_has_test_redis_keys(state) {
        return Ok(admin_monitoring_redis_unavailable_response());
    }

    let mut raw_keys = list_admin_monitoring_namespaced_keys(state, "model:*").await?;
    raw_keys.extend(list_admin_monitoring_namespaced_keys(state, "global_model:*").await?);
    raw_keys.sort();
    raw_keys.dedup();
    let deleted_count = delete_admin_monitoring_namespaced_keys(state, &raw_keys).await?;

    Ok(build_admin_monitoring_model_mapping_delete_success_response(deleted_count))
}

pub(in super::super) async fn build_admin_monitoring_model_mapping_delete_model_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some(model_name) =
        admin_monitoring_cache_model_name_from_path(&request_context.request_path)
    else {
        return Ok(admin_monitoring_bad_request_response("缺少 model_name"));
    };
    if state.redis_kv_runner().is_none() && !admin_monitoring_has_test_redis_keys(state) {
        return Ok(admin_monitoring_redis_unavailable_response());
    }

    let candidate_keys = [
        format!("global_model:resolve:{model_name}"),
        format!("global_model:name:{model_name}"),
    ];
    let mut existing_keys = Vec::new();
    for key in candidate_keys {
        let matches = list_admin_monitoring_namespaced_keys(state, key.as_str()).await?;
        existing_keys.extend(matches);
    }
    existing_keys.sort();
    existing_keys.dedup();

    let deleted_count = delete_admin_monitoring_namespaced_keys(state, &existing_keys).await?;
    let deleted_keys = if deleted_count == 0 {
        Vec::new()
    } else {
        existing_keys
    };

    Ok(
        build_admin_monitoring_model_mapping_delete_model_success_response(
            model_name,
            deleted_keys,
        ),
    )
}

pub(in super::super) async fn build_admin_monitoring_model_mapping_delete_provider_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    let Some((provider_id, global_model_id)) =
        admin_monitoring_cache_model_mapping_provider_params_from_path(
            &request_context.request_path,
        )
    else {
        return Ok(admin_monitoring_bad_request_response(
            "缺少 provider_id 或 global_model_id",
        ));
    };
    if state.redis_kv_runner().is_none() && !admin_monitoring_has_test_redis_keys(state) {
        return Ok(admin_monitoring_redis_unavailable_response());
    }

    let candidate_keys = [
        format!("model:provider_global:{provider_id}:{global_model_id}"),
        format!("model:provider_global:hits:{provider_id}:{global_model_id}"),
    ];
    let mut existing_keys = Vec::new();
    for key in candidate_keys {
        let matches = list_admin_monitoring_namespaced_keys(state, key.as_str()).await?;
        existing_keys.extend(matches);
    }
    existing_keys.sort();
    existing_keys.dedup();

    let _ = delete_admin_monitoring_namespaced_keys(state, &existing_keys).await?;

    Ok(
        build_admin_monitoring_model_mapping_delete_provider_success_response(
            provider_id,
            global_model_id,
            existing_keys,
        ),
    )
}
