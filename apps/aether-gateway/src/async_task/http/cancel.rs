use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskStatus,
};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::state::VideoTaskRouteAccess;
use crate::{AppState, GatewayError};

use super::super::finalize_video_task_if_terminal;
use super::super::{read_video_task_detail, read_video_task_detail_for_user};
use super::current_unix_secs;

#[derive(Debug)]
pub(crate) enum CancelVideoTaskError {
    NotFound,
    InvalidStatus(VideoTaskStatus),
    Response(axum::response::Response),
    Gateway(GatewayError),
}

impl From<GatewayError> for CancelVideoTaskError {
    fn from(value: GatewayError) -> Self {
        Self::Gateway(value)
    }
}

pub(crate) async fn cancel_video_task_record(
    state: &AppState,
    task_id: &str,
) -> Result<StoredVideoTask, CancelVideoTaskError> {
    cancel_video_task_record_inner(state, task_id, None).await
}

pub(crate) async fn cancel_video_task_record_for_user(
    state: &AppState,
    task_id: &str,
    user_id: &str,
) -> Result<StoredVideoTask, CancelVideoTaskError> {
    let user_id = user_id.trim();
    if user_id.is_empty() {
        return Err(CancelVideoTaskError::NotFound);
    }
    cancel_video_task_record_inner(state, task_id, Some(user_id)).await
}

async fn cancel_video_task_record_inner(
    state: &AppState,
    task_id: &str,
    expected_user_id: Option<&str>,
) -> Result<StoredVideoTask, CancelVideoTaskError> {
    let task = match expected_user_id {
        Some(user_id) => read_video_task_detail_for_user(state, task_id, user_id).await?,
        None => read_video_task_detail(state, task_id).await?,
    };
    let Some(task) = task else {
        return Err(CancelVideoTaskError::NotFound);
    };

    if matches!(
        task.status,
        VideoTaskStatus::Completed
            | VideoTaskStatus::Failed
            | VideoTaskStatus::Cancelled
            | VideoTaskStatus::Expired
            | VideoTaskStatus::Deleted
    ) {
        return Err(CancelVideoTaskError::InvalidStatus(task.status));
    }

    let trace_id = format!("async-task-admin-cancel-{task_id}");
    let mut finalize_mutation = None;
    if let Some(cancel_plan) = build_video_task_cancel_plan(&task) {
        let body_json = json!({});
        let follow_up = if let Some(user_id) = expected_user_id {
            if state
                .hydrate_video_task_for_route_for_user(
                    Some(cancel_plan.route_family),
                    &cancel_plan.request_path,
                    user_id,
                )
                .await?
                != VideoTaskRouteAccess::Allowed
            {
                return Err(CancelVideoTaskError::NotFound);
            }
            state.video_tasks.prepare_follow_up_sync_plan_for_user_id(
                cancel_plan.plan_kind,
                &cancel_plan.request_path,
                Some(&body_json),
                user_id,
                task.api_key_id.as_deref(),
                &trace_id,
            )
        } else {
            state
                .hydrate_video_task_for_route(
                    Some(cancel_plan.route_family),
                    &cancel_plan.request_path,
                )
                .await?;
            state.video_tasks.prepare_follow_up_sync_plan(
                cancel_plan.plan_kind,
                &cancel_plan.request_path,
                Some(&body_json),
                None,
                &trace_id,
            )
        };

        if let Some(follow_up) = follow_up {
            execute_video_task_cancel_plan(state, &trace_id, follow_up.plan)
                .await
                .map_err(CancelVideoTaskError::Response)?;
            finalize_mutation = Some((
                cancel_plan.request_path,
                cancel_plan.report_kind.to_string(),
            ));
        } else if expected_user_id.is_none() {
            finalize_mutation = Some((
                cancel_plan.request_path,
                cancel_plan.report_kind.to_string(),
            ));
        }
    }

    let stored = match persist_cancelled_video_task(state, &task).await? {
        Some(stored) => stored,
        None => {
            let current = match expected_user_id {
                Some(user_id) => read_video_task_detail_for_user(state, task_id, user_id).await?,
                None => read_video_task_detail(state, task_id).await?,
            };
            let Some(current) = current else {
                return Err(CancelVideoTaskError::NotFound);
            };
            if !current.status.is_active() {
                return Err(CancelVideoTaskError::InvalidStatus(current.status));
            }
            return Err(CancelVideoTaskError::Gateway(GatewayError::Internal(
                "video task repository is unavailable".to_string(),
            )));
        }
    };
    if let Some((request_path, report_kind)) = finalize_mutation {
        state
            .video_tasks
            .apply_finalize_mutation(&request_path, &report_kind);
    }
    finalize_video_task_if_terminal(state, &stored).await;
    Ok(stored)
}

#[derive(Debug, Clone)]
struct VideoTaskCancelPlan<'a> {
    route_family: &'a str,
    plan_kind: &'a str,
    report_kind: &'a str,
    request_path: String,
}

