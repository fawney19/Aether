use std::sync::{Arc, Mutex};

use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::video_tasks::InMemoryVideoTaskRepository;
use aether_data_contracts::repository::provider_catalog::ProviderCatalogReadRepository;
use aether_data_contracts::repository::video_tasks::{
    UpsertVideoTask, VideoTaskStatus, VideoTaskWriteRepository,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::response::Response;
use axum::routing::{any, get, post};
use axum::{extract::Request, Json, Router};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde_json::json;

use super::super::{
    build_router_with_state, build_state_with_execution_runtime_override, sample_endpoint,
    sample_key, sample_provider, start_server, AppState,
};
use crate::admin_api::{
    maybe_build_local_admin_video_tasks_response, AdminAppState, AdminRequestContext,
};
use crate::audit::AdminAuditEvent;
use crate::constants::{
    GATEWAY_HEADER, TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER, TRUSTED_ADMIN_SESSION_ID_HEADER,
    TRUSTED_ADMIN_USER_ID_HEADER, TRUSTED_ADMIN_USER_ROLE_HEADER,
};
use crate::control::resolve_public_request_context;
use crate::data::GatewayDataState;

fn trusted_admin_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(GATEWAY_HEADER, HeaderValue::from_static("rust-phase3b"));
    headers.insert(
        TRUSTED_ADMIN_USER_ID_HEADER,
        HeaderValue::from_static("admin-user-123"),
    );
    headers.insert(
        TRUSTED_ADMIN_USER_ROLE_HEADER,
        HeaderValue::from_static("admin"),
    );
    headers.insert(
        TRUSTED_ADMIN_SESSION_ID_HEADER,
        HeaderValue::from_static("session-123"),
    );
    headers.insert(
        TRUSTED_ADMIN_MANAGEMENT_TOKEN_ID_HEADER,
        HeaderValue::from_static("management-token-123"),
    );
    headers
}

async fn local_admin_video_tasks_response(
    state: &AppState,
    method: http::Method,
    uri: &str,
    _body: Option<serde_json::Value>,
) -> axum::response::Response<Body> {
    let headers = trusted_admin_headers();
    let request_context = resolve_public_request_context(
        state,
        &method,
        &uri.parse().expect("uri should parse"),
        &headers,
        "trace-123",
    )
    .await
    .expect("request context should resolve");
    maybe_build_local_admin_video_tasks_response(
        &AdminAppState::new(state),
        &AdminRequestContext::new(&request_context),
    )
    .await
    .expect("local video tasks response should build")
    .expect("video tasks route should resolve locally")
}

fn sample_admin_video_task(
    id: &str,
    status: VideoTaskStatus,
    created_at_unix_secs: u64,
    user_id: &str,
    username: &str,
    provider_id: &str,
    model: &str,
    prompt: &str,
) -> UpsertVideoTask {
    UpsertVideoTask {
        id: id.to_string(),
        short_id: Some(format!("short-{id}")),
        request_id: format!("request-{id}"),
        user_id: Some(user_id.to_string()),
        api_key_id: Some(format!("api-key-{id}")),
        username: Some(username.to_string()),
        api_key_name: Some("primary".to_string()),
        external_task_id: Some(format!("ext-{id}")),
        provider_id: Some(provider_id.to_string()),
        endpoint_id: Some("endpoint-1".to_string()),
        key_id: Some("provider-key-1".to_string()),
        client_api_format: Some("openai:video".to_string()),
        provider_api_format: Some("openai:video".to_string()),
        format_converted: false,
        model: Some(model.to_string()),
        prompt: Some(prompt.to_string()),
        original_request_body: Some(json!({ "prompt": prompt })),
        duration_seconds: Some(4),
        resolution: Some("720p".to_string()),
        aspect_ratio: Some("16:9".to_string()),
        size: Some("1280x720".to_string()),
        status,
        progress_percent: if matches!(status, VideoTaskStatus::Completed) {
            100
        } else {
            50
        },
        progress_message: Some("ok".to_string()),
        retry_count: 0,
        poll_interval_seconds: 10,
        next_poll_at_unix_secs: None,
        poll_count: 1,
        max_poll_count: 360,
        created_at_unix_secs,
        submitted_at_unix_secs: Some(created_at_unix_secs),
        completed_at_unix_secs: if matches!(status, VideoTaskStatus::Completed) {
            Some(created_at_unix_secs + 30)
        } else {
            None
        },
        updated_at_unix_secs: created_at_unix_secs + 5,
        error_code: None,
        error_message: None,
        video_url: Some(format!("https://example.com/{id}.mp4")),
        request_metadata: None,
    }
}

