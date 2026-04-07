use super::responses::build_admin_providers_data_unavailable_response;
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::provider::delete_task::{
    build_admin_provider_mapping_preview_payload, run_admin_provider_delete_task,
};
use crate::handlers::admin::provider::pool::runtime::{
    build_admin_provider_pool_status_payload, clear_admin_provider_pool_cooldown,
    reset_admin_provider_pool_cost,
};
use crate::handlers::admin::provider::shared::paths::{
    admin_provider_clear_pool_cooldown_parts, admin_provider_delete_task_parts,
    admin_provider_id_for_health_monitor, admin_provider_id_for_manage_path,
    admin_provider_id_for_mapping_preview, admin_provider_id_for_pool_status,
    admin_provider_id_for_summary, admin_provider_reset_pool_cost_parts, is_admin_providers_root,
};
use crate::handlers::admin::provider::shared::payloads::{
    AdminProviderCreateRequest, AdminProviderUpdateRequest,
};
use crate::handlers::admin::provider::shared::support::{
    build_admin_provider_delete_task_payload, put_admin_provider_delete_task,
};
use crate::handlers::admin::provider::summary::{
    build_admin_provider_health_monitor_payload, build_admin_provider_summary_payload,
    build_admin_provider_summary_value, build_admin_providers_payload,
    build_admin_providers_summary_payload,
};
use crate::handlers::admin::provider::write::provider::{
    build_admin_create_provider_record, build_admin_fixed_provider_endpoint_record,
    build_admin_update_provider_record,
};
use crate::handlers::admin::shared::{
    attach_admin_audit_response, query_param_optional_bool, query_param_value,
};
use crate::provider_transport::provider_types::fixed_provider_template;
use crate::{AppState, GatewayError, LocalProviderDeleteTaskState};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

fn attach_admin_provider_delete_task_terminal_audit(
    provider_id: &str,
    task_id: &str,
    task_status: &str,
    response: Response<Body>,
) -> Response<Body> {
    match task_status {
        "completed" => attach_admin_audit_response(
            response,
            "admin_provider_delete_task_completed_viewed",
            "view_provider_delete_task_terminal_state",
            "provider_delete_task",
            &format!("{provider_id}:{task_id}"),
        ),
        "failed" => attach_admin_audit_response(
            response,
            "admin_provider_delete_task_failed_viewed",
            "view_provider_delete_task_terminal_state",
            "provider_delete_task",
            &format!("{provider_id}:{task_id}"),
        ),
        _ => response,
    }
}

