use super::fixtures::*;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    GeminiVideoTaskSeed, LocalVideoTaskSnapshot, LocalVideoTaskStatus, OpenAiVideoTaskSeed,
    VideoTaskService, VideoTaskTruthSourceMode,
};

#[test]
fn rust_authoritative_service_builds_openai_cancel_follow_up_plan() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-local-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_ms: 1712345678,
        user_id: Some("user-123".to_string()),
        api_key_id: Some("key-123".to_string()),
        model: Some("sora-2".to_string()),
        prompt: None,
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

    let follow_up = service
        .prepare_follow_up_sync_plan(
            "openai_video_cancel_sync",
            "/v1/videos/task-local-123/cancel",
            None,
            Some(&sample_auth_context()),
            "trace-openai-cancel-123",
        )
        .expect("follow-up plan should build");

    assert_eq!(follow_up.plan.method, "DELETE");
    assert_eq!(
        follow_up.plan.url,
        "https://api.openai.example/v1/videos/ext-video-task-123"
    );
    assert_eq!(follow_up.plan.provider_api_format, "openai:video");
    assert!(follow_up.plan.body.json_body.is_none());
    assert_eq!(
        follow_up.report_kind.as_deref(),
        Some("openai_video_cancel_sync_finalize")
    );
    assert_eq!(
        follow_up
            .report_context
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|value| value.get("task_id"))
            .and_then(Value::as_str),
        Some("task-local-123")
    );

    let mut same_user_new_key = sample_auth_context();
    same_user_new_key.api_key_id = "key-rotated".to_string();
    assert!(service
        .prepare_follow_up_sync_plan_for_user(
            "openai_video_cancel_sync",
            "/v1/videos/task-local-123/cancel",
            None,
            Some(&same_user_new_key),
            "trace-openai-cancel-owner-rotated-key",
        )
        .is_some());
    let mut foreign_auth = sample_auth_context();
    foreign_auth.user_id = "user-foreign".to_string();
    foreign_auth.api_key_id = "key-foreign".to_string();
    assert!(service
        .prepare_follow_up_sync_plan_for_user(
            "openai_video_cancel_sync",
            "/v1/videos/task-local-123/cancel",
            None,
            Some(&foreign_auth),
            "trace-openai-cancel-foreign",
        )
        .is_none());
}

#[test]
fn rust_authoritative_service_builds_openai_remix_follow_up_plan() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-local-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_ms: 1712345678,
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
        video_url: Some("https://cdn.example.com/original.mp4".to_string()),
        persistence: sample_persistence("openai:video"),
        transport: sample_transport("https://api.openai.example", "openai:video"),
    }));

    let remix_body = json!({
        "prompt": "remix this",
        "model": "sora-2",
    });
    let follow_up = service
        .prepare_follow_up_sync_plan(
            "openai_video_remix_sync",
            "/v1/videos/task-local-123/remix",
            Some(&remix_body),
            Some(&sample_auth_context()),
            "trace-openai-remix-123",
        )
        .expect("follow-up plan should build");

    assert_eq!(follow_up.plan.method, "POST");
    assert_eq!(
        follow_up.plan.url,
        "https://api.openai.example/v1/videos/ext-video-task-123/remix"
    );
    assert_eq!(follow_up.plan.provider_api_format, "openai:video");
    assert_eq!(follow_up.plan.body.json_body, Some(remix_body.clone()));
    assert_eq!(
        follow_up.report_kind.as_deref(),
        Some("openai_video_remix_sync_finalize")
    );
    assert_eq!(
        follow_up
            .report_context
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|value| value.get("task_id"))
            .and_then(Value::as_str),
        Some("task-local-123")
    );

    let mut same_user_new_key = sample_auth_context();
    same_user_new_key.api_key_id = "key-rotated".to_string();
    assert!(service
        .prepare_follow_up_sync_plan_for_user(
            "openai_video_remix_sync",
            "/v1/videos/task-local-123/remix",
            Some(&remix_body),
            Some(&same_user_new_key),
            "trace-openai-remix-owner-rotated-key",
        )
        .is_some());
    let mut foreign_auth = sample_auth_context();
    foreign_auth.user_id = "user-foreign".to_string();
    foreign_auth.api_key_id = "key-foreign".to_string();
    assert!(service
        .prepare_follow_up_sync_plan_for_user(
            "openai_video_remix_sync",
            "/v1/videos/task-local-123/remix",
            Some(&remix_body),
            Some(&foreign_auth),
            "trace-openai-remix-foreign",
        )
        .is_none());
}

