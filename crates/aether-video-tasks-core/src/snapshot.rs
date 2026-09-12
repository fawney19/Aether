use aether_data_contracts::repository::video_tasks::{StoredVideoTask, UpsertVideoTask};
use serde_json::{json, Map, Value};

use crate::transport::{gemini_metadata_video_url, gemini_video_metadata};
use crate::types::sanitize_video_task_error_code;
use crate::{
    local_status_from_stored, non_empty_owned, request_body_string, GeminiVideoTaskSeed,
    LocalVideoTaskPersistence, LocalVideoTaskReadResponse, LocalVideoTaskSnapshot,
    LocalVideoTaskStatus, LocalVideoTaskTransport, OpenAiVideoTaskSeed,
};

impl LocalVideoTaskSnapshot {
    pub fn to_upsert_record(&self) -> UpsertVideoTask {
        match self {
            Self::OpenAi(seed) => seed.to_upsert_record(),
            Self::Gemini(seed) => seed.to_upsert_record(),
        }
    }

    pub fn from_stored_task(task: &StoredVideoTask) -> Option<Self> {
        let mut snapshot = task
            .request_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("rust_local_snapshot"))
            .cloned()
            .and_then(|value| serde_json::from_value::<LocalVideoTaskSnapshot>(value).ok())?;

