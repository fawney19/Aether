use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;

const BACKGROUND_TASK_DEFAULT_ERROR_CODE: &str = "background_task_failed";
const BACKGROUND_TASK_UNCLASSIFIED_EVENT: &str = "unclassified_event";
const MAX_BACKGROUND_TASK_METADATA_FIELDS: usize = 48;
const SAFE_BACKGROUND_TASK_METADATA_FIELDS: &[&str] = &[
    "automatic_deletions",
    "bytes",
    "compression",
    "created_count",
    "deleted_endpoints",
    "deleted_keys",
    "encryption",
    "error_code",
    "export_version",
    "exported_at",
    "failed",
    "import_kind",
    "legacy_encrypted_copies_created",
    "legacy_encrypted_copies_verified",
    "legacy_plaintext_objects_deleted",
    "legacy_plaintext_objects_retained",
    "object_cleanup_mode",
    "partition",
    "provider_type",
    "replaced_count",
    "retention_cleanup_candidates",
    "scheduled_slot",
    "scope",
    "sha256",
    "stage",
    "status",
    "success",
    "total",
    "total_endpoints",
    "total_keys",
    "trigger",
    "versioned_storage_cleanup_notice",
    "versioned_storage_cleanup_required",
];

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

impl StoredBackgroundTaskRun {
    pub fn sanitize_persisted_data(&mut self) {
        self.owner_instance = None;
        self.created_by = sanitize_background_task_actor(self.created_by.take());
        self.progress_message = None;
        self.payload_json = sanitize_background_task_metadata(self.payload_json.take());
        self.result_json = sanitize_background_task_metadata(self.result_json.take());
        self.error_message =
            sanitize_background_task_error_code(self.status, self.error_message.take());
    }
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
    pub fn sanitize_for_persistence(&mut self) {
        self.owner_instance = None;
        self.created_by = sanitize_background_task_actor(self.created_by.take());
        self.progress_message = None;
        self.payload_json = sanitize_background_task_metadata(self.payload_json.take());
        self.result_json = sanitize_background_task_metadata(self.result_json.take());
        self.error_message =
            sanitize_background_task_error_code(self.status, self.error_message.take());
    }

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

