use aether_contracts::{ExecutionPlan, RequestBody};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskStatus,
};
use serde_json::{json, Map, Value};

use crate::{
    build_video_follow_up_report_context, current_unix_timestamp_secs, gemini_metadata_video_url,
    request_body_string, request_body_u32, resolve_follow_up_auth, GeminiVideoTaskSeed,
    LocalVideoTaskFollowUpPlan, LocalVideoTaskReadResponse, LocalVideoTaskSnapshot,
    LocalVideoTaskStatus, VideoFollowUpReportContextInput, DEFAULT_VIDEO_TASK_MAX_POLL_COUNT,
    DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS,
};

pub fn map_gemini_stored_task_to_read_response(
    task: StoredVideoTask,
) -> LocalVideoTaskReadResponse {
    match task.status {
        VideoTaskStatus::Cancelled => LocalVideoTaskReadResponse {
            status_code: 404,
            body_json: json!({"detail": "Video task was cancelled"}),
        },
        VideoTaskStatus::Deleted => LocalVideoTaskReadResponse {
            status_code: 404,
            body_json: json!({"detail": "Video task not found"}),
        },
        VideoTaskStatus::Completed => LocalVideoTaskReadResponse {
            status_code: 200,
            body_json: build_gemini_completed_body(task),
        },
        VideoTaskStatus::Failed | VideoTaskStatus::Expired => LocalVideoTaskReadResponse {
            status_code: 200,
            body_json: build_gemini_failed_body(task),
        },
        _ => LocalVideoTaskReadResponse {
            status_code: 200,
            body_json: build_gemini_pending_body(task),
        },
    }
}

fn build_gemini_completed_body(task: StoredVideoTask) -> Value {
    let operation_name = stored_task_operation_name(&task);
    let short_id = task.short_id.unwrap_or_default();

    json!({
        "name": operation_name,
        "done": true,
        "response": {
            "generateVideoResponse": {
                "generatedSamples": [
                    {
                        "video": {
                            "uri": format!("/v1beta/files/aev_{short_id}:download?alt=media"),
                            "mimeType": "video/mp4"
                        }
                    }
                ]
            }
        }
    })
}

fn build_gemini_failed_body(task: StoredVideoTask) -> Value {
    json!({
        "name": stored_task_operation_name(&task),
        "done": true,
        "error": {
            "code": task.error_code.unwrap_or_else(|| "UNKNOWN".to_string()),
            "message": task
                .error_message
                .unwrap_or_else(|| "Video generation failed".to_string()),
        }
    })
}

fn build_gemini_pending_body(task: StoredVideoTask) -> Value {
    json!({
        "name": stored_task_operation_name(&task),
        "done": false,
        "metadata": {}
    })
}

fn stored_task_operation_name(task: &StoredVideoTask) -> String {
    let model = task.model.clone().unwrap_or_else(|| "unknown".to_string());
    let short_id = task.short_id.clone().unwrap_or_else(|| task.id.clone());
    format!("models/{model}/operations/{short_id}")
}

