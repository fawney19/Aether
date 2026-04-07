use super::support::{
    admin_pool_provider_id_from_path, build_admin_pool_error_response,
    ADMIN_POOL_BANNED_KEY_CLEANUP_EMPTY_MESSAGE,
    ADMIN_POOL_PROVIDER_CATALOG_READER_UNAVAILABLE_DETAIL,
    ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
};
use super::{payloads as pool_payloads, selection as pool_selection};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::pool::runtime::{
    clear_admin_provider_pool_cooldown, reset_admin_provider_pool_cost,
};
use crate::handlers::admin::shared::{
    attach_admin_audit_response, encrypt_catalog_secret_with_fallbacks,
};
use crate::{AppState, GatewayError, LocalProviderDeleteTaskState};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Default, serde::Deserialize)]
struct AdminPoolBatchActionRequest {
    #[serde(default)]
    key_ids: Vec<String>,
    #[serde(default)]
    action: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct AdminPoolBatchImportRequest {
    #[serde(default)]
    keys: Vec<AdminPoolBatchImportItem>,
    #[serde(default)]
    proxy_node_id: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct AdminPoolBatchImportItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    auth_type: String,
}

fn admin_pool_batch_delete_task_parts(request_path: &str) -> Option<(String, String)> {
    let raw = request_path.strip_prefix("/api/admin/pool/")?;
    let (provider_id, suffix) = raw.split_once("/keys/batch-delete-task/")?;
    let provider_id = provider_id.trim();
    let task_id = suffix.trim().trim_matches('/');
    if provider_id.is_empty()
        || provider_id.contains('/')
        || task_id.is_empty()
        || task_id.contains('/')
    {
        return None;
    }
    Some((provider_id.to_string(), task_id.to_string()))
}

fn admin_pool_key_proxy_value(proxy_node_id: Option<&str>) -> Option<serde_json::Value> {
    proxy_node_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| json!({ "node_id": value, "enabled": true }))
}

fn build_admin_pool_batch_delete_task_payload(
    task: &LocalProviderDeleteTaskState,
) -> serde_json::Value {
    json!({
        "task_id": task.task_id,
        "provider_id": task.provider_id,
        "status": task.status,
        "stage": task.stage,
        "total_keys": task.total_keys,
        "deleted_keys": task.deleted_keys,
        "total_endpoints": task.total_endpoints,
        "deleted_endpoints": task.deleted_endpoints,
        "message": task.message,
    })
}

fn attach_admin_pool_batch_delete_task_terminal_audit(
    provider_id: &str,
    task_id: &str,
    task_status: &str,
    response: Response<Body>,
) -> Response<Body> {
    match task_status {
        "completed" => attach_admin_audit_response(
            response,
            "admin_pool_batch_delete_task_completed_viewed",
            "view_pool_batch_delete_task_terminal_state",
            "provider_key_batch_delete_task",
            &format!("{provider_id}:{task_id}"),
        ),
        "failed" => attach_admin_audit_response(
            response,
            "admin_pool_batch_delete_task_failed_viewed",
            "view_pool_batch_delete_task_terminal_state",
            "provider_key_batch_delete_task",
            &format!("{provider_id}:{task_id}"),
        ),
        _ => response,
    }
}

fn admin_pool_resolved_api_formats(
    endpoints: &[StoredProviderCatalogEndpoint],
    existing_keys: &[StoredProviderCatalogKey],
) -> Vec<String> {
    let mut formats = Vec::new();
    let mut seen = BTreeSet::new();
    for endpoint in endpoints.iter().filter(|endpoint| endpoint.is_active) {
        let api_format = endpoint.api_format.trim();
        if api_format.is_empty() || !seen.insert(api_format.to_string()) {
            continue;
        }
        formats.push(api_format.to_string());
    }
    if !formats.is_empty() {
        return formats;
    }

    for key in existing_keys {
        for api_format in pool_payloads::admin_pool_api_formats(key) {
            if !seen.insert(api_format.clone()) {
                continue;
            }
            formats.push(api_format);
        }
    }
    formats
}

