//! Upstream WebSocket handshake and frame conversion utilities.
//!
//! These helpers intentionally do not parse messages.  A protocol adapter is
//! responsible for deciding when and what to send, while this module owns the
//! HTTP-to-WebSocket transport conversion and provider transport profile.

use std::collections::BTreeMap;
use std::time::Duration;

use aether_contracts::ProxySnapshot;
use axum::extract::ws::{CloseFrame as AxumCloseFrame, Message as AxumWsMessage, WebSocket};
use axum::http::header::{
    ACCEPT, ACCEPT_ENCODING, CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HOST,
    PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use axum::http::{HeaderMap, HeaderName};
use futures_util::{SinkExt, TryFutureExt};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;
use wreq::ws::message::{CloseFrame as WreqCloseFrame, Message as WreqWsMessage};

use crate::ai_serving::AiExecutionDecision;
use crate::execution_runtime::transport::{
    build_browser_wreq_client, build_request_headers, normalize_execution_proxy_url,
    validate_execution_upstream_url, ExecutionSafeDnsResolver, ExecutionTransportControls,
};
use crate::frontdoor_loop_guard::gateway_frontdoor_self_loop_guard_error;
use crate::handlers::proxy::websocket::session::{
    WebSocketSessionLimits, RELAY_WRITE_TIMEOUT, TEARDOWN_WRITE_TIMEOUT,
};

#[derive(Clone, Copy)]
pub(crate) struct UpstreamWebSocketErrorCodes {
    pub(crate) upstream_url_missing: &'static str,
    pub(crate) upstream_url_invalid: &'static str,
    pub(crate) frontdoor_self_loop: &'static str,
    pub(crate) headers_invalid: &'static str,
    pub(crate) client_build_failed: &'static str,
    pub(crate) proxy_invalid: &'static str,
    pub(crate) tunnel_proxy_unsupported: &'static str,
    pub(crate) handshake_failed: &'static str,
    pub(crate) upgrade_rejected: &'static str,
    pub(crate) upgrade_failed: &'static str,
}

pub(crate) struct UpstreamWebSocketConnection {
    pub(crate) socket: wreq::ws::WebSocket,
    pub(crate) response_headers: BTreeMap<String, String>,
}

pub(crate) async fn connect_upstream_websocket(
    decision: &AiExecutionDecision,
    limits: WebSocketSessionLimits,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<UpstreamWebSocketConnection, &'static str> {
    let upstream_url = decision
        .upstream_url
        .as_deref()
        .ok_or(errors.upstream_url_missing)?;
    let upstream_url = guarded_websocket_upstream_url(
        upstream_url,
        errors.upstream_url_invalid,
        errors.frontdoor_self_loop,
    )?;
    let headers =
        websocket_handshake_headers(&decision.provider_request_headers, errors.headers_invalid)?;
    let client = build_websocket_client(decision, errors)?;
    let response = client
        .websocket(upstream_url.as_str())
        .headers(headers)
        .max_frame_size(limits.max_frame_size)
        .max_message_size(limits.max_message_size)
        .send()
        .await
        .map_err(|_| errors.handshake_failed)?;
    if response.status().as_u16() != 101 {
        return Err(errors.upgrade_rejected);
    }
    let response_headers = websocket_response_headers(response.headers());
    let socket = response
        .into_websocket()
        .await
        .map_err(|_| errors.upgrade_failed)?;
    Ok(UpstreamWebSocketConnection {
        socket,
        response_headers,
    })
}

fn guarded_websocket_upstream_url(
    raw: &str,
    invalid_code: &'static str,
    frontdoor_self_loop_code: &'static str,
) -> Result<Url, &'static str> {
    let upstream_url = websocket_upstream_url(raw, invalid_code)?;
    if gateway_frontdoor_self_loop_guard_error(upstream_url.as_str()).is_some() {
        return Err(frontdoor_self_loop_code);
    }
    Ok(upstream_url)
}

fn websocket_response_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    let connection_declared = aether_http::connection_declared_header_names(
        headers
            .get_all(http::header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok()),
    );
    headers
        .iter()
        .filter(|(name, _)| websocket_response_header_is_safe_to_retain(name))
        .filter_map(|(name, value)| {
            let normalized = name.as_str().to_ascii_lowercase();
            if crate::headers::should_skip_response_header(&normalized)
                || connection_declared.contains(&normalized)
            {
                return None;
            }
            value
                .to_str()
                .ok()
                .map(|value| (normalized, value.to_string()))
        })
        .collect()
}

fn websocket_response_header_is_safe_to_retain(name: &HeaderName) -> bool {
    !matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "www-authenticate"
            | "proxy-authenticate"
            | "authentication-info"
            | "proxy-authentication-info"
            | "cookie"
            | "set-cookie"
            | "set-cookie2"
            | "x-api-key"
            | "api-key"
            | "x-goog-api-key"
    )
}

