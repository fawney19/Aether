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
async fn gateway_executes_openai_cli_sync_upstream_stream_via_local_finalize_response() {
    use base64::Engine as _;

    #[derive(Debug, Clone)]
    struct SeenRemoteExecutionRuntimeRequest {
        trace_id: String,
        request_id: String,
        url: String,
        model: String,
        authorization: String,
        endpoint_tag: String,
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
            Some(serde_json::json!(["openai:cli"])),
            Some(serde_json::json!(["gpt-5"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:cli"])),
            Some(serde_json::json!(["gpt-5"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-openai-cli-finalize-local-1".to_string(),
            provider_name: "openai".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-openai-cli-finalize-local-1".to_string(),
            endpoint_api_format: "openai:cli".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-openai-cli-finalize-local-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:cli": 1})),
            model_id: "model-openai-cli-finalize-local-1".to_string(),
            global_model_id: "global-model-openai-cli-finalize-local-1".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-5-upstream".to_string(),
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
            "provider-openai-cli-finalize-local-1".to_string(),
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

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-openai-cli-finalize-local-1".to_string(),
            "provider-openai-cli-finalize-local-1".to_string(),
            "openai:cli".to_string(),
            Some("openai".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.openai.example".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"x-endpoint-tag","value":"openai-cli-finalize-local"}
            ])),
            None,
            Some(2),
            Some("/custom/v1/responses".to_string()),
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-openai-cli-finalize-local-1".to_string(),
            "provider-openai-cli-finalize-local-1".to_string(),
            "prod".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:cli"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-upstream-openai-cli-finalize",
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
                    model: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("model"))
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
                });
                Json(json!({
                    "request_id": "trace-openai-cli-stream-sync-direct-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "text/event-stream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(
                            concat!(
                                "event: response.created\n",
                                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_stream_001\",\"object\":\"response\",\"model\":\"gpt-5-upstream\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
                                "event: response.output_text.delta\n",
                                "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n",
                                "event: response.completed\n",
                                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_stream_001\",\"object\":\"response\",\"model\":\"gpt-5-upstream\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n"
                            )
                        )
                    },
                    "telemetry": {
                        "elapsed_ms": 29
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-cli-finalize-local")),
        sample_auth_snapshot(
            "key-openai-cli-finalize-local-123",
            "user-openai-cli-finalize-local-123",
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
            "Bearer sk-client-openai-cli-finalize-local",
        )
        .header(TRACE_ID_HEADER, "trace-openai-cli-stream-sync-direct-123")
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
            "id": "resp_stream_001",
            "object": "response",
            "model": "gpt-5-upstream",
            "status": "completed",
            "output": [{
                "type": "message",
                "id": "resp_stream_001_msg",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello",
                    "annotations": []
                }]
            }],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 2,
                "total_tokens": 3
            }
        })
    );
    assert!(
        elapsed < std::time::Duration::from_millis(5_000),
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
        "trace-openai-cli-stream-sync-direct-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.request_id,
        "trace-openai-cli-stream-sync-direct-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.url,
        "https://api.openai.example/custom/v1/responses"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.model,
        "gpt-5-upstream"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.authorization,
        "Bearer sk-upstream-openai-cli-finalize"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.endpoint_tag,
        "openai-cli-finalize-local"
    );

    let mut stored_candidates = Vec::new();
    for _ in 0..50 {
        stored_candidates = request_candidate_repository
            .list_by_request_id("trace-openai-cli-stream-sync-direct-123")
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
async fn gateway_executes_kiro_claude_cli_sync_upstream_stream_via_local_finalize_response() {
    use base64::Engine as _;

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
                crc = (crc >> 1) ^ mask;
            }
        }
        !crc
    }

    fn encode_string_header(name: &str, value: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        out.push(7);
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
        out
    }

    fn encode_event_frame(
        message_type: &str,
        event_type: Option<&str>,
        payload: serde_json::Value,
    ) -> Vec<u8> {
        let mut headers = encode_string_header(":message-type", message_type);
        if let Some(event_type) = event_type {
            headers.extend_from_slice(&encode_string_header(":event-type", event_type));
        }
        let payload = serde_json::to_vec(&payload).expect("payload should encode");
        let total_len = 12 + headers.len() + payload.len() + 4;
        let mut out = Vec::with_capacity(total_len);
        out.extend_from_slice(&(total_len as u32).to_be_bytes());
        out.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        let prelude_crc = crc32(&out[..8]);
        out.extend_from_slice(&prelude_crc.to_be_bytes());
        out.extend_from_slice(&headers);
        out.extend_from_slice(&payload);
        let message_crc = crc32(&out);
        out.extend_from_slice(&message_crc.to_be_bytes());
        out
    }

    #[derive(Debug, Clone)]
    struct SeenRemoteExecutionRuntimeRequest {
        trace_id: String,
        request_id: String,
        url: String,
        authorization: String,
        accept: String,
        host: String,
        endpoint_tag: String,
        mapped_model: String,
        current_content: String,
        profile_arn: String,
        debug_tag: String,
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
            Some(serde_json::json!(["claude", "kiro"])),
            Some(serde_json::json!(["claude:cli"])),
            Some(serde_json::json!(["claude-sonnet-4"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            Some(serde_json::json!(["claude", "kiro"])),
            Some(serde_json::json!(["claude:cli"])),
            Some(serde_json::json!(["claude-sonnet-4"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-kiro-cli-finalize-local-1".to_string(),
            provider_name: "kiro".to_string(),
            provider_type: "kiro".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-kiro-cli-finalize-local-1".to_string(),
            endpoint_api_format: "claude:cli".to_string(),
            endpoint_api_family: Some("claude".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-kiro-cli-finalize-local-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["claude:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"claude:cli": 1})),
            model_id: "model-kiro-cli-finalize-local-1".to_string(),
            global_model_id: "global-model-kiro-cli-finalize-local-1".to_string(),
            global_model_name: "claude-sonnet-4".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "claude-sonnet-4-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "claude-sonnet-4-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["claude:cli".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-kiro-cli-finalize-local-1".to_string(),
            "kiro".to_string(),
            Some("https://example.com".to_string()),
            "kiro".to_string(),
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
            "endpoint-kiro-cli-finalize-local-1".to_string(),
            "provider-kiro-cli-finalize-local-1".to_string(),
            "claude:cli".to_string(),
            Some("claude".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://kiro.{region}.example?tenant=demo".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"accept","value":"text/plain"},
                {"action":"set","key":"x-endpoint-tag","value":"kiro-cli-finalize-local"}
            ])),
            Some(serde_json::json!([
                {"action":"set","path":"debugTag","value":"kiro-local-finalize"}
            ])),
            Some(2),
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        let auth_config = serde_json::json!({
            "provider_type": "kiro",
            "access_token": "cached-kiro-finalize-access-token",
            "expires_at": 4102444800_u64,
            "refresh_token": "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
            "machine_id": "123e4567-e89b-12d3-a456-426614174000",
            "api_region": "us-east-1",
            "kiro_version": "0.8.0",
            "system_version": "darwin#24.6.0",
            "node_version": "22.21.1",
            "profile_arn": "arn:aws:bedrock:us-east-1:123456789012:inference-profile/demo"
        });

        StoredProviderCatalogKey::new(
            "key-kiro-cli-finalize-local-1".to_string(),
            "provider-kiro-cli-finalize-local-1".to_string(),
            "prod".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["claude:cli"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "__placeholder__")
                .expect("api key should encrypt"),
            Some(
                encrypt_python_fernet_plaintext(
                    DEVELOPMENT_ENCRYPTION_KEY,
                    auth_config.to_string().as_str(),
                )
                .expect("auth config should encrypt"),
            ),
            None,
            Some(serde_json::json!({"claude:cli": 1})),
            None,
            None,
            Some(serde_json::json!({"enabled": true, "node_id":"proxy-node-kiro-cli-finalize-local"})),
            Some(serde_json::json!({"tls_profile":"chrome_136"})),
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
                    *report_hits_inner.lock().expect("mutex should lock") += 1;
                    let (_parts, body) = request.into_parts();
                    let _body = to_bytes(body, usize::MAX).await.expect("body should read");
                    Json(json!({"ok": true}))
                }
            }),
        )
        .route(
            "/v1/messages",
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
                    accept: payload
                        .get("headers")
                        .and_then(|value| value.get("accept"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    host: payload
                        .get("headers")
                        .and_then(|value| value.get("host"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    endpoint_tag: payload
                        .get("headers")
                        .and_then(|value| value.get("x-endpoint-tag"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    mapped_model: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("conversationState"))
                        .and_then(|value| value.get("currentMessage"))
                        .and_then(|value| value.get("userInputMessage"))
                        .and_then(|value| value.get("modelId"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    current_content: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("conversationState"))
                        .and_then(|value| value.get("currentMessage"))
                        .and_then(|value| value.get("userInputMessage"))
                        .and_then(|value| value.get("content"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    profile_arn: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("profileArn"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    debug_tag: payload
                        .get("body")
                        .and_then(|value| value.get("json_body"))
                        .and_then(|value| value.get("debugTag"))
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string(),
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

                let kiro_frames = [
                    encode_event_frame(
                        "event",
                        Some("assistantResponseEvent"),
                        json!({"content": "Hello from Kiro finalize"}),
                    ),
                    encode_event_frame(
                        "event",
                        Some("contextUsageEvent"),
                        json!({"contextUsagePercentage": 1.0}),
                    ),
                ]
                .concat();
                Json(json!({
                    "request_id": "trace-kiro-cli-finalize-local-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/vnd.amazon.eventstream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(kiro_frames)
                    },
                    "telemetry": {
                        "elapsed_ms": 27
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-kiro-cli-finalize-local")),
        sample_auth_snapshot(
            "key-kiro-cli-finalize-local-123",
            "user-kiro-cli-finalize-local-123",
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
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-kiro-cli-finalize-local",
        )
        .header("anthropic-beta", "output-128k-2025-02-19")
        .header(TRACE_ID_HEADER, "trace-kiro-cli-finalize-local-123")
        .body(
            "{\"model\":\"claude-sonnet-4\",\"messages\":[{\"role\":\"user\",\"content\":\"hello kiro\"}],\"thinking\":{\"type\":\"enabled\",\"budget_tokens\":64}}",
        )
        .send()
        .await
        .expect("request should succeed");
    let elapsed = started_at.elapsed();

    let status = response.status();
    let response_text = response.text().await.expect("body should read");
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected gateway response: {response_text}"
    );
    let response_json: serde_json::Value =
        serde_json::from_str(&response_text).expect("body should parse");
    let response_id = response_json
        .get("id")
        .and_then(|value| value.as_str())
        .expect("response id should exist");
    assert_eq!(
        response_json,
        json!({
            "id": response_id,
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-upstream",
            "content": [{
                "type": "text",
                "text": "Hello from Kiro finalize"
            }],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 2000,
                "output_tokens": 6
            }
        })
    );
    assert!(
        response_json
            .get("id")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value.starts_with("msg_")),
        "response id should be generated by the local Kiro rewriter"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(5_000),
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
        "trace-kiro-cli-finalize-local-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.request_id,
        "trace-kiro-cli-finalize-local-123"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.url,
        "https://kiro.us-east-1.example/generateAssistantResponse?tenant=demo"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.authorization,
        "Bearer cached-kiro-finalize-access-token"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.accept,
        "application/vnd.amazon.eventstream"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.host,
        "q.us-east-1.amazonaws.com"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.endpoint_tag,
        "kiro-cli-finalize-local"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.mapped_model,
        "claude-sonnet-4-upstream"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.current_content,
        "<thinking_mode>enabled</thinking_mode><max_thinking_length>64</max_thinking_length>\nhello kiro"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.profile_arn,
        "arn:aws:bedrock:us-east-1:123456789012:inference-profile/demo"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.debug_tag,
        "kiro-local-finalize"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.proxy_node_id,
        "proxy-node-kiro-cli-finalize-local"
    );
    assert_eq!(
        seen_remote_execution_runtime_request.tls_profile,
        "chrome_136"
    );

    let mut stored_candidates = Vec::new();
    for _ in 0..50 {
        stored_candidates = request_candidate_repository
            .list_by_request_id("trace-kiro-cli-finalize-local-123")
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
