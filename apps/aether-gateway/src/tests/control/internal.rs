use std::io;
use std::sync::{Arc, Mutex};

use aether_contracts::tunnel::{
    sign_tunnel_relay_request, tunnel_relay_payload_digest, RequestMeta,
    TUNNEL_RELAY_AUTH_NONCE_HEADER, TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
    TUNNEL_RELAY_AUTH_SENDER_HEADER, TUNNEL_RELAY_AUTH_SIGNATURE_HEADER,
    TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER, TUNNEL_RELAY_OWNER_INSTANCE_HEADER,
};
use aether_data::repository::proxy_nodes::ProxyNodeReadRepository;
use axum::body::Body;
use axum::routing::{any, post};
use axum::{extract::Request, Json, Router};
use bytes::Bytes;
use futures_util::stream;
use http::header::HeaderValue;
use http::StatusCode;
use serde_json::json;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RELAY_TEST_SECRET: &str = "relay-test-secret-at-least-32-bytes";
const RELAY_TEST_SENDER: &str = "execution-runtime-test";
const RELAY_TEST_NODE_ID: &str = "node-123";
const RELAY_TEST_TUNNEL_GENERATION: &str = "relay-test-generation-node-123";
const INTERNAL_GATEWAY_TEST_SECRET: &str = "internal-gateway-test-secret-at-least-32-bytes";

use super::{
    authenticated_tunnel_control_plane_request,
    authenticated_tunnel_control_plane_request_for_generation, build_router_with_state,
    hash_management_token, sample_management_token, sample_proxy_node, start_server,
    with_tunnel_control_plane_key, AppState, GatewayDataState, InMemoryManagementTokenRepository,
    InMemoryProxyNodeRepository, InMemoryUserReadRepository, TRACE_ID_HEADER,
    TUNNEL_CONTROL_PLANE_TEST_GENERATION, TUNNEL_CONTROL_PLANE_TEST_PSK,
};

const TUNNEL_HEARTBEAT_PATH: &str = "/api/internal/tunnel/heartbeat";
const TUNNEL_NODE_STATUS_PATH: &str = "/api/internal/tunnel/node-status";

fn authenticated_internal_gateway_request(
    client: &reqwest::Client,
    url: String,
    path_and_query: &str,
    body: &[u8],
    timestamp: u64,
    nonce: &str,
) -> reqwest::RequestBuilder {
    use aether_contracts::internal_gateway::{
        sign_internal_gateway_request, INTERNAL_GATEWAY_AUTH_NONCE_HEADER,
        INTERNAL_GATEWAY_AUTH_SIGNATURE_HEADER, INTERNAL_GATEWAY_AUTH_TIMESTAMP_HEADER,
    };

    let signature = sign_internal_gateway_request(
        INTERNAL_GATEWAY_TEST_SECRET.as_bytes(),
        "POST",
        path_and_query,
        timestamp,
        nonce,
        body,
    );
    client
        .post(url)
        .header("content-type", "application/json")
        .header(INTERNAL_GATEWAY_AUTH_TIMESTAMP_HEADER, timestamp)
        .header(INTERNAL_GATEWAY_AUTH_NONCE_HEADER, nonce)
        .header(INTERNAL_GATEWAY_AUTH_SIGNATURE_HEADER, signature)
        .body(body.to_vec())
}

#[tokio::test]
async fn internal_gateway_requires_hmac_even_when_peer_is_loopback_and_rejects_replay() {
    const PATH: &str = "/api/internal/gateway/resolve";
    const NONCE: &str = "internal-gateway-nonce-00000001";

    let state = AppState::new()
        .expect("gateway should build")
        .with_internal_gateway_auth_secret_for_tests(INTERNAL_GATEWAY_TEST_SECRET);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = reqwest::Client::new();
    let body = serde_json::to_vec(&json!({
        "method": "GET",
        "path": "/not-a-public-route",
        "headers": {}
    }))
    .expect("request body should encode");

    let unsigned = client
        .post(format!("{gateway_url}{PATH}"))
        .header("content-type", "application/json")
        .body(body.clone())
        .send()
        .await
        .expect("unsigned loopback request should complete");
    assert_eq!(unsigned.status(), StatusCode::FORBIDDEN);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let accepted = authenticated_internal_gateway_request(
        &client,
        format!("{gateway_url}{PATH}"),
        PATH,
        &body,
        timestamp,
        NONCE,
    )
    .send()
    .await
    .expect("signed request should complete");
    assert_eq!(accepted.status(), StatusCode::OK);

    let replay = authenticated_internal_gateway_request(
        &client,
        format!("{gateway_url}{PATH}"),
        PATH,
        &body,
        timestamp,
        NONCE,
    )
    .send()
    .await
    .expect("replayed request should complete");
    assert_eq!(replay.status(), StatusCode::FORBIDDEN);

    gateway_handle.abort();
}

