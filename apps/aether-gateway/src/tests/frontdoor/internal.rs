use super::{
    hash_api_key, sample_endpoint, sample_key, sample_models_candidate_row, sample_provider,
    unrestricted_models_snapshot, InMemoryAuthApiKeySnapshotRepository,
    InMemoryMinimalCandidateSelectionReadRepository, InMemoryProviderCatalogReadRepository,
    InMemoryRequestCandidateRepository, DEVELOPMENT_ENCRYPTION_KEY,
};
use crate::tests::{
    any, build_router, build_router_with_state, build_state_with_execution_runtime_override, json,
    start_server, AppState, Arc, Body, HeaderValue, Json, Mutex, Request, Response, Router,
    StatusCode, CONTROL_ACTION_PROXY_PUBLIC, CONTROL_EXECUTED_HEADER,
    EXECUTION_PATH_EXECUTION_RUNTIME_STREAM, EXECUTION_PATH_EXECUTION_RUNTIME_SYNC,
    EXECUTION_PATH_HEADER,
};
use base64::Engine as _;

#[tokio::test]
async fn gateway_handles_internal_gateway_resolve_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/resolve"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["route_class"], "ai_public");
    assert_eq!(payload["route_family"], "openai");
    assert_eq!(payload["route_kind"], "chat");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/resolve"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/messages",
            "headers": {
                "authorization": "Bearer local-token",
            },
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["route_class"], "ai_public");
    assert_eq!(payload["route_family"], "claude");
    assert_eq!(payload["route_kind"], "messages");
    assert_eq!(payload["request_auth_channel"], "bearer_like");
    assert_eq!(payload["auth_endpoint_signature"], "claude:messages");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_internal_gateway_proxy_public_action_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/execute-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/execute-sync"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {},
            "body_json": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], CONTROL_ACTION_PROXY_PUBLIC);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_internal_gateway_plan_sync_proxy_public_action_without_proxying_upstream()
{
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/plan-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/plan-sync"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {},
            "body_json": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], CONTROL_ACTION_PROXY_PUBLIC);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_execute_sync_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner
                    .lock()
                    .expect("mutex should lock") += 1;
                Json(json!({
                    "request_id": "req-internal-execute-sync-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-local-execute-sync",
                            "object": "chat.completion",
                            "choices": []
                        }
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-execute-sync", "user-client-execute-sync"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-execute-sync-1",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-execute-sync-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-execute-sync-1",
            "provider-execute-sync-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-execute-sync-1",
            "provider-execute-sync-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/execute-sync"))
        .json(&json!({
            "trace_id": "trace-internal-execute-sync",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            },
            "auth_context": {
                "user_id": "user-client-execute-sync",
                "api_key_id": "api-key-client-execute-sync",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_EXECUTION_RUNTIME_SYNC)
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["id"], "chatcmpl-local-execute-sync");
    assert_eq!(payload["object"], "chat.completion");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_returns_internal_gateway_execute_stream_proxy_public_action_without_proxying_upstream(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/execute-stream",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/execute-stream"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {},
            "body_json": {},
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], CONTROL_ACTION_PROXY_PUBLIC);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_execute_stream_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/stream",
        any(move |_request: Request| {
            let execution_runtime_hits_inner = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits_inner.lock().expect("mutex should lock") += 1;
                let frames = concat!(
                    "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: one\\n\\n\"}}\n",
                    "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: [DONE]\\n\\n\"}}\n",
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
        None,
        unrestricted_models_snapshot(
            "api-key-client-execute-stream",
            "user-client-execute-stream",
        ),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-execute-stream-1",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-execute-stream-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-execute-stream-1",
            "provider-execute-stream-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-execute-stream-1",
            "provider-execute-stream-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let gateway = build_router_with_state(
        build_state_with_execution_runtime_override(execution_runtime_url)
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/execute-stream"))
        .json(&json!({
            "trace_id": "trace-internal-execute-stream",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
                "stream": true,
            },
            "auth_context": {
                "user_id": "user-client-execute-stream",
                "api_key_id": "api-key-client-execute-stream",
                "access_allowed": true,
            }
        }))
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
        "data: one\n\ndata: [DONE]\n\n"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_resolve_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-internal-resolve")),
        unrestricted_models_snapshot("key-internal-resolve", "user-internal-resolve"),
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/resolve"))
        .json(&json!({
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "x-api-key": "sk-internal-resolve",
            },
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["route_class"], "ai_public");
    assert_eq!(payload["route_family"], "openai");
    assert_eq!(payload["route_kind"], "chat");
    assert_eq!(payload["auth_endpoint_signature"], "openai:chat");
    assert_eq!(payload["auth_context"]["user_id"], "user-internal-resolve");
    assert_eq!(
        payload["auth_context"]["api_key_id"],
        "key-internal-resolve"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_auth_context_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-internal-auth-context")),
        unrestricted_models_snapshot("key-internal-auth-context", "user-internal-auth-context"),
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/auth-context"))
        .json(&json!({
            "headers": {
                "x-api-key": "sk-internal-auth-context",
            },
            "auth_endpoint_signature": "openai:chat",
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload["auth_context"]["user_id"],
        "user-internal-auth-context"
    );
    assert_eq!(
        payload["auth_context"]["api_key_id"],
        "key-internal-auth-context"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_report_sync_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/report-sync"))
        .json(&json!({
            "trace_id": "trace-internal-report-sync",
            "report_kind": "openai_chat_sync_success",
            "report_context": {
                "user_id": "user-report-sync",
                "api_key_id": "api-key-report-sync",
            },
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "id": "chatcmpl-report-sync",
                "usage": {
                    "input_tokens": 1,
                    "output_tokens": 2,
                    "total_tokens": 3,
                }
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, json!({ "ok": true }));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_report_stream_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/report-stream"))
        .json(&json!({
            "trace_id": "trace-internal-report-stream",
            "report_kind": "openai_chat_stream_success",
            "report_context": {
                "user_id": "user-report-stream",
                "api_key_id": "api-key-report-stream",
            },
            "status_code": 200,
            "headers": {
                "content-type": "text/event-stream",
            },
            "body_base64": base64::engine::general_purpose::STANDARD.encode("data: [DONE]\\n\\n"),
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, json!({ "ok": true }));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-sync",
            "report_kind": "openai_chat_sync_finalize",
            "report_context": {
                "user_id": "user-finalize-sync",
                "api_key_id": "api-key-finalize-sync",
                "client_api_format": "openai:chat",
                "provider_api_format": "openai:chat",
            },
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "id": "chatcmpl-local-finalize",
                "object": "chat.completion",
                "choices": [],
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "id": "chatcmpl-local-finalize",
            "object": "chat.completion",
            "choices": [],
        })
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_openai_video_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-video",
            "report_kind": "openai_video_create_sync_finalize",
            "report_context": {
                "user_id": "user-finalize-video",
                "api_key_id": "api-key-finalize-video",
                "model": "sora-2",
                "local_task_id": "local-video-task-123",
                "local_created_at": 1712345678u64,
                "original_request_body": {
                    "prompt": "make a trailer"
                }
            },
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "id": "vid-ext-123",
                "status": "submitted",
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "id": "local-video-task-123",
            "object": "video",
            "status": "queued",
            "progress": 0,
            "created_at": 1712345678u64,
            "model": "sora-2",
            "prompt": "make a trailer",
        })
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_gemini_video_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-gemini-video",
            "report_kind": "gemini_video_create_sync_finalize",
            "report_context": {
                "user_id": "user-finalize-gemini-video",
                "api_key_id": "api-key-finalize-gemini-video",
                "model": "veo-3",
                "local_short_id": "gemini-short-123"
            },
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "name": "operations/123",
                "done": false,
                "metadata": {},
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "name": "models/veo-3/operations/gemini-short-123",
            "done": false,
            "metadata": {},
        })
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_openai_video_delete_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-video-delete",
            "report_kind": "openai_video_delete_sync_finalize",
            "report_context": {
                "task_id": "video-delete-123"
            },
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "id": "video-delete-123",
            "object": "video",
            "deleted": true,
        })
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_finalize_sync_gemini_video_cancel_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-gemini-video-cancel",
            "report_kind": "gemini_video_cancel_sync_finalize",
            "report_context": {
                "task_id": "gemini-cancel-123",
                "model": "veo-3"
            },
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTROL_EXECUTED_HEADER)
            .expect("control executed header should exist"),
        "true"
    );
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload, json!({}));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_internal_gateway_finalize_sync_unknown_kind_locally() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/gateway/finalize-sync",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                Json(json!({ "proxied": true }))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/finalize-sync"))
        .json(&json!({
            "trace_id": "trace-internal-finalize-unknown-kind",
            "report_kind": "openai_video_unknown_sync_finalize",
            "report_context": {},
            "status_code": 200,
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {}
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(
        payload,
        json!({
            "detail": "Unsupported gateway sync finalize kind",
        })
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_decision_sync_locally_with_supplied_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-1", "user-client-1"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row("provider-1", "openai", "openai:chat", "gpt-5", 10),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-1",
            "provider-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-1",
            "provider-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/decision-sync"))
        .json(&json!({
            "trace_id": "trace-internal-decision-sync",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            },
            "auth_context": {
                "user_id": "user-client-1",
                "api_key_id": "api-key-client-1",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], "execution_runtime_sync_decision");
    assert_eq!(payload["decision_kind"], "openai_chat_sync");
    assert_eq!(payload["provider_id"], "provider-1");
    assert_eq!(payload["endpoint_id"], "endpoint-provider-1");
    assert_eq!(payload["key_id"], "key-provider-1");
    assert_eq!(payload["provider_api_format"], "openai:chat");
    assert_eq!(payload["client_api_format"], "openai:chat");
    assert_eq!(payload["model_name"], "gpt-5");
    assert_eq!(payload["auth_context"], serde_json::Value::Null);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_returns_internal_gateway_decision_sync_fallback_with_resolved_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-decision-fallback")),
        unrestricted_models_snapshot("api-key-fallback-1", "user-fallback-1"),
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_auth_api_key_data_reader_for_tests(auth_repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/decision-sync"))
        .json(&json!({
            "trace_id": "trace-internal-decision-fallback",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
                "x-api-key": "sk-decision-fallback",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], "fallback_plan");
    assert_eq!(payload["auth_context"]["user_id"], "user-fallback-1");
    assert_eq!(payload["auth_context"]["api_key_id"], "api-key-fallback-1");
    assert_eq!(payload["auth_context"]["access_allowed"], true);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_decision_stream_locally_with_supplied_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-stream", "user-client-stream"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row("provider-stream-1", "openai", "openai:chat", "gpt-5", 10),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-stream-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-stream-1",
            "provider-stream-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-stream-1",
            "provider-stream-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/api/internal/gateway/decision-stream"
        ))
        .json(&json!({
            "trace_id": "trace-internal-decision-stream",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
                "stream": true,
            },
            "auth_context": {
                "user_id": "user-client-stream",
                "api_key_id": "api-key-client-stream",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], "execution_runtime_stream_decision");
    assert_eq!(payload["decision_kind"], "openai_chat_stream");
    assert_eq!(payload["provider_id"], "provider-stream-1");
    assert_eq!(payload["endpoint_id"], "endpoint-provider-stream-1");
    assert_eq!(payload["key_id"], "key-provider-stream-1");
    assert_eq!(payload["auth_context"], serde_json::Value::Null);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_plan_sync_locally_with_supplied_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-plan-sync", "user-client-plan-sync"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-plan-sync-1",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-plan-sync-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-plan-sync-1",
            "provider-plan-sync-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-plan-sync-1",
            "provider-plan-sync-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/plan-sync"))
        .json(&json!({
            "trace_id": "trace-internal-plan-sync",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
            },
            "auth_context": {
                "user_id": "user-client-plan-sync",
                "api_key_id": "api-key-client-plan-sync",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], "execution_runtime_sync");
    assert_eq!(payload["plan_kind"], "openai_chat_sync");
    assert_eq!(payload["plan"]["provider_id"], "provider-plan-sync-1");
    assert_eq!(
        payload["plan"]["endpoint_id"],
        "endpoint-provider-plan-sync-1"
    );
    assert_eq!(payload["plan"]["key_id"], "key-provider-plan-sync-1");
    assert_eq!(payload["auth_context"], serde_json::Value::Null);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_gateway_plan_stream_locally_with_supplied_auth_context() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("proxied"))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        unrestricted_models_snapshot("api-key-client-plan-stream", "user-client-plan-stream"),
    )]));
    let candidate_repository =
        Arc::new(InMemoryMinimalCandidateSelectionReadRepository::seed(vec![
            sample_models_candidate_row(
                "provider-plan-stream-1",
                "openai",
                "openai:chat",
                "gpt-5",
                10,
            ),
        ]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-plan-stream-1", "openai", 10)],
        vec![sample_endpoint(
            "endpoint-provider-plan-stream-1",
            "provider-plan-stream-1",
            "openai:chat",
            "https://api.openai.example",
        )],
        vec![sample_key(
            "key-provider-plan-stream-1",
            "provider-plan-stream-1",
            "openai:chat",
            "sk-upstream-openai",
        )],
    ));
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_auth_candidate_selection_provider_catalog_and_request_candidate_repository_for_tests(
                    auth_repository,
                    candidate_repository,
                    provider_catalog_repository,
                    request_candidate_repository,
                    DEVELOPMENT_ENCRYPTION_KEY,
                ),
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/gateway/plan-stream"))
        .json(&json!({
            "trace_id": "trace-internal-plan-stream",
            "method": "POST",
            "path": "/v1/chat/completions",
            "headers": {
                "content-type": "application/json",
            },
            "body_json": {
                "model": "gpt-5",
                "messages": [],
                "stream": true,
            },
            "auth_context": {
                "user_id": "user-client-plan-stream",
                "api_key_id": "api-key-client-plan-stream",
                "access_allowed": true,
            }
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["action"], "execution_runtime_stream");
    assert_eq!(payload["plan_kind"], "openai_chat_stream");
    assert_eq!(payload["plan"]["provider_id"], "provider-plan-stream-1");
    assert_eq!(
        payload["plan"]["endpoint_id"],
        "endpoint-provider-plan-stream-1"
    );
    assert_eq!(payload["plan"]["key_id"], "key-provider-plan-stream-1");
    assert_eq!(payload["plan"]["stream"], true);
    assert_eq!(payload["auth_context"], serde_json::Value::Null);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}