#[tokio::test]
async fn gateway_handles_admin_video_tasks_list_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/video-tasks",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_admin_video_task(
            "task-completed",
            VideoTaskStatus::Completed,
            1_710_000_100,
            "user-1",
            "alice",
            "provider-openai",
            "gpt-video",
            &"x".repeat(120),
        ))
        .await
        .expect("task should upsert");
    repository
        .upsert(sample_admin_video_task(
            "task-processing",
            VideoTaskStatus::Processing,
            1_710_000_200,
            "user-2",
            "bob",
            "provider-anthropic",
            "claude-video",
            "short prompt",
        ))
        .await
        .expect("task should upsert");

    let provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository> =
        Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![
                sample_provider("provider-openai", "OpenAI", 10),
                sample_provider("provider-anthropic", "Anthropic", 20),
            ],
            vec![],
            vec![],
        ));
    let data_state = GatewayDataState::with_video_task_repository_and_provider_transport_for_tests(
        Arc::clone(&repository),
        provider_catalog_repository,
        DEVELOPMENT_ENCRYPTION_KEY,
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/video-tasks?status=completed&page=1&page_size=20"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], json!(1));
    assert_eq!(payload["page"], json!(1));
    assert_eq!(payload["page_size"], json!(20));
    assert_eq!(payload["pages"], json!(1));
    assert_eq!(payload["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(payload["items"][0]["id"], "task-completed");
    assert_eq!(payload["items"][0]["username"], "alice");
    assert_eq!(payload["items"][0]["provider_name"], "OpenAI");
    assert_eq!(payload["items"][0]["status"], "completed");
    assert!(payload["items"][0]["prompt"]
        .as_str()
        .is_some_and(|value| value.ends_with("...")));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_video_tasks_stats_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/video-tasks/stats",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_admin_video_task(
            "task-processing",
            VideoTaskStatus::Processing,
            1_710_000_000,
            "user-1",
            "alice",
            "provider-openai",
            "gpt-video",
            "prompt one",
        ))
        .await
        .expect("task should upsert");
    repository
        .upsert(sample_admin_video_task(
            "task-completed",
            VideoTaskStatus::Completed,
            1_710_000_100,
            "user-2",
            "bob",
            "provider-openai",
            "gpt-video",
            "prompt two",
        ))
        .await
        .expect("task should upsert");
    repository
        .upsert(sample_admin_video_task(
            "task-failed",
            VideoTaskStatus::Failed,
            1_710_000_200,
            "user-2",
            "bob",
            "provider-openai",
            "veo-2",
            "prompt three",
        ))
        .await
        .expect("task should upsert");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_video_task_data_repository_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/video-tasks/stats"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["total"], json!(3));
    assert_eq!(payload["today_count"], json!(0));
    assert_eq!(payload["active_users"], json!(2));
    assert_eq!(payload["processing_count"], json!(1));
    assert_eq!(payload["by_status"]["processing"], json!(1));
    assert_eq!(payload["by_status"]["completed"], json!(1));
    assert_eq!(payload["by_status"]["failed"], json!(1));
    assert_eq!(payload["by_model"]["gpt-video"], json!(2));
    assert_eq!(payload["by_model"]["veo-2"], json!(1));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_admin_video_task_detail_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/video-tasks/task-detail",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_admin_video_task(
            "task-detail",
            VideoTaskStatus::Completed,
            1_710_000_300,
            "user-9",
            "charlie",
            "provider-openai",
            "gpt-video",
            "detail prompt",
        ))
        .await
        .expect("task should upsert");

    let provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository> =
        Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-openai", "OpenAI", 10)],
            vec![sample_endpoint(
                "endpoint-1",
                "provider-openai",
                "openai:video",
                "https://api.openai.example",
            )],
            vec![],
        ));
    let data_state = GatewayDataState::with_video_task_repository_and_provider_transport_for_tests(
        Arc::clone(&repository),
        provider_catalog_repository,
        DEVELOPMENT_ENCRYPTION_KEY,
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/video-tasks/task-detail"))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], "task-detail");
    assert_eq!(payload["username"], "charlie");
    assert_eq!(payload["provider_name"], "OpenAI");
    assert_eq!(payload["endpoint"]["id"], "endpoint-1");
    assert_eq!(payload["endpoint"]["api_format"], "openai:video");
    assert_eq!(payload["status"], "completed");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn local_admin_video_task_detail_attaches_explicit_audit() {
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_admin_video_task(
            "task-detail-audit",
            VideoTaskStatus::Completed,
            1_710_000_350,
            "user-3",
            "dana",
            "provider-openai",
            "gpt-video",
            "detail audit prompt",
        ))
        .await
        .expect("task should upsert");

    let provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository> =
        Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-openai", "OpenAI", 10)],
            vec![sample_endpoint(
                "endpoint-1",
                "provider-openai",
                "openai:video",
                "https://api.openai.example",
            )],
            vec![],
        ));
    let data_state = GatewayDataState::with_video_task_repository_and_provider_transport_for_tests(
        Arc::clone(&repository),
        provider_catalog_repository,
        DEVELOPMENT_ENCRYPTION_KEY,
    );
    let state = AppState::new()
        .expect("gateway state should build")
        .with_data_state_for_tests(data_state);

    let response = local_admin_video_tasks_response(
        &state,
        http::Method::GET,
        "/api/admin/video-tasks/task-detail-audit",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("video task detail should attach audit");
    assert_eq!(audit.event_name, "admin_video_task_detail_viewed");
    assert_eq!(audit.action, "view_video_task_detail");
    assert_eq!(audit.target_type, "video_task");
    assert_eq!(audit.target_id, "task-detail-audit");
}

