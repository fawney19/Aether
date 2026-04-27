use super::{
    any, build_router_with_execution_runtime_override, build_router_with_state,
    build_state_with_execution_runtime_override, json, start_server, to_bytes, Arc, Body, Bytes,
    HeaderName, HeaderValue, Json, Mutex, Request, Response, Router, StatusCode,
    CONTROL_EXECUTED_HEADER, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC, EXECUTION_PATH_HEADER,
    TRACE_ID_HEADER,
};
use crate::data::GatewayDataState;
use aether_crypto::{encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
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
async fn gateway_executes_openai_responses_cross_format_upstream_stream_via_local_finalize_response(
) {
    use base64::Engine as _;

    #[derive(Debug, Clone)]
    struct SeenRemoteExecutionRuntimeRequest {
        trace_id: String,
        request_id: String,
        url: String,
        authorization: String,
        endpoint_tag: String,
        provider_model: String,
        has_model_field: bool,
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
            Some(serde_json::json!(["openai", "gemini"])),
            Some(serde_json::json!(["openai:responses"])),
            Some(serde_json::json!(["gpt-5"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(serde_json::json!(["openai", "gemini"])),
            Some(serde_json::json!(["openai:responses"])),
            Some(serde_json::json!(["gpt-5"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-cli-gemini-finalize-local-1".to_string(),
            provider_name: "gemini".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-cli-gemini-finalize-local-1".to_string(),
            endpoint_api_format: "gemini:cli".to_string(),
            endpoint_api_family: Some("gemini".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-cli-gemini-finalize-local-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["gemini:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"gemini:cli": 1})),
            model_id: "model-openai-cli-gemini-finalize-local-1".to_string(),
            global_model_id: "global-model-openai-cli-gemini-finalize-local-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gemini-2.5-pro-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gemini-2.5-pro-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["gemini:cli".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-cli-gemini-finalize-local-1".to_string(),
            "gemini".to_string(),
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
            "endpoint-openai-cli-gemini-finalize-local-1".to_string(),
            "provider-openai-cli-gemini-finalize-local-1".to_string(),
            "gemini:cli".to_string(),
            Some("gemini".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://generativelanguage.googleapis.com".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"x-endpoint-tag","value":"openai-cli-gemini-finalize-cross-format"}
            ])),
            None,
            Some(2),
            Some("/custom/v1beta/models/gemini-2.5-pro-upstream:generateContent".to_string()),
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-cli-gemini-finalize-local-1".to_string(),
            "provider-openai-cli-gemini-finalize-local-1".to_string(),
            "prod".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["gemini:cli"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-upstream-openai-cli-gemini-finalize",
            )
            .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"gemini:cli": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_remote_execution_runtime =
        Arc::new(Mutex::new(None::<SeenRemoteExecutionRuntimeRequest>));
    let seen_remote_execution_runtime_clone = Arc::clone(&seen_remote_execution_runtime);
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
    let finalize_hits = Arc::new(Mutex::new(0usize));
    let finalize_hits_clone = Arc::clone(&finalize_hits);
    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    let upstream = Router::new()
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
            "/api/internal/gateway/finalize-sync",
            any(move |_request: Request| {
                let finalize_hits_inner = Arc::clone(&finalize_hits_clone);
                async move {
                    *finalize_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::IM_A_TEAPOT,
                        Body::from("finalize-sync-should-not-be-hit"),
                    )
                }
            }),
        )
        .route(
            "/api/internal/gateway/report-sync",
            any(move |request: Request| {
                let report_hits_inner = Arc::clone(&report_hits_clone);
                async move {
                    let (_parts, body) = request.into_parts();
                    let _raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                    *report_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"ok": true}))
                }
            }),
        )
        .route(
            "/v1/responses",
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
            let seen_remote_execution_runtime_inner = Arc::clone(&seen_remote_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("execution runtime payload should parse");
                *seen_remote_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenRemoteExecutionRuntimeRequest {
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    request_id: payload
                        .get("request_id")
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
                    endpoint_tag: payload
                        .get("headers")
                        .and_then(|value| value.get("x-endpoint-tag"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    provider_model: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("model"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    has_model_field: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("model"))
                        .is_some(),
                });
                Json(json!({
                    "request_id": "trace-openai-cli-xfmt-stream-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "text/event-stream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                            concat!(
                                "data: {\"responseId\":\"upstream-cli-stream-123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\"}\n\n",
                                "data: {\"responseId\":\"upstream-cli-stream-123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello Gemini CLI\"}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n"
                            )
                        )
                    },
                    "telemetry": {
                        "elapsed_ms": 31
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-cli-xfmt-stream")),
        sample_auth_snapshot(
            "api-key-openai-cli-xfmt-stream-1",
            "user-openai-cli-xfmt-stream-1",
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
                GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                    auth_repository,
                    candidate_selection_repository,
                    provider_catalog_repository,
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let started_at = std::time::Instant::now();
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-cli-xfmt-stream",
        )
        .header(TRACE_ID_HEADER, "trace-openai-cli-xfmt-stream-123")
        .body("{\"model\":\"gpt-5\",\"input\":\"hello\"}")
        .send()
        .await
        .expect("request should succeed");
    let elapsed = started_at.elapsed();
    let response_status = response.status();
    let response_body = response.text().await.expect("body should read");

    assert_eq!(response_status, StatusCode::OK);
    let response_json: serde_json::Value =
        serde_json::from_str(&response_body).expect("body should parse");
    assert_eq!(
        response_json,
        json!({
            "id": "upstream-cli-stream-123",
            "object": "response",
            "status": "completed",
            "model": "gemini-2.5-pro-upstream",
            "output": [{
                "type": "message",
                "id": "upstream-cli-stream-123_msg",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello Gemini CLI",
                    "annotations": []
                }]
            }],
            "usage": {
                "input_tokens": 2,
                "output_tokens": 3,
                "total_tokens": 5
            }
        })
    );
    assert!(
        elapsed < std::time::Duration::from_millis(10_000),
        "response took unexpectedly long for local finalize path: elapsed={elapsed:?} finalize_hits={} report_hits={}",
        *finalize_hits.lock().expect("mutex should lock"),
        *report_hits.lock().expect("mutex should lock"),
    );

    let seen_remote_execution_runtime_request = seen_remote_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("remote execution runtime plan should be captured");
    assert_eq!(
        seen_remote_execution_runtime_request.trace_id,
        "trace-openai-cli-xfmt-stream-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.request_id,
        "trace-openai-cli-xfmt-stream-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.url,
        "https://generativelanguage.googleapis.com/custom/v1beta/models/gemini-2.5-pro-upstream:generateContent"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-cli-gemini-finalize"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.endpoint_tag,
        "openai-cli-gemini-finalize-cross-format"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.provider_model,
        "gemini-2.5-pro-upstream"
    );
    assert!(seen_remote_execution_runtime_request.has_model_field);

    let mut stored_candidates = Vec::new();
    for _ in 0..50 {
        stored_candidates = request_candidate_repository
            .list_by_request_id("trace-openai-cli-xfmt-stream-123")
            .await
            .expect("request candidate trace should read");
        if stored_candidates.len() == 1
            && stored_candidates[0].status == RequestCandidateStatus::Success
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        *report_hits.lock().expect("mutex should lock"),
        0,
        "report-sync should stay local when request candidate persistence is available"
    );
    assert_eq!(*finalize_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_executes_openai_responses_cross_format_function_call_upstream_stream_via_local_finalize_response(
) {
    use base64::Engine as _;

    #[derive(Debug, Clone)]
    struct SeenRemoteExecutionRuntimeRequest {
        trace_id: String,
        request_id: String,
        url: String,
        authorization: String,
        endpoint_tag: String,
        provider_model: String,
        has_model_field: bool,
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
            Some(serde_json::json!(["openai", "gemini"])),
            Some(serde_json::json!(["openai:responses"])),
            Some(serde_json::json!(["gpt-5"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(serde_json::json!(["openai", "gemini"])),
            Some(serde_json::json!(["openai:responses"])),
            Some(serde_json::json!(["gpt-5"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-cli-gemini-tool-finalize-local-1".to_string(),
            provider_name: "gemini".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-cli-gemini-tool-finalize-local-1".to_string(),
            endpoint_api_format: "gemini:cli".to_string(),
            endpoint_api_family: Some("gemini".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-cli-gemini-tool-finalize-local-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["gemini:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"gemini:cli": 1})),
            model_id: "model-openai-cli-gemini-tool-finalize-local-1".to_string(),
            global_model_id: "global-model-openai-cli-gemini-tool-finalize-local-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gemini-2.5-pro-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gemini-2.5-pro-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["gemini:cli".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-cli-gemini-tool-finalize-local-1".to_string(),
            "gemini".to_string(),
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
            "endpoint-openai-cli-gemini-tool-finalize-local-1".to_string(),
            "provider-openai-cli-gemini-tool-finalize-local-1".to_string(),
            "gemini:cli".to_string(),
            Some("gemini".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://generativelanguage.googleapis.com".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"x-endpoint-tag","value":"openai-cli-gemini-tool-finalize-cross-format"}
            ])),
            None,
            Some(2),
            Some("/custom/v1beta/models/gemini-2.5-pro-upstream:generateContent".to_string()),
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-cli-gemini-tool-finalize-local-1".to_string(),
            "provider-openai-cli-gemini-tool-finalize-local-1".to_string(),
            "prod".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["gemini:cli"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-upstream-openai-cli-gemini-tool-finalize",
            )
            .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"gemini:cli": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_remote_execution_runtime =
        Arc::new(Mutex::new(None::<SeenRemoteExecutionRuntimeRequest>));
    let seen_remote_execution_runtime_clone = Arc::clone(&seen_remote_execution_runtime);
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
    let finalize_hits = Arc::new(Mutex::new(0usize));
    let finalize_hits_clone = Arc::clone(&finalize_hits);
    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    let upstream = Router::new()
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
            "/api/internal/gateway/finalize-sync",
            any(move |_request: Request| {
                let finalize_hits_inner = Arc::clone(&finalize_hits_clone);
                async move {
                    *finalize_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::IM_A_TEAPOT,
                        Body::from("finalize-sync-should-not-be-hit"),
                    )
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
            "/v1/responses",
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
            let seen_remote_execution_runtime_inner = Arc::clone(&seen_remote_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("execution runtime payload should parse");
                *seen_remote_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenRemoteExecutionRuntimeRequest {
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    request_id: payload
                        .get("request_id")
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
                    endpoint_tag: payload
                        .get("headers")
                        .and_then(|value| value.get("x-endpoint-tag"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    provider_model: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("model"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    has_model_field: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("model"))
                        .is_some(),
                });
                Json(json!({
                    "request_id": "trace-openai-cli-xfmt-tool-stream-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "text/event-stream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                            concat!(
                                "data: {\"responseId\":\"upstream-cli-tool-123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Need a tool.\"},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"location\":\"Tokyo\"}}}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n"
                            )
                        )
                    },
                    "telemetry": {
                        "elapsed_ms": 31
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-cli-xfmt-tool-stream")),
        sample_auth_snapshot(
            "api-key-openai-cli-xfmt-tool-stream-1",
            "user-openai-cli-xfmt-tool-stream-1",
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
                GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                    auth_repository,
                    candidate_selection_repository,
                    provider_catalog_repository,
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            );
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let started_at = std::time::Instant::now();
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-cli-xfmt-tool-stream",
        )
        .header(TRACE_ID_HEADER, "trace-openai-cli-xfmt-tool-stream-123")
        .body("{\"model\":\"gpt-5\",\"input\":\"weather\"}")
        .send()
        .await
        .expect("request should succeed");
    let elapsed = started_at.elapsed();

    assert_eq!(response.status(), StatusCode::OK);
    let response_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(
        response_json,
        json!({
            "id": "upstream-cli-tool-123",
            "object": "response",
            "status": "completed",
            "model": "gemini-2.5-pro-upstream",
            "output": [
                {
                    "type": "message",
                    "id": "upstream-cli-tool-123_msg",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Need a tool.",
                        "annotations": []
                    }]
                },
                {
                    "type": "function_call",
                    "id": "call_auto_1",
                    "call_id": "call_auto_1",
                    "name": "get_weather",
                    "arguments": "{\"location\":\"Tokyo\"}"
                }
            ],
            "usage": {
                "input_tokens": 2,
                "output_tokens": 3,
                "total_tokens": 5
            }
        })
    );
    assert!(
        elapsed < std::time::Duration::from_millis(10_000),
        "response took unexpectedly long for local finalize path: elapsed={elapsed:?} finalize_hits={} report_hits={}",
        *finalize_hits.lock().expect("mutex should lock"),
        *report_hits.lock().expect("mutex should lock"),
    );

    let seen_remote_execution_runtime_request = seen_remote_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("remote execution runtime plan should be captured");
    assert_eq!(
        seen_remote_execution_runtime_request.trace_id,
        "trace-openai-cli-xfmt-tool-stream-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.request_id,
        "trace-openai-cli-xfmt-tool-stream-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.url,
        "https://generativelanguage.googleapis.com/custom/v1beta/models/gemini-2.5-pro-upstream:generateContent"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-cli-gemini-tool-finalize"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.endpoint_tag,
        "openai-cli-gemini-tool-finalize-cross-format"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.provider_model,
        "gemini-2.5-pro-upstream"
    );
    assert!(seen_remote_execution_runtime_request.has_model_field);

    let mut stored_candidates = Vec::new();
    for _ in 0..50 {
        stored_candidates = request_candidate_repository
            .list_by_request_id("trace-openai-cli-xfmt-tool-stream-123")
            .await
            .expect("request candidate trace should read");
        if stored_candidates.len() == 1
            && stored_candidates[0].status == RequestCandidateStatus::Success
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*finalize_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_executes_openai_responses_antigravity_cross_format_upstream_stream_via_local_finalize_response(
) {
    use base64::Engine as _;

    #[derive(Debug, Clone)]
    struct SeenRefreshRequest {
        content_type: String,
        body: String,
    }

    #[derive(Debug, Clone)]
    struct SeenRemoteExecutionRuntimeRequest {
        trace_id: String,
        url: String,
        accept: String,
        authorization: String,
        x_client_name: String,
        x_client_version: String,
        x_vscode_sessionid: String,
        x_goog_api_client: String,
        project: String,
        request_id: String,
        model: String,
        user_agent: String,
        request_type: String,
        contents_len: usize,
        request_has_model: bool,
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
            Some(serde_json::json!(["openai", "antigravity"])),
            Some(serde_json::json!(["openai:responses"])),
            Some(serde_json::json!(["gpt-5"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(serde_json::json!(["openai", "antigravity"])),
            Some(serde_json::json!(["openai:responses"])),
            Some(serde_json::json!(["gpt-5"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-cli-antigravity-finalize-local-1".to_string(),
            provider_name: "antigravity".to_string(),
            provider_type: "antigravity".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-cli-antigravity-finalize-local-1".to_string(),
            endpoint_api_format: "gemini:cli".to_string(),
            endpoint_api_family: Some("gemini".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-cli-antigravity-finalize-local-1".to_string(),
            key_name: "oauth".to_string(),
            key_auth_type: "oauth".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["gemini:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"gemini:cli": 1})),
            model_id: "model-openai-cli-antigravity-finalize-local-1".to_string(),
            global_model_id: "global-model-openai-cli-antigravity-finalize-local-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "claude-sonnet-4-5".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "claude-sonnet-4-5".to_string(),
                priority: 1,
                api_formats: Some(vec!["gemini:cli".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-openai-cli-antigravity-finalize-local-1".to_string(),
            "antigravity".to_string(),
            Some("https://example.com".to_string()),
            "antigravity".to_string(),
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
            "endpoint-openai-cli-antigravity-finalize-local-1".to_string(),
            "provider-openai-cli-antigravity-finalize-local-1".to_string(),
            "gemini:cli".to_string(),
            Some("gemini".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://antigravity.googleapis.com".to_string(),
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
        let encrypted_auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"antigravity","project_id":"project-antigravity-local-1","client_version":"1.2.3","session_id":"sess-antigravity-local-123","refresh_token":"rt-antigravity-cli-local-123"}"#,
        )
        .expect("auth config should encrypt");
        StoredProviderCatalogKey::new(
            "key-openai-cli-antigravity-finalize-local-1".to_string(),
            "provider-openai-cli-antigravity-finalize-local-1".to_string(),
            "oauth".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["gemini:cli"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
                .expect("placeholder api key should encrypt"),
            Some(encrypted_auth_config),
            None,
            Some(serde_json::json!({"gemini:cli": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_remote_execution_runtime =
        Arc::new(Mutex::new(None::<SeenRemoteExecutionRuntimeRequest>));
    let seen_remote_execution_runtime_clone = Arc::clone(&seen_remote_execution_runtime);
    let seen_refresh = Arc::new(Mutex::new(None::<SeenRefreshRequest>));
    let seen_refresh_clone = Arc::clone(&seen_refresh);
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
    let finalize_hits = Arc::new(Mutex::new(0usize));
    let finalize_hits_clone = Arc::clone(&finalize_hits);
    let decision_hits = Arc::new(Mutex::new(0usize));
    let decision_hits_clone = Arc::clone(&decision_hits);
    let plan_hits = Arc::new(Mutex::new(0usize));
    let plan_hits_clone = Arc::clone(&plan_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    let upstream = Router::new()
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
            "/api/internal/gateway/finalize-sync",
            any(move |_request: Request| {
                let finalize_hits_inner = Arc::clone(&finalize_hits_clone);
                async move {
                    *finalize_hits_inner.lock().expect("mutex should lock") += 1;
                    (
                        StatusCode::IM_A_TEAPOT,
                        Body::from("finalize-sync-should-not-be-hit"),
                    )
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
            "/v1/responses",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
                }
            }),
        );

    let refresh = Router::new().route(
        "/oauth/token",
        any(move |request: Request| {
            let seen_refresh_inner = Arc::clone(&seen_refresh_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                *seen_refresh_inner.lock().expect("mutex should lock") = Some(SeenRefreshRequest {
                    content_type: parts
                        .headers
                        .get(http::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    body: String::from_utf8(raw_body.to_vec())
                        .expect("refresh body should be utf8"),
                });
                Json(json!({
                    "access_token": "refreshed-antigravity-cli-access-token",
                    "refresh_token": "rt-antigravity-cli-local-456",
                    "token_type": "Bearer",
                    "expires_in": 3600
                }))
            }
        }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_remote_execution_runtime_inner = Arc::clone(&seen_remote_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("execution runtime payload should parse");
                *seen_remote_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenRemoteExecutionRuntimeRequest {
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
                    x_client_name: payload
                        .get("headers")
                        .and_then(|value| value.get("x-client-name"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    x_client_version: payload
                        .get("headers")
                        .and_then(|value| value.get("x-client-version"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    x_vscode_sessionid: payload
                        .get("headers")
                        .and_then(|value| value.get("x-vscode-sessionid"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    x_goog_api_client: payload
                        .get("headers")
                        .and_then(|value| value.get("x-goog-api-client"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    project: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("project"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    request_id: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("requestId"))
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
                    user_agent: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("userAgent"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    request_type: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("requestType"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    contents_len: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("request"))
                        .and_then(|value| value.get("contents"))
                        .and_then(|value| value.as_array())
                        .map(Vec::len)
                        .unwrap_or_default(),
                    request_has_model: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("request"))
                        .and_then(|value| value.get("model"))
                        .is_some(),
                });
                Json(json!({
                    "request_id": "trace-openai-cli-antigravity-xfmt-stream-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "text/event-stream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                            concat!(
                                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"claude-sonnet-4-5\"},\"responseId\":\"resp_antigravity_cli_xfmt_123\"}\n\n",
                                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello Antigravity\"}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"claude-sonnet-4-5\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}},\"responseId\":\"resp_antigravity_cli_xfmt_123\"}\n\n"
                            )
                        )
                    },
                    "telemetry": {
                        "elapsed_ms": 31
                    }
                }))
            }
        }),
    );

    let client_api_key = "client-antigravity-cli-oauth-local";
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(client_api_key)),
        sample_auth_snapshot(
            "api-key-openai-cli-antigravity-finalize-local-1",
            "user-openai-cli-antigravity-finalize-local-1",
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
    let (refresh_url, refresh_handle) = start_server(refresh).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::oauth_refresh::GenericOAuthRefreshAdapter::default()
                    .with_token_url_for_tests("antigravity", format!("{refresh_url}/oauth/token")),
            ),
        ]);
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url.clone())
            .with_data_state_for_tests(
                GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                    auth_repository,
                    candidate_selection_repository,
                    provider_catalog_repository,
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            )
            .with_oauth_refresh_coordinator_for_tests(oauth_refresh);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let started_at = std::time::Instant::now();
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {client_api_key}"),
        )
        .header(
            TRACE_ID_HEADER,
            "trace-openai-cli-antigravity-xfmt-stream-123",
        )
        .body("{\"model\":\"gpt-5\",\"input\":\"hello\"}")
        .send()
        .await
        .expect("request should succeed");
    let elapsed = started_at.elapsed();

    assert_eq!(response.status(), StatusCode::OK);
    let response_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(
        response_json,
        json!({
            "id": "resp-local-stream",
            "object": "response",
            "status": "completed",
            "model": "claude-sonnet-4-5",
            "output": [{
                "type": "message",
                "id": "resp-local-stream_msg",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello Antigravity",
                    "annotations": []
                }]
            }],
            "usage": {
                "input_tokens": 2,
                "output_tokens": 3,
                "total_tokens": 5
            }
        })
    );
    assert!(
        elapsed < std::time::Duration::from_millis(10_000),
        "response took unexpectedly long for local finalize path: elapsed={elapsed:?} finalize_hits={} report_hits={}",
        *finalize_hits.lock().expect("mutex should lock"),
        *report_hits.lock().expect("mutex should lock"),
    );

    let seen_refresh_request = seen_refresh
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("refresh request should be captured");
    assert_eq!(
        seen_refresh_request.content_type,
        "application/x-www-form-urlencoded"
    );
    assert!(seen_refresh_request
        .body
        .contains("grant_type=refresh_token"));
    assert!(seen_refresh_request.body.contains(
        "client_id=1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com"
    ));
    assert!(seen_refresh_request
        .body
        .contains("client_secret=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"));
    assert!(seen_refresh_request
        .body
        .contains("refresh_token=rt-antigravity-cli-local-123"));

    let seen_remote_execution_runtime_request = seen_remote_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("remote execution runtime plan should be captured");
    assert_eq!(
        seen_remote_execution_runtime_request.trace_id,
        "trace-openai-cli-antigravity-xfmt-stream-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.url,
        "https://antigravity.googleapis.com/v1internal:streamGenerateContent?alt=sse"
    );
    assert_eq!(seen_remote_execution_runtime_request.accept, "*/*");
    assert_eq!(
        seen_remote_execution_runtime_request.authorization,
        "Bearer refreshed-antigravity-cli-access-token"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.x_client_name,
        "antigravity"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.x_client_version,
        "1.2.3"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.x_vscode_sessionid,
        "sess-antigravity-local-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.x_goog_api_client,
        "gl-node/18.18.2 fire/0.8.6 grpc/1.10.x"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.project,
        "project-antigravity-local-1"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.request_id,
        "trace-openai-cli-antigravity-xfmt-stream-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.model,
        "claude-sonnet-4-5"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.user_agent,
        "antigravity"
    );
    assert_eq!(seen_remote_execution_runtime_request.request_type, "agent");
    assert_eq!(seen_remote_execution_runtime_request.contents_len, 1);
    assert!(!seen_remote_execution_runtime_request.request_has_model);

    let mut stored_candidates = Vec::new();
    for _ in 0..50 {
        stored_candidates = request_candidate_repository
            .list_by_request_id("trace-openai-cli-antigravity-xfmt-stream-123")
            .await
            .expect("request candidate trace should read");
        if stored_candidates.len() == 1
            && stored_candidates[0].status == RequestCandidateStatus::Success
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*finalize_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
    refresh_handle.abort();
}
