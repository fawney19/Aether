use super::super::usage::{
    hash_api_key, sample_local_openai_auth_snapshot, sample_local_openai_candidate_row,
    sample_local_openai_endpoint, sample_local_openai_key, sample_local_openai_provider,
};
use crate::tests::{
    any, build_router_with_execution_runtime_override, build_router_with_state,
    build_state_with_execution_runtime_override, json, start_server, AppState, Arc, Body,
    FrontdoorCorsConfig, Mutex, Request, Router, StatusCode, FRONTDOOR_MANIFEST_PATH, READYZ_PATH,
};
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data::repository::auth::InMemoryAuthApiKeySnapshotRepository;
use aether_data::repository::candidate_selection::InMemoryMinimalCandidateSelectionReadRepository;
use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::usage::InMemoryUsageReadRepository;
use axum::Json;

use crate::data::GatewayDataState;

#[tokio::test]
async fn gateway_exposes_frontdoor_manifest_without_proxying_upstream() {
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
    let gateway = build_router_with_execution_runtime_override("http://127.0.0.1:19091");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}{FRONTDOOR_MANIFEST_PATH}"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["component"], "aether-gateway");
    assert_eq!(payload["mode"], "compatibility_frontdoor");
    assert_eq!(
        payload["entrypoints"]["public_manifest"],
        FRONTDOOR_MANIFEST_PATH
    );
    assert_eq!(payload["entrypoints"]["readiness"], READYZ_PATH);
    assert_eq!(payload["entrypoints"]["health"], "/_gateway/health");
    assert_eq!(
        payload["rust_frontdoor"]["capabilities"]["public_proxy_catch_all"],
        true
    );
    let owned_routes = payload["rust_frontdoor"]["owned_route_patterns"]
        .as_array()
        .expect("owned route patterns should be an array");
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1/chat/completions"));
    assert!(owned_routes.iter().any(|value| value == "/v1/messages"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1/messages/count_tokens"));
    assert!(owned_routes.iter().any(|value| value == "/v1/responses"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1/responses/compact"));
    assert!(owned_routes.iter().any(|value| value == "/v1/alpha/search"));
    assert!(owned_routes.iter().any(|value| value == "/health"));
    assert!(owned_routes.iter().any(|value| value == "/v1/health"));
    assert!(owned_routes.iter().any(|value| value == "/v1/providers"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1/providers/{path...}"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1/test-connection"));
    assert!(owned_routes.iter().any(|value| value == "/test-connection"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/public/providers"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/oauth/providers"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/oauth/{provider_type}/authorize"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/oauth/{provider_type}/callback"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/user/oauth/bindable-providers"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/user/oauth/links"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/user/oauth/{provider_type}/bind-token"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/user/oauth/{provider_type}/bind"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/user/oauth/{provider_type}"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/capabilities"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/public/health/api-formats"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/public/health/models"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/modules/auth-status"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/internal/gateway/{path...}"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/internal/proxy-tunnel"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/internal/tunnel/heartbeat"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/internal/tunnel/node-status"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/internal/tunnel/relay/{node_id}"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/capabilities/user-configurable"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/api/capabilities/model/{path...}"));
    assert!(owned_routes.iter().any(|value| value == "/v1/models"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1/models/{path...}"));
    assert!(owned_routes.iter().any(|value| value == "/v1beta/models"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1beta/models/{path...}"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1beta/models/{model}:generateContent"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1beta/models/{model}:streamGenerateContent"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1beta/models/{model}:predictLongRunning"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1beta/operations/{id}"));
    assert!(owned_routes.iter().any(|value| value == "/v1/videos"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1/videos/{path...}"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/upload/v1beta/files"));
    assert!(owned_routes.iter().any(|value| value == "/v1beta/files"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1beta/files/{path...}"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1internal:loadCodeAssist"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1internal:fetchAvailableModels"));
    assert!(owned_routes
        .iter()
        .any(|value| value == "/v1internal:streamGenerateContent"));
    assert_eq!(
        payload["rust_frontdoor"]["internal_gateway"]["status"],
        "rust_native_control_plane"
    );
    assert_eq!(
        payload["rust_frontdoor"]["internal_gateway"]["path_prefixes"][0],
        "/api/internal/gateway"
    );
    assert_eq!(payload["features"]["control_api_configured"], true);
    assert_eq!(payload["features"]["execution_runtime_configured"], true);
    assert!(payload["features"]
        .get("remote_executor_configured")
        .is_none());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_reports_local_control_plane_as_configured_without_external_control_config() {
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
    let gateway = build_router_with_execution_runtime_override("http://127.0.0.1:19091");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let manifest = reqwest::Client::new()
        .get(format!("{gateway_url}{FRONTDOOR_MANIFEST_PATH}"))
        .send()
        .await
        .expect("manifest request should succeed");
    assert_eq!(manifest.status(), StatusCode::OK);
    let manifest_payload: serde_json::Value = manifest.json().await.expect("manifest should parse");
    assert_eq!(manifest_payload["features"]["control_api_configured"], true);
    assert_eq!(
        manifest_payload["features"]["execution_runtime_configured"],
        true
    );
    assert!(manifest_payload["features"]
        .get("remote_executor_configured")
        .is_none());

    let health = reqwest::Client::new()
        .get(format!("{gateway_url}/_gateway/health"))
        .send()
        .await
        .expect("health request should succeed");
    assert_eq!(health.status(), StatusCode::OK);
    let health_payload: serde_json::Value = health.json().await.expect("health should parse");
    assert_eq!(health_payload["control_api_enabled"], true);

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_reports_execution_runtime_as_configured_without_execution_runtime_override() {
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
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let manifest = reqwest::Client::new()
        .get(format!("{gateway_url}{FRONTDOOR_MANIFEST_PATH}"))
        .send()
        .await
        .expect("manifest request should succeed");
    assert_eq!(manifest.status(), StatusCode::OK);
    let manifest_payload: serde_json::Value = manifest.json().await.expect("manifest should parse");
    assert_eq!(manifest_payload["features"]["control_api_configured"], true);
    assert_eq!(
        manifest_payload["features"]["execution_runtime_configured"],
        true
    );
    assert!(manifest_payload["features"]
        .get("remote_executor_configured")
        .is_none());

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_handles_cors_preflight_without_proxying_upstream() {
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
    let state = AppState::new()
        .expect("state should build")
        .with_frontdoor_cors_config(
            FrontdoorCorsConfig::new(vec!["http://localhost:3000".to_string()], true)
                .expect("cors config should build"),
        );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("{gateway_url}/v1/chat/completions"),
        )
        .header("origin", "http://localhost:3000")
        .header("access-control-request-method", "POST")
        .header(
            "access-control-request-headers",
            "authorization,content-type",
        )
        .send()
        .await
        .expect("preflight should succeed");

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .expect("allow origin header"),
        "http://localhost:3000"
    );
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-credentials")
            .expect("allow credentials header"),
        "true"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[test]
fn gateway_adds_cors_headers_to_proxied_responses() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(16 * 1024 * 1024)
        .enable_all()
        .build()
        .expect("runtime should build");

    runtime.block_on(gateway_adds_cors_headers_to_proxied_responses_inner());
}

async fn gateway_adds_cors_headers_to_proxied_responses_inner() {
    let execution_runtime_hits = Arc::new(Mutex::new(0usize));
    let execution_runtime_hits_clone = Arc::clone(&execution_runtime_hits);
    let execution_runtime = Router::new().route(
        "/v1/execute/sync",
        any(move |_request: Request| {
            let execution_runtime_hits = Arc::clone(&execution_runtime_hits_clone);
            async move {
                *execution_runtime_hits.lock().expect("mutex should lock") += 1;
                Json(json!({
                    "request_id": "trace-openai-cors-proxy-123",
                    "status_code": 200,
                    "headers": {
                        "content-type": "application/json"
                    },
                    "body": {
                        "json_body": {
                            "id": "chatcmpl-cors-proxy-123",
                            "object": "chat.completion",
                            "model": "gpt-5-upstream",
                            "choices": []
                        }
                    }
                }))
            }
        }),
    );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-cors")),
        sample_local_openai_auth_snapshot("api-key-openai-cors-1", "user-openai-cors-1"),
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
    let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
    let usage_repository = Arc::new(InMemoryUsageReadRepository::default());

    let (execution_runtime_url, execution_runtime_handle) = start_server(execution_runtime).await;
    let state = build_state_with_execution_runtime_override(execution_runtime_url)
        .with_frontdoor_cors_config(
            FrontdoorCorsConfig::new(vec!["http://localhost:3000".to_string()], true)
                .expect("cors config should build"),
        )
        .with_data_state_for_tests(
            GatewayDataState::with_auth_candidate_selection_provider_catalog_request_candidates_and_usage_for_tests(
                auth_repository,
                candidate_selection_repository,
                provider_catalog_repository,
                request_candidate_repository,
                usage_repository,
                DEVELOPMENT_ENCRYPTION_KEY,
            ),
        );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header("origin", "http://localhost:3000")
        .header(http::header::AUTHORIZATION, "Bearer sk-client-openai-cors")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("proxy request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let response_headers = response.headers().clone();
    assert_eq!(
        response
            .json::<serde_json::Value>()
            .await
            .expect("body should parse")["id"],
        "chatcmpl-cors-proxy-123"
    );
    assert_eq!(
        response_headers
            .get("access-control-allow-origin")
            .expect("allow origin header"),
        "http://localhost:3000"
    );
    assert_eq!(
        response_headers
            .get("access-control-expose-headers")
            .expect("expose headers header"),
        "*"
    );
    assert_eq!(
        *execution_runtime_hits.lock().expect("mutex should lock"),
        1
    );

    gateway_handle.abort();
    execution_runtime_handle.abort();
}
