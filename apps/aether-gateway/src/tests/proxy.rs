use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{
    any, build_router, build_router_with_state, json, send_request, start_server, to_bytes,
    AppState, Arc, Body, HeaderValue, Json, Mutex, Request, Response, Router, StatusCode,
    DEPENDENCY_REASON_HEADER, EXECUTION_PATH_HEADER, EXECUTION_PATH_LOCAL_AUTH_DENIED,
    EXECUTION_PATH_LOCAL_EXECUTION_LOOP_DETECTED, EXECUTION_PATH_LOCAL_ROUTE_NOT_FOUND,
    EXECUTION_RUNTIME_LOOP_GUARD_HEADER, EXECUTION_RUNTIME_LOOP_GUARD_VALUE, FORWARDED_FOR_HEADER,
    GATEWAY_HEADER, TRACE_ID_HEADER, TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
    TRUSTED_AUTH_API_KEY_ID_HEADER, TRUSTED_AUTH_USER_ID_HEADER,
    TUNNEL_AFFINITY_FORWARDED_BY_HEADER, TUNNEL_AFFINITY_NODE_ID_HEADER,
    TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
};

use aether_contracts::tunnel::{
    sign_tunnel_relay_request, tunnel_relay_payload_digest, TUNNEL_RELAY_AUTH_NONCE_HEADER,
    TUNNEL_RELAY_AUTH_PAYLOAD_HEADER, TUNNEL_RELAY_AUTH_SENDER_HEADER,
    TUNNEL_RELAY_AUTH_SIGNATURE_HEADER, TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
    TUNNEL_RELAY_FORWARDED_BY_HEADER, TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::proxy_nodes::{InMemoryProxyNodeRepository, StoredProxyNode};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use aether_runtime_state::{RedisClientConfig, RuntimeState};
use aether_scheduler_core::{
    build_scheduler_affinity_cache_key_for_api_key_id_with_client_session_and_scope,
    SchedulerAffinityScope,
};
use aether_test_support::ManagedRedisServer;
use sha2::{Digest, Sha256};

const RELAY_TEST_SECRET: &str = "relay-test-secret-at-least-32-bytes";
const AFFINITY_TEST_SENDER: &str = "gateway-a";
const AFFINITY_TEST_OWNER: &str = "gateway-b";
const AFFINITY_TEST_NODE: &str = "node-owner";

fn hash_api_key(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn system_default_affinity_cache_key(api_key_id: &str, api_format: &str, model: &str) -> String {
    let scope = SchedulerAffinityScope::new("system-default", Some(1));
    build_scheduler_affinity_cache_key_for_api_key_id_with_client_session_and_scope(
        api_key_id,
        api_format,
        model,
        None,
        Some(&scope),
    )
    .expect("system-default affinity cache key should build")
}

fn sample_auth_snapshot(
    api_key_id: &str,
    user_id: &str,
    allowed_model: &str,
) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!([allowed_model])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:chat"])),
        Some(serde_json::json!([allowed_model])),
    )
    .expect("auth snapshot should build")
}

fn sample_cli_auth_snapshot(
    api_key_id: &str,
    user_id: &str,
    allowed_model: &str,
) -> StoredAuthApiKeySnapshot {
    StoredAuthApiKeySnapshot::new(
        user_id.to_string(),
        "alice".to_string(),
        Some("alice@example.com".to_string()),
        "user".to_string(),
        "local".to_string(),
        true,
        false,
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:responses"])),
        Some(serde_json::json!([allowed_model])),
        api_key_id.to_string(),
        Some("default".to_string()),
        true,
        false,
        false,
        Some(60),
        Some(5),
        Some(4_102_444_800),
        Some(serde_json::json!(["openai"])),
        Some(serde_json::json!(["openai:responses"])),
        Some(serde_json::json!([allowed_model])),
    )
    .expect("auth snapshot should build")
}

fn sample_provider(provider_id: &str) -> StoredProviderCatalogProvider {
    sample_provider_with_request_timeout(provider_id, None)
}

fn sample_provider_with_request_timeout(
    provider_id: &str,
    request_timeout_secs: Option<f64>,
) -> StoredProviderCatalogProvider {
    StoredProviderCatalogProvider::new(
        provider_id.to_string(),
        provider_id.to_string(),
        Some("https://provider.example".to_string()),
        "custom".to_string(),
    )
    .expect("provider should build")
    .with_transport_fields(
        true,
        false,
        false,
        None,
        None,
        None,
        request_timeout_secs,
        None,
        None,
    )
}

fn sample_endpoint(endpoint_id: &str, provider_id: &str) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        endpoint_id.to_string(),
        provider_id.to_string(),
        "openai:chat".to_string(),
        Some("openai".to_string()),
        Some("chat".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        "https://api.provider.example".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("endpoint transport should build")
}

fn sample_codex_endpoint(endpoint_id: &str, provider_id: &str) -> StoredProviderCatalogEndpoint {
    StoredProviderCatalogEndpoint::new(
        endpoint_id.to_string(),
        provider_id.to_string(),
        "openai:responses".to_string(),
        Some("openai".to_string()),
        Some("cli".to_string()),
        true,
    )
    .expect("endpoint should build")
    .with_transport_fields(
        "https://chatgpt.com/backend-api/codex".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("endpoint transport should build")
}

fn sample_key(key_id: &str, provider_id: &str, node_id: &str) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        key_id.to_string(),
        provider_id.to_string(),
        "default".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!(["openai:chat"])),
        "plain-upstream-key".to_string(),
        None,
        None,
        Some(json!({"openai:chat": 1})),
        None,
        None,
        Some(json!({
            "enabled": true,
            "mode": "tunnel",
            "node_id": node_id,
        })),
        None,
    )
    .expect("key transport should build")
}

