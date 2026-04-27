use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override,
    encrypt_python_fernet_plaintext, hash_api_key, json, sample_local_openai_auth_snapshot,
    sample_local_openai_candidate_row, sample_local_openai_endpoint, sample_local_openai_key,
    sample_local_openai_provider, send_request, start_server, Arc, Body, GatewayDataState,
    HeaderValue, InMemoryAuthApiKeySnapshotRepository,
    InMemoryMinimalCandidateSelectionReadRepository, InMemoryProviderCatalogReadRepository,
    InMemoryRequestCandidateRepository, InMemoryUsageReadRepository, Json, Mutex, Request,
    RequestCandidateReadRepository, RequestCandidateStatus, Response, Router, StatusCode,
    StoredAuthApiKeySnapshot, StoredMinimalCandidateSelectionRow, StoredProviderCatalogEndpoint,
    StoredProviderCatalogKey, StoredProviderCatalogProvider, StoredProviderModelMapping,
    UsageReadRepository, UsageRuntimeConfig, DEVELOPMENT_ENCRYPTION_KEY, TRACE_ID_HEADER,
};
use crate::constants::LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER;
use aether_data_contracts::repository::usage::UsageBodyCaptureState;

fn deep_nested_metadata(levels: usize) -> serde_json::Value {
    let mut current = json!({"leaf": "value"});
    for depth in 0..levels {
        current = json!({
            "depth": depth,
            "child": current
        });
    }
    current
}