pub(crate) fn websocket_upstream_url(
    raw: &str,
    invalid_code: &'static str,
) -> Result<Url, &'static str> {
    let mut url = Url::parse(raw).map_err(|_| invalid_code)?;
    let (http_scheme, websocket_scheme) = match url.scheme() {
        "https" | "wss" => ("https", "wss"),
        "http" | "ws" => ("http", "ws"),
        _ => return Err(invalid_code),
    };
    url.set_scheme(http_scheme).map_err(|_| invalid_code)?;
    let mut url = validate_execution_upstream_url(url.as_str()).map_err(|_| invalid_code)?;
    url.set_scheme(websocket_scheme).map_err(|_| invalid_code)?;
    Ok(url)
}

pub(crate) fn websocket_handshake_headers(
    provider_headers: &BTreeMap<String, String>,
    invalid_code: &'static str,
) -> Result<HeaderMap, &'static str> {
    // `build_request_headers` already strips `Connection` itself. Read the
    // dynamic hop-by-hop names from the source map first, otherwise a header
    // named by `Connection: keep-alive, x-provider-hop` would survive.
    let connection_scoped_names = provider_headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(CONNECTION.as_str()))
        .flat_map(|(_, value)| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect::<Vec<_>>();
    let mut headers =
        build_request_headers(provider_headers, None, false).map_err(|_| invalid_code)?;
    for name in connection_scoped_names {
        headers.remove(name);
    }
    for header in [
        ACCEPT,
        ACCEPT_ENCODING,
        CONNECTION,
        CONTENT_ENCODING,
        CONTENT_LENGTH,
        CONTENT_TYPE,
        HOST,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        headers.remove(header);
    }
    for header in ["keep-alive", "proxy-connection"] {
        headers.remove(header);
    }
    // The WebSocket client owns every Sec-WebSocket-* field, including
    // extensions introduced after this gateway was built.  Passing a
    // downstream handshake field through here can corrupt negotiation or
    // disclose the client's nonce/subprotocol to a different upstream.
    let websocket_managed_names = headers
        .keys()
        .filter(|name| name.as_str().starts_with("sec-websocket-"))
        .cloned()
        .collect::<Vec<_>>();
    for name in websocket_managed_names {
        headers.remove(name);
    }
    Ok(headers)
}

fn build_websocket_client(
    decision: &AiExecutionDecision,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<wreq::Client, &'static str> {
    let timeouts = websocket_timeouts(decision);
    let proxy_url = resolve_websocket_proxy_url(decision.proxy.as_ref(), errors)?;
    if let Some(profile) = decision.transport_profile.as_ref() {
        return build_browser_wreq_client(
            timeouts.as_ref(),
            decision.proxy.as_ref(),
            profile,
            ExecutionTransportControls::default(),
            false,
        )
        .map_err(|_| errors.client_build_failed);
    }

    let mut builder = wreq::Client::builder().no_proxy();
    if let Some(connect_ms) = timeouts.as_ref().and_then(|timeouts| timeouts.connect_ms) {
        builder = builder.connect_timeout(Duration::from_millis(connect_ms));
    }
    if let Some(proxy_url) = proxy_url {
        let proxy = wreq::Proxy::all(proxy_url).map_err(|_| errors.proxy_invalid)?;
        builder = builder.proxy(proxy);
    } else {
        builder = builder.dns_resolver(ExecutionSafeDnsResolver);
    }
    builder.build().map_err(|_| errors.client_build_failed)
}

fn resolve_websocket_proxy_url(
    proxy: Option<&ProxySnapshot>,
    errors: UpstreamWebSocketErrorCodes,
) -> Result<Option<String>, &'static str> {
    let Some(proxy) = proxy else {
        return Ok(None);
    };
    if proxy.enabled == Some(false) {
        return Ok(None);
    }
    if let Some(proxy_url) = proxy
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let parsed = Url::parse(proxy_url).map_err(|_| errors.proxy_invalid)?;
        if !matches!(
            parsed.scheme().to_ascii_lowercase().as_str(),
            "http" | "https" | "socks5" | "socks5h"
        ) || parsed.host_str().is_none()
            || !matches!(parsed.path(), "" | "/")
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(errors.proxy_invalid);
        }
        // Manual proxy nodes bind credentials to the node identity before a
        // snapshot reaches this path. Reject userinfo on an otherwise
        // unbound snapshot so an arbitrary decision cannot smuggle proxy
        // credentials through a URL; preserve the established node-auth URL
        // form for authenticated manual proxy nodes.
        if (!parsed.username().is_empty() || parsed.password().is_some()) && proxy.node_id.is_none()
        {
            return Err(errors.proxy_invalid);
        }
        let normalized =
            normalize_execution_proxy_url(proxy_url).map_err(|_| errors.proxy_invalid)?;
        return Ok(Some(normalized));
    }
    if proxy.node_id.is_some() || proxy.mode.as_deref() == Some("tunnel") {
        return Err(errors.tunnel_proxy_unsupported);
    }
    Err(errors.proxy_invalid)
}