fn sample_bound_key(key_id: &str, provider_id: &str, node_id: &str) -> StoredProviderCatalogKey {
    let bootstrap = AppState::new()
        .expect("bootstrap state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::disabled()
                .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY),
        );
    let mut key = sample_key(key_id, provider_id, node_id);
    key.encrypted_api_key = Some(
        bootstrap
            .seal_provider_catalog_key_api_key(provider_id, key_id, "plain-upstream-key")
            .expect("bound provider api key ciphertext should build"),
    );
    key
}

fn sample_codex_key(key_id: &str, provider_id: &str, node_id: &str) -> StoredProviderCatalogKey {
    StoredProviderCatalogKey::new(
        key_id.to_string(),
        provider_id.to_string(),
        "default".to_string(),
        "api_key".to_string(),
        None,
        true,
    )
    .expect("key should build")
    .with_transport_fields(
        Some(json!(["openai:responses"])),
        "plain-upstream-key".to_string(),
        None,
        None,
        Some(json!({"openai:responses": 1})),
        None,
        None,
        Some(json!({
            "enabled": true,
            "mode": "tunnel",
            "node_id": node_id,
        })),
        None,
    )
    .expect("key transport should build")
}

fn sample_bound_codex_key(
    key_id: &str,
    provider_id: &str,
    node_id: &str,
) -> StoredProviderCatalogKey {
    let bootstrap = AppState::new()
        .expect("bootstrap state should build")
        .with_data_state_for_tests(
            crate::data::GatewayDataState::disabled()
                .with_encryption_key_for_tests(aether_crypto::DEVELOPMENT_ENCRYPTION_KEY),
        );
    let mut key = sample_codex_key(key_id, provider_id, node_id);
    key.encrypted_api_key = Some(
        bootstrap
            .seal_provider_catalog_key_api_key(provider_id, key_id, "plain-upstream-key")
            .expect("bound provider api key ciphertext should build"),
    );
    key
}

fn tunnel_attachment_key(node_id: &str) -> String {
    format!("tunnel.attachments.{node_id}")
}

fn sample_tunnel_proxy_node(node_id: &str, tunnel_generation: &str) -> StoredProxyNode {
    StoredProxyNode::new(
        node_id.to_string(),
        format!("proxy-{node_id}"),
        "127.0.0.1".to_string(),
        1,
        false,
        "online".to_string(),
        30,
        0,
        0,
        0,
        0,
        0,
        true,
        true,
        1,
    )
    .expect("tunnel proxy node should build")
    .with_tunnel_generation(tunnel_generation.to_string())
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn signed_affinity_headers(
    method: &http::Method,
    uri: &http::Uri,
    nonce: &str,
    user_id: &str,
    api_key_id: &str,
    body: &[u8],
) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        GATEWAY_HEADER,
        HeaderValue::from_static("rust-phase3b-affinity"),
    );
    headers.insert(
        TUNNEL_RELAY_FORWARDED_BY_HEADER,
        HeaderValue::from_static(AFFINITY_TEST_SENDER),
    );
    headers.insert(
        TUNNEL_AFFINITY_FORWARDED_BY_HEADER,
        HeaderValue::from_static(AFFINITY_TEST_SENDER),
    );
    headers.insert(
        TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
        HeaderValue::from_static(AFFINITY_TEST_OWNER),
    );
    headers.insert(
        TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER,
        HeaderValue::from_static(AFFINITY_TEST_OWNER),
    );
    headers.insert(
        TUNNEL_AFFINITY_NODE_ID_HEADER,
        HeaderValue::from_static(AFFINITY_TEST_NODE),
    );
    headers.insert(
        TRUSTED_AUTH_USER_ID_HEADER,
        HeaderValue::from_str(user_id).expect("trusted user id should be a valid header"),
    );
    headers.insert(
        TRUSTED_AUTH_API_KEY_ID_HEADER,
        HeaderValue::from_str(api_key_id).expect("trusted API key id should be a valid header"),
    );
    headers.insert(
        TRUSTED_AUTH_ACCESS_ALLOWED_HEADER,
        HeaderValue::from_static("true"),
    );
    headers.insert(
        FORWARDED_FOR_HEADER,
        HeaderValue::from_static("203.0.113.10"),
    );

    let timestamp = current_unix_secs();
    let metadata = crate::tunnel::build_tunnel_affinity_auth_metadata(method, uri, &headers)
        .expect("affinity authentication metadata should build");
    let payload_digest = tunnel_relay_payload_digest(&metadata, body);
    let signature = sign_tunnel_relay_request(
        RELAY_TEST_SECRET.as_bytes(),
        AFFINITY_TEST_SENDER,
        AFFINITY_TEST_OWNER,
        AFFINITY_TEST_NODE,
        AFFINITY_TEST_SENDER,
        false,
        timestamp,
        nonce,
        &payload_digest,
    );
    headers.insert(
        TUNNEL_RELAY_AUTH_SENDER_HEADER,
        HeaderValue::from_static(AFFINITY_TEST_SENDER),
    );
    headers.insert(
        TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER,
        HeaderValue::from_str(&timestamp.to_string()).expect("timestamp should be a valid header"),
    );
    headers.insert(
        TUNNEL_RELAY_AUTH_NONCE_HEADER,
        HeaderValue::from_str(nonce).expect("nonce should be a valid header"),
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

fn affinity_request(uri: http::Uri, headers: http::HeaderMap, body: &'static str) -> Request {
    let mut request = Request::builder()
        .method(http::Method::POST)
        .uri(uri)
        .body(Body::from(body))
        .expect("affinity request should build");
    *request.headers_mut() = headers;
    request.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    request
}

#[tokio::test]
async fn gateway_rejects_incomplete_tunnel_affinity_trusted_auth_headers() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_tunnel_identity_and_relay_secret_for_tests(
                AFFINITY_TEST_OWNER,
                None,
                RELAY_TEST_SECRET,
            ),
    );
    let uri: http::Uri = "/v1/chat/completions"
        .parse()
        .expect("affinity URI should parse");
    let mut headers = http::HeaderMap::new();
    headers.insert(
        TRUSTED_AUTH_USER_ID_HEADER,
        HeaderValue::from_static("forged-user"),
    );

    let response = send_request(
        gateway,
        affinity_request(uri, headers, r#"{"model":"gpt-5","messages":[]}"#),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_AUTH_DENIED)
    );
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read"),
    )
    .expect("response body should parse");
    assert_eq!(
        payload["error"]["message"],
        "invalid tunnel affinity authentication"
    );
}

