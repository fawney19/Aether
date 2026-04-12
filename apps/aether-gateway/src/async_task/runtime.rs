use std::time::Duration;

use aether_billing::enrich_usage_event_with_billing;
use aether_contracts::{ExecutionErrorKind, ExecutionResult};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskStatus,
};
use aether_usage_runtime::{build_upsert_usage_record_from_event, settle_usage_if_needed};
use serde_json::{Map, Value};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::log_ids::short_request_id;
use crate::usage::{UsageEvent, UsageEventData, UsageEventType};
use crate::video_tasks::{LocalVideoTaskReadRefreshPlan, LocalVideoTaskSnapshot};
use crate::{AppState, GatewayError};

const MAX_VIDEO_TASK_POLL_BACKOFF_SECONDS: u64 = 300;
const VIDEO_TASK_POLL_CLAIM_SECONDS: u64 = 30;

#[derive(Debug, Clone)]
struct VideoTaskRefreshError {
    message: String,
    permanent: bool,
}

enum VideoTaskRefreshAttempt {
    Success { provider_body: Map<String, Value> },
    Error(VideoTaskRefreshError),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct VideoTaskPollerConfig {
    pub(crate) interval: Duration,
    pub(crate) batch_size: usize,
}

pub(crate) async fn execute_video_task_refresh_plan(
    state: &AppState,
    refresh_plan: &LocalVideoTaskReadRefreshPlan,
) -> Result<bool, GatewayError> {
    match fetch_video_task_refresh_attempt(state, refresh_plan).await? {
        VideoTaskRefreshAttempt::Success { provider_body } => {
            let projected = state
                .video_tasks
                .apply_read_refresh_projection(refresh_plan, &provider_body);
            if projected {
                if let Some(snapshot) = state.video_tasks.snapshot_for_refresh_plan(refresh_plan) {
                    let _ = state.upsert_video_task_snapshot(&snapshot).await?;
                }
            }
            Ok(projected)
        }
        VideoTaskRefreshAttempt::Error(err) => {
            warn!(
                event_name = "video_task_refresh_failed",
                log_type = "event",
                error = %err.message,
                permanent = err.permanent,
                "gateway video task refresh failed"
            );
            Ok(false)
        }
    }
}

async fn poll_video_tasks_once(state: &AppState, batch_size: usize) -> Result<usize, GatewayError> {
    if !state.video_tasks.is_rust_authoritative() {
        return Ok(0);
    }
    let now_unix_secs = now_unix_secs();
    let tasks = state
        .claim_due_video_tasks(
            now_unix_secs,
            now_unix_secs.saturating_add(VIDEO_TASK_POLL_CLAIM_SECONDS),
            batch_size,
        )
        .await?;
    let mut refreshed = 0usize;
    for (index, task) in tasks.into_iter().enumerate() {
        let trace_id = format!("video-task-poller-{index}");
        let Some(refresh_plan) = state
            .video_tasks
            .prepare_poll_refresh_plan_for_stored_task(&task, &trace_id)
        else {
            continue;
        };

        match fetch_video_task_refresh_attempt(state, &refresh_plan).await? {
            VideoTaskRefreshAttempt::Success { provider_body } => {
                let Some(updated) =
                    build_successful_poll_update(&task, &provider_body, now_unix_secs)?
                else {
                    continue;
                };
                match state.update_active_video_task(updated).await? {
                    Some(stored) => {
                        if let Some(snapshot) = LocalVideoTaskSnapshot::from_stored_task(&stored) {
                            state.video_tasks.record_snapshot(snapshot);
                        }
                        info!(
                            event_name = "video_task_status_updated",
                            log_type = "event",
                            request_id = %short_request_id(stored.request_id.as_str()),
                            task_id = %stored.id,
                            status = ?stored.status,
                            "gateway updated video task status from poll refresh"
                        );
                        finalize_video_task_if_terminal(state, &stored).await;
                        refreshed += 1;
                    }
                    None => continue,
                }
            }
            VideoTaskRefreshAttempt::Error(err) => {
                let updated = build_failed_poll_update(&task, &err, now_unix_secs);
                match state.update_active_video_task(updated).await? {
                    Some(stored) => {
                        if let Some(snapshot) = LocalVideoTaskSnapshot::from_stored_task(&stored) {
                            state.video_tasks.record_snapshot(snapshot);
                        }
                        info!(
                            event_name = "video_task_status_updated",
                            log_type = "event",
                            request_id = %short_request_id(stored.request_id.as_str()),
                            task_id = %stored.id,
                            status = ?stored.status,
                            "gateway updated video task status from poll refresh"
                        );
                        finalize_video_task_if_terminal(state, &stored).await;
                        refreshed += 1;
                    }
                    None => continue,
                }
            }
        }
    }
    Ok(refreshed)
}

pub(crate) fn spawn_video_task_poller(state: AppState) -> Option<JoinHandle<()>> {
    let config = state.video_task_poller?;
    if !state.video_tasks.is_rust_authoritative() {
        return None;
    }

    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(err) = poll_video_tasks_once(&state, config.batch_size).await {
                warn!(
                    event_name = "video_task_poller_tick_failed",
                    log_type = "event",
                    error = ?err,
                    "gateway video task poller tick failed"
                );
            }
        }
    }))
}