async fn wait_for_usage_status<T>(
    repository: &T,
    request_id: &str,
    expected_status: &str,
) -> aether_data_contracts::repository::usage::StoredRequestUsageAudit
where
    T: UsageReadRepository + ?Sized,
{
    let mut stored = None;
    // Usage terminal events are written on a shared background runtime; under full-suite parallel
    // load they can lag noticeably behind the request/response assertion path.
    let timeout = std::time::Duration::from_secs(60);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        stored = repository
            .find_by_request_id(request_id)
            .await
            .expect("usage lookup should succeed");
        if stored
            .as_ref()
            .is_some_and(|usage| usage.status == expected_status)
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let observed = stored
                .as_ref()
                .map(|usage| usage.status.as_str())
                .unwrap_or("<missing>");
            panic!(
                "usage should reach status {expected_status} within {:?}, last observed status: {observed}",
                timeout
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    stored.expect("usage should be present once the expected status is observed")
}

#[tokio::test]
async fn gateway_handles_local_openai_chat_sync_report_with_local_reporting_when_usage_runtime_enabled(
) {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
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
            any(move |_request: Request| {
                let report_hits_inner = Arc::clone(&report_hits_clone);
                async move {
                    *report_hits_inner.lock().expect("mutex should lock") += 1;
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
        "/v1/execute/sync",
        any(|_request: Request| async move {
            Json(json!({
                "request_id": "trace-openai-chat-local-report-sync-123",
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-local-report-sync-123",
                        "object": "chat.completion",
                        "model": "gpt-5-upstream",
                        "choices": [],
                        "usage": {
                            "prompt_tokens": 2,
                            "completion_tokens": 3,
                            "total_tokens": 5
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 25
                }
            }))
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-local-report-sync")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-usage-local-1",
            "user-openai-usage-local-1",
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

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url)
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
    .with_usage_runtime_for_tests(UsageRuntimeConfig {
        enabled: true,
        ..UsageRuntimeConfig::default()
    });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-report-sync",
        )
        .header(TRACE_ID_HEADER, "trace-openai-chat-local-report-sync-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(body_json["model"], "gpt-5-upstream");

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-openai-chat-local-report-sync-123",
        "completed",
    )
    .await;
    assert_eq!(stored_usage.status, "completed");
    assert_eq!(stored_usage.total_tokens, 5);
    assert_eq!(stored_usage.response_time_ms, Some(25));

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-chat-local-report-sync-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_truncates_deep_request_echo_for_local_openai_chat_sync_usage() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let upstream = Router::new().route(
        "/api/internal/gateway/report-sync",
        any(|_request: Request| async move { Json(json!({"ok": true})) }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(|_request: Request| async move {
            Json(json!({
                "request_id": "trace-openai-chat-local-report-sync-deep-123",
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-local-report-sync-deep-123",
                        "object": "chat.completion",
                        "model": "gpt-5-upstream",
                        "choices": [],
                        "usage": {
                            "prompt_tokens": 2,
                            "completion_tokens": 3,
                            "total_tokens": 5
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 25
                }
            }))
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-local-report-sync-deep")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-usage-local-deep-1",
            "user-openai-usage-local-deep-1",
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

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url)
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
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let request_body = serde_json::to_string(&json!({
        "model": "gpt-5",
        "messages": [{
            "role": "user",
            "content": "x".repeat(128 * 1024)
        }],
        "metadata": deep_nested_metadata(96)
    }))
    .expect("request should encode");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-report-sync-deep",
        )
        .header(
            TRACE_ID_HEADER,
            "trace-openai-chat-local-report-sync-deep-123",
        )
        .body(request_body)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let mut stored_usage = None;
    for _ in 0..50 {
        stored_usage = usage_repository
            .find_by_request_id("trace-openai-chat-local-report-sync-deep-123")
            .await
            .expect("usage lookup should succeed");
        if stored_usage
            .as_ref()
            .is_some_and(|usage| usage.status == "completed")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let stored_usage = stored_usage.expect("usage should be recorded");
    assert_eq!(stored_usage.status, "completed");
    assert_eq!(stored_usage.total_tokens, 5);
    assert_eq!(
        stored_usage
            .request_body
            .as_ref()
            .and_then(|value| value.get("messages"))
            .and_then(|value| value.as_array())
            .and_then(|messages| messages.first())
            .and_then(|value| value.get("content"))
            .and_then(|value| value.as_str())
            .map(str::len),
        Some(128 * 1024)
    );
    assert_eq!(
        stored_usage
            .request_body
            .as_ref()
            .and_then(|value| value.get("metadata"))
            .and_then(|value| value.get("child"))
            .and_then(|value| value.get("child"))
            .and_then(|value| value.get("child"))
            .and_then(|value| value.get("child"))
            .and_then(|value| value.get("child"))
            .and_then(|value| value.as_object())
            .map(|value| value.contains_key("depth")),
        Some(true)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_applies_system_max_request_body_size_to_local_openai_chat_sync_usage() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let upstream = Router::new().route(
        "/api/internal/gateway/report-sync",
        any(|_request: Request| async move { Json(json!({"ok": true})) }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(|_request: Request| async move {
            Json(json!({
                "request_id": "trace-openai-chat-local-report-sync-request-limit-123",
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-local-report-sync-request-limit-123",
                        "object": "chat.completion",
                        "model": "gpt-5-upstream",
                        "choices": [],
                        "usage": {
                            "prompt_tokens": 2,
                            "completion_tokens": 3,
                            "total_tokens": 5
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 25
                }
            }))
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(
            "sk-client-openai-local-report-sync-request-limit",
        )),
        sample_local_openai_auth_snapshot(
            "api-key-openai-usage-local-request-limit-1",
            "user-openai-usage-local-request-limit-1",
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

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::clone(&request_candidate_repository),
                Arc::clone(&usage_repository),
                DEVELOPMENT_ENCRYPTION_KEY,
            )
            .with_system_config_values_for_tests([(
                "max_request_body_size".to_string(),
                json!(128),
            )]),
        )
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let request_body = serde_json::to_string(&json!({
        "model": "gpt-5",
        "messages": [{
            "role": "user",
            "content": "x".repeat(2048)
        }]
    }))
    .expect("request should encode");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-report-sync-request-limit",
        )
        .header(
            TRACE_ID_HEADER,
            "trace-openai-chat-local-report-sync-request-limit-123",
        )
        .body(request_body)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-openai-chat-local-report-sync-request-limit-123",
        "completed",
    )
    .await;
    assert_eq!(stored_usage.total_tokens, 5);
    assert_eq!(
        stored_usage.request_body_state,
        Some(UsageBodyCaptureState::Truncated)
    );
    assert_eq!(
        stored_usage.provider_request_body_state,
        Some(UsageBodyCaptureState::Truncated)
    );
    assert_eq!(
        stored_usage
            .request_body
            .as_ref()
            .and_then(|value| value.get("truncated"))
            .and_then(|value| value.as_bool()),
        Some(true)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_strips_request_and_response_bodies_when_request_record_level_is_base() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let upstream = Router::new().route(
        "/api/internal/gateway/report-sync",
        any(|_request: Request| async move { Json(json!({"ok": true})) }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(|_request: Request| async move {
            Json(json!({
                "request_id": "trace-openai-chat-local-report-sync-base-123",
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-local-report-sync-base-123",
                        "object": "chat.completion",
                        "model": "gpt-5-upstream",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "body should not be persisted"
                            }
                        }],
                        "usage": {
                            "prompt_tokens": 2,
                            "completion_tokens": 3,
                            "total_tokens": 5
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 25
                }
            }))
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-local-report-sync-base")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-usage-local-base-1",
            "user-openai-usage-local-base-1",
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

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::clone(&request_candidate_repository),
                Arc::clone(&usage_repository),
                DEVELOPMENT_ENCRYPTION_KEY,
            )
            .with_system_config_values_for_tests([(
                "request_record_level".to_string(),
                json!("base"),
            )]),
        )
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-report-sync-base",
        )
        .header(
            TRACE_ID_HEADER,
            "trace-openai-chat-local-report-sync-base-123",
        )
        .body(
            serde_json::to_string(&json!({
                "model": "gpt-5",
                "messages": [{
                    "role": "user",
                    "content": "request body should not be persisted"
                }]
            }))
            .expect("request should encode"),
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(body_json["model"], "gpt-5-upstream");

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-openai-chat-local-report-sync-base-123",
        "completed",
    )
    .await;
    assert_eq!(stored_usage.status, "completed");
    assert_eq!(stored_usage.total_tokens, 5);
    assert_eq!(stored_usage.response_time_ms, Some(25));
    assert!(stored_usage.request_body.is_none());
    assert!(stored_usage.request_body_ref.is_none());
    assert!(stored_usage.provider_request_body.is_none());
    assert!(stored_usage.provider_request_body_ref.is_none());
    assert!(stored_usage.response_body.is_none());
    assert!(stored_usage.response_body_ref.is_none());
    assert!(stored_usage.client_response_body.is_none());
    assert!(stored_usage.client_response_body_ref.is_none());

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-chat-local-report-sync-base-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_records_failed_usage_when_all_local_openai_chat_candidates_exhaust_after_retryable_sync_failure(
) {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-local-report-sync-failure")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-usage-local-failure-1",
            "user-openai-usage-local-failure-1",
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

    let gateway_state = crate::AppState::new()
        .expect("gateway should build")
        .with_execution_runtime_sync_override_for_tests(|plan| {
            Ok(aether_contracts::ExecutionResult {
                request_id: plan.request_id.clone(),
                candidate_id: plan.candidate_id.clone(),
                status_code: 503,
                headers: std::collections::BTreeMap::from([(
                    "content-type".to_string(),
                    "application/json".to_string(),
                )]),
                body: Some(aether_contracts::ResponseBody {
                    json_body: Some(json!({
                        "error": {
                            "message": "primary unavailable"
                        }
                    })),
                    body_bytes_b64: None,
                }),
                telemetry: Some(aether_contracts::ExecutionTelemetry {
                    ttfb_ms: None,
                    elapsed_ms: Some(25),
                    upstream_bytes: None,
                }),
                error: None,
            })
        })
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
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let request = Request::builder()
        .method(http::Method::POST)
        .uri("/v1/chat/completions")
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-report-sync-failure",
        )
        .header(
            TRACE_ID_HEADER,
            "trace-openai-chat-local-report-sync-failure-123",
        )
        .body(Body::from("{\"model\":\"gpt-5\",\"messages\":[]}"))
        .expect("request should build");
    let response = send_request(gateway, request).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_json: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read"),
    )
    .expect("body should parse");
    assert_eq!(body_json["error"]["type"], "http_error");

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-openai-chat-local-report-sync-failure-123",
        "failed",
    )
    .await;
    assert_eq!(stored_usage.status, "failed");
    assert_eq!(stored_usage.billing_status, "void");
    assert_eq!(stored_usage.status_code, Some(503));
    assert_eq!(stored_usage.error_category.as_deref(), Some("server_error"));
    assert_eq!(
        stored_usage.user_id.as_deref(),
        Some("user-openai-usage-local-failure-1")
    );
    assert_eq!(stored_usage.provider_name, "openai");
    assert_eq!(stored_usage.model, "gpt-5");
    assert_eq!(stored_usage.api_format.as_deref(), Some("openai:chat"));
    assert_eq!(
        stored_usage
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("trace_id"))
            .and_then(|value| value.as_str()),
        Some("trace-openai-chat-local-report-sync-failure-123")
    );
    assert_eq!(
        stored_usage
            .response_body
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|value| value.get("type"))
            .and_then(|value| value.as_str()),
        Some("upstream_error")
    );
    assert_eq!(
        stored_usage
            .client_response_body
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|value| value.get("type"))
            .and_then(|value| value.as_str()),
        Some("http_error")
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-chat-local-report-sync-failure-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Failed);
    assert_eq!(stored_candidates[0].status_code, Some(503));
}

#[tokio::test]
async fn gateway_records_failed_usage_for_claude_runtime_miss_without_execution_exhaustion() {
    fn sample_claude_auth_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
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

    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
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
            any(move |_request: Request| {
                let report_hits_inner = Arc::clone(&report_hits_clone);
                async move {
                    *report_hits_inner.lock().expect("mutex should lock") += 1;
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

    let execution_runtime = Router::new();
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-claude-runtime-miss-usage")),
        sample_claude_auth_snapshot(
            "api-key-claude-runtime-miss-usage-1",
            "user-claude-runtime-miss-usage-1",
        ),
    )]));
    let candidate_selection_repository = Arc::new(
        InMemoryMinimalCandidateSelectionReadRepository::seed(vec![]),
    );
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![],
        vec![],
        vec![],
    ));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url)
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
    .with_usage_runtime_for_tests(UsageRuntimeConfig {
        enabled: true,
        ..UsageRuntimeConfig::default()
    });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages?beta=true"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", "sk-client-claude-runtime-miss-usage")
        .header("anthropic-version", "2023-06-01")
        .header(TRACE_ID_HEADER, "trace-claude-runtime-miss-usage-123")
        .body("{\"model\":\"claude-sonnet-4-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should complete");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("candidate_list_empty")
    );
    let body_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(body_json["error"]["type"], "http_error");
    assert_eq!(
        body_json["error"]["message"],
        "没有可用提供商支持模型 claude-sonnet-4-5 的同步请求"
    );

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-claude-runtime-miss-usage-123",
        "failed",
    )
    .await;
    assert_eq!(stored_usage.status, "failed");
    assert_eq!(stored_usage.billing_status, "void");
    assert_eq!(stored_usage.status_code, Some(503));
    assert_eq!(stored_usage.error_category.as_deref(), Some("server_error"));
    assert_eq!(
        stored_usage.user_id.as_deref(),
        Some("user-claude-runtime-miss-usage-1")
    );
    assert_eq!(stored_usage.provider_name, "unknown");
    assert_eq!(stored_usage.model, "claude-sonnet-4-5");
    assert_eq!(stored_usage.api_format.as_deref(), Some("claude:chat"));
    assert_eq!(
        stored_usage.routing_execution_path(),
        Some("local_execution_runtime_miss")
    );
    assert_eq!(
        stored_usage.routing_local_execution_runtime_miss_reason(),
        Some("candidate_list_empty")
    );
    assert_eq!(stored_usage.routing_route_family(), Some("claude"));
    assert_eq!(stored_usage.routing_route_kind(), Some("chat"));
    assert_eq!(
        stored_usage
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("trace_id"))
            .and_then(|value| value.as_str()),
        Some("trace-claude-runtime-miss-usage-123")
    );
    assert_eq!(
        stored_usage
            .client_response_body
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|value| value.get("type"))
            .and_then(|value| value.as_str()),
        Some("http_error")
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-claude-runtime-miss-usage-123")
        .await
        .expect("request candidate trace should read");
    assert!(stored_candidates.is_empty());

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
}

