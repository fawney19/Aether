use async_trait::async_trait;
use serde_json::Value;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum VideoTaskStatus {
    Pending,
    Submitted,
    Queued,
    Processing,
    Completed,
    Failed,
    Cancelled,
    Expired,
    Deleted,
}

impl VideoTaskStatus {
    pub fn from_database(value: &str) -> Result<Self, crate::DataLayerError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "submitted" => Ok(Self::Submitted),
            "queued" => Ok(Self::Queued),
            "processing" => Ok(Self::Processing),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            "deleted" => Ok(Self::Deleted),
            other => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unsupported video_tasks.status: {other}"
            ))),
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Submitted | Self::Queued | Self::Processing
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredVideoTask {
    pub id: String,
    pub short_id: Option<String>,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub external_task_id: Option<String>,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub key_id: Option<String>,
    pub client_api_format: Option<String>,
    pub provider_api_format: Option<String>,
    pub format_converted: bool,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub original_request_body: Option<Value>,
    pub duration_seconds: Option<u32>,
    pub resolution: Option<String>,
    pub aspect_ratio: Option<String>,
    pub size: Option<String>,
    pub status: VideoTaskStatus,
    pub progress_percent: u16,
    pub progress_message: Option<String>,
    pub retry_count: u32,
    pub poll_interval_seconds: u32,
    pub next_poll_at_unix_secs: Option<u64>,
    pub poll_count: u32,
    pub max_poll_count: u32,
    pub created_at_unix_ms: u64,
    pub submitted_at_unix_secs: Option<u64>,
    pub completed_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub video_url: Option<String>,
    pub request_metadata: Option<Value>,
}

