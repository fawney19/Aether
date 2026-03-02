//! Per-stream request handler.
//!
//! Receives request frames, executes the upstream HTTP request,
//! and sends response frames back through the writer channel.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::state::{AppState, ServerContext};
use crate::target_filter;

use super::protocol::{
    compress_payload, decompress_if_gzip, flags, Frame, MsgType, RequestMeta, ResponseMeta,
};
use super::writer::FrameSender;

/// Maximum response body chunk size per frame (32 KB).
const MAX_CHUNK_SIZE: usize = 32 * 1024;

/// Timeout for sending a single frame to the writer channel.
/// If the writer is congested (TCP backpressure), we abandon the stream
/// rather than blocking indefinitely and exhausting the stream pool.
const FRAME_SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Minimum allowed upstream request timeout (seconds).
const MIN_TIMEOUT_SECS: u64 = 5;
/// Maximum allowed upstream request timeout (seconds).
const MAX_TIMEOUT_SECS: u64 = 300;

/// Headers that must not be forwarded to upstream (hop-by-hop or security-sensitive).
const BLOCKED_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Handle a single stream: receive body, execute upstream, send response.
pub async fn handle_stream(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    stream_id: u32,
    meta: RequestMeta,
    mut body_rx: mpsc::Receiver<Frame>,
    frame_tx: FrameSender,
) {
    server.active_connections.fetch_add(1, Ordering::Release);

    let connect_elapsed =
        handle_stream_inner(&state, &server, stream_id, meta, &mut body_rx, &frame_tx).await;

    server.active_connections.fetch_sub(1, Ordering::Release);
    if let Some(d) = connect_elapsed {
        server.metrics.record_request(d);
    }
}

/// Send a frame to the writer with a timeout. Returns false if send failed.
async fn send_frame(tx: &FrameSender, frame: Frame) -> bool {
    match tokio::time::timeout(FRAME_SEND_TIMEOUT, tx.send(frame)).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => {
            // Channel closed (writer exited)
            false
        }
        Err(_) => {
            // Timeout — writer is congested
            warn!("frame send timeout (writer congested), abandoning stream");
            false
        }
    }
}