async fn build_admin_pool_cleanup_banned_keys_response(
    state: &AppState,
    provider_id: String,
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

    let banned_keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?
        .into_iter()
        .filter(pool_selection::admin_pool_key_is_known_banned)
        .collect::<Vec<_>>();
    if banned_keys.is_empty() {
        return Ok(Json(json!({
            "affected": 0,
            "message": ADMIN_POOL_BANNED_KEY_CLEANUP_EMPTY_MESSAGE,
        }))
        .into_response());
    }

    let deleted_key_ids = banned_keys
        .iter()
        .map(|key| key.id.clone())
        .collect::<Vec<_>>();
    for key in &banned_keys {
        clear_admin_provider_pool_cooldown(state, &provider.id, &key.id).await;
        reset_admin_provider_pool_cost(state, &provider.id, &key.id).await;
    }

    let mut affected = 0usize;
    for key_id in &deleted_key_ids {
        if state.delete_provider_catalog_key(key_id).await? {
            affected += 1;
        }
    }
    state
        .cleanup_deleted_provider_catalog_refs(&provider.id, &[], &deleted_key_ids)
        .await?;

    Ok(Json(json!({
        "affected": affected,
        "message": format!("已清理 {affected} 个异常账号"),
    }))
    .into_response())
}

async fn build_admin_pool_batch_import_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
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

    let Some(provider_id) = admin_pool_provider_id_from_path(&request_context.request_path) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "provider_id 无效",
        ));
    };
    let payload = match request_body {
        Some(body) if !body.is_empty() => {
            match serde_json::from_slice::<AdminPoolBatchImportRequest>(body) {
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

    if payload.keys.len() > 500 {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "keys length must be less than or equal to 500",
        ));
    }

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

    let endpoints = state
        .list_provider_catalog_endpoints_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let existing_keys = state
        .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider.id))
        .await?;
    let api_formats = admin_pool_resolved_api_formats(&endpoints, &existing_keys);
    if api_formats.is_empty() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "Provider 没有可用 endpoint 或现有 key，无法推断 api_formats",
        ));
    }

    let proxy = admin_pool_key_proxy_value(payload.proxy_node_id.as_deref());
    let mut imported = 0usize;
    let skipped = 0usize;
    let mut errors = Vec::new();
    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    for (index, item) in payload.keys.iter().enumerate() {
        let api_key = item.api_key.trim();
        if api_key.is_empty() {
            errors.push(json!({
                "index": index,
                "reason": "api_key is empty",
            }));
            continue;
        }

        let Some(encrypted_api_key) = encrypt_catalog_secret_with_fallbacks(state, api_key) else {
            errors.push(json!({
                "index": index,
                "reason": "gateway 未配置 provider key 加密密钥",
            }));
            continue;
        };

        let auth_type = item.auth_type.trim().to_ascii_lowercase();
        let auth_type = if auth_type.is_empty() {
            "api_key".to_string()
        } else {
            auth_type
        };
        let name = item.name.trim();
        let mut record = match StoredProviderCatalogKey::new(
            Uuid::new_v4().to_string(),
            provider.id.clone(),
            if name.is_empty() {
                format!("imported-{index}")
            } else {
                name.to_string()
            },
            auth_type,
            None,
            true,
        ) {
            Ok(value) => value,
            Err(err) => {
                errors.push(json!({
                    "index": index,
                    "reason": err.to_string(),
                }));
                continue;
            }
        };
        record = match record.with_transport_fields(
            Some(json!(api_formats)),
            encrypted_api_key,
            None,
            None,
            None,
            None,
            None,
            proxy.clone(),
            None,
        ) {
            Ok(value) => value,
            Err(err) => {
                errors.push(json!({
                    "index": index,
                    "reason": err.to_string(),
                }));
                continue;
            }
        };
        record.request_count = Some(0);
        record.success_count = Some(0);
        record.error_count = Some(0);
        record.total_response_time_ms = Some(0);
        record.health_by_format = Some(json!({}));
        record.circuit_breaker_by_format = Some(json!({}));
        record.created_at_unix_secs = Some(now_unix_secs);
        record.updated_at_unix_secs = Some(now_unix_secs);

        let Some(_) = state.create_provider_catalog_key(&record).await? else {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::SERVICE_UNAVAILABLE,
                ADMIN_POOL_PROVIDER_CATALOG_WRITER_UNAVAILABLE_DETAIL,
            ));
        };
        imported += 1;
    }

    Ok(Json(json!({
        "imported": imported,
        "skipped": skipped,
        "errors": errors,
    }))
    .into_response())
}

