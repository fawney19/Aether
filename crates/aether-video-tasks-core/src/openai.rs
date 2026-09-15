use std::collections::BTreeMap;

use aether_contracts::{ExecutionPlan, RequestBody};
use aether_data_contracts::repository::video_tasks::{
    StoredVideoTask, UpsertVideoTask, VideoTaskStatus,
};
use serde_json::{json, Map, Value};

use crate::types::sanitize_video_task_error_code;
use crate::{
    build_video_follow_up_report_context, current_unix_timestamp_secs, map_openai_task_status,
    parse_video_content_variant, request_body_string, request_body_u32, resolve_follow_up_auth,
    LocalVideoTaskContentAction, LocalVideoTaskFollowUpPlan, LocalVideoTaskReadResponse,
    LocalVideoTaskStatus, OpenAiVideoTaskSeed, VideoFollowUpReportContextInput,
    DEFAULT_VIDEO_TASK_MAX_POLL_COUNT, DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS,
};

fn openai_video_resource_url(api_root: &str, suffix: &str) -> String {
    format!(
        "{}/videos/{}",
        api_root.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    )
}

pub fn map_openai_stored_task_to_read_response(
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
        status => LocalVideoTaskReadResponse {
            status_code: 200,
            body_json: build_openai_stored_task_body(task, status),
        },
    }
}

fn build_openai_stored_task_body(task: StoredVideoTask, status: VideoTaskStatus) -> Value {
    if task.client_api_format.as_deref() == Some("xai:video") {
        let mut body = json!({"status":match status {
            VideoTaskStatus::Completed => "done",
            VideoTaskStatus::Expired => "expired",
            VideoTaskStatus::Failed | VideoTaskStatus::Cancelled | VideoTaskStatus::Deleted => "failed",
            _ => "pending",
        }});
        if let Some(model) = task.model {
            body["model"] = json!(model);
        }
        if let Some(url) = task.video_url {
            body["video"] = json!({"url":url});
            if let Some(duration) = task.duration_seconds {
                body["video"]["duration"] = json!(duration);
            }
        }
        if status == VideoTaskStatus::Failed {
            body["error"] = json!({"code":sanitize_video_task_error_code(task.error_code).unwrap_or_else(|| "unknown".into()),"message":"Video generation failed"});
        }
        return body;
    }
    let mut body = json!({
        "id": task.id,
        "object": "video",
        "status": map_openai_stored_task_status(status),
        "progress": task.progress_percent,
        "created_at": task.created_at_unix_ms,
    });

    if let Some(model) = task.model {
        body["model"] = Value::String(model);
    }
    if let Some(prompt) = task.prompt {
        body["prompt"] = Value::String(prompt);
    }
    if let Some(seconds) = task.duration_seconds {
        body["seconds"] = json!(seconds.to_string());
    }
    if let Some(size) = task.size {
        body["size"] = Value::String(size);
    }
    if let Some(video_url) = task.video_url {
        body["video_url"] = Value::String(video_url);
    }
    if let Some(completed_at) = task.completed_at_unix_secs {
        body["completed_at"] = Value::Number(completed_at.into());
    }
    if matches!(
        status,
        VideoTaskStatus::Failed | VideoTaskStatus::Expired | VideoTaskStatus::Cancelled
    ) {
        body["error"] = json!({
            "code": sanitize_video_task_error_code(task.error_code)
                .unwrap_or_else(|| "unknown".to_string()),
            "message": "Video generation failed",
        });
    }

    body
}

fn map_openai_stored_task_status(status: VideoTaskStatus) -> &'static str {
    match status {
        VideoTaskStatus::Pending | VideoTaskStatus::Submitted | VideoTaskStatus::Queued => "queued",
        VideoTaskStatus::Processing => "processing",
        VideoTaskStatus::Completed => "completed",
        VideoTaskStatus::Failed | VideoTaskStatus::Cancelled | VideoTaskStatus::Expired => "failed",
        VideoTaskStatus::Deleted => "deleted",
    }
}

impl OpenAiVideoTaskSeed {
    pub fn uses_xai_provider(&self) -> bool {
        self.xai_provider || self.is_xai_native()
    }

    pub fn is_xai_native(&self) -> bool {
        self.persistence.client_api_format == "xai:video"
    }