pub(crate) fn websocket_timeouts(
    decision: &AiExecutionDecision,
) -> Option<aether_contracts::ExecutionTimeouts> {
    let mut timeouts = decision.timeouts.clone()?;
    timeouts.read_ms = None;
    timeouts.first_byte_ms = None;
    timeouts.total_ms = None;
    Some(timeouts)
}

/// Why a frame did not reach its peer.  A timeout is reported separately from
/// a socket error because the two describe different peers: one has gone away,
/// the other is still connected but has stopped reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSocketWriteError {
    Failed,
    TimedOut,
    Cancelled,
}

impl WebSocketWriteError {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "write_failed",
            Self::TimedOut => "write_timeout",
            Self::Cancelled => "write_cancelled",
        }
    }
}

/// A small per-direction buffer keeps a slow reader from blocking the opposite
/// WebSocket direction while still applying bounded backpressure. At the Live
/// audio cadence this is deliberately only a short burst buffer, not a place
/// where a session can accumulate unbounded media.
pub(crate) const RELAY_FRAME_QUEUE_CAPACITY: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebSocketRelayQueueError {
    Closed,
    Cancelled,
}

/// Shared cancellation for both read/write halves of a bidirectional relay.
///
/// Queue admission and socket writes both observe this token, so a connection
/// deadline or lease loss can interrupt a full queue and an in-flight slow
/// write immediately instead of waiting for [`RELAY_WRITE_TIMEOUT`].
#[derive(Clone, Default)]
pub(crate) struct WebSocketRelayPumpControl {
    cancellation: CancellationToken,
}

impl WebSocketRelayPumpControl {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(crate) async fn cancelled(&self) {
        self.cancellation.cancelled().await;
    }

    pub(crate) async fn enqueue<T>(
        &self,
        sender: &mpsc::Sender<T>,
        message: T,
    ) -> Result<(), WebSocketRelayQueueError> {
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(WebSocketRelayQueueError::Cancelled),
            result = sender.send(message) => {
                result.map_err(|_| WebSocketRelayQueueError::Closed)
            }
        }
    }

    pub(crate) async fn send<F>(&self, write: F) -> Result<(), WebSocketWriteError>
    where
        F: std::future::Future<Output = Result<(), ()>>,
    {
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(WebSocketWriteError::Cancelled),
            result = bounded_send(RELAY_WRITE_TIMEOUT, write) => result,
        }
    }
}

pub(crate) fn websocket_relay_frame_queue<T>() -> (mpsc::Sender<T>, mpsc::Receiver<T>) {
    mpsc::channel(RELAY_FRAME_QUEUE_CAPACITY)
}

/// Relays one frame to the client under [`RELAY_WRITE_TIMEOUT`].
pub(crate) async fn send_client_message(
    client_socket: &mut WebSocket,
    message: AxumWsMessage,
) -> Result<(), WebSocketWriteError> {
    bounded_send(
        RELAY_WRITE_TIMEOUT,
        client_socket.send(message).map_err(|_| ()),
    )
    .await
}

/// Sends one frame to the upstream under [`RELAY_WRITE_TIMEOUT`].
pub(crate) async fn send_upstream_message(
    upstream: &mut wreq::ws::WebSocket,
    message: WreqWsMessage,
) -> Result<(), WebSocketWriteError> {
    bounded_send(RELAY_WRITE_TIMEOUT, upstream.send(message).map_err(|_| ())).await
}

/// Queues one frame in the upstream sink without flushing it. Completion means
/// `start_send` succeeded, so callers must conservatively treat the frame as
/// possibly delivered even when a later flush fails or is cancelled.
pub(crate) async fn feed_upstream_message(
    upstream: &mut wreq::ws::WebSocket,
    message: WreqWsMessage,
) -> Result<(), WebSocketWriteError> {
    bounded_send(RELAY_WRITE_TIMEOUT, upstream.feed(message).map_err(|_| ())).await
}

/// Flushes frames previously queued with [`feed_upstream_message`].
pub(crate) async fn flush_upstream_messages(
    upstream: &mut wreq::ws::WebSocket,
) -> Result<(), WebSocketWriteError> {
    bounded_send(RELAY_WRITE_TIMEOUT, upstream.flush().map_err(|_| ())).await
}

/// Best-effort teardown write.  The caller is already ending the session, so
/// the outcome only matters for keeping the wait bounded.
async fn send_teardown_message<F>(write: F)
where
    F: std::future::Future<Output = Result<(), ()>>,
{
    let _ = bounded_send(TEARDOWN_WRITE_TIMEOUT, write).await;
}

async fn bounded_send<F>(budget: Duration, write: F) -> Result<(), WebSocketWriteError>
where
    F: std::future::Future<Output = Result<(), ()>>,
{
    match tokio::time::timeout(budget, write).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(())) => Err(WebSocketWriteError::Failed),
        Err(_) => Err(WebSocketWriteError::TimedOut),
    }
}

/// Sends a WebSocket Close frame upstream without waiting on an unresponsive
/// provider.  The socket is dropped by the caller either way.
pub(crate) async fn close_upstream_socket(
    upstream: &mut wreq::ws::WebSocket,
    frame: Option<WreqCloseFrame>,
) {
    send_teardown_message(upstream.send(WreqWsMessage::Close(frame)).map_err(|_| ())).await;
}

