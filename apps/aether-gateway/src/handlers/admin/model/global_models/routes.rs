use super::super::{
    build_admin_assign_global_model_to_providers_payload, build_admin_global_model_create_record,
    build_admin_global_model_payload, build_admin_global_model_providers_payload,
    build_admin_global_model_response, build_admin_global_model_routing_payload,
    build_admin_global_model_update_record, build_admin_global_models_payload,
    resolve_admin_global_model_by_id_or_err,
};
use super::helpers::{
    build_admin_global_models_data_unavailable_response,
    ADMIN_GLOBAL_MODELS_DATA_UNAVAILABLE_DETAIL,
};
use crate::control::GatewayPublicRequestContext;
use crate::handlers::admin::model::shared::{
    admin_global_model_assign_to_providers_id, admin_global_model_id_from_path,
    admin_global_model_providers_id, admin_global_model_routing_id, is_admin_global_models_root,
    AdminBatchAssignToProvidersRequest, AdminBatchDeleteIdsRequest, AdminGlobalModelCreateRequest,
    AdminGlobalModelUpdateRequest,
};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::handlers::admin::shared::{query_param_optional_bool, query_param_value};
use crate::{AppState, GatewayError};
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn maybe_build_local_admin_global_models_response(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.control_decision.as_ref() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("routing_preview")
        && request_context.request_method == http::Method::GET
    {
        if !state.has_global_model_data_reader() || !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        }
        let Some(global_model_id) = admin_global_model_routing_id(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "GlobalModel 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match build_admin_global_model_routing_payload(state, &global_model_id).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("GlobalModel {global_model_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("list_global_models")
        && is_admin_global_models_root(&request_context.request_path)
    {
        if !state.has_global_model_data_reader() {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        }
        let skip = query_param_value(request_context.request_query_string.as_deref(), "skip")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query_param_value(request_context.request_query_string.as_deref(), "limit")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0 && *value <= 1000)
            .unwrap_or(100);
        let is_active =
            query_param_optional_bool(request_context.request_query_string.as_deref(), "is_active");
        let search = query_param_value(request_context.request_query_string.as_deref(), "search");
        let Some(payload) =
            build_admin_global_models_payload(state, skip, limit, is_active, search).await
        else {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        };
        return Ok(Some(Json(payload).into_response()));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("get_global_model")
        && request_context.request_method == http::Method::GET
    {
        if !state.has_global_model_data_reader() {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        }
        let Some(global_model_id) = admin_global_model_id_from_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "GlobalModel 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match build_admin_global_model_payload(state, &global_model_id).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("GlobalModel {global_model_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("create_global_model")
        && request_context.request_method == http::Method::POST
        && is_admin_global_models_root(&request_context.request_path)
    {
        if !state.has_global_model_data_reader() || !state.has_global_model_data_writer() {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        }
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let payload = match serde_json::from_slice::<AdminGlobalModelCreateRequest>(request_body) {
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
        let record = match build_admin_global_model_create_record(state, payload).await {
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
        return Ok(Some(
            match state.create_admin_global_model(&record).await? {
                Some(created) => {
                    let now_unix_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    attach_admin_audit_response(
                        (
                            http::StatusCode::CREATED,
                            Json(build_admin_global_model_response(&created, now_unix_secs)),
                        )
                            .into_response(),
                        "admin_global_model_created",
                        "create_global_model",
                        "global_model",
                        &created.id,
                    )
                }
                None => (
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "detail": ADMIN_GLOBAL_MODELS_DATA_UNAVAILABLE_DETAIL })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("update_global_model")
        && request_context.request_method == http::Method::PATCH
    {
        if !state.has_global_model_data_reader() || !state.has_global_model_data_writer() {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        }
        let Some(global_model_id) = admin_global_model_id_from_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "GlobalModel 不存在" })),
                )
                    .into_response(),
            ));
        };
        let existing = match resolve_admin_global_model_by_id_or_err(state, &global_model_id).await
        {
            Ok(model) => model,
            Err(detail) => {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": detail })),
                    )
                        .into_response(),
                ));
            }
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
        let payload = match serde_json::from_value::<AdminGlobalModelUpdateRequest>(raw_value) {
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
        let record =
            match build_admin_global_model_update_record(state, &existing, &raw_payload, payload)
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
        return Ok(Some(
            match state.update_admin_global_model(&record).await? {
                Some(updated) => {
                    let now_unix_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    attach_admin_audit_response(
                        Json(build_admin_global_model_response(&updated, now_unix_secs))
                            .into_response(),
                        "admin_global_model_updated",
                        "update_global_model",
                        "global_model",
                        &updated.id,
                    )
                }
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("GlobalModel {} 不存在", existing.id) })),
                )
                    .into_response(),
            },
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("delete_global_model")
        && request_context.request_method == http::Method::DELETE
    {
        if !state.has_global_model_data_reader() || !state.has_global_model_data_writer() {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        }
        let Some(global_model_id) = admin_global_model_id_from_path(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "GlobalModel 不存在" })),
                )
                    .into_response(),
            ));
        };
        let existing = match resolve_admin_global_model_by_id_or_err(state, &global_model_id).await
        {
            Ok(model) => model,
            Err(detail) => {
                return Ok(Some(
                    (
                        http::StatusCode::NOT_FOUND,
                        Json(json!({ "detail": detail })),
                    )
                        .into_response(),
                ));
            }
        };
        if !state.delete_admin_global_model(&existing.id).await? {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("GlobalModel {} 不存在", existing.id) })),
                )
                    .into_response(),
            ));
        }
        return Ok(Some(attach_admin_audit_response(
            http::StatusCode::NO_CONTENT.into_response(),
            "admin_global_model_deleted",
            "delete_global_model",
            "global_model",
            &existing.id,
        )));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("batch_delete_global_models")
        && request_context.request_method == http::Method::POST
        && request_context.request_path == "/api/admin/models/global/batch-delete"
    {
        if !state.has_global_model_data_reader() || !state.has_global_model_data_writer() {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        }
        let Some(request_body) = request_body else {
            return Ok(Some(
                (
                    http::StatusCode::BAD_REQUEST,
                    Json(json!({ "detail": "请求体不能为空" })),
                )
                    .into_response(),
            ));
        };
        let payload = match serde_json::from_slice::<AdminBatchDeleteIdsRequest>(request_body) {
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
        let mut success_count = 0usize;
        let mut failed = Vec::new();
        for id in payload.ids {
            let trimmed = id.trim();
            if trimmed.is_empty() {
                failed.push(json!({"id": id, "error": "not found"}));
                continue;
            }
            let Some(existing) = state.get_admin_global_model_by_id(trimmed).await? else {
                failed.push(json!({"id": trimmed, "error": "not found"}));
                continue;
            };
            if state.delete_admin_global_model(&existing.id).await? {
                success_count += 1;
            } else {
                failed.push(json!({"id": existing.id, "error": "delete failed"}));
            }
        }
        return Ok(Some(attach_admin_audit_response(
            Json(json!({
                "success_count": success_count,
                "failed": failed,
            }))
            .into_response(),
            "admin_global_models_batch_deleted",
            "batch_delete_global_models",
            "global_models_batch",
            "batch",
        )));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("assign_to_providers")
        && request_context.request_method == http::Method::POST
    {
        let Some(global_model_id) =
            admin_global_model_assign_to_providers_id(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "GlobalModel 不存在" })),
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
        let payload =
            match serde_json::from_slice::<AdminBatchAssignToProvidersRequest>(request_body) {
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
        let payload = match build_admin_assign_global_model_to_providers_payload(
            state,
            &global_model_id,
            payload.provider_ids,
            payload.create_models.unwrap_or(false),
        )
        .await
        {
            Ok(payload) => payload,
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
        return Ok(Some(attach_admin_audit_response(
            Json(payload).into_response(),
            "admin_global_model_assigned_to_providers",
            "assign_global_model_to_providers",
            "global_model",
            &global_model_id,
        )));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("global_model_providers")
        && request_context.request_method == http::Method::GET
    {
        if !state.has_global_model_data_reader() || !state.has_provider_catalog_data_reader() {
            return Ok(Some(build_admin_global_models_data_unavailable_response()));
        }
        let Some(global_model_id) = admin_global_model_providers_id(&request_context.request_path)
        else {
            return Ok(Some(
                (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": "GlobalModel 不存在" })),
                )
                    .into_response(),
            ));
        };
        return Ok(Some(
            match build_admin_global_model_providers_payload(state, &global_model_id).await {
                Some(payload) => Json(payload).into_response(),
                None => (
                    http::StatusCode::NOT_FOUND,
                    Json(json!({ "detail": format!("GlobalModel {global_model_id} 不存在") })),
                )
                    .into_response(),
            },
        ));
    }

    Ok(None)
}