        // The row is the ownership source of truth. Older embedded snapshots can
        // contain stale identity fields after a task import or repair.
        match &mut snapshot {
            Self::OpenAi(seed) => {
                seed.user_id = task.user_id.clone();
                seed.api_key_id = task.api_key_id.clone();
            }
            Self::Gemini(seed) => {
                seed.user_id = task.user_id.clone();
                seed.api_key_id = task.api_key_id.clone();
            }
        }
        Some(snapshot)
    }

    pub fn from_stored_task_with_transport(
        task: &StoredVideoTask,
        transport: LocalVideoTaskTransport,
    ) -> Option<Self> {
        let provider_api_format = task.provider_api_format.as_deref()?.trim();
        let persistence = LocalVideoTaskPersistence::from_stored_task(task)?;

        match provider_api_format {
            "openai:video" => {
                let upstream_task_id = non_empty_owned(task.external_task_id.as_ref())?;
                Some(Self::OpenAi(OpenAiVideoTaskSeed {
                    local_task_id: task.id.clone(),
                    upstream_task_id,
                    created_at_unix_ms: task.created_at_unix_ms,
                    user_id: task.user_id.clone(),
                    api_key_id: task.api_key_id.clone(),
                    model: non_empty_owned(task.model.as_ref()),
                    prompt: non_empty_owned(task.prompt.as_ref()).or_else(|| {
                        request_body_string(&persistence.original_request_body, "prompt")
                    }),
                    size: non_empty_owned(task.size.as_ref()).or_else(|| {
                        request_body_string(&persistence.original_request_body, "size")
                    }),
                    seconds: task
                        .duration_seconds
                        .map(|value| value.to_string())
                        .or_else(|| {
                            request_body_string(&persistence.original_request_body, "seconds")
                        }),
                    remixed_from_video_id: request_body_string(
                        &persistence.original_request_body,
                        "remix_video_id",
                    )
                    .or_else(|| {
                        request_body_string(
                            &persistence.original_request_body,
                            "remixed_from_video_id",
                        )
                    }),
                    status: local_status_from_stored(task.status),
                    progress_percent: task.progress_percent,
                    completed_at_unix_secs: task.completed_at_unix_secs,
                    expires_at_unix_secs: None,
                    error_code: task.error_code.clone(),
                    error_message: task.error_message.clone(),
                    video_url: non_empty_owned(task.video_url.as_ref()),
                    persistence,
                    transport,
                }))
            }
            "gemini:video" => {
                let local_short_id =
                    non_empty_owned(task.short_id.as_ref()).unwrap_or_else(|| task.id.clone());
                let upstream_operation_name = non_empty_owned(task.external_task_id.as_ref())?;
                let model = non_empty_owned(task.model.as_ref())?;
                Some(Self::Gemini(GeminiVideoTaskSeed {
                    local_short_id,
                    upstream_operation_name,
                    user_id: task.user_id.clone(),
                    api_key_id: task.api_key_id.clone(),
                    model,
                    status: local_status_from_stored(task.status),
                    progress_percent: task.progress_percent,
                    error_code: task.error_code.clone(),
                    error_message: task.error_message.clone(),
                    metadata: gemini_video_metadata(task.video_url.as_deref()),
                    persistence,
                    transport,
                }))
            }
            _ => None,
        }
    }

    pub(crate) fn sanitize_persisted_diagnostics(&mut self) -> bool {
        match self {
            Self::OpenAi(seed) => {
                let previous_error_code = seed.error_code.clone();
                let error_code =
                    sanitized_error_code_for_status(seed.status, seed.error_code.take());
                let changed = previous_error_code != error_code || seed.error_message.is_some();
                seed.error_code = error_code;
                seed.error_message = None;
                changed
            }
            Self::Gemini(seed) => {
                let previous_error_code = seed.error_code.clone();
                let error_code =
                    sanitized_error_code_for_status(seed.status, seed.error_code.take());
                let safe_metadata =
                    gemini_video_metadata(gemini_metadata_video_url(&seed.metadata).as_deref());
                let changed = previous_error_code != error_code
                    || seed.error_message.is_some()
                    || seed.metadata != safe_metadata;
                seed.error_code = error_code;
                seed.error_message = None;
                seed.metadata = safe_metadata;
                changed
            }
        }
    }

    pub fn read_response(&self) -> LocalVideoTaskReadResponse {
        match self {
            Self::OpenAi(seed) => match seed.status {
                LocalVideoTaskStatus::Cancelled => LocalVideoTaskReadResponse {
                    status_code: 404,
                    body_json: json!({"detail": "Video task was cancelled"}),
                },
                LocalVideoTaskStatus::Deleted => LocalVideoTaskReadResponse {
                    status_code: 404,
                    body_json: json!({"detail": "Video task not found"}),
                },
                _ => LocalVideoTaskReadResponse {
                    status_code: 200,
                    body_json: seed.client_body_json(),
                },
            },
            Self::Gemini(seed) => match seed.status {
                LocalVideoTaskStatus::Cancelled => LocalVideoTaskReadResponse {
                    status_code: 404,
                    body_json: json!({"detail": "Video task was cancelled"}),
                },
                LocalVideoTaskStatus::Deleted => LocalVideoTaskReadResponse {
                    status_code: 404,
                    body_json: json!({"detail": "Video task not found"}),
                },
                _ => LocalVideoTaskReadResponse {
                    status_code: 200,
                    body_json: seed.client_body_json(),
                },
            },
        }
    }

    pub fn belongs_to_user(&self, user_id: &str) -> bool {
        let user_id = user_id.trim();
        if user_id.is_empty() {
            return false;
        }
        let owner = match self {
            Self::OpenAi(seed) => seed.user_id.as_deref(),
            Self::Gemini(seed) => seed.user_id.as_deref(),
        };
        owner.map(str::trim) == Some(user_id)
    }

    pub fn is_active_for_refresh(&self) -> bool {
        match self {
            Self::OpenAi(seed) => matches!(
                seed.status,
                LocalVideoTaskStatus::Submitted
                    | LocalVideoTaskStatus::Queued
                    | LocalVideoTaskStatus::Processing
            ),
            Self::Gemini(seed) => matches!(
                seed.status,
                LocalVideoTaskStatus::Submitted
                    | LocalVideoTaskStatus::Queued
                    | LocalVideoTaskStatus::Processing
            ),
        }
    }

    pub fn apply_provider_body(&mut self, provider_body: &Map<String, Value>) {
        match self {
            Self::OpenAi(seed) => seed.apply_provider_body(provider_body),
            Self::Gemini(seed) => seed.apply_provider_body(provider_body),
        }
    }

    pub fn provider_name(&self) -> Option<&str> {
        match self {
            Self::OpenAi(seed) => seed.transport.provider_name.as_deref(),
            Self::Gemini(seed) => seed.transport.provider_name.as_deref(),
        }
    }
}

fn sanitized_error_code_for_status(
    status: LocalVideoTaskStatus,
    error_code: Option<String>,
) -> Option<String> {
    match status {
        LocalVideoTaskStatus::Failed => sanitize_video_task_error_code(error_code)
            .or_else(|| Some("provider_error".to_string())),
        LocalVideoTaskStatus::Expired => Some("expired".to_string()),
        LocalVideoTaskStatus::Cancelled => Some("cancelled".to_string()),
        _ => None,
    }
}
