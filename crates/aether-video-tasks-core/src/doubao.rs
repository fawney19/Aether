use std::collections::BTreeMap;

use aether_contracts::{ExecutionPlan, RequestBody};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskStatus,
};
use serde_json::{json, Map, Value};

use crate::{
    build_video_follow_up_report_context, current_unix_timestamp_secs, doubao_video_tasks_url,
    parse_doubao_video_content_variant, resolve_follow_up_auth, DoubaoVideoTaskSeed,
    LocalVideoTaskContentAction, LocalVideoTaskFollowUpPlan, LocalVideoTaskReadResponse,
    LocalVideoTaskSnapshot, LocalVideoTaskStatus, VideoFollowUpReportContextInput,
    DEFAULT_VIDEO_TASK_MAX_POLL_COUNT, DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS,
};

pub fn map_doubao_stored_task_to_read_response(
    task: StoredVideoTask,
) -> LocalVideoTaskReadResponse {
    match task.status {
        // Ark folds cancel into delete, and a deleted task is simply gone.
        VideoTaskStatus::Cancelled | VideoTaskStatus::Deleted => LocalVideoTaskReadResponse {
            status_code: 404,
            body_json: json!({
                "error": {
                    "code": "NotFound",
                    "message": "The requested generation task was not found.",
                }
            }),
        },
        status => LocalVideoTaskReadResponse {
            status_code: 200,
            body_json: build_doubao_stored_task_body(task, status),
        },
    }
}

fn build_doubao_stored_task_body(task: StoredVideoTask, status: VideoTaskStatus) -> Value {
    let mut body = json!({
        "id": task.id,
        "status": map_doubao_stored_task_status(status),
        "created_at": task.created_at_unix_ms,
        "updated_at": task.updated_at_unix_secs,
    });

    if let Some(model) = task.model {
        body["model"] = Value::String(model);
    }
    if let Some(video_url) = task.video_url {
        body["content"] = json!({ "video_url": video_url });
    }
    if let Some(resolution) = task.resolution {
        body["resolution"] = Value::String(resolution);
    }
    if let Some(ratio) = task.aspect_ratio {
        body["ratio"] = Value::String(ratio);
    }
    if let Some(duration) = task.duration_seconds {
        body["duration"] = Value::Number(duration.into());
    }
    if matches!(status, VideoTaskStatus::Failed | VideoTaskStatus::Expired) {
        body["error"] = json!({
            "code": task.error_code.unwrap_or_else(|| "InternalServiceError".to_string()),
            "message": task
                .error_message
                .unwrap_or_else(|| "Video generation failed".to_string()),
        });
    }

    body
}

fn map_doubao_stored_task_status(status: VideoTaskStatus) -> &'static str {
    match status {
        VideoTaskStatus::Pending | VideoTaskStatus::Submitted | VideoTaskStatus::Queued => "queued",
        VideoTaskStatus::Processing => "running",
        VideoTaskStatus::Completed => "succeeded",
        VideoTaskStatus::Failed | VideoTaskStatus::Expired => "failed",
        VideoTaskStatus::Cancelled | VideoTaskStatus::Deleted => "cancelled",
    }
}

pub fn map_doubao_task_status(status: LocalVideoTaskStatus) -> &'static str {
    match status {
        LocalVideoTaskStatus::Submitted | LocalVideoTaskStatus::Queued => "queued",
        LocalVideoTaskStatus::Processing => "running",
        LocalVideoTaskStatus::Completed => "succeeded",
        LocalVideoTaskStatus::Failed | LocalVideoTaskStatus::Expired => "failed",
        LocalVideoTaskStatus::Cancelled | LocalVideoTaskStatus::Deleted => "cancelled",
    }
}

