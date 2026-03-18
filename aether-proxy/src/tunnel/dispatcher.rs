//! Frame dispatcher: reads incoming WebSocket frames and routes them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::state::{AppState, ServerContext};

use super::heartbeat::HeartbeatHandle;
use super::protocol::{decompress_if_gzip, Frame, MsgType, RequestMeta};
use super::stream_handler;
use super::writer::FrameSender;

/// Run the dispatcher loop, reading from the WebSocket stream.
pub async fn run<S>(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    mut ws_stream: S,
    frame_tx: FrameSender,
    heartbeat: HeartbeatHandle,
) -> Result<(), anyhow::Error>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send
        + 'static,
{
    // Active streams: stream_id -> body sender
    let mut streams: HashMap<u32, mpsc::Sender<Frame>> = HashMap::new();
    // Track spawned stream handlers so we can wait for them on shutdown
    let mut handler_handles: Vec<JoinHandle<()>> = Vec::new();
    let max_streams = state.config.tunnel_max_streams.unwrap_or(128) as usize;
    let mut frames_since_cleanup: u32 = 0;
    let stale_timeout = Duration::from_secs(state.config.tunnel_stale_timeout_secs);

    // Track last time we received any data to detect stale connections
    let mut last_data_at = tokio::time::Instant::now();

    let read_err = loop {
        let msg_result = tokio::select! {
            msg = ws_stream.next() => {
                match msg {
                    Some(r) => r,
                    None => break None,
                }
            }
            _ = tokio::time::sleep_until(last_data_at + stale_timeout) => {
                warn!(
                    stale_secs = stale_timeout.as_secs(),
                    "tunnel connection stale, no data received"
                );
                break None;
            }
        };

        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                error!(error = %e, "WebSocket read error");
                break Some(e);
            }
        };

        // Any successfully received message proves the connection is alive
        last_data_at = tokio::time::Instant::now();

        let data = match msg {
            Message::Binary(data) => Bytes::from(data),
            Message::Ping(_) => continue,
            Message::Pong(_) => continue,
            Message::Close(_) => {
                info!("received WebSocket close");
                break None;
            }
            _ => continue,
        };

        let frame = match Frame::decode(data) {
            Ok(f) => f,
            Err(e) => {
                warn!(error = %e, "failed to decode frame");
                continue;
            }
        };

        match frame.msg_type {
            MsgType::RequestHeaders => {
                // Decompress if the frame is gzip-compressed, then parse metadata
                let payload = match decompress_if_gzip(&frame) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(stream_id = frame.stream_id, error = %e, "frame decompress failed");
                        continue;
                    }
                };
                let meta: RequestMeta = match serde_json::from_slice(&payload) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!(stream_id = frame.stream_id, error = %e, "invalid request metadata");
                        // Use try_send to avoid blocking the read loop
                        if frame_tx
                            .try_send(Frame::new(
                                frame.stream_id,
                                MsgType::StreamError,
                                0,
                                Bytes::from(format!("invalid request metadata: {e}")),
                            ))
                            .is_err()
                        {
                            warn!(
                                stream_id = frame.stream_id,
                                "writer channel full, StreamError dropped"
                            );
                        }
                        continue;
                    }
                };

                if streams.len() >= max_streams {
                    warn!(
                        stream_id = frame.stream_id,
                        "max concurrent streams reached"
                    );
                    if frame_tx
                        .try_send(Frame::new(
                            frame.stream_id,
                            MsgType::StreamError,
                            0,
                            Bytes::from("max concurrent streams reached"),
                        ))
                        .is_err()
                    {
                        warn!(
                            stream_id = frame.stream_id,
                            "writer channel full, StreamError dropped"
                        );
                    }
                    continue;
                }

                // Create body channel and spawn handler
                let (body_tx, body_rx) = mpsc::channel::<Frame>(64);
                streams.insert(frame.stream_id, body_tx);

                let state_clone = Arc::clone(&state);
                let server_clone = Arc::clone(&server);
                let tx_clone = frame_tx.clone();
                let sid = frame.stream_id;
                let handle = tokio::spawn(async move {
                    stream_handler::handle_stream(
                        state_clone,
                        server_clone,
                        sid,
                        meta,
                        body_rx,
                        tx_clone,
                    )
                    .await;
                });
                handler_handles.push(handle);

                debug!(stream_id = frame.stream_id, "new stream started");
            }

            MsgType::RequestBody => {
                if let Some(tx) = streams.get(&frame.stream_id) {
                    let is_end = frame.is_end_stream();
                    let sid = frame.stream_id;
                    let _ = tx.send(frame).await;
                    if is_end {
                        streams.remove(&sid);
                    }
                }
            }

            MsgType::StreamEnd | MsgType::StreamError => {
                // Client-side cancellation or end
                if let Some(tx) = streams.remove(&frame.stream_id) {
                    let _ = tx.send(frame).await;
                }
            }

            MsgType::Ping => {
                // Use try_send to avoid blocking the read loop when writer is congested
                if frame_tx
                    .try_send(Frame::control(MsgType::Pong, frame.payload))
                    .is_err()
                {
                    warn!("writer channel full, Pong dropped");
                }
            }

            MsgType::HeartbeatAck => {
                heartbeat.on_ack(frame.payload).await;
            }

            MsgType::GoAway => {
                info!("received GOAWAY");
                break None;
            }

            _ => {
                debug!(msg_type = ?frame.msg_type, "ignoring unexpected frame type");
            }
        }

        // Periodically clean up finished handles to avoid unbounded growth.
        // Trigger every 64 frames OR when the count exceeds max_streams.
        frames_since_cleanup += 1;
        if frames_since_cleanup >= 64 || handler_handles.len() > max_streams {
            handler_handles.retain(|h| !h.is_finished());
            frames_since_cleanup = 0;
        }
    };

    // Drop body senders so stream handlers waiting on body_rx will unblock
    streams.clear();

    // Wait for active stream handlers to finish so their frame_tx clones
    // are dropped before the writer closes the sink.
    drain_handlers(handler_handles).await;

    match read_err {
        Some(e) => Err(e.into()),
        None => Ok(()),
    }
}

/// Wait for all active stream handlers to finish (with a timeout).
async fn drain_handlers(handles: Vec<JoinHandle<()>>) {
    if handles.is_empty() {
        return;
    }
    let count = handles.len();
    debug!(count, "waiting for active stream handlers to finish");
    let _ = tokio::time::timeout(Duration::from_secs(30), async {
        for h in handles {
            let _ = h.await;
        }
    })
    .await;
}
