use super::cache_config::ADMIN_MONITORING_REDIS_CACHE_CATEGORIES;
use super::cache_store::{
    admin_monitoring_has_test_redis_keys, list_admin_monitoring_namespaced_keys,
};
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use axum::{
    body::Body,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn build_admin_monitoring_model_mapping_stats_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    if state.redis_kv_runner().is_none() && !admin_monitoring_has_test_redis_keys(state) {
        return Ok(Json(json!({
            "status": "ok",
            "data": {
                "available": false,
                "message": "Redis 未启用，模型映射缓存不可用",
            }
        }))
        .into_response());
    };

    let model_id_keys = list_admin_monitoring_namespaced_keys(state, "model:id:*").await?;
    let global_model_id_keys =
        list_admin_monitoring_namespaced_keys(state, "global_model:id:*").await?;
    let global_model_name_keys =
        list_admin_monitoring_namespaced_keys(state, "global_model:name:*").await?;
    let global_model_resolve_keys =
        list_admin_monitoring_namespaced_keys(state, "global_model:resolve:*").await?;
    let provider_global_keys =
        list_admin_monitoring_namespaced_keys(state, "model:provider_global:*")
            .await?
            .into_iter()
            .filter(|key| !key.starts_with("model:provider_global:hits:"))
            .collect::<Vec<_>>();

    let total_keys = model_id_keys.len()
        + global_model_id_keys.len()
        + global_model_name_keys.len()
        + global_model_resolve_keys.len()
        + provider_global_keys.len();

    Ok(Json(json!({
        "status": "ok",
        "data": {
            "available": true,
            "ttl_seconds": 300,
            "total_keys": total_keys,
            "breakdown": {
                "model_by_id": model_id_keys.len(),
                "model_by_provider_global": provider_global_keys.len(),
                "global_model_by_id": global_model_id_keys.len(),
                "global_model_by_name": global_model_name_keys.len(),
                "global_model_resolve": global_model_resolve_keys.len(),
            },
            "mappings": [],
            "provider_model_mappings": serde_json::Value::Null,
            "unmapped": serde_json::Value::Null,
        }
    }))
    .into_response())
}

pub(super) async fn build_admin_monitoring_redis_cache_categories_response(
    state: &AdminAppState<'_>,
) -> Result<Response<Body>, GatewayError> {
    if state.redis_kv_runner().is_none() && !admin_monitoring_has_test_redis_keys(state) {
        return Ok(Json(json!({
            "status": "ok",
            "data": {
                "available": false,
                "message": "Redis 未启用",
            }
        }))
        .into_response());
    };

    let mut categories = Vec::with_capacity(ADMIN_MONITORING_REDIS_CACHE_CATEGORIES.len());
    let mut total_keys = 0usize;

    for (key, name, pattern, description) in ADMIN_MONITORING_REDIS_CACHE_CATEGORIES {
        let count = list_admin_monitoring_namespaced_keys(state, pattern)
            .await?
            .len();
        total_keys += count;
        categories.push(json!({
            "key": key,
            "name": name,
            "pattern": pattern,
            "description": description,
            "count": count,
        }));
    }

    Ok(Json(json!({
        "status": "ok",
        "data": {
            "available": true,
            "categories": categories,
            "total_keys": total_keys,
        }
    }))
    .into_response())
}