fn build_video_task_cancel_plan(task: &StoredVideoTask) -> Option<VideoTaskCancelPlan<'_>> {
    let provider_api_format = task.effective_api_format()?;

    match provider_api_format {
        "openai:video" => Some(VideoTaskCancelPlan {
            route_family: "openai",
            plan_kind: "openai_video_cancel_sync",
            report_kind: "openai_video_cancel_sync_finalize",
            request_path: format!("/v1/videos/{}/cancel", task.id),
        }),
        "gemini:video" => {
            let short_id = task.short_id.as_deref().unwrap_or(task.id.as_str()).trim();
            let model = task
                .model
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(VideoTaskCancelPlan {
                route_family: "gemini",
                plan_kind: "gemini_video_cancel_sync",
                report_kind: "gemini_video_cancel_sync_finalize",
                request_path: format!("/v1beta/models/{model}/operations/{short_id}:cancel"),
            })
        }
        _ => None,
    }
}

async fn execute_video_task_cancel_plan(
    state: &AppState,
    trace_id: &str,
    plan: aether_contracts::ExecutionPlan,
) -> Result<(), axum::response::Response> {
    let result =
        crate::execution_runtime::execute_execution_runtime_sync_plan(state, Some(trace_id), &plan)
            .await
            .map_err(|_| {
                GatewayError::UpstreamUnavailable {
                    trace_id: trace_id.to_string(),
                    message: "video cancellation request failed".to_string(),
                }
                .into_response()
            })?;

    if result.status_code >= 400 {
        return Err(build_video_task_cancel_upstream_error_response(&result));
    }

    Ok(())
}

fn build_video_task_cancel_upstream_error_response(
    result: &aether_contracts::ExecutionResult,
) -> axum::response::Response {
    let status = axum::http::StatusCode::from_u16(result.status_code)
        .unwrap_or(axum::http::StatusCode::BAD_GATEWAY);
    tracing::warn!(
        event_name = "video_task_cancel_upstream_error",
        upstream_status = result.status_code,
        "video cancellation upstream response body discarded"
    );
    (
        status,
        Json(json!({
            "error": {
                "message": format!(
                    "video cancellation upstream returned HTTP {}",
                    result.status_code
                ),
            }
        })),
    )
        .into_response()
}

async fn persist_cancelled_video_task(
    state: &AppState,
    task: &StoredVideoTask,
) -> Result<Option<StoredVideoTask>, GatewayError> {
    let now_unix_secs = current_unix_secs();
    state
        .update_active_video_task(UpsertVideoTask {
            id: task.id.clone(),
            short_id: task.short_id.clone(),
            request_id: task.request_id.clone(),
            user_id: task.user_id.clone(),
            api_key_id: task.api_key_id.clone(),
            username: task.username.clone(),
            api_key_name: task.api_key_name.clone(),
            external_task_id: task.external_task_id.clone(),
            provider_id: task.provider_id.clone(),
            endpoint_id: task.endpoint_id.clone(),
            key_id: task.key_id.clone(),
            client_api_format: task.client_api_format.clone(),
            provider_api_format: task.provider_api_format.clone(),
            format_converted: task.format_converted,
            model: task.model.clone(),
            prompt: task.prompt.clone(),
            original_request_body: None,
            duration_seconds: task.duration_seconds,
            resolution: task.resolution.clone(),
            aspect_ratio: task.aspect_ratio.clone(),
            size: task.size.clone(),
            status: VideoTaskStatus::Cancelled,
            progress_percent: task.progress_percent,
            progress_message: None,
            retry_count: task.retry_count,
            poll_interval_seconds: task.poll_interval_seconds,
            next_poll_at_unix_secs: None,
            poll_count: task.poll_count,
            max_poll_count: task.max_poll_count,
            created_at_unix_ms: task.created_at_unix_ms,
            submitted_at_unix_secs: task.submitted_at_unix_secs,
            completed_at_unix_secs: Some(now_unix_secs),
            updated_at_unix_secs: now_unix_secs,
            error_code: task.error_code.clone(),
            error_message: None,
            video_url: task.video_url.clone(),
            request_metadata: None,
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_contracts::{
        ExecutionError, ExecutionErrorKind, ExecutionPhase, ExecutionResult, ResponseBody,
    };
    use axum::body::to_bytes;
    use serde_json::json;

    use super::build_video_task_cancel_upstream_error_response;

    #[tokio::test]
    async fn cancellation_upstream_errors_do_not_expose_runtime_payloads() {
        let result = ExecutionResult {
            request_id: "cancel-secret-request-id".to_string(),
            candidate_id: Some("cancel-secret-candidate-id".to_string()),
            status_code: 502,
            headers: BTreeMap::from([(
                "x-internal-secret".to_string(),
                "cancel-secret-header".to_string(),
            )]),
            response_observation: None,
            body: Some(ResponseBody {
                json_body: Some(json!({
                    "error": {
                        "message": "cancel-secret-upstream-body",
                    }
                })),
                body_bytes_b64: None,
            }),
            telemetry: None,
            error: Some(ExecutionError {
                kind: ExecutionErrorKind::Upstream5xx,
                phase: ExecutionPhase::FirstByte,
                message: "cancel-secret-runtime-error".to_string(),
                upstream_status: Some(502),
                retryable: true,
                failover_recommended: false,
            }),
        };

        let response = build_video_task_cancel_upstream_error_response(&result);
        assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
        assert!(response.headers().get("x-internal-secret").is_none());
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response body should parse");

        assert_eq!(
            payload,
            json!({
                "error": {
                    "message": "video cancellation upstream returned HTTP 502",
                }
            })
        );
        let body = String::from_utf8(body.to_vec()).expect("response body should be utf-8");
        assert!(!body.contains("cancel-secret"));
    }
}