#[tokio::test]
async fn internal_gateway_signature_binds_body_and_disabled_mode_is_not_discoverable() {
    const PATH: &str = "/api/internal/gateway/resolve";
    const NONCE: &str = "internal-gateway-nonce-00000002";

    let state = AppState::new()
        .expect("gateway should build")
        .with_internal_gateway_auth_secret_for_tests(INTERNAL_GATEWAY_TEST_SECRET);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let client = reqwest::Client::new();
    let signed_body = serde_json::to_vec(&json!({
        "method": "GET",
        "path": "/v1/models",
        "headers": {}
    }))
    .expect("signed body should encode");
    let tampered_body = serde_json::to_vec(&json!({
        "method": "GET",
        "path": "/api/admin/providers",
        "headers": {}
    }))
    .expect("tampered body should encode");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let signature = aether_contracts::internal_gateway::sign_internal_gateway_request(
        INTERNAL_GATEWAY_TEST_SECRET.as_bytes(),
        "POST",
        PATH,
        timestamp,
        NONCE,
        &signed_body,
    );
    let tampered = client
        .post(format!("{gateway_url}{PATH}"))
        .header("content-type", "application/json")
        .header(
            aether_contracts::internal_gateway::INTERNAL_GATEWAY_AUTH_TIMESTAMP_HEADER,
            timestamp,
        )
        .header(
            aether_contracts::internal_gateway::INTERNAL_GATEWAY_AUTH_NONCE_HEADER,
            NONCE,
        )
        .header(
            aether_contracts::internal_gateway::INTERNAL_GATEWAY_AUTH_SIGNATURE_HEADER,
            signature,
        )
        .body(tampered_body)
        .send()
        .await
        .expect("tampered request should complete");
    assert_eq!(tampered.status(), StatusCode::FORBIDDEN);
    gateway_handle.abort();

    let disabled = AppState::new()
        .expect("gateway should build")
        .without_internal_gateway_for_tests();
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(disabled)).await;
    let hidden = client
        .post(format!("{gateway_url}{PATH}"))
        .header("content-type", "application/json")
        .body(signed_body)
        .send()
        .await
        .expect("disabled request should complete");
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

    gateway_handle.abort();
}