async fn build_admin_pool_batch_action_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
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

    let Some(provider_id) = admin_pool_provider_id_from_path(&request_context.request_path) else {
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

    let action = payload.action.trim().to_ascii_lowercase();
    let action_label = match action.as_str() {
        "enable" => "enabled",
        "disable" => "disabled",
        "clear_proxy" => "proxy cleared",
        "set_proxy" => "proxy set",
        "delete" => "deleted",
        _ => {
            return Ok(build_admin_pool_error_response(
                http::StatusCode::BAD_REQUEST,
                format!(
                    "Invalid action: {action}. Supported locally: enable, disable, clear_proxy, set_proxy, delete"
                ),
            ));
        }
    };

    let key_ids = payload
        .key_ids
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if key_ids.is_empty() {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::BAD_REQUEST,
            "key_ids should not be empty",
        ));
    }

    let proxy_payload = if action == "set_proxy" {
        match payload.payload {
            Some(serde_json::Value::Object(map)) if !map.is_empty() => {
                Some(serde_json::Value::Object(map))
            }
            _ => {
                return Ok(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "set_proxy action requires a non-empty payload with proxy config",
                ));
            }
        }
    } else {
        None
    };

    let keys = state
        .read_provider_catalog_keys_by_ids(&key_ids)
        .await?
        .into_iter()
        .filter(|key| key.provider_id == provider.id)
        .collect::<Vec<_>>();

    if action == "delete" {
        let deleted_key_ids = keys.iter().map(|key| key.id.clone()).collect::<Vec<_>>();
        for key in &keys {
            clear_admin_provider_pool_cooldown(state, &provider.id, &key.id).await;
            reset_admin_provider_pool_cost(state, &provider.id, &key.id).await;
        }

        let mut affected = 0usize;
        for key_id in &deleted_key_ids {
            if state.delete_provider_catalog_key(key_id).await? {
                affected = affected.saturating_add(1);
            }
        }
        state
            .cleanup_deleted_provider_catalog_refs(&provider.id, &[], &deleted_key_ids)
            .await?;

        return Ok(Json(json!({
            "affected": affected,
            "message": format!("{affected} keys {action_label}"),
        }))
        .into_response());
    }

    let mut affected = 0usize;
    for mut key in keys {
        match action.as_str() {
            "enable" => key.is_active = true,
            "disable" => key.is_active = false,
            "clear_proxy" => key.proxy = None,
            "set_proxy" => key.proxy = proxy_payload.clone(),
            _ => unreachable!(),
        }
        if state.update_provider_catalog_key(&key).await?.is_some() {
            affected = affected.saturating_add(1);
        }
    }

    Ok(Json(json!({
        "affected": affected,
        "message": format!("{affected} keys {action_label}"),
    }))
    .into_response())
}

async fn build_admin_pool_batch_delete_task_status_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
) -> Result<Response<Body>, GatewayError> {
    let Some((provider_id, task_id)) =
        admin_pool_batch_delete_task_parts(&request_context.request_path)
    else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            "批量删除任务不存在",
        ));
    };
    let Some(task) = state.get_provider_delete_task(&task_id) else {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            "批量删除任务不存在",
        ));
    };
    if task.provider_id != provider_id {
        return Ok(build_admin_pool_error_response(
            http::StatusCode::NOT_FOUND,
            "批量删除任务不存在",
        ));
    }

    Ok(attach_admin_pool_batch_delete_task_terminal_audit(
        &provider_id,
        &task_id,
        task.status.as_str(),
        Json(build_admin_pool_batch_delete_task_payload(&task)).into_response(),
    ))
}

pub(super) async fn maybe_build_local_admin_pool_batch_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    match decision_route_kind(request_context) {
        Some("cleanup_banned_keys") if request_context.request_method == http::Method::POST => {
            let Some(provider_id) = admin_pool_provider_id_from_path(&request_context.request_path)
            else {
                return Ok(Some(build_admin_pool_error_response(
                    http::StatusCode::BAD_REQUEST,
                    "provider_id 无效",
                )));
            };
            Ok(Some(
                build_admin_pool_cleanup_banned_keys_response(state, provider_id).await?,
            ))
        }
        Some("batch_import_keys") => Ok(Some(
            build_admin_pool_batch_import_response(state, request_context, request_body).await?,
        )),
        Some("batch_action_keys") => Ok(Some(
            build_admin_pool_batch_action_response(state, request_context, request_body).await?,
        )),
        Some("batch_delete_task_status") => Ok(Some(
            build_admin_pool_batch_delete_task_status_response(state, request_context).await?,
        )),
        _ => Ok(None),
    }
}

fn decision_route_kind<'a>(request_context: &'a GatewayPublicRequestContext) -> Option<&'a str> {
    request_context
        .control_decision
        .as_ref()?
        .route_kind
        .as_deref()
}