#[tokio::test]
async fn gateway_cancels_admin_video_task_locally_with_trusted_admin_principal() {
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct SeenExecutionRuntimeSyncRequest {
        method: String,
        url: String,
        authorization: String,
    }

    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/video-tasks/task-openai-cancel/cancel",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (_parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("payload should parse");
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
                    "request_id": "cancel-openai-task-1",
                    "status_code": 204,
                    "headers": {},
                    "telemetry": {
                        "elapsed_ms": 11
                    }
                }))
            }
        }),
    );

    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    let mut task = sample_admin_video_task(
        "task-openai-cancel",
        VideoTaskStatus::Processing,
        1_710_000_400,
        "user-1",
        "alice",
        "provider-openai",
        "sora-2",
        "cancel prompt",
    );
    task.external_task_id = Some("ext-video-task-123".to_string());
    task.request_metadata = Some(json!({
        "rust_local_snapshot": {
            "OpenAi": {
                "local_task_id": "task-openai-cancel",
                "upstream_task_id": "ext-video-task-123",
                "created_at_unix_secs": 1710000400,
                "user_id": "user-1",
                "api_key_id": "api-key-task-openai-cancel",
                "model": "sora-2",
                "prompt": "cancel prompt",
                "size": "1280x720",
                "seconds": "4",
                "remixed_from_video_id": null,
                "status": "Processing",
                "progress_percent": 50,
                "completed_at_unix_secs": null,
                "expires_at_unix_secs": null,
                "error_code": null,
                "error_message": null,
                "video_url": null,
                "persistence": {
                    "request_id": "request-task-openai-cancel",
                    "username": "alice",
                    "api_key_name": "primary",
                    "client_api_format": "openai:video",
                    "provider_api_format": "openai:video",
                    "original_request_body": {
                        "prompt": "cancel prompt"
                    },
                    "format_converted": false
                },
                "transport": {
                    "upstream_base_url": "https://api.openai.example",
                    "provider_name": "openai-video",
                    "provider_id": "provider-openai",
                    "endpoint_id": "endpoint-1",
                    "key_id": "provider-key-1",
                    "headers": {
                        "authorization": "Bearer sk-upstream-openai-video",
                        "content-type": "application/json"
                    },
                    "content_type": "application/json",
                    "model_name": "sora-2-upstream",
                    "proxy": null,
                    "tls_profile": null,
                    "timeouts": null
                }
            }
        }
    }));
    repository
        .upsert(task)
        .await
        .expect("upsert should succeed");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_video_task_data_repository_for_tests(Arc::clone(&repository)),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/admin/video-tasks/task-openai-cancel/cancel"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .json::<serde_json::Value>()
            .await
            .expect("body should parse"),
        json!({
            "id": "task-openai-cancel",
            "status": "cancelled",
            "message": "Task cancelled successfully",
        })
    );

    let detail = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/video-tasks/task-openai-cancel"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("detail request should succeed");
    assert_eq!(detail.status(), StatusCode::OK);
    let detail_json: serde_json::Value = detail.json().await.expect("detail should parse");
    assert_eq!(detail_json["status"], "cancelled");
    assert_eq!(
        detail_json["request_metadata"]["rust_local_snapshot"]["OpenAi"]["status"],
        "Cancelled"
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime should be called");
    assert_eq!(seen_execution_runtime_request.method, "DELETE");
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.openai.example/v1/videos/ext-video-task-123"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-video"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn local_admin_video_task_cancel_attaches_explicit_audit() {
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    let mut task = sample_admin_video_task(
        "task-cancel-audit",
        VideoTaskStatus::Processing,
        1_710_000_450,
        "user-4",
        "erin",
        "provider-openai",
        "gpt-video",
        "cancel audit prompt",
    );
    task.client_api_format = None;
    task.provider_api_format = None;
    task.request_metadata = Some(json!({}));
    repository.upsert(task).await.expect("task should upsert");

    let state = AppState::new()
        .expect("gateway state should build")
        .with_video_task_data_repository_for_tests(repository);

    let response = local_admin_video_tasks_response(
        &state,
        http::Method::POST,
        "/api/admin/video-tasks/task-cancel-audit/cancel",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("video task cancel should attach audit");
    assert_eq!(audit.event_name, "admin_video_task_cancelled");
    assert_eq!(audit.action, "cancel_video_task");
    assert_eq!(audit.target_type, "video_task");
    assert_eq!(audit.target_id, "task-cancel-audit");
}

#[tokio::test]
async fn gateway_redirects_admin_video_task_video_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/admin/video-tasks/task-redirect/video",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_admin_video_task(
            "task-redirect",
            VideoTaskStatus::Completed,
            1_710_000_500,
            "user-1",
            "alice",
            "provider-openai",
            "gpt-video",
            "redirect prompt",
        ))
        .await
        .expect("task should upsert");

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_video_task_data_repository_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client should build");
    let response = client
        .get(format!(
            "{gateway_url}/api/admin/video-tasks/task-redirect/video"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        response
            .headers()
            .get(http::header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("https://example.com/task-redirect.mp4")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn local_admin_video_task_video_redirect_attaches_explicit_audit() {
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(sample_admin_video_task(
            "task-video-audit",
            VideoTaskStatus::Completed,
            1_710_000_550,
            "user-5",
            "frank",
            "provider-openai",
            "gpt-video",
            "video audit prompt",
        ))
        .await
        .expect("task should upsert");

    let state = AppState::new()
        .expect("gateway state should build")
        .with_video_task_data_repository_for_tests(repository);

    let response = local_admin_video_tasks_response(
        &state,
        http::Method::GET,
        "/api/admin/video-tasks/task-video-audit/video",
        None,
    )
    .await;

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    let audit = response
        .extensions()
        .get::<AdminAuditEvent>()
        .cloned()
        .expect("video task video should attach audit");
    assert_eq!(audit.event_name, "admin_video_task_video_viewed");
    assert_eq!(audit.action, "view_video_task_video");
    assert_eq!(audit.target_type, "video_task_video");
    assert_eq!(audit.target_id, "task-video-audit");
}

#[tokio::test]
async fn gateway_proxies_admin_video_task_video_locally_with_trusted_admin_principal() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let seen_api_key = Arc::new(Mutex::new(None::<String>));
    let seen_api_key_clone = Arc::clone(&seen_api_key);
    let upstream = Router::new()
        .route(
            "/api/admin/video-tasks/task-proxy/video",
            any(move |_request: Request| {
                let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
                async move {
                    *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        )
        .route(
            "/generativelanguage.googleapis.com/v1beta/files/video-task-123:download",
            any(move |request: Request| {
                let seen_api_key_inner = Arc::clone(&seen_api_key_clone);
                async move {
                    *seen_api_key_inner.lock().expect("mutex should lock") = request
                        .headers()
                        .get("x-goog-api-key")
                        .and_then(|value| value.to_str().ok())
                        .map(ToOwned::to_owned);
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("proxied-video-bytes"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("video/mp4"),
                    );
                    response
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    let mut task = sample_admin_video_task(
        "task-proxy",
        VideoTaskStatus::Completed,
        1_710_000_600,
        "user-1",
        "alice",
        "provider-gemini",
        "veo-3",
        "proxy prompt",
    );
    task.provider_id = Some("provider-gemini".to_string());
    task.endpoint_id = Some("endpoint-gemini".to_string());
    task.key_id = Some("key-gemini".to_string());
    task.client_api_format = Some("gemini:video".to_string());
    task.provider_api_format = Some("gemini:video".to_string());
    task.video_url = Some(format!(
        "{upstream_url}/generativelanguage.googleapis.com/v1beta/files/video-task-123:download"
    ));
    repository.upsert(task).await.expect("task should upsert");

    let provider_catalog_repository: Arc<dyn ProviderCatalogReadRepository> =
        Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![sample_provider("provider-gemini", "Gemini", 10)],
            vec![sample_endpoint(
                "endpoint-gemini",
                "provider-gemini",
                "gemini:video",
                "https://generativelanguage.googleapis.com",
            )],
            vec![sample_key(
                "key-gemini",
                "provider-gemini",
                "gemini:video",
                "gemini-upstream-secret",
            )],
        ));
    let data_state = GatewayDataState::with_video_task_repository_and_provider_transport_for_tests(
        Arc::clone(&repository),
        provider_catalog_repository,
        DEVELOPMENT_ENCRYPTION_KEY,
    );

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/api/admin/video-tasks/task-proxy/video"
        ))
        .header(crate::constants::GATEWAY_HEADER, "rust-phase3b")
        .header(TRUSTED_ADMIN_USER_ID_HEADER, "admin-user-123")
        .header(TRUSTED_ADMIN_USER_ROLE_HEADER, "admin")
        .header(TRUSTED_ADMIN_SESSION_ID_HEADER, "session-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("video/mp4")
    );
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("inline; filename=\"video_task-proxy.mp4\"")
    );
    assert_eq!(
        response.bytes().await.expect("body should read"),
        Bytes::from_static(b"proxied-video-bytes")
    );
    assert_eq!(
        seen_api_key.lock().expect("mutex should lock").as_deref(),
        Some("gemini-upstream-secret")
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