/// Returns the connection-establishment duration (DNS + TCP/TLS + TTFB) if the
/// upstream request succeeded, or `None` if the request never reached the
/// response-headers stage.
async fn handle_stream_inner(
    state: &AppState,
    server: &ServerContext,
    stream_id: u32,
    meta: RequestMeta,
    body_rx: &mut mpsc::Receiver<Frame>,
    frame_tx: &FrameSender,
) -> Option<Duration> {
    // Collect request body
    let mut body_parts: Vec<Bytes> = Vec::new();
    let mut body_done = false;

    // Drain body frames
    while !body_done {
        match body_rx.recv().await {
            Some(frame) => {
                if frame.msg_type == MsgType::RequestBody {
                    let payload = match decompress_if_gzip(&frame) {
                        Ok(d) => d,
                        Err(e) => {
                            send_error(
                                frame_tx,
                                stream_id,
                                &format!("gzip decompress failed: {e}"),
                            )
                            .await;
                            return None;
                        }
                    };
                    if !payload.is_empty() {
                        body_parts.push(payload);
                    }
                    if frame.is_end_stream() {
                        body_done = true;
                    }
                } else if frame.msg_type == MsgType::StreamEnd
                    || frame.msg_type == MsgType::StreamError
                {
                    body_done = true;
                    if frame.msg_type == MsgType::StreamError {
                        return None; // Client cancelled
                    }
                }
            }
            None => return None, // Channel closed
        }
    }

    let body: Bytes = if body_parts.is_empty() {
        Bytes::new()
    } else if body_parts.len() == 1 {
        body_parts.into_iter().next().unwrap()
    } else {
        let total: usize = body_parts.iter().map(|b| b.len()).sum();
        let mut combined = Vec::with_capacity(total);
        for part in &body_parts {
            combined.extend_from_slice(part);
        }
        Bytes::from(combined)
    };

    // Validate target
    let target_url = match url::Url::parse(&meta.url) {
        Ok(u) => u,
        Err(e) => {
            send_error(frame_tx, stream_id, &format!("invalid URL: {e}")).await;
            return None;
        }
    };

    // Only allow http/https schemes (block file://, data://, etc.)
    match target_url.scheme() {
        "http" | "https" => {}
        other => {
            send_error(
                frame_tx,
                stream_id,
                &format!("unsupported URL scheme: {other}"),
            )
            .await;
            return None;
        }
    }

    let host = match target_url.host_str() {
        Some(h) => h.to_string(),
        None => {
            send_error(frame_tx, stream_id, "missing host in URL").await;
            return None;
        }
    };
    let port = target_url.port_or_known_default().unwrap_or(443);

    // DNS + target validation (populates dns_cache for SafeDnsResolver)
    let connect_start = Instant::now();
    {
        let allowed_ports = Arc::clone(&server.dynamic.load().allowed_ports);
        if let Err(e) =
            target_filter::validate_target(&host, port, &allowed_ports, &state.dns_cache).await
        {
            server.metrics.dns_failures.fetch_add(1, Ordering::Release);
            send_error(frame_tx, stream_id, &format!("target blocked: {e}")).await;
            return None;
        }
    }
    let dns_ms = connect_start.elapsed().as_millis() as u64;

    // Execute upstream request
    let client = &state.reqwest_client;
    let timeout = Duration::from_secs(meta.timeout.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS));

    let method: reqwest::Method = meta.method.parse().unwrap_or(reqwest::Method::GET);
    // Build a complete HeaderMap from tunnel headers, then set it all at once
    // via .headers() which *replaces* reqwest defaults (e.g. Accept: */*),
    // ensuring upstream sees exactly what Aether server intended.
    let mut header_map = reqwest::header::HeaderMap::with_capacity(meta.headers.len());
    for (k, v) in &meta.headers {
        let k_lower = k.to_ascii_lowercase();
        if BLOCKED_HEADERS.contains(&k_lower.as_str()) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_bytes()),
            reqwest::header::HeaderValue::from_str(v),
        ) {
            header_map.insert(name, value);
        }
    }
    let mut req = client.request(method, &meta.url).headers(header_map);
    let body_size = body.len();
    if !body.is_empty() {
        req = req.body(body);
    }
    req = req.timeout(timeout);

    let upstream_start = Instant::now();
    let response = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            server
                .metrics
                .failed_requests
                .fetch_add(1, Ordering::Release);
            let msg = if e.is_timeout() {
                "upstream timeout".to_string()
            } else if e.is_connect() {
                format!("upstream connect error: {e}")
            } else {
                format!("upstream error: {e}")
            };
            send_error(frame_tx, stream_id, &msg).await;
            return None;
        }
    };

    // Capture connection-establishment duration (DNS + TCP/TLS + TTFB)
    // before proceeding to stream the response body.
    let connect_elapsed = connect_start.elapsed();

    // Send RESPONSE_HEADERS
    let status = response.status().as_u16();
    let ttfb_ms = upstream_start.elapsed().as_millis() as u64;
    let mut resp_headers: Vec<(String, String)> = Vec::with_capacity(response.headers().len() + 1);
    for (k, v) in response.headers() {
        if let Ok(vs) = v.to_str() {
            resp_headers.push((k.as_str().to_string(), vs.to_string()));
        }
    }
    // Inject proxy timing (same format as delegate mode)
    let timing = serde_json::json!({
        "dns_ms": dns_ms,
        "ttfb_ms": ttfb_ms,
        "upstream_ms": ttfb_ms,
        "upstream_processing_ms": ttfb_ms.saturating_sub(dns_ms),
        "body_size": body_size,
        "mode": "tunnel",
    });
    resp_headers.push(("x-proxy-timing".to_string(), timing.to_string()));
    let resp_meta = ResponseMeta {
        status,
        headers: resp_headers,
    };
    let meta_json: Bytes = serde_json::to_vec(&resp_meta).unwrap_or_default().into();
    let (meta_payload, meta_flags) = compress_payload(meta_json);
    if !send_frame(
        frame_tx,
        Frame::new(
            stream_id,
            MsgType::ResponseHeaders,
            meta_flags,
            meta_payload,
        ),
    )
    .await
    {
        return Some(connect_elapsed);
    }

    // Stream response body — relay upstream bytes through the tunnel.
    // Apply tunnel-level frame compression for chunks that benefit from it
    // (e.g. uncompressed SSE text). Already-compressed data (gzip/br from
    // upstream Content-Encoding) won't shrink further and will be sent as-is
    // thanks to the size check in compress_payload().
    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                if chunk.len() <= MAX_CHUNK_SIZE {
                    let (payload, extra_flags) = compress_payload(chunk);
                    if !send_frame(
                        frame_tx,
                        Frame::new(stream_id, MsgType::ResponseBody, extra_flags, payload),
                    )
                    .await
                    {
                        return Some(connect_elapsed);
                    }
                } else {
                    // Split oversized chunks, compress each slice
                    let mut offset = 0;
                    while offset < chunk.len() {
                        let end = (offset + MAX_CHUNK_SIZE).min(chunk.len());
                        let slice = chunk.slice(offset..end);
                        let (payload, extra_flags) = compress_payload(slice);
                        if !send_frame(
                            frame_tx,
                            Frame::new(stream_id, MsgType::ResponseBody, extra_flags, payload),
                        )
                        .await
                        {
                            return Some(connect_elapsed);
                        }
                        offset = end;
                    }
                }
            }
            Err(e) => {
                server.metrics.stream_errors.fetch_add(1, Ordering::Release);
                warn!(stream_id, error = %e, "upstream body read error");
                send_error(frame_tx, stream_id, &format!("body read error: {e}")).await;
                return Some(connect_elapsed);
            }
        }
    }

    // Send STREAM_END
    let _ = send_frame(
        frame_tx,
        Frame::new(
            stream_id,
            MsgType::StreamEnd,
            flags::END_STREAM,
            Bytes::new(),
        ),
    )
    .await;

    debug!(stream_id, status, "stream completed");
    Some(connect_elapsed)
}

async fn send_error(tx: &FrameSender, stream_id: u32, msg: &str) {
    // Error frames use best-effort delivery — don't block if writer is congested
    let _ = send_frame(
        tx,
        Frame::new(
            stream_id,
            MsgType::StreamError,
            0,
            Bytes::from(msg.to_string()),
        ),
    )
    .await;
}
