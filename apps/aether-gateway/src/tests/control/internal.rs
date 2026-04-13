use std::io;
use std::sync::{Arc, Mutex};

use aether_data::repository::proxy_nodes::ProxyNodeReadRepository;
use axum::body::Body;
use axum::routing::{any, post};
use axum::{extract::Request, Json, Router};
use bytes::Bytes;
use futures_util::stream;
use http::header::HeaderValue;
use http::StatusCode;
use serde_json::json;

use super::{
    build_router_with_state, sample_proxy_node, start_server, AppState, GatewayDataState,
    InMemoryProxyNodeRepository, TRACE_ID_HEADER,
};

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

    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
        "node-123",
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                Arc::clone(&repository),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/tunnel/heartbeat"))
        .json(&json!({
            "node_id": "node-123",
            "heartbeat_id": 77,
            "heartbeat_interval": 45,
            "active_connections": 5,
            "total_requests": 9,
            "avg_latency_ms": 12.5,
            "failed_requests": 1,
            "dns_failures": 2,
            "stream_errors": 3,
            "proxy_metadata": {"arch": "arm64"},
            "proxy_version": "2.0.0",
        }))
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

    gateway_handle.abort();
    upstream_handle.abort();
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

    let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![sample_proxy_node(
        "node-123",
    )]));

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(GatewayDataState::with_proxy_node_repository_for_tests(
                Arc::clone(&repository),
            )),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/tunnel/node-status"))
        .json(&json!({
            "node_id": "node-123",
            "connected": true,
            "conn_count": 4,
            "observed_at_unix_secs": 1_800_000_321u64,
        }))
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

    let (upstream_url, upstream_handle) = start_server(upstream).await;
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .get(format!("{gateway_url}/api/internal/proxy-tunnel"))
        .header("x-node-id", "node-123")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(*upstream_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    upstream_handle.abort();
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
    let gateway = build_router_with_state(AppState::new().expect("gateway should build"));
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/tunnel/relay/node-123"))
        .body(Vec::<u8>::new())
        .send()
        .await
        .expect("request should succeed");

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
    let data_state = GatewayDataState::disabled().with_system_config_values_for_tests(vec![(
        "tunnel.attachments.node-123".to_string(),
        json!({
            "gateway_instance_id": "gateway-b",
            "relay_base_url": owner_url,
            "conn_count": 1,
            "observed_at_unix_secs": 4_102_444_800u64,
        }),
    )]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_for_tests("gateway-a", Some("http://gateway-a.internal")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/tunnel/relay/node-123"))
        .header(TRACE_ID_HEADER, "trace-owner-forward")
        .body("relay-envelope")
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
        response.text().await.expect("body should read"),
        "relay-envelope"
    );
    assert_eq!(*owner_hits.lock().expect("mutex should lock"), 1);

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
                assert_eq!(body, Bytes::from_static(b"relay-stream-envelope"));
                (StatusCode::OK, Body::from("stream-ok"))
            }
        }),
    );

    let (owner_url, owner_handle) = start_server(owner).await;
    let data_state = GatewayDataState::disabled().with_system_config_values_for_tests(vec![(
        "tunnel.attachments.node-123".to_string(),
        json!({
            "gateway_instance_id": "gateway-b",
            "relay_base_url": owner_url,
            "conn_count": 1,
            "observed_at_unix_secs": 4_102_444_800u64,
        }),
    )]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_for_tests("gateway-a", Some("http://gateway-a.internal")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let request_body = reqwest::Body::wrap_stream(stream::iter(vec![
        Ok::<Bytes, io::Error>(Bytes::from_static(b"relay-")),
        Ok::<Bytes, io::Error>(Bytes::from_static(b"stream-")),
        Ok::<Bytes, io::Error>(Bytes::from_static(b"envelope")),
    ]));
    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/tunnel/relay/node-123"))
        .body(request_body)
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.text().await.expect("body should read"),
        "stream-ok"
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
    let data_state = GatewayDataState::disabled().with_system_config_values_for_tests(vec![(
        "tunnel.attachments.node-123".to_string(),
        json!({
            "gateway_instance_id": "gateway-b",
            "relay_base_url": owner_url,
            "conn_count": 1,
            "observed_at_unix_secs": 4_102_444_800u64,
        }),
    )]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_for_tests("gateway-a", Some("http://gateway-a.internal")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/tunnel/relay/node-123"))
        .header(
            aether_contracts::tunnel::TUNNEL_RELAY_FORWARDED_BY_HEADER,
            "gateway-z",
        )
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(*owner_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    owner_handle.abort();
}

#[tokio::test]
async fn gateway_rejects_owner_relay_body_above_configured_limit() {
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
    let data_state = GatewayDataState::disabled().with_system_config_values_for_tests(vec![
        (
            "tunnel.attachments.node-123".to_string(),
            json!({
                "gateway_instance_id": "gateway-b",
                "relay_base_url": owner_url,
                "conn_count": 1,
                "observed_at_unix_secs": 4_102_444_800u64,
            }),
        ),
        ("max_request_body_size".to_string(), json!(8)),
    ]);
    let gateway = build_router_with_state(
        AppState::new()
            .expect("gateway should build")
            .with_data_state_for_tests(data_state)
            .with_tunnel_identity_for_tests("gateway-a", Some("http://gateway-a.internal")),
    );
    let (gateway_url, gateway_handle) = start_server(gateway).await;

    let response = reqwest::Client::new()
        .post(format!("{gateway_url}/api/internal/tunnel/relay/node-123"))
        .body("relay-envelope")
        .send()
        .await
        .expect("request should succeed");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(*owner_hits.lock().expect("mutex should lock"), 0);

    gateway_handle.abort();
    owner_handle.abort();
}
