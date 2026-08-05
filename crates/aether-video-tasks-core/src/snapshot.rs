use aether_data_contracts::repository::video_tasks::{StoredVideoTask, UpsertVideoTask};
use serde_json::{json, Map, Value};

use crate::{
    doubao_content_prompt, doubao_prompt_text, doubao_string_parameter, doubao_u32_parameter,
    local_status_from_stored, non_empty_owned, request_body_string, DoubaoVideoTaskSeed,
    GeminiVideoTaskSeed, LocalVideoTaskPersistence, LocalVideoTaskReadResponse,
    LocalVideoTaskSnapshot, LocalVideoTaskStatus, LocalVideoTaskTransport, OpenAiVideoTaskSeed,
};

impl LocalVideoTaskSnapshot {
    pub fn to_upsert_record(&self) -> UpsertVideoTask {
        match self {
            Self::OpenAi(seed) => seed.to_upsert_record(),
            Self::Gemini(seed) => seed.to_upsert_record(),
            Self::Doubao(seed) => seed.to_upsert_record(),
        }
    }

    pub fn from_stored_task(task: &StoredVideoTask) -> Option<Self> {
        task.request_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("rust_local_snapshot"))
            .cloned()
            .and_then(|value| serde_json::from_value::<LocalVideoTaskSnapshot>(value).ok())
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
                    metadata: Value::Object(Map::new()),
                    persistence,
                    transport,
                }))
            }
            "doubao:video" => {
                let upstream_task_id = non_empty_owned(task.external_task_id.as_ref())?;
                let request_body = &persistence.original_request_body;
                Some(Self::Doubao(DoubaoVideoTaskSeed {
                    local_task_id: task.id.clone(),
                    upstream_task_id,
                    created_at_unix_secs: task.created_at_unix_ms,
                    user_id: task.user_id.clone(),
                    api_key_id: task.api_key_id.clone(),
                    model: non_empty_owned(task.model.as_ref()),
                    prompt: non_empty_owned(task.prompt.as_ref()).or_else(|| {
                        doubao_content_prompt(request_body)
                            .map(|prompt| doubao_prompt_text(&prompt))
                    }),
                    resolution: non_empty_owned(task.resolution.as_ref()).or_else(|| {
                        doubao_string_parameter(request_body, "resolution", &["rs", "resolution"])
                    }),
                    ratio: non_empty_owned(task.aspect_ratio.as_ref()).or_else(|| {
                        doubao_string_parameter(request_body, "ratio", &["rt", "ratio"])
                    }),
                    duration_seconds: task.duration_seconds.or_else(|| {
                        doubao_u32_parameter(request_body, "duration", &["dur", "duration"])
                    }),
                    status: local_status_from_stored(task.status),
                    progress_percent: task.progress_percent,
                    completed_at_unix_secs: task.completed_at_unix_secs,
                    error_code: task.error_code.clone(),
                    error_message: task.error_message.clone(),
                    video_url: non_empty_owned(task.video_url.as_ref()),
                    // Only the video URL is persisted as a column; the last frame
                    // and token usage are recovered from the snapshot when present.
                    last_frame_url: None,
                    completion_tokens: None,
                    total_tokens: None,
                    persistence,
                    transport,
                }))
            }
            _ => None,
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
            Self::Doubao(seed) => match seed.status {
                // Ark reports a removed task as absent rather than as a state.
                LocalVideoTaskStatus::Cancelled | LocalVideoTaskStatus::Deleted => {
                    LocalVideoTaskReadResponse {
                        status_code: 404,
                        body_json: json!({
                            "error": {
                                "code": "NotFound",
                                "message": "The requested generation task was not found.",
                            }
                        }),
                    }
                }
                _ => LocalVideoTaskReadResponse {
                    status_code: 200,
                    body_json: seed.client_body_json(),
                },
            },
        }
    }

    pub fn is_active_for_refresh(&self) -> bool {
        let status = match self {
            Self::OpenAi(seed) => seed.status,
            Self::Gemini(seed) => seed.status,
            Self::Doubao(seed) => seed.status,
        };
        matches!(
            status,
            LocalVideoTaskStatus::Submitted
                | LocalVideoTaskStatus::Queued
                | LocalVideoTaskStatus::Processing
        )
    }

    pub fn apply_provider_body(&mut self, provider_body: &Map<String, Value>) {
        match self {
            Self::OpenAi(seed) => seed.apply_provider_body(provider_body),
            Self::Gemini(seed) => seed.apply_provider_body(provider_body),
            Self::Doubao(seed) => seed.apply_provider_body(provider_body),
        }
    }

    pub fn provider_name(&self) -> Option<&str> {
        match self {
            Self::OpenAi(seed) => seed.transport.provider_name.as_deref(),
            Self::Gemini(seed) => seed.transport.provider_name.as_deref(),
            Self::Doubao(seed) => seed.transport.provider_name.as_deref(),
        }
    }

    /// Token usage reported by the provider, for surfaces that bill by tokens.
    pub fn usage_tokens(&self) -> Option<(u64, u64)> {
        match self {
            Self::Doubao(seed) => {
                let completion_tokens = seed.completion_tokens?;
                Some((
                    completion_tokens,
                    seed.total_tokens.unwrap_or(completion_tokens),
                ))
            }
            Self::OpenAi(_) | Self::Gemini(_) => None,
        }
    }
}
