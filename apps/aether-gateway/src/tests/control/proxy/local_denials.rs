use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_contracts::tunnel::{
    sign_tunnel_relay_request, tunnel_relay_payload_digest, TUNNEL_RELAY_AUTH_NONCE_HEADER,
    TUNNEL_RELAY_AUTH_PAYLOAD_HEADER, TUNNEL_RELAY_AUTH_SENDER_HEADER,
    TUNNEL_RELAY_AUTH_SIGNATURE_HEADER, TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
    TUNNEL_RELAY_FORWARDED_BY_HEADER, TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
};
use axum::body::Body;
use axum::routing::any;
use axum::{extract::Request, Json, Router};
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri};
use serde_json::json;

use super::super::{
    build_router_with_state, hash_api_key, sample_currently_usable_auth_snapshot,
    sample_expired_auth_snapshot, sample_locked_auth_snapshot, start_server, AppState,
    GatewayDataState, InMemoryAuthApiKeySnapshotRepository, InMemoryProviderCatalogReadRepository,
    InMemoryWalletRepository, StoredProviderCatalogProvider,
};
use crate::constants::{
    CONTROL_ROUTE_CLASS_HEADER, EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_AUTH_DENIED,
    FORWARDED_FOR_HEADER, GATEWAY_HEADER, TRACE_ID_HEADER, TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
    TRUSTED_AUTH_API_KEY_ID_HEADER, TRUSTED_AUTH_BALANCE_HEADER, TRUSTED_AUTH_USER_ID_HEADER,
    TUNNEL_AFFINITY_FORWARDED_BY_HEADER, TUNNEL_AFFINITY_NODE_ID_HEADER,
    TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
};

const RELAY_TEST_SECRET: &str = "relay-test-secret-at-least-32-bytes";
const RELAY_TEST_SENDER: &str = "gateway-a";
const RELAY_TEST_OWNER: &str = "gateway-b";
const RELAY_TEST_NODE_ID: &str = "node-1";
const AFFINITY_TEST_BODY: &str = "{\"model\":\"gpt-5\",\"messages\":[]}";

fn signed_affinity_headers(
    method: &Method,
    uri: &Uri,
    user_id: &str,
    api_key_id: &str,
    access_allowed: bool,
    balance: Option<&str>,
    body: &[u8],
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        GATEWAY_HEADER,
        HeaderValue::from_static("rust-phase3b-affinity"),
    );
    headers.insert(
        TUNNEL_RELAY_FORWARDED_BY_HEADER,
        HeaderValue::from_static(RELAY_TEST_SENDER),
    );
    headers.insert(
        TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
        HeaderValue::from_static(RELAY_TEST_SENDER),
    );
    headers.insert(
        TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
        HeaderValue::from_static(RELAY_TEST_OWNER),
    );
    headers.insert(
        TUNNEL_AFFINITY_NODE_ID_HEADER,
        HeaderValue::from_static(RELAY_TEST_NODE_ID),
    );
    headers.insert(
        TRUSTED_AUTH_USER_ID_HEADER,
        HeaderValue::from_str(user_id).expect("trusted user ID should be a valid header value"),
    );
    headers.insert(
        TRUSTED_AUTH_API_KEY_ID_HEADER,
        HeaderValue::from_str(api_key_id)
            .expect("trusted API key ID should be a valid header value"),
    );
    headers.insert(
        TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
        HeaderValue::from_static(if access_allowed { "true" } else { "false" }),
    );
    headers.insert(
        FORWARDED_FOR_HEADER,
        HeaderValue::from_static("203.0.113.10"),
    );
    if let Some(balance) = balance {
        headers.insert(
            TRUSTED_AUTH_BALANCE_HEADER,
            HeaderValue::from_str(balance).expect("trusted balance should be a valid header value"),
        );
    }

    let metadata = crate::tunnel::build_tunnel_affinity_auth_metadata(method, uri, &headers)
        .expect("affinity authentication metadata should build");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after epoch")
        .as_secs();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let payload_digest = tunnel_relay_payload_digest(&metadata, body);
    let signature = sign_tunnel_relay_request(
        RELAY_TEST_SECRET.as_bytes(),
        RELAY_TEST_SENDER,
        RELAY_TEST_OWNER,
        RELAY_TEST_NODE_ID,
        RELAY_TEST_SENDER,
        false,
        timestamp,
        &nonce,
        &payload_digest,
    );
    headers.insert(
        TUNNEL_RELAY_AUTH_SENDER_HEADER,
        HeaderValue::from_static(RELAY_TEST_SENDER),
    );
    headers.insert(
        TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
        HeaderValue::from_static(RELAY_TEST_OWNER),
    );
    headers.insert(
        TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
        HeaderValue::from_str(&timestamp.to_string()).expect("timestamp should be a valid header"),
    );
    headers.insert(
        TUNNEL_RELAY_AUTH_NONCE_HEADER,
        HeaderValue::from_str(&nonce).expect("nonce should be a valid header"),
    );
    headers.insert(
        TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
        HeaderValue::from_str(&payload_digest.encode_header_value())
            .expect("payload digest should be a valid header"),
    );
    headers.insert(
        TUNNEL_RELAY_AUTH_SIGNATURE_HEADER,
        HeaderValue::from_str(&signature).expect("signature should be a valid header"),
    );
    headers
}