impl DoubaoVideoTaskSeed {
    pub fn apply_provider_body(&mut self, provider_body: &Map<String, Value>) {
        let raw_status = provider_body
            .get("status")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        self.status = match raw_status {
            "queued" => LocalVideoTaskStatus::Queued,
            "running" => LocalVideoTaskStatus::Processing,
            "succeeded" => LocalVideoTaskStatus::Completed,
            "failed" => LocalVideoTaskStatus::Failed,
            "cancelled" => LocalVideoTaskStatus::Cancelled,
            _ => self.status,
        };
        // Ark reports no progress percentage, so it is derived from the status.
        self.progress_percent = match self.status {
            LocalVideoTaskStatus::Completed => 100,
            LocalVideoTaskStatus::Processing => 50,
            LocalVideoTaskStatus::Failed | LocalVideoTaskStatus::Cancelled => 100,
            _ => self.progress_percent,
        };

        let content = provider_body.get("content").and_then(Value::as_object);
        if let Some(video_url) = content
            .and_then(|content| content.get("video_url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.video_url = Some(video_url.to_string());
        }
        if let Some(last_frame_url) = content
            .and_then(|content| content.get("last_frame_url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.last_frame_url = Some(last_frame_url.to_string());
        }

        let usage = provider_body.get("usage").and_then(Value::as_object);
        if let Some(completion_tokens) = usage
            .and_then(|usage| usage.get("completion_tokens"))
            .and_then(Value::as_u64)
        {
            self.completion_tokens = Some(completion_tokens);
        }
        if let Some(total_tokens) = usage
            .and_then(|usage| usage.get("total_tokens"))
            .and_then(Value::as_u64)
        {
            self.total_tokens = Some(total_tokens);
        }

        // The provider echoes the resolved generation parameters, which are more
        // authoritative than whatever the client asked for.
        if let Some(resolution) = provider_body
            .get("resolution")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.resolution = Some(resolution.to_string());
        }
        if let Some(ratio) = provider_body
            .get("ratio")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.ratio = Some(ratio.to_string());
        }
        if let Some(duration) = provider_body
            .get("duration")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
        {
            self.duration_seconds = Some(duration);
        }

        if let Some(updated_at) = provider_body.get("updated_at").and_then(Value::as_u64) {
            if matches!(
                self.status,
                LocalVideoTaskStatus::Completed
                    | LocalVideoTaskStatus::Failed
                    | LocalVideoTaskStatus::Cancelled
            ) {
                self.completed_at_unix_secs = Some(updated_at);
            }
        }

        let error = provider_body.get("error").and_then(Value::as_object);
        self.error_code = error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .map(str::to_string);
        self.error_message = error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    pub fn client_body_json(&self) -> Value {
        let mut body = json!({
            "id": self.local_task_id,
            "status": map_doubao_task_status(self.status),
            "created_at": self.created_at_unix_secs,
            "updated_at": self.completed_at_unix_secs.unwrap_or(self.created_at_unix_secs),
        });

        if let Some(model) = &self.model {
            body["model"] = Value::String(model.clone());
        }
        if let Some(resolution) = &self.resolution {
            body["resolution"] = Value::String(resolution.clone());
        }
        if let Some(ratio) = &self.ratio {
            body["ratio"] = Value::String(ratio.clone());
        }
        if let Some(duration) = self.duration_seconds {
            body["duration"] = Value::Number(duration.into());
        }
        if self.status == LocalVideoTaskStatus::Completed {
            let mut content = Map::new();
            if let Some(video_url) = &self.video_url {
                content.insert("video_url".to_string(), Value::String(video_url.clone()));
            }
            if let Some(last_frame_url) = &self.last_frame_url {
                content.insert(
                    "last_frame_url".to_string(),
                    Value::String(last_frame_url.clone()),
                );
            }
            if !content.is_empty() {
                body["content"] = Value::Object(content);
            }
        }
        if let Some(completion_tokens) = self.completion_tokens {
            body["usage"] = json!({
                "completion_tokens": completion_tokens,
                "total_tokens": self.total_tokens.unwrap_or(completion_tokens),
            });
        }
        if matches!(
            self.status,
            LocalVideoTaskStatus::Failed | LocalVideoTaskStatus::Expired
        ) {
            body["error"] = json!({
                "code": self
                    .error_code
                    .clone()
                    .unwrap_or_else(|| "InternalServiceError".to_string()),
                "message": self
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Video generation failed".to_string()),
            });
        }

        body
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
            url: doubao_video_tasks_url(
                &self.transport.upstream_base_url,
                Some(&self.upstream_task_id),
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
            client_api_format: "doubao:video".to_string(),
            provider_api_format: "doubao:video".to_string(),
            model_name: self
                .model
                .clone()
                .or_else(|| self.transport.model_name.clone()),
            proxy: self.transport.proxy.clone(),
            transport_profile: self.transport.transport_profile.clone(),
            timeouts: self.transport.timeouts.clone(),
        })
    }

