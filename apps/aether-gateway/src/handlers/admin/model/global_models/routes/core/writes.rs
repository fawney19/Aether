use super::super::super::super::{
    build_admin_assign_global_model_to_providers_payload, build_admin_global_model_create_record,
    build_admin_global_model_response, build_admin_global_model_update_record,
    resolve_admin_global_model_by_id_or_err,
};
use super::super::super::helpers::{
    build_admin_global_models_data_unavailable_response,
    ADMIN_GLOBAL_MODELS_DATA_UNAVAILABLE_DETAIL,
};
use super::shared::{
    bad_request_response, current_unix_secs, global_model_missing_response,
    global_model_not_found_response, not_found_detail_response, parse_required_json_body,
    parse_required_json_value, require_json_object,
};
use crate::handlers::admin::model::shared::{
    admin_global_model_assign_to_providers_id, admin_global_model_id_from_path,
    is_admin_global_models_root, AdminBatchAssignToProvidersRequest, AdminBatchDeleteIdsRequest,
    AdminGlobalModelCreateRequest, AdminGlobalModelUpdatePatch,
};
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::handlers::admin::shared::attach_admin_audit_response;
use crate::GatewayError;
use axum::{
    body::{Body, Bytes},
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

pub(super) async fn maybe_build_local_admin_global_models_write_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(decision) = request_context.decision() else {
        return Ok(None);
    };

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("create_global_model")
        && request_context.method() == http::Method::POST
        && is_admin_global_models_root(request_context.path())
    {
        return Ok(Some(
            build_create_global_model_response(state, request_body).await?,
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("update_global_model")
        && request_context.method() == http::Method::PATCH
    {
        return Ok(Some(
            build_update_global_model_response(state, request_context, request_body).await?,
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("delete_global_model")
        && request_context.method() == http::Method::DELETE
    {
        return Ok(Some(
            build_delete_global_model_response(state, request_context).await?,
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("batch_delete_global_models")
        && request_context.method() == http::Method::POST
        && request_context.path() == "/api/admin/models/global/batch-delete"
    {
        return Ok(Some(
            build_batch_delete_global_models_response(state, request_body).await?,
        ));
    }

    if decision.route_family.as_deref() == Some("global_models_manage")
        && decision.route_kind.as_deref() == Some("assign_to_providers")
        && request_context.method() == http::Method::POST
    {
        return Ok(Some(
            build_assign_to_providers_response(state, request_context, request_body).await?,
        ));
    }

    Ok(None)
}

async fn build_create_global_model_response(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_global_model_data_reader() || !state.has_global_model_data_writer() {
        return Ok(build_admin_global_models_data_unavailable_response());
    }
    let payload = match parse_required_json_body::<AdminGlobalModelCreateRequest>(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let record = match build_admin_global_model_create_record(state, payload).await {
        Ok(record) => record,
        Err(detail) => return Ok(bad_request_response(detail)),
    };

    Ok(match state.create_admin_global_model(&record).await? {
        Some(created) => attach_admin_audit_response(
            (
                http::StatusCode::CREATED,
                Json(build_admin_global_model_response(
                    &created,
                    current_unix_secs(),
                )),
            )
                .into_response(),
            "admin_global_model_created",
            "create_global_model",
            "global_model",
            &created.id,
        ),
        None => (
            http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "detail": ADMIN_GLOBAL_MODELS_DATA_UNAVAILABLE_DETAIL })),
        )
            .into_response(),
    })
}

async fn build_update_global_model_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_global_model_data_reader() || !state.has_global_model_data_writer() {
        return Ok(build_admin_global_models_data_unavailable_response());
    }
    let Some(global_model_id) = admin_global_model_id_from_path(request_context.path()) else {
        return Ok(global_model_missing_response());
    };
    let existing = match resolve_admin_global_model_by_id_or_err(state, &global_model_id).await {
        Ok(model) => model,
        Err(detail) => return Ok(not_found_detail_response(detail)),
    };

    let raw_value = match parse_required_json_value(request_body) {
        Ok(value) => value,
        Err(response) => return Ok(response),
    };
    let raw_payload = match require_json_object(&raw_value) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let patch = match AdminGlobalModelUpdatePatch::from_object(raw_payload) {
        Ok(patch) => patch,
        Err(_) => return Ok(bad_request_response("请求体必须是合法的 JSON 对象")),
    };
    let record = match build_admin_global_model_update_record(state, &existing, patch).await {
        Ok(record) => record,
        Err(detail) => return Ok(bad_request_response(detail)),
    };

    Ok(match state.update_admin_global_model(&record).await? {
        Some(updated) => attach_admin_audit_response(
            Json(build_admin_global_model_response(
                &updated,
                current_unix_secs(),
            ))
            .into_response(),
            "admin_global_model_updated",
            "update_global_model",
            "global_model",
            &updated.id,
        ),
        None => global_model_not_found_response(&existing.id),
    })
}

async fn build_delete_global_model_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_global_model_data_reader() || !state.has_global_model_data_writer() {
        return Ok(build_admin_global_models_data_unavailable_response());
    }
    let Some(global_model_id) = admin_global_model_id_from_path(request_context.path()) else {
        return Ok(global_model_missing_response());
    };
    let existing = match resolve_admin_global_model_by_id_or_err(state, &global_model_id).await {
        Ok(model) => model,
        Err(detail) => return Ok(not_found_detail_response(detail)),
    };
    if !state.delete_admin_global_model(&existing.id).await? {
        return Ok(global_model_not_found_response(&existing.id));
    }
    Ok(attach_admin_audit_response(
        http::StatusCode::NO_CONTENT.into_response(),
        "admin_global_model_deleted",
        "delete_global_model",
        "global_model",
        &existing.id,
    ))
}

async fn build_batch_delete_global_models_response(
    state: &AdminAppState<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    if !state.has_global_model_data_reader() || !state.has_global_model_data_writer() {
        return Ok(build_admin_global_models_data_unavailable_response());
    }
    let payload = match parse_required_json_body::<AdminBatchDeleteIdsRequest>(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
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

    Ok(attach_admin_audit_response(
        Json(json!({
            "success_count": success_count,
            "failed": failed,
        }))
        .into_response(),
        "admin_global_models_batch_deleted",
        "batch_delete_global_models",
        "global_models_batch",
        "batch",
    ))
}

async fn build_assign_to_providers_response(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response<Body>, GatewayError> {
    let Some(global_model_id) = admin_global_model_assign_to_providers_id(request_context.path())
    else {
        return Ok(global_model_missing_response());
    };
    let payload = match parse_required_json_body::<AdminBatchAssignToProvidersRequest>(request_body)
    {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };
    let payload: serde_json::Value = match build_admin_assign_global_model_to_providers_payload(
        state,
        &global_model_id,
        payload.provider_ids,
        payload.create_models.unwrap_or(false),
    )
    .await
    {
        Ok(payload) => payload,
        Err(detail) => return Ok(bad_request_response(detail)),
    };

    Ok(attach_admin_audit_response(
        Json(payload).into_response(),
        "admin_global_model_assigned_to_providers",
        "assign_global_model_to_providers",
        "global_model",
        &global_model_id,
    ))
}
