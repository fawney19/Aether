use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, json, start_server,
    to_bytes, Arc, Body, Json, Mutex, Request, Router, StatusCode,
    EXECUTION_PATH_EXECUTION_RUNTIME_SYNC, EXECUTION_PATH_HEADER, TRACE_ID_HEADER,
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

const SEARCH_SYNC_TEST_STACK_BYTES: usize = 16 * 1024 * 1024;

fn run_search_sync_test<F, Fut>(test_name: &'static str, make_future: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let handle = std::thread::Builder::new()
        .name(test_name.to_string())
        .stack_size(SEARCH_SYNC_TEST_STACK_BYTES)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime should build");
            runtime.block_on(make_future());
        })
        .expect("search sync test thread should spawn");

    if let Err(payload) = handle.join() {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn gateway_executes_codex_search_with_responses_permission_and_search_contract() {
    run_search_sync_test(
        "gateway_executes_codex_search_with_responses_permission_and_search_contract",
        gateway_executes_codex_search_with_responses_permission_and_search_contract_impl,
    );
}

async fn gateway_executes_codex_search_with_responses_permission_and_search_contract_impl() {
    fn hash_api_key(value: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    fn auth_snapshot() -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            "user-search-1".to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(json!(["openai", "codex"])),
            Some(json!(["openai:responses"])),
            None,
            "api-key-search-1".to_string(),
            Some("search-client".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(4_102_444_800_i64),
            Some(json!(["openai", "codex"])),
            Some(json!(["openai:responses"])),
            None,
        )
        .expect("auth snapshot should build")
    }

    fn candidate_row() -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-codex-search-1".to_string(),
            provider_name: "codex".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-codex-search-1".to_string(),
            endpoint_api_format: "openai:search".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("search".to_string()),
            endpoint_is_active: true,
            key_id: "key-codex-search-1".to_string(),
            key_name: "oauth".to_string(),
            key_auth_type: "oauth".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:responses".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(json!({"openai:search": 1})),
            model_id: "model-codex-search-1".to_string(),
            global_model_id: "global-model-codex-search-1".to_string(),
            global_model_name: "gpt-5.6-sol".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(false),
            model_provider_model_name: "gpt-5.6-sol".to_string(),
            model_provider_model_mappings: Some(vec![StoredProviderModelMapping {
                name: "gpt-5.6-sol".to_string(),
                priority: 1,
                api_formats: Some(vec!["openai:responses".to_string()]),
                endpoint_ids: None,
                operations: None,
            }]),
            model_supports_streaming: Some(false),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn provider() -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-codex-search-1".to_string(),
            "codex".to_string(),
            Some("https://chatgpt.com".to_string()),
            "codex".to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(2),
            None,
            Some(900.0),
            None,
            None,
        )
    }

    fn endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-codex-search-1".to_string(),
            "provider-codex-search-1".to_string(),
            "openai:search".to_string(),
            Some("openai".to_string()),
            Some("search".to_string()),
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

    fn key() -> StoredProviderCatalogKey {
        let auth_config = encrypt_python_fernet_plaintext(
            DEVELOPMENT_ENCRYPTION_KEY,
            r#"{"provider_type":"codex","account_id":"account-search-1","is_fedramp":true}"#,
        )
        .expect("auth config should encrypt");
        StoredProviderCatalogKey::new(
            "key-codex-search-1".to_string(),
            "provider-codex-search-1".to_string(),
            "oauth".to_string(),
            "oauth".to_string(),
            None,
            true,
        )
        .expect("key should build")
        .with_transport_fields(
            Some(json!(["openai:responses"])),
            encrypt_python_fernet_plaintext(
                DEVELOPMENT_ENCRYPTION_KEY,
                "codex-search-access-token",
            )
            .expect("access token should encrypt"),
            Some(auth_config),
            None,
            Some(json!({"openai:search": 1})),
            None,
            Some(4_102_444_800),
            None,
            None,
        )
        .expect("key transport should build")
    }

    let seen_plans = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let seen_plans_clone = Arc::clone(&seen_plans);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |request: Request| {
            let seen_plans_inner = Arc::clone(&seen_plans_clone);
            async move {
                let (_, body) = request.into_parts();
                let bytes = to_bytes(body, usize::MAX).await.expect("body should read");
                let payload: serde_json::Value =
                    serde_json::from_slice(&bytes).expect("execution payload should parse");
                let request_id = payload["request_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let provider_id = payload["provider_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                seen_plans_inner
                    .lock()
                    .expect("mutex should lock")
                    .push(payload);
                let execution_result = if request_id == "trace-search-error-1" {
                    json!({
                        "request_id": request_id,
                        "status_code": 400,
                        "headers": {
                            "content-type": "application/json",
                            "x-search-upstream": "rate-limited"
                        },
                        "body": {
                            "json_body": {
                                "error": {
                                    "type": "rate_limit_error",
                                    "message": "Search capacity reached",
                                    "param": null,
                                    "code": "rate_limit_exceeded"
                                },
                                "future_error_field": {"retryable": true}
                            }
                        },
                        "telemetry": {"elapsed_ms": 17}
                    })
                } else if request_id == "trace-search-failover-1"
                    && provider_id == "provider-codex-search-1"
                {
                    json!({
                        "request_id": request_id,
                        "status_code": 500,
                        "headers": {
                            "content-type": "application/json",
                            "x-search-upstream": "primary"
                        },
                        "body": {
                            "json_body": {
                                "error": {
                                    "type": "server_error",
                                    "message": "Search backend unavailable"
                                }
                            }
                        },
                        "telemetry": {"elapsed_ms": 11}
                    })
                } else if request_id == "trace-search-failover-1" {
                    json!({
                        "request_id": request_id,
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json",
                            "x-search-upstream": "backup"
                        },
                        "body": {
                            "json_body": {
                                "output": "search fallback result"
                            }
                        },
                        "telemetry": {"elapsed_ms": 23}
                    })
                } else {
                    json!({
                        "request_id": request_id,
                        "status_code": 201,
                        "headers": {
                            "content-type": "application/json",
                            "x-search-upstream": "alpha"
                        },
                        "body": {
                            "json_body": {
                                "output": "search result",
                                "encrypted_output": "encrypted-search-result",
                                "future_response_field": {"enabled": true}
                            }
                        },
                        "telemetry": {"elapsed_ms": 42}
                    })
                };
                (
                    StatusCode::OK,
                    [("x-search-source", "codex-alpha")],
                    Json(execution_result),
                )
            }
        }),
    );

    let client_api_key = "sk-client-search";
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(client_api_key)),
        auth_snapshot(),
    )]));
    let candidate_repository = Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed({
        let primary = candidate_row();
        let mut backup = primary.clone();
        backup.provider_id = "provider-codex-search-2".to_string();
        backup.provider_name = "codex-backup".to_string();
        backup.provider_priority = 20;
        backup.endpoint_id = "endpoint-codex-search-2".to_string();
        backup.key_id = "key-codex-search-2".to_string();
        backup.key_name = "oauth-backup".to_string();
        backup.key_internal_priority = 6;
        backup.key_global_priority_by_format = Some(json!({"openai:search": 2}));
        backup.model_id = "model-codex-search-2".to_string();
        vec![primary, backup]
    }));
    let catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        {
            let primary = provider();
            let mut backup = primary.clone();
            backup.id = "provider-codex-search-2".to_string();
            backup.name = "codex-backup".to_string();
            vec![primary, backup]
        },
        {
            let primary = endpoint();
            let mut backup = primary.clone();
            backup.id = "endpoint-codex-search-2".to_string();
            backup.provider_id = "provider-codex-search-2".to_string();
            vec![primary, backup]
        },
        {
            let primary = key();
            let mut backup = primary.clone();
            backup.id = "key-codex-search-2".to_string();
            backup.provider_id = "provider-codex-search-2".to_string();
            backup.name = "oauth-backup".to_string();
            backup.global_priority_by_format = Some(json!({"openai:search": 2}));
            vec![primary, backup]
        },
    ));
    let request_candidates = Arc::new(InMemoryRequestCandidateRepository::default());
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let data_state =
        crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
            auth_repository,
            candidate_repository,
            catalog_repository,
            Arc::clone(&request_candidates),
            DEVELOPMENT_ENCRYPTION_KEY,
        )
        .with_system_config_values_for_tests([(
            crate::system_features::ENABLE_MODEL_DIRECTIVES_CONFIG_KEY.to_string(),
            json!(true),
        )]);
    let state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(data_state);
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/alpha/search"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {client_api_key}"),
        )
        .header(TRACE_ID_HEADER, "trace-search-1")
        .json(&json!({
            "id": "session-search-1",
            "model": "gpt-5.6-sol-ultra-fast",
            "reasoning": {"effort": "low", "summary": "auto"},
            "input": "find current OpenAI documentation",
            "commands": {
                "search_query": [{"q": "OpenAI Codex search"}],
                "open": [{"ref_id": "turn0search0"}]
            },
            "settings": {
                "search_context_size": "high",
                "allowed_callers": ["direct"]
            },
            "max_output_tokens": 4096,
            "store": false,
            "stream": true,
            "future_request_field": {"enabled": true}
        }))
        .send()
        .await
        .expect("search request should succeed");

    if response.status() != StatusCode::CREATED {
        let status = response.status();
        let body = response.text().await.expect("error response should read");
        panic!("Search request returned {status}: {body}");
    }
    assert_eq!(
        response
            .headers()
            .get("x-search-upstream")
            .and_then(|value| value.to_str().ok()),
        Some("alpha")
    );
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_SYNC)
    );
    let response_json: serde_json::Value = response.json().await.expect("response should parse");
    assert_eq!(response_json["output"], "search result");
    assert_eq!(response_json["encrypted_output"], "encrypted-search-result");
    assert_eq!(response_json["future_response_field"]["enabled"], true);

    let plan = seen_plans
        .lock()
        .expect("mutex should lock")
        .first()
        .cloned()
        .expect("execution plan should be captured");
    assert_eq!(
        plan["url"],
        "https://chatgpt.com/backend-api/codex/alpha/search"
    );
    assert_eq!(plan["client_api_format"], "openai:search");
    assert_eq!(plan["provider_api_format"], "openai:search");
    assert_eq!(plan["stream"], false);
    assert_eq!(plan["timeouts"]["total_ms"], 900_000);
    assert_eq!(
        plan["headers"]["authorization"],
        "Bearer codex-search-access-token"
    );
    assert_eq!(plan["headers"]["chatgpt-account-id"], "account-search-1");
    assert_eq!(plan["headers"]["x-openai-fedramp"], "true");
    assert_eq!(plan["headers"]["originator"], "codex_cli_rs");
    assert!(plan["headers"]["user-agent"]
        .as_str()
        .is_some_and(|value| value.starts_with("codex_cli_rs/")));
    assert!(plan["headers"].get("openai-beta").is_none());
    assert!(plan["headers"]
        .get("x-openai-internal-codex-responses-lite")
        .is_none());
    assert_ne!(plan["headers"]["accept"], "text/event-stream");

    let body = &plan["body"]["json_body"];
    assert_eq!(body["id"], "session-search-1");
    assert_eq!(body["model"], "gpt-5.6-sol");
    assert_eq!(body["reasoning"]["effort"], "max");
    assert_eq!(body["reasoning"]["summary"], "auto");
    assert_eq!(
        body["commands"]["search_query"][0]["q"],
        "OpenAI Codex search"
    );
    assert_eq!(body["commands"]["open"][0]["ref_id"], "turn0search0");
    assert_eq!(body["settings"]["search_context_size"], "high");
    assert_eq!(body["max_output_tokens"], 4096);
    assert!(body.get("store").is_none());
    assert!(body.get("future_request_field").is_none());
    assert!(body.get("stream").is_none());
    assert!(body.get("service_tier").is_none());

    let candidates = request_candidates
        .list_by_request_id("trace-search-1")
        .await
        .expect("request candidates should read");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].status, RequestCandidateStatus::Success);

    let expected_error_body = json!({
        "error": {
            "type": "rate_limit_error",
            "message": "Search capacity reached",
            "param": null,
            "code": "rate_limit_exceeded"
        },
        "future_error_field": {"retryable": true}
    });
    let error_response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/alpha/search"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {client_api_key}"),
        )
        .header(TRACE_ID_HEADER, "trace-search-error-1")
        .json(&json!({
            "id": "session-search-error-1",
            "model": "gpt-5.6-sol",
            "input": "find current OpenAI documentation",
            "commands": {"search_query": [{"q": "OpenAI documentation"}]}
        }))
        .send()
        .await
        .expect("search error response should return");

    assert_eq!(error_response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        error_response
            .headers()
            .get("x-search-upstream")
            .and_then(|value| value.to_str().ok()),
        Some("rate-limited")
    );
    assert_eq!(
        error_response
            .json::<serde_json::Value>()
            .await
            .expect("error response should parse"),
        expected_error_body
    );
    let error_candidates = request_candidates
        .list_by_request_id("trace-search-error-1")
        .await
        .expect("error request candidates should read");
    assert_eq!(error_candidates.len(), 1);
    assert_eq!(error_candidates[0].status, RequestCandidateStatus::Failed);

    let failover_response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/alpha/search"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            format!("Bearer {client_api_key}"),
        )
        .header(TRACE_ID_HEADER, "trace-search-failover-1")
        .json(&json!({
            "id": "session-search-failover-1",
            "model": "gpt-5.6-sol",
            "input": "find current OpenAI documentation",
            "commands": {"search_query": [{"q": "OpenAI documentation"}]}
        }))
        .send()
        .await
        .expect("search failover response should return");

    assert_eq!(failover_response.status(), StatusCode::OK);
    assert_eq!(
        failover_response
            .headers()
            .get("x-search-upstream")
            .and_then(|value| value.to_str().ok()),
        Some("backup")
    );
    assert_eq!(
        failover_response
            .json::<serde_json::Value>()
            .await
            .expect("failover response should parse")["output"],
        "search fallback result"
    );
    let failover_plans = seen_plans
        .lock()
        .expect("mutex should lock")
        .iter()
        .filter(|plan| plan["request_id"] == "trace-search-failover-1")
        .map(|plan| plan["provider_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        failover_plans,
        vec![
            json!("provider-codex-search-1"),
            json!("provider-codex-search-2")
        ]
    );
    let failover_candidates = request_candidates
        .list_by_request_id("trace-search-failover-1")
        .await
        .expect("failover request candidates should read");
    assert_eq!(failover_candidates.len(), 2);
    assert_eq!(
        failover_candidates[0].status,
        RequestCandidateStatus::Failed
    );
    assert_eq!(failover_candidates[0].status_code, Some(500));
    assert_eq!(
        failover_candidates[1].status,
        RequestCandidateStatus::Success
    );
    assert_eq!(failover_candidates[1].status_code, Some(200));

    gateway_handle.abort();
    execution_runtime_handle.abort();
}
