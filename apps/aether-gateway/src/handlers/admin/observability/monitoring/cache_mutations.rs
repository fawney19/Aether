use super::cache_affinity::{
    clear_admin_monitoring_scheduler_affinity_entries,
    delete_admin_monitoring_cache_affinity_raw_keys,
};
use super::cache_config::ADMIN_MONITORING_REDIS_CACHE_CATEGORIES;
use super::cache_identity::{
    admin_monitoring_find_user_summary_by_id, admin_monitoring_list_export_api_key_records_by_ids,
};
use super::cache_route_helpers::{
    admin_monitoring_cache_affinity_delete_params_from_path,
    admin_monitoring_cache_affinity_unavailable_response,
    admin_monitoring_cache_model_mapping_provider_params_from_path,
    admin_monitoring_cache_model_name_from_path, admin_monitoring_cache_provider_id_from_path,
    admin_monitoring_cache_redis_category_from_path,
    admin_monitoring_cache_users_not_found_response,
    admin_monitoring_cache_users_user_identifier_from_path,
    admin_monitoring_redis_unavailable_response,
};
use super::cache_store::{
    admin_monitoring_has_test_redis_keys, delete_admin_monitoring_namespaced_keys,
    list_admin_monitoring_cache_affinity_records,
    list_admin_monitoring_cache_affinity_records_by_affinity_keys,
    list_admin_monitoring_namespaced_keys, load_admin_monitoring_cache_affinity_entries_for_tests,
};
use super::responses::{
    admin_monitoring_bad_request_response, admin_monitoring_not_found_response,
};
use crate::control::GatewayPublicRequestContext;
use crate::{AppState, GatewayError};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn build_admin_monitoring_cache_users_delete_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
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

    let direct_api_key_by_id = admin_monitoring_list_export_api_key_records_by_ids(
        state,
        std::slice::from_ref(&user_identifier),
    )
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
        return Ok(Json(json!({
            "status": "ok",
            "message": format!("已清除 API Key {api_key_name} 的缓存亲和性"),
            "user_info": {
                "user_id": Some(api_key.user_id.clone()),
                "username": user.as_ref().map(|item| item.username.clone()),
                "email": user.and_then(|item| item.email),
                "api_key_id": user_identifier,
                "api_key_name": api_key.name.clone(),
            },
        }))
        .into_response());
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

    Ok(Json(json!({
        "status": "ok",
        "message": format!("已清除用户 {} 的所有缓存亲和性", user.username),
        "user_info": {
            "user_id": user.id,
            "username": user.username,
            "email": user.email,
        },
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_cache_affinity_delete_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
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

    Ok(Json(json!({
        "status": "ok",
        "message": format!("已清除缓存亲和性: {api_key_name}"),
        "affinity_key": affinity_key,
        "endpoint_id": endpoint_id,
        "model_id": model_id,
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_cache_flush_response(
    state: &AppState,
) -> Result<Response<Body>, GatewayError> {
    let raw_affinities = list_admin_monitoring_cache_affinity_records(state).await?;
    if state.redis_kv_runner().is_none() && raw_affinities.is_empty() {
        return Ok(admin_monitoring_cache_affinity_unavailable_response());
    }

    let raw_keys = raw_affinities
        .iter()
        .map(|item| item.raw_key.clone())
        .collect::<Vec<_>>();
    let deleted = delete_admin_monitoring_cache_affinity_raw_keys(state, &raw_keys).await?;
    clear_admin_monitoring_scheduler_affinity_entries(state, &raw_affinities);

    Ok(Json(json!({
        "status": "ok",
        "message": "已清除全部缓存亲和性",
        "deleted_affinities": deleted,
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_cache_provider_delete_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
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
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({
                "detail": format!("未找到 provider {provider_id} 的缓存亲和性记录")
            })),
        )
            .into_response());
    }

    let raw_keys = target_affinities
        .iter()
        .map(|item| item.raw_key.clone())
        .collect::<Vec<_>>();
    let deleted = delete_admin_monitoring_cache_affinity_raw_keys(state, &raw_keys).await?;
    clear_admin_monitoring_scheduler_affinity_entries(state, &target_affinities);

    Ok(Json(json!({
        "status": "ok",
        "message": format!("已清除 provider {provider_id} 的缓存亲和性"),
        "provider_id": provider_id,
        "deleted_affinities": deleted,
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_model_mapping_delete_response(
    state: &AppState,
) -> Result<Response<Body>, GatewayError> {
    if state.redis_kv_runner().is_none() && !admin_monitoring_has_test_redis_keys(state) {
        return Ok(admin_monitoring_redis_unavailable_response());
    }

    let mut raw_keys = list_admin_monitoring_namespaced_keys(state, "model:*").await?;
    raw_keys.extend(list_admin_monitoring_namespaced_keys(state, "global_model:*").await?);
    raw_keys.sort();
    raw_keys.dedup();
    let deleted_count = delete_admin_monitoring_namespaced_keys(state, &raw_keys).await?;

    Ok(Json(json!({
        "status": "ok",
        "message": "已清除所有模型映射缓存",
        "deleted_count": deleted_count,
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_model_mapping_delete_model_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
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

    Ok(Json(json!({
        "status": "ok",
        "message": format!("已清除模型 {model_name} 的映射缓存"),
        "model_name": model_name,
        "deleted_keys": deleted_keys,
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_model_mapping_delete_provider_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
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

    Ok(Json(json!({
        "status": "ok",
        "message": "已清除 Provider 模型映射缓存",
        "provider_id": provider_id,
        "global_model_id": global_model_id,
        "deleted_keys": existing_keys,
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_redis_keys_delete_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
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
        return Ok((
            http::StatusCode::NOT_FOUND,
            Json(json!({ "detail": format!("未知的缓存分类: {category}") })),
        )
            .into_response());
    };

    if state.redis_kv_runner().is_none() && !admin_monitoring_has_test_redis_keys(state) {
        return Ok(admin_monitoring_redis_unavailable_response());
    }

    let raw_keys = list_admin_monitoring_namespaced_keys(state, pattern).await?;
    let deleted_count = delete_admin_monitoring_namespaced_keys(state, &raw_keys).await?;

    Ok(Json(json!({
        "status": "ok",
        "message": format!("已清除 {name} 缓存"),
        "category": cat_key,
        "deleted_count": deleted_count,
    }))
    .into_response())
}
