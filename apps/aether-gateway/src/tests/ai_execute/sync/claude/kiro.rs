use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override,
    encrypt_python_fernet_plaintext, json, start_server, to_bytes, Arc, Body, Digest,
    InMemoryAuthApiKeySnapshotRepository, InMemoryMinimalCandidateSelectionReadRepository,
    InMemoryProviderCatalogReadRepository, InMemoryRequestCandidateRepository, Json, Mutex,
    Request, RequestCandidateReadRepository, RequestCandidateStatus, Router, Sha256, StatusCode,
    StoredAuthApiKeySnapshot, StoredMinimalCandidateSelectionRow, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogProvider, StoredProviderModelMapping,
    DEVELOPMENT_ENCRYPTION_KEY, TRACE_ID_HEADER,
};

#[tokio::test]
async fn gateway_executes_kiro_claude_cli_sync_via_local_provider_catalog_candidate() {
    use base64::Engine as _;

    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        trace_id: String,
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
            provider_id: "provider-kiro-cli-local-sync-1".to_string(),
            provider_name: "kiro".to_string(),
            provider_type: "kiro".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-kiro-cli-local-sync-1".to_string(),
            endpoint_api_format: "claude:cli".to_string(),
            endpoint_api_family: Some("claude".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-kiro-cli-local-sync-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["claude:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"claude:cli": 1})),
            model_id: "model-kiro-cli-local-sync-1".to_string(),
            global_model_id: "global-model-kiro-cli-local-sync-1".to_string(),
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
            "provider-kiro-cli-local-sync-1".to_string(),
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
            "endpoint-kiro-cli-local-sync-1".to_string(),
            "provider-kiro-cli-local-sync-1".to_string(),
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
                {"action":"set","key":"x-endpoint-tag","value":"kiro-cli-local"}
            ])),
            Some(serde_json::json!([
                {"action":"set","path":"debugTag","value":"kiro-local-sync"}
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
            "node_version": "22.21.1",
            "profile_arn": "arn:aws:bedrock:us-east-1:123456789012:inference-profile/demo"
        });

        StoredProviderCatalogKey::new(
            "key-kiro-cli-local-sync-1".to_string(),
            "provider-kiro-cli-local-sync-1".to_string(),
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
            Some(serde_json::json!({"enabled": true, "node_id":"proxy-node-kiro-cli-local-sync"})),
            Some(serde_json::json!({"tls_profile":"chrome_136"})),
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
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
                        "user_id": "user-kiro-cli-local-sync-123",
                        "api_key_id": "key-kiro-cli-local-sync-123",
                        "access_allowed": true
                    },
                    "public_path": "/v1/messages"
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
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("execution runtime payload should parse");
                *seen_execution_runtime_inner.lock().expect("mutex should lock") =
                    Some(SeenExecutionRuntimeSyncRequest {
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
                        json!({"content": "Hello from Kiro local"}),
                    ),
                    encode_event_frame(
                        "event",
                        Some("contextUsageEvent"),
                        json!({"contextUsagePercentage": 1.0}),
                    ),
                ]
                .concat();

                Json(json!({
                    "request_id": "trace-kiro-cli-local-sync-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/vnd.amazon.eventstream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(kiro_frames)
                    },
                    "telemetry": {
                        "elapsed_ms": 29
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-kiro-cli-local-sync")),
        sample_auth_snapshot(
            "key-kiro-cli-local-sync-123",
            "user-kiro-cli-local-sync-123",
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
            "Bearer sk-client-kiro-cli-local-sync",
        )
        .header(TRACE_ID_HEADER, "trace-kiro-cli-local-sync-123")
        .body(
            "{\"model\":\"claude-sonnet-4\",\"messages\":[{\"role\":\"user\",\"content\":\"hello kiro\"}],\"thinking\":{\"type\":\"enabled\",\"budget_tokens\":64}}",
        )
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let response_body = response.text().await.expect("body should read");
    assert!(
        status == StatusCode::OK,
        "unexpected status={status} body={response_body} decision_hits={} plan_hits={} public_hits={}",
        *decision_hits.lock().expect("mutex should lock"),
        *plan_hits.lock().expect("mutex should lock"),
        *public_hits.lock().expect("mutex should lock"),
    );
    let response_json: serde_json::Value =
        serde_json::from_str(&response_body).expect("body should parse");
    assert_eq!(response_json["model"], "claude-sonnet-4-upstream");
    assert_eq!(
        response_json["content"][0]["text"],
        json!("Hello from Kiro local")
    );

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-kiro-cli-local-sync-123"
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
        "kiro-cli-local"
    );
    assert_eq!(
        seen_execution_runtime_request.mapped_model,
        "claude-sonnet-4-upstream"
    );
    assert_eq!(
        seen_execution_runtime_request.current_content,
        "<thinking_mode>enabled</thinking_mode><max_thinking_length>64</max_thinking_length>\nhello kiro"
    );
    assert_eq!(
        seen_execution_runtime_request.profile_arn,
        "arn:aws:bedrock:us-east-1:123456789012:inference-profile/demo"
    );
    assert_eq!(seen_execution_runtime_request.debug_tag, "kiro-local-sync");
    assert_eq!(
        seen_execution_runtime_request.proxy_node_id,
        "proxy-node-kiro-cli-local-sync"
    );
    assert_eq!(seen_execution_runtime_request.tls_profile, "chrome_136");

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-kiro-cli-local-sync-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !*seen_report.lock().expect("mutex should lock"),
        "report-sync should stay local when request candidate persistence is available"
    );

    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_executes_kiro_claude_cli_sync_via_local_provider_catalog_candidate_after_refresh()
{
    use base64::Engine as _;

    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        trace_id: String,
        authorization: String,
        mapped_model: String,
        current_content: String,
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
            provider_id: "provider-kiro-cli-local-refresh-1".to_string(),
            provider_name: "kiro".to_string(),
            provider_type: "kiro".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-kiro-cli-local-refresh-1".to_string(),
            endpoint_api_format: "claude:cli".to_string(),
            endpoint_api_family: Some("claude".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-kiro-cli-local-refresh-1".to_string(),
            key_name: "prod".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["claude:cli".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"claude:cli": 1})),
            model_id: "model-kiro-cli-local-refresh-1".to_string(),
            global_model_id: "global-model-kiro-cli-local-refresh-1".to_string(),
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
            "provider-kiro-cli-local-refresh-1".to_string(),
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
            None,
            Some(20.0),
            None,
            None,
        )
    }

    fn sample_provider_catalog_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-kiro-cli-local-refresh-1".to_string(),
            "provider-kiro-cli-local-refresh-1".to_string(),
            "claude:cli".to_string(),
            Some("claude".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://kiro.{region}.example?tenant=demo".to_string(),
            Some(serde_json::json!([
                {"action":"set","key":"x-endpoint-tag","value":"kiro-cli-local"}
            ])),
            Some(serde_json::json!([
                {"action":"set","path":"debugTag","value":"kiro-local-refresh"}
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
            "access_token": "stale-kiro-access-token",
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
            "key-kiro-cli-local-refresh-1".to_string(),
            "provider-kiro-cli-local-refresh-1".to_string(),
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
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_execution_runtime = Arc::new(Mutex::new(None::<SeenExecutionRuntimeSyncRequest>));
    let seen_execution_runtime_clone = Arc::clone(&seen_execution_runtime);
    let execution_hits = Arc::new(Mutex::new(0usize));
    let execution_hits_clone = Arc::clone(&execution_hits);
    let refresh_hits = Arc::new(Mutex::new(0usize));
    let refresh_hits_clone = Arc::clone(&refresh_hits);

    let refresh_server = Router::new().route(
        "/refreshToken",
        any(move |request: Request| {
            let refresh_hits_inner = Arc::clone(&refresh_hits_clone);
            async move {
                *refresh_hits_inner.lock().expect("mutex should lock") += 1;
                let raw_body = to_bytes(request.into_body(), usize::MAX)
                    .await
                    .expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("refresh payload should parse");
                assert_eq!(
                    payload.get("refreshToken").and_then(|value| value.as_str()),
                    Some(
                        "rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr"
                    )
                );
                Json(json!({
                    "accessToken": "refreshed-kiro-access-token",
                    "refreshToken": "ssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssssss",
                    "expiresIn": 3600,
                    "profileArn": "arn:aws:bedrock:us-east-1:123456789012:inference-profile/demo"
                }))
            }
        }),
    );

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
                        "user_id": "user-kiro-cli-local-refresh-123",
                        "api_key_id": "key-kiro-cli-local-refresh-123",
                        "access_allowed": true
                    },
                    "public_path": "/v1/messages"
                }))
            }),
        )
        .route(
            "/api/internal/gateway/decision-sync",
            any(|_request: Request| async move { Json(json!({"action": "proxy_public"})) }),
        )
        .route(
            "/api/internal/gateway/plan-sync",
            any(|_request: Request| async move { Json(json!({"action": "proxy_public"})) }),
        )
        .route(
            "/api/internal/gateway/report-sync",
            any(|_request: Request| async move { Json(json!({"ok": true})) }),
        )
        .route(
            "/v1/messages",
            any(|_request: Request| async move {
                (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
            }),
        );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_execution_runtime_inner = Arc::clone(&seen_execution_runtime_clone);
            let execution_hits_inner = Arc::clone(&execution_hits_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&raw_body).expect("execution runtime payload should parse");
                *execution_hits_inner.lock().expect("mutex should lock") += 1;
                let authorization = payload
                    .get("headers")
                    .and_then(|value| value.get("authorization"))
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                *seen_execution_runtime_inner.lock().expect("mutex should lock") =
                    Some(SeenExecutionRuntimeSyncRequest {
                        trace_id: parts
                            .headers
                            .get(TRACE_ID_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        authorization: authorization.clone(),
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
                    });

                if authorization == "Bearer stale-kiro-access-token" {
                    return Json(json!({
                        "request_id": "trace-kiro-cli-local-refresh-123",
                        "status_code": 401,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "message": "The security token included in the request is expired"
                            }
                        },
                        "telemetry": {
                            "elapsed_ms": 7
                        }
                    }));
                }

                let kiro_frames = [
                    encode_event_frame(
                        "event",
                        Some("assistantResponseEvent"),
                        json!({"content": "Hello from Kiro refresh"}),
                    ),
                    encode_event_frame(
                        "event",
                        Some("contextUsageEvent"),
                        json!({"contextUsagePercentage": 1.0}),
                    ),
                ]
                .concat();

                Json(json!({
                    "request_id": "trace-kiro-cli-local-refresh-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/vnd.amazon.eventstream"
                    },
                    "body": {
                        "body_bytes_b64": base64::engine::general_purpose::STANDARD.encode(kiro_frames)
                    },
                    "telemetry": {
                        "elapsed_ms": 31
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-kiro-cli-local-refresh")),
        sample_auth_snapshot(
            "key-kiro-cli-local-refresh-123",
            "user-kiro-cli-local-refresh-123",
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

    let (refresh_url, refresh_handle) = start_server(refresh_server).await;
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let oauth_refresh =
        crate::provider_transport::LocalOAuthRefreshCoordinator::with_adapters_for_tests(vec![
            Arc::new(
                crate::provider_transport::kiro::KiroOAuthRefreshAdapter::default()
                    .with_refresh_base_urls(Some(refresh_url), None),
            )
                as Arc<dyn crate::provider_transport::oauth_refresh::LocalOAuthRefreshAdapter>,
        ]);
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url.clone())
    .with_data_state_for_tests(
        crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
            auth_repository,
            candidate_selection_repository,
            provider_catalog_repository,
            Arc::clone(&request_candidate_repository),
            DEVELOPMENT_ENCRYPTION_KEY,
        ),
    )
    .with_oauth_refresh_coordinator_for_tests(oauth_refresh);
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-kiro-cli-local-refresh",
        )
        .header(TRACE_ID_HEADER, "trace-kiro-cli-local-refresh-123")
        .body("{\"model\":\"claude-sonnet-4\",\"messages\":[{\"role\":\"user\",\"content\":\"hello refresh\"}]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let response_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(response_json["model"], "claude-sonnet-4-upstream");
    assert_eq!(
        response_json["content"][0]["text"],
        json!("Hello from Kiro refresh")
    );

    assert_eq!(*refresh_hits.lock().expect("mutex should lock"), 1);
    assert_eq!(*execution_hits.lock().expect("mutex should lock"), 2);
    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-kiro-cli-local-refresh-123"
    );
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer refreshed-kiro-access-token"
    );
    assert_eq!(
        seen_execution_runtime_request.mapped_model,
        "claude-sonnet-4-upstream"
    );
    assert_eq!(
        seen_execution_runtime_request.current_content,
        "hello refresh"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
    refresh_handle.abort();
}
