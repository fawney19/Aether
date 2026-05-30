use super::fixtures::*;
use serde_json::{json, Map, Value};

use super::{
    GeminiVideoTaskSeed, LocalVideoTaskSeed, LocalVideoTaskSnapshot, LocalVideoTaskStatus,
    OpenAiVideoTaskSeed, VideoTaskService, VideoTaskSyncReportMode, VideoTaskTruthSourceMode,
};

#[test]
fn openai_video_seed_preserves_existing_local_identity() {
    let provider_body = json!({"id": "ext-video-task-123"});
    let report_context = json!({
        "local_task_id": "task-local-123",
        "local_created_at": 1712345678u64,
        "model": "sora-2",
        "original_request_body": {
            "prompt": "hello",
            "size": "1024x1024",
            "seconds": "8"
        }
    });

    let seed = LocalVideoTaskSeed::from_sync_finalize(
        "openai_video_create_sync_finalize",
        provider_body
            .as_object()
            .expect("provider body should be object"),
        report_context
            .as_object()
            .expect("report context should be object"),
        &sample_plan("https://api.openai.example/v1/videos", "openai:video"),
    )
    .expect("seed should build");

    let mut patched_context = report_context
        .as_object()
        .expect("report context should be object")
        .clone();
    seed.apply_to_report_context(&mut patched_context);
    let client_body = seed.client_body_json();

    assert_eq!(
        seed.success_report_kind(),
        "openai_video_create_sync_success"
    );
    assert_eq!(
        patched_context.get("local_task_id").and_then(Value::as_str),
        Some("task-local-123")
    );
    assert_eq!(
        patched_context
            .get("local_created_at")
            .and_then(Value::as_u64),
        Some(1712345678)
    );
    assert_eq!(
        client_body.get("id").and_then(Value::as_str),
        Some("task-local-123")
    );
    assert_eq!(
        client_body.get("created_at").and_then(Value::as_u64),
        Some(1712345678)
    );
    assert_eq!(
        client_body.get("model").and_then(Value::as_str),
        Some("sora-2")
    );
    assert_eq!(
        client_body.get("prompt").and_then(Value::as_str),
        Some("hello")
    );
}

#[test]
fn openai_video_remix_seed_carries_source_task() {
    let provider_body = json!({"id": "ext-remix-task-123"});
    let report_context = json!({
        "task_id": "task-source-123",
        "original_request_body": {
            "prompt": "remix it"
        }
    });

    let seed = LocalVideoTaskSeed::from_sync_finalize(
        "openai_video_remix_sync_finalize",
        provider_body
            .as_object()
            .expect("provider body should be object"),
        report_context
            .as_object()
            .expect("report context should be object"),
        &sample_plan(
            "https://api.openai.example/v1/videos/ext-source/remix",
            "openai:video",
        ),
    )
    .expect("seed should build");
    let client_body = seed.client_body_json();

    assert_eq!(
        seed.success_report_kind(),
        "openai_video_remix_sync_success"
    );
    assert_eq!(
        client_body
            .get("remixed_from_video_id")
            .and_then(Value::as_str),
        Some("task-source-123")
    );
}