#[test]
fn rust_authoritative_service_builds_openai_delete_follow_up_plan() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-local-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_ms: 1712345678,
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
        video_url: None,
        persistence: sample_persistence("openai:video"),
        transport: sample_transport("https://api.openai.example", "openai:video"),
    }));

    let follow_up = service
        .prepare_follow_up_sync_plan(
            "openai_video_delete_sync",
            "/v1/videos/task-local-123",
            None,
            Some(&sample_auth_context()),
            "trace-openai-delete-123",
        )
        .expect("follow-up plan should build");

    assert_eq!(follow_up.plan.method, "DELETE");
    assert_eq!(
        follow_up.plan.url,
        "https://api.openai.example/v1/videos/ext-video-task-123"
    );
    assert_eq!(follow_up.plan.provider_api_format, "openai:video");
    assert!(follow_up.plan.body.json_body.is_none());
    assert_eq!(
        follow_up.report_kind.as_deref(),
        Some("openai_video_delete_sync_finalize")
    );
    assert_eq!(
        follow_up
            .report_context
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|value| value.get("task_id"))
            .and_then(Value::as_str),
        Some("task-local-123")
    );

    let mut same_user_new_key = sample_auth_context();
    same_user_new_key.api_key_id = "key-rotated".to_string();
    assert!(service
        .prepare_follow_up_sync_plan_for_user(
            "openai_video_delete_sync",
            "/v1/videos/task-local-123",
            None,
            Some(&same_user_new_key),
            "trace-openai-delete-owner-rotated-key",
        )
        .is_some());
    let mut foreign_auth = sample_auth_context();
    foreign_auth.user_id = "user-foreign".to_string();
    foreign_auth.api_key_id = "key-foreign".to_string();
    assert!(service
        .prepare_follow_up_sync_plan_for_user(
            "openai_video_delete_sync",
            "/v1/videos/task-local-123",
            None,
            Some(&foreign_auth),
            "trace-openai-delete-foreign",
        )
        .is_none());
}

#[test]
fn rust_authoritative_service_builds_gemini_cancel_follow_up_plan() {
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

    let follow_up = service
        .prepare_follow_up_sync_plan(
            "gemini_video_cancel_sync",
            "/v1beta/models/veo-3/operations/localshort123:cancel",
            None,
            Some(&sample_auth_context()),
            "trace-gemini-cancel-123",
        )
        .expect("follow-up plan should build");

    assert_eq!(follow_up.plan.method, "POST");
    assert_eq!(
            follow_up.plan.url,
            "https://generativelanguage.googleapis.com/v1beta/models/veo-3/operations/ext-video-123:cancel"
        );
    assert_eq!(follow_up.plan.provider_api_format, "gemini:video");
    assert_eq!(follow_up.plan.body.json_body, Some(json!({})));
    assert_eq!(
        follow_up.report_kind.as_deref(),
        Some("gemini_video_cancel_sync_finalize")
    );
    assert_eq!(
        follow_up
            .report_context
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|value| value.get("task_id"))
            .and_then(Value::as_str),
        Some("localshort123")
    );

    let mut same_user_new_key = sample_auth_context();
    same_user_new_key.api_key_id = "key-rotated".to_string();
    assert!(service
        .prepare_follow_up_sync_plan_for_user(
            "gemini_video_cancel_sync",
            "/v1beta/models/veo-3/operations/localshort123:cancel",
            None,
            Some(&same_user_new_key),
            "trace-gemini-cancel-owner-rotated-key",
        )
        .is_some());
    let mut foreign_auth = sample_auth_context();
    foreign_auth.user_id = "user-foreign".to_string();
    foreign_auth.api_key_id = "key-foreign".to_string();
    assert!(service
        .prepare_follow_up_sync_plan_for_user(
            "gemini_video_cancel_sync",
            "/v1beta/models/veo-3/operations/localshort123:cancel",
            None,
            Some(&foreign_auth),
            "trace-gemini-cancel-foreign",
        )
        .is_none());
}