#[tokio::test]
async fn gateway_uses_signed_tunnel_affinity_identity_once_and_rejects_replay() {
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        None,
        sample_auth_snapshot("affinity-key", "affinity-user", "gpt-4.1"),
    )]));
    let data_state =
        crate::data::GatewayDataState::with_auth_api_key_reader_for_tests(auth_repository.clone());
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_and_relay_secret_for_tests(
                AFFINITY_TEST_OWNER,
                None,
                RELAY_TEST_SECRET,
            ),
    );
    let uri: http::Uri = "/v1/chat/completions?stream=false"
        .parse()
        .expect("affinity URI should parse");
    let body = r#"{"model":"gpt-5","messages":[]}"#;
    let headers = signed_affinity_headers(
        &http::Method::POST,
        &uri,
        "affinity-valid-once",
        "affinity-user",
        "affinity-key",
        body.as_bytes(),
    );

    let first = send_request(
        gateway.clone(),
        affinity_request(uri.clone(), headers.clone(), body),
    )
    .await;
    assert_eq!(first.status(), StatusCode::FORBIDDEN);
    let first_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(first.into_body(), usize::MAX)
            .await
            .expect("first response body should read"),
    )
    .expect("first response body should parse");
    assert_eq!(first_payload["error"]["type"], "http_error");
    assert!(first_payload["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("gpt-5")));
    assert_eq!(auth_repository.snapshot_lookup_count("affinity-key"), 1);

    let replay = send_request(gateway, affinity_request(uri, headers, body)).await;
    assert_eq!(replay.status(), StatusCode::FORBIDDEN);
    let replay_payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(replay.into_body(), usize::MAX)
            .await
            .expect("replay response body should read"),
    )
    .expect("replay response body should parse");
    assert_eq!(
        replay_payload["error"]["message"],
        "invalid tunnel affinity authentication"
    );
    assert_eq!(auth_repository.snapshot_lookup_count("affinity-key"), 1);
}

#[tokio::test]
async fn gateway_rejects_tunnel_affinity_path_and_trusted_identity_tampering() {
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_tunnel_identity_and_relay_secret_for_tests(
                AFFINITY_TEST_OWNER,
                None,
                RELAY_TEST_SECRET,
            ),
    );
    let signed_uri: http::Uri = "/v1/chat/completions?stream=false"
        .parse()
        .expect("signed affinity URI should parse");
    let tampered_uri: http::Uri = "/v1/chat/completions?stream=true"
        .parse()
        .expect("tampered affinity URI should parse");
    let body = r#"{"model":"gpt-5","messages":[]}"#;

    let path_headers = signed_affinity_headers(
        &http::Method::POST,
        &signed_uri,
        "affinity-path-tamper",
        "affinity-user",
        "affinity-key",
        body.as_bytes(),
    );
    let path_response = send_request(
        gateway.clone(),
        affinity_request(tampered_uri, path_headers, body),
    )
    .await;
    assert_eq!(path_response.status(), StatusCode::FORBIDDEN);

    let mut identity_headers = signed_affinity_headers(
        &http::Method::POST,
        &signed_uri,
        "affinity-identity-tamper",
        "affinity-user",
        "affinity-key",
        body.as_bytes(),
    );
    identity_headers.insert(
        TRUSTED_AUTH_USER_ID_HEADER,
        HeaderValue::from_static("forged-user"),
    );
    let identity_response = send_request(
        gateway,
        affinity_request(signed_uri, identity_headers, body),
    )
    .await;
    assert_eq!(identity_response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn gateway_fails_closed_when_shared_tunnel_affinity_nonce_state_is_unavailable() {
    let mut redis = match ManagedRedisServer::start().await {
        Ok(redis) => redis,
        Err(error) if error.to_string().contains("No such file or directory") => {
            eprintln!("skipping affinity Redis outage test: {error}");
            return;
        }
        Err(error) => panic!("Redis test server should start: {error}"),
    };
    let runtime_state = Arc::new(
        RuntimeState::redis(
            RedisClientConfig {
                url: redis.redis_url().to_string(),
                key_prefix: Some(format!("affinity-outage-{}", std::process::id())),
            },
            Some(250),
        )
        .await
        .expect("Redis runtime state should build"),
    );
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway state should build")
            .with_tunnel_identity_runtime_state_and_relay_secret_for_tests(
                AFFINITY_TEST_OWNER,
                None,
                runtime_state,
                RELAY_TEST_SECRET,
            ),
    );
    let uri: http::Uri = "/v1/chat/completions"
        .parse()
        .expect("affinity URI should parse");
    let body = r#"{"model":"gpt-5","messages":[]}"#;
    let headers = signed_affinity_headers(
        &http::Method::POST,
        &uri,
        "affinity-runtime-unavailable",
        "affinity-user",
        "affinity-key",
        body.as_bytes(),
    );
    redis.stop().expect("Redis test server should stop");

    let response = send_request(gateway, affinity_request(uri, headers, body)).await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read"),
    )
    .expect("response body should parse");
    assert_eq!(
        payload["error"]["message"],
        "tunnel affinity authentication is unavailable"
    );
}

