use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
use axum::http;
use chrono::{SecondsFormat, Utc};
use serde_json::json;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn admin_video_task_status_name(status: VideoTaskStatus) -> &'static str {
    match status {
        VideoTaskStatus::Pending => "pending",
        VideoTaskStatus::Submitted => "submitted",
        VideoTaskStatus::Queued => "queued",
        VideoTaskStatus::Processing => "processing",
        VideoTaskStatus::Completed => "completed",
        VideoTaskStatus::Failed => "failed",
        VideoTaskStatus::Cancelled => "cancelled",
        VideoTaskStatus::Expired => "expired",
        VideoTaskStatus::Deleted => "deleted",
    }
}

pub(super) fn admin_video_task_error_projection(task: &StoredVideoTask) -> Option<String> {
    if task.error_message.as_deref().is_none_or(str::is_empty)
        && task.error_code.as_deref().is_none_or(str::is_empty)
    {
        return None;
    }
    task.error_code
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| Some("provider_error".to_string()))
}

pub(super) fn admin_video_task_timestamp(unix_secs: Option<u64>) -> Option<String> {
    unix_secs.and_then(|value| {
        chrono::DateTime::<Utc>::from_timestamp(value as i64, 0)
            .map(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Secs, true))
    })
}

pub(super) fn truncate_admin_video_task_prompt(prompt: Option<&str>) -> Option<String> {
    prompt.map(|value| {
        if value.chars().count() <= 100 {
            value.to_string()
        } else {
            let mut truncated = value.chars().take(100).collect::<String>();
            truncated.push_str("...");
            truncated
        }
    })
}

pub(super) async fn build_admin_video_task_provider_names(
    state: &AdminAppState<'_>,
    tasks: &[StoredVideoTask],
) -> Result<BTreeMap<String, String>, GatewayError> {
    let provider_ids = tasks
        .iter()
        .filter_map(|task| task.provider_id.as_ref())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if provider_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    Ok(state
        .app()
        .read_provider_catalog_providers_by_ids(&provider_ids)
        .await?
        .into_iter()
        .map(|provider| (provider.id, provider.name))
        .collect())
}

pub(super) fn build_admin_video_task_list_item(
    task: &StoredVideoTask,
    provider_names: &BTreeMap<String, String>,
) -> Value {
    let provider_name = task
        .provider_id
        .as_deref()
        .and_then(|provider_id| provider_names.get(provider_id))
        .cloned()
        .unwrap_or_else(|| "Unknown".to_string());
    json!({
        "id": task.id,
        "external_task_id": task.external_task_id,
        "user_id": task.user_id,
        "username": task.username.clone().unwrap_or_else(|| "Unknown".to_string()),
        "model": task.model,
        "prompt": truncate_admin_video_task_prompt(task.prompt.as_deref()),
        "status": admin_video_task_status_name(task.status),
        "progress_percent": task.progress_percent,
        "progress_message": task.progress_message,
        "provider_id": task.provider_id,
        "provider_name": provider_name,
        "duration_seconds": task.duration_seconds,
        "resolution": task.resolution,
        "aspect_ratio": task.aspect_ratio,
        "video_url": task.video_url,
        "error_code": task.error_code,
        "error_message": admin_video_task_error_projection(task),
        "poll_count": task.poll_count,
        "max_poll_count": task.max_poll_count,
        "created_at": admin_video_task_timestamp(Some(task.created_at_unix_ms)),
        "completed_at": admin_video_task_timestamp(task.completed_at_unix_secs),
        "submitted_at": admin_video_task_timestamp(task.submitted_at_unix_secs),
    })
}

pub(super) fn admin_video_task_detail_id_from_path(request_path: &str) -> Option<&str> {
    let task_id = request_path.strip_prefix("/api/admin/video-tasks/")?;
    if task_id.is_empty() || task_id.contains('/') || task_id == "stats" {
        return None;
    }
    Some(task_id)
}

pub(super) fn admin_video_task_nested_id_from_path<'a>(
    request_path: &'a str,
    suffix: &str,
) -> Option<&'a str> {
    let task_id = request_path
        .strip_prefix("/api/admin/video-tasks/")?
        .strip_suffix(suffix)?;
    if task_id.is_empty() || task_id.contains('/') {
        return None;
    }
    Some(task_id)
}

pub(super) fn current_admin_video_task_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{admin_video_task_error_projection, build_admin_video_task_list_item};
    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
    use std::collections::BTreeMap;

    fn failed_task() -> StoredVideoTask {
        StoredVideoTask::new(
            "task-1".to_string(),
            None,
            "request-1".to_string(),
            Some("user-1".to_string()),
            None,
            Some("alice".to_string()),
            None,
            None,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("key-1".to_string()),
            Some("openai:video".to_string()),
            Some("openai:video".to_string()),
            false,
            Some("video-model".to_string()),
            None,
            None,
            None,
            None,
            None,
            None,
            VideoTaskStatus::Failed,
            100,
            None,
            0,
            10,
            None,
            1,
            10,
            1,
            None,
            Some(2),
            2,
            Some("authentication_error".to_string()),
            Some("Authorization: Bearer live-secret at https://api.example?key=secret".to_string()),
            None,
            None,
        )
        .expect("task")
    }

    #[test]
    fn video_task_payload_does_not_return_historical_raw_error_text() {
        let task = failed_task();
        assert_eq!(
            admin_video_task_error_projection(&task).as_deref(),
            Some("authentication_error")
        );
        let payload = build_admin_video_task_list_item(&task, &BTreeMap::new());
        assert_eq!(payload["error_message"], "authentication_error");
        assert!(!payload.to_string().contains("live-secret"));
    }
}
