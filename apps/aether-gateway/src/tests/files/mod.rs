use super::{
    any, build_router, build_router_with_state, build_state_with_execution_runtime_override, json,
    start_server, to_bytes, AppState, Arc, Body, Bytes, HeaderName, HeaderValue, Infallible, Json,
    Mutex, Request, Response, Router, StatusCode, CONTROL_EXECUTED_HEADER,
    CONTROL_EXECUTE_FALLBACK_HEADER, EXECUTION_PATH_HEADER, TRACE_ID_HEADER,
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
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use sha2::{Digest, Sha256};

mod registry_cleanup;
mod stream;
mod sync;

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn sample_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["gemini"])),
        Some(serde_json::json!(["gemini:chat"])),
        Some(serde_json::json!(["gemini-2.5-pro"])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800_i64),
        Some(serde_json::json!(["gemini"])),
        Some(serde_json::json!(["gemini:chat"])),
        Some(serde_json::json!(["gemini-2.5-pro"])),
    )
    .expect("auth snapshot should build")
}

fn sample_files_candidate_row() -> StoredMinimalCandidateSelectionRow {
    StoredMinimalCandidateSelectionRow {
        provider_id: "provider-gemini-files-local-1".to_string(),
        provider_name: "gemini".to_string(),
        provider_type: "custom".to_string(),
        provider_priority: 10,
        provider_is_active: true,
        endpoint_id: "endpoint-gemini-files-local-1".to_string(),
        endpoint_api_format: "gemini:chat".to_string(),
        endpoint_api_family: Some("gemini".to_string()),
        endpoint_kind: Some("chat".to_string()),
        endpoint_is_active: true,
        key_id: "key-gemini-files-local-1".to_string(),
        key_name: "prod".to_string(),
        key_auth_type: "api_key".to_string(),
        key_is_active: true,
        key_api_formats: Some(vec!["gemini:chat".to_string()]),
        key_allowed_models: None,
        key_capabilities: Some(serde_json::json!({"gemini_files": true})),
        key_internal_priority: 5,
        key_global_priority_by_format: Some(serde_json::json!({"gemini:chat": 1})),
        model_id: "model-gemini-files-local-1".to_string(),
        global_model_id: "global-model-gemini-files-local-1".to_string(),
        global_model_name: "gemini-2.5-pro".to_string(),
        global_model_mappings: None,
        global_model_supports_streaming: Some(true),
        model_provider_model_name: "gemini-2.5-pro-upstream".to_string(),
        model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
            name: "gemini-2.5-pro-upstream".to_string(),
            priority: 1,
            api_formats: Some(vec!["gemini:chat".to_string()]),
        }]),
        model_supports_streaming: Some(true),
        model_is_active: true,
        model_is_available: true,
    }
}

fn sample_files_provider_catalog_provider() -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        "provider-gemini-files-local-1".to_string(),
        "gemini".to_string(),
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

fn sample_files_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        "endpoint-gemini-files-local-1".to_string(),
        "provider-gemini-files-local-1".to_string(),
        "gemini:chat".to_string(),
        Some("gemini".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        "https://generativelanguage.googleapis.com".to_string(),
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

fn sample_files_provider_catalog_key() -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        "key-gemini-files-local-1".to_string(),
        "provider-gemini-files-local-1".to_string(),
        "prod".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(serde_json::json!(["gemini:chat"])),
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-gemini-files")
            .expect("api key should encrypt"),
        None,
        Some(serde_json::json!({"gemini_files": true})),
        Some(serde_json::json!({"gemini:chat": 1})),
        None,
        None,
        None,
        None,
    )
    .expect("key transport should build")
}