#[test]
fn gemini_video_seed_generates_short_id_and_pending_operation_name() {
    let provider_body = json!({"name": "operations/ext-video-task-123"});
    let report_context = json!({
        "model": "veo-3"
    });

    let seed = LocalVideoTaskSeed::from_sync_finalize(
        "gemini_video_create_sync_finalize",
        provider_body
            .as_object()
            .expect("provider body should be object"),
        report_context
            .as_object()
            .expect("report context should be object"),
        &sample_plan(
            "https://generativelanguage.googleapis.com/v1beta/models/veo-3:predictLongRunning",
            "gemini:video",
        ),
    )
    .expect("seed should build");
    let mut patched_context = Map::new();
    seed.apply_to_report_context(&mut patched_context);
    let client_body = seed.client_body_json();
    let local_short_id = patched_context
        .get("local_short_id")
        .and_then(Value::as_str)
        .expect("local_short_id should exist");

    assert_eq!(
        seed.success_report_kind(),
        "gemini_video_create_sync_success"
    );
    assert_eq!(local_short_id.len(), 12);
    assert_eq!(
        client_body.get("name").and_then(Value::as_str),
        Some(format!("models/veo-3/operations/{local_short_id}").as_str())
    );
    assert_eq!(
        client_body.get("done").and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn python_backed_video_truth_source_requires_inline_sync_report() {
    let provider_body = json!({"id": "ext-video-task-123"});
    let report_context = json!({});

    let plan = VideoTaskTruthSourceMode::PythonSyncReport
        .prepare_sync_success(
            "openai_video_create_sync_finalize",
            provider_body
                .as_object()
                .expect("provider body should be object"),
            report_context
                .as_object()
                .expect("report context should be object"),
            &sample_plan("https://api.openai.example/v1/videos", "openai:video"),
        )
        .expect("plan should build");

    assert_eq!(plan.report_mode(), VideoTaskSyncReportMode::InlineSync);
    assert_eq!(
        plan.success_report_kind(),
        "openai_video_create_sync_success"
    );
}

#[test]
fn rust_authoritative_video_truth_source_can_background_success_report() {
    let provider_body = json!({"name": "operations/ext-video-task-123"});
    let report_context = json!({
        "model": "veo-3"
    });

    let plan = VideoTaskTruthSourceMode::RustAuthoritative
        .prepare_sync_success(
            "gemini_video_create_sync_finalize",
            provider_body
                .as_object()
                .expect("provider body should be object"),
            report_context
                .as_object()
                .expect("report context should be object"),
            &sample_plan(
                "https://generativelanguage.googleapis.com/v1beta/models/veo-3:predictLongRunning",
                "gemini:video",
            ),
        )
        .expect("plan should build");

    assert_eq!(plan.report_mode(), VideoTaskSyncReportMode::Background);
    assert_eq!(
        plan.success_report_kind(),
        "gemini_video_create_sync_success"
    );
    let mut report_context = Map::new();
    plan.apply_to_report_context(&mut report_context);
    assert_eq!(
        report_context.get("rust_video_task_persisted"),
        Some(&Value::Bool(true))
    );
}

#[test]
fn rust_authoritative_service_reads_openai_task_from_local_registry() {
    let service = VideoTaskService::new(VideoTaskTruthSourceMode::RustAuthoritative);
    let snapshot = LocalVideoTaskSnapshot::OpenAi(OpenAiVideoTaskSeed {
        local_task_id: "task-local-123".to_string(),
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
    });
    service.record_snapshot(snapshot);

    let response = service
        .read_response(Some("openai"), "/v1/videos/task-local-123")
        .expect("read response should exist");

    assert_eq!(response.status_code, 200);
    assert_eq!(
        response.body_json.get("id").and_then(Value::as_str),
        Some("task-local-123")
    );
    assert_eq!(
        response.body_json.get("status").and_then(Value::as_str),
        Some("queued")
    );
}

#[test]
fn rust_authoritative_service_applies_cancel_and_delete_mutations() {
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
    service.record_snapshot(LocalVideoTaskSnapshot::Gemini(GeminiVideoTaskSeed {
        local_short_id: "short12345678".to_string(),
        upstream_operation_name: "operations/ext-video-task-123".to_string(),
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

    service.apply_finalize_mutation(
        "/v1/videos/task-local-123/cancel",
        "openai_video_cancel_sync_finalize",
    );
    let cancelled_openai = service
        .read_response(Some("openai"), "/v1/videos/task-local-123")
        .expect("openai read response should exist");
    assert_eq!(cancelled_openai.status_code, 404);
    assert_eq!(
        cancelled_openai.body_json,
        json!({"detail": "Video task was cancelled"})
    );

    service.apply_finalize_mutation(
        "/v1beta/models/veo-3/operations/short12345678:cancel",
        "gemini_video_cancel_sync_finalize",
    );
    let cancelled_gemini = service
        .read_response(
            Some("gemini"),
            "/v1beta/models/veo-3/operations/short12345678",
        )
        .expect("gemini read response should exist");
    assert_eq!(cancelled_gemini.status_code, 404);
    assert_eq!(
        cancelled_gemini.body_json,
        json!({"detail": "Video task was cancelled"})
    );

    service.apply_finalize_mutation(
        "/v1/videos/task-local-123",
        "openai_video_delete_sync_finalize",
    );
    let deleted_openai = service
        .read_response(Some("openai"), "/v1/videos/task-local-123")
        .expect("deleted openai read response should exist");
    assert_eq!(deleted_openai.status_code, 404);
    assert_eq!(
        deleted_openai.body_json,
        json!({"detail": "Video task not found"})
    );
}

#[test]
fn seed_captures_transport_metadata_from_execution_plan() {
    let provider_body = json!({"id": "ext-video-task-123"});
    let report_context = json!({});

    let seed = LocalVideoTaskSeed::from_sync_finalize(
        "openai_video_create_sync_finalize",
        provider_body
            .as_object()
            .expect("provider body should be object"),
        report_context
            .as_object()
            .expect("report context should be object"),
        &sample_plan("https://api.openai.example/v1/videos", "openai:video"),
    )
    .expect("seed should build");

    let LocalVideoTaskSeed::OpenAiCreate(seed) = seed else {
        panic!("seed should be openai");
    };
    assert_eq!(
        seed.transport.upstream_base_url,
        "https://api.openai.example/v1"
    );
    assert_eq!(seed.transport.provider_id, "provider-123");
    assert_eq!(seed.transport.endpoint_id, "endpoint-123");
    assert_eq!(seed.transport.key_id, "key-123");
    assert_eq!(
        seed.transport
            .headers
            .get("authorization")
            .map(String::as_str),
        Some("Bearer upstream-key")
    );
}
