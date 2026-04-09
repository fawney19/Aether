use super::execution::{
    estimate_admin_provider_oauth_batch_import_total,
    execute_admin_provider_oauth_batch_import_for_provider_type,
};
use super::parse::{
    build_admin_provider_oauth_batch_task_state, parse_admin_provider_oauth_batch_import_request,
};
use crate::handlers::admin::provider::oauth::errors::build_internal_control_error_response;
use crate::handlers::admin::provider::oauth::state::{
    admin_provider_oauth_template, build_admin_provider_oauth_backend_unavailable_response,
    is_fixed_provider_type_for_provider_oauth,
};
use crate::handlers::admin::provider::shared::paths::admin_provider_oauth_batch_import_task_provider_id;
use crate::handlers::admin::request::{AdminAppState, AdminRequestContext};
use crate::GatewayError;
use axum::{
    body::Bytes,
    http,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::task;
use uuid::Uuid;

const PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES: usize = 20;

pub(in super::super) async fn handle_admin_provider_oauth_start_batch_import_task(
    state: &AdminAppState<'_>,
    request_context: &AdminRequestContext<'_>,
    request_body: Option<&Bytes>,
) -> Result<Response, GatewayError> {
    if !state.has_provider_catalog_data_reader() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }
    let Some(provider_id) =
        admin_provider_oauth_batch_import_task_provider_id(request_context.path())
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let payload = match parse_admin_provider_oauth_batch_import_request(request_body) {
        Ok(payload) => payload,
        Err(response) => return Ok(response),
    };

    let Some(provider) = state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(build_internal_control_error_response(
            http::StatusCode::NOT_FOUND,
            "Provider 不存在",
        ));
    };
    let provider_type = provider.provider_type.trim().to_ascii_lowercase();
    if !is_fixed_provider_type_for_provider_oauth(&provider_type) {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "该 Provider 不是固定类型，无法使用 provider-oauth",
        ));
    }
    if provider_type != "kiro" && admin_provider_oauth_template(&provider_type).is_none() {
        return Ok(build_admin_provider_oauth_backend_unavailable_response());
    }

    let total = estimate_admin_provider_oauth_batch_import_total(
        &provider_type,
        payload.credentials.as_str(),
    );
    if total == 0 {
        return Ok(build_internal_control_error_response(
            http::StatusCode::BAD_REQUEST,
            "未找到有效的 Token 数据",
        ));
    }

    let task_id = Uuid::new_v4().to_string();
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let submitted_state = build_admin_provider_oauth_batch_task_state(
        &task_id,
        &provider_id,
        &provider_type,
        "submitted",
        total,
        0,
        0,
        0,
        Some("任务已提交，等待执行"),
        None,
        Vec::new(),
        created_at,
        None,
        None,
    );
    if state
        .save_provider_oauth_batch_task_payload(&task_id, &submitted_state)
        .await
        .is_err()
    {
        return Ok(build_internal_control_error_response(
            http::StatusCode::SERVICE_UNAVAILABLE,
            "provider oauth batch task redis unavailable",
        ));
    }

    let task_state = state.cloned_app();
    let task_id_for_worker = task_id.clone();
    let provider_id_for_worker = provider_id.clone();
    let provider_type_for_worker = provider_type.clone();
    let proxy_node_id = payload.proxy_node_id.clone();
    let raw_credentials = payload.credentials.clone();
    task::spawn(async move {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs())
            .unwrap_or(created_at);
        let processing_state = build_admin_provider_oauth_batch_task_state(
            &task_id_for_worker,
            &provider_id_for_worker,
            &provider_type_for_worker,
            "processing",
            total,
            0,
            0,
            0,
            Some("任务开始执行"),
            None,
            Vec::new(),
            created_at,
            Some(started_at),
            None,
        );
        let _ = AdminAppState::new(&task_state)
            .save_provider_oauth_batch_task_payload(&task_id_for_worker, &processing_state)
            .await;

        match execute_admin_provider_oauth_batch_import_for_provider_type(
            &AdminAppState::new(&task_state),
            &provider_id_for_worker,
            &provider_type_for_worker,
            raw_credentials.as_str(),
            proxy_node_id.as_deref(),
        )
        .await
        {
            Ok(outcome) => {
                let finished_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(started_at);
                let error_samples = outcome
                    .results
                    .iter()
                    .filter(|item| {
                        item.get("status").and_then(serde_json::Value::as_str) == Some("error")
                    })
                    .take(PROVIDER_OAUTH_BATCH_TASK_MAX_ERROR_SAMPLES)
                    .cloned()
                    .collect::<Vec<_>>();
                let message = format!(
                    "导入完成：成功 {}，失败 {}",
                    outcome.success, outcome.failed
                );
                let completed_state = build_admin_provider_oauth_batch_task_state(
                    &task_id_for_worker,
                    &provider_id_for_worker,
                    &provider_type_for_worker,
                    "completed",
                    outcome.total,
                    outcome.total,
                    outcome.success,
                    outcome.failed,
                    Some(message.as_str()),
                    None,
                    error_samples,
                    created_at,
                    Some(started_at),
                    Some(finished_at),
                );
                let _ = AdminAppState::new(&task_state)
                    .save_provider_oauth_batch_task_payload(&task_id_for_worker, &completed_state)
                    .await;
            }
            Err(err) => {
                let finished_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs())
                    .unwrap_or(started_at);
                let error_message = format!("{err:?}");
                let failed_state = build_admin_provider_oauth_batch_task_state(
                    &task_id_for_worker,
                    &provider_id_for_worker,
                    &provider_type_for_worker,
                    "failed",
                    total,
                    0,
                    0,
                    0,
                    Some("导入任务执行失败"),
                    Some(error_message.as_str()),
                    Vec::new(),
                    created_at,
                    Some(started_at),
                    Some(finished_at),
                );
                let _ = AdminAppState::new(&task_state)
                    .save_provider_oauth_batch_task_payload(&task_id_for_worker, &failed_state)
                    .await;
                tracing::warn!(
                    task_id = %task_id_for_worker,
                    provider_id = %provider_id_for_worker,
                    error = %error_message,
                    "provider oauth batch import task failed"
                );
            }
        }
    });

    let submitted_response = build_admin_provider_oauth_batch_task_state(
        &task_id,
        &provider_id,
        &provider_type,
        "submitted",
        total,
        0,
        0,
        0,
        Some("任务已提交，等待执行"),
        None,
        Vec::new(),
        created_at,
        None,
        None,
    );
    Ok(Json(submitted_response).into_response())
}
