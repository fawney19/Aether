//! Per-stream request handler.
//!
//! Receives request frames, executes the upstream HTTP request,
//! and sends response frames back through the writer channel.

use std::io;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::stream;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use hyper::body::Frame as BodyFrame;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::state::{AppState, ServerContext};
use crate::target_filter;
use crate::upstream_client;

use super::protocol::{
    compress_payload, decompress_if_gzip, flags, Frame as TunnelFrame, MsgType, RequestMeta,
    ResponseMeta,
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
///
/// `host` and `content-length` are managed by the HTTP client (reqwest/hyper):
/// - `host` → translated to `:authority` pseudo-header in HTTP/2; forwarding
///   the original `host` alongside `:authority` triggers PROTOCOL_ERROR on
///   strict H2 implementations (e.g. Google APIs).
/// - `content-length` → recalculated by hyper from the actual body; a stale
///   value from the tunnel (body may have been re-compressed) causes H2
///   PROTOCOL_ERROR when it mismatches the real frame length.
const BLOCKED_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "host",
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
    body_rx: mpsc::Receiver<TunnelFrame>,
    frame_tx: FrameSender,
) {
    server.active_connections.fetch_add(1, Ordering::Release);

    let connect_elapsed =
        handle_stream_inner(&state, &server, stream_id, meta, body_rx, &frame_tx).await;

    server.active_connections.fetch_sub(1, Ordering::Release);
    if let Some(d) = connect_elapsed {
        server.metrics.record_request(d);
    }
}

