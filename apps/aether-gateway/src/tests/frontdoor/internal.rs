use super::{
    hash_api_key, run_frontdoor_async_test, sample_endpoint, sample_key,
    sample_models_candidate_row, sample_provider, unrestricted_models_snapshot,
    InMemoryAuthApiKeySnapshotRepository, InMemoryMinimalCandidateSelectionReadRepository,
    InMemoryProviderCatalogReadRepository, InMemoryRequestCandidateRepository,
    InMemoryVideoTaskRepository, UpsertVideoTask, VideoTaskStatus, VideoTaskWriteRepository,
    DEVELOPMENT_ENCRYPTION_KEY,
};
use crate::data::GatewayDataState;
use crate::tests::{
    any, build_router, build_router_with_state, build_state_with_execution_runtime_override, json,
    start_server, strip_sse_keepalive_comments, AppState, Arc, Body, HeaderValue, Json, Mutex,
    Request, Response, Router, StatusCode, CONTROL_ACTION_PROXY_PUBLIC, CONTROL_EXECUTED_HEADER,
    EXECUTION_PATH_EXECUTION_RUNTIME_STREAM, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
    EXECUTION_PATH_HEADER,
};
use aether_data::repository::billing::InMemoryBillingReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use aether_data::repository::wallet::{InMemoryWalletRepository, StoredWalletSnapshot};
use base64::Engine as _;

const INTERNAL_REPORT_CAPABILITY_FIELD: &str = "_aether_internal_report_capability";
const INTERNAL_REPORT_CLIENT_KEY: &str = "sk-internal-report-capability";
const INTERNAL_REPORT_USER_ID: &str = "user-internal-report-capability";
const INTERNAL_REPORT_API_KEY_ID: &str = "api-key-internal-report-capability";

fn internal_report_planner_state() -> AppState {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(INTERNAL_REPORT_CLIENT_KEY)),
        unrestricted_models_snapshot(INTERNAL_REPORT_API_KEY_ID, INTERNAL_REPORT_USER_ID),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-internal-report",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-internal-report", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-internal-report",
            "provider-internal-report",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-internal-report",
            "provider-internal-report",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_repository,
                provider_catalog_repository,
                request_candidate_repository,
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
}

