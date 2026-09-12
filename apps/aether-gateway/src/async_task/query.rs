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

pub(crate) enum VideoTaskVideoSource {
    Redirect {
        url: url::Url,
    },
    Proxy {
        url: url::Url,
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

pub(crate) async fn read_video_task_detail_for_user(
    state: &AppState,
    task_id: &str,
    user_id: &str,
) -> Result<Option<StoredVideoTask>, GatewayError> {
    state.find_video_task_by_id_for_user(task_id, user_id).await
}

pub(crate) async fn read_video_task_video_source(
    state: &AppState,
    task_id: &str,
) -> Result<Option<VideoTaskVideoSource>, GatewayError> {
    let Some(task) = read_video_task_detail(state, task_id).await? else {
        return Ok(None);
    };
    video_task_video_source_from_task(state, &task).await
}

pub(crate) async fn video_task_video_source_from_task(
    state: &AppState,
    task: &StoredVideoTask,
) -> Result<Option<VideoTaskVideoSource>, GatewayError> {
    let Some(video_url) = task
        .video_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
    else {
        return Ok(None);
    };

    let video_url = parse_video_url(&video_url)?;

    if task.effective_api_format() != Some("gemini:video") {
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

    let endpoint_url = parse_video_url(transport.endpoint.base_url.trim()).map_err(|_| {
        GatewayError::Internal("provider endpoint URL is invalid for proxied video".to_string())
    })?;
    if !video_urls_share_origin(&endpoint_url, &video_url) {
        return Err(GatewayError::Client {
            status: axum::http::StatusCode::BAD_GATEWAY,
            message: "video URL origin does not match its provider endpoint".to_string(),
        });
    }
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
        filename: format!("video_{}.mp4", task.id),
    }))
}