pub(crate) fn upstream_message_to_client(message: WreqWsMessage) -> AxumWsMessage {
    match message {
        WreqWsMessage::Text(text) => AxumWsMessage::Text(text.to_string().into()),
        WreqWsMessage::Binary(data) => AxumWsMessage::Binary(data),
        WreqWsMessage::Ping(data) => AxumWsMessage::Ping(data),
        WreqWsMessage::Pong(data) => AxumWsMessage::Pong(data),
        WreqWsMessage::Close(frame) => AxumWsMessage::Close(frame.map(|frame| AxumCloseFrame {
            code: frame.code.into(),
            reason: frame.reason.to_string().into(),
        })),
    }
}

pub(crate) fn client_message_to_upstream(message: AxumWsMessage) -> WreqWsMessage {
    match message {
        AxumWsMessage::Text(text) => WreqWsMessage::Text(text.to_string().into()),
        AxumWsMessage::Binary(data) => WreqWsMessage::Binary(data),
        AxumWsMessage::Ping(data) => WreqWsMessage::Ping(data),
        AxumWsMessage::Pong(data) => WreqWsMessage::Pong(data),
        AxumWsMessage::Close(frame) => WreqWsMessage::Close(frame.map(|frame| WreqCloseFrame {
            code: frame.code.into(),
            reason: frame.reason.to_string().into(),
        })),
    }
}

/// Builds a Responses WebSocket error event in the shape understood by the
/// official client implementations.  The status is part of the event body,
/// not the WebSocket handshake, because the connection is already upgraded.
pub(crate) fn responses_websocket_error_event(
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
) -> serde_json::Value {
    responses_websocket_error_event_with_stream_id(status, error_type, code, message, None)
}

/// Builds a request-scoped Responses error. Callers must supply `stream_id`
/// only after validating the protocol's named-lane grammar; untrusted or
/// malformed identifiers must never be reflected into a provider event.
pub(crate) fn responses_websocket_error_event_with_stream_id(
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
    stream_id: Option<&str>,
) -> serde_json::Value {
    let mut event = json!({
        "type": "error",
        "status": status,
        "error": {
            "type": error_type,
            "code": code,
            "message": message,
        },
    });
    if let Some(stream_id) = stream_id {
        event
            .as_object_mut()
            .expect("Responses error events are JSON objects")
            .insert(
                "stream_id".to_string(),
                serde_json::Value::String(stream_id.to_string()),
            );
    }
    event
}

pub(crate) async fn send_responses_websocket_error(
    client_socket: &mut WebSocket,
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
) {
    send_responses_websocket_error_with_stream_id(
        client_socket,
        status,
        error_type,
        code,
        message,
        None,
    )
    .await;
}

/// Sends a standard invalid-request error with a bounded, server-owned
/// parameter name. This is used for protocol fields such as
/// `previous_response_id`; no untrusted value is reflected.
pub(crate) async fn send_responses_websocket_error_with_param(
    client_socket: &mut WebSocket,
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
    param: &'static str,
) {
    let mut event = responses_websocket_error_event(status, error_type, code, message);
    event["error"]["param"] = serde_json::Value::String(param.to_string());
    send_teardown_message(
        client_socket
            .send(AxumWsMessage::Text(event.to_string().into()))
            .map_err(|_| ()),
    )
    .await;
}

pub(crate) async fn send_responses_websocket_error_with_stream_id(
    client_socket: &mut WebSocket,
    status: u16,
    error_type: &str,
    code: &str,
    message: &str,
    stream_id: Option<&str>,
) {
    let event = responses_websocket_error_event_with_stream_id(
        status, error_type, code, message, stream_id,
    );
    send_teardown_message(
        client_socket
            .send(AxumWsMessage::Text(event.to_string().into()))
            .map_err(|_| ()),
    )
    .await;
}

pub(crate) async fn send_gateway_error(client_socket: &mut WebSocket, code: &str, message: &str) {
    send_gateway_error_with_status(client_socket, 400, code, message).await;
}

pub(crate) async fn send_gateway_error_with_stream_id(
    client_socket: &mut WebSocket,
    code: &str,
    message: &str,
    stream_id: Option<&str>,
) {
    send_gateway_error_with_status_and_stream_id(client_socket, 400, code, message, stream_id)
        .await;
}

pub(crate) async fn send_gateway_error_with_status(
    client_socket: &mut WebSocket,
    status: u16,
    code: &str,
    message: &str,
) {
    send_gateway_error_with_status_and_stream_id(client_socket, status, code, message, None).await;
}

pub(crate) async fn send_gateway_error_with_status_and_stream_id(
    client_socket: &mut WebSocket,
    status: u16,
    code: &str,
    message: &str,
    stream_id: Option<&str>,
) {
    send_responses_websocket_error_with_stream_id(
        client_socket,
        status,
        "gateway_error",
        code,
        message,
        stream_id,
    )
    .await;
}

