use super::fixtures::*;
use serde_json::{json, Value};

use super::{
    GeminiVideoTaskSeed, LocalVideoTaskContentAction, LocalVideoTaskSnapshot, LocalVideoTaskStatus,
    OpenAiVideoTaskSeed, VideoTaskService, VideoTaskTruthSourceMode,
};

#[test]
fn rust_authoritative_service_projects_openai_status_into_local_read_response() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-local-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_secs: 1712345678,
        user_id: Some("user-123".to_string()),
        api_key_id: Some("key-123".to_string()),
        model: Some("sora-2".to_string()),
        prompt: Some("hello".to_string()),
        size: None,
        seconds: None,
        remixed_from_video_id: None,
        status: LocalVideoTaskStatus::Submitted,
        progress_percent: 0,
        completed_at_unix_secs: None,
        expires_at_unix_secs: None,
        error_code: None,
        error_message: None,
        video_url: None,
        persistence: sample_persistence("openai:video"),
        transport: sample_transport("https://api.openai.example", "openai:video"),
    }));

    assert!(service.project_openai_task_response(
        "task-local-123",
        json!({
            "status": "processing",
            "progress": 42
        })
        .as_object()
        .expect("provider body should be object"),
    ));
    let processing = service
        .read_response(Some("openai"), "/v1/videos/task-local-123")
        .expect("processing read response should exist");
    assert_eq!(processing.status_code, 200);
    assert_eq!(
        processing.body_json.get("status"),
        Some(&json!("processing"))
    );
    assert_eq!(processing.body_json.get("progress"), Some(&json!(42)));

    assert!(service.project_openai_task_response(
        "task-local-123",
        json!({
            "status": "failed",
            "progress": 100,
            "completed_at": 1712345688u64,
            "error": {
                "code": "upstream_failed",
                "message": "provider failed"
            }
        })
        .as_object()
        .expect("provider body should be object"),
    ));
    let failed = service
        .read_response(Some("openai"), "/v1/videos/task-local-123")
        .expect("failed read response should exist");
    assert_eq!(failed.status_code, 200);
    assert_eq!(failed.body_json.get("status"), Some(&json!("failed")));
    assert_eq!(
        failed.body_json.get("completed_at"),
        Some(&json!(1712345688u64))
    );
    assert_eq!(
        failed
            .body_json
            .get("error")
            .and_then(Value::as_object)
            .and_then(|value| value.get("code")),
        Some(&json!("upstream_failed"))
    );
}

#[test]
fn rust_authoritative_service_builds_openai_content_stream_plan_from_direct_video_url() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-local-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_secs: 1712345678,
        user_id: Some("user-123".to_string()),
        api_key_id: Some("key-123".to_string()),
        model: Some("sora-2".to_string()),
        prompt: None,
        size: None,
        seconds: None,
        remixed_from_video_id: None,
        status: LocalVideoTaskStatus::Completed,
        progress_percent: 100,
        completed_at_unix_secs: Some(1712345688),
        expires_at_unix_secs: None,
        error_code: None,
        error_message: None,
        video_url: Some("https://cdn.example.com/ext-video-task-123.mp4".to_string()),
        persistence: sample_persistence("openai:video"),
        transport: sample_transport("https://api.openai.example", "openai:video"),
    }));

    let action = service
        .prepare_openai_content_stream_action(
            "/v1/videos/task-local-123/content",
            Some("variant=video"),
            "trace-openai-content-123",
        )
        .expect("content action should exist");

    let LocalVideoTaskContentAction::StreamPlan(plan) = action else {
        panic!("content action should be stream plan");
    };
    assert_eq!(plan.method.as_str(), "GET");
    assert_eq!(
        plan.url.as_str(),
        "https://cdn.example.com/ext-video-task-123.mp4"
    );
    assert!(plan.headers.is_empty());
}

#[test]
fn rust_authoritative_service_returns_processing_content_response_for_pending_openai_task() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-local-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_secs: 1712345678,
        user_id: Some("user-123".to_string()),
        api_key_id: Some("key-123".to_string()),
        model: Some("sora-2".to_string()),
        prompt: None,
        size: None,
        seconds: None,
        remixed_from_video_id: None,
        status: LocalVideoTaskStatus::Processing,
        progress_percent: 42,
        completed_at_unix_secs: None,
        expires_at_unix_secs: None,
        error_code: None,
        error_message: None,
        video_url: None,
        persistence: sample_persistence("openai:video"),
        transport: sample_transport("https://api.openai.example", "openai:video"),
    }));

    let action = service
        .prepare_openai_content_stream_action(
            "/v1/videos/task-local-123/content",
            Some("variant=video"),
            "trace-openai-content-processing-123",
        )
        .expect("content action should exist");

    let LocalVideoTaskContentAction::Immediate {
        status_code,
        body_json,
    } = action
    else {
        panic!("content action should be immediate response");
    };
    assert_eq!(status_code, 202);
    assert_eq!(
        body_json,
        json!({"detail": "Video is still processing (status: processing)"})
    );
}

#[test]
fn rust_authoritative_service_projects_gemini_status_into_local_read_response() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::Gemini(GeminiVideoTaskSeed {
        local_short_id: "localshort123".to_string(),
        upstream_operation_name: "operations/ext-video-123".to_string(),
        user_id: Some("user-123".to_string()),
        api_key_id: Some("key-123".to_string()),
        model: "veo-3".to_string(),
        status: LocalVideoTaskStatus::Submitted,
        progress_percent: 0,
        error_code: None,
        error_message: None,
        metadata: json!({}),
        persistence: sample_persistence("gemini:video"),
        transport: sample_transport("https://generativelanguage.googleapis.com", "gemini:video"),
    }));

    assert!(service.project_gemini_task_response(
        "localshort123",
        json!({
            "done": false,
            "metadata": {
                "state": "PROCESSING"
            }
        })
        .as_object()
        .expect("provider body should be object"),
    ));
    let processing = service
        .read_response(
            Some("gemini"),
            "/v1beta/models/veo-3/operations/localshort123",
        )
        .expect("processing read response should exist");
    assert_eq!(processing.status_code, 200);
    assert_eq!(processing.body_json.get("done"), Some(&json!(false)));
    assert_eq!(
        processing.body_json.get("metadata"),
        Some(&json!({"state": "PROCESSING"}))
    );

    assert!(service.project_gemini_task_response(
        "localshort123",
        json!({
            "done": true,
            "response": {
                "generateVideoResponse": {
                    "generatedSamples": [
                        {"video": {"uri": "https://example.invalid/video.mp4"}}
                    ]
                }
            }
        })
        .as_object()
        .expect("provider body should be object"),
    ));
    let completed = service
        .read_response(
            Some("gemini"),
            "/v1beta/models/veo-3/operations/localshort123",
        )
        .expect("completed read response should exist");
    assert_eq!(completed.status_code, 200);
    assert_eq!(completed.body_json.get("done"), Some(&json!(true)));
    assert_eq!(
        completed
            .body_json
            .get("response")
            .and_then(|value| value.get("generateVideoResponse"))
            .and_then(|value| value.get("generatedSamples"))
            .and_then(Value::as_array)
            .and_then(|value| value.first())
            .and_then(|value| value.get("video"))
            .and_then(|value| value.get("uri")),
        Some(&json!("/v1beta/files/aev_localshort123:download?alt=media"))
    );
}
