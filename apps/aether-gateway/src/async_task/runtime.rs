use std::time::Duration;

use aether_billing::enrich_usage_event_with_billing;
use aether_contracts::{ExecutionErrorKind, ExecutionResult};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskStatus,
};
use aether_usage_runtime::{build_upsert_usage_record_from_event, settle_usage_if_needed};
use serde_json::{Map, Value};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::log_ids::short_request_id;
use crate::usage::{UsageEvent, UsageEventData, UsageEventType};
use crate::video_tasks::{LocalVideoTaskReadRefreshPlan, LocalVideoTaskSnapshot};
use crate::{AppState, GatewayError};

const MAX_VIDEO_TASK_POLL_BACKOFF_SECONDS: u64 = 300;
const VIDEO_TASK_POLL_CLAIM_SECONDS: u64 = 30;

#[derive(Debug, Clone)]
struct VideoTaskRefreshError {
    category: &'static str,
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
                error_category = err.category,
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
        let Some(snapshot) = state.reconstruct_video_task_snapshot(&task).await? else {
            continue;
        };
        let Some(refresh_plan) = state
            .video_tasks
            .prepare_poll_refresh_plan_for_snapshot(snapshot.clone(), &trace_id)
        else {
            continue;
        };

        match fetch_video_task_refresh_attempt(state, &refresh_plan).await? {
            VideoTaskRefreshAttempt::Success { provider_body } => {
                let Some(updated) = build_successful_poll_update(
                    &task,
                    snapshot.clone(),
                    &provider_body,
                    now_unix_secs,
                )?
                else {
                    continue;
                };
                match state.update_active_video_task(updated).await? {
                    Some(stored) => {
                        if let Some(snapshot) =
                            state.reconstruct_video_task_snapshot(&stored).await?
                        {
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
                        if let Some(snapshot) =
                            state.reconstruct_video_task_snapshot(&stored).await?
                        {
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

    Some(crate::task_runtime::spawn_singleton_worker(
        state,
        crate::task_runtime::TASK_KEY_VIDEO_TASK_POLLER,
        move |state| async move {
            let mut interval = tokio::time::interval(config.interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut deferred_since = None;
            loop {
                interval.tick().await;
                if state
                    .data
                    .should_defer_maintenance_for_database_pool_pressure(&mut deferred_since)
                {
                    debug!(
                        event_name = "video_task_poller_deferred",
                        log_type = "event",
                        "gateway video task poller deferred because database pool has no idle reserve"
                    );
                    continue;
                }
                if let Err(err) = poll_video_tasks_once(&state, config.batch_size).await {
                    warn!(
                        event_name = "video_task_poller_tick_failed",
                        log_type = "event",
                        error = ?err,
                        "gateway video task poller tick failed"
                    );
                }
            }
        },
    ))
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
        Err(_) => {
            return Ok(VideoTaskRefreshAttempt::Error(VideoTaskRefreshError {
                category: "transport_error",
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
            category: "invalid_provider_response",
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
    let category = if status_code == 401 {
        "authentication_error"
    } else if status_code == 403 {
        "permission_denied"
    } else if status_code == 404 {
        "not_found"
    } else if status_code == 429 {
        "rate_limit"
    } else if status_code >= 500 {
        "server_error"
    } else {
        "provider_error"
    };
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

    VideoTaskRefreshError {
        category,
        permanent,
    }
}

fn build_successful_poll_update(
    task: &StoredVideoTask,
    mut snapshot: LocalVideoTaskSnapshot,
    provider_body: &Map<String, Value>,
    now_unix_secs: u64,
) -> Result<Option<UpsertVideoTask>, GatewayError> {
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
    record.original_request_body = None;
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
        record.error_message = None;
        record.completed_at_unix_secs = Some(now_unix_secs);
        record.next_poll_at_unix_secs = None;
    }
    record.request_metadata = None;

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
    record.progress_message = None;
    if err.permanent {
        record.status = VideoTaskStatus::Failed;
        record.error_code = Some("poll_permanent_error".to_string());
        record.error_message = None;
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
        record.error_message = None;
        record.completed_at_unix_secs = Some(now_unix_secs);
        record.next_poll_at_unix_secs = None;
    }
    record.request_metadata = None;
    record
}

fn stored_task_to_upsert(task: &StoredVideoTask) -> UpsertVideoTask {
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
        prompt: task.prompt.clone(),
        original_request_body: None,
        duration_seconds: task.duration_seconds,
        resolution: task.resolution.clone(),
        aspect_ratio: task.aspect_ratio.clone(),
        size: task.size.clone(),
        status: task.status,
        progress_percent: task.progress_percent,
        progress_message: None,
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
        error_message: None,
        video_url: task.video_url.clone(),
        request_metadata: None,
    }
}

fn compute_poll_backoff_seconds(poll_interval_seconds: u32, retry_count: u32) -> u64 {
    let exponent = retry_count.min(5);
    let multiplier = 1u64 << exponent;
    u64::from(poll_interval_seconds)
        .saturating_mul(multiplier)
        .min(MAX_VIDEO_TASK_POLL_BACKOFF_SECONDS)
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
    let provider_name = task
        .provider_id
        .clone()
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
            error_message: task.error_code.clone(),
            response_time_ms,
            request_body: None,
            request_metadata: None,
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
                transport_profile: None,
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
    fn stored_task_to_upsert_does_not_restore_sensitive_legacy_snapshot_fields() {
        let record = stored_task_to_upsert(&sample_sparse_stored_task());

        assert!(record.prompt.is_none());
        assert!(record.original_request_body.is_none());
        assert!(record.duration_seconds.is_none());
        assert!(record.resolution.is_none());
        assert!(record.aspect_ratio.is_none());
        assert!(record.size.is_none());
        assert!(record.progress_message.is_none());
        assert!(record.error_message.is_none());
        assert!(record.request_metadata.is_none());
    }

    #[test]
    fn failed_poll_update_drops_snapshot_backed_sensitive_fields() {
        let record = build_failed_poll_update(
            &sample_sparse_stored_task(),
            &VideoTaskRefreshError {
                category: "transport_error",
                permanent: false,
            },
            100,
        );

        assert!(record.original_request_body.is_none());
        assert!(record.prompt.is_none());
        assert!(record.resolution.is_none());
        assert!(record.progress_message.is_none());
        assert!(record.error_message.is_none());
        assert!(record.request_metadata.is_none());
    }
}
