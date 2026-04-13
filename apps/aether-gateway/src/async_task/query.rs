use std::collections::BTreeMap;

use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, VideoTaskModelCount, VideoTaskQueryFilter, VideoTaskStatus,
    VideoTaskStatusCount,
};
use serde::Serialize;

use crate::{AppState, GatewayError};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VideoTaskPageResponse {
    pub(crate) items: Vec<StoredVideoTask>,
    pub(crate) total: u64,
    pub(crate) page: usize,
    pub(crate) page_size: usize,
    pub(crate) pages: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct VideoTaskStatsResponse {
    pub(crate) total: u64,
    pub(crate) by_status: BTreeMap<String, u64>,
    pub(crate) by_model: BTreeMap<String, u64>,
    pub(crate) today_count: u64,
    pub(crate) processing_count: u64,
}

#[derive(Debug, Clone)]
pub(crate) enum VideoTaskVideoSource {
    Redirect {
        url: String,
    },
    Proxy {
        url: String,
        header_name: String,
        header_value: String,
        filename: String,
    },
}

pub(crate) async fn read_video_task_page(
    state: &AppState,
    filter: &VideoTaskQueryFilter,
    page: usize,
    page_size: usize,
) -> Result<VideoTaskPageResponse, GatewayError> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 100);
    let total = state.count_video_tasks(filter).await?;
    let offset = page_size.saturating_mul(page.saturating_sub(1));
    let items = state
        .list_video_task_page(filter, offset, page_size)
        .await?;
    let pages = if total == 0 {
        0
    } else {
        ((total as usize) + page_size - 1) / page_size
    };

    Ok(VideoTaskPageResponse {
        items,
        total,
        page,
        page_size,
        pages,
    })
}

pub(crate) async fn read_video_task_page_summary(
    state: &AppState,
    filter: &VideoTaskQueryFilter,
    page: usize,
    page_size: usize,
) -> Result<VideoTaskPageResponse, GatewayError> {
    let page = page.max(1);
    let page_size = page_size.clamp(1, 100);
    let total = state.count_video_tasks(filter).await?;
    let offset = page_size.saturating_mul(page.saturating_sub(1));
    let items = state
        .list_video_task_page_summary(filter, offset, page_size)
        .await?;
    let pages = if total == 0 {
        0
    } else {
        ((total as usize) + page_size - 1) / page_size
    };

    Ok(VideoTaskPageResponse {
        items,
        total,
        page,
        page_size,
        pages,
    })
}

pub(crate) async fn read_video_task_detail(
    state: &AppState,
    task_id: &str,
) -> Result<Option<StoredVideoTask>, GatewayError> {
    state.find_video_task_by_id(task_id).await
}

pub(crate) async fn read_video_task_video_source(
    state: &AppState,
    task_id: &str,
) -> Result<Option<VideoTaskVideoSource>, GatewayError> {
    let Some(task) = read_video_task_detail(state, task_id).await? else {
        return Ok(None);
    };
    let Some(video_url) = task
        .video_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(None);
    };

    if !video_url.contains("generativelanguage.googleapis.com") {
        return Ok(Some(VideoTaskVideoSource::Redirect { url: video_url }));
    }

    let Some(provider_id) = task.provider_id.as_deref() else {
        return Err(GatewayError::Internal(
            "video task is missing provider_id for proxied video".to_string(),
        ));
    };
    let Some(endpoint_id) = task.endpoint_id.as_deref() else {
        return Err(GatewayError::Internal(
            "video task is missing endpoint_id for proxied video".to_string(),
        ));
    };
    let Some(key_id) = task.key_id.as_deref() else {
        return Err(GatewayError::Internal(
            "video task is missing key_id for proxied video".to_string(),
        ));
    };

    let Some(transport) = state
        .read_provider_transport_snapshot(provider_id, endpoint_id, key_id)
        .await?
    else {
        return Err(GatewayError::Internal(
            "provider transport snapshot is unavailable for proxied video".to_string(),
        ));
    };

    let api_key = transport.key.decrypted_api_key.trim();
    if api_key.is_empty() {
        return Err(GatewayError::Internal(
            "provider transport key is unavailable for proxied video".to_string(),
        ));
    }

    Ok(Some(VideoTaskVideoSource::Proxy {
        url: video_url,
        header_name: "x-goog-api-key".to_string(),
        header_value: api_key.to_string(),
        filename: format!("video_{task_id}.mp4"),
    }))
}

pub(crate) async fn read_video_task_stats(
    state: &AppState,
    filter: &VideoTaskQueryFilter,
    now_unix_secs: u64,
) -> Result<VideoTaskStatsResponse, GatewayError> {
    let total = state.count_video_tasks(filter).await?;
    let by_status = state.count_video_tasks_by_status(filter).await?;
    let by_model = state.top_video_task_models(filter, 10).await?;
    let today_count = state
        .count_video_tasks_created_since(filter, start_of_utc_day(now_unix_secs))
        .await?;
    let processing_count = by_status
        .iter()
        .filter(|entry| {
            matches!(
                entry.status,
                VideoTaskStatus::Submitted | VideoTaskStatus::Queued | VideoTaskStatus::Processing
            )
        })
        .map(|entry| entry.count)
        .sum();

    Ok(VideoTaskStatsResponse {
        total,
        by_status: map_status_counts(by_status),
        by_model: map_model_counts(by_model),
        today_count,
        processing_count,
    })
}

fn map_status_counts(counts: Vec<VideoTaskStatusCount>) -> BTreeMap<String, u64> {
    counts
        .into_iter()
        .map(|entry| (status_key(entry.status), entry.count))
        .collect()
}

fn map_model_counts(counts: Vec<VideoTaskModelCount>) -> BTreeMap<String, u64> {
    counts
        .into_iter()
        .map(|entry| (entry.model, entry.count))
        .collect()
}

fn status_key(status: VideoTaskStatus) -> String {
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
    .to_string()
}

fn start_of_utc_day(now_unix_secs: u64) -> u64 {
    now_unix_secs - (now_unix_secs % 86_400)
}
