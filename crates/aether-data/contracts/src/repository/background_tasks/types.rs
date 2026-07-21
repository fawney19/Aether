use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum BackgroundTaskKind {
    Scheduled,
    Daemon,
    OnDemand,
    FireAndForget,
}

impl BackgroundTaskKind {
    pub fn as_database(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Daemon => "daemon",
            Self::OnDemand => "on_demand",
            Self::FireAndForget => "fire_and_forget",
        }
    }

    pub fn from_database(value: &str) -> Result<Self, crate::DataLayerError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "scheduled" => Ok(Self::Scheduled),
            "daemon" => Ok(Self::Daemon),
            "on_demand" => Ok(Self::OnDemand),
            "fire_and_forget" => Ok(Self::FireAndForget),
            other => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unsupported background_tasks.kind: {other}"
            ))),
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum BackgroundTaskStatus {
    Queued,
    Running,
    Retrying,
    Succeeded,
    Failed,
    Cancelled,
    Skipped,
}

impl BackgroundTaskStatus {
    pub fn as_database(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
        }
    }

    pub fn from_database(value: &str) -> Result<Self, crate::DataLayerError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "retrying" => Ok(Self::Retrying),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "skipped" => Ok(Self::Skipped),
            other => Err(crate::DataLayerError::UnexpectedValue(format!(
                "unsupported background_tasks.status: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredBackgroundTaskRun {
    pub id: String,
    pub task_key: String,
    pub kind: BackgroundTaskKind,
    pub trigger: String,
    pub status: BackgroundTaskStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    pub owner_instance: Option<String>,
    pub progress_percent: u16,
    pub progress_message: Option<String>,
    pub payload_json: Option<Value>,
    pub result_json: Option<Value>,
    pub error_message: Option<String>,
    pub cancel_requested: bool,
    pub created_by: Option<String>,
    pub created_at_unix_secs: u64,
    pub started_at_unix_secs: Option<u64>,
    pub finished_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertBackgroundTaskRun {
    pub id: String,
    pub task_key: String,
    pub kind: BackgroundTaskKind,
    pub trigger: String,
    pub status: BackgroundTaskStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    pub owner_instance: Option<String>,
    pub progress_percent: u16,
    pub progress_message: Option<String>,
    pub payload_json: Option<Value>,
    pub result_json: Option<Value>,
    pub error_message: Option<String>,
    pub cancel_requested: bool,
    pub created_by: Option<String>,
    pub created_at_unix_secs: u64,
    pub started_at_unix_secs: Option<u64>,
    pub finished_at_unix_secs: Option<u64>,
    pub updated_at_unix_secs: u64,
}

impl UpsertBackgroundTaskRun {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.id.trim().is_empty()
            || self.task_key.trim().is_empty()
            || self.trigger.trim().is_empty()
        {
            return Err(crate::DataLayerError::UnexpectedValue(
                "background task run identity is empty".to_string(),
            ));
        }
        if self.progress_percent > 100 {
            return Err(crate::DataLayerError::UnexpectedValue(format!(
                "background task progress_percent out of range: {}",
                self.progress_percent
            )));
        }
        Ok(())
    }

    pub fn into_stored(self) -> StoredBackgroundTaskRun {
        StoredBackgroundTaskRun {
            id: self.id,
            task_key: self.task_key,
            kind: self.kind,
            trigger: self.trigger,
            status: self.status,
            attempt: self.attempt,
            max_attempts: self.max_attempts,
            owner_instance: self.owner_instance,
            progress_percent: self.progress_percent,
            progress_message: self.progress_message,
            payload_json: self.payload_json,
            result_json: self.result_json,
            error_message: self.error_message,
            cancel_requested: self.cancel_requested,
            created_by: self.created_by,
            created_at_unix_secs: self.created_at_unix_secs,
            started_at_unix_secs: self.started_at_unix_secs,
            finished_at_unix_secs: self.finished_at_unix_secs,
            updated_at_unix_secs: self.updated_at_unix_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StoredBackgroundTaskEvent {
    pub id: String,
    pub run_id: String,
    pub event_type: String,
    pub message: String,
    pub payload_json: Option<Value>,
    pub created_at_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertBackgroundTaskEvent {
    pub id: String,
    pub run_id: String,
    pub event_type: String,
    pub message: String,
    pub payload_json: Option<Value>,
    pub created_at_unix_secs: u64,
}

impl UpsertBackgroundTaskEvent {
    pub fn validate(&self) -> Result<(), crate::DataLayerError> {
        if self.id.trim().is_empty()
            || self.run_id.trim().is_empty()
            || self.event_type.trim().is_empty()
            || self.message.trim().is_empty()
        {
            return Err(crate::DataLayerError::UnexpectedValue(
                "background task event identity is empty".to_string(),
            ));
        }
        Ok(())
    }

    pub fn into_stored(self) -> StoredBackgroundTaskEvent {
        StoredBackgroundTaskEvent {
            id: self.id,
            run_id: self.run_id,
            event_type: self.event_type,
            message: self.message,
            payload_json: self.payload_json,
            created_at_unix_secs: self.created_at_unix_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct BackgroundTaskListQuery {
    pub task_key_substring: Option<String>,
    pub kind: Option<BackgroundTaskKind>,
    pub status: Option<BackgroundTaskStatus>,
    pub trigger: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredBackgroundTaskRunPage {
    pub items: Vec<StoredBackgroundTaskRun>,
    pub total: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct BackgroundTaskSummary {
    pub total: u64,
    pub running_count: u64,
    pub by_status: BTreeMap<String, u64>,
    pub by_kind: BTreeMap<String, u64>,
}

#[async_trait]
pub trait BackgroundTaskReadRepository: Send + Sync {
    async fn find_run(
        &self,
        run_id: &str,
    ) -> Result<Option<StoredBackgroundTaskRun>, crate::DataLayerError>;

    async fn list_runs(
        &self,
        query: &BackgroundTaskListQuery,
    ) -> Result<StoredBackgroundTaskRunPage, crate::DataLayerError>;

    async fn list_events(
        &self,
        run_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<StoredBackgroundTaskEvent>, crate::DataLayerError>;

    async fn summarize_runs(&self) -> Result<BackgroundTaskSummary, crate::DataLayerError>;
}

#[async_trait]
pub trait BackgroundTaskWriteRepository: Send + Sync {
    async fn upsert_run(
        &self,
        run: UpsertBackgroundTaskRun,
    ) -> Result<StoredBackgroundTaskRun, crate::DataLayerError>;

    async fn request_cancel(
        &self,
        run_id: &str,
        updated_at_unix_secs: u64,
    ) -> Result<bool, crate::DataLayerError>;

    async fn upsert_event(
        &self,
        event: UpsertBackgroundTaskEvent,
    ) -> Result<StoredBackgroundTaskEvent, crate::DataLayerError>;
}

pub trait BackgroundTaskRepository:
    BackgroundTaskReadRepository + BackgroundTaskWriteRepository + Send + Sync
{
}

impl<T> BackgroundTaskRepository for T where
    T: BackgroundTaskReadRepository + BackgroundTaskWriteRepository + Send + Sync
{
}
