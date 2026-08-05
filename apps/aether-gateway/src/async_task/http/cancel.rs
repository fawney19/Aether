use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskStatus,
};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Map, Value};

use crate::{AppState, GatewayError};

use super::super::finalize_video_task_if_terminal;
use super::super::read_video_task_detail;
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
    let Some(task) = read_video_task_detail(state, task_id).await? else {
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
    if let Some(cancel_plan) = build_video_task_cancel_plan(&task) {
        state
            .hydrate_video_task_for_route(Some(cancel_plan.route_family), &cancel_plan.request_path)
            .await?;

        let body_json = json!({});
        let follow_up = state.video_tasks.prepare_follow_up_sync_plan(
            cancel_plan.plan_kind,
            &cancel_plan.request_path,
            Some(&body_json),
            None,
            &trace_id,
        );

        if let Some(follow_up) = follow_up {
            execute_video_task_cancel_plan(state, &trace_id, follow_up.plan)
                .await
                .map_err(CancelVideoTaskError::Response)?;
        }

        state
            .video_tasks
            .apply_finalize_mutation(&cancel_plan.request_path, cancel_plan.report_kind);
    }

    let request_metadata = build_cancelled_request_metadata(state, &task).await?;
    let stored = persist_cancelled_video_task(state, &task, request_metadata)
        .await?
        .ok_or_else(|| {
            CancelVideoTaskError::Gateway(GatewayError::Internal(
                "video task repository is unavailable".to_string(),
            ))
        })?;
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
    let provider_api_format = task
        .provider_api_format
        .as_deref()
        .or(task.client_api_format.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

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
        // Ark folds cancel into delete, so an admin-initiated cancel retires the task.
        "doubao:video" => Some(VideoTaskCancelPlan {
            route_family: "doubao",
            plan_kind: "doubao_video_delete_sync",
            report_kind: "doubao_video_delete_sync_finalize",
            request_path: format!(
                "{}/{}",
                aether_video_tasks_core::DOUBAO_VIDEO_TASKS_PATH,
                task.id
            ),
        }),
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
            .map_err(|err| {
                GatewayError::UpstreamUnavailable {
                    trace_id: trace_id.to_string(),
                    message: format!("{err:?}"),
                }
                .into_response()
            })?;

    if result.status_code >= 400 {
        let status = axum::http::StatusCode::from_u16(result.status_code)
            .unwrap_or(axum::http::StatusCode::BAD_GATEWAY);
        let body_json = result
            .body
            .and_then(|body| body.json_body)
            .unwrap_or_else(|| {
                json!({
                    "error": {
                        "message": result
                            .error
                            .as_ref()
                            .map(|error| error.message.clone())
                            .unwrap_or_else(|| {
                                format!("execution runtime returned {}", result.status_code)
                            }),
                    }
                })
            });
        return Err((status, Json(body_json)).into_response());
    }

    Ok(())
}

async fn build_cancelled_request_metadata(
    state: &AppState,
    task: &StoredVideoTask,
) -> Result<Option<Value>, GatewayError> {
    let mut metadata = match task.request_metadata.clone() {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    let mut snapshot_value = metadata.get("rust_local_snapshot").cloned();
    if snapshot_value.is_none() {
        snapshot_value = state
            .reconstruct_video_task_snapshot(task)
            .await?
            .map(|snapshot| {
                serde_json::to_value(snapshot)
                    .map_err(|err| GatewayError::Internal(err.to_string()))
            })
            .transpose()?;
    }
    if let Some(snapshot_value_ref) = snapshot_value.as_mut() {
        mark_snapshot_value_cancelled(snapshot_value_ref);
        metadata.insert(
            "rust_owner".to_string(),
            Value::String("async_task".to_string()),
        );
        metadata.insert(
            "rust_local_snapshot".to_string(),
            snapshot_value_ref.clone(),
        );
        return Ok(Some(Value::Object(metadata)));
    }

    Ok(task.request_metadata.clone())
}

fn mark_snapshot_value_cancelled(snapshot_value: &mut Value) {
    if let Some(object) = snapshot_value
        .get_mut("OpenAi")
        .and_then(Value::as_object_mut)
    {
        object.insert("status".to_string(), Value::String("Cancelled".to_string()));
        return;
    }
    if let Some(object) = snapshot_value
        .get_mut("Gemini")
        .and_then(Value::as_object_mut)
    {
        object.insert("status".to_string(), Value::String("Cancelled".to_string()));
    }
}

async fn persist_cancelled_video_task(
    state: &AppState,
    task: &StoredVideoTask,
    request_metadata: Option<Value>,
) -> Result<Option<StoredVideoTask>, GatewayError> {
    let now_unix_secs = current_unix_secs();
    state
        .data
        .upsert_video_task(UpsertVideoTask {
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
            original_request_body: task.original_request_body.clone(),
            duration_seconds: task.duration_seconds,
            resolution: task.resolution.clone(),
            aspect_ratio: task.aspect_ratio.clone(),
            size: task.size.clone(),
            status: VideoTaskStatus::Cancelled,
            progress_percent: task.progress_percent,
            progress_message: task.progress_message.clone(),
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
            error_message: task.error_message.clone(),
            video_url: task.video_url.clone(),
            request_metadata,
        })
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))
}
