use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use aether_contracts::tunnel::{
    resolve_tunnel_request_timeouts, try_decode_tunnel_relay_request_meta,
    TUNNEL_RELAY_FORWARDED_BY_HEADER,
};
use aether_runtime::{maybe_hold_axum_response_permit, AdmissionPermit};
use async_stream::stream;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use bytes::BytesMut;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::warn;

use crate::api::response::apply_streaming_response_headers;
use crate::headers::should_skip_response_header;
use crate::maintenance::record_proxy_upgrade_traffic_success;

use super::hub::{LocalBodyEvent, LocalStream};
use super::protocol;
use super::AppState;

pub const TUNNEL_ERROR_HEADER: &str = "x-aether-tunnel-error";

struct StreamGuard {
    hub: std::sync::Arc<super::hub::HubRouter>,
    stream_id: u64,
    finished: bool,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.hub
                .cancel_local_stream(self.stream_id, "local relay client dropped");
        }
    }
}

pub(crate) struct DirectRelayResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body_rx: mpsc::Receiver<LocalBodyEvent>,
    request_guard: StreamGuard,
    _request_permit: Option<AdmissionPermit>,
}

impl DirectRelayResponse {
    pub(crate) fn status(&self) -> u16 {
        self.status
    }

    pub(crate) fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    pub(crate) async fn next_chunk(&mut self) -> Result<Option<Bytes>, String> {
        let event = self.body_rx.recv().await;
        match event {
            Some(LocalBodyEvent::Chunk(chunk)) => Ok(Some(chunk)),
            Some(LocalBodyEvent::End) | None => {
                self.request_guard.finished = true;
                Ok(None)
            }
            Some(LocalBodyEvent::Error(error)) => {
                self.request_guard.finished = true;
                Err(error)
            }
        }
    }
}

pub(crate) async fn open_direct_relay_stream(
    state: &AppState,
    node_id: &str,
    meta: protocol::RequestMeta,
    body: Bytes,
) -> Result<DirectRelayResponse, String> {
    let request_permit = state
        .try_acquire_request_permit()
        .await
        .map_err(map_request_admission_error)?;
    let stream = state
        .hub
        .open_local_stream(node_id, &meta)
        .await
        .map_err(|error| format!("connect: {error}"))?;
    if let Err(error) = state
        .hub
        .push_local_request_body(stream.id, body, true)
        .await
    {
        state.hub.cancel_local_stream(stream.id, &error);
        return Err(format!("connect: {error}"));
    }

    let wait_timeout = relay_header_timeout(&meta);
    let response_head = match stream.wait_headers(wait_timeout).await {
        Ok(response) => response,
        Err(error) => {
            state.hub.cancel_local_stream(stream.id, &error);
            return Err(format!("timeout: {error}"));
        }
    };
    if let Err(error) = record_proxy_upgrade_traffic_success(state.data.as_ref(), node_id).await {
        warn!(
            node_id = %node_id,
            error = %error,
            "failed to record proxy upgrade traffic confirmation"
        );
    }

    let Some(body_rx) = stream.take_body_receiver() else {
        state
            .hub
            .cancel_local_stream(stream.id, "missing relay response body receiver");
        return Err("relay: missing relay response body receiver".to_string());
    };

    Ok(DirectRelayResponse {
        status: response_head.status,
        headers: response_head.headers,
        body_rx,
        request_guard: StreamGuard {
            hub: state.hub.clone(),
            stream_id: stream.id,
            finished: false,
        },
        _request_permit: request_permit,
    })
}

fn map_request_admission_error(error: super::RequestAdmissionError) -> String {
    match error {
        super::RequestAdmissionError::Local(aether_runtime::ConcurrencyError::Saturated {
            ..
        })
        | super::RequestAdmissionError::Distributed(
            aether_runtime_state::RuntimeSemaphoreError::Saturated { .. },
        )
        | super::RequestAdmissionError::Distributed(
            aether_runtime_state::RuntimeSemaphoreError::Unavailable { .. },
        ) => "overloaded: hub relay overloaded".to_string(),
        super::RequestAdmissionError::Local(aether_runtime::ConcurrencyError::Closed {
            ..
        }) => "overloaded: hub relay gate closed".to_string(),
        super::RequestAdmissionError::Distributed(
            aether_runtime_state::RuntimeSemaphoreError::InvalidConfiguration(_),
        ) => "overloaded: hub relay distributed gate invalid".to_string(),
    }
}

