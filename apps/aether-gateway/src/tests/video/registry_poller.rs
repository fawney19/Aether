use std::sync::{Arc, Mutex};

use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskLookupKey, VideoTaskReadRepository, VideoTaskStatus,
    VideoTaskWriteRepository,
};
use axum::body::to_bytes;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use serde_json::json;

use super::{
    build_state_with_execution_runtime_override, start_server, AppState, VideoTaskTruthSourceMode,
};

fn sample_due_openai_task(upstream_base_url: &str) -> UpsertVideoTask {
    UpsertVideoTask {
        id: "task-local-123".to_string(),
        short_id: Some("task-local-123".to_string()),
        request_id: "request-video-poller-local-123".to_string(),
        user_id: Some("user-video-poller-123".to_string()),
        api_key_id: Some("key-video-poller-123".to_string()),
        username: Some("video-user".to_string()),
        api_key_name: Some("video-key".to_string()),
        external_task_id: Some("ext-video-task-123".to_string()),
        provider_id: Some("provider-openai-video-local-1".to_string()),
        endpoint_id: Some("endpoint-openai-video-local-1".to_string()),
        key_id: Some("key-openai-video-local-1".to_string()),
        client_api_format: Some("openai:video".to_string()),
        provider_api_format: Some("openai:video".to_string()),
        format_converted: false,
        model: Some("sora-2".to_string()),
        prompt: Some("hello".to_string()),
        original_request_body: Some(json!({"prompt": "hello"})),
        duration_seconds: Some(4),
        resolution: Some("720p".to_string()),
        aspect_ratio: Some("16:9".to_string()),
        size: Some("1280x720".to_string()),
        status: VideoTaskStatus::Submitted,
        progress_percent: 0,
        progress_message: None,
        retry_count: 0,
        poll_interval_seconds: 10,
        next_poll_at_unix_secs: Some(0),
        poll_count: 0,
        max_poll_count: 360,
        created_at_unix_ms: 123,
        submitted_at_unix_secs: Some(123),
        completed_at_unix_secs: None,
        updated_at_unix_secs: 123,
        error_code: None,
        error_message: None,
        video_url: None,
        request_metadata: Some(json!({
            "rust_local_snapshot": {
                "OpenAi": {
                    "local_task_id": "task-local-123",
                    "upstream_task_id": "ext-video-task-123",
                    "created_at_unix_ms": 123,
                    "user_id": "user-video-poller-123",
                    "api_key_id": "key-video-poller-123",
                    "model": "sora-2",
                    "prompt": "hello",
                    "size": "1280x720",
                    "seconds": "4",
                    "remixed_from_video_id": null,
                    "status": "Submitted",
                    "progress_percent": 0,
                    "completed_at_unix_secs": null,
                    "expires_at_unix_secs": null,
                    "error_code": null,
                    "error_message": null,
                    "video_url": null,
                    "persistence": {
                        "request_id": "request-video-poller-local-123",
                        "username": "video-user",
                        "api_key_name": "video-key",
                        "client_api_format": "openai:video",
                        "provider_api_format": "openai:video",
                        "original_request_body": {
                            "prompt": "hello"
                        },
                        "format_converted": false
                    },
                    "transport": {
                        "upstream_base_url": upstream_base_url,
                        "provider_name": "openai-video",
                        "provider_id": "provider-openai-video-local-1",
                        "endpoint_id": "endpoint-openai-video-local-1",
                        "key_id": "key-openai-video-local-1",
                        "headers": {
                            "authorization": "Bearer sk-upstream-openai-video",
                            "content-type": "application/json"
                        },
                        "content_type": "application/json",
                        "model_name": "sora-2-upstream",
                        "proxy": null,
                        "transport_profile": null,
                        "timeouts": null
                    }
                }
            }
        })),
    }
}