#[tokio::test]
async fn gateway_locally_denies_gemini_files_download_control_sync_even_with_opt_in_headers_when_execution_runtime_missing(
) {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("file-bytes"))
                        .expect("response should build");
                    response.headers_mut().insert(
                        http::header::CONTENT_TYPE,
                        HeaderValue::from_static("application/octet-stream"),
                    );
                    response.headers_mut().insert(
                        HeaderName::from_static(CONTROL_EXECUTED_HEADER),
                        HeaderValue::from_static("true"),
                    );
                    response
                }
            }),
        )
        .route(
            "/v1beta/files/file-123:download",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/v1beta/files/file-123:download?alt=media"
        ))
        .header(CONTROL_EXECUTE_FALLBACK_HEADER, "true")
        .header(TRACE_ID_HEADER, "trace-files-download-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "当前 Gemini Files 请求无法在本地执行：没有匹配到可用的执行路径"
    );
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_gemini_files_download_control_sync_without_opt_in_header_when_execution_runtime_missing(
) {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let public_execution_path = Arc::new(Mutex::new(None::<String>));
    let public_execution_path_clone = Arc::clone(&public_execution_path);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("unexpected-execute"))
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
            "/v1beta/files/file-123:download",
            any(move |request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                let public_execution_path_inner = Arc::clone(&public_execution_path_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    *public_execution_path_inner
                        .lock()
                        .expect("mutex should lock") = Some(
                        request
                            .headers()
                            .get(EXECUTION_PATH_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/v1beta/files/file-123:download?alt=media"
        ))
        .header(CONTROL_EXECUTE_FALLBACK_HEADER, "true")
        .header(TRACE_ID_HEADER, "trace-files-download-public-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "当前 Gemini Files 请求无法在本地执行：没有匹配到可用的执行路径"
    );
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        public_execution_path
            .lock()
            .expect("mutex should lock")
            .clone()
            .as_deref(),
        None
    );

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_skips_gemini_files_download_control_sync_without_opt_in_header() {
    let execute_hits = Arc::new(Mutex::new(0usize));
    let execute_hits_clone = Arc::clone(&execute_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/execute-sync",
            any(move |_request: Request| {
                let execute_hits_inner = Arc::clone(&execute_hits_clone);
                async move {
                    *execute_hits_inner.lock().expect("mutex should lock") += 1;
                    let mut response = Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from("file-bytes"))
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
            "/v1beta/files/file-123:download",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{gateway_url}/v1beta/files/file-123:download?alt=media"
        ))
        .header(TRACE_ID_HEADER, "trace-files-download-local-only-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "当前 Gemini Files 请求无法在本地执行：没有匹配到可用的执行路径"
    );
    assert_eq!(*execute_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_executes_gemini_files_get_via_local_decision_gate_with_local_planning_only() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        method: String,
        url: String,
        auth_header_value: String,
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
                    "route_family": "gemini",
                    "route_kind": "files",
                    "auth_endpoint_signature": "gemini:chat",
                    "execution_runtime_candidate": true,
                    "auth_context": {
                        "user_id": "user-files-local-123",
                        "api_key_id": "key-files-local-123",
                        "access_allowed": true
                    },
                    "public_path": "/v1beta/files/files/abc-123"
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
                    StatusCode::NO_CONTENT
                }
            }),
        )
        .route(
            "/v1beta/files/files/abc-123",
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
                    auth_header_value: payload
                        .get("headers")
                        .and_then(|value| value.get("x-goog-api-key"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
                Json(json!({
                    "request_id": "trace-gemini-files-local-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "name": "files/abc-123"
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 19
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("client-files-local-key")),
        sample_auth_snapshot("key-files-local-123", "user-files-local-123"),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_files_candidate_row(),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_files_provider_catalog_provider()],
        vec![sample_files_provider_catalog_endpoint()],
        vec![sample_files_provider_catalog_key()],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url.clone())
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
        .get(format!(
            "{gateway_url}/v1beta/files/files/abc-123?view=FULL&key=client-files-local-key"
        ))
        .header("x-goog-api-key", "client-header-key")
        .header(TRACE_ID_HEADER, "trace-gemini-files-local-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.text().await.expect("body should read"),
        "{\"name\":\"files/abc-123\"}"
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(seen_execution_runtime_request.method, "GET");
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://generativelanguage.googleapis.com/v1beta/files/files/abc-123?view=FULL"
    );
    assert_eq!(
        seen_execution_runtime_request.auth_header_value,
        "sk-upstream-gemini-files"
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-gemini-files-local-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_non_post_gemini_upload_without_hitting_fallback_probe() {
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new().route(
        "/upload/v1beta/files",
        any(move |_request: Request| {
            let public_hits_inner = Arc::clone(&public_hits_clone);
            async move {
                *public_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/upload/v1beta/files"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["detail"], "Method not allowed");
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