    pub fn native_create_body_json(&self) -> Value {
        let mut body = self.native_response.clone().unwrap_or_else(|| json!({}));
        body["request_id"] = json!(self.local_task_id);
        if body.get("id").is_some() {
            body["id"] = json!(self.local_task_id);
        }
        body
    }

    fn native_read_body_json(&self) -> Value {
        if let Some(mut body) = self.native_response.clone().filter(|body| {
            body.get("status").is_some()
                || body.get("error").is_some()
                || body.get("code").is_some()
        }) {
            if body.get("request_id").is_some() {
                body["request_id"] = json!(self.local_task_id);
            }
            if body.get("id").is_some() {
                body["id"] = json!(self.local_task_id);
            }
            return body;
        }
        let mut body = json!({"status":match self.status {
            LocalVideoTaskStatus::Completed => "done",
            LocalVideoTaskStatus::Expired => "expired",
            LocalVideoTaskStatus::Failed | LocalVideoTaskStatus::Cancelled | LocalVideoTaskStatus::Deleted => "failed",
            _ => "pending",
        }});
        if let Some(model) = &self.model {
            body["model"] = json!(model);
        }
        if let Some(url) = &self.video_url {
            body["video"] = json!({"url":url});
            if let Some(duration) = self.seconds.as_deref().and_then(|v| v.parse::<u64>().ok()) {
                body["video"]["duration"] = json!(duration);
            }
        }
        if self.error_code.is_some() {
            body["error"] = json!({"code":self.error_code,"message":"Video generation failed"});
        }
        body
    }