fn internal_video_create_planner_state(api_format: &str, model: &str) -> AppState {
    let family = api_format
        .split_once(':')
        .map(|(family, _)| family)
        .expect("video api format should contain a family");
    let provider_id = format!("provider-internal-{family}-video");
    let mut candidate = sample_models_candidate_row(&provider_id, family, api_format, model, 10);
    candidate.endpoint_api_family = Some(family.to_string());
    candidate.endpoint_kind = Some("video".to_string());
    candidate.global_model_supports_streaming = Some(false);
    candidate.model_supports_streaming = Some(false);

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(INTERNAL_REPORT_CLIENT_KEY)),
        unrestricted_models_snapshot(INTERNAL_REPORT_API_KEY_ID, INTERNAL_REPORT_USER_ID),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            candidate,
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider(&provider_id, family, 10)],
        vec![sample_endpoint(
            &format!("endpoint-{provider_id}"),
            &provider_id,
            api_format,
            if family == "gemini" {
                "https://generativelanguage.googleapis.com"
            } else {
                "https://api.openai.example"
            },
        )],
        vec![sample_key(
            &format!("key-{provider_id}"),
            &provider_id,
            api_format,
            "sk-upstream-video",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    AppState::new()
        .expect("state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                auth_repository,
                candidate_repository,
                provider_catalog_repository,
                request_candidate_repository,
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
}

async fn internal_video_followup_planner_state(
    api_format: &str,
    task_id: &str,
    short_id: Option<&str>,
    external_task_id: &str,
    model: &str,
) -> AppState {
    let family = api_format
        .split_once(':')
        .map(|(family, _)| family)
        .expect("video api format should contain a family");
    let provider_id = format!("provider-internal-{family}-video-followup");
    let endpoint_id = format!("endpoint-{provider_id}");
    let key_id = format!("key-{provider_id}");
    let repository = Arc::new(InMemoryVideoTaskRepository::default());
    repository
        .upsert(UpsertVideoTask {
            id: task_id.to_string(),
            short_id: short_id.map(ToOwned::to_owned),
            request_id: format!("request-{task_id}"),
            user_id: Some(INTERNAL_REPORT_USER_ID.to_string()),
            api_key_id: Some(INTERNAL_REPORT_API_KEY_ID.to_string()),
            username: Some("alice".to_string()),
            api_key_name: Some("default".to_string()),
            external_task_id: Some(external_task_id.to_string()),
            provider_id: Some(provider_id.clone()),
            endpoint_id: Some(endpoint_id.clone()),
            key_id: Some(key_id.clone()),
            client_api_format: Some(api_format.to_string()),
            provider_api_format: Some(api_format.to_string()),
            format_converted: false,
            model: Some(model.to_string()),
            prompt: Some("internal capability video".to_string()),
            original_request_body: Some(json!({
                "model": model,
                "prompt": "internal capability video",
            })),
            duration_seconds: Some(4),
            resolution: Some("720p".to_string()),
            aspect_ratio: Some("16:9".to_string()),
            size: Some("1280x720".to_string()),
            status: if family == "openai" {
                VideoTaskStatus::Completed
            } else {
                VideoTaskStatus::Submitted
            },
            progress_percent: if family == "openai" { 100 } else { 0 },
            progress_message: None,
            retry_count: 0,
            poll_interval_seconds: 10,
            next_poll_at_unix_secs: (family != "openai").then_some(1_700_000_010),
            poll_count: 0,
            max_poll_count: 360,
            created_at_unix_ms: 1_700_000_000_000,
            submitted_at_unix_secs: Some(1_700_000_000),
            completed_at_unix_secs: (family == "openai").then_some(1_700_000_100),
            updated_at_unix_secs: 1_700_000_000,
            error_code: None,
            error_message: None,
            video_url: None,
            request_metadata: None,
        })
        .await
        .expect("video task should seed");

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(INTERNAL_REPORT_CLIENT_KEY)),
        unrestricted_models_snapshot(INTERNAL_REPORT_API_KEY_ID, INTERNAL_REPORT_USER_ID),
    )]));
    let provider_catalog_repository = crate::tests::video::video_provider_catalog_repository(
        &provider_id,
        family,
        &endpoint_id,
        api_format,
        if family == "gemini" {
            "https://generativelanguage.googleapis.com"
        } else {
            "https://api.openai.example/v1"
        },
        &key_id,
        "sk-upstream-video",
    );
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let data_state = crate::data::GatewayDataState::with_video_task_provider_transport_and_request_candidate_repository_for_tests(
        repository,
        provider_catalog_repository,
        request_candidate_repository,
        DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_auth_api_key_reader(auth_repository);

    AppState::new()
        .expect("state should build")
        .with_video_task_truth_source_mode(crate::VideoTaskTruthSourceMode::RustAuthoritative)
        .with_data_state_for_tests(data_state)
}

async fn issue_internal_gateway_report_capability(
    client: &reqwest::Client,
    gateway_url: &str,
    endpoint: &str,
    trace_id: &str,
    method: &str,
    path: &str,
    request_headers: serde_json::Value,
    body_json: serde_json::Value,
) -> (String, serde_json::Value) {
    let response = client
        .post(format!("{gateway_url}/api/internal/gateway/{endpoint}"))
        .json(&json!({
            "trace_id": trace_id,
            "method": method,
            "path": path,
            "headers": request_headers,
            "body_json": body_json,
        }))
        .send()
        .await
        .expect("planner request should succeed");
    let status = response.status();
    let payload: serde_json::Value = response.json().await.expect("planner body should parse");
    assert_eq!(
        status,
        StatusCode::OK,
        "planner should issue a report capability: {payload}"
    );
    let report_kind = payload["report_kind"]
        .as_str()
        .expect("planner should return a report kind")
        .to_string();
    let report_context = payload["report_context"].clone();
    assert!(
        report_context[INTERNAL_REPORT_CAPABILITY_FIELD]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "planner should return an opaque report capability: {payload}"
    );
    (report_kind, report_context)
}