#[tokio::test]
async fn internal_gateway_rejects_caller_supplied_user_identity() {
    const PATH: &str = "/api/internal/gateway/decision-sync";
    const NONCE: &str = "internal-gateway-nonce-identity01";

    let state = AppState::new()
        .expect("gateway should build")
        .with_internal_gateway_auth_secret_for_tests(INTERNAL_GATEWAY_TEST_SECRET);
    let (gateway_url, gateway_handle) = start_server(build_router_with_state(state)).await;
    let body = serde_json::to_vec(&json!({
        "method": "POST",
        "path": "/v1/chat/completions",
        "headers": { "content-type": "application/json" },
        "body_json": { "model": "gpt-5", "messages": [] },
        "auth_context": {
            "user_id": "enumerated-user-id",
            "api_key_id": "enumerated-api-key-id",
            "access_allowed": true
        }
    }))
    .expect("request body should encode");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let response = authenticated_internal_gateway_request(
        &reqwest::Client::new(),
        format!("{gateway_url}{PATH}"),
        PATH,
        &body,
        timestamp,
        NONCE,
    )
    .send()
    .await
    .expect("signed identity-injection request should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let payload: serde_json::Value = response.json().await.expect("error should be JSON");
    assert_eq!(
        payload["detail"],
        "supplied auth_context is not accepted; authenticate through request headers"
    );

    gateway_handle.abort();
}

fn relay_request_meta(
    stream: bool,
    request_timeout_ms: Option<u64>,
    stream_first_byte_timeout_ms: Option<u64>,
) -> RequestMeta {
    RequestMeta {
        provider_id: Some("provider-1".to_string()),
        endpoint_id: Some("endpoint-1".to_string()),
        key_id: Some("key-1".to_string()),
        method: "POST".to_string(),
        url: "https://example.com/responses".to_string(),
        headers: HashMap::new(),
        stream,
        request_timeout_ms,
        stream_first_byte_timeout_ms,
        timeout: 60,
        follow_redirects: None,
        http1_only: false,
        transport_profile: None,
    }
}

fn relay_envelope(meta: &RequestMeta, body: &[u8]) -> Vec<u8> {
    let encoded_meta = serde_json::to_vec(meta).expect("metadata should encode");
    let mut envelope = Vec::with_capacity(4 + encoded_meta.len() + body.len());
    envelope.extend_from_slice(&(encoded_meta.len() as u32).to_be_bytes());
    envelope.extend_from_slice(&encoded_meta);
    envelope.extend_from_slice(body);
    envelope
}

fn relay_metadata_envelope(envelope: &[u8]) -> &[u8] {
    let meta_len = u32::from_be_bytes(envelope[..4].try_into().expect("metadata prefix")) as usize;
    &envelope[..4 + meta_len]
}

fn authenticated_relay_request(
    client: &reqwest::Client,
    url: String,
    owner: &str,
    node_id: &str,
    envelope: &[u8],
) -> reqwest::RequestBuilder {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let metadata = relay_metadata_envelope(envelope);
    let payload_digest = tunnel_relay_payload_digest(metadata, &envelope[metadata.len()..]);
    let signature = sign_tunnel_relay_request(
        RELAY_TEST_SECRET.as_bytes(),
        RELAY_TEST_SENDER,
        owner,
        node_id,
        "",
        false,
        timestamp,
        &nonce,
        &payload_digest,
    );
    client
        .post(url)
        .header(TUNNEL_RELAY_AUTH_SENDER_HEADER, RELAY_TEST_SENDER)
        .header(TUNNEL_RELAY_OWNER_INSTANCE_HEADER, owner)
        .header(TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER, timestamp)
        .header(TUNNEL_RELAY_AUTH_NONCE_HEADER, nonce)
        .header(
            TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
            payload_digest.encode_header_value(),
        )
        .header(TUNNEL_RELAY_AUTH_SIGNATURE_HEADER, signature)
}

fn authenticated_forwarded_relay_request(
    client: &reqwest::Client,
    url: String,
    owner: &str,
    node_id: &str,
    envelope: &[u8],
) -> reqwest::RequestBuilder {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let metadata = relay_metadata_envelope(envelope);
    let payload_digest = tunnel_relay_payload_digest(metadata, &envelope[metadata.len()..]);
    let signature = sign_tunnel_relay_request(
        RELAY_TEST_SECRET.as_bytes(),
        RELAY_TEST_SENDER,
        owner,
        node_id,
        RELAY_TEST_SENDER,
        false,
        timestamp,
        &nonce,
        &payload_digest,
    );
    client
        .post(url)
        .header(TUNNEL_RELAY_AUTH_SENDER_HEADER, RELAY_TEST_SENDER)
        .header(TUNNEL_RELAY_OWNER_INSTANCE_HEADER, owner)
        .header(TUNNEL_RELAY_AUTH_TIMESTAMP_HEADER, timestamp)
        .header(TUNNEL_RELAY_AUTH_NONCE_HEADER, nonce)
        .header(
            TUNNEL_RELAY_AUTH_PAYLOAD_HEADER,
            payload_digest.encode_header_value(),
        )
        .header(TUNNEL_RELAY_AUTH_SIGNATURE_HEADER, signature)
        .header(
            aether_contracts::tunnel::TUNNEL_RELAY_FORWARDED_BY_HEADER,
            RELAY_TEST_SENDER,
        )
}

fn relay_test_proxy_node_repository() -> Arc<InMemoryProxyNodeRepository> {
    Arc::new(InMemoryProxyNodeRepository::seed([sample_proxy_node(
        RELAY_TEST_NODE_ID,
    )
    .with_tunnel_generation(RELAY_TEST_TUNNEL_GENERATION.to_string())]))
}

fn relay_test_data_state(
    config_values: impl IntoIterator<Item = (String, serde_json::Value)>,
) -> GatewayDataState {
    GatewayDataState::with_proxy_node_repository_for_tests(relay_test_proxy_node_repository())
        .with_system_config_values_for_tests(config_values)
}

#[tokio::test]
async fn gateway_handles_internal_tunnel_heartbeat_locally_with_loopback() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/tunnel/heartbeat",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        with_tunnel_control_plane_key(sample_proxy_node("node-123"), TUNNEL_CONTROL_PLANE_TEST_PSK),
    ]));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                Arc::clone(&repository),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let client = reqwest::Client::new();
    let heartbeat = json!({
        "node_id": "node-123",
        "heartbeat_session_id": "session-77",
        "heartbeat_id": 77,
        "heartbeat_interval": 45,
        "active_connections": 5,
        "total_requests": 100,
        "avg_latency_ms": 12.5,
        "failed_requests": 20,
        "dns_failures": 30,
        "stream_errors": 40,
        "window_total_requests": 9,
        "window_failed_requests": 1,
        "window_dns_failures": 2,
        "window_stream_errors": 3,
        "proxy_metadata": {"arch": "arm64"},
        "proxy_version": "2.0.0"
    });
    let anonymous = client
        .post(format!("{gateway_url}{TUNNEL_HEARTBEAT_PATH}"))
        .json(&heartbeat)
        .send()
        .await
        .expect("anonymous loopback request should complete");
    assert_eq!(anonymous.status(), StatusCode::FORBIDDEN);

    let forged = authenticated_tunnel_control_plane_request(
        &client,
        format!("{gateway_url}{TUNNEL_HEARTBEAT_PATH}"),
        TUNNEL_HEARTBEAT_PATH,
        "different-node",
        &heartbeat,
    )
    .send()
    .await
    .expect("forged identity request should complete");
    assert_eq!(forged.status(), StatusCode::FORBIDDEN);

    let stale_generation = authenticated_tunnel_control_plane_request_for_generation(
        &client,
        format!("{gateway_url}{TUNNEL_HEARTBEAT_PATH}"),
        TUNNEL_HEARTBEAT_PATH,
        "node-123",
        "deleted-node-generation",
        &heartbeat,
    )
    .send()
    .await
    .expect("stale generation request should complete");
    assert_eq!(stale_generation.status(), StatusCode::FORBIDDEN);
    let unchanged = repository
        .find_proxy_node("node-123")
        .await
        .expect("node lookup should succeed")
        .expect("node should exist");
    assert_eq!(unchanged.total_requests, 0);
    assert_eq!(unchanged.active_connections, 0);

    let response = authenticated_tunnel_control_plane_request(
        &client,
        format!("{gateway_url}{TUNNEL_HEARTBEAT_PATH}"),
        TUNNEL_HEARTBEAT_PATH,
        "node-123",
        &heartbeat,
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["heartbeat_id"], 77);
    assert_eq!(payload["config_version"], 7);
    assert_eq!(payload["upgrade_to"], "1.2.3");
    assert_eq!(payload["remote_config"]["allowed_ports"][0], 443);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    let node = repository
        .find_proxy_node("node-123")
        .await
        .expect("node lookup should succeed")
        .expect("node should exist");
    assert_eq!(node.total_requests, 9);
    assert_eq!(node.failed_requests, 1);
    assert_eq!(node.dns_failures, 2);
    assert_eq!(node.stream_errors, 3);

    let replay = json!({
        "node_id": "node-123",
        "heartbeat_session_id": "session-77",
        "heartbeat_id": 77,
        "heartbeat_interval": 45,
        "active_connections": 5,
        "window_total_requests": 9,
        "window_failed_requests": 1,
        "window_dns_failures": 2,
        "window_stream_errors": 3,
        "proxy_metadata": {"arch": "arm64"},
        "proxy_version": "2.0.0"
    });
    let replay_response = authenticated_tunnel_control_plane_request(
        &client,
        format!("{gateway_url}{TUNNEL_HEARTBEAT_PATH}"),
        TUNNEL_HEARTBEAT_PATH,
        "node-123",
        &replay,
    )
    .send()
    .await
    .expect("replayed request should receive the original ACK");
    assert_eq!(replay_response.status(), StatusCode::OK);
    let replay_payload: serde_json::Value = replay_response
        .json()
        .await
        .expect("replayed ACK should be JSON");
    assert_eq!(replay_payload["heartbeat_id"], 77);

    let node_after_replay = repository
        .find_proxy_node("node-123")
        .await
        .expect("node lookup should succeed")
        .expect("node should exist");
    assert_eq!(node_after_replay.total_requests, 9);
    assert_eq!(node_after_replay.failed_requests, 1);
    assert_eq!(node_after_replay.dns_failures, 2);
    assert_eq!(node_after_replay.stream_errors, 3);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_internal_tunnel_heartbeat_without_heartbeat_id() {
    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        with_tunnel_control_plane_key(sample_proxy_node("node-123"), TUNNEL_CONTROL_PLANE_TEST_PSK),
    ]));

    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                Arc::clone(&repository),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let body = json!({
        "node_id": "node-123",
        "heartbeat_interval": 45,
        "active_connections": 5
    });
    let response = authenticated_tunnel_control_plane_request(
        &reqwest::Client::new(),
        format!("{gateway_url}{TUNNEL_HEARTBEAT_PATH}"),
        TUNNEL_HEARTBEAT_PATH,
        "node-123",
        &body,
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_tunnel_node_status_locally_with_loopback() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/tunnel/node-status",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![
        with_tunnel_control_plane_key(sample_proxy_node("node-123"), TUNNEL_CONTROL_PLANE_TEST_PSK),
    ]));

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                Arc::clone(&repository),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let status_body = json!({
        "node_id": "node-123",
        "connected": true,
        "conn_count": 4,
        "observed_at_unix_secs": 1_800_000_321u64
    });
    let client = reqwest::Client::new();
    let anonymous = client
        .post(format!("{gateway_url}{TUNNEL_NODE_STATUS_PATH}"))
        .json(&status_body)
        .send()
        .await
        .expect("anonymous loopback request should complete");
    assert_eq!(anonymous.status(), StatusCode::FORBIDDEN);

    let forged = authenticated_tunnel_control_plane_request(
        &client,
        format!("{gateway_url}{TUNNEL_NODE_STATUS_PATH}"),
        TUNNEL_NODE_STATUS_PATH,
        "different-node",
        &status_body,
    )
    .send()
    .await
    .expect("forged identity request should complete");
    assert_eq!(forged.status(), StatusCode::FORBIDDEN);

    let response = authenticated_tunnel_control_plane_request(
        &client,
        format!("{gateway_url}{TUNNEL_NODE_STATUS_PATH}"),
        TUNNEL_NODE_STATUS_PATH,
        "node-123",
        &status_body,
    )
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value = response.json().await.expect("json body should parse");
    assert_eq!(payload["updated"], json!(true));
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);
    let node = repository
        .find_proxy_node("node-123")
        .await
        .expect("lookup should succeed")
        .expect("node should exist");
    assert_eq!(node.tunnel_connected_at_unix_secs, Some(1_800_000_321));

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_owns_proxy_tunnel_path_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/proxy-tunnel",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (_upstream_url, upstream_handle) = start_server(upstream).await;
    const NODE_ID: &str = "node-123";
    const SESSION: &str = "0123456789abcdef0123456789abcdef";
    const NONCE: &str = "abcdef0123456789abcdef0123456789";
    const PSK: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
    let repository = Arc::new(InMemoryProxyNodeRepository::seed([
        with_tunnel_control_plane_key(sample_proxy_node(NODE_ID), PSK),
    ]));
    let state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
            repository,
        ));
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let unsigned = reqwest::Client::new()
        .get(format!("{gateway_url}/api/internal/proxy-tunnel"))
        .header(http::header::CONNECTION, "upgrade")
        .header(http::header::UPGRADE, "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("x-node-id", NODE_ID)
        .header(
            aether_contracts::tunnel_security::TUNNEL_GENERATION_HEADER,
            TUNNEL_CONTROL_PLANE_TEST_GENERATION,
        )
        .header(
            aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
            aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION_STR,
        )
        .header(
            aether_contracts::tunnel_security::TUNNEL_SECURITY_HEADER,
            aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED,
        )
        .header(
            aether_contracts::tunnel_security::TUNNEL_SECURITY_SESSION_HEADER,
            SESSION,
        )
        .send()
        .await
        .expect("request should succeed");
    assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let signature =
        aether_contracts::tunnel_security::sign_tunnel_security_handshake_for_generation(
            PSK,
            NODE_ID,
            TUNNEL_CONTROL_PLANE_TEST_GENERATION,
            aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED,
            SESSION,
            aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION,
            timestamp,
            NONCE,
        )
        .expect("handshake proof should sign");
    let send_signed_upgrade = || {
        reqwest::Client::new()
            .get(format!("{gateway_url}/api/internal/proxy-tunnel"))
            .header(http::header::CONNECTION, "upgrade")
            .header(http::header::UPGRADE, "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("x-node-id", NODE_ID)
            .header(
                aether_contracts::tunnel_security::TUNNEL_GENERATION_HEADER,
                TUNNEL_CONTROL_PLANE_TEST_GENERATION,
            )
            .header(
                aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
                aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION_STR,
            )
            .header(
                aether_contracts::tunnel_security::TUNNEL_SECURITY_HEADER,
                aether_contracts::tunnel_security::TUNNEL_SECURITY_NON_TLS_REQUIRED,
            )
            .header(
                aether_contracts::tunnel_security::TUNNEL_SECURITY_SESSION_HEADER,
                SESSION,
            )
            .header(
                aether_contracts::tunnel_security::TUNNEL_SECURITY_PROOF_TIMESTAMP_HEADER,
                timestamp.to_string(),
            )
            .header(
                aether_contracts::tunnel_security::TUNNEL_SECURITY_PROOF_NONCE_HEADER,
                NONCE,
            )
            .header(
                aether_contracts::tunnel_security::TUNNEL_SECURITY_PROOF_SIGNATURE_HEADER,
                signature.clone(),
            )
    };

    let accepted = send_signed_upgrade()
        .send()
        .await
        .expect("signed WebSocket upgrade should complete");
    assert_eq!(accepted.status(), StatusCode::SWITCHING_PROTOCOLS);
    drop(accepted);

    let replay = send_signed_upgrade()
        .send()
        .await
        .expect("replayed WebSocket upgrade should complete");
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_requires_current_authorized_management_token_for_default_proxy_tunnel() {
    const NODE_ID: &str = "node-default-auth";
    const PROTOCOL_VERSION: &str = aether_contracts::tunnel::CURRENT_TUNNEL_PROTOCOL_VERSION_STR;
    let raw_token = "ae-default-tunnel-auth-token";

    let state = AppState::new().expect("gateway should build");
    let admin_user = state
        .create_local_auth_user_with_settings(
            Some("tunnel-admin@example.com".to_string()),
            true,
            "tunnel-admin".to_string(),
            "hash".to_string(),
            "admin".to_string(),
            None,
            None,
            None,
            None,
        )
        .await
        .expect("admin user should be created")
        .expect("admin user should exist");
    let mut token = sample_management_token(
        "token-default-tunnel-auth",
        &admin_user.id,
        "tunnel-admin",
        true,
    );
    token.token.allowed_ips = None;
    token.token.permissions = Some(json!(["admin:proxy_nodes:admin"]));
    let token_repository = Arc::new(InMemoryManagementTokenRepository::seed_with_hashes(
        [token],
        [(
            hash_management_token(raw_token),
            "token-default-tunnel-auth".to_string(),
        )],
    ));
    let proxy_node_repository = Arc::new(InMemoryProxyNodeRepository::seed([sample_proxy_node(
        NODE_ID,
    )
    .with_runtime_fields(
        Some("test".to_string()),
        Some("different-registering-admin".to_string()),
        Some(1_710_000_000),
        None,
        None,
        None,
        None,
        Some(1_710_000_010),
        None,
        Some(1_709_000_000),
        Some(1_710_000_100),
    )]));
    let state = state.with_data_state_for_tests(
        GatewayDataState::with_management_token_repository_for_tests(token_repository)
            .attach_proxy_node_repository_for_tests(proxy_node_repository)
            .with_user_reader(Arc::new(InMemoryUserReadRepository::seed_auth_users([
                admin_user,
            ]))),
    );
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let upgrade = |token: Option<&str>| {
        let request = reqwest::Client::new()
            .get(format!("{gateway_url}/api/internal/proxy-tunnel"))
            .header(http::header::CONNECTION, "upgrade")
            .header(http::header::UPGRADE, "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("x-node-id", NODE_ID)
            .header(
                aether_contracts::tunnel_security::TUNNEL_GENERATION_HEADER,
                TUNNEL_CONTROL_PLANE_TEST_GENERATION,
            )
            .header(
                aether_contracts::tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
                PROTOCOL_VERSION,
            );
        match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    };

    let missing = upgrade(None)
        .send()
        .await
        .expect("missing-token upgrade should complete");
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let invalid = upgrade(Some("ae-invalid-default-tunnel-token"))
        .send()
        .await
        .expect("invalid-token upgrade should complete");
    assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);

    let accepted = upgrade(Some(raw_token))
        .send()
        .await
        .expect("valid-token upgrade should complete");
    assert_eq!(accepted.status(), StatusCode::SWITCHING_PROTOCOLS);
    drop(accepted);

    gateway_handle.abort();
}

#[tokio::test]
async fn gateway_handles_internal_tunnel_relay_locally_without_proxying_upstream() {
    let upstream_hits = Arc::new(Mutex::new(0usize));
    let upstream_hits_clone = Arc::clone(&upstream_hits);
    let upstream = Router::new().route(
        "/api/internal/tunnel/relay/node-123",
        any(move |_request: Request| {
            let upstream_hits_inner = Arc::clone(&upstream_hits_clone);
            async move {
                *upstream_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected upstream hit"))
            }
        }),
    );

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_tunnel_identity_and_relay_secret_for_tests(
                "gateway-a",
                Some("http://gateway-a.internal"),
                RELAY_TEST_SECRET,
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    // Authenticate the request so the relay handler reaches its local body
    // parser. An empty metadata envelope is malformed and must be rejected
    // locally without ever attempting to proxy to an upstream gateway.
    let envelope = 0u32.to_be_bytes().to_vec();
    let response = authenticated_relay_request(
        &reqwest::Client::new(),
        format!("{gateway_url}/api/internal/tunnel/relay/node-123"),
        "gateway-a",
        "node-123",
        &envelope,
    )
    .body(envelope)
    .send()
    .await
    .expect("request should succeed");

    // Once authenticated, malformed relay metadata is rejected locally; the
    // request must never be dispatched to an upstream gateway.
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
}

#[tokio::test]
async fn gateway_forwards_tunnel_relay_to_attachment_owner() {
    let owner_hits = Arc::new(Mutex::new(0usize));
    let owner_hits_clone = Arc::clone(&owner_hits);
    let owner = Router::new().route(
        "/api/internal/tunnel/relay/node-123",
        post(move |headers: axum::http::HeaderMap, body: Body| {
            let owner_hits_inner = Arc::clone(&owner_hits_clone);
            async move {
                *owner_hits_inner.lock().expect("mutex should lock") += 1;
                assert_eq!(
                    headers
                        .get(aether_contracts::tunnel::TUNNEL_RELAY_FORWARDED_BY_HEADER)
                        .and_then(|value| value.to_str().ok()),
                    Some("gateway-a")
                );
                let body = axum::body::to_bytes(body, usize::MAX)
                    .await
                    .expect("body should read");
                let mut response = axum::http::Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(body))
                    .expect("response should build");
                response.headers_mut().insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/octet-stream"),
                );
                response
            }
        }),
    );

    let (owner_url, owner_handle) = start_server(owner).await;
    let data_state = relay_test_data_state([(
        "tunnel.attachments.node-123".to_string(),
        json!({
            "gateway_instance_id": "gateway-b",
            "relay_base_url": owner_url,
            "tunnel_generation": RELAY_TEST_TUNNEL_GENERATION,
            "conn_count": 1,
            "observed_at_unix_secs": 4_102_444_800u64,
        }),
    )]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_and_relay_secret_for_tests(
                "gateway-a",
                Some("http://gateway-a.internal"),
                RELAY_TEST_SECRET,
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let envelope = relay_envelope(&relay_request_meta(false, None, None), b"relay-envelope");

    let client = reqwest::Client::new();
    let response = authenticated_relay_request(
        &client,
        format!("{gateway_url}/api/internal/tunnel/relay/node-123"),
        "gateway-a",
        "node-123",
        &envelope,
    )
    .header(TRACE_ID_HEADER, "trace-owner-forward")
    .header(http::header::CONTENT_TYPE, "application/octet-stream")
    .body(envelope.clone())
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(TRACE_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("trace-owner-forward")
    );
    assert_eq!(
        response.bytes().await.expect("body should read"),
        Bytes::from(envelope)
    );
    assert_eq!(*owner_hits.lock().expect("mutex should lock"), 1);

    gateway_handle.abort();
    owner_handle.abort();
}

#[tokio::test]
async fn gateway_owner_relay_uses_non_stream_timeout_from_envelope() {
    let owner = Router::new().route(
        "/api/internal/tunnel/relay/node-123",
        post(|body: Body| async move {
            let body = axum::body::to_bytes(body, usize::MAX)
                .await
                .expect("body should read");
            tokio::time::sleep(Duration::from_millis(40)).await;
            (StatusCode::OK, Body::from(body))
        }),
    );

    let (owner_url, owner_handle) = start_server(owner).await;
    let data_state = relay_test_data_state([(
        "tunnel.attachments.node-123".to_string(),
        json!({
            "gateway_instance_id": "gateway-b",
            "relay_base_url": owner_url,
            "tunnel_generation": RELAY_TEST_TUNNEL_GENERATION,
            "conn_count": 1,
            "observed_at_unix_secs": 4_102_444_800u64,
        }),
    )]);
    let mut state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state)
        .with_tunnel_identity_and_relay_secret_for_tests(
            "gateway-a",
            Some("http://gateway-a.internal"),
            RELAY_TEST_SECRET,
        );
    let short_timeout_client = reqwest::Client::builder()
        .timeout(Duration::from_millis(10))
        .build()
        .expect("test client should build");
    state.client = short_timeout_client.clone();
    state.owner_forward_client = short_timeout_client;
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let meta = relay_request_meta(false, Some(100), None);
    let envelope = relay_envelope(&meta, b"relay-body");
    let encoded_meta = serde_json::to_vec(&meta).expect("metadata should encode");
    let split_at = 4 + encoded_meta.len() / 2;
    let request_body = reqwest::Body::wrap_stream(stream::iter(vec![
        Ok::<Bytes, io::Error>(Bytes::copy_from_slice(&envelope[..split_at])),
        Ok::<Bytes, io::Error>(Bytes::copy_from_slice(&envelope[split_at..])),
    ]));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("request client should build");
    let response = authenticated_relay_request(
        &client,
        format!("{gateway_url}/api/internal/tunnel/relay/node-123"),
        "gateway-a",
        "node-123",
        &envelope,
    )
    .body(request_body)
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.expect("response body should read"),
        Bytes::from(envelope)
    );

    gateway_handle.abort();
    owner_handle.abort();
}

#[tokio::test]
async fn gateway_streams_tunnel_relay_body_to_attachment_owner() {
    let owner_hits = Arc::new(Mutex::new(0usize));
    let owner_hits_clone = Arc::clone(&owner_hits);
    let owner = Router::new().route(
        "/api/internal/tunnel/relay/node-123",
        post(move |body: Body| {
            let owner_hits_inner = Arc::clone(&owner_hits_clone);
            async move {
                *owner_hits_inner.lock().expect("mutex should lock") += 1;
                let body = axum::body::to_bytes(body, usize::MAX)
                    .await
                    .expect("body should read");
                let response_body = Body::from_stream(async_stream::stream! {
                    yield Ok::<_, io::Error>(Bytes::from_static(b"stream-"));
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    yield Ok::<_, io::Error>(Bytes::from_static(b"ok"));
                });
                (StatusCode::OK, response_body)
            }
        }),
    );

    let (owner_url, owner_handle) = start_server(owner).await;
    let data_state = relay_test_data_state([
        (
            "tunnel.attachments.node-123".to_string(),
            json!({
                "gateway_instance_id": "gateway-b",
                "relay_base_url": owner_url,
                "tunnel_generation": RELAY_TEST_TUNNEL_GENERATION,
                "conn_count": 1,
                "observed_at_unix_secs": 4_102_444_800u64,
            }),
        ),
        ("max_request_body_size".to_string(), json!(8)),
    ]);
    let mut state = AppState::new()
        .expect("gateway should build")
        .with_data_state_for_tests(data_state)
        .with_tunnel_identity_and_relay_secret_for_tests(
            "gateway-a",
            Some("http://gateway-a.internal"),
            RELAY_TEST_SECRET,
        );
    state.client = reqwest::Client::builder()
        .timeout(Duration::from_millis(10))
        .build()
        .expect("short shared client should build");
    let gateway = build_router_with_state(state);
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let meta = relay_request_meta(true, Some(900_000), Some(100));
    let envelope = relay_envelope(&meta, b"relay-stream-envelope");
    let expected_envelope = Bytes::copy_from_slice(&envelope);
    let request_body = reqwest::Body::wrap_stream(stream::iter(vec![Ok::<Bytes, io::Error>(
        expected_envelope.clone(),
    )]));
    let client = reqwest::Client::new();
    let response = authenticated_relay_request(
        &client,
        format!("{gateway_url}/api/internal/tunnel/relay/node-123"),
        "gateway-a",
        "node-123",
        &envelope,
    )
    .body(request_body)
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.expect("body should read"),
        Bytes::from_static(b"stream-ok")
    );
    assert_eq!(*owner_hits.lock().expect("mutex should lock"), 1);

    gateway_handle.abort();
    owner_handle.abort();
}