fn relay_header_timeout(meta: &protocol::RequestMeta) -> Duration {
    Duration::from_millis(resolve_tunnel_request_timeouts(meta).first_byte_ms)
}

fn is_rollout_probe_request(headers: &HeaderMap, forwarded_by_gateway: bool) -> bool {
    forwarded_by_gateway
        && headers
            .get(crate::tunnel::TUNNEL_RELAY_ROLLOUT_PROBE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .is_some_and(|value| value == crate::tunnel::TUNNEL_RELAY_ROLLOUT_PROBE_VALUE)
}

pub async fn relay_request(
    Path(node_id): Path<String>,
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
) -> impl IntoResponse {
    let forwarded_by_gateway = request
        .headers()
        .get(TUNNEL_RELAY_FORWARDED_BY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    let rollout_probe = is_rollout_probe_request(request.headers(), forwarded_by_gateway);
    if !addr.ip().is_loopback() && !forwarded_by_gateway {
        return tunnel_error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            "local relay only accepts loopback requests",
        );
    }

    let request_permit = match state.try_acquire_request_permit().await {
        Ok(permit) => permit,
        Err(super::RequestAdmissionError::Local(aether_runtime::ConcurrencyError::Saturated {
            ..
        }))
        | Err(super::RequestAdmissionError::Distributed(
            aether_runtime_state::RuntimeSemaphoreError::Saturated { .. },
        ))
        | Err(super::RequestAdmissionError::Distributed(
            aether_runtime_state::RuntimeSemaphoreError::Unavailable { .. },
        )) => {
            return tunnel_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "overloaded",
                "hub relay overloaded",
            );
        }
        Err(super::RequestAdmissionError::Local(aether_runtime::ConcurrencyError::Closed {
            ..
        })) => {
            return tunnel_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "overloaded",
                "hub relay gate closed",
            );
        }
        Err(super::RequestAdmissionError::Distributed(
            aether_runtime_state::RuntimeSemaphoreError::InvalidConfiguration(_),
        )) => {
            return tunnel_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "overloaded",
                "hub relay distributed gate invalid",
            );
        }
    };

    let mut body_stream = request.into_body().into_data_stream();
    let mut envelope_buf = BytesMut::new();
    let mut meta: Option<protocol::RequestMeta> = None;
    let mut stream: Option<std::sync::Arc<LocalStream>> = None;

    while let Some(chunk_result) = body_stream.next().await {
        let chunk = match chunk_result {
            Ok(chunk) => chunk,
            Err(error) => {
                if let Some(active_stream) = &stream {
                    state
                        .hub
                        .cancel_local_stream(active_stream.id, "failed to read relay request body");
                }
                warn!(error = %error, "failed to read local relay request body");
                return release_permit_response(
                    tunnel_error_response(
                        StatusCode::BAD_GATEWAY,
                        "relay",
                        "failed to read relay request body",
                    ),
                    request_permit,
                );
            }
        };

        if stream.is_none() {
            envelope_buf.extend_from_slice(&chunk);
            let Some((parsed_meta, body_offset)) =
                (match try_decode_tunnel_relay_request_meta(&envelope_buf) {
                    Ok(result) => result,
                    Err(error) => {
                        return release_permit_response(
                            tunnel_error_response(StatusCode::BAD_REQUEST, "bad_request", &error),
                            request_permit,
                        );
                    }
                })
            else {
                continue;
            };

            let opened_stream = match state.hub.open_local_stream(&node_id, &parsed_meta).await {
                Ok(stream) => stream,
                Err(error) => {
                    return release_permit_response(
                        tunnel_error_response(StatusCode::SERVICE_UNAVAILABLE, "connect", &error),
                        request_permit,
                    );
                }
            };

            if envelope_buf.len() > body_offset {
                let first_body_chunk = Bytes::copy_from_slice(&envelope_buf[body_offset..]);
                if let Err(error) = state
                    .hub
                    .push_local_request_body(opened_stream.id, first_body_chunk, false)
                    .await
                {
                    state.hub.cancel_local_stream(opened_stream.id, &error);
                    return release_permit_response(
                        tunnel_error_response(StatusCode::SERVICE_UNAVAILABLE, "connect", &error),
                        request_permit,
                    );
                }
            }

            envelope_buf.clear();
            meta = Some(parsed_meta);
            stream = Some(opened_stream);
            continue;
        }

        let Some(active_stream) = &stream else {
            continue;
        };
        if let Err(error) = state
            .hub
            .push_local_request_body(active_stream.id, chunk, false)
            .await
        {
            state.hub.cancel_local_stream(active_stream.id, &error);
            return release_permit_response(
                tunnel_error_response(StatusCode::SERVICE_UNAVAILABLE, "connect", &error),
                request_permit,
            );
        }
    }

    let (meta, stream) = match (meta, stream) {
        (Some(meta), Some(stream)) => (meta, stream),
        _ => {
            return release_permit_response(
                tunnel_error_response(
                    StatusCode::BAD_REQUEST,
                    "bad_request",
                    "relay envelope metadata truncated",
                ),
                request_permit,
            );
        }
    };

    if let Err(error) = state
        .hub
        .push_local_request_body(stream.id, Bytes::new(), true)
        .await
    {
        state.hub.cancel_local_stream(stream.id, &error);
        return release_permit_response(
            tunnel_error_response(StatusCode::SERVICE_UNAVAILABLE, "connect", &error),
            request_permit,
        );
    }

    let request_guard = StreamGuard {
        hub: state.hub.clone(),
        stream_id: stream.id,
        finished: false,
    };

    let wait_timeout = relay_header_timeout(&meta);
    let response_head = match stream.wait_headers(wait_timeout).await {
        Ok(response) => response,
        Err(error) => {
            state.hub.cancel_local_stream(stream.id, &error);
            return release_permit_response(
                tunnel_error_response(StatusCode::GATEWAY_TIMEOUT, "timeout", &error),
                request_permit,
            );
        }
    };
    if !rollout_probe {
        if let Err(error) =
            record_proxy_upgrade_traffic_success(state.data.as_ref(), &node_id).await
        {
            warn!(
                node_id = %node_id,
                error = %error,
                "failed to record proxy upgrade traffic confirmation"
            );
        }
    }

    let Some(mut body_rx) = stream.take_body_receiver() else {
        state
            .hub
            .cancel_local_stream(stream.id, "missing relay response body receiver");
        return release_permit_response(
            tunnel_error_response(
                StatusCode::BAD_GATEWAY,
                "relay",
                "missing relay response body receiver",
            ),
            request_permit,
        );
    };

    let hub = state.hub.clone();
    let stream_id = stream.id;
    let body_stream = stream! {
        let mut guard = request_guard;
        guard.hub = hub;
        guard.stream_id = stream_id;
        while let Some(event) = body_rx.recv().await {
            match event {
                LocalBodyEvent::Chunk(chunk) => yield Ok::<Bytes, io::Error>(chunk),
                LocalBodyEvent::End => {
                    guard.finished = true;
                    break;
                }
                LocalBodyEvent::Error(error) => {
                    guard.finished = true;
                    yield Err(io::Error::other(error));
                    break;
                }
            }
        }
        guard.finished = true;
    };

    let mut builder = Response::builder().status(response_head.status);
    if let Some(headers) = builder.headers_mut() {
        append_headers(headers, &response_head.headers);
        apply_streaming_response_headers(headers);
    }
    match builder.body(Body::from_stream(body_stream)) {
        Ok(response) => maybe_hold_axum_response_permit(response, request_permit),
        Err(error) => {
            warn!(error = %error, "failed to build relay response");
            release_permit_response(
                tunnel_error_response(
                    StatusCode::BAD_GATEWAY,
                    "relay",
                    "failed to build relay response",
                ),
                request_permit,
            )
        }
    }
}