#[test]
fn rust_authoritative_service_builds_openai_read_refresh_plan() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-local-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_ms: 1712345678,
        user_id: Some("user-123".to_string()),
        api_key_id: Some("key-123".to_string()),
        model: Some("sora-2".to_string()),
        prompt: None,
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

    let refresh = service
        .prepare_read_refresh_sync_plan(
            Some("openai"),
            "/v1/videos/task-local-123",
            "trace-openai-read-123",
        )
        .expect("read refresh plan should build");

    assert_eq!(refresh.plan.method, "GET");
    assert_eq!(
        refresh.plan.url,
        "https://api.openai.example/v1/videos/ext-video-task-123"
    );
    assert!(refresh.plan.body.json_body.is_none());
}

#[test]
fn rust_authoritative_service_builds_gemini_read_refresh_plan() {
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

    let refresh = service
        .prepare_read_refresh_sync_plan(
            Some("gemini"),
            "/v1beta/models/veo-3/operations/localshort123",
            "trace-gemini-read-123",
        )
        .expect("read refresh plan should build");

    assert_eq!(refresh.plan.method, "GET");
    assert_eq!(
        refresh.plan.url,
        "https://generativelanguage.googleapis.com/v1beta/models/veo-3/operations/ext-video-123"
    );
    assert!(refresh.plan.body.json_body.is_none());
}

#[test]
fn rust_authoritative_service_builds_poll_refresh_batch_for_active_tasks_only() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-active-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_ms: 1712345678,
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
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-completed-123".to_string(),
        upstream_task_id: "ext-video-task-999".to_string(),
        created_at_unix_ms: 1712345678,
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
        video_url: Some("https://cdn.example.com/ext-video-task-999.mp4".to_string()),
        persistence: sample_persistence("openai:video"),
        transport: sample_transport("https://api.openai.example", "openai:video"),
    }));

    let batch = service.prepare_poll_refresh_batch(10, "trace-poller");

    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].plan.method, "GET");
    assert_eq!(
        batch[0].plan.url,
        "https://api.openai.example/v1/videos/ext-video-task-123"
    );
}

#[test]
fn file_video_task_store_persists_snapshots_across_service_rebuilds() {
    let store_path =
        std::env::temp_dir().join(format!("aether-video-task-store-{}.json", Uuid::new_v4()));
    let encryption_key = "video-task-test-encryption-key";
    let service = VideoTaskService::with_file_store(
        VideoTaskTruthSourceMode::RustAuthoritative,
        &store_path,
        encryption_key,
    )
    .expect("file-backed service should build");
    service.record_snapshot(LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-file-123".to_string(),
        upstream_task_id: "ext-video-task-123".to_string(),
        created_at_unix_ms: 1712345678,
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
    drop(service);

    let stored_bytes = std::fs::read(&store_path).expect("encrypted store should exist");
    assert!(stored_bytes.starts_with(b"aether-video-tasks-v2\n"));
    assert!(!String::from_utf8_lossy(&stored_bytes).contains("api.openai.example"));

    let reopened = VideoTaskService::with_file_store(
        VideoTaskTruthSourceMode::RustAuthoritative,
        &store_path,
        encryption_key,
    )
    .expect("reopened file-backed service should build");
    let response = reopened
        .read_response(Some("openai"), "/v1/videos/task-file-123")
        .expect("persisted read response should exist");
    assert_eq!(response.status_code, 200);
    assert_eq!(
        response.body_json.get("id").and_then(Value::as_str),
        Some("task-file-123")
    );

    let _ = std::fs::remove_file(store_path);
}