fn signed_affinity_request(
    client: &reqwest::Client,
    url: String,
    path: &str,
    user_id: &str,
    api_key_id: &str,
    access_allowed: bool,
    balance: Option<&str>,
    body: &[u8],
) -> reqwest::RequestBuilder {
    let method = Method::POST;
    let uri = path.parse::<Uri>().expect("request path should be valid");
    client
        .request(method.clone(), url)
        .headers(signed_affinity_headers(
            &method,
            &uri,
            user_id,
            api_key_id,
            access_allowed,
            balance,
            body,
        ))
}

#[tokio::test]
async fn gateway_locally_denies_explicit_trusted_balance_failure_without_hitting_control_or_upstream(
) {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({
                        "auth_context": {
                            "user_id": "user-from-control",
                            "api_key_id": "key-from-control",
                            "balance_remaining": 99.0,
                            "access_allowed": true
                        }
                    }))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_currently_usable_auth_snapshot("key-123", "user-123"),
    )]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository)
            .with_tunnel_identity_and_relay_secret_for_tests(
                RELAY_TEST_OWNER,
                None,
                RELAY_TEST_SECRET,
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let response = signed_affinity_request(
        &client,
        format!("{gateway_url}/v1/chat/completions"),
        "/v1/chat/completions",
        "user-123",
        "key-123",
        false,
        Some("0"),
        AFFINITY_TEST_BODY.as_bytes(),
    )
    .header(http::header::CONTENT_TYPE, "application/json")
    .header(TRACE_ID_HEADER, "trace-control-balance-denied-1")
    .body(AFFINITY_TEST_BODY)
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    assert_eq!(
        response
            .headers()
            .get(CONTROL_ROUTE_CLASS_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("ai_public")
    );
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["error"]["type"], "balance_exceeded");
    assert_eq!(payload["error"]["message"], "余额不足（剩余: $0.00）");
    assert_eq!(payload["error"]["details"]["balance_type"], "USD");
    assert_eq!(payload["error"]["details"]["remaining"], 0.0);

    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_invalid_trusted_snapshot_without_hitting_control_or_upstream() {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({
                        "auth_context": {
                            "user_id": "user-from-control",
                            "api_key_id": "key-from-control",
                            "access_allowed": true
                        }
                    }))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_expired_auth_snapshot("key-123", "user-123"),
    )]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository)
            .with_tunnel_identity_and_relay_secret_for_tests(
                RELAY_TEST_OWNER,
                None,
                RELAY_TEST_SECRET,
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let response = signed_affinity_request(
        &client,
        format!("{gateway_url}/v1/chat/completions"),
        "/v1/chat/completions",
        "user-123",
        "key-123",
        true,
        None,
        AFFINITY_TEST_BODY.as_bytes(),
    )
    .header(http::header::CONTENT_TYPE, "application/json")
    .header(TRACE_ID_HEADER, "trace-control-invalid-trusted-1")
    .body(AFFINITY_TEST_BODY)
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(payload["error"]["message"], "无效的API密钥");

    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_missing_wallet_without_hitting_control_or_upstream() {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"auth_context": null}))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_currently_usable_auth_snapshot("key-123", "user-123"),
    )]));
    let wallet_repository = Arc::new(InMemoryWalletRepository::default());
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let data_state =
        GatewayDataState::with_auth_and_wallet_for_tests(auth_repository, wallet_repository);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_and_relay_secret_for_tests(
                RELAY_TEST_OWNER,
                None,
                RELAY_TEST_SECRET,
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let response = signed_affinity_request(
        &client,
        format!("{gateway_url}/v1/chat/completions"),
        "/v1/chat/completions",
        "user-123",
        "key-123",
        true,
        None,
        AFFINITY_TEST_BODY.as_bytes(),
    )
    .header(http::header::CONTENT_TYPE, "application/json")
    .header(TRACE_ID_HEADER, "trace-control-wallet-missing-1")
    .body(AFFINITY_TEST_BODY)
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(payload["error"]["message"], "钱包不可用");

    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_invalid_bearer_api_key_without_hitting_control_or_upstream() {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"auth_context": null}))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-other")),
        sample_currently_usable_auth_snapshot("key-123", "user-123"),
    )]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::AUTHORIZATION, "Bearer sk-missing")
        .header(TRACE_ID_HEADER, "trace-control-invalid-bearer-1")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert!(payload.get("type").is_none());
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(payload["error"]["message"], "无效的API密钥");
    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_claude_routes_use_anthropic_authentication_error_for_invalid_api_key() {
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-other-claude-key")),
        sample_currently_usable_auth_snapshot("key-claude-other", "user-claude-other"),
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;
    let client = reqwest::Client::new();

    for (path, trace_id) in [
        ("/v1/messages", "trace-control-claude-invalid-key-messages"),
        (
            "/v1/messages/count_tokens",
            "trace-control-claude-invalid-key-count-tokens",
        ),
    ] {
        let response = client
            .post(format!("{gateway_url}{path}"))
            .header(http::header::CONTENT_TYPE, "application/json")
            .header("x-api-key", "sk-missing-claude-key")
            .header(TRACE_ID_HEADER, trace_id)
            .body("{\"model\":\"claude-sonnet-4-5\",\"messages\":[]}")
            .send()
            .await
            .expect("request should complete locally");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "path: {path}");
        assert_eq!(
            response
                .headers()
                .get(EXECUTION_PATH_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(EXECUTION_PATH_LOCAL_AUTH_DENIED),
            "path: {path}"
        );
        let payload: serde_json::Value = response.json().await.expect("response json should parse");
        assert_eq!(payload["type"], "error", "path: {path}");
        assert_eq!(
            payload["error"]["type"], "authentication_error",
            "path: {path}"
        );
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_admin_proxy_without_admin_principal_and_without_hitting_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);

    let upstream = Router::new().route(
        "/api/admin/endpoints/health/summary",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/endpoints/health/summary"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["detail"], "admin authentication required");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_unclassified_oauth_public_route_as_local_not_found_without_hitting_upstream(
) {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);

    let upstream = Router::new().route(
        "/api/oauth/linuxdo/unsupported",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/oauth/linuxdo/unsupported"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(payload["error"]["message"], "Route not found");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_rejects_unclassified_admin_route_without_hitting_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);

    let upstream = Router::new().route(
        "/api/admin/unsupported",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/admin/unsupported"))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(
        payload["detail"],
        "admin proxy route not implemented in rust frontdoor"
    );
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_disallowed_claude_api_format_without_hitting_control_or_upstream() {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"auth_context": null}))
                }
            }),
        )
        .route(
            "/v1/messages",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let mut snapshot = sample_currently_usable_auth_snapshot("key-claude-123", "user-claude-123");
    snapshot.api_key_allowed_providers = Some(vec!["claude".to_string()]);
    snapshot.user_allowed_providers = Some(vec!["claude".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["openai:chat".to_string()]);
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-claude-auth-123")),
        snapshot,
    )]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", "sk-claude-auth-123")
        .header(TRACE_ID_HEADER, "trace-control-claude-format-1")
        .body("{\"model\":\"claude-3-7-sonnet\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["type"], "error");
    assert_eq!(payload["error"]["type"], "permission_error");
    assert_eq!(
        payload["error"]["message"],
        "当前用户、用户组或密钥的访问控制策略不允许访问 claude:messages 格式"
    );
    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_disallowed_provider_without_hitting_control_or_upstream() {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"auth_context": null}))
                }
            }),
        )
        .route(
            "/v1/messages",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let mut snapshot =
        sample_currently_usable_auth_snapshot("key-claude-allow-123", "user-claude-allow-123");
    snapshot.api_key_allowed_api_formats = None;
    snapshot.user_allowed_api_formats = None;
    snapshot.api_key_allowed_providers = Some(vec!["openai".to_string()]);
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-claude-provider-123")),
        snapshot,
    )]));
    let provider_catalog = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![StoredProviderCatalogProvider::new(
            "provider-openai-1".to_string(),
            "OpenAI Pool 1".to_string(),
            None,
            "openai".to_string(),
        )
        .expect("provider should build")],
        Vec::new(),
        Vec::new(),
    ));
    let data = GatewayDataState::with_auth_api_key_reader_for_tests(repository)
        .with_provider_catalog_reader(provider_catalog);
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", "sk-claude-provider-123")
        .header(TRACE_ID_HEADER, "trace-control-provider-guard-1")
        .body("{\"model\":\"claude-3-7-sonnet\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["type"], "error");
    assert_eq!(payload["error"]["type"], "permission_error");
    assert_eq!(
        payload["error"]["message"],
        "当前用户、用户组或密钥的访问控制策略不允许访问 claude 提供商"
    );
    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_disallowed_gemini_model_without_hitting_control_or_upstream() {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"auth_context": null}))
                }
            }),
        )
        .route(
            "/v1beta/models/{*path}",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let mut snapshot = sample_currently_usable_auth_snapshot("key-gemini-123", "user-gemini-123");
    snapshot.api_key_allowed_providers = Some(vec!["gemini".to_string()]);
    snapshot.user_allowed_providers = Some(vec!["gemini".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["gemini:generate_content".to_string()]);
    snapshot.user_allowed_api_formats = Some(vec!["gemini:generate_content".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["gemini-1.5-pro".to_string()]);
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("gemini-client-key-123")),
        snapshot,
    )]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{gateway_url}/v1beta/models/gemini-2.5-pro:generateContent"
        ))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-goog-api-key", "gemini-client-key-123")
        .header(TRACE_ID_HEADER, "trace-control-gemini-model-1")
        .body("{\"contents\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "当前用户、用户组或密钥的访问控制策略不允许访问模型 gemini-2.5-pro"
    );
    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_locked_trusted_snapshot_without_hitting_control_or_upstream() {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"auth_context": null}))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some("hash-1".to_string()),
        sample_locked_auth_snapshot("key-locked-123", "user-locked-123"),
    )]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository)
            .with_tunnel_identity_and_relay_secret_for_tests(
                RELAY_TEST_OWNER,
                None,
                RELAY_TEST_SECRET,
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let response = signed_affinity_request(
        &client,
        format!("{gateway_url}/v1/chat/completions"),
        "/v1/chat/completions",
        "user-locked-123",
        "key-locked-123",
        true,
        None,
        AFFINITY_TEST_BODY.as_bytes(),
    )
    .header(http::header::CONTENT_TYPE, "application/json")
    .header(TRACE_ID_HEADER, "trace-control-locked-trusted-1")
    .body(AFFINITY_TEST_BODY)
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "该密钥已被管理员锁定，请联系管理员"
    );
    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_disallowed_openai_model_without_hitting_control_or_upstream() {
    let auth_context_hits = Arc::new(Mutex::new(0usize));
    let auth_context_hits_clone = Arc::clone(&auth_context_hits);
    let public_hits = Arc::new(Mutex::new(0usize));
    let public_hits_clone = Arc::clone(&public_hits);

    let upstream = Router::new()
        .route(
            "/api/internal/gateway/auth-context",
            any(move |_request: Request| {
                let auth_context_hits_inner = Arc::clone(&auth_context_hits_clone);
                async move {
                    *auth_context_hits_inner.lock().expect("mutex should lock") += 1;
                    Json(json!({"auth_context": null}))
                }
            }),
        )
        .route(
            "/v1/chat/completions",
            any(move |_request: Request| {
                let public_hits_inner = Arc::clone(&public_hits_clone);
                async move {
                    *public_hits_inner.lock().expect("mutex should lock") += 1;
                    (StatusCode::OK, Body::from("unexpected upstream hit"))
                }
            }),
        );

    let mut snapshot =
        sample_currently_usable_auth_snapshot("key-openai-model-123", "user-openai-model-123");
    snapshot.api_key_allowed_models = Some(vec!["gpt-4.1".to_string()]);
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-openai-model-guard-123")),
        snapshot,
    )]));
    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-openai-model-guard-123",
        )
        .header(TRACE_ID_HEADER, "trace-control-openai-model-guard-1")
        .body("{\"model\":\"gpt-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "当前用户、用户组或密钥的访问控制策略不允许访问模型 gpt-5"
    );
    assert_eq!(*auth_context_hits.lock().expect("mutex should lock"), 0);
    assert_eq!(*public_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_locally_denies_disallowed_claude_model_with_anthropic_permission_error() {
    let mut snapshot =
        sample_currently_usable_auth_snapshot("key-claude-model-123", "user-claude-model-123");
    snapshot.api_key_allowed_providers = Some(vec!["claude".to_string()]);
    snapshot.user_allowed_providers = Some(vec!["claude".to_string()]);
    snapshot.api_key_allowed_api_formats = Some(vec!["claude:messages".to_string()]);
    snapshot.user_allowed_api_formats = Some(vec!["claude:messages".to_string()]);
    snapshot.api_key_allowed_models = Some(vec!["claude-haiku-4-5".to_string()]);
    let repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-claude-model-guard-123")),
        snapshot,
    )]));
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_auth_api_key_data_reader_for_tests(repository),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header("x-api-key", "sk-claude-model-guard-123")
        .header(TRACE_ID_HEADER, "trace-control-claude-model-guard-1")
        .body("{\"model\":\"claude-sonnet-4-5\",\"messages\":[]}")
        .send()
        .await
        .expect("request should complete locally");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    let payload: serde_json::Value = response.json().await.expect("response json should parse");
    assert_eq!(payload["type"], "error");
    assert_eq!(payload["error"]["type"], "permission_error");
    assert_eq!(
        payload["error"]["message"],
        "当前用户、用户组或密钥的访问控制策略不允许访问模型 claude-sonnet-4-5"
    );

    gateway_handle.abort();
}