#[tokio::test]
async fn gateway_rejects_unknown_path_locally_and_generates_trace_id() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::CREATED, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!("{gateway_url}/does/not/exist?stream=true"))
        .header(http::header::HOST, "api.example.com")
        .header(DEPENDENCY_REASON_HEADER, "forged")
        .body("{\"hello\":\"world\"}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get(GATEWAY_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("rust-phase3b")
    );
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_ROUTE_NOT_FOUND)
    );
    assert_eq!(
        response
            .headers()
            .get(DEPENDENCY_REASON_HEADER)
            .and_then(|value| value.to_str().ok()),
        None
    );

    let response_trace_id = response
        .headers()
        .get(TRACE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .expect("response trace id should exist")
        .to_string();
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(payload["error"]["message"], "Route not found");
    assert!(!response_trace_id.is_empty());
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_preserves_existing_trace_id_on_unknown_local_not_found() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/streaming-proxy"))
        .header(TRACE_ID_HEADER, "trace-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get(TRACE_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("trace-123")
    );
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_ROUTE_NOT_FOUND)
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(payload["error"]["message"], "Route not found");
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_execution_runtime_loop_guarded_ai_request() {
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .header(TRACE_ID_HEADER, "trace-loop-guard-123")
        .header(
            EXECUTION_RUNTIME_LOOP_GUARD_HEADER,
            EXECUTION_RUNTIME_LOOP_GUARD_VALUE,
        )
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(r#"{"model":"gpt-5.4","input":"hello"}"#)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::LOOP_DETECTED);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_EXECUTION_LOOP_DETECTED)
    );
    let payload: serde_json::Value = response.json().await.expect("body should parse");
    assert_eq!(payload["error"]["type"], "http_error");
    assert_eq!(
        payload["error"]["message"],
        "Gateway detected an execution runtime request loop back into the local frontdoor"
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_shapes_execution_loop_rejections_for_claude_routes() {
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    for path in ["/v1/messages", "/v1/messages/count_tokens"] {
        let response = reqwest::Client::new()
            .post(format!("{gateway_url}{path}"))
            .header(
                EXECUTION_RUNTIME_LOOP_GUARD_HEADER,
                EXECUTION_RUNTIME_LOOP_GUARD_VALUE,
            )
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(r#"{"model":"claude-sonnet-4","messages":[]}"#)
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(response.status(), StatusCode::LOOP_DETECTED, "path: {path}");
        assert_eq!(
            response
                .headers()
                .get(EXECUTION_PATH_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(EXECUTION_PATH_LOCAL_EXECUTION_LOOP_DETECTED),
            "path: {path}"
        );
        let payload: serde_json::Value = response.json().await.expect("body should parse");
        assert_eq!(payload["type"], "error", "path: {path}");
        assert_eq!(payload["error"]["type"], "api_error", "path: {path}");
        assert_eq!(
            payload["error"]["message"],
            "Gateway detected an execution runtime request loop back into the local frontdoor",
            "path: {path}"
        );
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_shapes_wrong_method_rejections_for_claude_routes() {
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    for path in ["/v1/messages", "/v1/messages/count_tokens"] {
        let response = reqwest::Client::new()
            .get(format!("{gateway_url}{path}"))
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "path: {path}"
        );
        assert_eq!(
            response
                .headers()
                .get(http::header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some("POST"),
            "path: {path}"
        );
        let payload: serde_json::Value = response.json().await.expect("body should parse");
        assert_eq!(payload["type"], "error", "path: {path}");
        assert_eq!(
            payload["error"]["type"], "invalid_request_error",
            "path: {path}"
        );
        assert_eq!(
            payload["error"]["message"], "Method not allowed",
            "path: {path}"
        );
    }

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_execution_runtime_via_guarded_ai_request() {
    let gateway = build_router().expect("gateway should build");
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/messages"))
        .header(TRACE_ID_HEADER, "trace-loop-via-123")
        .header("via", "1.1 aether-execution-runtime")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(r#"{"model":"claude-sonnet-4","messages":[{"role":"user","content":"hello"}]}"#)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::LOOP_DETECTED);
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(EXECUTION_PATH_LOCAL_EXECUTION_LOOP_DETECTED)
    );

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_forwards_public_request_to_remote_tunnel_owner_before_fallback_probe() {
    #[derive(Debug, Clone)]
    struct SeenOwnerRequest {
        path: String,
        body: String,
        trace_id: String,
        gateway_marker: String,
        authorization: String,
        trusted_user_id: String,
        trusted_api_key_id: String,
        trusted_access_allowed: String,
        forwarded_for: String,
        forwarded_by: String,
        owner_instance_id: String,
        relay_sender: String,
        relay_owner: String,
        relay_timestamp: String,
        relay_nonce: String,
        relay_signature: String,
        cookie: String,
        cookie2: String,
    }

    let fallback_probe_hits = Arc::new(Mutex::new(0usize));
    let fallback_probe_hits_clone = Arc::clone(&fallback_probe_hits);
    let fallback_probe = Router::new().route(
        "/{*path}",
        any(move |_request: Request| {
            let fallback_probe_hits_inner = Arc::clone(&fallback_probe_hits_clone);
            async move {
                *fallback_probe_hits_inner.lock().expect("mutex should lock") += 1;
                (
                    StatusCode::OK,
                    Body::from("fallback-probe-should-not-be-hit"),
                )
            }
        }),
    );

    let seen_owner = Arc::new(Mutex::new(None::<SeenOwnerRequest>));
    let seen_owner_clone = Arc::clone(&seen_owner);
    let owner = Router::new().route(
        "/v1/chat/completions",
        any(move |request: Request| {
            let seen_owner_inner = Arc::clone(&seen_owner_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                tokio::time::sleep(Duration::from_millis(40)).await;
                *seen_owner_inner.lock().expect("mutex should lock") = Some(SeenOwnerRequest {
                    path: parts
                        .uri
                        .path_and_query()
                        .map(|value| value.as_str())
                        .unwrap_or("/")
                        .to_string(),
                    body: String::from_utf8(raw_body.to_vec()).expect("utf-8 body"),
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    gateway_marker: parts
                        .headers
                        .get(GATEWAY_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    authorization: parts
                        .headers
                        .get(http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_user_id: parts
                        .headers
                        .get(TRUSTED_AUTH_USER_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_api_key_id: parts
                        .headers
                        .get(TRUSTED_AUTH_API_KEY_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_access_allowed: parts
                        .headers
                        .get(TRUSTED_AUTH_ACCESS_ALLOWED_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    forwarded_for: parts
                        .headers
                        .get(FORWARDED_FOR_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    forwarded_by: parts
                        .headers
                        .get(TUNNEL_AFFINITY_FORWARDED_BY_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    owner_instance_id: parts
                        .headers
                        .get(TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    relay_sender: parts
                        .headers
                        .get(TUNNEL_RELAY_AUTH_SENDER_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    relay_owner: parts
                        .headers
                        .get(TUNNEL_RELAY_OWNER_INSTANCE_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    relay_timestamp: parts
                        .headers
                        .get(TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    relay_nonce: parts
                        .headers
                        .get(TUNNEL_RELAY_AUTH_NONCE_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    relay_signature: parts
                        .headers
                        .get(TUNNEL_RELAY_AUTH_SIGNATURE_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    cookie: parts
                        .headers
                        .get(http::header::COOKIE)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    cookie2: parts
                        .headers
                        .get("cookie2")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                });
                (
                    StatusCode::OK,
                    [(GATEWAY_HEADER, "gateway-b-owner")],
                    Body::from("owner-gateway-response"),
                )
            }
        }),
    );

    let (_unused_fallback_probe_url, fallback_probe_handle) = start_server(fallback_probe).await;
    let (owner_url, owner_handle) = start_server(owner).await;

    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider_with_request_timeout(
            "provider-owner",
            Some(2.0),
        )],
        vec![sample_endpoint("endpoint-owner", "provider-owner")],
        vec![sample_bound_key(
            "key-owner",
            "provider-owner",
            "node-owner",
        )],
    ));
    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-affinity")),
        sample_auth_snapshot("api-key-affinity-1", "user-affinity-1", "gpt-4.1"),
    )]));
    let observed_at_unix_secs = current_unix_secs();
    let data_state = crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
        provider_catalog_repository,
        aether_crypto::DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_auth_api_key_reader(auth_repository)
    .attach_proxy_node_repository_for_tests(Arc::new(InMemoryProxyNodeRepository::seed([
        sample_tunnel_proxy_node("node-owner", "test-generation-owner"),
    ])))
    .with_system_config_values_for_tests(vec![(
        tunnel_attachment_key("node-owner"),
        serde_json::to_value(crate::tunnel::TunnelAttachmentRecord {
            gateway_instance_id: "gateway-b".to_string(),
            relay_base_url: owner_url.clone(),
            tunnel_generation: "test-generation-owner".to_string(),
            conn_count: 1,
            observed_at_unix_secs,
        })
        .expect("attachment should serialize"),
    )])
    .with_system_default_routing_group_for_tests();

    let mut state = AppState::new().expect("gateway state should build");
    state = state
        .with_data_state_for_tests(data_state)
        .with_tunnel_identity_and_relay_secret_for_tests(
            "gateway-a",
            Some("http://gateway-a:8080"),
            RELAY_TEST_SECRET,
        );
    let short_timeout_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(10))
        .build()
        .expect("test client should build");
    state.client = short_timeout_client.clone();
    let affinity_cache_key =
        system_default_affinity_cache_key("api-key-affinity-1", "openai:chat", "gpt-4.1");
    state.remember_scheduler_affinity_target(
        &affinity_cache_key,
        crate::cache::SchedulerAffinityTarget {
            provider_id: "provider-owner".to_string(),
            endpoint_id: "endpoint-owner".to_string(),
            key_id: "key-owner".to_string(),
        },
        Duration::from_secs(300),
        100,
    );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/chat/completions?stream=false"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-affinity",
        )
        .header(TRACE_ID_HEADER, "trace-tunnel-affinity-forward-1")
        .header(http::header::COOKIE, "session=must-not-forward")
        .header("cookie2", "legacy-session=must-not-forward")
        .body("{\"model\":\"gpt-4.1\",\"messages\":[]}")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(GATEWAY_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("rust-phase3b")
    );
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("tunnel_affinity_forward")
    );
    assert_eq!(
        response
            .headers()
            .get(TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("gateway-b")
    );
    assert_eq!(
        response.text().await.expect("body should read"),
        "owner-gateway-response"
    );

    assert_eq!(*fallback_probe_hits.lock().expect("mutex should lock"), 0);
    let owner_request = seen_owner
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("owner request should be captured");
    assert_eq!(owner_request.path, "/v1/chat/completions?stream=false");
    assert_eq!(
        owner_request.body,
        "{\"model\":\"gpt-4.1\",\"messages\":[]}"
    );
    assert_eq!(owner_request.trace_id, "trace-tunnel-affinity-forward-1");
    assert_eq!(owner_request.gateway_marker, "rust-phase3b-affinity");
    assert_eq!(owner_request.authorization, "");
    assert_eq!(owner_request.trusted_user_id, "user-affinity-1");
    assert_eq!(owner_request.trusted_api_key_id, "api-key-affinity-1");
    assert_eq!(owner_request.trusted_access_allowed, "true");
    assert_eq!(owner_request.forwarded_for, "127.0.0.1");
    assert_eq!(owner_request.forwarded_by, "gateway-a");
    assert_eq!(owner_request.owner_instance_id, "gateway-b");
    assert_eq!(owner_request.relay_sender, "gateway-a");
    assert_eq!(owner_request.relay_owner, "gateway-b");
    assert!(!owner_request.relay_timestamp.is_empty());
    assert!(!owner_request.relay_nonce.is_empty());
    assert!(!owner_request.relay_signature.is_empty());
    assert_eq!(owner_request.cookie, "");
    assert_eq!(owner_request.cookie2, "");

    gateway_handle.abort();
    owner_handle.abort();
    fallback_probe_handle.abort();
}

#[tokio::test]
async fn owner_forward_client_does_not_follow_redirects_with_relay_credentials() {
    let redirected_hits = Arc::new(Mutex::new(0usize));
    let redirected_hits_clone = Arc::clone(&redirected_hits);
    let redirected_target = Router::new().route(
        "/captured",
        any(move |_request: Request| {
            let redirected_hits_inner = Arc::clone(&redirected_hits_clone);
            async move {
                *redirected_hits_inner.lock().expect("mutex should lock") += 1;
                StatusCode::OK
            }
        }),
    );
    let (redirected_url, redirected_handle) = start_server(redirected_target).await;

    let redirect_location = format!("{redirected_url}/captured");
    let redirect_source = Router::new().route(
        "/relay",
        any(move || {
            let location = redirect_location.clone();
            async move {
                Response::builder()
                    .status(StatusCode::TEMPORARY_REDIRECT)
                    .header(http::header::LOCATION, location)
                    .body(Body::empty())
                    .expect("redirect response should build")
            }
        }),
    );
    let (redirect_url, redirect_handle) = start_server(redirect_source).await;

    let state = AppState::new().expect("gateway state should build");
    let response = state
        .owner_forward_client
        .post(format!("{redirect_url}/relay"))
        .header(TUNNEL_RELAY_AUTH_SIGNATURE_HEADER, "sensitive-signature")
        .body("sensitive-relay-envelope")
        .send()
        .await
        .expect("owner forward request should return the redirect response");

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(*redirected_hits.lock().expect("mutex should lock"), 0);

    redirect_handle.abort();
    redirected_handle.abort();
}

#[tokio::test]
async fn gateway_aggregates_sync_sse_from_remote_tunnel_owner_before_returning_to_client() {
    #[derive(Debug, Clone)]
    struct SeenOwnerRequest {
        path: String,
        body: String,
        trace_id: String,
        gateway_marker: String,
        trusted_user_id: String,
        trusted_api_key_id: String,
        trusted_access_allowed: String,
        forwarded_by: String,
        owner_instance_id: String,
    }

    let seen_owner = Arc::new(Mutex::new(None::<SeenOwnerRequest>));
    let seen_owner_clone = Arc::clone(&seen_owner);
    let owner = Router::new().route(
        "/v1/responses",
        any(move |request: Request| {
            let seen_owner_inner = Arc::clone(&seen_owner_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                *seen_owner_inner.lock().expect("mutex should lock") = Some(SeenOwnerRequest {
                    path: parts
                        .uri
                        .path_and_query()
                        .map(|value| value.as_str())
                        .unwrap_or("/")
                        .to_string(),
                    body: String::from_utf8(raw_body.to_vec()).expect("utf-8 body"),
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    gateway_marker: parts
                        .headers
                        .get(GATEWAY_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_user_id: parts
                        .headers
                        .get(TRUSTED_AUTH_USER_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_api_key_id: parts
                        .headers
                        .get(TRUSTED_AUTH_API_KEY_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_access_allowed: parts
                        .headers
                        .get(TRUSTED_AUTH_ACCESS_ALLOWED_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    forwarded_by: parts
                        .headers
                        .get(TUNNEL_AFFINITY_FORWARDED_BY_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    owner_instance_id: parts
                        .headers
                        .get(TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                });
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(concat!(
                        "event: response.created\n",
                        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-codex-affinity-123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
                        "event: response.output_text.delta\n",
                        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello from Codex\"}\n\n",
                        "event: response.completed\n",
                        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-codex-affinity-123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n"
                    )))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                response.headers_mut().insert(
                    http::header::CACHE_CONTROL,
                    HeaderValue::from_static("no-cache"),
                );
                response.headers_mut().insert(
                    http::header::HeaderName::from_static(GATEWAY_HEADER),
                    HeaderValue::from_static("gateway-b-owner"),
                );
                response
            }
        }),
    );

    let (owner_url, owner_handle) = start_server(owner).await;

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-cli-affinity")),
        sample_cli_auth_snapshot("api-key-affinity-cli-1", "user-affinity-cli-1", "gpt-5.4"),
    )]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-cli-owner")],
        vec![sample_codex_endpoint(
            "endpoint-cli-owner",
            "provider-cli-owner",
        )],
        vec![sample_bound_codex_key(
            "key-cli-owner",
            "provider-cli-owner",
            "node-cli-owner",
        )],
    ));
    let observed_at_unix_secs = current_unix_secs();
    let data_state = crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
        provider_catalog_repository,
        aether_crypto::DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_auth_api_key_reader(auth_repository)
    .attach_proxy_node_repository_for_tests(Arc::new(InMemoryProxyNodeRepository::seed([
        sample_tunnel_proxy_node("node-cli-owner", "test-generation-cli-owner"),
    ])))
    .with_system_config_values_for_tests(vec![(
        tunnel_attachment_key("node-cli-owner"),
        serde_json::to_value(crate::tunnel::TunnelAttachmentRecord {
            gateway_instance_id: "gateway-b".to_string(),
            relay_base_url: owner_url.clone(),
            tunnel_generation: "test-generation-cli-owner".to_string(),
            conn_count: 1,
            observed_at_unix_secs,
        })
        .expect("attachment should serialize"),
    )])
    .with_system_default_routing_group_for_tests();

    let mut state = AppState::new().expect("gateway state should build");
    state = state
        .with_data_state_for_tests(data_state)
        .with_tunnel_identity_and_relay_secret_for_tests(
            "gateway-a",
            Some("http://gateway-a:8080"),
            RELAY_TEST_SECRET,
        );
    let affinity_cache_key =
        system_default_affinity_cache_key("api-key-affinity-cli-1", "openai:responses", "gpt-5.4");
    state.remember_scheduler_affinity_target(
        &affinity_cache_key,
        crate::cache::SchedulerAffinityTarget {
            provider_id: "provider-cli-owner".to_string(),
            endpoint_id: "endpoint-cli-owner".to_string(),
            key_id: "key-cli-owner".to_string(),
        },
        Duration::from_secs(300),
        100,
    );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-cli-affinity",
        )
        .header(TRACE_ID_HEADER, "trace-tunnel-affinity-cli-sync-1")
        .json(&json!({
            "model": "gpt-5.4",
            "input": "hello",
            "stream": false
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(GATEWAY_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("rust-phase3b")
    );
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("tunnel_affinity_forward")
    );
    assert_eq!(
        response
            .headers()
            .get(TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("gateway-b")
    );
    assert!(response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("application/json")));
    let body: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(body["id"], "resp-codex-affinity-123");
    assert_eq!(body["object"], "response");
    assert_eq!(body["status"], "completed");
    assert_eq!(body["output"][0]["content"][0]["text"], "Hello from Codex");
    assert_eq!(body["usage"]["total_tokens"], 3);

    let owner_request = seen_owner
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("owner request should be captured");
    assert_eq!(owner_request.path, "/v1/responses");
    let owner_body: serde_json::Value =
        serde_json::from_str(&owner_request.body).expect("owner body should parse");
    assert_eq!(owner_body["model"], "gpt-5.4");
    assert_eq!(owner_body["stream"], false);
    assert_eq!(owner_request.trace_id, "trace-tunnel-affinity-cli-sync-1");
    assert_eq!(owner_request.gateway_marker, "rust-phase3b-affinity");
    assert_eq!(owner_request.trusted_user_id, "user-affinity-cli-1");
    assert_eq!(owner_request.trusted_api_key_id, "api-key-affinity-cli-1");
    assert_eq!(owner_request.trusted_access_allowed, "true");
    assert_eq!(owner_request.forwarded_by, "gateway-a");
    assert_eq!(owner_request.owner_instance_id, "gateway-b");

    gateway_handle.abort();
    owner_handle.abort();
}

#[tokio::test]
async fn gateway_streamifies_sync_json_from_remote_tunnel_owner_before_returning_to_client() {
    #[derive(Debug, Clone)]
    struct SeenOwnerRequest {
        path: String,
        body: String,
        trace_id: String,
        gateway_marker: String,
        trusted_user_id: String,
        trusted_api_key_id: String,
        trusted_access_allowed: String,
        forwarded_by: String,
        owner_instance_id: String,
    }

    let seen_owner = Arc::new(Mutex::new(None::<SeenOwnerRequest>));
    let seen_owner_clone = Arc::clone(&seen_owner);
    let owner = Router::new().route(
        "/v1/responses",
        any(move |request: Request| {
            let seen_owner_inner = Arc::clone(&seen_owner_clone);
            async move {
                let (parts, body) = request.into_parts();
                let raw_body = to_bytes(body, usize::MAX).await.expect("body should read");
                *seen_owner_inner.lock().expect("mutex should lock") = Some(SeenOwnerRequest {
                    path: parts
                        .uri
                        .path_and_query()
                        .map(|value| value.as_str())
                        .unwrap_or("/")
                        .to_string(),
                    body: String::from_utf8(raw_body.to_vec()).expect("utf-8 body"),
                    trace_id: parts
                        .headers
                        .get(TRACE_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    gateway_marker: parts
                        .headers
                        .get(GATEWAY_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_user_id: parts
                        .headers
                        .get(TRUSTED_AUTH_USER_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_api_key_id: parts
                        .headers
                        .get(TRUSTED_AUTH_API_KEY_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    trusted_access_allowed: parts
                        .headers
                        .get(TRUSTED_AUTH_ACCESS_ALLOWED_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    forwarded_by: parts
                        .headers
                        .get(TUNNEL_AFFINITY_FORWARDED_BY_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                    owner_instance_id: parts
                        .headers
                        .get(TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string(),
                });
                let encoded_response = serde_json::to_vec(&json!({
                    "id": "resp-codex-affinity-stream-123",
                    "object": "response",
                    "model": "gpt-5.4",
                    "status": "completed",
                    "output": [{
                        "type": "message",
                        "id": "msg-codex-affinity-stream-123",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": "Hello from affinity sync json",
                            "annotations": []
                        }]
                    }],
                    "usage": {
                        "input_tokens": 1,
                        "output_tokens": 2,
                        "total_tokens": 3
                    }
                }))
                .expect("body should encode");
                let split_at = encoded_response.len() / 2;
                let first = axum::body::Bytes::copy_from_slice(&encoded_response[..split_at]);
                let second = axum::body::Bytes::copy_from_slice(&encoded_response[split_at..]);
                let response_body = Body::from_stream(async_stream::stream! {
                    yield Ok::<_, std::io::Error>(first);
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    yield Ok::<_, std::io::Error>(second);
                });
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .body(response_body)
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response.headers_mut().insert(
                    http::header::HeaderName::from_static(GATEWAY_HEADER),
                    HeaderValue::from_static("gateway-b-owner"),
                );
                response
            }
        }),
    );

    let (owner_url, owner_handle) = start_server(owner).await;

    let auth_repository = Arc::new(InMemoryAuthApiKeySnapshotRepository::seed(vec![(
        Some(hash_api_key("sk-client-openai-cli-affinity")),
        sample_cli_auth_snapshot("api-key-affinity-cli-1", "user-affinity-cli-1", "gpt-5.4"),
    )]));
    let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
        vec![sample_provider("provider-cli-owner")],
        vec![sample_codex_endpoint(
            "endpoint-cli-owner",
            "provider-cli-owner",
        )],
        vec![sample_bound_codex_key(
            "key-cli-owner",
            "provider-cli-owner",
            "node-cli-owner",
        )],
    ));
    let observed_at_unix_secs = current_unix_secs();
    let data_state = crate::data::GatewayDataState::with_provider_transport_reader_for_tests(
        provider_catalog_repository,
        aether_crypto::DEVELOPMENT_ENCRYPTION_KEY,
    )
    .with_auth_api_key_reader(auth_repository)
    .attach_proxy_node_repository_for_tests(Arc::new(InMemoryProxyNodeRepository::seed([
        sample_tunnel_proxy_node("node-cli-owner", "test-generation-cli-owner"),
    ])))
    .with_system_config_values_for_tests(vec![(
        tunnel_attachment_key("node-cli-owner"),
        serde_json::to_value(crate::tunnel::TunnelAttachmentRecord {
            gateway_instance_id: "gateway-b".to_string(),
            relay_base_url: owner_url.clone(),
            tunnel_generation: "test-generation-cli-owner".to_string(),
            conn_count: 1,
            observed_at_unix_secs,
        })
        .expect("attachment should serialize"),
    )])
    .with_system_default_routing_group_for_tests();

    let mut state = AppState::new().expect("gateway state should build");
    state = state
        .with_data_state_for_tests(data_state)
        .with_tunnel_identity_and_relay_secret_for_tests(
            "gateway-a",
            Some("http://gateway-a:8080"),
            RELAY_TEST_SECRET,
        );
    state.client = reqwest::Client::builder()
        .timeout(Duration::from_millis(10))
        .build()
        .expect("short shared client should build");
    let affinity_cache_key =
        system_default_affinity_cache_key("api-key-affinity-cli-1", "openai:responses", "gpt-5.4");
    state.remember_scheduler_affinity_target(
        &affinity_cache_key,
        crate::cache::SchedulerAffinityTarget {
            provider_id: "provider-cli-owner".to_string(),
            endpoint_id: "endpoint-cli-owner".to_string(),
            key_id: "key-cli-owner".to_string(),
        },
        Duration::from_secs(300),
        100,
    );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/v1/responses"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::AUTHORIZATION,
            "Bearer sk-client-openai-cli-affinity",
        )
        .header(TRACE_ID_HEADER, "trace-tunnel-affinity-cli-stream-1")
        .json(&json!({
            "model": "gpt-5.4",
            "input": "hello",
            "stream": true
        }))
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(GATEWAY_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("rust-phase3b")
    );
    assert_eq!(
        response
            .headers()
            .get(EXECUTION_PATH_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("tunnel_affinity_forward")
    );
    assert_eq!(
        response
            .headers()
            .get(TUNNEL_AFFINITY_OWNER_INSTANCE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("gateway-b")
    );
    assert_eq!(
        response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let body = response.text().await.expect("body should read");
    assert!(body.contains("event: response.output_text.delta"));
    assert!(body.contains("Hello from affinity sync json"));
    assert!(body.contains("event: response.completed"));

    let owner_request = seen_owner
        .lock()
        .expect("mutex should lock")
        .clone()
        .expect("owner request should be captured");
    assert_eq!(owner_request.path, "/v1/responses");
    let owner_body: serde_json::Value =
        serde_json::from_str(&owner_request.body).expect("owner body should parse");
    assert_eq!(owner_body["model"], "gpt-5.4");
    assert_eq!(owner_body["stream"], true);
    assert_eq!(owner_request.trace_id, "trace-tunnel-affinity-cli-stream-1");
    assert_eq!(owner_request.gateway_marker, "rust-phase3b-affinity");
    assert_eq!(owner_request.trusted_user_id, "user-affinity-cli-1");
    assert_eq!(owner_request.trusted_api_key_id, "api-key-affinity-cli-1");
    assert_eq!(owner_request.trusted_access_allowed, "true");
    assert_eq!(owner_request.forwarded_by, "gateway-a");
    assert_eq!(owner_request.owner_instance_id, "gateway-b");

    gateway_handle.abort();
    owner_handle.abort();
}