fn release_permit_response(
    response: Response<Body>,
    _request_permit: Option<AdmissionPermit>,
) -> Response<Body> {
    response
}

fn append_headers(target: &mut HeaderMap, headers: &[(String, String)]) {
    for (name, value) in headers {
        if should_skip_local_relay_response_header(name) {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(value) else {
            continue;
        };
        target.append(name, value);
    }
}

fn should_skip_local_relay_response_header(name: &str) -> bool {
    should_skip_response_header(name) || name.eq_ignore_ascii_case("content-length")
}

fn tunnel_error_response(status: StatusCode, kind: &str, message: &str) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    if let Some(headers) = builder.headers_mut() {
        headers.insert(
            HeaderName::from_static(TUNNEL_ERROR_HEADER),
            HeaderValue::from_str(kind).unwrap_or_else(|_| HeaderValue::from_static("relay")),
        );
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
    }
    builder
        .body(Body::from(message.to_string()))
        .unwrap_or_else(|_| Response::new(Body::from("relay error")))
}

#[cfg(test)]
mod tests {
    use super::super::hub::ProxyConn;
    use super::super::{protocol, AppState, ConnConfig, ControlPlaneClient};
    use super::{
        is_rollout_probe_request, relay_header_timeout, relay_request, Body, HeaderMap, Request,
        SocketAddr, StatusCode, TUNNEL_ERROR_HEADER,
    };
    use crate::data::GatewayDataState;
    use crate::maintenance::start_proxy_upgrade_rollout;
    use aether_contracts::tunnel::TUNNEL_RELAY_FORWARDED_BY_HEADER;
    use aether_data::repository::proxy_nodes::{
        InMemoryProxyNodeRepository, ProxyNodeHeartbeatMutation, ProxyNodeWriteRepository,
        StoredProxyNode,
    };
    use axum::extract::ws::Message;
    use axum::extract::{ConnectInfo, Path, State};
    use axum::response::IntoResponse;
    use bytes::Bytes;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::watch;