async fn issue_openai_chat_report_capability(
    client: &reqwest::Client,
    gateway_url: &str,
    endpoint: &str,
    trace_id: &str,
    stream: bool,
) -> (String, serde_json::Value) {
    issue_internal_gateway_report_capability(
        client,
        gateway_url,
        endpoint,
        trace_id,
        "POST",
        "/v1/chat/completions",
        json!({
            "content-type": "application/json",
            "x-api-key": INTERNAL_REPORT_CLIENT_KEY,
        }),
        json!({
            "model": "gpt-5",
            "messages": [],
            "stream": stream,
        }),
    )
    .await
}

async fn post_internal_sync_report(
    client: &reqwest::Client,
    gateway_url: &str,
    trace_id: &str,
    report_kind: &str,
    report_context: serde_json::Value,
) -> reqwest::Response {
    client
        .post(format!("{gateway_url}/api/internal/gateway/report-sync"))
        .json(&json!({
            "trace_id": trace_id,
            "report_kind": report_kind,
            "report_context": report_context,
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "id": "chatcmpl-internal-capability",
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2,
                    "total_tokens": 3,
                }
            }
        }))
        .send()
        .await
        .expect("report request should succeed")
}

async fn assert_internal_report_capability_rejected(response: reqwest::Response) {
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "detail": "internal gateway report context does not carry a valid planner capability",
        })
    );
}

async fn assert_supplied_auth_context_rejected(response: reqwest::Response) {
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "detail": "supplied auth_context is not accepted; authenticate through request headers",
        })
    );
}

#[tokio::test]
async fn gateway_handles_internal_gateway_resolve_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/resolve"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["route_class"], "ai_public");
    assert_eq!(payload["route_family"], "openai");
    assert_eq!(payload["route_kind"], "chat");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/resolve"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/messages",
            "headers": {
                "authorization": "Bearer local-token",
            },
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["route_class"], "ai_public");
    assert_eq!(payload["route_family"], "claude");
    assert_eq!(payload["route_kind"], "messages");
    assert_eq!(payload["request_auth_channel"], "bearer_like");
    assert_eq!(payload["auth_endpoint_signature"], "claude:messages");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_internal_gateway_proxy_public_action_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/execute-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/execute-sync"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {},
            "body_json": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], CONTROL_ACTION_PROXY_PUBLIC);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_internal_gateway_plan_sync_proxy_public_action_without_proxying_upstream()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/plan-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/plan-sync"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {},
            "body_json": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], CONTROL_ACTION_PROXY_PUBLIC);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_handles_internal_gateway_execute_sync_locally() {
    run_frontdoor_async_test(
        "gateway_handles_internal_gateway_execute_sync_locally",
        gateway_handles_internal_gateway_execute_sync_locally_impl(),
    );
}