impl GeminiVideoTaskSeed {
    pub fn apply_provider_body(&mut self, provider_body: &Map<String, Value>) {
        let done = provider_body
            .get("done")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if done {
            let error = provider_body.get("error").and_then(Value::as_object);
            if let Some(error) = error {
                self.status = LocalVideoTaskStatus::Failed;
                self.progress_percent = 100;
                self.error_code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.error_message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            } else {
                self.status = LocalVideoTaskStatus::Completed;
                self.progress_percent = 100;
                self.error_code = None;
                self.error_message = None;
            }
            self.metadata = json!({});
            return;
        }

        self.status = LocalVideoTaskStatus::Processing;
        self.progress_percent = 50;
        self.error_code = None;
        self.error_message = None;
        self.metadata = provider_body
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| json!({}));
    }

    pub fn build_get_follow_up_plan(&self, trace_id: &str) -> Option<ExecutionPlan> {
        if !matches!(
            self.status,
            LocalVideoTaskStatus::Submitted
                | LocalVideoTaskStatus::Queued
                | LocalVideoTaskStatus::Processing
        ) {
            return None;
        }

        let operation_path = self.resolve_operation_path()?;
        let mut headers = self.transport.headers.clone();
        headers.remove("content-type");
        headers.remove("content-length");

        Some(ExecutionPlan {
            request_id: trace_id.to_string(),
            candidate_id: None,
            provider_name: self.transport.provider_name.clone(),
            provider_id: self.transport.provider_id.clone(),
            endpoint_id: self.transport.endpoint_id.clone(),
            key_id: self.transport.key_id.clone(),
            method: "GET".to_string(),
            url: format!(
                "{}/v1beta/{}",
                self.transport.upstream_base_url.trim_end_matches('/'),
                operation_path
            ),
            headers,
            content_type: None,
            content_encoding: None,
            body: RequestBody {
                json_body: None,
                body_bytes_b64: None,
                body_ref: None,
            },
            stream: false,
            client_api_format: "gemini:video".to_string(),
            provider_api_format: "gemini:video".to_string(),
            model_name: Some(self.model.clone()),
            proxy: self.transport.proxy.clone(),
            tls_profile: self.transport.tls_profile.clone(),
            timeouts: self.transport.timeouts.clone(),
        })
    }

    pub fn build_cancel_follow_up_plan(
        &self,
        fallback_user_id: Option<&str>,
        fallback_api_key_id: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskFollowUpPlan> {
        if !matches!(
            self.status,
            LocalVideoTaskStatus::Submitted
                | LocalVideoTaskStatus::Queued
                | LocalVideoTaskStatus::Processing
        ) {
            return None;
        }
        let (user_id, api_key_id) = resolve_follow_up_auth(
            self.user_id.as_deref(),
            self.api_key_id.as_deref(),
            fallback_user_id,
            fallback_api_key_id,
        )?;

        let operation_path = self.resolve_operation_path()?;
        let mut headers = self.transport.headers.clone();
        let content_type = self
            .transport
            .content_type
            .clone()
            .unwrap_or_else(|| "application/json".to_string());
        headers
            .entry("content-type".to_string())
            .or_insert_with(|| content_type.clone());

        Some(LocalVideoTaskFollowUpPlan {
            plan: ExecutionPlan {
                request_id: trace_id.to_string(),
                candidate_id: None,
                provider_name: self.transport.provider_name.clone(),
                provider_id: self.transport.provider_id.clone(),
                endpoint_id: self.transport.endpoint_id.clone(),
                key_id: self.transport.key_id.clone(),
                method: "POST".to_string(),
                url: format!(
                    "{}/v1beta/{}:cancel",
                    self.transport.upstream_base_url.trim_end_matches('/'),
                    operation_path
                ),
                headers,
                content_type: Some(content_type),
                content_encoding: None,
                body: RequestBody::from_json(json!({})),
                stream: false,
                client_api_format: "gemini:video".to_string(),
                provider_api_format: "gemini:video".to_string(),
                model_name: Some(self.model.clone()),
                proxy: self.transport.proxy.clone(),
                tls_profile: self.transport.tls_profile.clone(),
                timeouts: self.transport.timeouts.clone(),
            },
            report_kind: Some("gemini_video_cancel_sync_finalize".to_string()),
            report_context: Some(build_video_follow_up_report_context(
                VideoFollowUpReportContextInput {
                    request_id: &self.persistence.request_id,
                    user_id: &user_id,
                    api_key_id: &api_key_id,
                    task_id: &self.local_short_id,
                    provider_id: &self.transport.provider_id,
                    endpoint_id: &self.transport.endpoint_id,
                    key_id: &self.transport.key_id,
                    provider_name: self.transport.provider_name.as_deref(),
                    model_name: Some(self.model.as_str()),
                    client_api_format: "gemini:video",
                    provider_api_format: "gemini:video",
                },
            )),
        })
    }

    pub fn client_body_json(&self) -> Value {
        let operation_name = format!("models/{}/operations/{}", self.model, self.local_short_id);
        match self.status {
            LocalVideoTaskStatus::Completed => json!({
                "name": operation_name,
                "done": true,
                "response": {
                    "generateVideoResponse": {
                        "generatedSamples": [
                            {
                                "video": {
                                    "uri": format!("/v1beta/files/aev_{}:download?alt=media", self.local_short_id),
                                    "mimeType": "video/mp4"
                                }
                            }
                        ]
                    }
                }
            }),
            LocalVideoTaskStatus::Failed | LocalVideoTaskStatus::Expired => json!({
                "name": operation_name,
                "done": true,
                "error": {
                    "code": self.error_code.clone().unwrap_or_else(|| "UNKNOWN".to_string()),
                    "message": self
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "Video generation failed".to_string()),
                }
            }),
            _ => json!({
                "name": operation_name,
                "done": false,
                "metadata": self.metadata.clone(),
            }),
        }
    }

    pub fn to_upsert_record(&self) -> UpsertVideoTask {
        let now_unix_secs = current_unix_timestamp_secs();
        let next_poll_at_unix_secs = match self.status {
            LocalVideoTaskStatus::Submitted
            | LocalVideoTaskStatus::Queued
            | LocalVideoTaskStatus::Processing => Some(
                now_unix_secs.saturating_add(u64::from(DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS)),
            ),
            _ => None,
        };
        UpsertVideoTask {
            id: self.local_short_id.clone(),
            short_id: Some(self.local_short_id.clone()),
            request_id: self.persistence.request_id.clone(),
            user_id: self.user_id.clone(),
            api_key_id: self.api_key_id.clone(),
            username: self.persistence.username.clone(),
            api_key_name: self.persistence.api_key_name.clone(),
            external_task_id: Some(self.upstream_operation_name.clone()),
            provider_id: Some(self.transport.provider_id.clone()),
            endpoint_id: Some(self.transport.endpoint_id.clone()),
            key_id: Some(self.transport.key_id.clone()),
            client_api_format: Some(self.persistence.client_api_format.clone()),
            provider_api_format: Some(self.persistence.provider_api_format.clone()),
            format_converted: self.persistence.format_converted,
            model: Some(self.model.clone()),
            prompt: request_body_string(&self.persistence.original_request_body, "prompt")
                .or_else(|| Some(String::new())),
            original_request_body: Some(self.persistence.original_request_body.clone()),
            duration_seconds: request_body_u32(&self.persistence.original_request_body, "seconds")
                .or_else(|| {
                    request_body_u32(&self.persistence.original_request_body, "duration_seconds")
                }),
            resolution: request_body_string(&self.persistence.original_request_body, "resolution"),
            aspect_ratio: request_body_string(
                &self.persistence.original_request_body,
                "aspect_ratio",
            ),
            size: request_body_string(&self.persistence.original_request_body, "size"),
            status: self.status.as_database_status(),
            progress_percent: self.progress_percent,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS,
            next_poll_at_unix_secs,
            poll_count: 0,
            max_poll_count: DEFAULT_VIDEO_TASK_MAX_POLL_COUNT,
            created_at_unix_secs: now_unix_secs,
            submitted_at_unix_secs: Some(now_unix_secs),
            completed_at_unix_secs: None,
            updated_at_unix_secs: now_unix_secs,
            error_code: self.error_code.clone(),
            error_message: self.error_message.clone(),
            video_url: gemini_metadata_video_url(&self.metadata),
            request_metadata: Some(json!({
                "rust_owner": "async_task",
                "rust_local_snapshot": LocalVideoTaskSnapshot::Gemini(self.clone()),
            })),
        }
    }

    fn resolve_operation_path(&self) -> Option<String> {
        if self.upstream_operation_name.starts_with("models/") {
            Some(self.upstream_operation_name.clone())
        } else if self.upstream_operation_name.starts_with("operations/") && !self.model.is_empty()
        {
            Some(format!(
                "models/{}/{}",
                self.model, self.upstream_operation_name
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};

    use super::map_gemini_stored_task_to_read_response;

    fn sample_stored_task(status: VideoTaskStatus) -> StoredVideoTask {
        StoredVideoTask {
            id: "task-gemini-123".to_string(),
            short_id: Some("localshort123".to_string()),
            request_id: "req-gemini-123".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            external_task_id: Some("operations/ext-gemini-123".to_string()),
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            client_api_format: Some("gemini:video".to_string()),
            provider_api_format: Some("gemini:video".to_string()),
            format_converted: false,
            model: Some("veo-3".to_string()),
            prompt: None,
            original_request_body: None,
            duration_seconds: None,
            resolution: None,
            aspect_ratio: None,
            size: None,
            status,
            progress_percent: 50,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_secs: 1712345678,
            submitted_at_unix_secs: Some(1712345678),
            completed_at_unix_secs: None,
            updated_at_unix_secs: 1712345679,
            error_code: Some("UNKNOWN".to_string()),
            error_message: Some("provider failed".to_string()),
            video_url: None,
            request_metadata: None,
        }
    }

    #[test]
    fn maps_cancelled_gemini_stored_task_into_not_found_response() {
        let response =
            map_gemini_stored_task_to_read_response(sample_stored_task(VideoTaskStatus::Cancelled));

        assert_eq!(response.status_code, 404);
        assert_eq!(response.body_json["detail"], "Video task was cancelled");
    }
}