impl StoredVideoTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        short_id: Option<String>,
        request_id: String,
        user_id: Option<String>,
        api_key_id: Option<String>,
        username: Option<String>,
        api_key_name: Option<String>,
        external_task_id: Option<String>,
        provider_id: Option<String>,
        endpoint_id: Option<String>,
        key_id: Option<String>,
        client_api_format: Option<String>,
        provider_api_format: Option<String>,
        format_converted: bool,
        model: Option<String>,
        prompt: Option<String>,
        original_request_body: Option<Value>,
        duration_seconds: Option<i32>,
        resolution: Option<String>,
        aspect_ratio: Option<String>,
        size: Option<String>,
        status: VideoTaskStatus,
        progress_percent: i32,
        progress_message: Option<String>,
        retry_count: i32,
        poll_interval_seconds: i32,
        next_poll_at_unix_secs: Option<i64>,
        poll_count: i32,
        max_poll_count: i32,
        created_at_unix_ms: i64,
        submitted_at_unix_secs: Option<i64>,
        completed_at_unix_secs: Option<i64>,
        updated_at_unix_secs: i64,
        error_code: Option<String>,
        error_message: Option<String>,
        video_url: Option<String>,
        request_metadata: Option<Value>,
    ) -> Result<Self, crate::DataLayerError> {
        let progress_percent = u16::try_from(progress_percent).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid progress_percent: {progress_percent}"
            ))
        })?;
        let retry_count = u32::try_from(retry_count).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!("invalid retry_count: {retry_count}"))
        })?;
        let poll_interval_seconds = u32::try_from(poll_interval_seconds).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid poll_interval_seconds: {poll_interval_seconds}"
            ))
        })?;
        let next_poll_at_unix_secs =
            coerce_optional_unix_secs(next_poll_at_unix_secs, "next_poll_at_unix_secs")?;
        let poll_count = u32::try_from(poll_count).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!("invalid poll_count: {poll_count}"))
        })?;
        let max_poll_count = u32::try_from(max_poll_count).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid max_poll_count: {max_poll_count}"
            ))
        })?;
        let created_at_unix_ms = u64::try_from(created_at_unix_ms).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid created_at_unix_ms: {created_at_unix_ms}"
            ))
        })?;
        let submitted_at_unix_secs =
            coerce_optional_unix_secs(submitted_at_unix_secs, "submitted_at_unix_secs")?;
        let completed_at_unix_secs =
            coerce_optional_unix_secs(completed_at_unix_secs, "completed_at_unix_secs")?;
        let updated_at_unix_secs = u64::try_from(updated_at_unix_secs).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!(
                "invalid updated_at_unix_secs: {updated_at_unix_secs}"
            ))
        })?;
        let duration_seconds = match duration_seconds {
            Some(value) => Some(u32::try_from(value).map_err(|_| {
                crate::DataLayerError::UnexpectedValue(format!("invalid duration_seconds: {value}"))
            })?),
            None => None,
        };

        Ok(Self {
            id,
            short_id,
            request_id,
            user_id,
            api_key_id,
            username,
            api_key_name,
            external_task_id,
            provider_id,
            endpoint_id,
            key_id,
            client_api_format,
            provider_api_format,
            format_converted,
            model,
            prompt,
            original_request_body,
            duration_seconds,
            resolution,
            aspect_ratio,
            size,
            status,
            progress_percent,
            progress_message,
            retry_count,
            poll_interval_seconds,
            next_poll_at_unix_secs,
            poll_count,
            max_poll_count,
            created_at_unix_ms,
            submitted_at_unix_secs,
            completed_at_unix_secs,
            updated_at_unix_secs,
            error_code,
            error_message,
            video_url,
            request_metadata,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertVideoTask {
    pub id: String,
    pub short_id: Option<String>,
    pub request_id: String,
    pub user_id: Option<String>,
    pub api_key_id: Option<String>,
    pub username: Option<String>,
    pub api_key_name: Option<String>,
    pub external_task_id: Option<String>,
    pub provider_id: Option<String>,
    pub endpoint_id: Option<String>,
    pub key_id: Option<String>,
    pub client_api_format: Option<String>,
    pub provider_api_format: Option<String>,
    pub format_converted: bool,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub original_request_body: Option<Value>,
    pub duration_seconds: Option<u32>,
    pub resolution: Option<String>,
    pub aspect_ratio: Option<String>,
    pub size: Option<String>,
    pub status: VideoTaskStatus,
    pub progress_percent: u16,
    pub progress_message: Option<String>,
    pub retry_count: u32,
    pub poll_interval_seconds: u32,
    pub next_poll_at_unix_secs: Option<u64>,
    pub poll_count: u32,
    pub max_poll_count: u32,
    pub created_at_unix_ms: u64,
    pub submitted_at_unix_secs: Option<u64>,
    pub completed_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub video_url: Option<String>,
    pub request_metadata: Option<Value>,
}

impl UpsertVideoTask {
    pub fn into_stored(self) -> StoredVideoTask {
        StoredVideoTask {
            id: self.id,
            short_id: self.short_id,
            request_id: self.request_id,
            user_id: self.user_id,
            api_key_id: self.api_key_id,
            username: self.username,
            api_key_name: self.api_key_name,
            external_task_id: self.external_task_id,
            provider_id: self.provider_id,
            endpoint_id: self.endpoint_id,
            key_id: self.key_id,
            client_api_format: self.client_api_format,
            provider_api_format: self.provider_api_format,
            format_converted: self.format_converted,
            model: self.model,
            prompt: self.prompt,
            original_request_body: self.original_request_body,
            duration_seconds: self.duration_seconds,
            resolution: self.resolution,
            aspect_ratio: self.aspect_ratio,
            size: self.size,
            status: self.status,
            progress_percent: self.progress_percent,
            progress_message: self.progress_message,
            retry_count: self.retry_count,
            poll_interval_seconds: self.poll_interval_seconds,
            next_poll_at_unix_secs: self.next_poll_at_unix_secs,
            poll_count: self.poll_count,
            max_poll_count: self.max_poll_count,
            created_at_unix_ms: self.created_at_unix_ms,
            submitted_at_unix_secs: self.submitted_at_unix_secs,
            completed_at_unix_secs: self.completed_at_unix_secs,
            updated_at_unix_secs: self.updated_at_unix_secs,
            error_code: self.error_code,
            error_message: self.error_message,
            video_url: self.video_url,
            request_metadata: self.request_metadata,
        }
    }
}

impl From<StoredVideoTask> for UpsertVideoTask {
    fn from(task: StoredVideoTask) -> Self {
        Self {
            id: task.id,
            short_id: task.short_id,
            request_id: task.request_id,
            user_id: task.user_id,
            api_key_id: task.api_key_id,
            username: task.username,
            api_key_name: task.api_key_name,
            external_task_id: task.external_task_id,
            provider_id: task.provider_id,
            endpoint_id: task.endpoint_id,
            key_id: task.key_id,
            client_api_format: task.client_api_format,
            provider_api_format: task.provider_api_format,
            format_converted: task.format_converted,
            model: task.model,
            prompt: task.prompt,
            original_request_body: task.original_request_body,
            duration_seconds: task.duration_seconds,
            resolution: task.resolution,
            aspect_ratio: task.aspect_ratio,
            size: task.size,
            status: task.status,
            progress_percent: task.progress_percent,
            progress_message: task.progress_message,
            retry_count: task.retry_count,
            poll_interval_seconds: task.poll_interval_seconds,
            next_poll_at_unix_secs: task.next_poll_at_unix_secs,
            poll_count: task.poll_count,
            max_poll_count: task.max_poll_count,
            created_at_unix_ms: task.created_at_unix_ms,
            submitted_at_unix_secs: task.submitted_at_unix_secs,
            completed_at_unix_secs: task.completed_at_unix_secs,
            updated_at_unix_secs: task.updated_at_unix_secs,
            error_code: task.error_code,
            error_message: task.error_message,
            video_url: task.video_url,
            request_metadata: task.request_metadata,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTaskLookupKey<'a> {
    Id(&'a str),
    ShortId(&'a str),
    UserExternal {
        user_id: &'a str,
        external_task_id: &'a str,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoTaskQueryFilter {
    pub user_id: Option<String>,
    pub status: Option<VideoTaskStatus>,
    pub model_substring: Option<String>,
    pub client_api_format: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VideoTaskStatusCount {
    pub status: VideoTaskStatus,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VideoTaskModelCount {
    pub model: String,
    pub count: u64,
}

#[async_trait]
pub trait VideoTaskReadRepository: Send + Sync {
    async fn find(
        &self,
        key: VideoTaskLookupKey<'_>,
    ) -> Result<Option<StoredVideoTask>, crate::DataLayerError>;

    async fn list_active(
        &self,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, crate::DataLayerError>;

    async fn list_due(
        &self,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, crate::DataLayerError>;

    async fn list_page(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, crate::DataLayerError>;

    async fn list_page_summary(
        &self,
        filter: &VideoTaskQueryFilter,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, crate::DataLayerError>;

    async fn count(&self, filter: &VideoTaskQueryFilter) -> Result<u64, crate::DataLayerError>;

    async fn count_by_status(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<Vec<VideoTaskStatusCount>, crate::DataLayerError>;

    async fn count_distinct_users(
        &self,
        filter: &VideoTaskQueryFilter,
    ) -> Result<u64, crate::DataLayerError>;

    async fn top_models(
        &self,
        filter: &VideoTaskQueryFilter,
        limit: usize,
    ) -> Result<Vec<VideoTaskModelCount>, crate::DataLayerError>;

    async fn count_created_since(
        &self,
        filter: &VideoTaskQueryFilter,
        created_since_unix_secs: u64,
    ) -> Result<u64, crate::DataLayerError>;
}

#[async_trait]
pub trait VideoTaskWriteRepository: Send + Sync {
    async fn upsert(&self, task: UpsertVideoTask)
        -> Result<StoredVideoTask, crate::DataLayerError>;

    async fn update_if_active(
        &self,
        task: UpsertVideoTask,
    ) -> Result<Option<StoredVideoTask>, crate::DataLayerError>;

    async fn claim_due(
        &self,
        now_unix_secs: u64,
        claim_until_unix_secs: u64,
        limit: usize,
    ) -> Result<Vec<StoredVideoTask>, crate::DataLayerError>;
}

pub trait VideoTaskRepository:
    VideoTaskReadRepository + VideoTaskWriteRepository + Send + Sync
{
}

impl<T> VideoTaskRepository for T where
    T: VideoTaskReadRepository + VideoTaskWriteRepository + Send + Sync
{
}

fn coerce_optional_unix_secs(
    value: Option<i64>,
    field: &str,
) -> Result<Option<u64>, crate::DataLayerError> {
    match value {
        Some(value) => Ok(Some(u64::try_from(value).map_err(|_| {
            crate::DataLayerError::UnexpectedValue(format!("invalid {field}: {value}"))
        })?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{StoredVideoTask, VideoTaskStatus};

    #[allow(clippy::type_complexity)]
    fn base_new_args() -> (
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
        Option<String>,
        Option<String>,
        Option<serde_json::Value>,
        Option<i32>,
        Option<String>,
        Option<String>,
        Option<String>,
        VideoTaskStatus,
        i32,
        Option<String>,
        i32,
        i32,
        Option<i64>,
        i32,
        i32,
        i64,
        Option<i64>,
        Option<i64>,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<serde_json::Value>,
    ) {
        (
            "task-1".to_string(),
            None,
            "request-1".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            VideoTaskStatus::Submitted,
            10,
            None,
            0,
            10,
            Some(1),
            0,
            360,
            1,
            None,
            None,
            1,
            None,
            None,
            None,
            None,
        )
    }

    #[test]
    fn parses_status_from_database_text() {
        assert_eq!(
            VideoTaskStatus::from_database("processing").expect("status should parse"),
            VideoTaskStatus::Processing
        );
    }

    #[test]
    fn rejects_invalid_database_status() {
        assert!(VideoTaskStatus::from_database("mystery").is_err());
    }

    #[test]
    fn rejects_invalid_numeric_fields() {
        let mut args = base_new_args();
        args.22 = -1;
        assert!(StoredVideoTask::new(
            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8, args.9,
            args.10, args.11, args.12, args.13, args.14, args.15, args.16, args.17, args.18,
            args.19, args.20, args.21, args.22, args.23, args.24, args.25, args.26, args.27,
            args.28, args.29, args.30, args.31, args.32, args.33, args.34, args.35, args.36,
        )
        .is_err());
    }

    #[test]
    fn rejects_negative_updated_at_values() {
        let mut args = base_new_args();
        args.32 = -1;
        assert!(StoredVideoTask::new(
            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8, args.9,
            args.10, args.11, args.12, args.13, args.14, args.15, args.16, args.17, args.18,
            args.19, args.20, args.21, args.22, args.23, args.24, args.25, args.26, args.27,
            args.28, args.29, args.30, args.31, args.32, args.33, args.34, args.35, args.36,
        )
        .is_err());
    }

    #[test]
    fn rejects_negative_created_at_values() {
        let mut args = base_new_args();
        args.29 = -1;
        assert!(StoredVideoTask::new(
            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8, args.9,
            args.10, args.11, args.12, args.13, args.14, args.15, args.16, args.17, args.18,
            args.19, args.20, args.21, args.22, args.23, args.24, args.25, args.26, args.27,
            args.28, args.29, args.30, args.31, args.32, args.33, args.34, args.35, args.36,
        )
        .is_err());
    }

    #[test]
    fn rejects_negative_optional_completed_at_values() {
        let mut args = base_new_args();
        args.31 = Some(-1);
        assert!(StoredVideoTask::new(
            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8, args.9,
            args.10, args.11, args.12, args.13, args.14, args.15, args.16, args.17, args.18,
            args.19, args.20, args.21, args.22, args.23, args.24, args.25, args.26, args.27,
            args.28, args.29, args.30, args.31, args.32, args.33, args.34, args.35, args.36,
        )
        .is_err());
    }
}