    /// Ark exposes a single DELETE that cancels an in-flight task and removes a
    /// finished one, so both client intents map onto this plan.
    pub fn build_delete_follow_up_plan(
        &self,
        fallback_user_id: Option<&str>,
        fallback_api_key_id: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskFollowUpPlan> {
        if matches!(self.status, LocalVideoTaskStatus::Deleted) {
            return None;
        }
        let (user_id, api_key_id) = resolve_follow_up_auth(
            self.user_id.as_deref(),
            self.api_key_id.as_deref(),
            fallback_user_id,
            fallback_api_key_id,
        )?;
        let model_name = self
            .model
            .clone()
            .or_else(|| self.transport.model_name.clone());

        let mut headers = self.transport.headers.clone();
        headers.remove("content-type");
        headers.remove("content-length");

        Some(LocalVideoTaskFollowUpPlan {
            plan: ExecutionPlan {
                request_id: trace_id.to_string(),
                candidate_id: None,
                provider_name: self.transport.provider_name.clone(),
                provider_id: self.transport.provider_id.clone(),
                endpoint_id: self.transport.endpoint_id.clone(),
                key_id: self.transport.key_id.clone(),
                method: "DELETE".to_string(),
                url: doubao_video_tasks_url(
                    &self.transport.upstream_base_url,
                    Some(&self.upstream_task_id),
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
                client_api_format: "doubao:video".to_string(),
                provider_api_format: "doubao:video".to_string(),
                model_name: model_name.clone(),
                proxy: self.transport.proxy.clone(),
                transport_profile: self.transport.transport_profile.clone(),
                timeouts: self.transport.timeouts.clone(),
            },
            report_kind: Some("doubao_video_delete_sync_finalize".to_string()),
            report_context: Some(build_video_follow_up_report_context(
                VideoFollowUpReportContextInput {
                    request_id: &self.persistence.request_id,
                    user_id: &user_id,
                    api_key_id: &api_key_id,
                    task_id: &self.local_task_id,
                    provider_id: &self.transport.provider_id,
                    endpoint_id: &self.transport.endpoint_id,
                    key_id: &self.transport.key_id,
                    provider_name: self.transport.provider_name.as_deref(),
                    model_name: model_name.as_deref(),
                    client_api_format: "doubao:video",
                    provider_api_format: "doubao:video",
                },
            )),
        })
    }

    /// Proxies the generated asset so clients never see the signed upstream URL,
    /// which also avoids handing out a link that expires within a day.
    pub fn build_content_stream_action(
        &self,
        query_string: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskContentAction> {
        match self.status {
            LocalVideoTaskStatus::Submitted
            | LocalVideoTaskStatus::Queued
            | LocalVideoTaskStatus::Processing => {
                return Some(LocalVideoTaskContentAction::Immediate {
                    status_code: 202,
                    body_json: json!({
                        "error": {
                            "code": "TaskNotCompleted",
                            "message": format!(
                                "Video is still processing (status: {})",
                                map_doubao_task_status(self.status)
                            ),
                        }
                    }),
                });
            }
            LocalVideoTaskStatus::Failed | LocalVideoTaskStatus::Expired => {
                return Some(LocalVideoTaskContentAction::Immediate {
                    status_code: 422,
                    body_json: json!({
                        "error": {
                            "code": self
                                .error_code
                                .clone()
                                .unwrap_or_else(|| "InternalServiceError".to_string()),
                            "message": self
                                .error_message
                                .clone()
                                .unwrap_or_else(|| "Video generation failed".to_string()),
                        }
                    }),
                });
            }
            LocalVideoTaskStatus::Cancelled | LocalVideoTaskStatus::Deleted => {
                return Some(LocalVideoTaskContentAction::Immediate {
                    status_code: 404,
                    body_json: json!({
                        "error": {
                            "code": "NotFound",
                            "message": "The requested generation task was not found.",
                        }
                    }),
                });
            }
            LocalVideoTaskStatus::Completed => {}
        }

        let variant = parse_doubao_video_content_variant(query_string)?;
        let url = match variant {
            "last_frame" => self.last_frame_url.clone(),
            _ => self.video_url.clone(),
        }
        .filter(|value| value.starts_with("http://") || value.starts_with("https://"))?;

        Some(LocalVideoTaskContentAction::StreamPlan(Box::new(
            ExecutionPlan {
                request_id: trace_id.to_string(),
                candidate_id: None,
                provider_name: self.transport.provider_name.clone(),
                provider_id: self.transport.provider_id.clone(),
                endpoint_id: self.transport.endpoint_id.clone(),
                key_id: self.transport.key_id.clone(),
                method: "GET".to_string(),
                url,
                // The asset URL is pre-signed, so upstream credentials must not ride along.
                headers: BTreeMap::new(),
                content_type: None,
                content_encoding: None,
                body: RequestBody {
                    json_body: None,
                    body_bytes_b64: None,
                    body_ref: None,
                },
                stream: true,
                client_api_format: "doubao:video".to_string(),
                provider_api_format: "doubao:video".to_string(),
                model_name: self
                    .model
                    .clone()
                    .or_else(|| self.transport.model_name.clone()),
                proxy: self.transport.proxy.clone(),
                transport_profile: self.transport.transport_profile.clone(),
                timeouts: self.transport.timeouts.clone(),
            },
        )))
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
            id: self.local_task_id.clone(),
            // The column is NOT NULL; Doubao has no short-id concept of its own.
            short_id: Some(crate::derive_video_task_short_id(&self.local_task_id)),
            request_id: self.persistence.request_id.clone(),
            user_id: self.user_id.clone(),
            api_key_id: self.api_key_id.clone(),
            username: self.persistence.username.clone(),
            api_key_name: self.persistence.api_key_name.clone(),
            external_task_id: Some(self.upstream_task_id.clone()),
            provider_id: Some(self.transport.provider_id.clone()),
            endpoint_id: Some(self.transport.endpoint_id.clone()),
            key_id: Some(self.transport.key_id.clone()),
            client_api_format: Some(self.persistence.client_api_format.clone()),
            provider_api_format: Some(self.persistence.provider_api_format.clone()),
            format_converted: self.persistence.format_converted,
            model: self.model.clone().or_else(|| Some(String::new())),
            prompt: self.prompt.clone().or_else(|| Some(String::new())),
            original_request_body: Some(self.persistence.original_request_body.clone()),
            duration_seconds: self.duration_seconds,
            resolution: self.resolution.clone(),
            aspect_ratio: self.ratio.clone(),
            size: None,
            status: self.status.as_database_status(),
            progress_percent: self.progress_percent,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS,
            next_poll_at_unix_secs,
            poll_count: 0,
            max_poll_count: DEFAULT_VIDEO_TASK_MAX_POLL_COUNT,
            created_at_unix_ms: self.created_at_unix_secs,
            submitted_at_unix_secs: Some(self.created_at_unix_secs),
            completed_at_unix_secs: self.completed_at_unix_secs,
            updated_at_unix_secs: self.completed_at_unix_secs.unwrap_or(now_unix_secs),
            error_code: self.error_code.clone(),
            error_message: self.error_message.clone(),
            video_url: self.video_url.clone(),
            request_metadata: Some(json!({
                "rust_owner": "async_task",
                "rust_local_snapshot": LocalVideoTaskSnapshot::Doubao(self.clone()),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
    use serde_json::json;

    use super::map_doubao_stored_task_to_read_response;
    use crate::{
        DoubaoVideoTaskSeed, LocalVideoTaskPersistence, LocalVideoTaskStatus,
        LocalVideoTaskTransport,
    };

    fn sample_seed() -> DoubaoVideoTaskSeed {
        DoubaoVideoTaskSeed {
            local_task_id: "cgt-local-123".to_string(),
            upstream_task_id: "cgt-upstream-123".to_string(),
            created_at_unix_secs: 1_768_294_532,
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            model: Some("doubao-seedance-2-0-260128".to_string()),
            prompt: Some("a cat yawning".to_string()),
            resolution: None,
            ratio: Some("16:9".to_string()),
            duration_seconds: Some(11),
            status: LocalVideoTaskStatus::Submitted,
            progress_percent: 0,
            completed_at_unix_secs: None,
            error_code: None,
            error_message: None,
            video_url: None,
            last_frame_url: None,
            completion_tokens: None,
            total_tokens: None,
            persistence: LocalVideoTaskPersistence {
                request_id: "req-1".to_string(),
                username: None,
                api_key_name: None,
                client_api_format: "doubao:video".to_string(),
                provider_api_format: "doubao:video".to_string(),
                original_request_body: json!({}),
                format_converted: false,
            },
            transport: LocalVideoTaskTransport {
                upstream_base_url: "https://ark.cn-beijing.volces.com/api".to_string(),
                provider_name: Some("Ark".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "key-1".to_string(),
                headers: Default::default(),
                content_type: Some("application/json".to_string()),
                model_name: Some("doubao-seedance-2-0-260128".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: None,
            },
        }
    }

    fn sample_stored_task(status: VideoTaskStatus) -> StoredVideoTask {
        StoredVideoTask {
            id: "cgt-local-123".to_string(),
            short_id: None,
            request_id: "req-1".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            external_task_id: Some("cgt-upstream-123".to_string()),
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            client_api_format: Some("doubao:video".to_string()),
            provider_api_format: Some("doubao:video".to_string()),
            format_converted: false,
            model: Some("doubao-seedance-2-0-260128".to_string()),
            prompt: Some("a cat yawning".to_string()),
            original_request_body: None,
            duration_seconds: Some(11),
            resolution: None,
            aspect_ratio: Some("16:9".to_string()),
            size: None,
            status,
            progress_percent: 100,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: 1_768_294_532,
            submitted_at_unix_secs: Some(1_768_294_532),
            completed_at_unix_secs: Some(1_768_294_581),
            updated_at_unix_secs: 1_768_294_581,
            error_code: None,
            error_message: None,
            video_url: Some("https://tos.example.com/video.mp4?X-Sig=abc".to_string()),
            request_metadata: None,
        }
    }

    #[test]
    fn applies_succeeded_provider_body() {
        let mut seed = sample_seed();
        let body = json!({
            "id": "cgt-upstream-123",
            "model": "doubao-seedance-2-0-260128",
            "status": "succeeded",
            "content": {"video_url": "https://tos.example.com/v.mp4?X-Sig=abc"},
            "usage": {"completion_tokens": 295_800, "total_tokens": 295_800},
            "created_at": 1_768_294_532,
            "updated_at": 1_768_294_581,
            "resolution": "1080p",
            "ratio": "16:9",
            "duration": 11
        });

        seed.apply_provider_body(body.as_object().expect("object"));

        assert_eq!(seed.status, LocalVideoTaskStatus::Completed);
        assert_eq!(seed.progress_percent, 100);
        assert_eq!(
            seed.video_url.as_deref(),
            Some("https://tos.example.com/v.mp4?X-Sig=abc")
        );
        assert_eq!(seed.completion_tokens, Some(295_800));
        assert_eq!(seed.total_tokens, Some(295_800));
        assert_eq!(seed.resolution.as_deref(), Some("1080p"));
        assert_eq!(seed.completed_at_unix_secs, Some(1_768_294_581));
    }

    #[test]
    fn maps_running_status_to_processing() {
        let mut seed = sample_seed();
        seed.apply_provider_body(json!({"status": "running"}).as_object().expect("object"));

        assert_eq!(seed.status, LocalVideoTaskStatus::Processing);
        assert_eq!(seed.progress_percent, 50);
    }

    #[test]
    fn applies_failed_provider_body_with_error() {
        let mut seed = sample_seed();
        seed.apply_provider_body(
            json!({
                "status": "failed",
                "error": {"code": "InputImageSensitiveContentDetected", "message": "blocked"}
            })
            .as_object()
            .expect("object"),
        );

        assert_eq!(seed.status, LocalVideoTaskStatus::Failed);
        assert_eq!(
            seed.error_code.as_deref(),
            Some("InputImageSensitiveContentDetected")
        );
        assert_eq!(seed.error_message.as_deref(), Some("blocked"));
    }

    #[test]
    fn unknown_status_keeps_previous_state() {
        let mut seed = sample_seed();
        seed.status = LocalVideoTaskStatus::Processing;
        seed.apply_provider_body(json!({"status": "weird"}).as_object().expect("object"));

        assert_eq!(seed.status, LocalVideoTaskStatus::Processing);
    }

    #[test]
    fn client_body_uses_local_id_and_doubao_shape() {
        let mut seed = sample_seed();
        seed.status = LocalVideoTaskStatus::Completed;
        seed.video_url = Some("https://tos.example.com/v.mp4".to_string());
        seed.completion_tokens = Some(1_000);

        let body = seed.client_body_json();

        assert_eq!(body["id"], "cgt-local-123");
        assert_eq!(body["status"], "succeeded");
        assert_eq!(
            body["content"]["video_url"],
            "https://tos.example.com/v.mp4"
        );
        assert_eq!(body["usage"]["completion_tokens"], 1_000);
        assert_eq!(body["ratio"], "16:9");
        assert_eq!(body["duration"], 11);
    }

    #[test]
    fn get_follow_up_plan_targets_task_resource() {
        let seed = sample_seed();
        let plan = seed.build_get_follow_up_plan("trace-1").expect("plan");

        assert_eq!(plan.method, "GET");
        assert_eq!(
            plan.url,
            "https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks/cgt-upstream-123"
        );
        assert_eq!(plan.provider_api_format, "doubao:video");
    }

    #[test]
    fn terminal_task_has_no_get_follow_up_plan() {
        let mut seed = sample_seed();
        seed.status = LocalVideoTaskStatus::Completed;

        assert!(seed.build_get_follow_up_plan("trace-1").is_none());
    }

    #[test]
    fn content_stream_action_drops_upstream_auth_for_signed_urls() {
        let mut seed = sample_seed();
        seed.status = LocalVideoTaskStatus::Completed;
        seed.video_url = Some("https://tos.example.com/v.mp4?X-Sig=abc".to_string());
        seed.transport.headers.insert(
            "authorization".to_string(),
            "Bearer upstream-secret".to_string(),
        );

        let action = seed
            .build_content_stream_action(None, "trace-1")
            .expect("action");

        match action {
            crate::LocalVideoTaskContentAction::StreamPlan(plan) => {
                assert_eq!(plan.url, "https://tos.example.com/v.mp4?X-Sig=abc");
                assert!(plan.headers.is_empty(), "signed URL must not carry auth");
                assert!(plan.stream);
            }
            other => panic!("expected stream plan, got {other:?}"),
        }
    }

    #[test]
    fn content_stream_action_reports_pending_task() {
        let seed = sample_seed();
        let action = seed
            .build_content_stream_action(None, "trace-1")
            .expect("action");

        match action {
            crate::LocalVideoTaskContentAction::Immediate {
                status_code,
                body_json,
            } => {
                assert_eq!(status_code, 202);
                assert_eq!(body_json["error"]["code"], "TaskNotCompleted");
            }
            other => panic!("expected immediate response, got {other:?}"),
        }
    }

    #[test]
    fn delete_follow_up_plan_uses_delete_verb() {
        let seed = sample_seed();
        let plan = seed
            .build_delete_follow_up_plan(None, None, "trace-1")
            .expect("plan");

        assert_eq!(plan.plan.method, "DELETE");
        assert_eq!(
            plan.plan.url,
            "https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks/cgt-upstream-123"
        );
        assert_eq!(
            plan.report_kind.as_deref(),
            Some("doubao_video_delete_sync_finalize")
        );
    }

    #[test]
    fn upsert_record_carries_billing_dimensions_and_snapshot() {
        let mut seed = sample_seed();
        seed.status = LocalVideoTaskStatus::Completed;
        seed.completion_tokens = Some(295_800);

        let record = seed.to_upsert_record();

        assert_eq!(record.id, "cgt-local-123");
        assert_eq!(record.external_task_id.as_deref(), Some("cgt-upstream-123"));
        assert_eq!(record.aspect_ratio.as_deref(), Some("16:9"));
        assert_eq!(record.duration_seconds, Some(11));
        assert_eq!(record.next_poll_at_unix_secs, None);
        assert!(record
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("rust_local_snapshot"))
            .is_some());
    }

    #[test]
    fn upsert_record_satisfies_the_short_id_column_constraint() {
        // `video_tasks.short_id` is NOT NULL and capped at 16 characters; a
        // missing or oversized value fails the insert at runtime only.
        let seed = DoubaoVideoTaskSeed {
            local_task_id: format!("cgt-{}", "a".repeat(32)),
            ..sample_seed()
        };

        let short_id = seed.to_upsert_record().short_id.expect("short id required");

        assert!(!short_id.is_empty());
        assert!(short_id.len() <= 16, "short_id must fit the column");
    }

    #[test]
    fn maps_deleted_stored_task_to_not_found() {
        let response =
            map_doubao_stored_task_to_read_response(sample_stored_task(VideoTaskStatus::Deleted));

        assert_eq!(response.status_code, 404);
        assert_eq!(response.body_json["error"]["code"], "NotFound");
    }

    #[test]
    fn maps_completed_stored_task_to_doubao_body() {
        let response =
            map_doubao_stored_task_to_read_response(sample_stored_task(VideoTaskStatus::Completed));

        assert_eq!(response.status_code, 200);
        assert_eq!(response.body_json["id"], "cgt-local-123");
        assert_eq!(response.body_json["status"], "succeeded");
        assert_eq!(
            response.body_json["content"]["video_url"],
            "https://tos.example.com/video.mp4?X-Sig=abc"
        );
    }
}
