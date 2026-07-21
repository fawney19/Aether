use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
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
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

use crate::constants::TRACE_ID_HEADER;

use super::{build_router_with_state, build_state_with_execution_runtime_override, start_server};

#[tokio::test]
async fn gateway_executes_openai_video_create_via_local_decision_gate_with_local_planning_only() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        method: String,
        url: String,
        authorization: String,
        model: String,
        prompt: String,
        endpoint_tag: String,
        conditional_header: String,
        renamed_header: String,
        dropped_header_present: bool,
        metadata_mode: String,
        metadata_source: String,
        store_present: bool,
        proxy_node_id: String,
        transport_profile_id: String,
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

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-video-local-1".to_string(),
            provider_name: "openai".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-video-local-1".to_string(),
            endpoint_api_format: "openai:video".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("video".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-video-local-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:video".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(json!({"openai:video": 1})),
            model_id: "model-openai-video-local-1".to_string(),
            global_model_id: "global-model-openai-video-local-1".to_string(),
            global_model_name: "sora-2".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(false),
            model_provider_model_name: "sora-2-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "sora-2-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:video".to_string()]),
                endpoint_ids: None,
                operations: None,
            }]),
            model_supports_streaming: Some(false),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-video-local-1".to_string(),
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
            Some(serde_json::json!({"enabled": true, "node_id":"proxy-node-openai-video-local"})),
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-openai-video-local-1".to_string(),
            "provider-openai-video-local-1".to_string(),
            "openai:video".to_string(),
            Some("openai".to_string()),
            Some("video".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.example".to_string(),
            Some(json!([
                {"action":"set","key":"x-endpoint-tag","value":"openai-video-local"},
                {"action":"set","key":"x-conditional-tag","value":"video-body-rule-applied","condition":{"path":"metadata.mode","op":"eq","value":"safe","source":"current"}},
                {"action":"rename","from":"x-client-rename","to":"x-upstream-rename"},
                {"action":"drop","key":"x-drop-me"}
            ])),
            Some(json!([
                {"action":"set","path":"metadata.mode","value":"safe","condition":{"path":"metadata.mode","op":"not_exists","source":"current"}},
                {"action":"rename","from":"metadata.client","to":"metadata.source"},
                {"action":"drop","path":"store"}
            ])),
            Some(2),
            Some("/custom/v1/videos".to_string()),
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-video-local-1".to_string(),
            "provider-openai-video-local-1".to_string(),
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
            Some(json!({"transport_profile":"chrome_136"})),
        )
        .expect("key transport should build")
    }

    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

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
                        "user_id": "user-openai-video-local-123",
                        "api_key_id": "key-openai-video-local-123",
                        "access_allowed": true
                    },
                    "public_path": "/v1/videos"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-sync",
            any(move |_request: Request| {
                let decision_hits_inner = Arc::clone(&decision_hits_clone);
                async move {
                    *decision_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/plan-sync",
            any(move |_request: Request| {
                let plan_hits_inner = Arc::clone(&plan_hits_clone);
                async move {
                    *plan_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
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
            "/v1/videos",
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
                    model: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("model"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    prompt: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("prompt"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    endpoint_tag: payload
                        .get("headers")
                        .and_then(|value| value.get("x-endpoint-tag"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    conditional_header: payload
                        .get("headers")
                        .and_then(|value| value.get("x-conditional-tag"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    renamed_header: payload
                        .get("headers")
                        .and_then(|value| value.get("x-upstream-rename"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    dropped_header_present: payload
                        .get("headers")
                        .and_then(|value| value.get("x-drop-me"))
                        .is_some(),
                    metadata_mode: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("metadata"))
                        .and_then(|value| value.get("mode"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    metadata_source: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("metadata"))
                        .and_then(|value| value.get("source"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    store_present: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("store"))
                        .is_some(),
                    proxy_node_id: payload
                        .get("proxy")
                        .and_then(|value| value.get("node_id"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    transport_profile_id: payload
                        .get("transport_profile")
                        .and_then(|value| value.get("profile_id"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
                Json(json!({
                    "request_id": "trace-openai-video-local-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "ext-video-task-123",
                            "status": "submitted"
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 18
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("client-openai-video-local-key")),
        sample_auth_snapshot("key-openai-video-local-123", "user-openai-video-local-123"),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider()],
        vec![sample_endpoint()],
        vec![sample_key()],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url)
    .with_data_state_for_tests(
        crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
            auth_repository,
            candidate_selection_repository,
            provider_catalog_repository,
            Arc::clone(&request_candidate_repository),
            DEVELOPMENT_ENCRYPTION_KEY,
        ),
    );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/videos"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer client-openai-video-local-key",
        )
        .header("x-client-rename", "rename-openai-video")
        .header("x-drop-me", "drop-openai-video")
        .header(TRACE_ID_HEADER, "trace-openai-video-local-123")
        .body("{\"model\":\"sora-2\",\"prompt\":\"hello local video\",\"metadata\":{\"client\":\"desktop-openai-video\"},\"store\":false}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(body.get("object"), Some(&json!("video")));
    assert_eq!(body.get("status"), Some(&json!("queued")));
    assert_eq!(body.get("prompt"), Some(&json!("hello local video")));

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(seen_execution_runtime_request.method, "POST");
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.openai.example/custom/v1/videos"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-video"
    );
    assert_eq!(seen_execution_runtime_request.model, "sora-2-upstream");
    assert_eq!(seen_execution_runtime_request.prompt, "hello local video");
    assert_eq!(
        seen_execution_runtime_request.endpoint_tag,
        "openai-video-local"
    );
    assert_eq!(
        seen_execution_runtime_request.conditional_header,
        "video-body-rule-applied"
    );
    assert_eq!(
        seen_execution_runtime_request.renamed_header,
        "rename-openai-video"
    );
    assert!(!seen_execution_runtime_request.dropped_header_present);
    assert_eq!(seen_execution_runtime_request.metadata_mode, "safe");
    assert_eq!(
        seen_execution_runtime_request.metadata_source,
        "desktop-openai-video"
    );
    assert!(!seen_execution_runtime_request.store_present);
    assert_eq!(
        seen_execution_runtime_request.proxy_node_id,
        "proxy-node-openai-video-local"
    );
    assert_eq!(
        seen_execution_runtime_request.transport_profile_id,
        "chrome_136"
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-video-local-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_executes_openai_video_remix_via_data_backed_local_follow_up_with_local_planning_only(
) {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        method: String,
        url: String,
        authorization: String,
        prompt: String,
    }

    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

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
                        "user_id": "user-openai-video-remix-local-123",
                        "api_key_id": "key-openai-video-remix-local-123",
                        "access_allowed": true
                    },
                    "public_path": "/v1/videos/task-local-123/remix"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-sync",
            any(move |_request: Request| {
                let decision_hits_inner = Arc::clone(&decision_hits_clone);
                async move {
                    *decision_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/plan-sync",
            any(move |_request: Request| {
                let plan_hits_inner = Arc::clone(&plan_hits_clone);
                async move {
                    *plan_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
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
            "/v1/videos/task-local-123/remix",
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
                    prompt: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("prompt"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
                Json(json!({
                    "request_id": "trace-openai-video-remix-local-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "ext-remix-task-123",
                            "status": "submitted"
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 23
                    }
                }))
            }
        }),
    );

    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-local-123".to_string(),
            short_id: Some("task-local-123".to_string()),
            request_id: "request-openai-video-remix-local-123".to_string(),
            user_id: Some("user-openai-video-remix-local-123".to_string()),
            api_key_id: Some("key-openai-video-remix-local-123".to_string()),
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
            prompt: Some("original prompt".to_string()),
            original_request_body: Some(json!({"prompt": "original prompt"})),
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
            poll_count: 1,
            max_poll_count: 360,
            created_at_unix_ms: 123,
            submitted_at_unix_secs: Some(123),
            completed_at_unix_secs: Some(124),
            updated_at_unix_secs: 124,
            error_code: None,
            error_message: None,
            video_url: Some("https://cdn.example.com/original.mp4".to_string()),
            request_metadata: Some(json!({
                "rust_local_snapshot": {
                    "OpenAi": {
                        "local_task_id": "task-local-123",
                        "upstream_task_id": "ext-video-task-123",
                        "created_at_unix_ms": 123,
                        "user_id": "user-openai-video-remix-local-123",
                        "api_key_id": "key-openai-video-remix-local-123",
                        "model": "sora-2",
                        "prompt": "original prompt",
                        "size": "1280x720",
                        "seconds": "4",
                        "remixed_from_video_id": null,
                        "status": "Completed",
                        "progress_percent": 100,
                        "completed_at_unix_secs": 124,
                        "expires_at_unix_secs": null,
                        "error_code": null,
                        "error_message": null,
                        "video_url": "https://cdn.example.com/original.mp4",
                        "persistence": {
                            "request_id": "request-openai-video-remix-local-123",
                            "username": "video-user",
                            "api_key_name": "video-key",
                            "client_api_format": "openai:video",
                            "provider_api_format": "openai:video",
                            "original_request_body": {
                                "prompt": "original prompt"
                            },
                            "format_converted": false
                        },
                        "transport": {
                            "upstream_base_url": "https://api.openai.example/v1",
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
        })
        .await
        .expect("upsert should succeed");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
        crate::data::GatewayDataState::with_video_task_and_request_candidate_repository_for_tests(
            repository,
            Arc::clone(&request_candidate_repository),
        ),
    );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/videos/task-local-123/remix"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(TRACE_ID_HEADER, "trace-openai-video-remix-local-123")
        .body("{\"prompt\":\"remix this\",\"model\":\"sora-2\"}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(body.get("object"), Some(&json!("video")));
    assert_eq!(body.get("status"), Some(&json!("queued")));
    assert_eq!(body.get("prompt"), Some(&json!("remix this")));
    assert_eq!(
        body.get("remixed_from_video_id"),
        Some(&json!("task-local-123"))
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(seen_execution_runtime_request.method, "POST");
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.openai.example/v1/videos/ext-video-task-123/remix"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-video"
    );
    assert_eq!(seen_execution_runtime_request.prompt, "remix this");

    let stored_candidates = request_candidate_repository
        .list_by_request_id("request-openai-video-remix-local-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
