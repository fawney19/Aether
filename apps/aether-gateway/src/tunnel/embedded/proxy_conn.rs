/// Proxy-side WebSocket connection handler
///
/// Handles the lifecycle of a single aether-tunnel connection:
/// accept -> authenticate (headers) -> read loop -> cleanup
use std::sync::Arc;
use std::time::Duration;

use aether_runtime::bounded_queue;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use super::hub::{ConnConfig, HubRouter, ProxyConn, SendStatus};
use super::protocol;
use aether_contracts::tunnel::Frame;
use aether_contracts::tunnel_security::{SecureFrameCodec, TunnelSecurityRole};

/// Maximum single frame size: 64 MB
const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

pub async fn handle_proxy_connection(
    ws: WebSocket,
    hub: Arc<HubRouter>,
    node_id: String,
    node_name: String,
    max_streams: usize,
    protocol_version: u8,
    security_key: Option<String>,
    security_session: String,
    cfg: ConnConfig,
) {
    let conn_id = hub.alloc_conn_id();
    let (mut ws_tx, ws_rx) = ws.split();

    let (tx, mut rx) = bounded_queue::<Message>(cfg.outbound_queue_capacity);
    let (close_tx, mut close_rx) = watch::channel(false);
    let security = match security_key.as_deref() {
        Some(key) => {
            match SecureFrameCodec::new(key, &security_session, TunnelSecurityRole::Server) {
                Ok(codec) => Some(Arc::new(codec)),
                Err(error) => {
                    warn!(conn_id, node_id = %node_id, error = %error, "secure tunnel codec initialization failed");
                    return;
                }
            }
        }
        None => None,
    };

    let conn = Arc::new(ProxyConn::new(
        conn_id,
        node_id.clone(),
        node_name.clone(),
        tx,
        close_tx,
        max_streams,
        protocol_version,
    ));

    hub.register_proxy(conn.clone());

    let writer_conn_id = conn_id;
    let writer_conn = conn.clone();
    let writer_security = security.clone();
    let writer = tokio::spawn(async move {
        let mut frames_sent: u64 = 0;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => {
                        let is_binary = matches!(&msg, Message::Binary(_));
                        let msg_len = match &msg {
                            Message::Binary(b) => b.len(),
                            _ => 0,
                        };
                        let send_started_at = std::time::Instant::now();
                        let msg = match encrypt_message(msg, writer_security.as_deref()) {
                            Ok(msg) => msg,
                            Err(error) => {
                                warn!(conn_id = writer_conn_id, error = %error, "failed to encrypt outbound proxy frame");
                                break;
                            }
                        };
                        let send_result = tokio::time::timeout(
                            Duration::from_secs(15),
                            ws_tx.send(msg),
                        ).await;
                        match send_result {
                            Ok(Ok(())) => {
                                writer_conn.record_write_latency(send_started_at.elapsed());
                            }
                            Ok(Err(e)) => {
                                let snapshot = writer_conn.outbound.snapshot();
                                warn!(
                                    conn_id = writer_conn_id,
                                    frames_sent = frames_sent,
                                    queue_depth = snapshot.depth,
                                    queue_capacity = snapshot.capacity,
                                    stream_count = writer_conn.stream_count.load(std::sync::atomic::Ordering::Relaxed),
                                    closing = writer_conn.outbound.is_closing(),
                                    draining = writer_conn.is_draining(),
                                    error = %e,
                                    "writer ws_tx.send failed"
                                );
                                break;
                            }
                            Err(_) => {
                                let snapshot = writer_conn.outbound.snapshot();
                                warn!(
                                    conn_id = writer_conn_id,
                                    frames_sent = frames_sent,
                                    queue_depth = snapshot.depth,
                                    queue_capacity = snapshot.capacity,
                                    stream_count = writer_conn.stream_count.load(std::sync::atomic::Ordering::Relaxed),
                                    closing = writer_conn.outbound.is_closing(),
                                    draining = writer_conn.is_draining(),
                                    "writer ws_tx.send timed out"
                                );
                                break;
                            }
                        }
                        frames_sent += 1;
                        if is_binary && msg_len > protocol::HEADER_SIZE {
                            debug!(
                                conn_id = writer_conn_id,
                                size = msg_len,
                                frames_sent = frames_sent,
                                "writer sent binary frame"
                            );
                        }
                    }
                    None => break,
                },
                changed = close_rx.changed() => {
                    if changed.is_err() || *close_rx.borrow() {
                        let snapshot = writer_conn.outbound.snapshot();
                        info!(
                            conn_id = writer_conn_id,
                            frames_sent = frames_sent,
                            queue_depth = snapshot.depth,
                            queue_capacity = snapshot.capacity,
                            stream_count = writer_conn.stream_count.load(std::sync::atomic::Ordering::Relaxed),
                            closing = writer_conn.outbound.is_closing(),
                            draining = writer_conn.is_draining(),
                            "writer close signal received"
                        );
                        break;
                    }
                }
            }
        }
        info!(
            conn_id = writer_conn_id,
            frames_sent = frames_sent,
            "writer task exiting"
        );
        writer_conn.request_close();
        match tokio::time::timeout(Duration::from_secs(5), ws_tx.close()).await {
            Ok(Ok(())) => debug!(
                conn_id = writer_conn_id,
                frames_sent = frames_sent,
                "writer WebSocket close completed"
            ),
            Ok(Err(error)) => warn!(
                conn_id = writer_conn_id,
                frames_sent = frames_sent,
                error = %error,
                "writer WebSocket close failed"
            ),
            Err(_) => warn!(
                conn_id = writer_conn_id,
                frames_sent = frames_sent,
                "writer WebSocket close timed out"
            ),
        }
    });

    let ping_conn = conn.clone();
    let ping_interval = cfg.ping_interval;
    let ping_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(ping_interval).await;
            let ping = protocol::encode_ping();
            let status = ping_conn.send(Message::Binary(ping.into()));
            if !matches!(status, SendStatus::Queued) {
                let snapshot = ping_conn.outbound.snapshot();
                match status {
                    SendStatus::Closed => info!(
                        conn_id = ping_conn.id,
                        queue_depth = snapshot.depth,
                        queue_capacity = snapshot.capacity,
                        stream_count = ping_conn
                            .stream_count
                            .load(std::sync::atomic::Ordering::Relaxed),
                        closing = ping_conn.outbound.is_closing(),
                        draining = ping_conn.is_draining(),
                        "ping task stopped because connection is closing"
                    ),
                    SendStatus::Congested => warn!(
                        conn_id = ping_conn.id,
                        queue_depth = snapshot.depth,
                        queue_capacity = snapshot.capacity,
                        stream_count = ping_conn
                            .stream_count
                            .load(std::sync::atomic::Ordering::Relaxed),
                        closing = ping_conn.outbound.is_closing(),
                        draining = ping_conn.is_draining(),
                        "ping task stopped because outbound queue is congested"
                    ),
                    SendStatus::Queued => {}
                }
                break;
            }
        }
    });

    let reader_hub = hub.clone();
    let reader_conn = conn.clone();
    let reader = tokio::spawn(async move {
        run_proxy_reader(ws_rx, reader_hub, reader_conn, cfg.idle_timeout, security).await;
    });

    let _ = reader.await;
    ping_task.abort();
    let snapshot = conn.outbound.snapshot();
    info!(
        conn_id = conn.id,
        node_id = %conn.node_id,
        queue_depth = snapshot.depth,
        queue_capacity = snapshot.capacity,
        stream_count = conn.stream_count.load(std::sync::atomic::Ordering::Relaxed),
        closing = conn.outbound.is_closing(),
        draining = conn.is_draining(),
        "proxy connection cleanup starting"
    );
    conn.request_close();
    hub.unregister_proxy(conn_id, &node_id);
    drop(conn);
    tokio::time::sleep(Duration::from_millis(100)).await;
    writer.abort();
    let _ = writer.await;
}

