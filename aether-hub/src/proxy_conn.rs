/// Proxy-side WebSocket connection handler
///
/// Handles the lifecycle of a single aether-proxy connection:
/// accept -> authenticate (headers) -> read loop -> cleanup
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::hub::{ConnConfig, HubRouter, ProxyConn, SendStatus};
use crate::protocol;

/// Maximum single frame size: 64 MB
const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

pub async fn handle_proxy_connection(
    ws: WebSocket,
    hub: Arc<HubRouter>,
    node_id: String,
    node_name: String,
    max_streams: usize,
    cfg: ConnConfig,
) {
    let conn_id = hub.alloc_conn_id();
    let (mut ws_tx, ws_rx) = ws.split();

    let (tx, mut rx) = mpsc::channel::<Message>(cfg.outbound_queue_capacity);
    let (close_tx, mut close_rx) = watch::channel(false);

    let conn = Arc::new(ProxyConn::new(
        conn_id,
        node_id.clone(),
        node_name.clone(),
        tx,
        close_tx,
        max_streams,
    ));

    hub.register_proxy(conn.clone());

    let writer = tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => {
                        if ws_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                },
                changed = close_rx.changed() => {
                    if changed.is_err() || *close_rx.borrow() {
                        break;
                    }
                }
            }
        }
        let _ = ws_tx.close().await;
    });

    let ping_conn = conn.clone();
    let ping_interval = cfg.ping_interval;
    let ping_task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(ping_interval).await;
            let ping = protocol::encode_ping();
            if !matches!(
                ping_conn.send(Message::Binary(ping.into())),
                SendStatus::Queued
            ) {
                break;
            }
        }
    });

    let reader_hub = hub.clone();
    let reader_conn = conn.clone();
    let reader = tokio::spawn(async move {
        run_proxy_reader(ws_rx, reader_hub, reader_conn, cfg.idle_timeout).await;
    });

    let _ = reader.await;
    ping_task.abort();
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
) {
    let idle_enabled = !idle_timeout.is_zero();
    let mut oversized_count = 0u32;
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
                let mut data = data.to_vec();
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
                info!(conn_id = conn.id, node_id = %conn.node_id, "proxy WebSocket closed");
                break;
            }
            Some(Err(e)) => {
                warn!(conn_id = conn.id, error = %e, "proxy WebSocket error");
                break;
            }
            _ => {}
        }
    }
}
