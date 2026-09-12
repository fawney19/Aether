use aether_contracts::{StreamFrame, StreamFramePayload, StreamFrameType};
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskStatus, VideoTaskWriteRepository,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::response::Response;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use http::header::{HeaderName, HeaderValue};
use http::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

use crate::constants::{CONTROL_EXECUTED_HEADER, CONTROL_EXECUTE_FALLBACK_HEADER, TRACE_ID_HEADER};

use super::{
    build_router_with_state, build_state_with_execution_runtime_override, start_server,
    video_provider_catalog_repository, VideoTaskTruthSourceMode,
};

#[tokio::test]
async fn gateway_executes_openai_video_content_from_reconstructed_data_task_without_decision_stream(
) {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SeenExecutionRuntimeStreamRequest {
        method: String,
        url: String,
        headers: serde_json::Value,
    }

    fn hash_api_key(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "video-user".to_string(),
            Some("video@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(json!(["openai"])),
            Some(json!(["openai:video"])),
            Some(json!(["sora-2"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            Some(json!(["openai"])),
            Some(json!(["openai:video"])),
            Some(json!(["sora-2"])),
        )
        .expect("auth snapshot should build")
    }

    let decision_stream_hits = Arc::new(Mutex::new(0usize));
    let decision_stream_hits_clone = Arc::clone(&decision_stream_hits);
    let execute_stream_hits = Arc::new(Mutex::new(0usize));
    let execute_stream_hits_clone = Arc::clone(&execute_stream_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let seen_execution_runtime_stream =
        Arc::new(Mutex::new(None::<SeenExecutionRuntimeStreamRequest>));
    let seen_execution_runtime_stream_clone = Arc::clone(&seen_execution_runtime_stream);
    let upstream = Router::new()
        .route(
            "/api/internal/gateway/resolve",
            any(|request: Request| async move {
                Json(json!({
                    "action": "proxy_public",
                    "route_class": "ai_public",
                    "route_family": "openai",
                    "route_kind": "video",
                    "auth_endpoint_signature": "openai:video",
                    "execution_runtime_candidate": true,
                    "public_path": request.uri().path()
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-stream",
            any(move |_request: Request| {
                let decision_stream_hits_inner = Arc::clone(&decision_stream_hits_clone);
                async move {
                    *decision_stream_hits_inner
                        .lock()
                        .expect("mutex should lock") += 1;
                    Json(json!({
                        "action": "execution_runtime_stream_decision",
                        "decision_kind": "openai_video_content",
                        "request_id": "unexpected-decision-stream-hit"
                    }))
                }
            }),
        )
        .route(
            "/api/internal/gateway/execute-stream",
            any(move |_request: Request| {
                let execute_stream_hits_inner = Arc::clone(&execute_stream_hits_clone);
                async move {
                    *execute_stream_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("fallback"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        HeaderName::from_static(CONTROL_EXECUTED_HEADER),
                        HeaderValue::from_static("true"),
                    );
                    response
                }
            }),
        )
        .route(
            "/v1/videos/task-content-local-123/content",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(move |request: Request| {
            let seen_execution_runtime_stream_inner =
                Arc::clone(&seen_execution_runtime_stream_clone);
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                *seen_execution_runtime_stream_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeStreamRequest {
                    method: payload
                        .get("method")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    url: payload
                        .get("url")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    headers: payload.get("headers").cloned().unwrap_or_else(|| json!({})),
                });

                let frames = [
                    StreamFrame {
                        frame_type: StreamFrameType::Headers,
                        payload: StreamFramePayload::Headers {
                            status_code: 200,
                            headers: std::collections::BTreeMap::from([(
                                "content-type".to_string(),
                                "video/mp4".to_string(),
                            )]),
                            response_observation: None,
                        },
                    },
                    StreamFrame {
                        frame_type: StreamFrameType::Data,
                        payload: StreamFramePayload::Data {
                            chunk_b64: Some(BASE64_STANDARD.encode(b"video-")),
                            text: None,
                        },
                    },
                    StreamFrame {
                        frame_type: StreamFrameType::Data,
                        payload: StreamFramePayload::Data {
                            chunk_b64: Some(BASE64_STANDARD.encode(b"content")),
                            text: None,
                        },
                    },
                    StreamFrame::eof(),
                ];
                let body = frames
                    .into_iter()
                    .map(|frame| serde_json::to_string(&frame).expect("frame should serialize"))
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(body))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-ndjson"),
                );
                response
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-content-local-123".to_string(),
            short_id: None,
            request_id: "request-openai-video-content-local-123".to_string(),
            user_id: Some("user-video-content-local-123".to_string()),
            api_key_id: Some("key-video-content-local-123".to_string()),
            username: Some("video-user".to_string()),
            api_key_name: Some("video-key".to_string()),
            external_task_id: Some("ext-video-content-followup-123".to_string()),
            provider_id: Some("provider-openai-video-content-followup-1".to_string()),
            endpoint_id: Some("endpoint-openai-video-content-followup-1".to_string()),
            key_id: Some("key-openai-video-content-followup-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("video content".to_string()),
            original_request_body: Some(json!({
                "model": "sora-2",
                "prompt": "video content"
            })),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: VideoTaskStatus::Completed,
            progress_percent: 100,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: 123,
            submitted_at_unix_secs: Some(123),
            completed_at_unix_secs: Some(456),
            updated_at_unix_secs: 456,
            error_code: None,
            error_message: None,
            video_url: Some(
                "https://cdn.example.com/video-content.mp4?signature=a%2Fb%2Bc%3D&part=2&part=1"
                    .to_string(),
            ),
            request_metadata: None,
        })
        .await
        .expect("upsert should succeed");
    let provider_catalog_repository = video_provider_catalog_repository(
        "provider-openai-video-content-followup-1",
        "openai",
        "endpoint-openai-video-content-followup-1",
        "openai:video",
        "https://api.openai.example/v1",
        "key-openai-video-content-followup-1",
        "sk-upstream-openai-video",
    );
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![
        (
            Some(hash_api_key("client-video-content-foreign-key")),
            sample_auth_snapshot(
                "key-video-content-foreign-123",
                "user-video-content-foreign-123",
            ),
        ),
        (
            Some(hash_api_key("client-video-content-owner-key")),
            sample_auth_snapshot(
                "key-video-content-local-rotated-123",
                "user-video-content-local-123",
            ),
        ),
    ]));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_video_task_truth_source_mode(VideoTaskTruthSourceMode::RustAuthoritative)
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_video_task_repository_and_provider_transport_for_tests(
                    repository,
                    provider_catalog_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                )
                .with_auth_api_key_reader(auth_repository),
            )
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let foreign_response = client
        .get(format!(
            "{gateway_url}/v1/videos/task-content-local-123/content?variant=video"
        ))
        .header(CONTROL_EXECUTE_FALLBACK_HEADER, "true")
        .bearer_auth("client-video-content-foreign-key")
        .header(
            TRACE_ID_HEADER,
            "trace-openai-video-content-foreign-local-123",
        )
        .send()
        .await
        .expect("foreign content request should complete");
    let foreign_status = foreign_response.status();
    let foreign_body = foreign_response.text().await.expect("body should read");
    assert_eq!(
        foreign_status,
        StatusCode::NOT_FOUND,
        "unexpected foreign response body: {foreign_body}"
    );
    assert!(seen_execution_runtime_stream
        .lock()
        .expect("mutex should lock")
        .is_none());
    assert_eq!(*decision_stream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*execute_stream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    let response = client
        .get(format!(
            "{gateway_url}/v1/videos/task-content-local-123/content?variant=video"
        ))
        .header(CONTROL_EXECUTE_FALLBACK_HEADER, "true")
        .bearer_auth("client-video-content-owner-key")
        .header(TRACE_ID_HEADER, "trace-openai-video-content-local-123")
        .send()
        .await
        .expect("content request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("video/mp4")
    );
    assert_eq!(
        response.bytes().await.expect("body should read"),
        Bytes::from_static(b"video-content")
    );

    let seen_stream_request = seen_execution_runtime_stream
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime stream should be captured");
    assert_eq!(seen_stream_request.method, "GET");
    assert_eq!(
        seen_stream_request.url,
        "https://cdn.example.com/video-content.mp4?signature=a%2Fb%2Bc%3D&part=2&part=1"
    );
    assert!(seen_stream_request.headers.get("authorization").is_none());
    assert_eq!(*decision_stream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*execute_stream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