async fn run_proxy_reader(
    mut ws_rx: futures_util::stream::SplitStream<WebSocket>,
    hub: Arc<HubRouter>,
    conn: Arc<ProxyConn>,
    idle_timeout: Duration,
    security: Option<Arc<SecureFrameCodec>>,
) {
    let idle_enabled = !idle_timeout.is_zero();
    let mut oversized_count = 0u32;
    let mut frames_received: u64 = 0;
    loop {
        let msg = if idle_enabled {
            tokio::select! {
                msg = ws_rx.next() => msg,
                _ = tokio::time::sleep(idle_timeout) => {
                    warn!(conn_id = conn.id, node_id = %conn.node_id, "proxy idle timeout");
                    let _ = conn.send(Message::Binary(protocol::encode_goaway().into()));
                    conn.request_close();
                    break;
                }
            }
        } else {
            ws_rx.next().await
        };

        match msg {
            Some(Ok(Message::Binary(data))) => {
                frames_received += 1;
                let mut data = match decrypt_message(data, security.as_deref()) {
                    Ok(data) => data,
                    Err(error) => {
                        warn!(conn_id = conn.id, error = %error, "failed to decrypt secure proxy frame");
                        conn.request_close();
                        break;
                    }
                };
                if data.len() > MAX_FRAME_SIZE {
                    oversized_count += 1;
                    warn!(
                        conn_id = conn.id,
                        size = data.len(),
                        "oversized frame from proxy"
                    );
                    if oversized_count >= 5 {
                        warn!(conn_id = conn.id, "too many oversized frames, closing");
                        conn.request_close();
                        break;
                    }
                    continue;
                }
                oversized_count = 0;

                if data.len() < protocol::HEADER_SIZE {
                    debug!(conn_id = conn.id, "frame too small, skipping");
                    continue;
                }

                hub.handle_proxy_frame(conn.id, &mut data).await;
            }
            Some(Ok(Message::Close(_))) | None => {
                info!(
                    conn_id = conn.id,
                    node_id = %conn.node_id,
                    frames_received = frames_received,
                    "proxy WebSocket closed"
                );
                break;
            }
            Some(Err(e)) => {
                warn!(
                    conn_id = conn.id,
                    frames_received = frames_received,
                    error = %e,
                    "proxy WebSocket error"
                );
                break;
            }
            Some(Ok(Message::Ping(payload))) => {
                conn.send(Message::Pong(payload));
            }
            _ => {}
        }
    }
}

fn encrypt_message(
    msg: Message,
    security: Option<&SecureFrameCodec>,
) -> Result<Message, aether_contracts::tunnel_security::TunnelSecurityError> {
    let Some(codec) = security else {
        return Ok(msg);
    };
    match msg {
        Message::Binary(data) => {
            let frame = Frame::decode(bytes::Bytes::from(data.to_vec()))
                .map_err(|_| aether_contracts::tunnel_security::TunnelSecurityError::Encrypt)?;
            Ok(Message::Binary(codec.encrypt_frame(frame)?))
        }
        other => Ok(other),
    }
}

fn decrypt_message(
    data: bytes::Bytes,
    security: Option<&SecureFrameCodec>,
) -> Result<Vec<u8>, aether_contracts::tunnel_security::TunnelSecurityError> {
    let Some(codec) = security else {
        return Ok(data.to_vec());
    };
    let frame = Frame::decode(data)
        .map_err(|_| aether_contracts::tunnel_security::TunnelSecurityError::Decrypt)?;
    let frame = codec.decrypt_frame(frame)?;
    Ok(frame.encode().to_vec())
}