fn parse_video_url(raw_url: &str) -> Result<url::Url, GatewayError> {
    let url = url::Url::parse(raw_url.trim()).map_err(|_| GatewayError::Client {
        status: axum::http::StatusCode::BAD_GATEWAY,
        message: "video URL is invalid".to_string(),
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(GatewayError::Client {
            status: axum::http::StatusCode::BAD_GATEWAY,
            message: "video URL must be an absolute HTTP(S) URL without credentials".to_string(),
        });
    }
    Ok(url)
}

fn video_urls_share_origin(left: &url::Url, right: &url::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host() == right.host()
        && left.port_or_known_default() == right.port_or_known_default()
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
    use aether_data_contracts::repository::provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    };
    use aether_data_contracts::repository::video_tasks::{UpsertVideoTask, VideoTaskStatus};
    use serde_json::json;

    use super::{
        parse_video_url, video_task_video_source_from_task, video_urls_share_origin,
        VideoTaskVideoSource,
    };
    use crate::{data::GatewayDataState, AppState};

    fn legacy_gemini_video_task() -> aether_data_contracts::repository::video_tasks::StoredVideoTask
    {
        UpsertVideoTask {
            id: "legacy-gemini-task".to_string(),
            short_id: Some("legacy-short".to_string()),
            request_id: "legacy-request".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("client-key-1".to_string()),
            username: None,
            api_key_name: None,
            external_task_id: Some("operations/upstream-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("provider-key-1".to_string()),
            client_api_format: Some("gemini:video".to_string()),
            provider_api_format: None,
            format_converted: false,
            model: Some("veo-3".to_string()),
            prompt: None,
            original_request_body: None,
            duration_seconds: Some(8),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: VideoTaskStatus::Completed,
            progress_percent: 100,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 1,
            max_poll_count: 360,
            created_at_unix_ms: 1,
            submitted_at_unix_secs: Some(1),
            completed_at_unix_secs: Some(2),
            updated_at_unix_secs: 2,
            error_code: None,
            error_message: None,
            video_url: Some(
                "https://generativelanguage.googleapis.com/v1beta/files/video-1:download?alt=media"
                    .to_string(),
            ),
            request_metadata: None,
        }
        .into_stored()
    }

    fn state_with_gemini_transport() -> AppState {
        let state = AppState::new().expect("gateway state should build");
        let provider = StoredProviderCatalogProvider::new(
            "provider-1".to_string(),
            "Gemini".to_string(),
            Some("https://ai.google.dev".to_string()),
            "gemini".to_string(),
        )
        .expect("provider should build");
        let endpoint = StoredProviderCatalogEndpoint::new(
            "endpoint-1".to_string(),
            "provider-1".to_string(),
            "gemini:video".to_string(),
            None,
            None,
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://generativelanguage.googleapis.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build");
        let encrypted_api_key = state
            .seal_provider_catalog_key_api_key(
                "provider-1",
                "provider-key-1",
                "gemini-provider-secret",
            )
            .expect("provider key should encrypt");
        let key = StoredProviderCatalogKey::new(
            "provider-key-1".to_string(),
            "provider-1".to_string(),
            "default".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("provider key should build")
        .with_transport_fields(
            Some(json!(["gemini:video"])),
            encrypted_api_key,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("provider key transport should build");
        let provider_catalog: Arc<dyn ProviderCatalogReadRepository> = Arc::new(
            InMemoryProviderCatalogReadRepository::seed(vec![provider], vec![endpoint], vec![key]),
        );
        let video_tasks = Arc::new(InMemoryVideoTaskRepository::default());
        let data = GatewayDataState::with_video_task_repository_and_provider_transport_for_tests(
            video_tasks,
            provider_catalog,
            DEVELOPMENT_ENCRYPTION_KEY,
        );
        state.with_data_state_for_tests(data)
    }

    #[test]
    fn video_url_parser_rejects_non_http_and_embedded_credentials() {
        for raw_url in [
            "file:///etc/passwd",
            "data:video/mp4;base64,AAAA",
            "https://user@example.com/video.mp4",
            "https://user:password@example.com/video.mp4",
            "/relative/video.mp4",
        ] {
            assert!(
                parse_video_url(raw_url).is_err(),
                "URL should be rejected: {raw_url}"
            );
        }
    }

    #[test]
    fn video_origin_comparison_uses_scheme_host_and_effective_port() {
        let base = parse_video_url("https://generativelanguage.googleapis.com/v1beta").unwrap();
        for same_origin in [
            "https://generativelanguage.googleapis.com/file",
            "https://generativelanguage.googleapis.com:443/file",
        ] {
            assert!(video_urls_share_origin(
                &base,
                &parse_video_url(same_origin).unwrap()
            ));
        }
        for different_origin in [
            "http://generativelanguage.googleapis.com/file",
            "https://generativelanguage.googleapis.com:444/file",
            "https://generativelanguage.googleapis.com.evil.test/file",
            "https://evil.test/generativelanguage.googleapis.com/file",
        ] {
            assert!(!video_urls_share_origin(
                &base,
                &parse_video_url(different_origin).unwrap()
            ));
        }
    }

    #[tokio::test]
    async fn legacy_gemini_client_format_uses_authenticated_proxy_source() {
        let source = video_task_video_source_from_task(
            &state_with_gemini_transport(),
            &legacy_gemini_video_task(),
        )
        .await
        .expect("video source should resolve")
        .expect("video source should exist");

        match source {
            VideoTaskVideoSource::Proxy {
                url,
                header_name,
                header_value,
                filename,
            } => {
                assert_eq!(
                    url.as_str(),
                    "https://generativelanguage.googleapis.com/v1beta/files/video-1:download?alt=media"
                );
                assert_eq!(header_name, "x-goog-api-key");
                assert_eq!(header_value, "gemini-provider-secret");
                assert_eq!(filename, "video_legacy-gemini-task.mp4");
            }
            VideoTaskVideoSource::Redirect { .. } => {
                panic!("legacy Gemini video must not bypass the authenticated proxy")
            }
        }
    }
}
