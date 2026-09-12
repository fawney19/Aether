use std::sync::{Arc, Mutex};

use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskStatus, VideoTaskWriteRepository,
};
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{build_router_with_state, build_state_with_execution_runtime_override, start_server};

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
        None,
        None,
        None,
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        None,
        None,
        None,
    )
    .expect("auth snapshot should build")
}

#[tokio::test]
async fn gateway_reads_openai_video_task_via_data_read_side_without_hitting_public_route() {
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
                        "user_id": "user-video-db-123",
                        "api_key_id": "key-video-db-123",
                        "access_allowed": true
                    },
                    "public_path": "/v1/videos/task-db-123"
                }))
            }),
        )
        .route(
            "/v1/videos/task-db-123",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-db-123".to_string(),
            short_id: Some("short-task-db-123".to_string()),
            request_id: "request-db-123".to_string(),
            user_id: Some("user-video-db-123".to_string()),
            api_key_id: Some("api-key-video-db-123".to_string()),
            username: Some("video-user".to_string()),
            api_key_name: Some("video-key".to_string()),
            external_task_id: Some("ext-video-db-123".to_string()),
            provider_id: Some("provider-video-db-123".to_string()),
            endpoint_id: Some("endpoint-video-db-123".to_string()),
            key_id: Some("provider-key-video-db-123".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("hello from db".to_string()),
            original_request_body: Some(json!({"prompt": "hello from db"})),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: VideoTaskStatus::Processing,
            progress_percent: 45,
            progress_message: Some("working".to_string()),
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: Some(124),
            poll_count: 1,
            max_poll_count: 360,
            created_at_unix_ms: 123,
            submitted_at_unix_secs: Some(123),
            completed_at_unix_secs: None,
            updated_at_unix_secs: 124,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: None,
        })
        .await
        .expect("upsert should succeed");
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("client-video-read-owner-key")),
        sample_auth_snapshot("api-key-video-db-rotated-123", "user-video-db-123"),
    )]));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(upstream_url.clone())
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_video_task_repository_for_tests(
                    auth_repository,
                    repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/v1/videos/task-db-123"))
        .bearer_auth("client-video-read-owner-key")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(body["id"], "task-db-123");
    assert_eq!(body["status"], "processing");
    assert_eq!(body["progress"], 45);
    assert_eq!(body["created_at"], 123);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_reads_gemini_video_task_via_data_read_side_without_hitting_public_route() {
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/resolve",
            any(|_request: Request| async move {
                Json(json!({
                    "action": "proxy_public",
                    "route_class": "ai_public",
                    "route_family": "gemini",
                    "route_kind": "video",
                    "auth_endpoint_signature": "gemini:video",
                    "execution_runtime_candidate": true,
                    "auth_context": {
                        "user_id": "user-video-db-123",
                        "api_key_id": "key-video-db-123",
                        "access_allowed": true
                    },
                    "public_path": "/v1beta/models/veo-3/operations/localshort123"
                }))
            }),
        )
        .route(
            "/v1beta/models/veo-3/operations/localshort123",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-db-456".to_string(),
            short_id: Some("localshort123".to_string()),
            request_id: "request-db-456".to_string(),
            user_id: Some("user-video-db-123".to_string()),
            api_key_id: Some("api-key-video-db-123".to_string()),
            username: Some("video-user".to_string()),
            api_key_name: Some("video-key".to_string()),
            external_task_id: Some("operations/ext-video-db-123".to_string()),
            provider_id: Some("provider-video-db-123".to_string()),
            endpoint_id: Some("endpoint-video-db-123".to_string()),
            key_id: Some("provider-key-video-db-123".to_string()),
            client_api_format: Some("gemini:video".to_string()),
            provider_api_format: Some("gemini:video".to_string()),
            format_converted: false,
            model: Some("veo-3".to_string()),
            prompt: Some("hello from gemini db".to_string()),
            original_request_body: Some(json!({"prompt": "hello from gemini db"})),
            duration_seconds: Some(8),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("720p".to_string()),
            status: VideoTaskStatus::Completed,
            progress_percent: 100,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: None,
            poll_count: 3,
            max_poll_count: 360,
            created_at_unix_ms: 223,
            submitted_at_unix_secs: Some(223),
            completed_at_unix_secs: Some(224),
            updated_at_unix_secs: 224,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: Some(json!({
                "rust_local_snapshot": {
                    "metadata": {
                        "generateVideoResponse": {
                            "generatedSamples": [
                                {
                                    "video": {
                                        "uri": "/v1beta/files/aev_localshort123:download?alt=media"
                                    }
                                }
                            ]
                        }
                    }
                }
            })),
        })
        .await
        .expect("upsert should succeed");
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("client-gemini-video-read-owner-key")),
        sample_auth_snapshot("api-key-video-db-rotated-123", "user-video-db-123"),
    )]));

    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(upstream_url.clone())
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_video_task_repository_for_tests(
                    auth_repository,
                    repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/v1beta/models/veo-3/operations/localshort123"
        ))
        .header("x-goog-api-key", "client-gemini-video-read-owner-key")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(body["name"], "models/veo-3/operations/localshort123");
    assert_eq!(body["done"], true);
    assert_eq!(
        body["response"]["generateVideoResponse"]["generatedSamples"][0]["video"]["uri"],
        "/v1beta/files/aev_localshort123:download?alt=media"
    );
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_hides_data_backed_video_task_from_non_owner() {
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
                        "user_id": "user-video-foreign",
                        "api_key_id": "key-video-foreign",
                        "access_allowed": true
                    },
                    "public_path": "/v1/videos/task-owned-123"
                }))
            }),
        )
        .route(
            "/v1/videos/task-owned-123",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: "task-owned-123".to_string(),
            short_id: None,
            request_id: "request-owned-123".to_string(),
            user_id: Some("user-video-owner".to_string()),
            api_key_id: Some("key-video-owner".to_string()),
            username: None,
            api_key_name: None,
            external_task_id: Some("ext-owned-123".to_string()),
            provider_id: Some("provider-owned-123".to_string()),
            endpoint_id: Some("endpoint-owned-123".to_string()),
            key_id: Some("provider-key-owned-123".to_string()),
            client_api_format: Some("openai:video".to_string()),
            provider_api_format: Some("openai:video".to_string()),
            format_converted: false,
            model: Some("sora-2".to_string()),
            prompt: Some("private video".to_string()),
            original_request_body: Some(json!({"prompt": "private video"})),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: VideoTaskStatus::Processing,
            progress_percent: 50,
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: Some(124),
            poll_count: 1,
            max_poll_count: 360,
            created_at_unix_ms: 123,
            submitted_at_unix_secs: Some(123),
            completed_at_unix_secs: None,
            updated_at_unix_secs: 124,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: None,
        })
        .await
        .expect("upsert should succeed");
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("client-video-read-foreign-key")),
        sample_auth_snapshot("key-video-foreign", "user-video-foreign"),
    )]));
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(upstream_url.clone())
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_and_video_task_repository_for_tests(
                    auth_repository,
                    repository,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/v1/videos/task-owned-123"))
        .bearer_auth("client-video-read-foreign-key")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .json::<serde_json::Value>()
            .await
            .expect("json body"),
        json!({"detail": "Video task not found"})
    );
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