#[tokio::test]
async fn gateway_handles_local_openai_chat_stream_report_with_local_reporting_when_usage_runtime_enabled(
) {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let report_hits = Arc::new(Mutex::new(0usize));
    let report_hits_clone = Arc::clone(&report_hits);
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
            any(move |_request: Request| {
                let report_hits_inner = Arc::clone(&report_hits_clone);
                async move {
                    *report_hits_inner.lock().expect("mutex should lock") += 1;
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
            let frames = concat!(
                "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-local-report-stream-123\\\",\\\"usage\\\":{\\\"input_tokens\\\":2,\\\"output_tokens\\\":4,\\\"total_tokens\\\":6}}\\n\\n\"}}\n",
                "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n",
                "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":31,\"ttfb_ms\":11}}}\n",
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
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-local-report-stream")),
        sample_local_openai_auth_snapshot(
            "api-key-openai-usage-local-1",
            "user-openai-usage-local-1",
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

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url)
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
    .with_usage_runtime_for_tests(UsageRuntimeConfig {
        enabled: true,
        ..UsageRuntimeConfig::default()
    });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-report-stream",
        )
        .header(TRACE_ID_HEADER, "trace-openai-chat-local-report-stream-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response.text().await.expect("stream body should read");
    assert_eq!(
        body_text,
        "data: {\"id\":\"chatcmpl-local-report-stream-123\",\"usage\":{\"input_tokens\":2,\"output_tokens\":4,\"total_tokens\":6}}\n\ndata: [DONE]\n\n"
    );

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-openai-chat-local-report-stream-123",
        "completed",
    )
    .await;
    assert_eq!(stored_usage.status, "completed");
    assert_eq!(stored_usage.total_tokens, 6);
    assert_eq!(stored_usage.first_byte_time_ms, Some(11));
    assert!(stored_usage.is_stream);

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-openai-chat-local-report-stream-123")
        .await
        .expect("request candidate trace should read");
    assert_eq!(stored_candidates.len(), 1);
    assert_eq!(stored_candidates[0].status, RequestCandidateStatus::Success);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(*report_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*decision_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*plan_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_preserves_stream_usage_when_max_response_body_size_truncates_capture() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let upstream = Router::new().route(
        "/api/internal/gateway/report-stream",
        any(|_request: Request| async move { Json(json!({"ok": true})) }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(|_request: Request| async move {
            let delta_chunk = format!(
                "data: {{\"id\":\"chatcmpl-local-report-stream-truncated-123\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
                "x".repeat(2048)
            );
            let summary = json!({
                "standardized_usage": {
                    "input_tokens": 2,
                    "output_tokens": 4,
                    "cache_creation_tokens": 0,
                    "cache_creation_ephemeral_5m_tokens": 0,
                    "cache_creation_ephemeral_1h_tokens": 0,
                    "cache_read_tokens": 0,
                    "reasoning_tokens": 0,
                    "cache_storage_token_hours": 0.0,
                    "request_count": 1,
                    "dimensions": {}
                },
                "finish_reason": "stop",
                "response_id": "chatcmpl-local-report-stream-truncated-123",
                "model": "gpt-5-upstream",
                "observed_finish": true
            });
            let frames = [
                json!({
                    "type": "headers",
                    "payload": {
                        "kind": "headers",
                        "status_code": 200,
                        "headers": {"content-type": "text/event-stream"}
                    }
                }),
                json!({
                    "type": "data",
                    "payload": {"kind": "data", "text": delta_chunk}
                }),
                json!({
                    "type": "data",
                    "payload": {"kind": "data", "text": "data: [DONE]\\n\\n"}
                }),
                json!({
                    "type": "telemetry",
                    "payload": {
                        "kind": "telemetry",
                        "telemetry": {"elapsed_ms": 31, "ttfb_ms": 11}
                    }
                }),
                json!({
                    "type": "eof",
                    "payload": {"kind": "eof", "summary": summary}
                }),
            ]
            .into_iter()
            .map(|frame| serde_json::to_string(&frame).expect("frame should encode"))
            .collect::<Vec<_>>()
            .join("\n")
                + "\n";
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .body(Body::from(frames))
                .expect("response should build");
            response.headers_mut().insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/x-ndjson"),
            );
            response
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key(
            "sk-client-openai-local-report-stream-truncated",
        )),
        sample_local_openai_auth_snapshot(
            "api-key-openai-usage-local-stream-truncated-1",
            "user-openai-usage-local-stream-truncated-1",
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

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::clone(&request_candidate_repository),
                Arc::clone(&usage_repository),
                DEVELOPMENT_ENCRYPTION_KEY,
            )
            .with_system_config_values_for_tests([(
                "max_response_body_size".to_string(),
                json!(128),
            )]),
        )
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-local-report-stream-truncated",
        )
        .header(
            TRACE_ID_HEADER,
            "trace-openai-chat-local-report-stream-truncated-123",
        )
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let body_text = response.text().await.expect("stream body should read");
    assert!(body_text.contains("chatcmpl-local-report-stream-truncated-123"));

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-openai-chat-local-report-stream-truncated-123",
        "completed",
    )
    .await;
    assert_eq!(stored_usage.total_tokens, 6);
    assert_eq!(
        stored_usage.response_body_state,
        Some(UsageBodyCaptureState::Truncated)
    );
    assert_eq!(
        stored_usage.client_response_body_state,
        Some(UsageBodyCaptureState::Truncated)
    );
    assert_eq!(
        stored_usage
            .response_body
            .as_ref()
            .and_then(|value| value.get("truncated"))
            .and_then(|value| value.as_bool()),
        Some(true)
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_records_failed_usage_when_all_local_claude_cli_candidates_are_skipped() {
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
            provider_id: "provider-claude-cli-usage-local-miss-1".to_string(),
            provider_name: "RightCode".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-claude-cli-usage-local-miss-1".to_string(),
            endpoint_api_format: "openai:responses".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-claude-cli-usage-local-miss-1".to_string(),
            key_name: "codex".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:responses".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:responses": 1})),
            model_id: "model-claude-cli-usage-local-miss-1".to_string(),
            global_model_id: "global-model-claude-cli-usage-local-miss-1".to_string(),
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
            "provider-claude-cli-usage-local-miss-1".to_string(),
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
            "endpoint-claude-cli-usage-local-miss-1".to_string(),
            "provider-claude-cli-usage-local-miss-1".to_string(),
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
            "key-claude-cli-usage-local-miss-1".to_string(),
            "provider-claude-cli-usage-local-miss-1".to_string(),
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
                "sk-upstream-openai-cli-usage-local-miss",
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

    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
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
        Some(hash_api_key("sk-client-claude-cli-usage-local-miss")),
        sample_auth_snapshot(
            "api-key-claude-cli-usage-local-miss-1",
            "user-claude-cli-usage-local-miss-1",
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

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url)
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
    .with_usage_runtime_for_tests(UsageRuntimeConfig {
        enabled: true,
        ..UsageRuntimeConfig::default()
    });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages?beta=true"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-claude-cli-usage-local-miss",
        )
        .header(TRACE_ID_HEADER, "trace-claude-cli-usage-local-miss-123")
        .body("{\"model\":\"gpt-5.4\",\"messages\":[]}")
        .send()
        .await
        .expect("request should complete");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("candidate_list_empty")
    );
    let body_json: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(body_json["error"]["type"], "http_error");
    assert_eq!(
        body_json["error"]["message"],
        "没有可用提供商支持模型 gpt-5.4 的同步请求"
    );

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-claude-cli-usage-local-miss-123",
        "failed",
    )
    .await;
    assert_eq!(stored_usage.status, "failed");
    assert_eq!(stored_usage.billing_status, "void");
    assert_eq!(stored_usage.status_code, Some(503));
    assert_eq!(stored_usage.error_category.as_deref(), Some("server_error"));
    assert_eq!(
        stored_usage.user_id.as_deref(),
        Some("user-claude-cli-usage-local-miss-1")
    );
    assert_eq!(stored_usage.provider_name, "unknown");
    assert_eq!(stored_usage.model, "gpt-5.4");
    assert_eq!(stored_usage.api_format.as_deref(), Some("claude:cli"));
    assert_eq!(
        stored_usage.endpoint_api_format.as_deref(),
        Some("claude:cli")
    );
    assert_eq!(stored_usage.routing_key_name(), None);
    assert_eq!(stored_usage.routing_planner_kind(), Some("claude_cli_sync"));
    assert_eq!(stored_usage.routing_route_family(), Some("claude"));
    assert_eq!(stored_usage.routing_route_kind(), Some("cli"));
    assert_eq!(
        stored_usage.routing_execution_path(),
        Some("local_execution_runtime_miss")
    );
    assert_eq!(
        stored_usage.routing_local_execution_runtime_miss_reason(),
        Some("candidate_list_empty")
    );
    assert_eq!(
        stored_usage.error_message.as_deref(),
        Some(
            "没有可用提供商支持模型 gpt-5.4 的同步请求。请检查模型映射、端点启用状态和 API Key 权限（原因代码: candidate_list_empty）"
        )
    );
    assert_eq!(
        stored_usage
            .request_headers
            .as_ref()
            .and_then(|value| value.get("authorization"))
            .and_then(|value| value.as_str()),
        Some("Bear****miss")
    );
    assert_eq!(
        stored_usage
            .request_headers
            .as_ref()
            .and_then(|value| value.get("content-type"))
            .and_then(|value| value.as_str()),
        Some("application/json")
    );
    assert_eq!(
        stored_usage
            .request_body
            .as_ref()
            .and_then(|value| value.get("model"))
            .and_then(|value| value.as_str()),
        Some("gpt-5.4")
    );
    assert!(stored_usage.provider_request_body.is_none());
    assert_eq!(
        stored_usage
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("trace_id"))
            .and_then(|value| value.as_str()),
        Some("trace-claude-cli-usage-local-miss-123")
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-claude-cli-usage-local-miss-123")
        .await
        .expect("request candidate trace should read");
    assert!(stored_candidates.is_empty());
    assert_eq!(stored_usage.routing_candidate_id(), None);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_keeps_failed_usage_request_capture_lightweight_for_large_local_claude_cli_runtime_miss(
) {
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
            provider_id: "provider-claude-cli-usage-local-miss-large-1".to_string(),
            provider_name: "RightCode".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-claude-cli-usage-local-miss-large-1".to_string(),
            endpoint_api_format: "openai:responses".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("cli".to_string()),
            endpoint_is_active: true,
            key_id: "key-claude-cli-usage-local-miss-large-1".to_string(),
            key_name: "codex".to_string(),
            key_auth_type: "bearer".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:responses".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 5,
            key_global_priority_by_format: Some(serde_json::json!({"openai:responses": 1})),
            model_id: "model-claude-cli-usage-local-miss-large-1".to_string(),
            global_model_id: "global-model-claude-cli-usage-local-miss-large-1".to_string(),
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
            "provider-claude-cli-usage-local-miss-large-1".to_string(),
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
            "endpoint-claude-cli-usage-local-miss-large-1".to_string(),
            "provider-claude-cli-usage-local-miss-large-1".to_string(),
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
            "key-claude-cli-usage-local-miss-large-1".to_string(),
            "provider-claude-cli-usage-local-miss-large-1".to_string(),
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
                "sk-upstream-openai-cli-usage-local-miss-large",
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

    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let execution_runtime = Router::new();
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-claude-cli-usage-local-miss-large")),
        sample_auth_snapshot(
            "api-key-claude-cli-usage-local-miss-large-1",
            "user-claude-cli-usage-local-miss-large-1",
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

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state =
        build_state_with_execution_runtime_override(execution_runtime_url)
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
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let request_body = serde_json::to_string(&json!({
        "model": "gpt-5.4",
        "messages": [{
            "role": "user",
            "content": "x".repeat(128 * 1024)
        }],
        "metadata": deep_nested_metadata(96)
    }))
    .expect("request should encode");

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages?beta=true"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-claude-cli-usage-local-miss-large",
        )
        .header(
            TRACE_ID_HEADER,
            "trace-claude-cli-usage-local-miss-large-123",
        )
        .body(request_body)
        .send()
        .await
        .expect("request should complete");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(LOCAL_EXECUTION_RUNTIME_MISS_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("candidate_list_empty")
    );

    let stored_usage = wait_for_usage_status(
        usage_repository.as_ref(),
        "trace-claude-cli-usage-local-miss-large-123",
        "failed",
    )
    .await;
    assert_eq!(stored_usage.status, "failed");
    assert_eq!(
        stored_usage.request_body_state,
        Some(UsageBodyCaptureState::Inline)
    );
    assert_eq!(
        stored_usage
            .request_body
            .as_ref()
            .and_then(|value| value.get("model"))
            .and_then(|value| value.as_str()),
        Some("gpt-5.4")
    );
    assert!(stored_usage.provider_request_body.is_none());
    assert_eq!(
        stored_usage
            .request_metadata
            .as_ref()
            .and_then(|value| value.get("trace_id"))
            .and_then(|value| value.as_str()),
        Some("trace-claude-cli-usage-local-miss-large-123")
    );

    let stored_candidates = request_candidate_repository
        .list_by_request_id("trace-claude-cli-usage-local-miss-large-123")
        .await
        .expect("request candidate trace should read");
    assert!(stored_candidates.is_empty());

    gateway_handle.abort();
    execution_runtime_handle.abort();
}
