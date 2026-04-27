use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    to_bytes, Arc, Body, Bytes, HeaderName, HeaderValue, Json, Mutex, Request, Response, Router,
    StatusCode, TRACE_ID_HEADER,
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
async fn gateway_executes_kiro_claude_cli_stream_via_local_provider_catalog_candidate() {
    use base64::Engine as _;

    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeStreamRequest {
        trace_id: String,
        url: String,
        authorization: String,
        accept: String,
        host: String,
        endpoint_tag: String,
        mapped_model: String,
        current_content: String,
        debug_tag: String,
        proxy_node_id: String,
        tls_profile: String,
    }

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
            provider_id: "provider-kiro-cli-local-stream-1".to_string(),
            provider_name: "kiro".to_string(),
            provider_type: "kiro".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-kiro-cli-local-stream-1".to_string(),
            endpoint_api_format: "claude:cli".to_string(),
            endpoint_api_family: Some("claude".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-kiro-cli-local-stream-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["claude:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"claude:cli": 1})),
            model_id: "model-kiro-cli-local-stream-1".to_string(),
            global_model_id: "global-model-kiro-cli-local-stream-1".to_string(),
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
            "provider-kiro-cli-local-stream-1".to_string(),
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
            "endpoint-kiro-cli-local-stream-1".to_string(),
            "provider-kiro-cli-local-stream-1".to_string(),
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
                {"action":"set","key":"x-endpoint-tag","value":"kiro-cli-local-stream"}
            ])),
            Some(serde_json::json!([
                {"action":"set","path":"debugTag","value":"kiro-local-stream"}
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
            "access_token": "cached-kiro-access-token",
            "expires_at": 4102444800_u64,
            "refresh_token": "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
            "machine_id": "123e4567-e89b-12d3-a456-426614174000",
            "api_region": "us-east-1",
            "kiro_version": "0.8.0",
            "system_version": "darwin#24.6.0",
            "node_version": "22.21.1"
        });

        StoredProviderCatalogKey::new(
            "key-kiro-cli-local-stream-1".to_string(),
            "provider-kiro-cli-local-stream-1".to_string(),
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
            Some(
                serde_json::json!({"enabled": true, "node_id":"proxy-node-kiro-cli-local-stream"}),
            ),
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
                    "route_family": "claude",
                    "route_kind": "cli",
                    "auth_endpoint_signature": "claude:cli",
                    "execution_runtime_candidate": true,
                    "auth_context": {
                        "user_id": "user-kiro-cli-local-stream-123",
                        "api_key_id": "key-kiro-cli-local-stream-123",
                        "access_allowed": true
                    },
                    "public_path": "/v1/messages"
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
                        json!({"content": "Hello from Kiro stream"}),
                    ),
                    encode_event_frame(
                        "event",
                        Some("contextUsageEvent"),
                        json!({"contextUsagePercentage": 1.0}),
                    ),
                ]
                .concat();
                let frames = format!(
                    concat!(
                        "{{\"type\":\"headers\",\"payload\":{{\"kind\":\"headers\",\"status_code\":200,\"headers\":{{\"content-type\":\"application/vnd.amazon.eventstream\"}}}}}}\n",
                        "{{\"type\":\"data\",\"payload\":{{\"kind\":\"data\",\"chunk_b64\":\"{}\"}}}}\n",
                        "{{\"type\":\"telemetry\",\"payload\":{{\"kind\":\"telemetry\",\"telemetry\":{{\"elapsed_ms\":27,\"upstream_bytes\":64}}}}}}\n",
                        "{{\"type\":\"eof\",\"payload\":{{\"kind\":\"eof\"}}}}\n"
                    ),
                    base64::engine::general_purpose::STANDARD.encode(kiro_frames)
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
        Some(hash_api_key("sk-client-kiro-cli-local-stream")),
        sample_auth_snapshot(
            "key-kiro-cli-local-stream-123",
            "user-kiro-cli-local-stream-123",
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
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url.clone())
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
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-kiro-cli-local-stream",
        )
        .header(TRACE_ID_HEADER, "trace-kiro-cli-local-stream-123")
        .body("{\"model\":\"claude-sonnet-4\",\"messages\":[{\"role\":\"user\",\"content\":\"hello stream\"}],\"stream\":true}")
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
    let body = response.text().await.expect("body should read");
    assert!(body.contains("Hello from Kiro stream"));
    assert!(body.contains("event: message_start"));

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime stream should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-kiro-cli-local-stream-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://kiro.us-east-1.example/generateAssistantResponse?tenant=demo"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer cached-kiro-access-token"
    );
    assert_eq!(
        seen_execution_runtime_request.accept,
        "application/vnd.amazon.eventstream"
    );
    assert_eq!(
        seen_execution_runtime_request.host,
        "q.us-east-1.amazonaws.com"
    );
    assert_eq!(
        seen_execution_runtime_request.endpoint_tag,
        "kiro-cli-local-stream"
    );
    assert_eq!(
        seen_execution_runtime_request.mapped_model,
        "claude-sonnet-4-upstream"
    );
    assert_eq!(
        seen_execution_runtime_request.current_content,
        "hello stream"
    );
    assert_eq!(
        seen_execution_runtime_request.debug_tag,
        "kiro-local-stream"
    );
    assert_eq!(
        seen_execution_runtime_request.proxy_node_id,
        "proxy-node-kiro-cli-local-stream"
    );
    assert_eq!(seen_execution_runtime_request.tls_profile, "chrome_136");

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-kiro-cli-local-stream-123")
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
async fn gateway_executes_claude_cli_stream_via_local_decision_gate_without_waiting_for_same_format_prefetch(
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
        metadata_mode: String,
        metadata_source: String,
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
            Some(serde_json::json!(["claude"])),
            Some(serde_json::json!(["claude:cli"])),
            Some(serde_json::json!(["claude-code"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            Some(serde_json::json!(["claude"])),
            Some(serde_json::json!(["claude:cli"])),
            Some(serde_json::json!(["claude-code"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-claude-cli-local-1".to_string(),
            provider_name: "claude".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-claude-cli-local-1".to_string(),
            endpoint_api_format: "claude:cli".to_string(),
            endpoint_api_family: Some("claude".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-claude-cli-local-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["claude:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"claude:cli": 1})),
            model_id: "model-claude-cli-local-1".to_string(),
            global_model_id: "global-model-claude-cli-local-1".to_string(),
            global_model_name: "claude-code".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "claude-code-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "claude-code-upstream".to_string(),
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
            "provider-claude-cli-local-1".to_string(),
            "claude".to_string(),
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
            "endpoint-claude-cli-local-1".to_string(),
            "provider-claude-cli-local-1".to_string(),
            "claude:cli".to_string(),
            Some("claude".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.anthropic.example".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"x-endpoint-tag","value":"claude-cli-local"}
            ])),
            Some(serde_json::json!([
                {"action":"set","path":"metadata.mode","value":"safe"},
                {"action":"rename","from":"metadata.client","to":"metadata.source"}
            ])),
            Some(2),
            Some("/custom/v1/messages".to_string()),
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-claude-cli-local-1".to_string(),
            "provider-claude-cli-local-1".to_string(),
            "prod".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["claude:cli"])),
            encrypt_python_fernet_plaintext(DEVELOPMENT_ENCRYPTION_KEY, "sk-upstream-claude-cli")
                .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"claude:cli": 1})),
            None,
            None,
            Some(serde_json::json!({"enabled": true, "node_id":"proxy-node-claude-cli-local"})),
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
                let body_stream = async_stream::stream! {
                    yield Ok::<Bytes, std::convert::Infallible>(Bytes::from_static(
                        b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n"
                    ));
                    yield Ok::<Bytes, std::convert::Infallible>(Bytes::from_static(
                        b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"event: message_start\\ndata: {\\\"type\\\":\\\"message_start\\\"}\\n\\n\"}}\n"
                    ));
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    yield Ok::<Bytes, std::convert::Infallible>(Bytes::from_static(
                        b"{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":31,\"ttfb_ms\":11,\"upstream_bytes\":37}}}\n"
                    ));
                    yield Ok::<Bytes, std::convert::Infallible>(Bytes::from_static(
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
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-claude-cli-local")),
        sample_auth_snapshot("api-key-claude-cli-local-1", "user-claude-cli-local-1"),
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
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url.clone())
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

    let mut response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-claude-cli-local",
        )
        .header(TRACE_ID_HEADER, "trace-claude-cli-local-stream-123")
        .body(
            "{\"model\":\"claude-code\",\"messages\":[],\"stream\":true,\"metadata\":{\"client\":\"desktop-claude-cli\"}}",
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_millis(100), response.chunk())
            .await
            .expect("same-format passthrough should yield first chunk before eof")
            .expect("first chunk should read")
            .expect("first chunk should exist"),
        Bytes::from_static(b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n")
    );
    assert_eq!(
        response.text().await.expect("remaining body should read"),
        ""
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime stream should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-claude-cli-local-stream-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.anthropic.example/custom/v1/messages"
    );
    assert_eq!(seen_execution_runtime_request.model, "claude-code-upstream");
    assert!(seen_execution_runtime_request.stream);
    assert_eq!(seen_execution_runtime_request.accept, "text/event-stream");
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-claude-cli"
    );
    assert_eq!(
        seen_execution_runtime_request.endpoint_tag,
        "claude-cli-local"
    );
    assert_eq!(seen_execution_runtime_request.metadata_mode, "safe");
    assert_eq!(
        seen_execution_runtime_request.metadata_source,
        "desktop-claude-cli"
    );
    assert_eq!(
        seen_execution_runtime_request.proxy_node_id,
        "proxy-node-claude-cli-local"
    );
    assert_eq!(seen_execution_runtime_request.tls_profile, "chrome_136");

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-claude-cli-local-stream-123")
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
async fn gateway_executes_claude_code_cli_stream_via_local_decision_gate_with_local_stream_decision(
) {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeStreamRequest {
        trace_id: String,
        url: String,
        model: String,
        stream: bool,
        accept: String,
        authorization: String,
        anthropic_version: String,
        anthropic_beta: String,
        x_app: String,
        x_stainless_helper_method: String,
        user_agent: String,
        endpoint_tag: String,
        metadata_mode: String,
        metadata_source: String,
        assistant_content: serde_json::Value,
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
            Some(serde_json::json!(["claude", "claude_code"])),
            Some(serde_json::json!(["claude:cli"])),
            Some(serde_json::json!(["claude-code"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            Some(serde_json::json!(["claude", "claude_code"])),
            Some(serde_json::json!(["claude:cli"])),
            Some(serde_json::json!(["claude-code"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-claude-code-cli-local-1".to_string(),
            provider_name: "claude_code".to_string(),
            provider_type: "claude_code".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-claude-code-cli-local-1".to_string(),
            endpoint_api_format: "claude:cli".to_string(),
            endpoint_api_family: Some("claude".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-claude-code-cli-local-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "oauth".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["claude:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"claude:cli": 1})),
            model_id: "model-claude-code-cli-local-1".to_string(),
            global_model_id: "global-model-claude-code-cli-local-1".to_string(),
            global_model_name: "claude-code".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "claude-code-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "claude-code-upstream".to_string(),
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
            "provider-claude-code-cli-local-1".to_string(),
            "claude_code".to_string(),
            Some("https://example.com".to_string()),
            "claude_code".to_string(),
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
            Some(serde_json::json!({
                "claude_code_advanced": {
                    "cli_only_enabled": false
                }
            })),
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-claude-code-cli-local-1".to_string(),
            "provider-claude-code-cli-local-1".to_string(),
            "claude:cli".to_string(),
            Some("claude".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.anthropic.example/v1/messages".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"x-endpoint-tag","value":"claude-code-cli-local"}
            ])),
            Some(serde_json::json!([
                {"action":"set","path":"metadata.mode","value":"safe"},
                {"action":"rename","from":"metadata.client","to":"metadata.source"}
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
        StoredProviderCatalogKey::new(
            "key-claude-code-cli-local-1".to_string(),
            "provider-claude-code-cli-local-1".to_string(),
            "prod".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["claude:cli"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-upstream-claude-code-oauth",
            )
            .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"claude:cli": 1})),
            None,
            None,
            Some(
                serde_json::json!({"enabled": true, "node_id":"proxy-node-claude-code-cli-local"}),
            ),
            Some(serde_json::json!({
                "tls_profile":"claude_code_nodejs",
                "user_agent":"Claude-Code/9.9"
            })),
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
                        anthropic_version: payload
                            .get("headers")
                            .and_then(|value| value.get("anthropic-version"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        anthropic_beta: payload
                            .get("headers")
                            .and_then(|value| value.get("anthropic-beta"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        x_app: payload
                            .get("headers")
                            .and_then(|value| value.get("x-app"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        x_stainless_helper_method: payload
                            .get("headers")
                            .and_then(|value| value.get("x-stainless-helper-method"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        user_agent: payload
                            .get("headers")
                            .and_then(|value| value.get("user-agent"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        endpoint_tag: payload
                            .get("headers")
                            .and_then(|value| value.get("x-endpoint-tag"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
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
                        assistant_content: payload
                            .get("body")
                            .and_then(|value| value.get("json_body"))
                            .and_then(|value| value.get("messages"))
                            .and_then(|value| value.get(0))
                            .and_then(|value| value.get("content"))
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
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
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"event: message_start\\ndata: {\\\"type\\\":\\\"message_start\\\"}\\n\\n\"}}\n",
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
        Some(hash_api_key("sk-client-claude-code-cli-local")),
        sample_auth_snapshot(
            "api-key-claude-code-cli-local-1",
            "user-claude-code-cli-local-1",
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
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url.clone())
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
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-claude-code-cli-local",
        )
        .header("anthropic-beta", "context-1m-2025-08-07,custom-beta")
        .header(TRACE_ID_HEADER, "trace-claude-code-cli-local-stream-123")
        .body(
            serde_json::json!({
                "model":"claude-code",
                "stream":true,
                "thinking":{"type":"enabled"},
                "messages":[{
                    "role":"assistant",
                    "content":[
                        {"type":"thinking","thinking":"keep","signature":"sig_valid"},
                        {"type":"thinking","thinking":"drop-empty-signature","signature":""},
                        {"type":"redacted_thinking","data":"keep-redacted","signature":"sig_redacted"},
                        {"type":"redacted_thinking","data":"drop-no-signature"},
                        {"type":"text","text":"ok"}
                    ]
                }],
                "metadata":{"client":"desktop-claude-code-cli"}
            })
            .to_string(),
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.text().await.expect("body should read"),
        "event: message_start\ndata: {\"type\":\"message_start\"}\n\n"
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime stream should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-claude-code-cli-local-stream-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.anthropic.example/v1/messages"
    );
    assert_eq!(seen_execution_runtime_request.model, "claude-code-upstream");
    assert!(seen_execution_runtime_request.stream);
    assert_eq!(seen_execution_runtime_request.accept, "text/event-stream");
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-claude-code-oauth"
    );
    assert_eq!(
        seen_execution_runtime_request.anthropic_version,
        "2023-06-01"
    );
    assert_eq!(
        seen_execution_runtime_request.anthropic_beta,
        "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,custom-beta"
    );
    assert_eq!(seen_execution_runtime_request.x_app, "cli");
    assert_eq!(
        seen_execution_runtime_request.x_stainless_helper_method,
        "stream"
    );
    assert_eq!(seen_execution_runtime_request.user_agent, "Claude-Code/9.9");
    assert_eq!(
        seen_execution_runtime_request.endpoint_tag,
        "claude-code-cli-local"
    );
    assert_eq!(seen_execution_runtime_request.metadata_mode, "safe");
    assert_eq!(
        seen_execution_runtime_request.metadata_source,
        "desktop-claude-code-cli"
    );
    assert_eq!(
        seen_execution_runtime_request.assistant_content,
        json!([
            {"type":"thinking","thinking":"keep","signature":"sig_valid"},
            {"type":"redacted_thinking","data":"keep-redacted","signature":"sig_redacted"},
            {"type":"text","text":"ok"}
        ])
    );
    assert_eq!(
        seen_execution_runtime_request.proxy_node_id,
        "proxy-node-claude-code-cli-local"
    );
    assert_eq!(
        seen_execution_runtime_request.tls_profile,
        "claude_code_nodejs"
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-claude-code-cli-local-stream-123")
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
async fn gateway_executes_claude_chat_stream_via_local_decision_gate_with_local_stream_decision() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeStreamRequest {
        trace_id: String,
        url: String,
        model: String,
        stream: bool,
        accept: String,
        auth_header_value: String,
        endpoint_tag: String,
        metadata_mode: String,
        metadata_source: String,
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
            Some(serde_json::json!(["claude"])),
            Some(serde_json::json!(["claude:chat"])),
            Some(serde_json::json!(["claude-sonnet-4-5"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            Some(serde_json::json!(["claude"])),
            Some(serde_json::json!(["claude:chat"])),
            Some(serde_json::json!(["claude-sonnet-4-5"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-claude-chat-local-stream-1".to_string(),
            provider_name: "claude".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-claude-chat-local-stream-1".to_string(),
            endpoint_api_format: "claude:chat".to_string(),
            endpoint_api_family: Some("claude".to_string()),
            endpoint_kind: Some("chat".to_string()),
            endpoint_is_active: true,
            key_id: "key-claude-chat-local-stream-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "api_key".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["claude:chat".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"claude:chat": 1})),
            model_id: "model-claude-chat-local-stream-1".to_string(),
            global_model_id: "global-model-claude-chat-local-stream-1".to_string(),
            global_model_name: "claude-sonnet-4-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "claude-sonnet-4-5-upstream".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "claude-sonnet-4-5-upstream".to_string(),
                priority: 1,
                api_formats: Some(vec!["claude:chat".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-claude-chat-local-stream-1".to_string(),
            "claude".to_string(),
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
            "endpoint-claude-chat-local-stream-1".to_string(),
            "provider-claude-chat-local-stream-1".to_string(),
            "claude:chat".to_string(),
            Some("claude".to_string()),
            Some("chat".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://api.anthropic.example".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"x-endpoint-tag","value":"claude-chat-local-stream"}
            ])),
            Some(serde_json::json!([
                {"action":"set","path":"metadata.mode","value":"safe"},
                {"action":"rename","from":"metadata.client","to":"metadata.source"}
            ])),
            Some(2),
            Some("/custom/v1/messages".to_string()),
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-claude-chat-local-stream-1".to_string(),
            "provider-claude-chat-local-stream-1".to_string(),
            "prod".to_string(),
            "api_key".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["claude:chat"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-upstream-claude-chat-stream",
            )
            .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"claude:chat": 1})),
            None,
            None,
            Some(serde_json::json!({"enabled": true, "node_id":"proxy-node-claude-chat-stream"})),
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
                        auth_header_value: payload
                            .get("headers")
                            .and_then(|value| value.get("x-api-key"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        endpoint_tag: payload
                            .get("headers")
                            .and_then(|value| value.get("x-endpoint-tag"))
                            .and_then(|value| value.as_str())
                            .unwrap_or_default()
                            .to_string(),
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
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"event: message_start\\ndata: {\\\"type\\\":\\\"message_start\\\"}\\n\\n\"}}\n",
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
        Some(hash_api_key("sk-client-claude-chat-stream-local")),
        sample_auth_snapshot(
            "api-key-claude-chat-local-stream-1",
            "user-claude-chat-local-stream-1",
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
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url.clone())
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
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", "sk-client-claude-chat-stream-local")
        .header(TRACE_ID_HEADER, "trace-claude-chat-local-stream-123")
        .body(
            "{\"model\":\"claude-sonnet-4-5\",\"messages\":[],\"stream\":true,\"metadata\":{\"client\":\"desktop-claude-stream\"}}",
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.text().await.expect("body should read"),
        "event: message_start\ndata: {\"type\":\"message_start\"}\n\n"
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime stream should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-claude-chat-local-stream-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.anthropic.example/custom/v1/messages"
    );
    assert_eq!(
        seen_execution_runtime_request.model,
        "claude-sonnet-4-5-upstream"
    );
    assert!(seen_execution_runtime_request.stream);
    assert_eq!(seen_execution_runtime_request.accept, "text/event-stream");
    assert_eq!(
        seen_execution_runtime_request.auth_header_value,
        "sk-upstream-claude-chat-stream"
    );
    assert_eq!(
        seen_execution_runtime_request.endpoint_tag,
        "claude-chat-local-stream"
    );
    assert_eq!(seen_execution_runtime_request.metadata_mode, "safe");
    assert_eq!(
        seen_execution_runtime_request.metadata_source,
        "desktop-claude-stream"
    );
    assert_eq!(
        seen_execution_runtime_request.proxy_node_id,
        "proxy-node-claude-chat-stream"
    );
    assert_eq!(seen_execution_runtime_request.tls_profile, "chrome_136");

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-claude-chat-local-stream-123")
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