    pub fn apply_provider_body(&mut self, provider_body: &Map<String, Value>) {
        if self.uses_xai_provider() {
            self.native_response = Some(Value::Object(provider_body.clone()));
        }

        let raw_status = provider_body
            .get("status")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        // Accept xAI's native lifecycle vocabulary alongside OpenAI's fields.
        self.status = match raw_status.to_ascii_lowercase().as_str() {
            "queued" | "pending" => LocalVideoTaskStatus::Queued,
            "processing" | "in_progress" | "running" => LocalVideoTaskStatus::Processing,
            "completed" | "done" | "succeeded" | "success" => LocalVideoTaskStatus::Completed,
            "failed" | "error" => LocalVideoTaskStatus::Failed,
            "cancelled" | "canceled" => LocalVideoTaskStatus::Cancelled,
            "expired" => LocalVideoTaskStatus::Expired,
            _ => LocalVideoTaskStatus::Submitted,
        };
        let error = provider_body.get("error").filter(|value| !value.is_null());
        let error_code = provider_body
            .get("code")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                error
                    .and_then(|value| value.get("code"))
                    .and_then(Value::as_str)
            });
        // xAI may report a failed job as a 200 response with code/error only.
        if (error.is_some() || error_code.is_some())
            && !matches!(
                self.status,
                LocalVideoTaskStatus::Cancelled | LocalVideoTaskStatus::Expired
            )
        {
            self.status = LocalVideoTaskStatus::Failed;
        }
        self.progress_percent = provider_body
            .get("progress")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(match self.status {
                LocalVideoTaskStatus::Completed => 100,
                LocalVideoTaskStatus::Processing => 50,
                _ => self.progress_percent,
            });
        self.completed_at_unix_secs = provider_body.get("completed_at").and_then(Value::as_u64);
        self.expires_at_unix_secs = provider_body.get("expires_at").and_then(Value::as_u64);
        self.error_code = sanitize_video_task_error_code(error_code.map(str::to_string));
        self.error_message = None;
        self.video_url = provider_body
            .get("video_url")
            .or_else(|| provider_body.get("url"))
            .or_else(|| provider_body.get("result_url"))
            .or_else(|| {
                provider_body
                    .get("video")
                    .and_then(|video| video.get("url"))
            })
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(seconds) = provider_body
            .get("seconds")
            .or_else(|| {
                provider_body
                    .get("video")
                    .and_then(|video| video.get("duration"))
            })
            .filter(|value| value.is_string() || value.is_number())
        {
            self.seconds = Some(
                seconds
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| seconds.to_string()),
            );
        }
    }

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
                        "detail": format!(
                            "Video is still processing (status: {})",
                            map_openai_task_status(self.status)
                        )
                    }),
                });
            }
            LocalVideoTaskStatus::Failed | LocalVideoTaskStatus::Expired => {
                return Some(LocalVideoTaskContentAction::Immediate {
                    status_code: 422,
                    body_json: json!({"detail": "Video generation failed"}),
                });
            }
            LocalVideoTaskStatus::Cancelled => {
                return Some(LocalVideoTaskContentAction::Immediate {
                    status_code: 404,
                    body_json: json!({"detail": "Video task was cancelled"}),
                });
            }
            LocalVideoTaskStatus::Deleted => {
                return Some(LocalVideoTaskContentAction::Immediate {
                    status_code: 404,
                    body_json: json!({"detail": "Video task not found"}),
                });
            }
            LocalVideoTaskStatus::Completed => {}
        }

        let variant = parse_video_content_variant(query_string)?;
        let (url, headers) = if variant == "video" {
            if let Some(video_url) = self
                .video_url
                .clone()
                .filter(|value| value.starts_with("http://") || value.starts_with("https://"))
            {
                (video_url, BTreeMap::new())
            } else {
                let mut headers = self.transport.headers.clone();
                headers.remove("content-type");
                headers.remove("content-length");
                (
                    openai_video_resource_url(
                        &self.transport.upstream_base_url,
                        format!("{}/content", self.upstream_task_id).as_str(),
                    ),
                    headers,
                )
            }
        } else {
            let mut headers = self.transport.headers.clone();
            headers.remove("content-type");
            headers.remove("content-length");
            (
                openai_video_resource_url(
                    &self.transport.upstream_base_url,
                    format!("{}/content?variant={variant}", self.upstream_task_id).as_str(),
                ),
                headers,
            )
        };

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
                headers,
                content_type: None,
                content_encoding: None,
                body: RequestBody {
                    json_body: None,
                    body_bytes_b64: None,
                    body_ref: None,
                },
                stream: true,
                client_api_format: "openai:video".to_string(),
                provider_api_format: "openai:video".to_string(),
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

    pub fn client_body_json(&self) -> Value {
        if self.is_xai_native() {
            return self.native_read_body_json();
        }
        let mut body = json!({
            "id": self.local_task_id,
            "object": "video",
            "status": map_openai_task_status(self.status),
            "progress": self.progress_percent,
            "created_at": self.created_at_unix_ms,
        });

        if let Some(model) = &self.model {
            body["model"] = Value::String(model.clone());
        }
        if let Some(prompt) = &self.prompt {
            body["prompt"] = Value::String(prompt.clone());
        }
        if let Some(size) = &self.size {
            body["size"] = Value::String(size.clone());
        }
        if let Some(seconds) = &self.seconds {
            body["seconds"] = Value::String(seconds.clone());
        }
        if let Some(video_url) = &self.video_url {
            body["video_url"] = Value::String(video_url.clone());
        }
        if let Some(remixed_from_video_id) = &self.remixed_from_video_id {
            body["remixed_from_video_id"] = Value::String(remixed_from_video_id.clone());
        }
        if let Some(completed_at) = self.completed_at_unix_secs {
            body["completed_at"] = Value::Number(completed_at.into());
        }
        if let Some(expires_at) = self.expires_at_unix_secs {
            body["expires_at"] = Value::Number(expires_at.into());
        }
        if self.status == LocalVideoTaskStatus::Failed
            || self.status == LocalVideoTaskStatus::Expired
        {
            body["error"] = json!({
                "code": sanitize_video_task_error_code(self.error_code.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                "message": "Video generation failed",
            });
        }

        body
    }

    pub fn build_delete_follow_up_plan(
        &self,
        fallback_user_id: Option<&str>,
        fallback_api_key_id: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskFollowUpPlan> {
        if !matches!(
            self.status,
            LocalVideoTaskStatus::Completed | LocalVideoTaskStatus::Failed
        ) {
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
                url: openai_video_resource_url(
                    &self.transport.upstream_base_url,
                    &self.upstream_task_id,
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
                client_api_format: "openai:video".to_string(),
                provider_api_format: "openai:video".to_string(),
                model_name: model_name.clone(),
                proxy: self.transport.proxy.clone(),
                transport_profile: self.transport.transport_profile.clone(),
                timeouts: self.transport.timeouts.clone(),
            },
            report_kind: Some("openai_video_delete_sync_finalize".to_string()),
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
                    client_api_format: "openai:video",
                    provider_api_format: "openai:video",
                },
            )),
        })
    }

    pub fn build_get_follow_up_plan(&self, trace_id: &str) -> Option<ExecutionPlan> {
        let refreshable = matches!(
            self.status,
            LocalVideoTaskStatus::Submitted
                | LocalVideoTaskStatus::Queued
                | LocalVideoTaskStatus::Processing
        ) || (self.uses_xai_provider()
            && self.native_response.is_none()
            && matches!(
                self.status,
                LocalVideoTaskStatus::Completed
                    | LocalVideoTaskStatus::Failed
                    | LocalVideoTaskStatus::Expired
            ));
        if !refreshable {
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
            url: openai_video_resource_url(
                &self.transport.upstream_base_url,
                &self.upstream_task_id,
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
            client_api_format: "openai:video".to_string(),
            provider_api_format: "openai:video".to_string(),
            model_name: self
                .model
                .clone()
                .or_else(|| self.transport.model_name.clone()),
            proxy: self.transport.proxy.clone(),
            transport_profile: self.transport.transport_profile.clone(),
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
                url: openai_video_resource_url(
                    &self.transport.upstream_base_url,
                    &self.upstream_task_id,
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
                client_api_format: "openai:video".to_string(),
                provider_api_format: "openai:video".to_string(),
                model_name: model_name.clone(),
                proxy: self.transport.proxy.clone(),
                transport_profile: self.transport.transport_profile.clone(),
                timeouts: self.transport.timeouts.clone(),
            },
            report_kind: Some("openai_video_cancel_sync_finalize".to_string()),
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
                    client_api_format: "openai:video",
                    provider_api_format: "openai:video",
                },
            )),
        })
    }

    pub fn build_remix_follow_up_plan(
        &self,
        body_json: &Value,
        fallback_user_id: Option<&str>,
        fallback_api_key_id: Option<&str>,
        trace_id: &str,
    ) -> Option<LocalVideoTaskFollowUpPlan> {
        if !matches!(self.status, LocalVideoTaskStatus::Completed) || body_json.is_null() {
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
        headers.remove("content-length");
        let content_type = self
            .transport
            .content_type
            .clone()
            .unwrap_or_else(|| "application/json".to_string());
        headers
            .entry("content-type".to_string())
            .or_insert_with(|| content_type.clone());

        let mut report_context =
            build_video_follow_up_report_context(VideoFollowUpReportContextInput {
                request_id: &self.persistence.request_id,
                user_id: &user_id,
                api_key_id: &api_key_id,
                task_id: &self.local_task_id,
                provider_id: &self.transport.provider_id,
                endpoint_id: &self.transport.endpoint_id,
                key_id: &self.transport.key_id,
                provider_name: self.transport.provider_name.as_deref(),
                model_name: model_name.as_deref(),
                client_api_format: "openai:video",
                provider_api_format: "openai:video",
            });
        if let Some(report_context_object) = report_context.as_object_mut() {
            report_context_object.insert("original_request_body".to_string(), body_json.clone());
        }

        Some(LocalVideoTaskFollowUpPlan {
            plan: ExecutionPlan {
                request_id: trace_id.to_string(),
                candidate_id: None,
                provider_name: self.transport.provider_name.clone(),
                provider_id: self.transport.provider_id.clone(),
                endpoint_id: self.transport.endpoint_id.clone(),
                key_id: self.transport.key_id.clone(),
                method: "POST".to_string(),
                url: openai_video_resource_url(
                    &self.transport.upstream_base_url,
                    format!("{}/remix", self.upstream_task_id).as_str(),
                ),
                headers,
                content_type: Some(content_type),
                content_encoding: None,
                body: RequestBody::from_json(body_json.clone()),
                stream: false,
                client_api_format: "openai:video".to_string(),
                provider_api_format: "openai:video".to_string(),
                model_name,
                proxy: self.transport.proxy.clone(),
                transport_profile: self.transport.transport_profile.clone(),
                timeouts: self.transport.timeouts.clone(),
            },
            report_kind: Some("openai_video_remix_sync_finalize".to_string()),
            report_context: Some(report_context),
        })
    }

    pub fn to_upsert_record(&self) -> UpsertVideoTask {
        let now_unix_secs = current_unix_timestamp_secs();
        let next_poll_at_unix_secs = match self.status {
            LocalVideoTaskStatus::Submitted
            | LocalVideoTaskStatus::Queued
            | LocalVideoTaskStatus::Processing => Some(
                self.created_at_unix_ms
                    .saturating_add(u64::from(DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS)),
            ),
            _ => None,
        };
        let mut record = UpsertVideoTask {
            id: self.local_task_id.clone(),
            // The production schema requires a unique, non-null short_id (at most 16 chars).
            // Derive it deterministically so repeated capture and legacy snapshot reloads agree.
            short_id: Some(self.local_short_id.clone().unwrap_or_else(|| {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(self.local_task_id.as_bytes()))[..16].to_string()
            })),
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
            original_request_body: None,
            duration_seconds: self
                .seconds
                .as_deref()
                .and_then(|value| value.parse().ok())
                .or_else(|| request_body_u32(&self.persistence.original_request_body, "seconds")),
            resolution: request_body_string(&self.persistence.original_request_body, "resolution"),
            aspect_ratio: request_body_string(
                &self.persistence.original_request_body,
                "aspect_ratio",
            ),
            size: self.size.clone(),
            status: self.status.as_database_status(),
            progress_percent: self.progress_percent,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: DEFAULT_VIDEO_TASK_POLL_INTERVAL_SECONDS,
            next_poll_at_unix_secs,
            poll_count: 0,
            max_poll_count: DEFAULT_VIDEO_TASK_MAX_POLL_COUNT,
            created_at_unix_ms: self.created_at_unix_ms,
            submitted_at_unix_secs: Some(self.created_at_unix_ms),
            completed_at_unix_secs: self.completed_at_unix_secs,
            updated_at_unix_secs: self.completed_at_unix_secs.unwrap_or(now_unix_secs),
            error_code: self.error_code.clone(),
            error_message: None,
            video_url: self.video_url.clone(),
            request_metadata: None,
        };
        record.sanitize_for_persistence();
        record
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_data_contracts::repository::video_tasks::{StoredVideoTask, VideoTaskStatus};
    use serde_json::json;

    use crate::{
        LocalVideoTaskContentAction, LocalVideoTaskPersistence, LocalVideoTaskSnapshot,
        LocalVideoTaskStatus, LocalVideoTaskTransport, OpenAiVideoTaskSeed,
    };

    use super::map_openai_stored_task_to_read_response;

    fn sample_stored_task(status: VideoTaskStatus) -> StoredVideoTask {
        StoredVideoTask {
            id: "task-openai-123".to_string(),
            short_id: None,
            request_id: "req-openai-123".to_string(),
            user_id: None,
            api_key_id: None,
            username: None,
            api_key_name: None,
            external_task_id: Some("ext-openai-123".to_string()),
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("hello".to_string()),
            original_request_body: None,
            duration_seconds: None,
            resolution: None,
            aspect_ratio: None,
            size: Some("1280x720".to_string()),
            status,
            progress_percent: 100,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: 1712345678,
            submitted_at_unix_secs: Some(1712345678),
            completed_at_unix_secs: Some(1712345688),
            updated_at_unix_secs: 1712345688,
            error_code: Some("upstream_failed".to_string()),
            error_message: Some("provider failed".to_string()),
            video_url: Some("https://cdn.example.com/video.mp4".to_string()),
            request_metadata: None,
        }
    }

    #[test]
    fn maps_openai_failed_stored_task_into_read_response() {
        let response =
            map_openai_stored_task_to_read_response(sample_stored_task(VideoTaskStatus::Failed));

        assert_eq!(response.status_code, 200);
        assert_eq!(response.body_json["id"], "task-openai-123");
        assert_eq!(response.body_json["status"], "failed");
        assert_eq!(response.body_json["completed_at"], 1712345688u64);
        assert_eq!(response.body_json["error"]["code"], "provider_error");
        assert_eq!(
            response.body_json["error"]["message"],
            "Video generation failed"
        );
        assert_eq!(
            response.body_json["video_url"],
            "https://cdn.example.com/video.mp4"
        );
    }

    #[test]
    fn builds_minimal_openai_persistence_record_without_sensitive_snapshot() {
        let seed = OpenAiVideoTaskSeed {
            local_short_id: None,
            native_response: None,
            xai_provider: false,
            local_task_id: "task-openai-sensitive".to_string(),
            upstream_task_id: "upstream-openai-sensitive".to_string(),
            created_at_unix_ms: 1_712_345_678,
            user_id: Some("user-1".to_string()),
            api_key_id: Some("api-key-1".to_string()),
            model: Some("sora-2".to_string()),
            prompt: Some("business prompt".to_string()),
            size: Some("1280x720".to_string()),
            seconds: Some("4".to_string()),
            remixed_from_video_id: None,
            status: LocalVideoTaskStatus::Failed,
            progress_percent: 100,
            completed_at_unix_secs: Some(1_712_345_700),
            expires_at_unix_secs: None,
            error_code: Some("provider-secret-diagnostic".to_string()),
            error_message: Some("Bearer sk-sensitive-provider-error".to_string()),
            video_url: Some("https://cdn.example/video.mp4?token=sensitive".to_string()),
            persistence: LocalVideoTaskPersistence {
                request_id: "request-openai-sensitive".to_string(),
                username: Some("alice".to_string()),
                api_key_name: Some("primary".to_string()),
                client_api_format: "openai:video".to_string(),
                provider_api_format: "openai:video".to_string(),
                original_request_body: json!({
                    "prompt": "business prompt",
                    "seconds": "4",
                    "provider_token": "sk-sensitive"
                }),
                format_converted: false,
            },
            transport: LocalVideoTaskTransport {
                upstream_base_url: "https://api.example/v1".to_string(),
                provider_name: Some("openai".to_string()),
                provider_id: "provider-1".to_string(),
                endpoint_id: "endpoint-1".to_string(),
                key_id: "provider-key-1".to_string(),
                headers: BTreeMap::from([(
                    "authorization".to_string(),
                    "Bearer sk-sensitive".to_string(),
                )]),
                content_type: Some("application/json".to_string()),
                model_name: Some("sora-2".to_string()),
                proxy: None,
                transport_profile: None,
                timeouts: None,
            },
        };

        let record = seed.to_upsert_record();

        let short_id = record
            .short_id
            .as_deref()
            .expect("database short_id is required");
        assert_eq!(short_id.len(), 16);
        assert_eq!(seed.to_upsert_record().short_id, record.short_id);
        assert_eq!(record.error_code.as_deref(), Some("provider_error"));
        assert!(record.original_request_body.is_none());
        assert!(record.progress_message.is_none());
        assert!(record.error_message.is_none());
        assert_eq!(record.video_url, seed.video_url);
        assert_eq!(record.prompt, seed.prompt);
        assert!(record.request_metadata.is_none());
        assert_eq!(record.duration_seconds, Some(4));
        assert_eq!(record.size.as_deref(), Some("1280x720"));

        let mut stored = record.into_stored();
        stored.status = VideoTaskStatus::Completed;
        // Migrated tasks can already have a short ID unrelated to the derived ID.
        stored.short_id = Some("legacy-short-id".to_string());
        let snapshot =
            LocalVideoTaskSnapshot::from_stored_task_with_transport(&stored, seed.transport)
                .expect("stored task should reconstruct with current transport");
        let LocalVideoTaskSnapshot::OpenAi(restored) = snapshot else {
            panic!("expected OpenAI snapshot");
        };
        assert_eq!(restored.prompt, stored.prompt);
        assert_eq!(restored.to_upsert_record().short_id, stored.short_id);
        let mut embedded = stored.clone();
        let mut legacy_snapshot =
            serde_json::to_value(LocalVideoTaskSnapshot::OpenAi(restored.clone())).unwrap();
        legacy_snapshot["OpenAi"]
            .as_object_mut()
            .unwrap()
            .remove("local_short_id");
        embedded.request_metadata = Some(json!({"rust_local_snapshot": legacy_snapshot}));
        let embedded_snapshot = LocalVideoTaskSnapshot::from_stored_task(&embedded)
            .expect("legacy embedded snapshot should hydrate");
        assert_eq!(
            embedded_snapshot.to_upsert_record().short_id,
            stored.short_id
        );
        assert_eq!(restored.to_upsert_record().video_url, stored.video_url);
        let Some(LocalVideoTaskContentAction::StreamPlan(plan)) =
            restored.build_content_stream_action(None, "trace-download")
        else {
            panic!("completed stored task should stream content");
        };
        assert_eq!(Some(plan.url.as_str()), stored.video_url.as_deref());
        assert!(plan.headers.is_empty());
        let response = map_openai_stored_task_to_read_response(stored.clone());
        assert_eq!(
            response.body_json["video_url"].as_str(),
            stored.video_url.as_deref()
        );
    }
}