    #[test]
    fn rollout_probe_marker_is_only_trusted_from_a_forwarding_gateway() {
        let mut headers = HeaderMap::new();
        headers.insert(
            crate::tunnel::TUNNEL_RELAY_ROLLOUT_PROBE_HEADER,
            crate::tunnel::TUNNEL_RELAY_ROLLOUT_PROBE_VALUE
                .parse()
                .expect("probe marker should be a valid header"),
        );

        assert!(!is_rollout_probe_request(&headers, false));
        assert!(is_rollout_probe_request(&headers, true));
    }

    fn test_app_state() -> AppState {
        AppState::new(
            ControlPlaneClient::disabled(),
            ConnConfig {
                ping_interval: Duration::from_secs(15),
                idle_timeout: Duration::from_secs(0),
                outbound_queue_capacity: 128,
            },
            128,
        )
    }

    #[test]
    fn relay_header_timeout_ignores_request_timeout_for_stream_requests() {
        let meta = protocol::RequestMeta {
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            method: "GET".to_string(),
            url: "https://example.com/stream".to_string(),
            headers: HashMap::new(),
            stream: true,
            request_timeout_ms: Some(90_000),
            stream_first_byte_timeout_ms: None,
            timeout: 7,
            follow_redirects: None,
            http1_only: false,
            transport_profile: None,
        };

        assert_eq!(relay_header_timeout(&meta), Duration::from_secs(7));
    }

    #[test]
    fn relay_header_timeout_keeps_the_protocol_maximum_for_non_stream_requests() {
        let meta = protocol::RequestMeta {
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            method: "POST".to_string(),
            url: "https://example.com/responses".to_string(),
            headers: HashMap::new(),
            stream: false,
            request_timeout_ms: Some(aether_contracts::MAX_EXECUTION_REQUEST_TIMEOUT_MS),
            stream_first_byte_timeout_ms: None,
            timeout: 60,
            follow_redirects: None,
            http1_only: false,
            transport_profile: None,
        };

        assert_eq!(
            relay_header_timeout(&meta),
            Duration::from_millis(aether_contracts::MAX_EXECUTION_REQUEST_TIMEOUT_MS)
        );
    }

