use super::{
    any, build_router_with_state, build_state_with_execution_runtime_override, hash_api_key, json,
    sample_local_openai_auth_snapshot, sample_local_openai_candidate_row,
    sample_local_openai_endpoint, sample_local_openai_key, sample_local_openai_provider,
    start_server, Arc, Body, GatewayDataState, HeaderValue, InMemoryAuthApiKeySnapshotRepository,
    InMemoryMinimalCandidateSelectionReadRepository, InMemoryProviderCatalogReadRepository,
    InMemoryRequestCandidateRepository, InMemoryUsageReadRepository, Json, Request, Response,
    Router, StatusCode, UsageReadRepository, UsageRuntimeConfig, DEVELOPMENT_ENCRYPTION_KEY,
    TRACE_ID_HEADER,
};

fn large_request_body(stream: bool) -> String {
    let large_message = "x".repeat(128 * 1024);
    serde_json::to_string(&json!({
        "model": "gpt-5",
        "messages": [{"role": "user", "content": large_message}],
        "stream": stream
    }))
    .expect("request body should encode")
}

#[tokio::test]
async fn gateway_records_usage_for_execution_runtime_sync_when_runtime_enabled() {
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
                "request_id": "req-usage-sync-123",
                "status_code": 200,
                "headers": {
                    "content-type": "application/json"
                },
                "body": {
                    "json_body": {
                        "id": "chatcmpl-usage-sync-123",
                        "usage": {
                            "input_tokens": 3,
                            "output_tokens": 5,
                            "total_tokens": 8
                        }
                    }
                },
                "telemetry": {
                    "elapsed_ms": 45
                }
            }))
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-usage-sync")),
        sample_local_openai_auth_snapshot("api-key-usage-sync-123", "user-usage-sync-123"),
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
            "Bearer sk-client-openai-usage-sync",
        )
        .header(TRACE_ID_HEADER, "req-usage-sync-123")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);

    let mut stored = None;
    for _ in 0..50 {
        stored = usage_repository
            .find_by_request_id("req-usage-sync-123")
            .await
            .expect("usage lookup should succeed");
        if stored.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let stored = stored.expect("usage should be recorded");
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.billing_status, "pending");
    assert_eq!(stored.total_tokens, 8);
    assert_eq!(stored.response_time_ms, Some(45));
    assert_eq!(stored.user_id.as_deref(), Some("user-usage-sync-123"));

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_records_pending_usage_before_execution_runtime_sync_result_arrives() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let execution_request_started = Arc::new(tokio::sync::Notify::new());
    let allow_execution_response = Arc::new(tokio::sync::Notify::new());

    let upstream = Router::new().route(
        "/api/internal/gateway/report-sync",
        any(|_request: Request| async move { Json(json!({"ok": true})) }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any({
            let execution_request_started = Arc::clone(&execution_request_started);
            let allow_execution_response = Arc::clone(&allow_execution_response);
            move |_request: Request| {
                let execution_request_started = Arc::clone(&execution_request_started);
                let allow_execution_response = Arc::clone(&allow_execution_response);
                async move {
                    execution_request_started.notify_one();
                    allow_execution_response.notified().await;
                    Json(json!({
                        "request_id": "req-usage-sync-pending-123",
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "id": "chatcmpl-usage-sync-pending-123",
                                "usage": {
                                    "input_tokens": 3,
                                    "output_tokens": 5,
                                    "total_tokens": 8
                                }
                            }
                        },
                        "telemetry": {
                            "elapsed_ms": 45
                        }
                    }))
                }
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-usage-sync-pending")),
        sample_local_openai_auth_snapshot(
            "api-key-usage-sync-pending-123",
            "user-usage-sync-pending-123",
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

    let request_task = tokio::spawn({
        let gateway_url = gateway_url.clone();
        async move {
            let response = reqwest::Client::new()
                .post(format!("{gateway_url}/v1/chat/completions"))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(
                    http::header::AUTHORIZATION,
                    "Bearer sk-client-openai-usage-sync-pending",
                )
                .header(TRACE_ID_HEADER, "req-usage-sync-pending-123")
                .body("{\"model\":\"gpt-5\",\"messages\":[]}")
                .send()
                .await
                .expect("request should succeed");
            let status = response.status();
            let body = response.text().await.expect("body should read");
            (status, body)
        }
    });

    execution_request_started.notified().await;

    let mut pending = None;
    for _ in 0..50 {
        pending = usage_repository
            .find_by_request_id("req-usage-sync-pending-123")
            .await
            .expect("usage lookup should succeed");
        if pending
            .as_ref()
            .is_some_and(|stored| stored.status == "pending")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let pending = pending.expect("pending usage should be recorded before sync result resolves");
    assert_eq!(pending.status, "pending");
    assert_eq!(pending.billing_status, "pending");
    assert_eq!(pending.response_time_ms, None);

    allow_execution_response.notify_one();

    let (status, _body) = request_task.await.expect("request task should join");
    assert_eq!(status, StatusCode::OK);

    let mut stored = None;
    for _ in 0..50 {
        stored = usage_repository
            .find_by_request_id("req-usage-sync-pending-123")
            .await
            .expect("usage lookup should succeed");
        if stored.as_ref().is_some_and(|row| row.status == "completed") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let stored = stored.expect("usage should be finalized");
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.response_time_ms, Some(45));

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_keeps_pending_sync_usage_lightweight_for_large_request_body() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let execution_request_started = Arc::new(tokio::sync::Notify::new());
    let allow_execution_response = Arc::new(tokio::sync::Notify::new());

    let upstream = Router::new().route(
        "/api/internal/gateway/report-sync",
        any(|_request: Request| async move { Json(json!({"ok": true})) }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any({
            let execution_request_started = Arc::clone(&execution_request_started);
            let allow_execution_response = Arc::clone(&allow_execution_response);
            move |_request: Request| {
                let execution_request_started = Arc::clone(&execution_request_started);
                let allow_execution_response = Arc::clone(&allow_execution_response);
                async move {
                    execution_request_started.notify_one();
                    allow_execution_response.notified().await;
                    Json(json!({
                        "request_id": "req-usage-sync-large-pending-123",
                        "status_code": 200,
                        "headers": {
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "id": "chatcmpl-usage-sync-large-pending-123",
                                "usage": {
                                    "input_tokens": 3,
                                    "output_tokens": 5,
                                    "total_tokens": 8
                                }
                            }
                        },
                        "telemetry": {
                            "elapsed_ms": 45
                        }
                    }))
                }
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-usage-sync-large-pending")),
        sample_local_openai_auth_snapshot(
            "api-key-usage-sync-large-pending-123",
            "user-usage-sync-large-pending-123",
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
            ),
        )
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let request_task = tokio::spawn({
        let gateway_url = gateway_url.clone();
        async move {
            let response = reqwest::Client::new()
                .post(format!("{gateway_url}/v1/chat/completions"))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(
                    http::header::AUTHORIZATION,
                    "Bearer sk-client-openai-usage-sync-large-pending",
                )
                .header(TRACE_ID_HEADER, "req-usage-sync-large-pending-123")
                .body(large_request_body(false))
                .send()
                .await
                .expect("request should succeed");
            let status = response.status();
            let body = response.text().await.expect("body should read");
            (status, body)
        }
    });

    execution_request_started.notified().await;

    let mut pending = None;
    for _ in 0..50 {
        pending = usage_repository
            .find_by_request_id("req-usage-sync-large-pending-123")
            .await
            .expect("usage lookup should succeed");
        if pending
            .as_ref()
            .is_some_and(|stored| stored.status == "pending")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let pending = pending.expect("pending usage should be recorded before sync result resolves");
    assert_eq!(pending.status, "pending");
    assert!(pending.request_headers.is_none());
    assert!(pending.request_body.is_none());
    assert!(pending.provider_request_headers.is_none());
    assert!(pending.provider_request_body.is_none());
    assert!(pending.response_headers.is_none());
    assert!(pending.client_response_headers.is_none());

    allow_execution_response.notify_one();

    let (status, _body) = request_task.await.expect("request task should join");
    assert_eq!(status, StatusCode::OK);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

async fn gateway_records_usage_for_execution_runtime_stream_when_runtime_enabled() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/plan-stream",
            any(|_request: Request| async move {
                Json(json!({
                    "action": "execution_runtime_stream",
                    "plan_kind": "openai_chat_stream",
                    "plan": {
                        "request_id": "req-usage-stream-123",
                        "provider_name": "openai",
                        "provider_id": "provider-usage-stream-123",
                        "endpoint_id": "endpoint-usage-stream-123",
                        "key_id": "key-usage-stream-123",
                        "method": "POST",
                        "url": "https://api.openai.example/v1/chat/completions",
                        "headers": {
                            "authorization": "Bearer upstream-key",
                            "content-type": "application/json"
                        },
                        "body": {
                            "json_body": {
                                "model": "gpt-5",
                                "messages": [],
                                "stream": true
                            }
                        },
                        "stream": true,
                        "client_api_format": "openai:chat",
                        "provider_api_format": "openai:chat",
                        "model_name": "gpt-5"
                    },
                    "report_kind": "openai_chat_stream_success",
                    "report_context": {
                        "user_id": "user-usage-stream-123",
                        "api_key_id": "api-key-usage-stream-123",
                        "provider_name": "openai",
                        "provider_id": "provider-usage-stream-123",
                        "endpoint_id": "endpoint-usage-stream-123",
                        "key_id": "key-usage-stream-123",
                        "client_api_format": "openai:chat",
                        "provider_api_format": "openai:chat",
                        "model": "gpt-5",
                        "mapped_model": "gpt-5"
                    }
                }))
            }),
        )
        .route(
            "/api/internal/gateway/report-stream",
            any(|_request: Request| async move { Json(json!({"ok": true})) }),
        );

    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(|_request: Request| async move {
            let frames = concat!(
                "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-usage-stream-123\\\",\\\"usage\\\":{\\\"input_tokens\\\":2,\\\"output_tokens\\\":4,\\\"total_tokens\\\":6}}\\n\\n\"}}\n",
                "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n",
                "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":51,\"ttfb_ms\":19}}}\n",
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

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_usage_data_repository_for_tests(usage_repository.clone())
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.text().await.expect("stream body should read");

    let mut stored = None;
    for _ in 0..50 {
        stored = usage_repository
            .find_by_request_id("req-usage-stream-123")
            .await
            .expect("usage lookup should succeed");
        if stored.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let stored = stored.expect("usage should be recorded");
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.billing_status, "pending");
    assert_eq!(stored.total_tokens, 6);
    assert_eq!(stored.first_byte_time_ms, Some(19));
    assert_eq!(stored.is_stream, true);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_records_pending_usage_before_execution_runtime_stream_headers_arrive() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let execution_request_started = Arc::new(tokio::sync::Notify::new());
    let allow_execution_response = Arc::new(tokio::sync::Notify::new());

    let upstream = Router::new().route(
        "/api/internal/gateway/report-stream",
        any(|_request: Request| async move { Json(json!({"ok": true})) }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any({
            let execution_request_started = Arc::clone(&execution_request_started);
            let allow_execution_response = Arc::clone(&allow_execution_response);
            move |_request: Request| {
                let execution_request_started = Arc::clone(&execution_request_started);
                let allow_execution_response = Arc::clone(&allow_execution_response);
                async move {
                    execution_request_started.notify_one();
                    allow_execution_response.notified().await;
                    let frames = concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-usage-stream-pending-123\\\",\\\"usage\\\":{\\\"input_tokens\\\":2,\\\"output_tokens\\\":4,\\\"total_tokens\\\":6}}\\n\\n\"}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n",
                        "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":51,\"ttfb_ms\":19}}}\n",
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
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-stream-pending")),
        sample_local_openai_auth_snapshot(
            "api-key-usage-stream-pending-123",
            "user-usage-stream-pending-123",
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
    let gateway_state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_data_state_for_tests(
            GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                Arc::clone(&request_candidate_repository),
                usage_repository.clone(),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let request_task = tokio::spawn({
        let gateway_url = gateway_url.clone();
        async move {
            let response = reqwest::Client::new()
                .post(format!("{gateway_url}/v1/chat/completions"))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(
                    http::header::AUTHORIZATION,
                    "Bearer sk-client-openai-stream-pending",
                )
                .header(TRACE_ID_HEADER, "req-usage-stream-pending-123")
                .body("{\"model\":\"gpt-5\",\"messages\":[],\"stream\":true}")
                .send()
                .await
                .expect("request should succeed");
            let status = response.status();
            let body = response.text().await.expect("stream body should read");
            (status, body)
        }
    });

    execution_request_started.notified().await;

    let mut pending = None;
    for _ in 0..50 {
        pending = usage_repository
            .find_by_request_id("req-usage-stream-pending-123")
            .await
            .expect("usage lookup should succeed");
        if pending
            .as_ref()
            .is_some_and(|stored| stored.status == "pending")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let pending = pending.expect("pending usage should be recorded before stream headers arrive");
    assert_eq!(pending.status, "pending");
    assert_eq!(pending.billing_status, "pending");
    assert_eq!(pending.first_byte_time_ms, None);
    assert_eq!(pending.response_time_ms, None);

    allow_execution_response.notify_one();

    let (status, _body) = request_task.await.expect("request task should join");
    assert_eq!(status, StatusCode::OK);

    let mut stored = None;
    for _ in 0..50 {
        stored = usage_repository
            .find_by_request_id("req-usage-stream-pending-123")
            .await
            .expect("usage lookup should succeed");
        if stored.as_ref().is_some_and(|row| row.status == "completed") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let stored = stored.expect("usage should be finalized");
    assert_eq!(stored.status, "completed");
    assert_eq!(stored.first_byte_time_ms, Some(19));

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_keeps_pending_stream_usage_lightweight_for_large_request_body() {
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let execution_request_started = Arc::new(tokio::sync::Notify::new());
    let allow_execution_response = Arc::new(tokio::sync::Notify::new());

    let upstream = Router::new().route(
        "/api/internal/gateway/report-stream",
        any(|_request: Request| async move { Json(json!({"ok": true})) }),
    );

    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any({
            let execution_request_started = Arc::clone(&execution_request_started);
            let allow_execution_response = Arc::clone(&allow_execution_response);
            move |_request: Request| {
                let execution_request_started = Arc::clone(&execution_request_started);
                let allow_execution_response = Arc::clone(&allow_execution_response);
                async move {
                    execution_request_started.notify_one();
                    allow_execution_response.notified().await;
                    let frames = concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"chatcmpl-usage-stream-large-pending-123\\\",\\\"usage\\\":{\\\"input_tokens\\\":2,\\\"output_tokens\\\":4,\\\"total_tokens\\\":6}}\\n\\n\"}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n",
                        "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":51,\"ttfb_ms\":19}}}\n",
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
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-stream-large-pending")),
        sample_local_openai_auth_snapshot(
            "api-key-usage-stream-large-pending-123",
            "user-usage-stream-large-pending-123",
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
                usage_repository.clone(),
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        )
        .with_usage_runtime_for_tests(UsageRuntimeConfig {
            enabled: true,
            ..UsageRuntimeConfig::default()
        });
    let gateway = build_router_with_state(gateway_state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let request_task = tokio::spawn({
        let gateway_url = gateway_url.clone();
        async move {
            let response = reqwest::Client::new()
                .post(format!("{gateway_url}/v1/chat/completions"))
                .header(http::header::CONTENT_TYPE, "application/json")
                .header(
                    http::header::AUTHORIZATION,
                    "Bearer sk-client-openai-stream-large-pending",
                )
                .header(TRACE_ID_HEADER, "req-usage-stream-large-pending-123")
                .body(large_request_body(true))
                .send()
                .await
                .expect("request should succeed");
            let status = response.status();
            let body = response.text().await.expect("stream body should read");
            (status, body)
        }
    });

    execution_request_started.notified().await;

    let mut pending = None;
    for _ in 0..50 {
        pending = usage_repository
            .find_by_request_id("req-usage-stream-large-pending-123")
            .await
            .expect("usage lookup should succeed");
        if pending
            .as_ref()
            .is_some_and(|stored| stored.status == "pending")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let pending = pending.expect("pending usage should be recorded before stream headers arrive");
    assert_eq!(pending.status, "pending");
    assert!(pending.request_headers.is_none());
    assert!(pending.request_body.is_none());
    assert!(pending.provider_request_headers.is_none());
    assert!(pending.provider_request_body.is_none());
    assert!(pending.response_headers.is_none());
    assert!(pending.client_response_headers.is_none());

    allow_execution_response.notify_one();

    let (status, _body) = request_task.await.expect("request task should join");
    assert_eq!(status, StatusCode::OK);

    gateway_handle.abort();
    execution_runtime_handle.abort();
    upstream_handle.abort();
}