/// Send a frame to the writer with a timeout. Returns false if send failed.
async fn send_frame(tx: &FrameSender, frame: TunnelFrame) -> bool {
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
    body_rx: mpsc::Receiver<TunnelFrame>,
    frame_tx: &FrameSender,
) -> Option<Duration> {
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
    let client = &state.upstream_client;
    let timeout = Duration::from_secs(meta.timeout.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS));
    let request_body_size = Arc::new(AtomicUsize::new(0));
    let request_body = build_streaming_request_body(body_rx, Arc::clone(&request_body_size));

    let method: hyper::Method = meta.method.parse().unwrap_or(hyper::Method::GET);
    let mut request = match hyper::Request::builder()
        .method(method)
        .uri(meta.url.as_str())
        .body(request_body)
    {
        Ok(request) => request,
        Err(e) => {
            send_error(
                frame_tx,
                stream_id,
                &format!("invalid upstream request: {e}"),
            )
            .await;
            return None;
        }
    };

    let headers = request.headers_mut();
    for (k, v) in &meta.headers {
        let k_lower = k.to_ascii_lowercase();
        if BLOCKED_HEADERS.contains(&k_lower.as_str()) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            hyper::header::HeaderName::from_bytes(k.as_bytes()),
            hyper::header::HeaderValue::from_str(v),
        ) {
            headers.insert(name, value);
        }
    }

    let mut captured_connection = upstream_client::capture_connection(&mut request);
    let connection_start = Instant::now();
    let connection_capture = tokio::spawn(async move {
        let connected = captured_connection.wait_for_connection_metadata().await;
        connected
            .as_ref()
            .map(|_| connection_start.elapsed().as_millis() as u64)
    });

    let upstream_start = Instant::now();
    let response = match tokio::time::timeout(timeout, client.request(request)).await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => {
            connection_capture.abort();
            server
                .metrics
                .failed_requests
                .fetch_add(1, Ordering::Release);
            let msg = if e.is_connect() {
                format!("upstream connect error: {e}")
            } else {
                format!("upstream error: {e}")
            };
            send_error(frame_tx, stream_id, &msg).await;
            return None;
        }
        Err(_) => {
            connection_capture.abort();
            server
                .metrics
                .failed_requests
                .fetch_add(1, Ordering::Release);
            send_error(frame_tx, stream_id, "upstream timeout").await;
            return None;
        }
    };

    // Capture connection-establishment duration (DNS + TCP/TLS + TTFB)
    // before proceeding to stream the response body.
    let connect_elapsed = connect_start.elapsed();

    // Send RESPONSE_HEADERS
    let status = response.status().as_u16();
    let ttfb_ms = upstream_start.elapsed().as_millis() as u64;
    // Short timeout: on connection reuse hyper may never fire the connect
    // callback, so avoid blocking indefinitely.
    let connection_acquire_ms =
        match tokio::time::timeout(Duration::from_millis(100), connection_capture).await {
            Ok(Ok(ms)) => ms,
            Ok(Err(_)) => None, // JoinError (task panicked / cancelled)
            Err(_) => None,     // timeout -- task is detached but lightweight
        };
    let request_timing =
        upstream_client::resolve_request_timing(&response, connection_acquire_ms, ttfb_ms);
    let mut resp_headers: Vec<(String, String)> = Vec::with_capacity(response.headers().len() + 1);
    for (k, v) in response.headers() {
        if let Ok(vs) = v.to_str() {
            resp_headers.push((k.as_str().to_string(), vs.to_string()));
        }
    }
    let timing = serde_json::json!({
        "dns_ms": dns_ms,
        "connection_acquire_ms": request_timing.connection_acquire_ms,
        "connection_reused": request_timing.connection_reused,
        "connect_ms": request_timing.connect_ms,
        "tls_ms": request_timing.tls_ms,
        "ttfb_ms": ttfb_ms,
        "upstream_ms": ttfb_ms,
        "response_wait_ms": request_timing.response_wait_ms,
        "upstream_processing_ms": request_timing.response_wait_ms,
        "timing_source": "instrumented_connector",
        "total_ms": connect_elapsed.as_millis() as u64,
        "body_size": request_body_size.load(Ordering::Relaxed),
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
        TunnelFrame::new(
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
    let mut stream = response.into_body().into_data_stream();
    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                if chunk.len() <= MAX_CHUNK_SIZE {
                    let (payload, extra_flags) = compress_payload(chunk);
                    if !send_frame(
                        frame_tx,
                        TunnelFrame::new(stream_id, MsgType::ResponseBody, extra_flags, payload),
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
                            TunnelFrame::new(
                                stream_id,
                                MsgType::ResponseBody,
                                extra_flags,
                                payload,
                            ),
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
        TunnelFrame::new(
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
        TunnelFrame::new(
            stream_id,
            MsgType::StreamError,
            0,
            Bytes::from(msg.to_string()),
        ),
    )
    .await;
}

fn build_streaming_request_body(
    body_rx: mpsc::Receiver<TunnelFrame>,
    body_size: Arc<AtomicUsize>,
) -> upstream_client::UpstreamRequestBody {
    let body_stream = stream::unfold(
        (body_rx, body_size, false),
        |(mut body_rx, body_size, finished)| async move {
            if finished {
                return None;
            }

            loop {
                let frame = match body_rx.recv().await {
                    Some(frame) => frame,
                    None => return None,
                };

                match frame.msg_type {
                    MsgType::RequestBody => {
                        let end_stream = frame.is_end_stream();
                        let payload = match decompress_if_gzip(&frame) {
                            Ok(payload) => payload,
                            Err(error) => {
                                let err =
                                    io::Error::other(format!("gzip decompress failed: {error}"));
                                return Some((Err(err), (body_rx, body_size, true)));
                            }
                        };

                        if payload.is_empty() {
                            if end_stream {
                                return None;
                            }
                            continue;
                        }

                        body_size.fetch_add(payload.len(), Ordering::Relaxed);
                        return Some((
                            Ok(BodyFrame::data(payload)),
                            (body_rx, body_size, end_stream),
                        ));
                    }
                    MsgType::StreamError => {
                        let message = String::from_utf8(frame.payload.to_vec())
                            .unwrap_or_else(|_| "client cancelled request body".to_string());
                        return Some((Err(io::Error::other(message)), (body_rx, body_size, true)));
                    }
                    MsgType::StreamEnd => return None,
                    _ => continue,
                }
            }
        },
    );

    upstream_client::stream_request_body(body_stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streaming_request_body_yields_chunks_and_tracks_size() {
        let (tx, rx) = mpsc::channel(4);
        let body_size = Arc::new(AtomicUsize::new(0));
        let mut body = build_streaming_request_body(rx, Arc::clone(&body_size));

        tx.send(TunnelFrame::new(
            1,
            MsgType::RequestBody,
            0,
            Bytes::from_static(b"abc"),
        ))
        .await
        .expect("send first chunk");
        tx.send(TunnelFrame::new(
            1,
            MsgType::RequestBody,
            flags::END_STREAM,
            Bytes::from_static(b"def"),
        ))
        .await
        .expect("send final chunk");
        drop(tx);

        let first = body
            .frame()
            .await
            .expect("first frame")
            .expect("first frame ok")
            .into_data()
            .expect("first data frame");
        let second = body
            .frame()
            .await
            .expect("second frame")
            .expect("second frame ok")
            .into_data()
            .expect("second data frame");

        assert_eq!(first, Bytes::from_static(b"abc"));
        assert_eq!(second, Bytes::from_static(b"def"));
        assert!(body.frame().await.is_none());
        assert_eq!(body_size.load(Ordering::Relaxed), 6);
    }

    #[tokio::test]
    async fn streaming_request_body_surfaces_client_cancel_as_error() {
        let (tx, rx) = mpsc::channel(4);
        let body_size = Arc::new(AtomicUsize::new(0));
        let mut body = build_streaming_request_body(rx, Arc::clone(&body_size));

        tx.send(TunnelFrame::new(
            1,
            MsgType::StreamError,
            0,
            Bytes::from_static(b"client cancelled"),
        ))
        .await
        .expect("send cancel frame");
        drop(tx);

        let err = body
            .frame()
            .await
            .expect("error frame present")
            .expect_err("body should surface cancellation error");
        assert!(err.to_string().contains("client cancelled"));
        assert!(body.frame().await.is_none());
        assert_eq!(body_size.load(Ordering::Relaxed), 0);
    }
}