    pub fn into_stored(mut self) -> StoredBackgroundTaskRun {
        self.sanitize_for_persistence();
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

impl StoredBackgroundTaskEvent {
    pub fn sanitize_persisted_data(&mut self) {
        self.event_type = sanitize_background_task_event_type(&self.event_type);
        self.message = self.event_type.clone();
        self.payload_json = sanitize_background_task_metadata(self.payload_json.take());
    }
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
    pub fn sanitize_for_persistence(&mut self) {
        self.event_type = sanitize_background_task_event_type(&self.event_type);
        self.message = self.event_type.clone();
        self.payload_json = sanitize_background_task_metadata(self.payload_json.take());
    }

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

    pub fn into_stored(mut self) -> StoredBackgroundTaskEvent {
        self.sanitize_for_persistence();
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

fn sanitize_background_task_error_code(
    status: BackgroundTaskStatus,
    value: Option<String>,
) -> Option<String> {
    if status != BackgroundTaskStatus::Failed {
        return None;
    }
    let value = value?.trim().to_ascii_lowercase();
    let code = match value.as_str() {
        "background_task_failed"
        | "background_task_panicked"
        | "provider_delete_failed"
        | "provider_oauth_batch_import_failed"
        | "s3_backup_failed"
        | "s3_backup_slot_record_failed" => value,
        _ => BACKGROUND_TASK_DEFAULT_ERROR_CODE.to_string(),
    };
    Some(code)
}

fn sanitize_background_task_event_type(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "cancel_requested" | "failed" | "queued" | "running" | "skipped" | "succeeded"
        | "worker_boot" => value,
        _ => BACKGROUND_TASK_UNCLASSIFIED_EVENT.to_string(),
    }
}

fn sanitize_background_task_actor(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    matches!(value.as_str(), "admin" | "scheduler" | "system").then_some(value)
}

fn sanitize_background_task_metadata(value: Option<Value>) -> Option<Value> {
    let Value::Object(object) = value? else {
        return None;
    };
    let mut sanitized = serde_json::Map::new();
    for (key, value) in object.into_iter().take(MAX_BACKGROUND_TASK_METADATA_FIELDS) {
        let normalized_key = key.trim().to_ascii_lowercase();
        if !SAFE_BACKGROUND_TASK_METADATA_FIELDS.contains(&normalized_key.as_str()) {
            continue;
        }
        let Some(value) = sanitize_background_task_metadata_value(&normalized_key, value) else {
            continue;
        };
        sanitized.insert(normalized_key, value);
    }
    (!sanitized.is_empty()).then_some(Value::Object(sanitized))
}

fn sanitize_background_task_metadata_value(key: &str, value: Value) -> Option<Value> {
    match key {
        "automatic_deletions"
        | "bytes"
        | "created_count"
        | "deleted_endpoints"
        | "deleted_keys"
        | "failed"
        | "legacy_encrypted_copies_created"
        | "legacy_encrypted_copies_verified"
        | "legacy_plaintext_objects_deleted"
        | "legacy_plaintext_objects_retained"
        | "partition"
        | "replaced_count"
        | "retention_cleanup_candidates"
        | "success"
        | "total"
        | "total_endpoints"
        | "total_keys" => value.is_u64().then_some(value),
        "versioned_storage_cleanup_required" => value.is_boolean().then_some(value),
        "error_code" => value
            .as_str()
            .and_then(sanitize_background_task_metadata_error_code)
            .map(Value::String),
        "scope" => sanitize_background_task_metadata_enum(value, &["config", "data", "users"]),
        "compression" => sanitize_background_task_metadata_enum(value, &["zstd"]),
        "encryption" => sanitize_background_task_metadata_enum(value, &["aes-256-gcm-v2"]),
        "trigger" => sanitize_background_task_metadata_enum(value, &["manual", "scheduled"]),
        "import_kind" => sanitize_background_task_metadata_enum(
            value,
            &["agent_identity", "cookie_authorize", "oauth_batch"],
        ),
        "provider_type" => sanitize_background_task_metadata_enum(
            value,
            &[
                "antigravity",
                "chatgpt_web",
                "claude_code",
                "codex",
                "gemini_cli",
                "kiro",
                "windsurf",
            ],
        ),
        "status" => sanitize_background_task_metadata_enum(
            value,
            &[
                "cancelled",
                "completed",
                "failed",
                "pending",
                "processing",
                "queued",
                "running",
                "skipped",
                "succeeded",
            ],
        ),
        "stage" => sanitize_background_task_metadata_enum(
            value,
            &[
                "completed",
                "deleting_endpoints",
                "deleting_keys",
                "deleting_models",
                "deleting_provider",
                "disabling",
                "failed",
                "preparing",
                "queued",
                "skipped",
            ],
        ),
        "object_cleanup_mode" => sanitize_background_task_metadata_enum(
            value,
            &["legacy_plaintext_deleted_after_verified_encryption"],
        ),
        "sha256" => sanitize_background_task_metadata_hex(value, 64, 64),
        "export_version" => sanitize_background_task_metadata_version(value),
        "exported_at" => sanitize_background_task_metadata_rfc3339(value),
        "scheduled_slot" => sanitize_background_task_metadata_scheduled_slot(value),
        "versioned_storage_cleanup_notice" => sanitize_background_task_metadata_enum(
            value,
            &["legacy_plaintext_versions_require_external_cleanup"],
        ),
        _ => None,
    }
}

fn sanitize_background_task_metadata_enum(value: Value, allowed: &[&str]) -> Option<Value> {
    let value = value.as_str()?.trim().to_ascii_lowercase();
    allowed
        .contains(&value.as_str())
        .then_some(Value::String(value))
}

fn sanitize_background_task_metadata_hex(value: Value, min: usize, max: usize) -> Option<Value> {
    let value = value.as_str()?.trim().to_ascii_lowercase();
    ((min..=max).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(Value::String(value))
}

fn sanitize_background_task_metadata_version(value: Value) -> Option<Value> {
    let value = value.as_str()?.trim();
    (!value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')))
    .then_some(Value::String(value.to_string()))
}

fn sanitize_background_task_metadata_rfc3339(value: Value) -> Option<Value> {
    let value = value.as_str()?.trim();
    (value.len() <= 64 && chrono::DateTime::parse_from_rfc3339(value).is_ok())
        .then_some(Value::String(value.to_string()))
}

fn sanitize_background_task_metadata_scheduled_slot(value: Value) -> Option<Value> {
    let value = value.as_str()?.trim();
    let (unit, timestamp) = value.split_once(':')?;
    (matches!(unit, "hours" | "days" | "weeks" | "months")
        && value.len() <= 80
        && chrono::DateTime::parse_from_rfc3339(timestamp).is_ok())
    .then_some(Value::String(value.to_string()))
}

fn sanitize_background_task_metadata_error_code(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    Some(match value.as_str() {
        "background_task_failed"
        | "background_task_panicked"
        | "provider_delete_failed"
        | "provider_oauth_batch_import_failed"
        | "s3_backup_failed"
        | "s3_backup_slot_record_failed" => value,
        _ if value.is_empty() => return None,
        _ => BACKGROUND_TASK_DEFAULT_ERROR_CODE.to_string(),
    })
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn background_task_run(status: BackgroundTaskStatus) -> UpsertBackgroundTaskRun {
        UpsertBackgroundTaskRun {
            id: "run-1".to_string(),
            task_key: "security-review".to_string(),
            kind: BackgroundTaskKind::OnDemand,
            trigger: "manual".to_string(),
            status,
            attempt: 1,
            max_attempts: 3,
            owner_instance: None,
            progress_percent: 50,
            progress_message: None,
            payload_json: None,
            result_json: None,
            error_message: None,
            cancel_requested: false,
            created_by: None,
            created_at_unix_secs: 1,
            started_at_unix_secs: Some(2),
            finished_at_unix_secs: None,
            updated_at_unix_secs: 3,
        }
    }

    #[test]
    fn run_sanitization_removes_sensitive_and_nested_metadata() {
        let mut run = background_task_run(BackgroundTaskStatus::Running);
        run.owner_instance = Some("gateway-a".to_string());
        run.created_by = Some("admin@example.com".to_string());
        run.progress_message = Some("Bearer secret-access-token".to_string());
        run.payload_json = Some(json!({
            "provider_id": "provider-1",
            "gateway_instance_id": "gateway-a",
            "bucket": "safe-bucket",
            "partition": 7,
            "success": 1,
            "access_token": "secret-access-token",
            "authorization": "Bearer secret-access-token",
            "password": "secret-password",
            "error": "upstream detail containing secret",
            "detail": "private diagnostic",
            "provider_id ": "Bearer secret-access-token",
            "gateway_instance_id ": "gateway-a; Authorization: secret",
            "bucket ": "secret/bucket",
            "nested": {"refresh_token": "secret-refresh-token"},
            "unknown": "must not be persisted"
        }));

        let stored = run.into_stored();

        assert_eq!(stored.progress_message, None);
        assert_eq!(stored.owner_instance, None);
        assert_eq!(stored.created_by, None);
        assert_eq!(
            stored.payload_json,
            Some(json!({
                "partition": 7,
                "success": 1
            }))
        );
    }

    #[test]
    fn run_sanitization_classifies_errors_and_clears_non_failure_errors() {
        let mut failed = background_task_run(BackgroundTaskStatus::Failed);
        failed.error_message = Some("upstream response included a credential".to_string());
        failed.result_json = Some(json!({
            "error_code": "raw-provider-error",
            "failed": 2,
            "token": "secret"
        }));

        let failed = failed.into_stored();
        assert_eq!(
            failed.error_message.as_deref(),
            Some(BACKGROUND_TASK_DEFAULT_ERROR_CODE)
        );
        assert_eq!(
            failed.result_json,
            Some(json!({
                "error_code": BACKGROUND_TASK_DEFAULT_ERROR_CODE,
                "failed": 2
            }))
        );

        let mut succeeded = background_task_run(BackgroundTaskStatus::Succeeded);
        succeeded.error_message = Some("provider_delete_failed".to_string());
        assert_eq!(succeeded.into_stored().error_message, None);
    }

    #[test]
    fn historical_run_sanitization_applies_the_same_read_boundary() {
        let mut stored = background_task_run(BackgroundTaskStatus::Failed).into_stored();
        stored.progress_message = Some("legacy diagnostic with token".to_string());
        stored.payload_json = Some(json!({
            "scope": "data",
            "refresh_token": "legacy-secret",
            "nested": {"password": "legacy-password"}
        }));
        stored.error_message = Some("legacy upstream error: legacy-secret".to_string());

        stored.sanitize_persisted_data();

        assert_eq!(stored.progress_message, None);
        assert_eq!(stored.payload_json, Some(json!({"scope": "data"})));
        assert_eq!(
            stored.error_message.as_deref(),
            Some(BACKGROUND_TASK_DEFAULT_ERROR_CODE)
        );
    }

    #[test]
    fn event_sanitization_canonicalizes_type_message_and_payload() {
        let event = UpsertBackgroundTaskEvent {
            id: "event-1".to_string(),
            run_id: "run-1".to_string(),
            event_type: "provider returned secret-token".to_string(),
            message: "Authorization: Bearer secret-token".to_string(),
            payload_json: Some(json!({
                "stage": "finalize",
                "bytes": 42,
                "error_code": "upstream said secret-token",
                "error": "secret-token",
                "detail": "credential detail",
                "token": "secret-token",
                "nested": {"authorization": "Bearer secret-token"}
            })),
            created_at_unix_secs: 4,
        }
        .into_stored();

        assert_eq!(event.event_type, BACKGROUND_TASK_UNCLASSIFIED_EVENT);
        assert_eq!(event.message, BACKGROUND_TASK_UNCLASSIFIED_EVENT);
        assert_eq!(
            event.payload_json,
            Some(json!({
                "bytes": 42,
                "error_code": BACKGROUND_TASK_DEFAULT_ERROR_CODE
            }))
        );
    }
}