    fn sample_connected_proxy_node(node_id: &str) -> StoredProxyNode {
        StoredProxyNode::new(
            node_id.to_string(),
            format!("proxy-{node_id}"),
            "127.0.0.1".to_string(),
            0,
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
            0,
        )
        .expect("node should build")
        .with_runtime_fields(
            Some("test".to_string()),
            None,
            Some(1_800_000_000),
            None,
            None,
            None,
            None,
            Some(1_800_000_000),
            None,
            Some(1_800_000_000),
            Some(1_800_000_000),
        )
    }

    fn encode_relay_envelope(meta: &protocol::RequestMeta, body: &[u8]) -> Vec<u8> {
        let meta_bytes = serde_json::to_vec(meta).expect("meta should serialize");
        let mut payload = Vec::with_capacity(4 + meta_bytes.len() + body.len());
        payload.extend_from_slice(&(meta_bytes.len() as u32).to_be_bytes());
        payload.extend_from_slice(&meta_bytes);
        payload.extend_from_slice(body);
        payload
    }

    #[tokio::test]
    async fn relay_rejects_non_loopback_without_forwarded_header() {
        let request = Request::builder()
            .body(Body::empty())
            .expect("request should build");
        let response = relay_request(
            Path("node-123".to_string()),
            State(test_app_state()),
            ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 4242))),
            request,
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn relay_accepts_forwarded_gateway_request_from_non_loopback() {
        let request = Request::builder()
            .header(TUNNEL_RELAY_FORWARDED_BY_HEADER, "gateway-a")
            .body(Body::empty())
            .expect("request should build");
        let response = relay_request(
            Path("node-123".to_string()),
            State(test_app_state()),
            ConnectInfo(SocketAddr::from(([10, 0, 0, 1], 4242))),
            request,
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(TUNNEL_ERROR_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("bad_request")
        );
    }

    #[tokio::test]
    async fn relay_records_real_traffic_confirmation_for_upgrade_rollout() {
        let mut node = sample_connected_proxy_node("node-123");
        node.proxy_metadata = Some(json!({"version": "1.0.0"}));
        let repository = Arc::new(InMemoryProxyNodeRepository::seed(vec![node]));
        let data = Arc::new(
            GatewayDataState::with_proxy_node_repository_for_tests(Arc::clone(&repository))
                .with_system_config_values_for_tests(Vec::<(String, serde_json::Value)>::new()),
        );

        let started = start_proxy_upgrade_rollout(data.as_ref(), "2.0.0".to_string(), 1, 0, None)
            .await
            .expect("rollout should start");
        assert_eq!(started.node_ids, vec!["node-123".to_string()]);

        repository
            .apply_heartbeat(&ProxyNodeHeartbeatMutation {
                node_id: "node-123".to_string(),
                heartbeat_interval: None,
                active_connections: Some(1),
                total_requests_delta: Some(1),
                avg_latency_ms: Some(2.0),
                failed_requests_delta: Some(0),
                dns_failures_delta: Some(0),
                stream_errors_delta: Some(0),
                proxy_metadata: Some(json!({"version": "2.0.0"})),
                proxy_version: Some("2.0.0".to_string()),
            })
            .await
            .expect("heartbeat should succeed");

        let observed = start_proxy_upgrade_rollout(data.as_ref(), "2.0.0".to_string(), 1, 0, None)
            .await
            .expect("rollout should observe version confirmation");
        assert!(observed.blocked);
        assert_eq!(observed.pending_node_ids, vec!["node-123".to_string()]);

        let state = test_app_state().with_data(Arc::clone(&data));
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        state.hub.register_proxy(Arc::new(ProxyConn::new(
            500,
            "node-123".to_string(),
            "Node 123".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        )));

        let meta = protocol::RequestMeta {
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            method: "GET".to_string(),
            url: "https://example.com/health".to_string(),
            headers: HashMap::new(),
            stream: false,
            request_timeout_ms: None,
            stream_first_byte_timeout_ms: None,
            timeout: 30,
            follow_redirects: None,
            http1_only: false,
            transport_profile: None,
        };
        let request = Request::builder()
            .body(Body::from(encode_relay_envelope(&meta, &[])))
            .expect("request should build");

        let relay_state = state.clone();
        let relay_task = tokio::spawn(async move {
            relay_request(
                Path("node-123".to_string()),
                State(relay_state),
                ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))),
                request,
            )
            .await
            .into_response()
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, protocol::REQUEST_HEADERS);

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header =
            protocol::FrameHeader::parse(&request_body).expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, protocol::REQUEST_BODY);

        let response_meta = protocol::ResponseMeta {
            status: 200,
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = protocol::encode_frame(
            request_header.stream_id,
            protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        state
            .hub
            .handle_proxy_frame(500, &mut response_headers_frame)
            .await;

        let mut response_body_frame = protocol::encode_frame(
            request_header.stream_id,
            protocol::RESPONSE_BODY,
            0,
            Bytes::new().as_ref(),
        );
        state
            .hub
            .handle_proxy_frame(500, &mut response_body_frame)
            .await;
        let mut response_end_frame =
            protocol::encode_frame(request_header.stream_id, protocol::STREAM_END, 0, &[]);
        state
            .hub
            .handle_proxy_frame(500, &mut response_end_frame)
            .await;

        let response = relay_task.await.expect("relay task should complete");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        assert!(body.is_empty());

        let rollout_entry = data
            .list_system_config_entries()
            .await
            .expect("system config list should succeed")
            .into_iter()
            .find(|entry| entry.key == "proxy_node_upgrade_rollout")
            .expect("rollout entry should exist");
        let tracked_nodes = rollout_entry.value["tracked_nodes"]
            .as_array()
            .expect("tracked nodes should be an array");
        assert_eq!(tracked_nodes.len(), 1);
        assert!(tracked_nodes[0]["version_confirmed_at_unix_secs"].is_u64());
        assert!(tracked_nodes[0]["traffic_confirmed_at_unix_secs"].is_u64());
    }

    #[tokio::test]
    async fn relay_strips_hop_by_hop_and_stale_length_headers_from_proxy_response() {
        let state = test_app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        state.hub.register_proxy(Arc::new(ProxyConn::new(
            501,
            "node-123".to_string(),
            "Node 123".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        )));

        let meta = protocol::RequestMeta {
            provider_id: None,
            endpoint_id: None,
            key_id: None,
            method: "GET".to_string(),
            url: "https://example.com/headers".to_string(),
            headers: HashMap::new(),
            stream: false,
            request_timeout_ms: None,
            stream_first_byte_timeout_ms: None,
            timeout: 30,
            follow_redirects: None,
            http1_only: false,
            transport_profile: None,
        };
        let request = Request::builder()
            .body(Body::from(encode_relay_envelope(&meta, &[])))
            .expect("request should build");

        let relay_state = state.clone();
        let relay_task = tokio::spawn(async move {
            relay_request(
                Path("node-123".to_string()),
                State(relay_state),
                ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4242))),
                request,
            )
            .await
            .into_response()
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, protocol::REQUEST_HEADERS);

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header =
            protocol::FrameHeader::parse(&request_body).expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, protocol::REQUEST_BODY);

        let response_meta = protocol::ResponseMeta {
            status: 200,
            headers: vec![
                ("content-length".to_string(), "999".to_string()),
                ("transfer-encoding".to_string(), "chunked".to_string()),
                ("connection".to_string(), "keep-alive".to_string()),
                ("content-type".to_string(), "text/plain".to_string()),
                (
                    "x-proxy-timing".to_string(),
                    "{\"mode\":\"tunnel\"}".to_string(),
                ),
            ],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = protocol::encode_frame(
            request_header.stream_id,
            protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        state
            .hub
            .handle_proxy_frame(501, &mut response_headers_frame)
            .await;

        let mut response_end_frame =
            protocol::encode_frame(request_header.stream_id, protocol::STREAM_END, 0, &[]);
        state
            .hub
            .handle_proxy_frame(501, &mut response_end_frame)
            .await;

        let response = relay_task.await.expect("relay task should complete");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("content-length").is_none());
        assert!(response.headers().get("transfer-encoding").is_none());
        assert!(response.headers().get("connection").is_none());
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("text/plain")
        );
        assert_eq!(
            response
                .headers()
                .get("x-proxy-timing")
                .and_then(|value| value.to_str().ok()),
            Some("{\"mode\":\"tunnel\"}")
        );
    }
}
