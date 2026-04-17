use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
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
use std::sync::{Arc, Mutex};

use crate::constants::{CONTROL_EXECUTED_HEADER, CONTROL_EXECUTE_FALLBACK_HEADER, TRACE_ID_HEADER};

use super::{
    build_router_with_state, build_state_with_execution_runtime_override, start_server,
    VideoTaskTruthSourceMode,
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

    fn sample_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-video-followup-1".to_string(),
            "openai".to_string(),
            Some("https://example.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            None,
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-openai-video-followup-1".to_string(),
            "provider-openai-video-followup-1".to_string(),
            "openai:video".to_string(),
            Some("openai".to_string()),
            Some("video".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.example".to_string(),
            None,
            None,
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-video-followup-1".to_string(),
            "provider-openai-video-followup-1".to_string(),
            "prod".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:video"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-openai-video")
                .expect("api key should encrypt"),
            None,
            None,
            Some(json!({"openai:video": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
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
                    "auth_context": {
                        "user_id": "user-openai-video-delete-local-123",
                        "api_key_id": "key-openai-video-delete-local-123",
                        "access_allowed": true
                    },
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
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
        .with_video_task_truth_source_mode(VideoTaskTruthSourceMode::RustAuthoritative)
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_video_task_provider_transport_and_request_candidate_repository_for_tests(
                repository,
                provider_catalog_repository,
                Arc::clone(&request_candidate_repository),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .delete(format!("{gateway_url}/v1/videos/task-local-followup-123"))
        .header(CONTROL_EXECUTE_FALLBACK_HEADER, "true")
        .header(TRACE_ID_HEADER, "trace-openai-video-delete-local-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let response_json: serde_json::Value = response.json().await.expect("body should parse");
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
