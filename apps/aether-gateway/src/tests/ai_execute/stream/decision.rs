use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    to_bytes, AppState, Arc, Body, Bytes, HeaderName, HeaderValue, Json, Mutex, Request, Response,
    Router, StatusCode, EXECUTION_PATH_EXECUTION_RUNTIME_STREAM, EXECUTION_PATH_HEADER,
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
use aether_data_contracts::repository::candidates::{
    RequestCandidateReadRepository, RequestCandidateStatus,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use sha2::{Digest, Sha256};

#[tokio::test]
async fn gateway_executes_openai_chat_stream_via_local_decision_gate_without_execution_runtime_override(
) {
    #[derive(Debug, Clone)]
    struct SeenUpstreamStreamRequest {
        model: String,
        stream: bool,
        accept: String,
        authorization: String,
        content_type: String,
    }

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

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-local-stream-1".to_string(),
            provider_name: "openai".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-local-stream-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-local-stream-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 1})),
            model_id: "model-openai-local-stream-1".to_string(),
            global_model_id: "global-model-openai-local-stream-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-5-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:chat".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-local-stream-1".to_string(),
            "openai".to_string(),
            Some("https://example.com".to_string()),
            "custom".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            true,
            None,
            Some(2),
            None,
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-openai-local-stream-1".to_string(),
            "provider-openai-local-stream-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
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

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-local-stream-1".to_string(),
            "provider-openai-local-stream-1".to_string(),
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

    let seen_upstream = Arc::new(Mutex::new(None::<SeenUpstreamStreamRequest>));
    let seen_upstream_clone = Arc::clone(&seen_upstream);
    let seen_report = Arc::new(Mutex::new(false));
    let seen_report_clone = Arc::clone(&seen_report);
    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
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
                    "route_kind": "chat",
                    "auth_endpoint_signature": "openai:chat",
                    "execution_runtime_candidate": true,
                    "auth_context": {
                        "user_id": "user-openai-local-stream-1",
                        "api_key_id": "api-key-openai-local-stream-1",
                        "access_allowed": true
                    },
                    "public_path": "/v1/chat/completions"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-stream",
            any(move |_request: Request| {
                let decision_hits_inner = Arc::clone(&decision_hits_clone);
                async move {
                    *decision_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/plan-stream",
            any(move |_request: Request| {
                let plan_hits_inner = Arc::clone(&plan_hits_clone);
                async move {
                    *plan_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/report-stream",
            any(move |request: Request| {
                let seen_report_inner = Arc::clone(&seen_report_clone);
                async move {
                    let (_parts, body) = request.into_parts();
                    let _raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                    *seen_report_inner.lock().expect("mutex should lock") = true;
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

    let provider = Router::new().route(
        "/v1/chat/completions",
        any(move |request: Request| {
            let seen_upstream_inner = Arc::clone(&seen_upstream_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("upstream payload should parse");
                *seen_upstream_inner.lock().expect("mutex should lock") =
                    Some(SeenUpstreamStreamRequest {
                        model: payload
                            .get("model")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        stream: payload
                            .get("stream")
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        accept: parts
                            .headers
                            .get(http::header::ACCEPT)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        authorization: parts
                            .headers
                            .get(http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        content_type: parts
                            .headers
                            .get(http::header::CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    });
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(
                        "data: {\"id\":\"chatcmpl-local-123\"}\n\ndata: [DONE]\n\n",
                    ))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                response
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-local-stream")),
        sample_auth_snapshot(
            "api-key-openai-local-stream-1",
            "user-openai-local-stream-1",
        ),
    )]));
    let mut backup_candidate_row = sample_candidate_row();
    backup_candidate_row.provider_id = "provider-openai-local-stream-2".to_string();
    backup_candidate_row.endpoint_id = "endpoint-openai-local-stream-2".to_string();
    backup_candidate_row.key_id = "key-openai-local-stream-2".to_string();
    backup_candidate_row.key_name = "backup".to_string();
    backup_candidate_row.key_internal_priority = 6;
    backup_candidate_row.model_id = "model-openai-local-stream-2".to_string();
    backup_candidate_row.global_model_id = "global-model-openai-local-stream-2".to_string();
    backup_candidate_row.model_provider_model_name = "gpt-5-upstream-backup".to_string();
    backup_candidate_row.model_provider_model_mappings = Some(vec![StoredProviderModelMapping {
        name: "gpt-5-upstream-backup".to_string(),
        priority: 1,
        api_formats: Some(vec!["openai:chat".to_string()]),
    }]);
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
            backup_candidate_row,
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let mut backup_provider = sample_provider_catalog_provider();
    backup_provider.id = "provider-openai-local-stream-2".to_string();
    let mut backup_endpoint = sample_provider_catalog_endpoint();
    backup_endpoint.id = "endpoint-openai-local-stream-2".to_string();
    backup_endpoint.provider_id = "provider-openai-local-stream-2".to_string();
    backup_endpoint.base_url = "https://api.openai.backup.example".to_string();
    let mut backup_key = sample_provider_catalog_key();
    backup_key.id = "key-openai-local-stream-2".to_string();
    backup_key.provider_id = "provider-openai-local-stream-2".to_string();
    backup_key.name = "backup".to_string();
    backup_key.encrypted_api_key =
        encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-openai-backup")
            .expect("api key should encrypt");
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (provider_url, provider_handle) = start_server(provider).await;
    let mut primary_endpoint = sample_provider_catalog_endpoint();
    primary_endpoint.base_url = provider_url.clone();
    backup_endpoint.base_url = "http://127.0.0.1:9".to_string();
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider(), backup_provider],
        vec![primary_endpoint, backup_endpoint],
        vec![sample_provider_catalog_key(), backup_key],
    ));
    let gateway_state = AppState::new()
        .expect("gateway state should build")
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
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-stream",
        )
        .header(TRACE_ID_HEADER, "trace-openai-chat-local-stream-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_STREAM)
    );
    assert_eq!(
        response.text().await.expect("body should read"),
        "data: {\"id\":\"chatcmpl-local-123\"}\n\ndata: [DONE]\n\n"
    );

    let seen_upstream_request = seen_upstream
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("upstream stream should be captured");
    assert_eq!(seen_upstream_request.model, "gpt-5-upstream");
    assert!(seen_upstream_request.stream);
    assert_eq!(seen_upstream_request.accept, "text/event-stream");
    assert_eq!(
        seen_upstream_request.authorization,
        "Bearer sk-upstream-openai"
    );
    assert_eq!(seen_upstream_request.content_type, "application/json");
    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-chat-local-stream-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 2);
    assert_eq!(
        stored_candidates
            .iter()
            .filter(|candidate| candidate.status == RequestCandidateStatus::Success)
            .count(),
        1
    );
    assert_eq!(
        stored_candidates
            .iter()
            .filter(|candidate| candidate.status == RequestCandidateStatus::Failed)
            .count(),
        0
    );

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !*seen_report.lock().expect("mutex should lock"),
        "report-stream should stay local when request candidate persistence is available"
    );

    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    provider_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_executes_openai_chat_stream_via_local_openai_cli_cross_format_candidate() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeStreamRequest {
        trace_id: String,
        url: String,
        model: String,
        stream: bool,
        accept: String,
        authorization: String,
        x_client_request_id: String,
        session_id: String,
        conversation_id: String,
        instructions: String,
        user_text: String,
        prompt_cache_key: String,
    }

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
            Some(serde_json::json!(["openai", "codex"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5.4"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            Some(serde_json::json!(["openai", "codex"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5.4"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-chat-cli-local-1".to_string(),
            provider_name: "codex".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-chat-cli-local-1".to_string(),
            endpoint_api_format: "openai:cli".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-chat-cli-local-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:cli": 1})),
            model_id: "model-openai-chat-cli-local-1".to_string(),
            global_model_id: "global-model-openai-chat-cli-local-1".to_string(),
            global_model_name: "gpt-5.4".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5.4".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-5.4".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:cli".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-chat-cli-local-1".to_string(),
            "codex".to_string(),
            Some("https://chatgpt.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            true,
            None,
            Some(2),
            None,
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-openai-chat-cli-local-1".to_string(),
            "provider-openai-chat-cli-local-1".to_string(),
            "openai:cli".to_string(),
            Some("openai".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://chatgpt.com/backend-api/codex".to_string(),
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

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-chat-cli-local-1".to_string(),
            "provider-openai-chat-cli-local-1".to_string(),
            "prod".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:cli"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-upstream-openai-chat-cli",
            )
            .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"openai:cli": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeStreamRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let seen_report = Arc::new(Mutex::new(false));
    let seen_report_clone = Arc::clone(&seen_report);
    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/resolve",
            any(|_request: Request| async move {
                Json(json!({
                    "action": "proxy_public",
                    "route_class": "ai_public",
                    "route_family": "openai",
                    "route_kind": "chat",
                    "auth_endpoint_signature": "openai:chat",
                    "execution_runtime_candidate": true,
                    "auth_context": {
                        "user_id": "user-openai-chat-cli-local-1",
                        "api_key_id": "api-key-openai-chat-cli-local-1",
                        "access_allowed": true
                    },
                    "public_path": "/v1/chat/completions"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-stream",
            any(move |_request: Request| {
                let decision_hits_inner = Arc::clone(&decision_hits_clone);
                async move {
                    *decision_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/plan-stream",
            any(move |_request: Request| {
                let plan_hits_inner = Arc::clone(&plan_hits_clone);
                async move {
                    *plan_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/report-stream",
            any(move |request: Request| {
                let seen_report_inner = Arc::clone(&seen_report_clone);
                async move {
                    let (_parts, body) = request.into_parts();
                    let _raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                    *seen_report_inner.lock().expect("mutex should lock") = true;
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
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("execution runtime payload should parse");
                *seen_execution_runtime_inner.lock().expect("mutex should lock") =
                    Some(SeenExecutionRuntimeStreamRequest {
                        trace_id: parts
                            .headers
                            .get(TRACE_ID_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        url: payload
                            .get("url")
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
                        stream: payload
                            .get("body")
                            .and_then(|value| value.get("json_body"))
                            .and_then(|value| value.get("stream"))
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        accept: payload
                            .get("headers")
                            .and_then(|value| value.get("accept"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        authorization: payload
                            .get("headers")
                            .and_then(|value| value.get("authorization"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        x_client_request_id: payload
                            .get("headers")
                            .and_then(|value| value.get("x-client-request-id"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        session_id: payload
                            .get("headers")
                            .and_then(|value| value.get("session_id"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        conversation_id: payload
                            .get("headers")
                            .and_then(|value| value.get("conversation_id"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        instructions: payload
                            .get("body")
                            .and_then(|value| value.get("json_body"))
                            .and_then(|value| value.get("instructions"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        user_text: payload
                            .get("body")
                            .and_then(|value| value.get("json_body"))
                            .and_then(|value| value.get("input"))
                            .and_then(|value| value.as_array())
                            .and_then(|value| value.first())
                            .and_then(|value| value.get("content"))
                            .and_then(|value| value.as_array())
                            .and_then(|value| value.first())
                            .and_then(|value| value.get("text"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        prompt_cache_key: payload
                            .get("body")
                            .and_then(|value| value.get("json_body"))
                            .and_then(|value| value.get("prompt_cache_key"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    });
                let frames = concat!(
                    "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"event: response.completed\\ndata: {\\\"type\\\":\\\"response.completed\\\",\\\"response\\\":{\\\"id\\\":\\\"resp_cli_chat_local_123\\\",\\\"object\\\":\\\"response\\\",\\\"model\\\":\\\"gpt-5.4\\\",\\\"status\\\":\\\"completed\\\",\\\"output\\\":[{\\\"type\\\":\\\"message\\\",\\\"id\\\":\\\"msg_cli_chat_local_123\\\",\\\"role\\\":\\\"assistant\\\",\\\"status\\\":\\\"completed\\\",\\\"content\\\":[{\\\"type\\\":\\\"output_text\\\",\\\"text\\\":\\\"Hello Codex\\\",\\\"annotations\\\":[]}]}],\\\"usage\\\":{\\\"input_tokens\\\":1,\\\"output_tokens\\\":2,\\\"total_tokens\\\":3}}}\\n\\n\"}}\n",
                    "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":31,\"ttfb_ms\":11,\"upstream_bytes\":197}}}\n",
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
        Some(hash_api_key("sk-client-openai-chat-cli")),
        sample_auth_snapshot(
            "api-key-openai-chat-cli-local-1",
            "user-openai-chat-cli-local-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
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
        )
        .with_system_config_values_for_tests(vec![(
            "provider_priority_mode".to_string(),
            json!("global_key"),
        )]),
    );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-chat-cli",
        )
        .header(TRACE_ID_HEADER, "trace-openai-chat-cli-local-123")
        .body(
            "{\"model\":\"gpt-5.4\",\"messages\":[{\"role\":\"system\",\"content\":\"You are terse.\"},{\"role\":\"user\",\"content\":\"Say hello\"}],\"stream\":true}",
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_STREAM)
    );
    let response_text = response.text().await.expect("body should read");
    assert!(response_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(response_text.contains("\"content\":\"Hello Codex\""));
    assert!(response_text.contains("\"finish_reason\":\"stop\""));
    assert!(response_text.contains("data: [DONE]"));

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime stream should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-openai-chat-cli-local-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(seen_execution_runtime_request.model, "gpt-5.4");
    assert!(seen_execution_runtime_request.stream);
    assert_eq!(seen_execution_runtime_request.accept, "text/event-stream");
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-chat-cli"
    );
    assert_eq!(
        seen_execution_runtime_request.x_client_request_id,
        "trace-openai-chat-cli-local-123"
    );
    assert_eq!(
        seen_execution_runtime_request.prompt_cache_key,
        "b6741389-8b9e-5c00-bef6-fbce92aee45a"
    );
    assert_eq!(
        seen_execution_runtime_request.session_id,
        "9fa08f4f14ccba13"
    );
    assert_eq!(
        seen_execution_runtime_request.conversation_id,
        "9fa08f4f14ccba13"
    );
    assert_eq!(
        seen_execution_runtime_request.instructions,
        "You are terse."
    );
    assert_eq!(seen_execution_runtime_request.user_text, "Say hello");

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-chat-cli-local-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);
    let extra_data = stored_candidates[0]
        .extra_data
        .as_ref()
        .expect("request candidate extra_data should exist");
    assert_eq!(extra_data["execution_strategy"], "local_cross_format");
    assert_eq!(extra_data["conversion_mode"], "bidirectional");
    assert_eq!(extra_data["client_contract"], "openai:chat");
    assert_eq!(extra_data["provider_contract"], "openai:cli");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !*seen_report.lock().expect("mutex should lock"),
        "report-stream should stay local when request candidate persistence is available"
    );

    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_executes_openai_chat_stream_with_custom_path_via_local_decision_gate_with_local_stream_decision(
) {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeStreamRequest {
        trace_id: String,
        url: String,
        model: String,
        stream: bool,
        accept: String,
        authorization: String,
        endpoint_tag: String,
        renamed_header: String,
        dropped_header_present: bool,
        metadata_mode: String,
        metadata_source: String,
        temperature_present: bool,
        proxy_node_id: String,
        tls_profile: String,
    }

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

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-custom-stream-1".to_string(),
            provider_name: "openai".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-custom-stream-1".to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-custom-stream-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:chat": 1})),
            model_id: "model-openai-custom-stream-1".to_string(),
            global_model_id: "global-model-openai-custom-stream-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-5-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:chat".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-custom-stream-1".to_string(),
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
            Some(serde_json::json!({"url":"http://provider-proxy.internal:8080"})),
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-openai-custom-stream-1".to_string(),
            "provider-openai-custom-stream-1".to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.example".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"x-endpoint-tag","value":"openai-chat-local-stream"},
                {"action":"rename","from":"x-client-rename","to":"x-upstream-rename"},
                {"action":"drop","key":"x-drop-me"}
            ])),
            Some(serde_json::json!([
                {"action":"set","path":"metadata.mode","value":"safe"},
                {"action":"rename","from":"metadata.client","to":"metadata.source"},
                {"action":"drop","path":"temperature"}
            ])),
            Some(2),
            Some("/custom/v1/chat/completions".to_string()),
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-custom-stream-1".to_string(),
            "provider-openai-custom-stream-1".to_string(),
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
            Some(serde_json::json!({"enabled": true, "node_id":"proxy-node-openai-custom-stream"})),
            Some(serde_json::json!({"tls_profile":"chrome_136"})),
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeStreamRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let seen_report = Arc::new(Mutex::new(false));
    let seen_report_clone = Arc::clone(&seen_report);
    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
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
                    "route_kind": "chat",
                    "auth_endpoint_signature": "openai:chat",
                    "execution_runtime_candidate": true,
                    "auth_context": {
                        "user_id": "user-openai-custom-stream-1",
                        "api_key_id": "api-key-openai-custom-stream-1",
                        "access_allowed": true
                    },
                    "public_path": "/v1/chat/completions"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-stream",
            any(move |_request: Request| {
                let decision_hits_inner = Arc::clone(&decision_hits_clone);
                async move {
                    *decision_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/plan-stream",
            any(move |_request: Request| {
                let plan_hits_inner = Arc::clone(&plan_hits_clone);
                async move {
                    *plan_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/report-stream",
            any(move |request: Request| {
                let seen_report_inner = Arc::clone(&seen_report_clone);
                async move {
                    let (_parts, body) = request.into_parts();
                    let _raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                    *seen_report_inner.lock().expect("mutex should lock") = true;
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
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("execution runtime payload should parse");
                *seen_execution_runtime_inner.lock().expect("mutex should lock") =
                    Some(SeenExecutionRuntimeStreamRequest {
                        trace_id: parts
                            .headers
                            .get(TRACE_ID_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        url: payload
                            .get("url")
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
                        stream: payload
                            .get("body")
                            .and_then(|value| value.get("json_body"))
                            .and_then(|value| value.get("stream"))
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        accept: payload
                            .get("headers")
                            .and_then(|value| value.get("accept"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        authorization: payload
                            .get("headers")
                            .and_then(|value| value.get("authorization"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        endpoint_tag: payload
                            .get("headers")
                            .and_then(|value| value.get("x-endpoint-tag"))
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
                        temperature_present: payload
                            .get("body")
                            .and_then(|value| value.get("json_body"))
                            .and_then(|value| value.get("temperature"))
                            .is_some(),
                        proxy_node_id: payload
                            .get("proxy")
                            .and_then(|value| value.get("node_id"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        tls_profile: payload
                            .get("tls_profile")
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    });
                let frames = concat!(
                    "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-local-custom-path-123\\\"}\\n\\n\"}}\n",
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n",
                    "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":31,\"ttfb_ms\":11,\"upstream_bytes\":37}}}\n",
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
        Some(hash_api_key("sk-client-openai-custom-stream")),
        sample_auth_snapshot(
            "api-key-openai-custom-stream-1",
            "user-openai-custom-stream-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_catalog_provider()],
        vec![sample_provider_catalog_endpoint()],
        vec![sample_provider_catalog_key()],
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
        )
        .with_system_config_values_for_tests(vec![(
            "provider_priority_mode".to_string(),
            json!("global_key"),
        )]),
    );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-custom-stream",
        )
        .header("x-client-rename", "rename-openai-stream")
        .header("x-drop-me", "drop-openai-stream")
        .header(TRACE_ID_HEADER, "trace-openai-chat-custom-stream-123")
        .body(
            "{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true,\"metadata\":{\"client\":\"desktop-openai-stream\"},\"temperature\":0.3}",
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_STREAM)
    );
    assert_eq!(
        response.text().await.expect("body should read"),
        "data: {\"id\":\"chatcmpl-local-custom-path-123\"}\n\ndata: [DONE]\n\n"
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime stream should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-openai-chat-custom-stream-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.openai.example/custom/v1/chat/completions"
    );
    assert_eq!(seen_execution_runtime_request.model, "gpt-5-upstream");
    assert!(seen_execution_runtime_request.stream);
    assert_eq!(seen_execution_runtime_request.accept, "text/event-stream");
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai"
    );
    assert_eq!(
        seen_execution_runtime_request.endpoint_tag,
        "openai-chat-local-stream"
    );
    assert_eq!(
        seen_execution_runtime_request.renamed_header,
        "rename-openai-stream"
    );
    assert!(!seen_execution_runtime_request.dropped_header_present);
    assert_eq!(seen_execution_runtime_request.metadata_mode, "safe");
    assert_eq!(
        seen_execution_runtime_request.metadata_source,
        "desktop-openai-stream"
    );
    assert!(!seen_execution_runtime_request.temperature_present);
    assert_eq!(
        seen_execution_runtime_request.proxy_node_id,
        "proxy-node-openai-custom-stream"
    );
    assert_eq!(seen_execution_runtime_request.tls_profile, "chrome_136");

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-chat-custom-stream-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !*seen_report.lock().expect("mutex should lock"),
        "report-stream should stay local when request candidate persistence is available"
    );

    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_retries_next_local_openai_chat_stream_candidate_after_retryable_429_execution_runtime_status(
) {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeStreamRequest {
        trace_id: String,
        url: String,
        model: String,
        stream: bool,
        accept: String,
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

    fn sample_candidate_row(
        provider_id: &str,
        endpoint_id: &str,
        key_id: &str,
        provider_priority: i32,
        global_priority: i32,
        mapped_model: &str,
    ) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: provider_id.to_string(),
            provider_name: "openai".to_string(),
            provider_type: "custom".to_string(),
            provider_priority,
            provider_is_active: true,
            endpoint_id: endpoint_id.to_string(),
            endpoint_api_format: "openai:chat".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(
                serde_json::json!({"openai:chat": global_priority}),
            ),
            model_id: format!("model-{provider_id}"),
            global_model_id: "global-model-openai-stream-failover".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: mapped_model.to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: mapped_model.to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:chat".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider(
        provider_id: &str,
        provider_name: &str,
    ) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            provider_id.to_string(),
            provider_name.to_string(),
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

    fn sample_provider_catalog_endpoint(
        endpoint_id: &str,
        provider_id: &str,
        base_url: &str,
    ) -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            endpoint_id.to_string(),
            provider_id.to_string(),
            "openai:chat".to_string(),
            Some("openai".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            base_url.to_string(),
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

    fn sample_provider_catalog_key(
        key_id: &str,
        provider_id: &str,
        secret: &str,
        global_priority: i32,
    ) -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            key_id.to_string(),
            provider_id.to_string(),
            "prod".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:chat"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, secret)
                .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"openai:chat": global_priority})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime =
        Arc::new(Mutex::new(Vec::<SeenExecutionRuntimeStreamRequest>::new()));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let seen_report = Arc::new(Mutex::new(false));
    let seen_report_clone = Arc::clone(&seen_report);
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
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
                    "route_kind": "chat",
                    "auth_endpoint_signature": "openai:chat",
                    "execution_runtime_candidate": true,
                    "auth_context": {
                        "user_id": "user-openai-local-stream-failover-1",
                        "api_key_id": "api-key-openai-local-stream-failover-1",
                        "access_allowed": true
                    },
                    "public_path": "/v1/chat/completions"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-stream",
            any(move |_request: Request| {
                let decision_hits_inner = Arc::clone(&decision_hits_clone);
                async move {
                    *decision_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/plan-stream",
            any(move |_request: Request| {
                let plan_hits_inner = Arc::clone(&plan_hits_clone);
                async move {
                    *plan_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"action": "proxy_public"}))
                }
            }),
        )
        .route(
            "/api/internal/gateway/report-stream",
            any(move |request: Request| {
                let seen_report_inner = Arc::clone(&seen_report_clone);
                async move {
                    let (_parts, body) = request.into_parts();
                    let _raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                    *seen_report_inner.lock().expect("mutex should lock") = true;
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
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("execution runtime payload should parse");
                let mut hits = execution_runtime_hits_inner.lock().expect("mutex should lock");
                *hits += 1;
                let attempt = *hits;
                drop(hits);

                seen_execution_runtime_inner.lock().expect("mutex should lock").push(
                    SeenExecutionRuntimeStreamRequest {
                        trace_id: parts
                            .headers
                            .get(TRACE_ID_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        url: payload
                            .get("url")
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
                        stream: payload
                            .get("body")
                            .and_then(|value| value.get("json_body"))
                            .and_then(|value| value.get("stream"))
                            .and_then(|value| value.as_bool())
                            .unwrap_or(false),
                        accept: payload
                            .get("headers")
                            .and_then(|value| value.get("accept"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        authorization: payload
                            .get("headers")
                            .and_then(|value| value.get("authorization"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    },
                );

                let frames = if attempt == 1 {
                    concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":429,\"headers\":{\"content-type\":\"application/json\"}}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"{\\\"error\\\":{\\\"message\\\":\\\"rate limited\\\",\\\"type\\\":\\\"rate_limit_error\\\"}}\"}}\n",
                        "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                    )
                } else {
                    concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-local-stream-failover-123\\\"}\\n\\n\"}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n",
                        "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":33,\"ttfb_ms\":12,\"upstream_bytes\":48}}}\n",
                        "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                    )
                };
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
        Some(hash_api_key("sk-client-openai-local-stream-failover")),
        sample_auth_snapshot(
            "api-key-openai-local-stream-failover-1",
            "user-openai-local-stream-failover-1",
        ),
    )]));
    let candidate_selection_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_candidate_row(
                "provider-openai-local-stream-primary",
                "endpoint-openai-local-stream-primary",
                "key-openai-local-stream-primary",
                10,
                1,
                "gpt-5-upstream-primary",
            ),
            sample_candidate_row(
                "provider-openai-local-stream-backup",
                "endpoint-openai-local-stream-backup",
                "key-openai-local-stream-backup",
                20,
                2,
                "gpt-5-upstream-backup",
            ),
        ]));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![
            sample_provider_catalog_provider("provider-openai-local-stream-primary", "openai"),
            sample_provider_catalog_provider("provider-openai-local-stream-backup", "openai"),
        ],
        vec![
            sample_provider_catalog_endpoint(
                "endpoint-openai-local-stream-primary",
                "provider-openai-local-stream-primary",
                "https://api.openai.primary.example",
            ),
            sample_provider_catalog_endpoint(
                "endpoint-openai-local-stream-backup",
                "provider-openai-local-stream-backup",
                "https://api.openai.backup.example",
            ),
        ],
        vec![
            sample_provider_catalog_key(
                "key-openai-local-stream-primary",
                "provider-openai-local-stream-primary",
                "sk-upstream-openai-primary",
                1,
            ),
            sample_provider_catalog_key(
                "key-openai-local-stream-backup",
                "provider-openai-local-stream-backup",
                "sk-upstream-openai-backup",
                2,
            ),
        ],
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
        )
        .with_system_config_values_for_tests(vec![(
            "provider_priority_mode".to_string(),
            json!("global_key"),
        )]),
    );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-stream-failover",
        )
        .header(
            TRACE_ID_HEADER,
            "trace-openai-chat-local-stream-failover-123",
        )
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_STREAM)
    );
    assert_eq!(
        response.text().await.expect("body should read"),
        "data: {\"id\":\"chatcmpl-local-stream-failover-123\"}\n\ndata: [DONE]\n\n"
    );

    let seen_execution_runtime_requests = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone();
    assert_eq!(seen_execution_runtime_requests.len(), 2);
    assert_eq!(
        seen_execution_runtime_requests[0].trace_id,
        "trace-openai-chat-local-stream-failover-123"
    );
    assert_eq!(
        seen_execution_runtime_requests[0].url,
        "https://api.openai.primary.example/v1/chat/completions"
    );
    assert_eq!(
        seen_execution_runtime_requests[0].authorization,
        "Bearer sk-upstream-openai-primary"
    );
    assert_eq!(
        seen_execution_runtime_requests[1].url,
        "https://api.openai.backup.example/v1/chat/completions"
    );
    assert_eq!(
        seen_execution_runtime_requests[1].model,
        "gpt-5-upstream-backup"
    );
    assert!(seen_execution_runtime_requests[1].stream);
    assert_eq!(
        seen_execution_runtime_requests[1].accept,
        "text/event-stream"
    );
    assert_eq!(
        seen_execution_runtime_requests[1].authorization,
        "Bearer sk-upstream-openai-backup"
    );
    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-chat-local-stream-failover-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 2);
    assert_eq!(stored_candidates[0].candidate_index, 0);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Failed);
    assert_eq!(stored_candidates[0].status_code, Some(429));
    assert_eq!(
        stored_candidates[0].error_type.as_deref(),
        Some("retryable_upstream_status")
    );
    assert_eq!(
        stored_candidates[0].error_message.as_deref(),
        Some("execution runtime stream returned retryable status 429")
    );
    assert_eq!(stored_candidates[1].candidate_index, 1);
    assert_eq!(stored_candidates[1].status, RequestCandidateStatus::Success);
    assert_eq!(stored_candidates[1].status_code, Some(200));
    assert!(stored_candidates[1].started_at_unix_ms.is_some());
    assert!(stored_candidates[1].finished_at_unix_ms.is_some());

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !*seen_report.lock().expect("mutex should lock"),
        "report-stream should stay local when request candidate persistence is available"
    );

    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        2
    );
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