async fn gateway_handles_internal_gateway_execute_sync_locally_impl() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                Json(json!({
                    "request_id": "req-internal-execute-sync-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-local-execute-sync",
                            "object": "chat.completion",
                            "choices": []
                        }
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-execute-sync", "user-client-execute-sync"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-execute-sync-1",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-execute-sync-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-execute-sync-1",
            "provider-execute-sync-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-execute-sync-1",
            "provider-execute-sync-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/execute-sync"))
        .json(&json!({
            "trace_id": "trace-internal-execute-sync",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            },
            "auth_context": {
                "user_id": "user-client-execute-sync",
                "api_key_id": "api-key-client-execute-sync",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_supplied_auth_context_rejected(response).await;
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        0
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_returns_internal_gateway_execute_stream_proxy_public_action_without_proxying_upstream(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/execute-stream",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/execute-stream"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {},
            "body_json": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], CONTROL_ACTION_PROXY_PUBLIC);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_execute_stream_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(move |_request: Request| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner.lock().expect("mutex should lock") += 1;
                let frames = concat!(
                    "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: one\\n\\n\"}}\n",
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n",
                    "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                );
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(frames))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-ndjson"),
                );
                response
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot(
            "api-key-client-execute-stream",
            "user-client-execute-stream",
        ),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-execute-stream-1",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-execute-stream-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-execute-stream-1",
            "provider-execute-stream-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-execute-stream-1",
            "provider-execute-stream-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/execute-stream"))
        .json(&json!({
            "trace_id": "trace-internal-execute-stream",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
                "stream": true,
            },
            "auth_context": {
                "user_id": "user-client-execute-stream",
                "api_key_id": "api-key-client-execute-stream",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_supplied_auth_context_rejected(response).await;
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        0
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_resolve_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-internal-resolve")),
        unrestricted_models_snapshot("key-internal-resolve", "user-internal-resolve"),
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/resolve"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "x-api-key": "sk-internal-resolve",
            },
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["route_class"], "ai_public");
    assert_eq!(payload["route_family"], "openai");
    assert_eq!(payload["route_kind"], "chat");
    assert_eq!(payload["auth_endpoint_signature"], "openai:chat");
    assert_eq!(payload["auth_context"]["user_id"], "user-internal-resolve");
    assert_eq!(
        payload["auth_context"]["api_key_id"],
        "key-internal-resolve"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_auth_context_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-internal-auth-context")),
        unrestricted_models_snapshot("key-internal-auth-context", "user-internal-auth-context"),
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/auth-context"))
        .json(&json!({
            "headers": {
                "x-api-key": "sk-internal-auth-context",
            },
            "auth_endpoint_signature": "openai:chat",
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["auth_context"]["user_id"],
        "user-internal-auth-context"
    );
    assert_eq!(
        payload["auth_context"]["api_key_id"],
        "key-internal-auth-context"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_report_sync_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(internal_report_planner_state());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_openai_chat_report_capability(
        &client,
        &gateway_url,
        "decision-sync",
        "trace-internal-report-sync",
        false,
    )
    .await;
    assert_eq!(report_kind, "openai_chat_sync_success");

    let response = client
        .post(format!("{gateway_url}/api/internal/gateway/report-sync"))
        .json(&json!({
            "trace_id": "trace-internal-report-sync",
            "report_kind": report_kind,
            "report_context": report_context,
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "id": "chatcmpl-report-sync",
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2,
                    "total_tokens": 3,
                }
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, json!({ "ok": true }));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_report_stream_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(internal_report_planner_state());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_openai_chat_report_capability(
        &client,
        &gateway_url,
        "decision-stream",
        "trace-internal-report-stream",
        true,
    )
    .await;
    assert_eq!(report_kind, "openai_chat_stream_success");

    let response = client
        .post(format!("{gateway_url}/api/internal/gateway/report-stream"))
        .json(&json!({
            "trace_id": "trace-internal-report-stream",
            "report_kind": report_kind,
            "report_context": report_context,
            "status_code": 200,
            "headers": {
                "content-type": "text/event-stream",
            },
            "body_base64": base64::engine::general_purpose::STANDARD.encode("data: [DONE]\\n\\n"),
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, json!({ "ok": true }));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_internal_gateway_report_with_tampered_protected_context() {
    let gateway = build_router_with_state(internal_report_planner_state());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_openai_chat_report_capability(
        &client,
        &gateway_url,
        "decision-sync",
        "trace-internal-report-tampered-context",
        false,
    )
    .await;

    for (field, forged_value) in [
        ("user_id", json!("user-unrelated-victim")),
        ("api_key_id", json!("api-key-unrelated-victim")),
        ("provider_id", json!("provider-unrelated-victim")),
        ("endpoint_id", json!("endpoint-unrelated-victim")),
        ("key_id", json!("key-unrelated-victim")),
        ("client_api_format", json!("gemini:video")),
        ("task_id", json!("task-unrelated-victim")),
        ("local_task_id", json!("local-task-unrelated-victim")),
        ("local_short_id", json!("short-unrelated-victim")),
        ("file_name", json!("files/unrelated-victim")),
        ("file_key_id", json!("file-key-unrelated-victim")),
    ] {
        let mut tampered_context = report_context.clone();
        tampered_context
            .as_object_mut()
            .expect("planner report context should be an object")
            .insert(field.to_string(), forged_value);
        let response = post_internal_sync_report(
            &client,
            &gateway_url,
            "trace-internal-report-tampered-context",
            &report_kind,
            tampered_context,
        )
        .await;
        assert_internal_report_capability_rejected(response).await;
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_internal_gateway_report_without_a_known_capability() {
    let gateway = build_router_with_state(internal_report_planner_state());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_openai_chat_report_capability(
        &client,
        &gateway_url,
        "decision-sync",
        "trace-internal-report-missing-capability",
        false,
    )
    .await;

    let mut missing_capability = report_context.clone();
    missing_capability
        .as_object_mut()
        .expect("planner report context should be an object")
        .remove(INTERNAL_REPORT_CAPABILITY_FIELD);
    let response = post_internal_sync_report(
        &client,
        &gateway_url,
        "trace-internal-report-missing-capability",
        &report_kind,
        missing_capability,
    )
    .await;
    assert_internal_report_capability_rejected(response).await;

    let mut unknown_capability = report_context;
    unknown_capability[INTERNAL_REPORT_CAPABILITY_FIELD] =
        json!("00000000-0000-4000-8000-000000000000");
    let response = post_internal_sync_report(
        &client,
        &gateway_url,
        "trace-internal-report-missing-capability",
        &report_kind,
        unknown_capability,
    )
    .await;
    assert_internal_report_capability_rejected(response).await;

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_internal_gateway_report_for_wrong_trace_or_scope() {
    let gateway = build_router_with_state(internal_report_planner_state());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_openai_chat_report_capability(
        &client,
        &gateway_url,
        "decision-sync",
        "trace-internal-report-boundary",
        false,
    )
    .await;

    let response = post_internal_sync_report(
        &client,
        &gateway_url,
        "trace-internal-report-wrong-trace",
        &report_kind,
        report_context.clone(),
    )
    .await;
    assert_internal_report_capability_rejected(response).await;

    let response = post_internal_sync_report(
        &client,
        &gateway_url,
        "trace-internal-report-boundary",
        "openai_image_sync_success",
        report_context,
    )
    .await;
    assert_internal_report_capability_rejected(response).await;

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_allows_internal_gateway_report_observation_fields() {
    let gateway = build_router_with_state(internal_report_planner_state());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, mut report_context) = issue_openai_chat_report_capability(
        &client,
        &gateway_url,
        "decision-sync",
        "trace-internal-report-observations",
        false,
    )
    .await;
    let context = report_context
        .as_object_mut()
        .expect("planner report context should be an object");
    context.insert(
        "provider_response_headers".to_string(),
        json!({"x-request-id": "upstream-request-123"}),
    );
    context.insert(
        "client_response_headers".to_string(),
        json!({"content-type": "application/json"}),
    );
    context.insert("upstream_response".to_string(), json!({"status_code": 200}));
    context.insert("error_flow".to_string(), json!({"attempted": false}));

    let response = post_internal_sync_report(
        &client,
        &gateway_url,
        "trace-internal-report-observations",
        &report_kind,
        report_context,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .json::<serde_json::Value>()
            .await
            .expect("json body should parse"),
        json!({"ok": true})
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(internal_report_planner_state());
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (_report_kind, report_context) = issue_openai_chat_report_capability(
        &client,
        &gateway_url,
        "plan-sync",
        "trace-internal-finalize-sync",
        false,
    )
    .await;

    let response = client
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-sync",
            "report_kind": "openai_chat_sync_finalize",
            "report_context": report_context,
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "id": "chatcmpl-local-finalize",
                "object": "chat.completion",
                "choices": [],
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "id": "chatcmpl-local-finalize",
            "object": "chat.completion",
            "choices": [],
        })
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_openai_video_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(internal_video_create_planner_state(
        "openai:video",
        "sora-2",
    ));
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_internal_gateway_report_capability(
        &client,
        &gateway_url,
        "decision-sync",
        "trace-internal-finalize-video",
        "POST",
        "/v1/videos",
        json!({
            "authorization": format!("Bearer {INTERNAL_REPORT_CLIENT_KEY}"),
            "content-type": "application/json",
        }),
        json!({
            "model": "sora-2",
            "prompt": "make a trailer",
        }),
    )
    .await;
    assert_eq!(report_kind, "openai_video_create_sync_finalize");

    let response = client
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-video",
            "report_kind": report_kind,
            "report_context": report_context,
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "id": "vid-ext-123",
                "status": "submitted",
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert!(payload["id"]
        .as_str()
        .is_some_and(|value| !value.is_empty() && value != "vid-ext-123"));
    assert_eq!(payload["object"], "video");
    assert_eq!(payload["status"], "queued");
    assert_eq!(payload["progress"], 0);
    assert!(payload["created_at"].as_u64().is_some());
    assert_eq!(payload["model"], "sora-2");
    assert_eq!(payload["prompt"], "make a trailer");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_gemini_video_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway =
        build_router_with_state(internal_video_create_planner_state("gemini:video", "veo-3"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_internal_gateway_report_capability(
        &client,
        &gateway_url,
        "plan-sync",
        "trace-internal-finalize-gemini-video",
        "POST",
        "/v1beta/models/veo-3:predictLongRunning",
        json!({
            "content-type": "application/json",
            "x-goog-api-key": INTERNAL_REPORT_CLIENT_KEY,
        }),
        json!({
            "prompt": "make a gemini trailer",
        }),
    )
    .await;
    assert_eq!(report_kind, "gemini_video_create_sync_finalize");

    let response = client
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-gemini-video",
            "report_kind": report_kind,
            "report_context": report_context,
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "name": "operations/123",
                "done": false,
                "metadata": {},
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert!(payload["name"]
        .as_str()
        .is_some_and(|value| value.starts_with("models/veo-3/operations/")));
    assert_eq!(payload["done"], false);
    assert_eq!(payload["metadata"], json!({}));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_openai_video_delete_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = internal_video_followup_planner_state(
        "openai:video",
        "video-delete-123",
        None,
        "ext-video-delete-123",
        "sora-2",
    )
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_internal_gateway_report_capability(
        &client,
        &gateway_url,
        "decision-sync",
        "trace-internal-finalize-video-delete",
        "DELETE",
        "/v1/videos/video-delete-123",
        json!({
            "authorization": format!("Bearer {INTERNAL_REPORT_CLIENT_KEY}"),
            "content-type": "application/json",
        }),
        json!({}),
    )
    .await;
    assert_eq!(report_kind, "openai_video_delete_sync_finalize");

    let response = client
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-video-delete",
            "report_kind": report_kind,
            "report_context": report_context,
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "id": "video-delete-123",
            "object": "video",
            "deleted": true,
        })
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_gemini_video_cancel_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let state = internal_video_followup_planner_state(
        "gemini:video",
        "gemini-cancel-task-record",
        Some("gemini-cancel-123"),
        "operations/ext-gemini-cancel-123",
        "veo-3",
    )
    .await;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();
    let (report_kind, report_context) = issue_internal_gateway_report_capability(
        &client,
        &gateway_url,
        "plan-sync",
        "trace-internal-finalize-gemini-video-cancel",
        "POST",
        "/v1beta/models/veo-3/operations/gemini-cancel-123:cancel",
        json!({
            "content-type": "application/json",
            "x-goog-api-key": INTERNAL_REPORT_CLIENT_KEY,
        }),
        json!({}),
    )
    .await;
    assert_eq!(report_kind, "gemini_video_cancel_sync_finalize");

    let response = client
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-gemini-video-cancel",
            "report_kind": report_kind,
            "report_context": report_context,
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, json!({}));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_internal_gateway_finalize_sync_unknown_kind_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-unknown-kind",
            "report_kind": "openai_video_unknown_sync_finalize",
            "report_context": {},
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "detail": "Unsupported gateway sync finalize kind",
        })
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_decision_sync_locally_with_supplied_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-1", "user-client-1"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row("provider-1", "openai", "openai:chat", "gpt-5", 10),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-1",
            "provider-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/decision-sync"))
        .json(&json!({
            "trace_id": "trace-internal-decision-sync",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            },
            "auth_context": {
                "user_id": "user-client-1",
                "api_key_id": "api-key-client-1",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_supplied_auth_context_rejected(response).await;
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_internal_decision_sync_revalidates_supplied_auth_context_wallet() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-empty-wallet", "user-empty-wallet"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row("provider-1", "openai", "openai:chat", "gpt-5", 10),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-1",
            "provider-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let billing_repository = Arc::new(InMemoryBillingReadRepository::seed(Vec::new()));
    let wallet_repository = Arc::new(InMemoryWalletRepository::seed(vec![
        StoredWalletSnapshot::new(
            "wallet-empty".to_string(),
            Some("user-empty-wallet".to_string()),
            None,
            0.0,
            0.0,
            "finite".to_string(),
            "USD".to_string(),
            "active".to_string(),
            0.0,
            0.0,
            0.0,
            0.0,
            100,
        )
        .expect("wallet should build"),
    ]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_usage_billing_and_wallet_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    usage_repository,
                    billing_repository,
                    wallet_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/decision-sync"))
        .json(&json!({
            "trace_id": "trace-internal-decision-sync-empty-wallet",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            },
            "auth_context": {
                "user_id": "user-empty-wallet",
                "api_key_id": "api-key-empty-wallet",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_supplied_auth_context_rejected(response).await;
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_internal_gateway_decision_sync_fallback_with_resolved_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-decision-fallback")),
        unrestricted_models_snapshot("api-key-fallback-1", "user-fallback-1"),
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository)
                    .with_system_default_routing_group_for_tests(),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/decision-sync"))
        .json(&json!({
            "trace_id": "trace-internal-decision-fallback",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
                "x-api-key": "sk-decision-fallback",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], "fallback_plan");
    assert_eq!(payload["auth_context"]["user_id"], "user-fallback-1");
    assert_eq!(payload["auth_context"]["api_key_id"], "api-key-fallback-1");
    assert_eq!(payload["auth_context"]["access_allowed"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_decision_stream_locally_with_supplied_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-stream", "user-client-stream"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row("provider-stream-1", "openai", "openai:chat", "gpt-5", 10),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-stream-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-stream-1",
            "provider-stream-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-stream-1",
            "provider-stream-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/internal/gateway/decision-stream"
        ))
        .json(&json!({
            "trace_id": "trace-internal-decision-stream",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
                "stream": true,
            },
            "auth_context": {
                "user_id": "user-client-stream",
                "api_key_id": "api-key-client-stream",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_supplied_auth_context_rejected(response).await;
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_plan_sync_locally_with_supplied_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-plan-sync", "user-client-plan-sync"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-plan-sync-1",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-plan-sync-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-plan-sync-1",
            "provider-plan-sync-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-plan-sync-1",
            "provider-plan-sync-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/plan-sync"))
        .json(&json!({
            "trace_id": "trace-internal-plan-sync",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            },
            "auth_context": {
                "user_id": "user-client-plan-sync",
                "api_key_id": "api-key-client-plan-sync",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_supplied_auth_context_rejected(response).await;
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_plan_stream_locally_with_supplied_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-plan-stream", "user-client-plan-stream"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-plan-stream-1",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-plan-stream-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-plan-stream-1",
            "provider-plan-stream-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-plan-stream-1",
            "provider-plan-stream-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/plan-stream"))
        .json(&json!({
            "trace_id": "trace-internal-plan-stream",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
                "stream": true,
            },
            "auth_context": {
                "user_id": "user-client-plan-stream",
                "api_key_id": "api-key-client-plan-stream",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_supplied_auth_context_rejected(response).await;
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
