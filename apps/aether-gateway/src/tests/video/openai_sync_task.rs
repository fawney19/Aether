use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus,
};
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskStatus, VideoTaskWriteRepository,
};
use axum::body::{to_bytes, Body};
use axum::response::Response;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
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
async fn gateway_executes_openai_video_delete_via_reconstructed_data_backed_local_follow_up_with_local_follow_up_routing(
) {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SeenExecutionRuntimeSyncRequest {
        method: String,
        url: String,
        authorization: String,
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

    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let upstream = Router::new()
        .route(
            "/api/internal/gateway/resolve",
            any(|_request: Request| async move {
                Json(json!({
                    "action": "proxy_public",
                    "route_class": "ai_public",
                    "route_family": "openai",
                    "route_kind": "video",
                    "auth_endpoint_signature": "openai:video",
                    "execution_runtime_candidate": true,
                    "public_path": "/v1/videos/task-local-followup-123"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-sync",
            any(move |_request: Request| {
                let decision_hits_inner = Arc::clone(&decision_hits_clone);
                async move {
                    *decision_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({
                        "action": "execution_runtime_sync_decision",
                        "decision_kind": "openai_video_delete_sync",
                        "request_id": "unexpected-decision-hit"
                    }))
                }
            }),
        )
        .route(
            "/api/internal/gateway/report-sync",
            any(move |_request: Request| {
                let report_hits_inner = Arc::clone(&report_hits_clone);
                async move {
                    *report_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"ok": true}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("{\"fallback\":true}"))
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
            "/v1/videos/task-local-followup-123",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeSyncRequest {
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
                    authorization: payload
                        .get("headers")
                        .and_then(|value| value.get("authorization"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
                Json(json!({
                    "request_id": "trace-openai-video-delete-local-123",
                    "status_code": 404,
                    "headers": {
                        "content-type": "application/json"
                    }
                }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;

    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-local-followup-123".to_string(),
            short_id: None,
            request_id: "request-openai-video-delete-local-123".to_string(),
            user_id: Some("user-openai-video-delete-local-123".to_string()),
            api_key_id: Some("key-openai-video-delete-local-123".to_string()),
            username: Some("video-user".to_string()),
            api_key_name: Some("video-key".to_string()),
            external_task_id: Some("ext-video-task-followup-123".to_string()),
            provider_id: Some("provider-openai-video-followup-1".to_string()),
            endpoint_id: Some("endpoint-openai-video-followup-1".to_string()),
            key_id: Some("key-openai-video-followup-1".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("video delete".to_string()),
            original_request_body: Some(json!({
                "model": "sora-2",
                "prompt": "video delete"
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
            video_url: Some("https://cdn.example.com/video-delete.mp4".to_string()),
            request_metadata: None,
        })
        .await
        .expect("upsert should succeed");
    let provider_catalog_repository = video_provider_catalog_repository(
        "provider-openai-video-followup-1",
        "openai",
        "endpoint-openai-video-followup-1",
        "openai:video",
        "https://api.openai.example/v1",
        "key-openai-video-followup-1",
        "sk-upstream-openai-video",
    );
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![
        (
            Some(hash_api_key("client-video-delete-foreign-key")),
            sample_auth_snapshot(
                "key-openai-video-delete-foreign-123",
                "user-openai-video-delete-foreign-123",
            ),
        ),
        (
            Some(hash_api_key("client-video-delete-owner-key")),
            sample_auth_snapshot(
                "key-openai-video-delete-rotated-local-123",
                "user-openai-video-delete-local-123",
            ),
        ),
    ]));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_video_task_truth_source_mode(VideoTaskTruthSourceMode::RustAuthoritative)
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_video_task_provider_transport_and_request_candidate_repository_for_tests(
                    repository,
                    provider_catalog_repository,
                    Arc::clone(&request_candidate_repository),
                    DEVELOPMENT_ENCRYPTION_KEY,
                )
                .with_auth_api_key_reader(auth_repository),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let foreign_response = client
        .delete(format!("{gateway_url}/v1/videos/task-local-followup-123"))
        .header(CONTROL_EXECUTE_FALLBACK_HEADER, "true")
        .bearer_auth("client-video-delete-foreign-key")
        .header(
            TRACE_ID_HEADER,
            "trace-openai-video-delete-foreign-local-123",
        )
        .send()
        .await
        .expect("foreign delete request should complete");
    let foreign_status = foreign_response.status();
    let foreign_body = foreign_response.text().await.expect("body should read");
    assert_eq!(
        foreign_status,
        StatusCode::NOT_FOUND,
        "unexpected foreign response body: {foreign_body}"
    );
    assert!(seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .is_none());
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    let response = client
        .delete(format!("{gateway_url}/v1/videos/task-local-followup-123"))
        .header(CONTROL_EXECUTE_FALLBACK_HEADER, "true")
        .bearer_auth("client-video-delete-owner-key")
        .header(TRACE_ID_HEADER, "trace-openai-video-delete-local-123")
        .send()
        .await
        .expect("request should succeed");

    let response_status = response.status();
    let response_text = response.text().await.expect("body should read");
    assert_eq!(
        response_status,
        StatusCode::OK,
        "unexpected response body: {response_text}"
    );
    let response_json: serde_json::Value =
        serde_json::from_str(&response_text).expect("body should parse");
    assert_eq!(
        response_json,
        json!({
            "id": "task-local-followup-123",
            "object": "video",
            "deleted": true
        })
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(seen_execution_runtime_request.method, "DELETE");
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.openai.example/v1/videos/ext-video-task-followup-123"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-video"
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("request-openai-video-delete-local-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
