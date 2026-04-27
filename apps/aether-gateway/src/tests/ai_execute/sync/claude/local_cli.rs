use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override,
    encrypt_python_fernet_plaintext, json, start_server, to_bytes, Arc, Body, Digest,
    InMemoryAuthApiKeySnapshotRepository, InMemoryMinimalCandidateSelectionReadRepository,
    InMemoryProviderCatalogReadRepository, InMemoryRequestCandidateRepository, Json, Mutex,
    Request, RequestCandidateReadRepository, RequestCandidateStatus, Router, Sha256, StatusCode,
    StoredAuthApiKeySnapshot, StoredMinimalCandidateSelectionRow, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogProvider, StoredProviderModelMapping,
    DEVELOPMENT_ENCRYPTION_KEY, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC, EXECUTION_PATH_HEADER,
    EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS, LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER,
    TRACE_ID_HEADER,
};

#[tokio::test]
async fn gateway_executes_claude_cli_sync_via_local_decision_gate_with_local_sync_decision() {
    #[derive(Debug, Clone)]
    struct SeenExecutionRuntimeSyncRequest {
        trace_id: String,
        url: String,
        model: String,
        authorization: String,
        anthropic_version: String,
        anthropic_beta: String,
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
                let payload: serde_json::Value = serde_json::from_slice(&raw_body)
                    .expect("execution runtime payload should parse");
                *seen_execution_runtime_inner
                    .lock()
                    .expect("mutex should lock") = Some(SeenExecutionRuntimeSyncRequest {
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
                Json(json!({
                    "request_id": "trace-claude-cli-local-sync-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "msg-local-claude-cli-123",
                            "type": "message",
                            "model": "claude-code-upstream",
                            "role": "assistant",
                            "content": [],
                            "usage": {
                                "input_tokens": 2,
                                "output_tokens": 3
                            }
                        }
                    },
                    "telemetry": {
                        "elapsed_ms": 29
                    }
                }))
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

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-claude-cli-local",
        )
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "prompt-caching-2024-07-31")
        .header(TRACE_ID_HEADER, "trace-claude-cli-local-sync-123")
        .body(
            "{\"model\":\"claude-code\",\"messages\":[],\"metadata\":{\"client\":\"desktop-claude-cli\"}}",
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
    assert_eq!(response_json["model"], "claude-code-upstream");

    let seen_execution_runtime_request = seen_execution_runtime
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("execution runtime sync should be captured");
    assert_eq!(
        seen_execution_runtime_request.trace_id,
        "trace-claude-cli-local-sync-123"
    );
    assert_eq!(
        seen_execution_runtime_request.url,
        "https://api.anthropic.example/custom/v1/messages"
    );
    assert_eq!(seen_execution_runtime_request.model, "claude-code-upstream");
    assert_eq!(
        seen_execution_runtime_request.authorization,
        "Bearer sk-upstream-claude-cli"
    );
    assert_eq!(
        seen_execution_runtime_request.anthropic_version,
        "2023-06-01"
    );
    assert_eq!(
        seen_execution_runtime_request.anthropic_beta,
        "prompt-caching-2024-07-31"
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
        .list_by_request_id("trace-claude-cli-local-sync-123")
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
async fn gateway_returns_claude_cli_error_for_local_sync_failure() {
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

    let seen_report = Arc::new(Mutex::new(false));
    let seen_report_clone = Arc::clone(&seen_report);

    let upstream = Router::new().route(
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
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| async move {
            Json(json!({
                "request_id": "trace-claude-cli-local-error-123",
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "type": "error",
                        "error": {
                            "type": "rate_limit_error",
                            "message": "slow down"
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 29
                }
            }))
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-claude-cli-local-error")),
        sample_auth_snapshot(
            "api-key-claude-cli-local-error-1",
            "user-claude-cli-local-error-1",
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

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
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
            "Bearer sk-client-claude-cli-local-error",
        )
        .header(TRACE_ID_HEADER, "trace-claude-cli-local-error-123")
        .body(
            "{\"model\":\"claude-code\",\"messages\":[],\"metadata\":{\"client\":\"desktop-claude-cli\"}}",
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_SYNC)
    );
    let response_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(
        response_json,
        json!({
            "type": "error",
            "error": {
                "type": "rate_limit_error",
                "message": "slow down"
            }
        })
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-claude-cli-local-error-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Failed);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !*seen_report.lock().expect("mutex should lock"),
        "report-sync should stay local when request candidate persistence is available"
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_marks_claude_cli_cross_format_runtime_miss_when_format_conversion_is_disabled() {
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
            None,
            Some(serde_json::json!(["claude:cli"])),
            Some(serde_json::json!(["gpt-5.4"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800),
            None,
            Some(serde_json::json!(["claude:cli"])),
            Some(serde_json::json!(["gpt-5.4"])),
        )
        .expect("auth snapshot should build")
    }

    fn sample_candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-claude-cli-openai-local-miss-1".to_string(),
            provider_name: "RightCode".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-claude-cli-openai-local-miss-1".to_string(),
            endpoint_api_format: "openai:responses".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-claude-cli-openai-local-miss-1".to_string(),
            key_name: "codex".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:responses".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:responses": 1})),
            model_id: "model-claude-cli-openai-local-miss-1".to_string(),
            global_model_id: "global-model-claude-cli-openai-local-miss-1".to_string(),
            global_model_name: "gpt-5.4".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5.4".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-5.4".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:responses".to_string()]),
            }]),
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn sample_provider_catalog_provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-claude-cli-openai-local-miss-1".to_string(),
            "RightCode".to_string(),
            Some("https://right.codes".to_string()),
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
            "endpoint-claude-cli-openai-local-miss-1".to_string(),
            "provider-claude-cli-openai-local-miss-1".to_string(),
            "openai:responses".to_string(),
            Some("openai".to_string()),
            Some("cli".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://right.codes/codex".to_string(),
            None,
            None,
            Some(2),
            Some("/v1/messages".to_string()),
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn sample_provider_catalog_key() -> StoredProviderCatalogKey {
        StoredProviderCatalogKey::new(
            "key-claude-cli-openai-local-miss-1".to_string(),
            "provider-claude-cli-openai-local-miss-1".to_string(),
            "codex".to_string(),
            "bearer".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(serde_json::json!(["openai:responses"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "sk-upstream-openai-cli-local-miss",
            )
            .expect("api key should encrypt"),
            None,
            None,
            Some(serde_json::json!({"openai:responses": 1})),
            None,
            None,
            None,
            None,
        )
        .expect("key transport should build")
    }

    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);
    let upstream = Router::new().route(
        "/v1/messages",
        any(move |_request: Request| {
            let public_hits_inner = Arc::clone(&public_hits_clone);
            async move {
                *public_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::IM_A_TEAPOT, Body::from("public-route-hit"))
            }
        }),
    );
    let execution_runtime = Router::new();

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-claude-cli-openai-local-miss")),
        sample_auth_snapshot(
            "api-key-claude-cli-openai-local-miss-1",
            "user-claude-cli-openai-local-miss-1",
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

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
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
        .post(format!("{gateway_url}/v1/messages?beta=true"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-claude-cli-openai-local-miss",
        )
        .header(TRACE_ID_HEADER, "trace-claude-cli-openai-local-miss-123")
        .body("{\"model\":\"gpt-5.4\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS)
    );
    assert_eq!(
        response
            .headers()
            .get(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("candidate_list_empty")
    );
    let response_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(response_json["error"]["type"], "http_error");
    assert_eq!(
        response_json["error"]["message"],
        "没有可用提供商支持模型 gpt-5.4 的同步请求"
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-claude-cli-openai-local-miss-123")
        .await
        .expect("request candidate trace should read");
    assert!(stored_candidates.is_empty());
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