#[tokio::test]
async fn gateway_does_not_forward_tunnel_relay_twice() {
    let owner_hits = Arc::new(Mutex::new(0usize));
    let owner_hits_clone = Arc::clone(&owner_hits);
    let owner = Router::new().route(
        "/api/internal/tunnel/relay/node-123",
        post(move |_request: Request| {
            let owner_hits_inner = Arc::clone(&owner_hits_clone);
            async move {
                *owner_hits_inner.lock().expect("mutex should lock") += 1;
                (StatusCode::OK, Body::from("unexpected owner hit"))
            }
        }),
    );

    let (owner_url, owner_handle) = start_server(owner).await;
    let data_state = relay_test_data_state([(
        "tunnel.attachments.node-123".to_string(),
        json!({
            "gateway_instance_id": "gateway-b",
            "relay_base_url": owner_url,
            "tunnel_generation": RELAY_TEST_TUNNEL_GENERATION,
            "conn_count": 1,
            "observed_at_unix_secs": 4_102_444_800u64,
        }),
    )]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_and_relay_secret_for_tests(
                "gateway-a",
                Some("http://gateway-a.internal"),
                RELAY_TEST_SECRET,
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let envelope = relay_envelope(&relay_request_meta(false, None, None), &[]);
    let client = reqwest::Client::new();
    let response = authenticated_forwarded_relay_request(
        &client,
        format!("{gateway_url}/api/internal/tunnel/relay/node-123"),
        "gateway-a",
        "node-123",
        &envelope,
    )
    .body(envelope)
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(*owner_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    owner_handle.abort();
}

#[tokio::test]
async fn gateway_forwards_owner_relay_body_above_recording_limit() {
    const RECORDING_LIMIT_BYTES: usize = 8 * 1024 * 1024;

    let captured_body = Arc::new(Mutex::new(None::<Bytes>));
    let captured_body_clone = Arc::clone(&captured_body);
    let owner = Router::new().route(
        "/api/internal/tunnel/relay/node-123",
        post(move |body: Body| {
            let captured_body_inner = Arc::clone(&captured_body_clone);
            async move {
                let body = axum::body::to_bytes(body, usize::MAX)
                    .await
                    .expect("owner body should read");
                *captured_body_inner.lock().expect("mutex should lock") = Some(body);
                StatusCode::OK
            }
        }),
    );

    let (owner_url, owner_handle) = start_server(owner).await;
    let data_state = relay_test_data_state([
        (
            "tunnel.attachments.node-123".to_string(),
            json!({
                "gateway_instance_id": "gateway-b",
                "relay_base_url": owner_url,
                "tunnel_generation": RELAY_TEST_TUNNEL_GENERATION,
                "conn_count": 1,
                "observed_at_unix_secs": 4_102_444_800u64,
            }),
        ),
        (
            "max_request_body_size".to_string(),
            json!(RECORDING_LIMIT_BYTES),
        ),
    ]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_and_relay_secret_for_tests(
                "gateway-a",
                Some("http://gateway-a.internal"),
                RELAY_TEST_SECRET,
            ),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let request_payload = vec![b'x'; RECORDING_LIMIT_BYTES + 1];
    let envelope = relay_envelope(
        &relay_request_meta(false, Some(60_000), None),
        &request_payload,
    );
    assert!(envelope.len() > RECORDING_LIMIT_BYTES);
    let client = reqwest::Client::new();
    let response = authenticated_relay_request(
        &client,
        format!("{gateway_url}/api/internal/tunnel/relay/node-123"),
        "gateway-a",
        "node-123",
        &envelope,
    )
    .body(envelope.clone())
    .send()
    .await
    .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        captured_body.lock().expect("mutex should lock").as_ref(),
        Some(&Bytes::from(envelope))
    );

    gateway_handle.abort();
    owner_handle.abort();
}
