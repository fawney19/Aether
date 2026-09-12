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
    pub fn effective_api_format(&self) -> Option<&str> {
        effective_video_task_api_format(
            self.client_api_format.as_deref(),
            self.provider_api_format.as_deref(),
        )
    }

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

        let mut task = Self {
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
        };
        task.sanitize_persisted_diagnostics();
        Ok(task)
    }

    fn sanitize_persisted_diagnostics(&mut self) {
        self.original_request_body = None;
        self.progress_message = None;
        self.error_code = sanitize_video_task_error_code(self.error_code.take());
        self.error_message = None;
        self.video_url = sanitize_video_task_url(self.video_url.take());
        self.request_metadata = None;
    }

    /// Verifies that an update still refers to the task identity persisted for `id`.
    ///
    /// These fields select the owner, upstream target, request shape, or immutable
    /// creation identity of a video task. Repository implementations must reject an
    /// upsert that changes any of them instead of treating possession of `id` as
    /// permission to replace the row.
    ///
    /// `created_at_unix_ms` is deliberately not compared because some snapshot
    /// projections recompute it. Repositories preserve the already stored creation
    /// time while accepting an otherwise matching lifecycle update.
    pub fn ensure_immutable_identity_matches(
        &self,
        incoming: &UpsertVideoTask,
    ) -> Result<(), crate::DataLayerError> {
        let mismatched_field = if self.id != incoming.id {
            Some("id")
        } else if self.short_id != incoming.short_id {
            Some("short_id")
        } else if self.request_id != incoming.request_id {
            Some("request_id")
        } else if self.user_id != incoming.user_id {
            Some("user_id")
        } else if self.api_key_id != incoming.api_key_id {
            Some("api_key_id")
        } else if self.external_task_id != incoming.external_task_id {
            Some("external_task_id")
        } else if self.provider_id != incoming.provider_id {
            Some("provider_id")
        } else if self.endpoint_id != incoming.endpoint_id {
            Some("endpoint_id")
        } else if self.key_id != incoming.key_id {
            Some("key_id")
        } else if self.client_api_format != incoming.client_api_format {
            Some("client_api_format")
        } else if self.provider_api_format != incoming.provider_api_format {
            Some("provider_api_format")
        } else if self.format_converted != incoming.format_converted {
            Some("format_converted")
        } else if self.model != incoming.model {
            Some("model")
        } else if self.duration_seconds != incoming.duration_seconds {
            Some("duration_seconds")
        } else if self.resolution != incoming.resolution {
            Some("resolution")
        } else if self.aspect_ratio != incoming.aspect_ratio {
            Some("aspect_ratio")
        } else if self.size != incoming.size {
            Some("size")
        } else {
            None
        };

        match mismatched_field {
            Some(field) => Err(crate::DataLayerError::InvalidInput(format!(
                "video task {} conflicts with persisted immutable field {field}",
                incoming.id
            ))),
            None => Ok(()),
        }
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
    pub fn sanitize_for_persistence(&mut self) {
        self.original_request_body = None;
        self.progress_message = None;
        self.error_code = sanitize_video_task_error_code(self.error_code.take());
        self.error_message = None;
        self.video_url = sanitize_video_task_url(self.video_url.take());
        self.request_metadata = None;
    }

    pub fn into_stored(mut self) -> StoredVideoTask {
        self.sanitize_for_persistence();
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

fn sanitize_video_task_error_code(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    Some(match value.as_str() {
        "authentication_error"
        | "cancelled"
        | "content_policy_violation"
        | "expired"
        | "invalid_request"
        | "not_found"
        | "permission_denied"
        | "poll_permanent_error"
        | "poll_timeout"
        | "provider_error"
        | "rate_limit_exceeded"
        | "server_error"
        | "unknown" => value,
        _ => "provider_error".to_string(),
    })
}

fn sanitize_video_task_url(value: Option<String>) -> Option<String> {
    let mut url = url::Url::parse(value?.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }

    url.set_fragment(None);
    Some(url.into())
}

fn effective_video_task_api_format<'a>(
    client_api_format: Option<&'a str>,
    provider_api_format: Option<&'a str>,
) -> Option<&'a str> {
    provider_api_format
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            client_api_format
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
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

    /// Resolve a public task identifier only when the persisted task belongs
    /// to `user_id`. The lookup key and owner predicate must be evaluated by
    /// one repository operation.
    async fn find_for_user(
        &self,
        key: VideoTaskLookupKey<'_>,
        user_id: &str,
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
    use super::{StoredVideoTask, UpsertVideoTask, VideoTaskStatus};

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

    #[test]
    fn immutable_identity_validation_rejects_every_protected_field() {
        let task = UpsertVideoTask {
            id: "task-1".to_string(),
            short_id: Some("short-1".to_string()),
            request_id: "request-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: None,
            api_key_name: None,
            external_task_id: Some("external-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("gemini:video".to_string()),
            format_converted: true,
            model: Some("video-model".to_string()),
            prompt: None,
            original_request_body: None,
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: VideoTaskStatus::Submitted,
            progress_percent: 0,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: Some(10),
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: 1,
            submitted_at_unix_secs: Some(1),
            completed_at_unix_secs: None,
            updated_at_unix_secs: 1,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: None,
        };
        let stored = task.clone().into_stored();

        let same_identity_update = UpsertVideoTask {
            status: VideoTaskStatus::Completed,
            progress_percent: 100,
            created_at_unix_ms: 2,
            completed_at_unix_secs: Some(2),
            updated_at_unix_secs: 2,
            ..task.clone()
        };
        stored
            .ensure_immutable_identity_matches(&same_identity_update)
            .expect("mutable state changes should keep the same identity");

        macro_rules! assert_identity_conflict {
            ($field:ident, $value:expr) => {{
                let mut conflicting = task.clone();
                conflicting.$field = $value;
                let error = stored
                    .ensure_immutable_identity_matches(&conflicting)
                    .expect_err(concat!(stringify!($field), " should be immutable"));
                assert!(
                    error
                        .to_string()
                        .contains(concat!("immutable field ", stringify!($field))),
                    "unexpected error for {}: {error}",
                    stringify!($field)
                );
            }};
        }

        assert_identity_conflict!(id, "task-2".to_string());
        assert_identity_conflict!(short_id, Some("short-2".to_string()));
        assert_identity_conflict!(request_id, "request-2".to_string());
        assert_identity_conflict!(user_id, Some("user-2".to_string()));
        assert_identity_conflict!(api_key_id, Some("api-key-2".to_string()));
        assert_identity_conflict!(external_task_id, Some("external-2".to_string()));
        assert_identity_conflict!(provider_id, Some("provider-2".to_string()));
        assert_identity_conflict!(endpoint_id, Some("endpoint-2".to_string()));
        assert_identity_conflict!(key_id, Some("key-2".to_string()));
        assert_identity_conflict!(client_api_format, Some("gemini:video".to_string()));
        assert_identity_conflict!(provider_api_format, Some("openai:video".to_string()));
        assert_identity_conflict!(format_converted, false);
        assert_identity_conflict!(model, Some("other-model".to_string()));
        assert_identity_conflict!(duration_seconds, Some(8));
        assert_identity_conflict!(resolution, Some("1080p".to_string()));
        assert_identity_conflict!(aspect_ratio, Some("9:16".to_string()));
        assert_identity_conflict!(size, Some("1920x1080".to_string()));
    }

    #[test]
    fn upsert_sanitization_drops_sensitive_diagnostics() {
        let mut task = UpsertVideoTask {
            id: "task-1".to_string(),
            short_id: None,
            request_id: "request-1".to_string(),
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            username: Some("private-user-name".to_string()),
            api_key_name: Some("private-key-name".to_string()),
            external_task_id: Some("upstream-1".to_string()),
            provider_id: Some("provider-1".to_string()),
            endpoint_id: Some("endpoint-1".to_string()),
            key_id: Some("key-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("video-model".to_string()),
            prompt: Some("prompt".to_string()),
            original_request_body: Some(serde_json::json!({
                "prompt": "private prompt",
                "api_key": "secret"
            })),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: VideoTaskStatus::Failed,
            progress_percent: 100,
            progress_message: Some("provider response: secret".to_string()),
            retry_count: 1,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 2,
            max_poll_count: 360,
            created_at_unix_ms: 1,
            submitted_at_unix_secs: Some(1),
            completed_at_unix_secs: Some(2),
            updated_at_unix_secs: 2,
            error_code: Some("secret provider code".to_string()),
            error_message: Some("Authorization: Bearer secret".to_string()),
            video_url: Some("https://cdn.example.test/video.mp4?token=secret".to_string()),
            request_metadata: Some(serde_json::json!({
                "rust_local_snapshot": {"transport": {"headers": {"authorization": "secret"}}},
                "poll_raw_response": {"error": "secret"}
            })),
        };

        task.sanitize_for_persistence();

        assert_eq!(task.user_id.as_deref(), Some("user-1"));
        assert_eq!(task.api_key_id.as_deref(), Some("api-key-1"));
        assert_eq!(task.username.as_deref(), Some("private-user-name"));
        assert_eq!(task.api_key_name.as_deref(), Some("private-key-name"));
        assert_eq!(task.original_request_body, None);
        assert_eq!(task.progress_message, None);
        assert_eq!(task.error_message, None);
        assert_eq!(task.error_code.as_deref(), Some("provider_error"));
        assert_eq!(
            task.video_url.as_deref(),
            Some("https://cdn.example.test/video.mp4?token=secret")
        );
        assert_eq!(task.request_metadata, None);
        assert_eq!(task.prompt.as_deref(), Some("prompt"));
        assert_eq!(task.duration_seconds, Some(4));
    }

    #[test]
    fn stored_task_preserves_prompt_and_signed_download_url() {
        let mut args = base_new_args();
        args.12 = Some("gemini:video".to_string());
        args.15 = Some("private prompt".to_string());
        args.35 = Some(
            "https://cdn.example.test/video.mp4?key=secret&alt=media&signature=private#fragment"
                .to_string(),
        );
        let task = StoredVideoTask::new(
            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8, args.9,
            args.10, args.11, args.12, args.13, args.14, args.15, args.16, args.17, args.18,
            args.19, args.20, args.21, args.22, args.23, args.24, args.25, args.26, args.27,
            args.28, args.29, args.30, args.31, args.32, args.33, args.34, args.35, args.36,
        )
        .expect("stored task should build");
        assert_eq!(task.prompt.as_deref(), Some("private prompt"));
        assert_eq!(
            task.video_url.as_deref(),
            Some("https://cdn.example.test/video.mp4?key=secret&alt=media&signature=private")
        );
    }

    #[test]
    fn stored_task_preserves_download_url_when_legacy_provider_format_is_blank() {
        let mut args = base_new_args();
        args.11 = Some("gemini:video".to_string());
        args.12 = Some("   ".to_string());
        args.35 =
            Some("https://cdn.example.test/video.mp4?key=secret&alt=media#fragment".to_string());
        let task = StoredVideoTask::new(
            args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8, args.9,
            args.10, args.11, args.12, args.13, args.14, args.15, args.16, args.17, args.18,
            args.19, args.20, args.21, args.22, args.23, args.24, args.25, args.26, args.27,
            args.28, args.29, args.30, args.31, args.32, args.33, args.34, args.35, args.36,
        )
        .expect("legacy stored task should build");

        assert_eq!(
            task.video_url.as_deref(),
            Some("https://cdn.example.test/video.mp4?key=secret&alt=media")
        );
    }

    #[test]
    fn video_url_sanitization_preserves_signed_query_encoding_and_order() {
        let video_url =
            "https://cdn.example.test/video.mp4?signature=a%2Fb%2Bc%3D&part=2&part=1&name=a%20b";
        assert_eq!(
            super::sanitize_video_task_url(Some(format!("{video_url}#fragment"))).as_deref(),
            Some(video_url)
        );
    }

    #[test]
    fn video_url_sanitization_rejects_invalid_schemes_and_embedded_credentials() {
        for video_url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:video/mp4;base64,AAAA",
            "https://user:password@cdn.example.test/video.mp4",
            "https://user@cdn.example.test/video.mp4",
            "/relative/video.mp4",
            "not a url",
        ] {
            assert!(super::sanitize_video_task_url(Some(video_url.to_string())).is_none());
        }
    }
}