pub(crate) async fn close_client_socket(client_socket: &mut WebSocket, code: u16, reason: &str) {
    send_teardown_message(
        client_socket
            .send(AxumWsMessage::Close(Some(AxumCloseFrame {
                code,
                reason: reason.to_string().into(),
            })))
            .map_err(|_| ()),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_send, build_websocket_client, guarded_websocket_upstream_url,
        resolve_websocket_proxy_url, responses_websocket_error_event,
        responses_websocket_error_event_with_stream_id, websocket_handshake_headers,
        websocket_relay_frame_queue, websocket_response_headers, websocket_upstream_url,
        UpstreamWebSocketErrorCodes, WebSocketRelayPumpControl, WebSocketRelayQueueError,
        WebSocketWriteError, RELAY_FRAME_QUEUE_CAPACITY, RELAY_WRITE_TIMEOUT,
        TEARDOWN_WRITE_TIMEOUT,
    };
    use crate::ai_serving::AiExecutionDecision;
    use crate::frontdoor_loop_guard::configured_gateway_frontdoor_base_url;
    use aether_contracts::{ProxySnapshot, ResolvedTransportProfile};
    use axum::http::HeaderMap;
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[tokio::test]
    async fn a_peer_that_never_drains_its_window_times_out_instead_of_pinning_the_relay() {
        let stalled = std::future::pending::<Result<(), ()>>();

        let outcome = bounded_send(Duration::from_millis(1), stalled).await;

        assert_eq!(outcome, Err(WebSocketWriteError::TimedOut));
    }

    #[tokio::test]
    async fn a_socket_error_is_reported_separately_from_a_stalled_peer() {
        let outcome = bounded_send(RELAY_WRITE_TIMEOUT, std::future::ready(Err(()))).await;

        assert_eq!(outcome, Err(WebSocketWriteError::Failed));
        assert_eq!(WebSocketWriteError::Failed.as_str(), "write_failed");
        assert_eq!(WebSocketWriteError::TimedOut.as_str(), "write_timeout");
        assert_eq!(WebSocketWriteError::Cancelled.as_str(), "write_cancelled");
    }

    #[tokio::test]
    async fn relay_frame_queue_is_bounded_and_fifo() {
        let (sender, mut receiver) = websocket_relay_frame_queue();
        for frame in 0..RELAY_FRAME_QUEUE_CAPACITY {
            sender
                .try_send(frame)
                .expect("the configured burst buffer should accept this frame");
        }
        assert!(matches!(
            sender.try_send(RELAY_FRAME_QUEUE_CAPACITY),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
        for expected in 0..RELAY_FRAME_QUEUE_CAPACITY {
            assert_eq!(receiver.recv().await, Some(expected));
        }
    }

    #[tokio::test]
    async fn relay_cancellation_interrupts_a_full_queue_without_waiting_for_capacity() {
        let control = WebSocketRelayPumpControl::new();
        let (sender, _receiver) = websocket_relay_frame_queue();
        for frame in 0..RELAY_FRAME_QUEUE_CAPACITY {
            sender.try_send(frame).expect("queue should fill exactly");
        }
        let enqueue = control.enqueue(&sender, RELAY_FRAME_QUEUE_CAPACITY);
        tokio::pin!(enqueue);
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut enqueue)
            .await
            .is_err());

        control.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(100), enqueue)
                .await
                .expect("cancellation should wake a blocked producer"),
            Err(WebSocketRelayQueueError::Cancelled)
        );
    }

    #[tokio::test]
    async fn relay_cancellation_interrupts_a_stalled_socket_write() {
        let control = WebSocketRelayPumpControl::new();
        let write = control.send(std::future::pending::<Result<(), ()>>());
        tokio::pin!(write);
        assert!(tokio::time::timeout(Duration::from_millis(5), &mut write)
            .await
            .is_err());

        control.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(100), write)
                .await
                .expect("cancellation should wake a stalled writer"),
            Err(WebSocketWriteError::Cancelled)
        );
    }

    #[tokio::test]
    async fn a_write_that_completes_within_its_budget_succeeds() {
        let outcome = bounded_send(RELAY_WRITE_TIMEOUT, std::future::ready(Ok::<(), ()>(()))).await;

        assert_eq!(outcome, Ok(()));
    }

    #[test]
    fn teardown_writes_are_given_a_shorter_budget_than_relayed_frames() {
        assert!(TEARDOWN_WRITE_TIMEOUT < RELAY_WRITE_TIMEOUT);
    }

    #[test]
    fn upstream_handshake_observability_drops_credential_bearing_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-primary-used-percent", "10".parse().unwrap());
        headers.insert("x-request-id", "request-123".parse().unwrap());
        headers.insert("set-cookie", "session=secret".parse().unwrap());
        headers.insert("www-authenticate", "Bearer secret".parse().unwrap());
        headers.insert("authentication-info", "nextnonce=secret".parse().unwrap());

        let retained = websocket_response_headers(&headers);

        assert_eq!(
            retained
                .get("x-codex-primary-used-percent")
                .map(String::as_str),
            Some("10")
        );
        assert_eq!(
            retained.get("x-request-id").map(String::as_str),
            Some("request-123")
        );
        assert!(!retained.contains_key("set-cookie"));
        assert!(!retained.contains_key("www-authenticate"));
        assert!(!retained.contains_key("authentication-info"));
    }

    #[test]
    fn builds_a_client_compatible_responses_error_event() {
        let event = responses_websocket_error_event(
            400,
            "invalid_request_error",
            "previous_response_not_found",
            "Previous response was not found.",
        );

        assert_eq!(event["type"], "error");
        assert_eq!(event["status"], 400);
        assert_eq!(event["error"]["type"], "invalid_request_error");
        assert_eq!(event["error"]["code"], "previous_response_not_found");
        assert_eq!(
            event["error"]["message"],
            "Previous response was not found."
        );
        assert!(event.get("stream_id").is_none());
    }

    #[test]
    fn request_scoped_responses_errors_include_the_validated_named_stream() {
        let event = responses_websocket_error_event_with_stream_id(
            400,
            "gateway_error",
            "responses_websocket_named_stream_unsupported",
            "Named streams are not supported.",
            Some("main-lane_1.test"),
        );

        assert_eq!(event["stream_id"], "main-lane_1.test");
        assert_eq!(
            event["error"]["code"],
            "responses_websocket_named_stream_unsupported"
        );
    }

    #[test]
    fn maps_http_url_to_websocket_url_without_losing_path_or_query() {
        for (http_scheme, websocket_scheme) in [("https", "wss"), ("http", "ws")] {
            let url = websocket_upstream_url(
                &format!("{http_scheme}://example.test:8080/backend-api/codex/responses?x=1"),
                "invalid",
            )
            .expect("URL should be converted");
            assert_eq!(
                url.as_str(),
                format!("{websocket_scheme}://example.test:8080/backend-api/codex/responses?x=1")
            );
        }
    }

    #[test]
    fn rejects_upstream_url_with_credentials() {
        assert!(websocket_upstream_url("https://token@example.test/responses", "invalid").is_err());
    }

    #[test]
    fn websocket_upstream_url_accepts_ws_and_wss_with_safe_targets() {
        for allowed in [
            "wss://example.test/v1/responses",
            "https://example.test/v1/responses",
            "ws://example.test:8080/v1/responses",
            "http://example.test:8080/v1/responses",
            "http://8.8.8.8:8080/v1/responses",
            "wss://8.8.8.8/v1/responses",
            "ws://[2606:4700:4700::1111]:8080/v1/responses",
            "wss://[2606:4700:4700::1111]/v1/responses",
            "ws://localhost:8080/v1/responses",
            "http://127.42.0.1:8080/v1/responses",
            "ws://[::1]:8080/v1/responses",
        ] {
            assert!(
                websocket_upstream_url(allowed, "invalid").is_ok(),
                "{allowed}"
            );
        }
        for rejected in [
            "http://10.0.0.1/v1/responses",
            "wss://10.0.0.1/v1/responses",
            "wss://127.0.0.1/v1/responses",
            "wss://[::1]/v1/responses",
            "wss://[fd00::1]/v1/responses",
            "wss://[::ffff:127.0.0.1]/v1/responses",
            "wss://169.254.169.254/v1/responses",
            "wss://198.18.78.41/v1/responses",
            "wss://198.19.1.2/v1/responses",
            "ws://0.0.0.0:8080/v1/responses",
            "ws://[::ffff:127.0.0.1]:8080/v1/responses",
            "wss://example.test/v1/responses#secret",
            "ws://example.test/v1/responses#secret",
            "http://token@example.test/v1/responses",
            "ws://token@example.test/v1/responses",
            "ftp://example.test/v1/responses",
        ] {
            assert!(
                websocket_upstream_url(rejected, "invalid").is_err(),
                "{rejected}"
            );
        }
    }

    #[tokio::test]
    async fn websocket_client_build_defers_provider_dns_for_all_transport_profiles() {
        let errors = UpstreamWebSocketErrorCodes {
            upstream_url_missing: "missing",
            upstream_url_invalid: "upstream_invalid",
            frontdoor_self_loop: "frontdoor_self_loop",
            headers_invalid: "headers_invalid",
            client_build_failed: "client_build_failed",
            proxy_invalid: "proxy_invalid",
            tunnel_proxy_unsupported: "tunnel_unsupported",
            handshake_failed: "handshake_failed",
            upgrade_rejected: "upgrade_rejected",
            upgrade_failed: "upgrade_failed",
        };
        for profile in [
            None,
            Some(ResolvedTransportProfile {
                profile_id: "chrome136".to_string(),
                backend: aether_contracts::TRANSPORT_BACKEND_BROWSER_WREQ.to_string(),
                ..Default::default()
            }),
        ] {
            for proxy in [
                None,
                Some(ProxySnapshot {
                    enabled: Some(false),
                    url: Some("http://proxy.invalid:8080".to_string()),
                    ..Default::default()
                }),
                Some(ProxySnapshot {
                    enabled: Some(true),
                    url: Some("http://proxy.invalid:8080".to_string()),
                    ..Default::default()
                }),
                Some(ProxySnapshot {
                    enabled: Some(true),
                    url: Some("socks5h://proxy.invalid:1080".to_string()),
                    ..Default::default()
                }),
            ] {
                let mut decision: AiExecutionDecision = serde_json::from_value(serde_json::json!({
                    "action": "proxy",
                    "upstream_url": "wss://upstream.invalid/v1/responses"
                }))
                .expect("minimal provider decision should deserialize");
                decision.transport_profile = profile.clone();
                decision.proxy = proxy;

                build_websocket_client(&decision, errors)
                    .expect("building a client must not resolve the provider or proxy hostname");
            }
        }
    }

    #[test]
    fn active_websocket_proxy_without_a_target_fails_closed() {
        let errors = UpstreamWebSocketErrorCodes {
            upstream_url_missing: "missing",
            upstream_url_invalid: "upstream_invalid",
            frontdoor_self_loop: "frontdoor_self_loop",
            headers_invalid: "headers_invalid",
            client_build_failed: "client_build_failed",
            proxy_invalid: "proxy_invalid",
            tunnel_proxy_unsupported: "tunnel_unsupported",
            handshake_failed: "handshake_failed",
            upgrade_rejected: "upgrade_rejected",
            upgrade_failed: "upgrade_failed",
        };
        let missing = ProxySnapshot {
            enabled: Some(true),
            ..ProxySnapshot::default()
        };
        assert_eq!(
            resolve_websocket_proxy_url(Some(&missing), errors),
            Err("proxy_invalid")
        );

        let tunnel = ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".to_string()),
            ..ProxySnapshot::default()
        };
        assert_eq!(
            resolve_websocket_proxy_url(Some(&tunnel), errors),
            Err("tunnel_unsupported")
        );
    }

    #[tokio::test]
    async fn websocket_handshake_keeps_provider_dns_remote_for_http_and_socks_proxies() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let errors = UpstreamWebSocketErrorCodes {
            upstream_url_missing: "missing",
            upstream_url_invalid: "upstream_invalid",
            frontdoor_self_loop: "frontdoor_self_loop",
            headers_invalid: "headers_invalid",
            client_build_failed: "client_build_failed",
            proxy_invalid: "proxy_invalid",
            tunnel_proxy_unsupported: "tunnel_unsupported",
            handshake_failed: "handshake_failed",
            upgrade_rejected: "upgrade_rejected",
            upgrade_failed: "upgrade_failed",
        };
        for profile in [
            None,
            Some(ResolvedTransportProfile {
                profile_id: "chrome136".to_string(),
                backend: aether_contracts::TRANSPORT_BACKEND_BROWSER_WREQ.to_string(),
                ..Default::default()
            }),
        ] {
            for scheme in ["http", "socks5", "socks5h"] {
                let listener = crate::test_support::bind_loopback_listener().await.unwrap();
                let proxy_addr = listener.local_addr().unwrap();
                let (release, released) = tokio::sync::oneshot::channel::<()>();
                let server = tokio::spawn(async move {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    if scheme != "http" {
                        let mut greeting = [0; 2];
                        stream.read_exact(&mut greeting).await.unwrap();
                        assert_eq!(greeting[0], 5);
                        let mut methods = vec![0; greeting[1] as usize];
                        stream.read_exact(&mut methods).await.unwrap();
                        assert!(methods.contains(&0));
                        stream.write_all(&[5, 0]).await.unwrap();

                        let mut request = [0; 4];
                        stream.read_exact(&mut request).await.unwrap();
                        assert_eq!(
                            request,
                            [5, 1, 0, 3],
                            "proxy must receive a domain, not an IP"
                        );
                        let host_len = stream.read_u8().await.unwrap();
                        let mut host = vec![0; host_len as usize];
                        stream.read_exact(&mut host).await.unwrap();
                        assert_eq!(host, b"provider-dns.invalid");
                        assert_eq!(stream.read_u16().await.unwrap(), 80);
                        stream
                            .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                            .await
                            .unwrap();
                    }
                    let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let _ = released.await;
                    drop(socket);
                });
                let mut decision: AiExecutionDecision = serde_json::from_value(serde_json::json!({
                    "action": "proxy",
                    "upstream_url": "ws://provider-dns.invalid/v1/responses",
                    "proxy": {"enabled": true, "url": format!("{scheme}://{proxy_addr}")}
                }))
                .unwrap();
                decision.transport_profile = profile.clone();
                let connection = tokio::time::timeout(
                    Duration::from_secs(5),
                    super::connect_upstream_websocket(
                        &decision,
                        crate::handlers::proxy::websocket::session::RESPONSES_WEBSOCKET_SESSION_LIMITS,
                        errors,
                    ),
                )
                .await
                .expect("proxied handshake must not wait for local provider DNS")
                .unwrap_or_else(|error| panic!("{scheme} handshake failed: {error}"));
                release.send(()).unwrap();
                tokio::time::timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap();
                drop(connection);
            }
        }
    }

    #[test]
    fn rejects_responses_websocket_frontdoor_self_loop_before_connecting() {
        let base_url = configured_gateway_frontdoor_base_url();
        let raw_url = format!("{base_url}/v1/responses");

        assert_eq!(
            guarded_websocket_upstream_url(
                raw_url.as_str(),
                "responses_upstream_url_invalid",
                "responses_websocket_frontdoor_self_loop",
            ),
            Err("responses_websocket_frontdoor_self_loop")
        );
    }

    #[test]
    fn websocket_proxy_url_must_be_an_allowed_origin() {
        let errors = UpstreamWebSocketErrorCodes {
            upstream_url_missing: "missing",
            upstream_url_invalid: "upstream_invalid",
            frontdoor_self_loop: "frontdoor_self_loop",
            headers_invalid: "headers_invalid",
            client_build_failed: "client_build_failed",
            proxy_invalid: "proxy_invalid",
            tunnel_proxy_unsupported: "tunnel_unsupported",
            handshake_failed: "handshake_failed",
            upgrade_rejected: "upgrade_rejected",
            upgrade_failed: "upgrade_failed",
        };

        for value in [
            "file:///tmp/proxy",
            "http://proxy.example:8080/path",
            "http://proxy.example:8080?token=secret",
            "http://proxy.example:8080#fragment",
            "http://alice:password@proxy.example:8080",
        ] {
            let proxy = ProxySnapshot {
                enabled: Some(true),
                url: Some(value.to_string()),
                ..ProxySnapshot::default()
            };
            assert_eq!(
                resolve_websocket_proxy_url(Some(&proxy), errors),
                Err("proxy_invalid"),
                "proxy URL should be rejected: {value}"
            );
        }

        let authenticated_node = ProxySnapshot {
            enabled: Some(true),
            node_id: Some("manual-node-1".to_string()),
            url: Some("http://alice:password@proxy.example:8080".to_string()),
            ..ProxySnapshot::default()
        };
        assert_eq!(
            resolve_websocket_proxy_url(Some(&authenticated_node), errors),
            Ok(Some(
                "http://alice:password@proxy.example:8080/".to_string()
            ))
        );

        let socks = ProxySnapshot {
            enabled: Some(true),
            url: Some("socks5://proxy.example:1080".to_string()),
            ..ProxySnapshot::default()
        };
        assert_eq!(
            resolve_websocket_proxy_url(Some(&socks), errors),
            Ok(Some("socks5h://proxy.example:1080".to_string()))
        );
    }

    #[test]
    fn rejects_live_direct_and_sideband_frontdoor_self_loops_before_connecting() {
        let base_url = configured_gateway_frontdoor_base_url();

        for path in ["/v1/live", "/v1/live/rtc_test"] {
            let raw_url = format!("{base_url}{path}");
            assert_eq!(
                guarded_websocket_upstream_url(
                    raw_url.as_str(),
                    "codex_live_upstream_url_invalid",
                    "codex_live_websocket_frontdoor_self_loop",
                ),
                Err("codex_live_websocket_frontdoor_self_loop"),
                "{path} must be rejected before an upstream handshake"
            );
        }
    }

    #[test]
    fn upstream_handshake_keeps_provider_auth_but_drops_transport_managed_headers() {
        let provider_headers = BTreeMap::from([
            (
                "authorization".to_string(),
                "Bearer provider-token".to_string(),
            ),
            ("x-api-key".to_string(), "provider-api-key".to_string()),
            (
                "cookie".to_string(),
                "provider_session=provider-cookie".to_string(),
            ),
            (
                "connection".to_string(),
                "keep-alive, x-provider-hop".to_string(),
            ),
            ("x-provider-hop".to_string(), "must-not-pass".to_string()),
            ("upgrade".to_string(), "websocket".to_string()),
            (
                "sec-websocket-key".to_string(),
                "downstream-nonce".to_string(),
            ),
            (
                "sec-websocket-future-field".to_string(),
                "future-value".to_string(),
            ),
            (
                "proxy-authorization".to_string(),
                "Basic must-not-pass".to_string(),
            ),
            ("x-provider-header".to_string(), "safe".to_string()),
        ]);

        let headers = websocket_handshake_headers(&provider_headers, "invalid")
            .expect("provider headers should be valid");

        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer provider-token")
        );
        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("provider-api-key")
        );
        assert_eq!(
            headers.get("cookie").and_then(|value| value.to_str().ok()),
            Some("provider_session=provider-cookie")
        );
        assert_eq!(
            headers
                .get("x-provider-header")
                .and_then(|value| value.to_str().ok()),
            Some("safe")
        );
        for name in [
            "connection",
            "x-provider-hop",
            "upgrade",
            "sec-websocket-key",
            "sec-websocket-future-field",
            "proxy-authorization",
        ] {
            assert!(headers.get(name).is_none(), "{name} must not survive");
        }
    }
}