async fn fetch_video_task_refresh_attempt(
    state: &AppState,
    refresh_plan: &LocalVideoTaskReadRefreshPlan,
) -> Result<VideoTaskRefreshAttempt, GatewayError> {
    let result = match crate::execution_runtime::execute_execution_runtime_sync_plan(
        state,
        None,
        &refresh_plan.plan,
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            return Ok(VideoTaskRefreshAttempt::Error(VideoTaskRefreshError {
                message: format!("{err:?}"),
                permanent: false,
            }));
        }
    };
    if result.status_code >= 400 {
        return Ok(VideoTaskRefreshAttempt::Error(
            classify_refresh_result_error(&result),
        ));
    }

    let Some(provider_body) = result
        .body
        .and_then(|body| body.json_body)
        .and_then(|body| body.as_object().cloned())
    else {
        return Ok(VideoTaskRefreshAttempt::Error(VideoTaskRefreshError {
            message: "video task refresh missing json provider body".to_string(),
            permanent: false,
        }));
    };

    Ok(VideoTaskRefreshAttempt::Success { provider_body })
}

fn classify_refresh_result_error(result: &ExecutionResult) -> VideoTaskRefreshError {
    let status_code = result
        .error
        .as_ref()
        .and_then(|error| error.upstream_status)
        .unwrap_or(result.status_code);
    let message = result
        .error
        .as_ref()
        .map(|error| error.message.clone())
        .or_else(|| {
            result
                .body
                .as_ref()
                .and_then(|body| body.json_body.as_ref())
                .and_then(|value| value.get("error"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("upstream returned {status_code}"));
    let permanent = result.error.as_ref().map_or(
        matches!(status_code, 400 | 401 | 403 | 404 | 422),
        |error| match error.kind {
            ExecutionErrorKind::Upstream4xx => !matches!(status_code, 408 | 409 | 429),
            ExecutionErrorKind::Upstream5xx
            | ExecutionErrorKind::ConnectTimeout
            | ExecutionErrorKind::FirstByteTimeout
            | ExecutionErrorKind::ReadTimeout
            | ExecutionErrorKind::TlsError
            | ExecutionErrorKind::ProxyError
            | ExecutionErrorKind::ProtocolError
            | ExecutionErrorKind::Internal => false,
            ExecutionErrorKind::Cancelled => true,
        },
    );

    VideoTaskRefreshError { message, permanent }
}

fn build_successful_poll_update(
    task: &StoredVideoTask,
    provider_body: &Map<String, Value>,
    now_unix_secs: u64,
) -> Result<Option<UpsertVideoTask>, GatewayError> {
    let Some(mut snapshot) = LocalVideoTaskSnapshot::from_stored_task(task) else {
        return Ok(None);
    };
    snapshot.apply_provider_body(provider_body);

    let mut record = snapshot.to_upsert_record();
    record.id = task.id.clone();
    record.short_id = task.short_id.clone().or(record.short_id);
    record.request_id = task.request_id.clone();
    record.user_id = task.user_id.clone();
    record.api_key_id = task.api_key_id.clone();
    record.username = task.username.clone();
    record.api_key_name = task.api_key_name.clone();
    record.external_task_id = task.external_task_id.clone().or(record.external_task_id);
    record.provider_id = task.provider_id.clone();
    record.endpoint_id = task.endpoint_id.clone();
    record.key_id = task.key_id.clone();
    record.client_api_format = task.client_api_format.clone();
    record.provider_api_format = task.provider_api_format.clone();
    record.format_converted = task.format_converted;
    record.model = task.model.clone().or(record.model);
    record.prompt = task.prompt.clone().or(record.prompt);
    record.original_request_body = task
        .original_request_body
        .clone()
        .or(record.original_request_body);
    record.duration_seconds = task.duration_seconds.or(record.duration_seconds);
    record.resolution = task.resolution.clone().or(record.resolution);
    record.aspect_ratio = task.aspect_ratio.clone().or(record.aspect_ratio);
    record.size = task.size.clone().or(record.size);
    record.created_at_unix_ms = task.created_at_unix_ms;
    record.submitted_at_unix_secs = task.submitted_at_unix_secs;
    record.updated_at_unix_secs = now_unix_secs;
    record.retry_count = task.retry_count;
    record.poll_interval_seconds = task.poll_interval_seconds.max(1);
    record.poll_count = task.poll_count.saturating_add(1);
    record.max_poll_count = task.max_poll_count.max(1);
    record.next_poll_at_unix_secs = if record.status.is_active() {
        Some(now_unix_secs.saturating_add(u64::from(record.poll_interval_seconds)))
    } else {
        None
    };
    if !record.status.is_active() && record.completed_at_unix_secs.is_none() {
        record.completed_at_unix_secs = Some(now_unix_secs);
    }
    if record.status.is_active() && record.poll_count >= record.max_poll_count {
        record.status = VideoTaskStatus::Failed;
        record.error_code = Some("poll_timeout".to_string());
        record.error_message = Some(format!("Task timed out after {} polls", record.poll_count));
        record.completed_at_unix_secs = Some(now_unix_secs);
        record.next_poll_at_unix_secs = None;
    }
    record.request_metadata = merge_video_task_request_metadata(
        task.request_metadata.clone(),
        &snapshot,
        Some(provider_body),
        None,
    )
    .map_err(|err| GatewayError::Internal(err.to_string()))?;

    Ok(Some(record))
}

fn build_failed_poll_update(
    task: &StoredVideoTask,
    err: &VideoTaskRefreshError,
    now_unix_secs: u64,
) -> UpsertVideoTask {
    let mut record = stored_task_to_upsert(task);
    record.updated_at_unix_secs = now_unix_secs;
    record.poll_count = task.poll_count.saturating_add(1);
    record.progress_message = Some(format!("Poll error: {}", err.message));
    if err.permanent {
        record.status = VideoTaskStatus::Failed;
        record.error_code = Some("poll_permanent_error".to_string());
        record.error_message = Some(err.message.clone());
        record.completed_at_unix_secs = Some(now_unix_secs);
        record.next_poll_at_unix_secs = None;
    } else {
        let backoff =
            compute_poll_backoff_seconds(task.poll_interval_seconds.max(1), task.retry_count);
        record.retry_count = task.retry_count.saturating_add(1);
        record.next_poll_at_unix_secs = Some(now_unix_secs.saturating_add(backoff));
    }
    if record.status.is_active() && record.poll_count >= record.max_poll_count {
        record.status = VideoTaskStatus::Failed;
        record.error_code = Some("poll_timeout".to_string());
        record.error_message = Some(format!("Task timed out after {} polls", record.poll_count));
        record.completed_at_unix_secs = Some(now_unix_secs);
        record.next_poll_at_unix_secs = None;
    }
    record.request_metadata = LocalVideoTaskSnapshot::from_stored_task(task)
        .and_then(|snapshot| {
            merge_video_task_request_metadata(
                task.request_metadata.clone(),
                &snapshot,
                None,
                Some(err),
            )
            .ok()
            .flatten()
        })
        .or(task.request_metadata.clone());
    record
}

fn stored_task_to_upsert(task: &StoredVideoTask) -> UpsertVideoTask {
    let snapshot_record =
        LocalVideoTaskSnapshot::from_stored_task(task).map(|snapshot| snapshot.to_upsert_record());
    UpsertVideoTask {
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
        prompt: task.prompt.clone().or_else(|| {
            snapshot_record
                .as_ref()
                .and_then(|record| record.prompt.clone())
        }),
        original_request_body: task.original_request_body.clone().or_else(|| {
            snapshot_record
                .as_ref()
                .and_then(|record| record.original_request_body.clone())
        }),
        duration_seconds: task.duration_seconds.or_else(|| {
            snapshot_record
                .as_ref()
                .and_then(|record| record.duration_seconds)
        }),
        resolution: task.resolution.clone().or_else(|| {
            snapshot_record
                .as_ref()
                .and_then(|record| record.resolution.clone())
        }),
        aspect_ratio: task.aspect_ratio.clone().or_else(|| {
            snapshot_record
                .as_ref()
                .and_then(|record| record.aspect_ratio.clone())
        }),
        size: task.size.clone().or_else(|| {
            snapshot_record
                .as_ref()
                .and_then(|record| record.size.clone())
        }),
        status: task.status,
        progress_percent: task.progress_percent,
        progress_message: task.progress_message.clone(),
        retry_count: task.retry_count,
        poll_interval_seconds: task.poll_interval_seconds.max(1),
        next_poll_at_unix_secs: task.next_poll_at_unix_secs,
        poll_count: task.poll_count,
        max_poll_count: task.max_poll_count.max(1),
        created_at_unix_ms: task.created_at_unix_ms,
        submitted_at_unix_secs: task.submitted_at_unix_secs,
        completed_at_unix_secs: task.completed_at_unix_secs,
        updated_at_unix_secs: task.updated_at_unix_secs,
        error_code: task.error_code.clone(),
        error_message: task.error_message.clone(),
        video_url: task.video_url.clone(),
        request_metadata: task.request_metadata.clone(),
    }
}

fn compute_poll_backoff_seconds(poll_interval_seconds: u32, retry_count: u32) -> u64 {
    let exponent = retry_count.min(5);
    let multiplier = 1u64 << exponent;
    u64::from(poll_interval_seconds)
        .saturating_mul(multiplier)
        .min(MAX_VIDEO_TASK_POLL_BACKOFF_SECONDS)
}

fn merge_video_task_request_metadata(
    existing: Option<Value>,
    snapshot: &LocalVideoTaskSnapshot,
    provider_body: Option<&Map<String, Value>>,
    poll_error: Option<&VideoTaskRefreshError>,
) -> Result<Option<Value>, serde_json::Error> {
    let mut metadata = match existing {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    metadata.insert(
        "rust_owner".to_string(),
        Value::String("async_task".to_string()),
    );
    metadata.insert(
        "rust_local_snapshot".to_string(),
        serde_json::to_value(snapshot)?,
    );
    if let Some(provider_body) = provider_body {
        metadata.insert(
            "poll_raw_response".to_string(),
            Value::Object(provider_body.clone()),
        );
        metadata.remove("poll_error");
    }
    if let Some(poll_error) = poll_error {
        metadata.insert(
            "poll_error".to_string(),
            serde_json::json!({
                "message": poll_error.message,
                "permanent": poll_error.permanent,
                "observed_at_unix_secs": now_unix_secs(),
            }),
        );
    }
    Ok(Some(Value::Object(metadata)))
}

pub(crate) async fn finalize_video_task_if_terminal(state: &AppState, task: &StoredVideoTask) {
    let Some(event) = build_video_task_terminal_usage_event(task) else {
        return;
    };
    let mut event = event;
    if let Err(err) = enrich_usage_event_with_billing(state.data.as_ref(), &mut event).await {
        warn!(
            event_name = "video_task_finalize_billing_enrichment_failed",
            log_type = "event",
            request_id = %short_request_id(task.request_id.as_str()),
            error = %err,
            "gateway video task finalize failed to enrich billing"
        );
    }
    match build_upsert_usage_record_from_event(&event) {
        Ok(record) => match state.data.upsert_usage(record).await {
            Ok(Some(stored)) => {
                if let Err(err) = settle_usage_if_needed(state.data.as_ref(), &stored).await {
                    warn!(
                        event_name = "video_task_finalize_settlement_failed",
                        log_type = "event",
                        request_id = %short_request_id(task.request_id.as_str()),
                        error = %err,
                        "gateway video task finalize failed to settle usage"
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                warn!(
                    event_name = "video_task_finalize_usage_upsert_failed",
                    log_type = "event",
                    request_id = %short_request_id(task.request_id.as_str()),
                    error = %err,
                    "gateway video task finalize failed to upsert usage"
                );
            }
        },
        Err(err) => {
            warn!(
                event_name = "video_task_finalize_usage_build_failed",
                log_type = "event",
                request_id = %short_request_id(task.request_id.as_str()),
                error = %err,
                "gateway video task finalize failed to build usage record"
            );
        }
    }
}

fn build_video_task_terminal_usage_event(task: &StoredVideoTask) -> Option<UsageEvent> {
    let event_type = match task.status {
        VideoTaskStatus::Completed => UsageEventType::Completed,
        VideoTaskStatus::Failed | VideoTaskStatus::Expired => UsageEventType::Failed,
        VideoTaskStatus::Cancelled | VideoTaskStatus::Deleted => UsageEventType::Cancelled,
        VideoTaskStatus::Pending
        | VideoTaskStatus::Submitted
        | VideoTaskStatus::Queued
        | VideoTaskStatus::Processing => {
            return None;
        }
    };
    let provider_name = LocalVideoTaskSnapshot::from_stored_task(task)
        .and_then(|snapshot| snapshot.provider_name().map(str::to_string))
        .or_else(|| task.provider_id.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let response_time_ms = task
        .submitted_at_unix_secs
        .zip(
            task.completed_at_unix_secs
                .or(Some(task.updated_at_unix_secs)),
        )
        .map(|(submitted, completed)| completed.saturating_sub(submitted).saturating_mul(1_000));
    let status_code = match event_type {
        UsageEventType::Completed => Some(200),
        UsageEventType::Cancelled => Some(499),
        UsageEventType::Failed => Some(500),
        UsageEventType::Pending | UsageEventType::Streaming => None,
    };

    Some(UsageEvent::new(
        event_type,
        task.request_id.clone(),
        UsageEventData {
            user_id: task.user_id.clone(),
            api_key_id: task.api_key_id.clone(),
            username: task.username.clone(),
            api_key_name: task.api_key_name.clone(),
            provider_name,
            model: task.model.clone().unwrap_or_else(|| "unknown".to_string()),
            provider_id: task.provider_id.clone(),
            provider_endpoint_id: task.endpoint_id.clone(),
            provider_api_key_id: task.key_id.clone(),
            request_type: Some("video".to_string()),
            api_format: task.client_api_format.clone(),
            endpoint_api_format: task.provider_api_format.clone(),
            has_format_conversion: Some(task.format_converted),
            is_stream: Some(false),
            status_code,
            error_message: task.error_message.clone().or(task.error_code.clone()),
            response_time_ms,
            request_body: task.original_request_body.clone(),
            request_metadata: task.request_metadata.clone(),
            ..UsageEventData::default()
        },
    ))
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{build_failed_poll_update, stored_task_to_upsert, VideoTaskRefreshError};
    use crate::video_tasks::{
        LocalVideoTaskPersistence, LocalVideoTaskSnapshot, LocalVideoTaskStatus,
        LocalVideoTaskTransport, OpenAiVideoTaskSeed,
    };
    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn sample_sparse_stored_task() -> StoredVideoTask {
        let snapshot = LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
            local_task_id: "task-1".to_string(),
            upstream_task_id: "ext-1".to_string(),
            created_at_unix_ms: 1,
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            model: Some("sora-2".to_string()),
            prompt: Some("hello".to_string()),
            size: Some("1280x720".to_string()),
            seconds: Some("4".to_string()),
            remixed_from_video_id: None,
            status: LocalVideoTaskStatus::Processing,
            progress_percent: 50,
            completed_at_unix_secs: None,
            expires_at_unix_secs: None,
            error_code: None,
            error_message: None,
            video_url: None,
            persistence: LocalVideoTaskPersistence {
                request_id: "request-1".to_string(),
                username: Some("user".to_string()),
                api_key_name: Some("primary".to_string()),
                client_api_format: "openai:video".to_string(),
                provider_api_format: "openai:video".to_string(),
                original_request_body: json!({
                    "prompt": "hello",
                    "seconds": "4",
                    "resolution": "720p",
                    "aspect_ratio": "16:9",
                    "size": "1280x720"
                }),
                format_converted: false,
            },
            transport: LocalVideoTaskTransport {
                upstream_base_url: "https://example.com".to_string(),
                provider_name: Some("provider".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                headers: BTreeMap::new(),
                content_type: Some("application/json".to_string()),
                model_name: Some("sora-2".to_string()),
                proxy: None,
                tls_profile: None,
                timeouts: None,
            },
        });

        StoredVideoTask {
            id: "task-1".to_string(),
            short_id: Some("short-task-1".to_string()),
            request_id: "request-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: Some("user".to_string()),
            api_key_name: Some("primary".to_string()),
            external_task_id: Some("ext-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: None,
            original_request_body: None,
            duration_seconds: None,
            resolution: None,
            aspect_ratio: None,
            size: None,
            status: VideoTaskStatus::Processing,
            progress_percent: 50,
            progress_message: Some("polling".to_string()),
            retry_count: 1,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: Some(20),
            poll_count: 2,
            max_poll_count: 360,
            created_at_unix_ms: 1,
            submitted_at_unix_secs: Some(1),
            completed_at_unix_secs: None,
            updated_at_unix_secs: 20,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: Some(json!({
                "rust_local_snapshot": serde_json::to_value(snapshot)
                    .expect("snapshot should serialize")
            })),
        }
    }

    #[test]
    fn stored_task_to_upsert_restores_sparse_fields_from_snapshot() {
        let record = stored_task_to_upsert(&sample_sparse_stored_task());

        assert_eq!(record.prompt.as_deref(), Some("hello"));
        assert_eq!(
            record.original_request_body,
            Some(json!({
                "prompt": "hello",
                "seconds": "4",
                "resolution": "720p",
                "aspect_ratio": "16:9",
                "size": "1280x720"
            }))
        );
        assert_eq!(record.duration_seconds, Some(4));
        assert_eq!(record.resolution.as_deref(), Some("720p"));
        assert_eq!(record.aspect_ratio.as_deref(), Some("16:9"));
        assert_eq!(record.size.as_deref(), Some("1280x720"));
    }

    #[test]
    fn failed_poll_update_keeps_snapshot_backed_request_body() {
        let record = build_failed_poll_update(
            &sample_sparse_stored_task(),
            &VideoTaskRefreshError {
                message: "temporary failure".to_string(),
                permanent: false,
            },
            100,
        );

        assert_eq!(
            record.original_request_body,
            Some(json!({
                "prompt": "hello",
                "seconds": "4",
                "resolution": "720p",
                "aspect_ratio": "16:9",
                "size": "1280x720"
            }))
        );
        assert_eq!(record.prompt.as_deref(), Some("hello"));
        assert_eq!(record.resolution.as_deref(), Some("720p"));
    }
}
