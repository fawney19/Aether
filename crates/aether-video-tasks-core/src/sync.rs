use aether_contracts::ExecutionPlan;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{
    context_text, context_u64, current_unix_timestamp_secs, generate_local_short_id,
    request_body_text, GeminiVideoTaskSeed, LocalVideoTaskPersistence, LocalVideoTaskReadResponse,
    LocalVideoTaskSeed, LocalVideoTaskSnapshot, LocalVideoTaskStatus, LocalVideoTaskSuccessPlan,
    LocalVideoTaskTransport, OpenAiVideoTaskSeed, VideoTaskSyncReportMode,
    VideoTaskTruthSourceMode,
};

impl LocalVideoTaskSeed {
    pub fn from_sync_finalize(
        report_kind: &str,
        provider_body: &Map<String, Value>,
        report_context: &Map<String, Value>,
        plan: &ExecutionPlan,
    ) -> Option<Self> {
        let transport = LocalVideoTaskTransport::from_plan(plan)?;
        let persistence = LocalVideoTaskPersistence::from_report_context(report_context, plan);
        let mut seed = match report_kind {
            "openai_video_create_sync_finalize" => {
                let upstream_id = openai_video_provider_task_id(provider_body)?;

                Some(Self::OpenAiCreate(OpenAiVideoTaskSeed {
                    local_short_id: None,
                    native_response: None,
                    xai_provider: report_context
                        .get("video_provider_xai")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    local_task_id: context_text(report_context, "local_task_id")
                        .unwrap_or_else(|| Uuid::new_v4().to_string()),
                    upstream_task_id: upstream_id.to_string(),
                    created_at_unix_ms: context_u64(report_context, "local_created_at")
                        .unwrap_or_else(current_unix_timestamp_secs),
                    user_id: context_text(report_context, "user_id"),
                    api_key_id: context_text(report_context, "api_key_id"),
                    model: context_text(report_context, "model")
                        .or_else(|| request_body_text(report_context, "model")),
                    prompt: request_body_text(report_context, "prompt"),
                    size: context_text(report_context, "video_size")
                        .or_else(|| request_body_text(report_context, "size")),
                    seconds: context_u64(report_context, "video_duration")
                        .map(|v| v.to_string())
                        .or_else(|| request_body_text(report_context, "seconds"))
                        .or_else(|| request_body_text(report_context, "duration")),
                    remixed_from_video_id: None,
                    status: LocalVideoTaskStatus::Submitted,
                    progress_percent: 0,
                    completed_at_unix_secs: None,
                    expires_at_unix_secs: None,
                    error_code: None,
                    error_message: None,
                    video_url: None,
                    persistence: persistence.clone(),
                    transport: transport.clone(),
                }))
            }
            "openai_video_remix_sync_finalize" => {
                let upstream_id = openai_video_provider_task_id(provider_body)?;

                Some(Self::OpenAiRemix(OpenAiVideoTaskSeed {
                    local_short_id: None,
                    native_response: None,
                    xai_provider: report_context
                        .get("video_provider_xai")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    local_task_id: context_text(report_context, "local_task_id")
                        .unwrap_or_else(|| Uuid::new_v4().to_string()),
                    upstream_task_id: upstream_id.to_string(),
                    created_at_unix_ms: context_u64(report_context, "local_created_at")
                        .unwrap_or_else(current_unix_timestamp_secs),
                    user_id: context_text(report_context, "user_id"),
                    api_key_id: context_text(report_context, "api_key_id"),
                    model: context_text(report_context, "model")
                        .or_else(|| request_body_text(report_context, "model")),
                    prompt: request_body_text(report_context, "prompt"),
                    size: context_text(report_context, "video_size")
                        .or_else(|| request_body_text(report_context, "size")),
                    seconds: context_u64(report_context, "video_duration")
                        .map(|v| v.to_string())
                        .or_else(|| request_body_text(report_context, "seconds"))
                        .or_else(|| request_body_text(report_context, "duration")),
                    remixed_from_video_id: context_text(report_context, "task_id")
                        .or_else(|| request_body_text(report_context, "remix_video_id")),
                    status: LocalVideoTaskStatus::Submitted,
                    progress_percent: 0,
                    completed_at_unix_secs: None,
                    expires_at_unix_secs: None,
                    error_code: None,
                    error_message: None,
                    video_url: None,
                    persistence: persistence.clone(),
                    transport: transport.clone(),
                }))
            }
            "gemini_video_create_sync_finalize" => {
                let operation_name = provider_body
                    .get("name")
                    .or_else(|| provider_body.get("id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())?;

                Some(Self::GeminiCreate(GeminiVideoTaskSeed {
                    local_short_id: context_text(report_context, "local_short_id")
                        .unwrap_or_else(generate_local_short_id),
                    upstream_operation_name: operation_name.to_string(),
                    user_id: context_text(report_context, "user_id"),
                    api_key_id: context_text(report_context, "api_key_id"),
                    model: context_text(report_context, "model")
                        .or_else(|| context_text(report_context, "model_name"))
                        .unwrap_or_else(|| "unknown".to_string()),
                    status: LocalVideoTaskStatus::Submitted,
                    progress_percent: 0,
                    error_code: None,
                    error_message: None,
                    metadata: json!({}),
                    persistence,
                    transport,
                }))
            }
            _ => None,
        }?;
        if let Self::OpenAiCreate(task) | Self::OpenAiRemix(task) = &mut seed {
            task.apply_provider_body(provider_body);
        }
        Some(seed)
    }

    pub fn success_report_kind(&self) -> &'static str {
        match self {
            Self::OpenAiCreate(_) => "openai_video_create_sync_success",
            Self::OpenAiRemix(_) => "openai_video_remix_sync_success",
            Self::GeminiCreate(_) => "gemini_video_create_sync_success",
        }
    }

    pub fn apply_to_report_context(&self, report_context: &mut Map<String, Value>) {
        match self {
            Self::OpenAiCreate(seed) | Self::OpenAiRemix(seed) => {
                report_context.insert(
                    "local_task_id".to_string(),
                    Value::String(seed.local_task_id.clone()),
                );
                report_context.insert(
                    "local_created_at".to_string(),
                    Value::Number(seed.created_at_unix_ms.into()),
                );
            }
            Self::GeminiCreate(seed) => {
                report_context.insert(
                    "local_short_id".to_string(),
                    Value::String(seed.local_short_id.clone()),
                );
            }
        }
    }

    pub fn client_body_json(&self) -> Value {
        match self {
            Self::OpenAiCreate(seed) | Self::OpenAiRemix(seed) => {
                if seed.is_xai_native() {
                    seed.native_create_body_json()
                } else {
                    seed.client_body_json()
                }
            }
            Self::GeminiCreate(seed) => seed.client_body_json(),
        }
    }
}

fn openai_video_provider_task_id(body: &Map<String, Value>) -> Option<&str> {
    // xAI's OpenAI-compatible video creation returns request_id instead of id.
    ["id", "request_id"].into_iter().find_map(|field| {
        body.get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

impl VideoTaskTruthSourceMode {
    pub fn prepare_sync_success(
        self,
        report_kind: &str,
        provider_body: &Map<String, Value>,
        report_context: &Map<String, Value>,
        plan: &ExecutionPlan,
    ) -> Option<LocalVideoTaskSuccessPlan> {
        let seed = LocalVideoTaskSeed::from_sync_finalize(
            report_kind,
            provider_body,
            report_context,
            plan,
        )?;
        let report_mode = match self {
            Self::PythonSyncReport => VideoTaskSyncReportMode::InlineSync,
            Self::RustAuthoritative => VideoTaskSyncReportMode::Background,
        };
        Some(LocalVideoTaskSuccessPlan { seed, report_mode })
    }
}

impl LocalVideoTaskSuccessPlan {
    pub fn success_report_kind(&self) -> &'static str {
        self.seed.success_report_kind()
    }

    pub fn report_mode(&self) -> VideoTaskSyncReportMode {
        self.report_mode
    }

    pub fn apply_to_report_context(&self, report_context: &mut Map<String, Value>) {
        self.seed.apply_to_report_context(report_context);
        if matches!(self.report_mode, VideoTaskSyncReportMode::Background) {
            report_context.insert("rust_video_task_persisted".to_string(), Value::Bool(true));
        }
    }

    pub fn client_body_json(&self) -> Value {
        self.seed.client_body_json()
    }

    pub fn to_snapshot(&self) -> LocalVideoTaskSnapshot {
        match &self.seed {
            LocalVideoTaskSeed::OpenAiCreate(seed) | LocalVideoTaskSeed::OpenAiRemix(seed) => {
                LocalVideoTaskSnapshot::OpenAi(seed.clone())
            }
            LocalVideoTaskSeed::GeminiCreate(seed) => LocalVideoTaskSnapshot::Gemini(seed.clone()),
        }
    }
}

pub fn build_internal_finalize_video_plan(
    trace_id: &str,
    signature: &str,
    report_context: Option<&Value>,
) -> Option<ExecutionPlan> {
    if !matches!(signature, "openai:video" | "gemini:video") {
        return None;
    }

    let report_context = report_context.and_then(Value::as_object);
    let context_text = |key: &str| {
        report_context
            .and_then(|value| value.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };

    let provider_name = signature
        .split(':')
        .next()
        .expect("video finalize signature should include provider name")
        .to_string();
    let model_name = context_text("model")
        .or_else(|| context_text("model_name"))
        .or_else(|| match signature {
            "openai:video" => Some("sora-2".to_string()),
            "gemini:video" => Some("veo-3".to_string()),
            _ => None,
        });
    let original_request_body = report_context
        .and_then(|value| value.get("original_request_body"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let url = match signature {
        "openai:video" => "https://internal.gateway.invalid/v1/videos".to_string(),
        "gemini:video" => format!(
            "https://internal.gateway.invalid/v1beta/models/{}:predictLongRunning",
            model_name.clone().unwrap_or_else(|| "veo-3".to_string())
        ),
        _ => return None,
    };

    Some(ExecutionPlan {
        request_id: context_text("request_id").unwrap_or_else(|| trace_id.to_string()),
        candidate_id: None,
        provider_name: Some(provider_name.clone()),
        provider_id: context_text("provider_id")
            .unwrap_or_else(|| format!("internal-{provider_name}-video-provider")),
        endpoint_id: context_text("endpoint_id")
            .unwrap_or_else(|| format!("internal-{provider_name}-video-endpoint")),
        key_id: context_text("key_id")
            .or_else(|| context_text("api_key_id"))
            .unwrap_or_else(|| format!("internal-{provider_name}-video-key")),
        method: "POST".to_string(),
        url,
        headers: std::collections::BTreeMap::from([(
            "authorization".to_string(),
            "Bearer internal-gateway".to_string(),
        )]),
        content_type: Some("application/json".to_string()),
        content_encoding: None,
        body: aether_contracts::RequestBody::from_json(original_request_body),
        stream: false,
        client_api_format: signature.to_string(),
        provider_api_format: signature.to_string(),
        model_name,
        proxy: Some(aether_contracts::ProxySnapshot {
            enabled: Some(false),
            mode: Some("direct".to_string()),
            node_id: None,
            label: None,
            url: None,
            extra: None,
        }),
        transport_profile: None,
        timeouts: None,
    })
}

pub fn build_local_sync_finalize_read_response(
    report_kind: &str,
    upstream_status_code: u16,
    report_context: Option<&Value>,
) -> Option<LocalVideoTaskReadResponse> {
    match report_kind {
        "openai_video_delete_sync_finalize" => {
            if upstream_status_code >= 400 && upstream_status_code != 404 {
                return None;
            }
            let task_id = report_context
                .and_then(|value| value.get("task_id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(LocalVideoTaskReadResponse {
                status_code: 200,
                body_json: json!({
                    "id": task_id,
                    "object": "video",
                    "deleted": true,
                }),
            })
        }
        "openai_video_cancel_sync_finalize" | "gemini_video_cancel_sync_finalize" => {
            if upstream_status_code >= 400 {
                return None;
            }
            Some(LocalVideoTaskReadResponse {
                status_code: 200,
                body_json: json!({}),
            })
        }
        _ => None,
    }
}

pub fn resolve_local_sync_success_background_report_kind(
    report_kind: &str,
) -> Option<&'static str> {
    match report_kind {
        "openai_video_delete_sync_finalize" => Some("openai_video_delete_sync_success"),
        "openai_video_cancel_sync_finalize" => Some("openai_video_cancel_sync_success"),
        "gemini_video_cancel_sync_finalize" => Some("gemini_video_cancel_sync_success"),
        _ => None,
    }
}

pub fn resolve_local_sync_error_background_report_kind(report_kind: &str) -> Option<&'static str> {
    match report_kind {
        "openai_video_create_sync_finalize" => Some("openai_video_create_sync_error"),
        "openai_video_remix_sync_finalize" => Some("openai_video_remix_sync_error"),
        "gemini_video_create_sync_finalize" => Some("gemini_video_create_sync_error"),
        "openai_video_delete_sync_finalize" => Some("openai_video_delete_sync_error"),
        "openai_video_cancel_sync_finalize" => Some("openai_video_cancel_sync_error"),
        "gemini_video_cancel_sync_finalize" => Some("gemini_video_cancel_sync_error"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        build_internal_finalize_video_plan, build_local_sync_finalize_read_response,
        resolve_local_sync_error_background_report_kind,
        resolve_local_sync_success_background_report_kind,
    };

    #[test]
    fn xai_native_video_protocol_survives_persistence_and_preserves_provider_fields() {
        use crate::{
            LocalVideoTaskContentAction, LocalVideoTaskSnapshot, VideoTaskService,
            VideoTaskTruthSourceMode,
        };
        let mut plan =
            build_internal_finalize_video_plan("native-create", "openai:video", None).unwrap();
        plan.url = "https://api.x.ai/v1/videos/generations".into();
        plan.headers
            .insert("authorization".into(), "Bearer test-key".into());
        let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
        let context = json!({"local_task_id":"native-local", "user_id":"owner", "model":"grok-imagine-video", "video_client_protocol":"xai", "video_duration":6});
        let success = service
            .prepare_sync_success(
                "openai_video_create_sync_finalize",
                json!({"request_id":"native-upstream", "future_field":true})
                    .as_object()
                    .unwrap(),
                context.as_object().unwrap(),
                &plan,
            )
            .unwrap();
        assert_eq!(
            success.client_body_json(),
            json!({"request_id":"native-local","future_field":true})
        );
        let mut snapshot = success.to_snapshot();
        let body = json!({"status":"done","video":{"url":"https://vidgen.x.ai/video.mp4","duration":6,"respect_moderation":true},"future_field":[1,2]});
        snapshot.apply_provider_body(body.as_object().unwrap());
        assert_eq!(snapshot.read_response().body_json, body);
        assert_eq!(
            snapshot
                .read_response_for_path("/openai/v1/videos/native-local")
                .body_json["status"],
            "completed"
        );
        let LocalVideoTaskSnapshot::OpenAi(seed) = &snapshot else {
            panic!("openai task expected")
        };
        let Some(LocalVideoTaskContentAction::StreamPlan(download)) =
            seed.build_content_stream_action(None, "download")
        else {
            panic!("download expected")
        };
        assert_eq!(download.url, "https://vidgen.x.ai/video.mp4");
        assert!(download.headers.is_empty());
        let stored = snapshot.to_upsert_record().into_stored();
        assert!(stored.request_metadata.is_none());
        assert_eq!(stored.client_api_format.as_deref(), Some("xai:video"));
        let restored = LocalVideoTaskSnapshot::from_stored_task_with_transport(
            &stored,
            seed.transport.clone(),
        )
        .unwrap();
        assert_eq!(restored.read_response().body_json["status"], "done");
        service.record_snapshot(restored);
        let poll = service
            .prepare_read_refresh_sync_plan_for_user(
                Some("openai"),
                "/v1/videos/native-local",
                "owner",
                "poll",
            )
            .unwrap();
        assert_eq!(poll.plan.url, "https://api.x.ai/v1/videos/native-upstream");
        assert!(service
            .prepare_read_refresh_sync_plan_for_user(
                Some("openai"),
                "/v1/videos/native-local",
                "foreign",
                "poll"
            )
            .is_none());
        assert!(service.apply_read_refresh_projection(&poll, body.as_object().unwrap()));
        assert_eq!(
            service
                .read_response_for_user(Some("openai"), "/v1/videos/native-local", "owner")
                .unwrap()
                .body_json,
            body
        );
    }

    #[test]
    fn xai_video_lifecycle_creates_polls_persists_and_downloads() {
        use crate::{
            LocalVideoTaskContentAction, LocalVideoTaskSnapshot, VideoTaskService,
            VideoTaskTruthSourceMode,
        };
        for api_root in ["https://cli-chat-proxy.grok.com/v1", "https://api.x.ai/v1"] {
            let mut plan =
                build_internal_finalize_video_plan("xai-create", "openai:video", None).unwrap();
            plan.url = format!("{api_root}/videos/generations");
            plan.headers
                .insert("authorization".into(), "Bearer test-token".into());
            let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
            let context = json!({"local_task_id": "local-video", "model": "grok-imagine-video", "original_request_body": {"prompt": "A cat", "seconds": "6"}});
            let success = service
                .prepare_sync_success(
                    "openai_video_create_sync_finalize",
                    json!({"request_id": "xai-request"}).as_object().unwrap(),
                    context.as_object().unwrap(),
                    &plan,
                )
                .unwrap();
            assert_eq!(success.client_body_json()["id"], "local-video");
            assert_eq!(success.client_body_json()["status"], "queued");
            let snapshot = success.to_snapshot();
            assert_eq!(
                snapshot.to_upsert_record().external_task_id.as_deref(),
                Some("xai-request")
            );
            service.record_snapshot(snapshot.clone());
            let poll = service
                .prepare_poll_refresh_plan_for_snapshot(snapshot, "xai-poll")
                .unwrap();
            assert_eq!(poll.plan.method, "GET");
            assert_eq!(poll.plan.url, format!("{api_root}/videos/xai-request"));
            assert_eq!(
                poll.plan.headers.get("authorization"),
                plan.headers.get("authorization")
            );
            assert!(service.apply_read_refresh_projection(
                &poll,
                json!({"status": "pending"}).as_object().unwrap()
            ));
            assert_eq!(
                service
                    .read_response(Some("openai"), "/v1/videos/local-video")
                    .unwrap()
                    .body_json["status"],
                "queued"
            );
            assert!(service.apply_read_refresh_projection(&poll, json!({
                "status": "done", "video": {"url": "https://vidgen.x.ai/result.mp4", "duration": 6}
            }).as_object().unwrap()));
            let snapshot = service
                .snapshot_for_route(Some("openai"), "/v1/videos/local-video")
                .unwrap();
            assert!(!snapshot.is_active_for_refresh());
            let record = snapshot.to_upsert_record();
            assert_eq!(
                record.status,
                aether_data_contracts::repository::video_tasks::VideoTaskStatus::Completed
            );
            assert_eq!(
                record.video_url.as_deref(),
                Some("https://vidgen.x.ai/result.mp4")
            );
            assert_eq!(record.duration_seconds, Some(6));
            let response = snapshot.read_response();
            assert_eq!(response.body_json["status"], "completed");
            assert_eq!(response.body_json["progress"], 100);
            assert_eq!(
                response.body_json["video_url"],
                "https://vidgen.x.ai/result.mp4"
            );
            let LocalVideoTaskSnapshot::OpenAi(seed) = snapshot else {
                panic!("OpenAI video expected")
            };
            let Some(LocalVideoTaskContentAction::StreamPlan(download)) =
                seed.build_content_stream_action(None, "download")
            else {
                panic!("download expected")
            };
            assert_eq!(download.url, "https://vidgen.x.ai/result.mp4");
            assert!(
                download.headers.is_empty(),
                "provider credentials must not be sent to the media CDN"
            );
        }
    }

    #[test]
    fn xai_video_errors_are_terminal_even_without_a_status() {
        use crate::{LocalVideoTaskSnapshot, VideoTaskTruthSourceMode};
        let mut plan =
            build_internal_finalize_video_plan("xai-create", "openai:video", None).unwrap();
        plan.url = "https://cli-chat-proxy.grok.com/v1/videos/generations".into();
        for body in [
            json!({"code": "content_policy_violation", "error": "Rejected"}),
            json!({"error": {"code": "content_policy_violation", "message": "Rejected"}}),
            json!({"status": "failed", "error": "Rejected"}),
        ] {
            let mut snapshot = VideoTaskTruthSourceMode::RustAuthoritative
                .prepare_sync_success(
                    "openai_video_create_sync_finalize",
                    json!({"request_id": "xai-request"}).as_object().unwrap(),
                    &Default::default(),
                    &plan,
                )
                .unwrap()
                .to_snapshot();
            snapshot.apply_provider_body(body.as_object().unwrap());
            assert!(!snapshot.is_active_for_refresh());
            assert_eq!(snapshot.read_response().body_json["status"], "failed");
            let LocalVideoTaskSnapshot::OpenAi(seed) = snapshot else {
                panic!("OpenAI video expected")
            };
            assert!(seed.error_message.is_none());
        }
    }

    #[test]
    fn openai_video_id_takes_precedence_over_xai_alias() {
        assert_eq!(
            super::openai_video_provider_task_id(
                json!({"id": "openai-id", "request_id": "trace-id"})
                    .as_object()
                    .unwrap()
            ),
            Some("openai-id")
        );
        assert_eq!(
            super::openai_video_provider_task_id(
                json!({"id": " ", "request_id": "xai-id"})
                    .as_object()
                    .unwrap()
            ),
            Some("xai-id")
        );
        assert_eq!(
            super::openai_video_provider_task_id(json!({"request_id": " "}).as_object().unwrap()),
            None
        );
    }

    #[test]
    fn builds_local_sync_finalize_read_response_for_supported_video_finalize_kinds() {
        let delete_response = build_local_sync_finalize_read_response(
            "openai_video_delete_sync_finalize",
            404,
            Some(&json!({"task_id": "task-123"})),
        )
        .expect("delete finalize response should build");
        assert_eq!(delete_response.status_code, 200);
        assert_eq!(
            delete_response.body_json,
            json!({
                "id": "task-123",
                "object": "video",
                "deleted": true,
            })
        );

        let cancel_response = build_local_sync_finalize_read_response(
            "gemini_video_cancel_sync_finalize",
            200,
            Some(&json!({})),
        )
        .expect("cancel finalize response should build");
        assert_eq!(cancel_response.status_code, 200);
        assert_eq!(cancel_response.body_json, json!({}));
    }

    #[test]
    fn rejects_local_sync_finalize_read_response_when_status_or_context_is_invalid() {
        assert!(build_local_sync_finalize_read_response(
            "openai_video_delete_sync_finalize",
            500,
            Some(&json!({"task_id": "task-123"})),
        )
        .is_none());
        assert!(build_local_sync_finalize_read_response(
            "openai_video_delete_sync_finalize",
            200,
            Some(&json!({})),
        )
        .is_none());
        assert!(build_local_sync_finalize_read_response(
            "openai_video_cancel_sync_finalize",
            500,
            Some(&json!({})),
        )
        .is_none());
    }

    #[test]
    fn resolves_local_sync_success_background_report_kind_for_supported_video_finalize_kinds() {
        assert_eq!(
            resolve_local_sync_success_background_report_kind("openai_video_delete_sync_finalize"),
            Some("openai_video_delete_sync_success")
        );
        assert_eq!(
            resolve_local_sync_success_background_report_kind("openai_video_create_sync_finalize"),
            None
        );
    }

    #[test]
    fn resolves_local_sync_error_background_report_kind_for_supported_video_finalize_kinds() {
        assert_eq!(
            resolve_local_sync_error_background_report_kind("openai_video_remix_sync_finalize"),
            Some("openai_video_remix_sync_error")
        );
        assert_eq!(
            resolve_local_sync_error_background_report_kind("unknown_finalize_kind"),
            None
        );
    }

    #[test]
    fn builds_internal_finalize_video_plan_for_supported_video_signatures() {
        let openai_plan = build_internal_finalize_video_plan(
            "trace-openai-video",
            "openai:video",
            Some(&json!({
                "request_id": "req-openai-video",
                "model": "sora-2",
                "api_key_id": "api-key-openai-video",
                "original_request_body": { "prompt": "make a trailer" }
            })),
        )
        .expect("openai video plan should build");
        assert_eq!(openai_plan.request_id, "req-openai-video");
        assert_eq!(
            openai_plan.url,
            "https://internal.gateway.invalid/v1/videos"
        );
        assert_eq!(openai_plan.model_name.as_deref(), Some("sora-2"));

        let gemini_plan = build_internal_finalize_video_plan(
            "trace-gemini-video",
            "gemini:video",
            Some(&json!({
                "local_short_id": "short-123"
            })),
        )
        .expect("gemini video plan should build");
        assert_eq!(
            gemini_plan.url,
            "https://internal.gateway.invalid/v1beta/models/veo-3:predictLongRunning"
        );
        assert_eq!(gemini_plan.model_name.as_deref(), Some("veo-3"));
        assert_eq!(gemini_plan.request_id, "trace-gemini-video");
    }

    #[test]
    fn rejects_internal_finalize_video_plan_for_non_video_signatures() {
        assert!(
            build_internal_finalize_video_plan("trace-123", "openai:chat", Some(&json!({})))
                .is_none()
        );
    }
}
