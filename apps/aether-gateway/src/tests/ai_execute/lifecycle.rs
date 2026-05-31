use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    Arc, Body, Bytes, HeaderValue, Infallible, Json, Mutex, Request, Response, Router, StatusCode,
    TRACE_ID_HEADER,
};

use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data_contracts::repository::candidate_selection::{
    StoredMinimalCandidateSelectionRow, StoredProviderModelMapping,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use sha2::{Digest, Sha256};

use crate::data::GatewayDataState;
use crate::tests::next_non_keepalive_chunk;

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn sample_local_openai_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!(["gpt-5"])),
    )
    .expect("auth snapshot should build")
}

fn sample_local_openai_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-openai-lifecycle-local-1".to_string(),
        provider_name: "openai".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: "endpoint-openai-lifecycle-local-1".to_string(),
        endpoint_api_format: "openai:chat".to_string(),
        endpoint_api_family: Some("openai".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_is_active: true,
        key_id: "key-openai-lifecycle-local-1".to_string(),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["openai:chat".to_string()]),
        key_allowed_models: None,
        key_capabilities: None,
        key_internal_priority: 5,
        key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 1})),
        model_id: "model-openai-lifecycle-local-1".to_string(),
        global_model_id: "global-model-openai-lifecycle-local-1".to_string(),
        global_model_name: "gpt-5".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "gpt-5-upstream".to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: "gpt-5-upstream".to_string(),
            priority: 1,
            api_formats: Some(vec!["openai:chat".to_string()]),
            endpoint_ids: None,
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn sample_local_openai_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-openai-lifecycle-local-1".to_string(),
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

fn sample_local_openai_endpoint() -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        "endpoint-openai-lifecycle-local-1".to_string(),
        "provider-openai-lifecycle-local-1".to_string(),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        "https://api.openai.example/v1".to_string(),
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

fn sample_local_openai_key() -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        "key-openai-lifecycle-local-1".to_string(),
        "provider-openai-lifecycle-local-1".to_string(),
        "prod".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(serde_json::json!(["openai:chat"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-openai")
            .expect("api key should encrypt"),
        None,
        None,
        Some(serde_json::json!({"openai:chat": 1})),
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_completes_sync_response_on_local_execution_runtime_path() {
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let upstream = Router::new().route(
        "/v1/chat/completions",
        any(move |_request: Request| {
            let public_hits_inner = Arc::clone(&public_hits_clone);
            async move {
                *public_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::IM_A_TEAPOT, Body::from("public"))
            }
        }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(|_request: Request| async move {
            Json(json!({
                "request_id": "req-openai-chat-async-report-123",
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-async-report-123",
                        "object": "chat.completion",
                        "model": "gpt-5",
                        "choices": [],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 2,
                            "total_tokens": 3
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 12
                }
            }))
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-async-report")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-lifecycle-local-1",
            "user-openai-lifecycle-local-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_local_openai_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_local_openai_provider()],
        vec![sample_local_openai_endpoint()],
        vec![sample_local_openai_key()],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_selection_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-async-report",
        )
        .header(TRACE_ID_HEADER, "req-openai-chat-async-report-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_stops_execution_runtime_stream_when_client_disconnects() {
    let seen_report = Arc::new(Mutex::new(0usize));
    let seen_report_clone = Arc::clone(&seen_report);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/report-stream",
            any(move |_request: Request| {
                let seen_report_inner = Arc::clone(&seen_report_clone);
                async move {
                    *seen_report_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"ok": true}))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
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
        any(|_request: Request| async move {
            let body_stream = async_stream::stream! {
                yield Ok::<Bytes, Infallible>(Bytes::from_static(
                    b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n"
                ));
                yield Ok::<Bytes, Infallible>(Bytes::from_static(
                    b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-first\\\"}\\n\\n\"}}\n"
                ));
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                yield Ok::<Bytes, Infallible>(Bytes::from_static(
                    b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n"
                ));
                yield Ok::<Bytes, Infallible>(Bytes::from_static(
                    b"{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":41,\"ttfb_ms\":12,\"upstream_bytes\":31}}}\n"
                ));
                yield Ok::<Bytes, Infallible>(Bytes::from_static(
                    b"{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                ));
            };
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .body(Body::from_stream(body_stream))
                .expect("response should build");
            response.headers_mut().insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-ndjson"),
            );
            response
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-stream-disconnect")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-lifecycle-local-1",
            "user-openai-lifecycle-local-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_local_openai_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_local_openai_provider()],
        vec![sample_local_openai_endpoint()],
        vec![sample_local_openai_key()],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_selection_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-stream-disconnect",
        )
        .header(TRACE_ID_HEADER, "trace-openai-chat-stream-disconnect-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let mut response = response;
    let first_chunk = next_non_keepalive_chunk(&mut response).await;
    assert_eq!(
        first_chunk,
        Bytes::from_static(b"data: {\"id\":\"chatcmpl-first\"}\n\n")
    );
    drop(response);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(*seen_report.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gateway_returns_error_body_when_prefetch_detects_embedded_stream_error() {
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new().route(
        "/v1/chat/completions",
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
        any(|_request: Request| async move {
            let body_stream = async_stream::stream! {
                yield Ok::<Bytes, Infallible>(Bytes::from_static(
                    b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n"
                ));
                yield Ok::<Bytes, Infallible>(Bytes::from_static(
                    b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"error\\\":{\\\"message\\\":\\\"slow down\\\",\\\"type\\\":\\\"rate_limit_error\\\",\\\"code\\\":\\\"rate_limit\\\"}}\\n\\n\"}}\n"
                ));
            };
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .body(Body::from_stream(body_stream))
                .expect("response should build");
            response.headers_mut().insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-ndjson"),
            );
            response
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-stream-prefetch-error")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-lifecycle-local-1",
            "user-openai-lifecycle-local-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_local_openai_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_local_openai_provider()],
        vec![sample_local_openai_endpoint()],
        vec![sample_local_openai_key()],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_selection_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-stream-prefetch-error",
        )
        .header(
            TRACE_ID_HEADER,
            "trace-openai-chat-stream-prefetch-error-123",
        )
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body_text = response.text().await.expect("response body should read");
    assert!(body_text.contains("\"rate_limit_error\""));
    assert!(body_text.contains("\"slow down\""));
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