pub(crate) async fn maybe_build_local_admin_providers_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("create_provider")
        && request_context.request_method == http::Method::POST
        && matches!(
            request_context.request_path.as_str(),
            "/api/admin/providers" | "/api/admin/providers/"
        )
    {
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let payload = match serde_json::from_slice::<AdminProviderCreateRequest>(request_body) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        let (record, shift_existing_priorities_from) =
            match build_admin_create_provider_record(state, payload).await {
                Ok(record) => record,
                Err(message) => {
                    return Ok(Some(
                        (
                            http::StatusCode::BAD_REQUEST,
                            Json(json!({ "detail": message })),
                        )
                            .into_response(),
                    ));
                }
            };
        let Some(created_provider) = state
            .create_provider_catalog_provider(&record, shift_existing_priorities_from)
            .await?
        else {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        };

        if let Some((base_url, endpoint_signatures)) =
            fixed_provider_template(&created_provider.provider_type)
        {
            for endpoint_signature in endpoint_signatures {
                let endpoint = match build_admin_fixed_provider_endpoint_record(
                    &created_provider,
                    endpoint_signature,
                    base_url,
                ) {
                    Ok(endpoint) => endpoint,
                    Err(message) => {
                        return Ok(Some(
                            (
                                http::StatusCode::BAD_REQUEST,
                                Json(json!({ "detail": message })),
                            )
                                .into_response(),
                        ));
                    }
                };
                let Some(_) = state.create_provider_catalog_endpoint(&endpoint).await? else {
                    return Ok(Some(build_admin_providers_data_unavailable_response()));
                };
            }
        }

        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "id": created_provider.id,
                "name": created_provider.name,
                "message": "提供商创建成功",
            }))
            .into_response(),
            "admin_provider_created",
            "create_provider",
            "provider",
            &created_provider.id,
        )));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("list_providers")
        && is_admin_providers_root(&request_context.request_path)
    {
        if !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let skip = query_param_value(request_context.request_query_string.as_deref(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.request_query_string.as_deref(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 500)
            .unwrap_or(100);
        let is_active =
            query_param_optional_bool(request_context.request_query_string.as_deref(), "is_active");
        let Some(payload) = build_admin_providers_payload(state, skip, limit, is_active).await
        else {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("update_provider")
        && request_context.request_method == http::Method::PATCH
        && request_context
            .request_path
            .starts_with("/api/admin/providers/")
    {
        let Some(provider_id) = admin_provider_id_for_manage_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        if !state.has_provider_catalog_data_reader() || !state.has_provider_catalog_data_writer() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let raw_value = match serde_json::from_slice::<serde_json::Value>(request_body) {
            Ok(value) => value,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(raw_payload) = raw_value.as_object().cloned() else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                )
                    .into_response(),
            ));
        };
        let payload = match serde_json::from_value::<AdminProviderUpdateRequest>(raw_value) {
            Ok(payload) => payload,
            Err(_) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": "请求体必须是合法的 JSON 对象" })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(existing_provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        let updated_record = match build_admin_update_provider_record(
            state,
            &existing_provider,
            &raw_payload,
            payload,
        )
        .await
        {
            Ok(record) => record,
            Err(detail) => {
                return Ok(Some(
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({ "detail": detail })),
                    )
                        .into_response(),
                ));
            }
        };
        let Some(_updated) = state
            .update_provider_catalog_provider(&updated_record)
            .await?
        else {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        };
        return Ok(Some(
            match build_admin_provider_summary_payload(state, &provider_id).await {
                Some(payload) => attach_admin_audit_response(
                    Json(payload).into_response(),
                    "admin_provider_updated",
                    "update_provider",
                    "provider",
                    &provider_id,
                ),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("delete_provider")
        && request_context.request_method == http::Method::DELETE
    {
        let Some(provider_id) = admin_provider_id_for_manage_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let Some(provider) = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "提供商不存在" })),
                )
                    .into_response(),
            ));
        };
        let task_id = Uuid::new_v4().simple().to_string()[..16].to_string();
        let pending_task = LocalProviderDeleteTaskState {
            task_id: task_id.clone(),
            provider_id: provider.id.clone(),
            status: "pending".to_string(),
            stage: "queued".to_string(),
            total_keys: 0,
            deleted_keys: 0,
            total_endpoints: 0,
            deleted_endpoints: 0,
            message: "delete task submitted".to_string(),
        };
        put_admin_provider_delete_task(state, &pending_task);
        if let Err(err) = run_admin_provider_delete_task(state, &provider.id, &task_id).await {
            warn!(
                "gateway admin provider delete task failed for provider {}: {:?}",
                provider.id, err
            );
            put_admin_provider_delete_task(
                state,
                &LocalProviderDeleteTaskState {
                    task_id: task_id.clone(),
                    provider_id: provider.id.clone(),
                    status: "failed".to_string(),
                    stage: "failed".to_string(),
                    total_keys: 0,
                    deleted_keys: 0,
                    total_endpoints: 0,
                    deleted_endpoints: 0,
                    message: format!("provider delete failed: {err:?}"),
                },
            );
        }
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "task_id": task_id,
                "status": "pending",
                "message": "删除任务已提交，提供商已进入后台删除队列",
            }))
            .into_response(),
            "admin_provider_delete_queued",
            "delete_provider",
            "provider",
            &provider.id,
        )));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("summary_list")
        && request_context.request_path == "/api/admin/providers/summary"
    {
        if !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let page = query_param_value(request_context.request_query_string.as_deref(), "page")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(1);
        let page_size =
            query_param_value(request_context.request_query_string.as_deref(), "page_size")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0 && *value <= 10_000)
                .unwrap_or(20);
        let search = query_param_value(request_context.request_query_string.as_deref(), "search")
            .unwrap_or_default();
        let status = query_param_value(request_context.request_query_string.as_deref(), "status")
            .unwrap_or_else(|| "all".to_string());
        let api_format = query_param_value(
            request_context.request_query_string.as_deref(),
            "api_format",
        )
        .unwrap_or_else(|| "all".to_string());
        let model_id =
            query_param_value(request_context.request_query_string.as_deref(), "model_id")
                .unwrap_or_else(|| "all".to_string());
        let Some(payload) = build_admin_providers_summary_payload(
            state,
            page,
            page_size,
            &search,
            &status,
            &api_format,
            &model_id,
        )
        .await
        else {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("provider_summary")
        && request_context
            .request_path
            .starts_with("/api/admin/providers/")
        && request_context.request_path.ends_with("/summary")
    {
        if !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_providers_data_unavailable_response()));
        }
        let Some(provider_id) = admin_provider_id_for_summary(&request_context.request_path) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match build_admin_provider_summary_payload(state, &provider_id).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("delete_provider_task")
        && request_context.request_method == http::Method::GET
    {
        let Some((provider_id, task_id)) =
            admin_provider_delete_task_parts(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Task not found" })),
                )
                    .into_response(),
            ));
        };
        let Some(task) = state.get_provider_delete_task(&task_id) else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Task not found" })),
                )
                    .into_response(),
            ));
        };
        if task.provider_id != provider_id {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Task not found" })),
                )
                    .into_response(),
            ));
        }
        return Ok(Some(attach_admin_provider_delete_task_terminal_audit(
            &provider_id,
            &task_id,
            task.status.as_str(),
            Json(build_admin_provider_delete_task_payload(&task)).into_response(),
        )));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("health_monitor")
        && request_context
            .request_path
            .starts_with("/api/admin/providers/")
        && request_context.request_path.ends_with("/health-monitor")
    {
        let Some(provider_id) = admin_provider_id_for_health_monitor(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        let lookback_hours = query_param_value(
            request_context.request_query_string.as_deref(),
            "lookback_hours",
        )
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=72).contains(value))
        .unwrap_or(6);
        let per_endpoint_limit = query_param_value(
            request_context.request_query_string.as_deref(),
            "per_endpoint_limit",
        )
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (10..=200).contains(value))
        .unwrap_or(48);
        return Ok(Some(
            match build_admin_provider_health_monitor_payload(
                state,
                &provider_id,
                lookback_hours,
                per_endpoint_limit,
            )
            .await
            {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("mapping_preview")
        && request_context
            .request_path
            .starts_with("/api/admin/providers/")
        && request_context.request_path.ends_with("/mapping-preview")
    {
        let Some(provider_id) =
            admin_provider_id_for_mapping_preview(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match build_admin_provider_mapping_preview_payload(state, &provider_id).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("pool_status")
        && request_context.request_method == http::Method::GET
        && request_context.request_path.ends_with("/pool-status")
    {
        let Some(provider_id) = admin_provider_id_for_pool_status(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Provider 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match build_admin_provider_pool_status_payload(state, &provider_id).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("clear_pool_cooldown")
        && request_context.request_method == http::Method::POST
    {
        let Some((provider_id, key_id)) =
            admin_provider_clear_pool_cooldown_parts(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let provider_exists = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
            .is_some();
        if !provider_exists {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            ));
        }
        let key = state
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .find(|key| key.id == key_id);
        let Some(key) = key else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        clear_admin_provider_pool_cooldown(state, &provider_id, &key_id).await;
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "message": format!("已清除 Key {} 的冷却状态", key.name),
            }))
            .into_response(),
            "admin_provider_pool_cooldown_cleared",
            "clear_provider_pool_cooldown",
            "provider_key",
            &key_id,
        )));
    }

    if decision.route_family.as_deref() == Some("providers_manage")
        && decision.route_kind.as_deref() == Some("reset_pool_cost")
        && request_context.request_method == http::Method::POST
    {
        let Some((provider_id, key_id)) =
            admin_provider_reset_pool_cost_parts(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "Key 不存在" })),
                )
                    .into_response(),
            ));
        };
        let provider_exists = state
            .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .next()
            .is_some();
        if !provider_exists {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Provider {provider_id} 不存在") })),
                )
                    .into_response(),
            ));
        }
        let key = state
            .list_provider_catalog_keys_by_provider_ids(std::slice::from_ref(&provider_id))
            .await?
            .into_iter()
            .find(|key| key.id == key_id);
        let Some(key) = key else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("Key {key_id} 不存在") })),
                )
                    .into_response(),
            ));
        };
        reset_admin_provider_pool_cost(state, &provider_id, &key_id).await;
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "message": format!("已重置 Key {} 的成本窗口", key.name),
            }))
            .into_response(),
            "admin_provider_pool_cost_reset",
            "reset_provider_pool_cost",
            "provider_key",
            &key_id,
        )));
    }

    Ok(None)
}