#[tokio::test]
async fn gateway_background_video_task_poller_refreshes_due_openai_task_from_repository() {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SeenExecutionRuntimeRequest {
        method: String,
        url: String,
    }

    let seen_execution_runtime_requests =
        Arc::new(Mutex::new(Vec::<SeenExecutionRuntimeRequest>::new()));
    let seen_execution_runtime_requests_clone = Arc::clone(&seen_execution_runtime_requests);

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_requests_inner =
                Arc::clone(&seen_execution_runtime_requests_clone);
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                let method = payload
                    .get("method")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let url = payload
                    .get("url")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                seen_execution_runtime_requests_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(SeenExecutionRuntimeRequest {
                        method: method.clone(),
                        url: url.clone(),
                    });
                Json(json!({
                    "request_id": "req-openai-video-poller-refresh-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "ext-video-task-123",
                            "status": "processing",
                            "progress": 37
                        }
                    }
                }))
            }
        }),
    );

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_due_openai_task("https://api.openai.example/v1"))
        .await
        .expect("task upsert should succeed");

    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_video_task_data_repository_for_tests(Arc::clone(&repository))
        .with_video_task_truth_source_mode(VideoTaskTruthSourceMode::RustAuthoritative)
        .with_video_task_poller_config(std::time::Duration::from_millis(25), 8);
    let background_tasks = gateway_state.spawn_background_tasks();
    assert!(!background_tasks.is_empty(), "poller task should spawn");

    let stored = {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let stored = repository
                .find(VideoTaskLookupKey::Id("task-local-123"))
                .await
                .expect("video task lookup should succeed")
                .expect("video task should exist");
            if stored.progress_percent == 37 {
                break stored;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "poller did not refresh task within 5s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };

    assert_eq!(stored.status, VideoTaskStatus::Processing);
    assert_eq!(stored.progress_percent, 37);
    assert_eq!(stored.poll_count, 1);
    assert!(
        stored.next_poll_at_unix_secs.is_some_and(|value| value > 0),
        "poller should push next poll into the future"
    );
    assert_eq!(
        stored
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("rust_owner"))
            .and_then(serde_json::Value::as_str),
        Some("async_task")
    );
    assert_eq!(
        stored
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("poll_raw_response"))
            .and_then(|value| value.get("status"))
            .and_then(serde_json::Value::as_str),
        Some("processing")
    );

    assert_eq!(
        seen_execution_runtime_requests
            .lock()
            .expect("mutex should lock")
            .clone(),
        vec![SeenExecutionRuntimeRequest {
            method: "GET".to_string(),
            url: "https://api.openai.example/v1/videos/ext-video-task-123".to_string(),
        }]
    );

    background_tasks.shutdown().await;
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_background_video_task_poller_refreshes_due_openai_task_from_repository_without_execution_runtime_override(
) {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SeenUpstreamRequest {
        method: String,
        path: String,
    }

    let seen_upstream_requests = Arc::new(Mutex::new(Vec::<SeenUpstreamRequest>::new()));
    let seen_upstream_requests_clone = Arc::clone(&seen_upstream_requests);
    let upstream = Router::new().route(
        "/v1/videos/ext-video-task-123",
        any(move |request: Request| {
            let seen_upstream_requests_inner = Arc::clone(&seen_upstream_requests_clone);
            async move {
                seen_upstream_requests_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(SeenUpstreamRequest {
                        method: request.method().as_str().to_string(),
                        path: request.uri().path().to_string(),
                    });
                Json(json!({
                    "id": "ext-video-task-123",
                    "status": "processing",
                    "progress": 37
                }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let upstream_api_root = format!("{upstream_url}/v1");
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_due_openai_task(&upstream_api_root))
        .await
        .expect("task upsert should succeed");

    let gateway_state = AppState::new()
        .expect("gateway state should build")
        .with_video_task_data_repository_for_tests(Arc::clone(&repository))
        .with_video_task_truth_source_mode(VideoTaskTruthSourceMode::RustAuthoritative)
        .with_video_task_poller_config(std::time::Duration::from_millis(25), 8);
    let background_tasks = gateway_state.spawn_background_tasks();
    assert!(!background_tasks.is_empty(), "poller task should spawn");

    let stored = {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let stored = repository
                .find(VideoTaskLookupKey::Id("task-local-123"))
                .await
                .expect("video task lookup should succeed")
                .expect("video task should exist");
            if stored.progress_percent == 37 {
                break stored;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "poller did not refresh task within 5s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };

    assert_eq!(stored.status, VideoTaskStatus::Processing);
    assert_eq!(stored.progress_percent, 37);
    assert_eq!(stored.poll_count, 1);
    assert!(
        stored.next_poll_at_unix_secs.is_some_and(|value| value > 0),
        "poller should push next poll into the future"
    );
    assert_eq!(
        seen_upstream_requests
            .lock()
            .expect("mutex should lock")
            .clone(),
        vec![SeenUpstreamRequest {
            method: "GET".to_string(),
            path: "/v1/videos/ext-video-task-123".to_string(),
        }]
    );

    background_tasks.shutdown().await;
    upstream_handle.abort();
}
