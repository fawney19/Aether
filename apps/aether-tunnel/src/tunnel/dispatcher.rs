//! Frame dispatcher: reads incoming WebSocket frames and routes them.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};
use tokio::task::{AbortHandle, JoinSet};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::state::{AppState, ServerContext};

use super::heartbeat::HeartbeatHandle;
use super::protocol::{decompress_if_gzip_with_limit, Frame, MsgType, RequestMeta};
use super::stream_handler;
use super::stream_handler::StreamSendWindow;
use super::writer::FrameSender;
use aether_contracts::tunnel_security::SecureFrameCodec;

const REQUEST_BODY_QUEUE_BUDGET_BYTES: usize = 256 * 1024 * 1024;
static REQUEST_BODY_QUEUE_BUDGET: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(REQUEST_BODY_QUEUE_BUDGET_BYTES)));

struct BudgetedFramePayload {
    bytes: Bytes,
    _permit: OwnedSemaphorePermit,
}

impl AsRef<[u8]> for BudgetedFramePayload {
    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamDispatchStatus {
    Delivered,
    Closed,
    Congested,
}

#[derive(Clone)]
struct StreamDispatchTarget {
    body_tx: mpsc::Sender<Frame>,
    response_window: Arc<StreamSendWindow>,
    handler: Option<AbortHandle>,
}

struct StreamCompletion {
    stream_id: u32,
    finished_tx: mpsc::UnboundedSender<u32>,
}

impl Drop for StreamCompletion {
    fn drop(&mut self) {
        let _ = self.finished_tx.send(self.stream_id);
    }
}

/// A request stream is identified by a non-zero id and may only be opened
/// once while its handler is active.  Replacing an entry in `streams` would
/// orphan the old body channel while still spawning another handler, making
/// the active-stream limit ineffective and allowing unbounded task growth.
fn validate_request_stream_id(
    streams: &HashMap<u32, StreamDispatchTarget>,
    active_handler_ids: &HashSet<u32>,
    stream_id: u32,
) -> Result<(), &'static str> {
    if stream_id == 0 {
        return Err("invalid stream id");
    }
    if streams.contains_key(&stream_id) || active_handler_ids.contains(&stream_id) {
        return Err("duplicate stream id");
    }
    Ok(())
}

/// Run the dispatcher loop, reading from the WebSocket stream.
#[allow(dead_code)]
pub async fn run<S>(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    ws_stream: S,
    frame_tx: FrameSender,
    heartbeat: HeartbeatHandle,
    drain: watch::Receiver<bool>,
) -> Result<(), anyhow::Error>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send
        + 'static,
{
    run_with_security(state, server, ws_stream, frame_tx, heartbeat, drain, None).await
}

pub async fn run_with_security<S>(
    state: Arc<AppState>,
    server: Arc<ServerContext>,
    mut ws_stream: S,
    frame_tx: FrameSender,
    heartbeat: HeartbeatHandle,
    mut drain: watch::Receiver<bool>,
    security: Option<Arc<SecureFrameCodec>>,
) -> Result<(), anyhow::Error>
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin
        + Send
        + 'static,
{
    // Active streams: stream_id -> body sender + response flow-control window.
    let mut streams: HashMap<u32, StreamDispatchTarget> = HashMap::new();
    // A handler can outlive its routing entry when body dispatch fails. Keep
    // its id reserved until the handler reports completion so a peer cannot
    // reopen the same id and bypass the stream admission limit.
    let mut active_handler_ids: HashSet<u32> = HashSet::new();
    // Track spawned stream handlers so we can wait for them on shutdown
    let mut handler_handles = JoinSet::new();
    let (handler_finished_tx, mut handler_finished_rx) = mpsc::unbounded_channel::<u32>();
    let max_streams = state.config.tunnel_max_streams.unwrap_or(128) as usize;
    let mut frames_since_cleanup: u32 = 0;
    let stale_timeout = state
        .config
        .tunnel_stale_timeout()
        .expect("validated config should resolve tunnel stale timeout");

    // Track last time we received any data to detect stale connections
    let mut last_data_at = tokio::time::Instant::now();
    let mut draining = *drain.borrow();
    let mut drain_open = true;
    let mut drain_deadline = draining.then(|| {
        tokio::time::Instant::now() + Duration::from_millis(state.config.tunnel_drain_deadline_ms)
    });
    let mut initial_window_bytes = state.config.tunnel_stream_initial_window_bytes;
    let mut close_rx = frame_tx.subscribe_close();

    let read_err = loop {
        if *close_rx.borrow() {
            break None;
        }
        if draining && streams.is_empty() && active_handler_ids.is_empty() {
            info!("tunnel drained after in-flight streams completed");
            break None;
        }

        let msg_result = tokio::select! {
            _ = close_rx.changed() => break None,
            msg = ws_stream.next() => {
                match msg {
                    Some(r) => r,
                    None => break None,
                }
            }
            changed = drain.changed(), if drain_open => {
                if changed.is_err() {
                    drain_open = false;
                    continue;
                }
                if *drain.borrow() {
                    info!("tunnel drain requested, waiting for in-flight streams");
                    draining = true;
                    drain_deadline.get_or_insert_with(|| tokio::time::Instant::now() + Duration::from_millis(state.config.tunnel_drain_deadline_ms));
                }
                continue;
            }
            _ = async {
                match drain_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            } => break None,
            finished = handler_finished_rx.recv() => {
                if let Some(stream_id) = finished {
                    active_handler_ids.remove(&stream_id);
                    streams.remove(&stream_id);
                    if draining && streams.is_empty() && active_handler_ids.is_empty() {
                        info!("tunnel drained after stream handler completion");
                        break None;
                    }
                }
                continue;
            }
            _ = tokio::time::sleep_until(last_data_at + stale_timeout) => {
                warn!(
                    stale_ms = stale_timeout.as_millis(),
                    "tunnel connection stale, no data received"
                );
                server.tunnel_metrics.record_error(
                    "stale_timeout",
                    &format!("no tunnel frame received for {}ms", stale_timeout.as_millis()),
                );
                break None;
            }
        };

        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                error!(error = %e, "WebSocket read error");
                server
                    .tunnel_metrics
                    .record_error("ws_read_error", &e.to_string());
                break Some(e);
            }
        };

        // Any successfully received message proves the connection is alive
        last_data_at = tokio::time::Instant::now();

        let data = match msg {
            Message::Binary(data) => {
                server.tunnel_metrics.record_ws_incoming_frame(data.len());
                Bytes::from(data)
            }
            Message::Ping(_) => continue,
            Message::Pong(_) => continue,
            Message::Close(_) => {
                debug!("received WebSocket close");
                break None;
            }
            _ => continue,
        };

        let frame = match Frame::decode(data) {
            Ok(f) => f,
            Err(e) => {
                warn!(error = %e, "failed to decode frame");
                server
                    .tunnel_metrics
                    .record_error("frame_decode_error", &e.to_string());
                continue;
            }
        };
        let frame = match security.as_deref() {
            Some(codec) => match codec.decrypt_frame(frame) {
                Ok(frame) => frame,
                Err(e) => {
                    warn!(error = %e, "failed to decrypt secure tunnel frame");
                    server
                        .tunnel_metrics
                        .record_error("secure_frame_decrypt_error", &e.to_string());
                    break None;
                }
            },
            None => frame,
        };

        match frame.msg_type {
            MsgType::RequestHeaders => {
                if let Err(reason) =
                    validate_request_stream_id(&streams, &active_handler_ids, frame.stream_id)
                {
                    warn!(
                        stream_id = frame.stream_id,
                        reason, "rejecting request headers with invalid stream id"
                    );
                    // Zero is reserved for connection-level control frames,
                    // so do not emit a stream-scoped error using that id.
                    if frame.stream_id != 0 {
                        try_send_stream_error(&frame_tx, frame.stream_id, reason);
                    }
                    continue;
                }
                if draining {
                    try_send_stream_error(&frame_tx, frame.stream_id, "tunnel draining");
                    continue;
                }

                // Decompress if the frame is gzip-compressed, then parse metadata
                let payload = match decompress_if_gzip_with_limit(
                    &frame,
                    aether_contracts::tunnel::MAX_TUNNEL_RELAY_META_LEN,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(stream_id = frame.stream_id, error = %e, "frame decompress failed");
                        try_send_stream_error(
                            &frame_tx,
                            frame.stream_id,
                            "invalid request metadata",
                        );
                        continue;
                    }
                };
                let meta: RequestMeta = match serde_json::from_slice(&payload) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!(stream_id = frame.stream_id, error = %e, "invalid request metadata");
                        try_send_stream_error(
                            &frame_tx,
                            frame.stream_id,
                            "invalid request metadata",
                        );
                        continue;
                    }
                };

                if active_handler_ids.len() >= max_streams {
                    warn!(
                        stream_id = frame.stream_id,
                        "max concurrent streams reached"
                    );
                    try_send_stream_error(
                        &frame_tx,
                        frame.stream_id,
                        "max concurrent streams reached",
                    );
                    continue;
                }

                // Create body channel and spawn handler
                let body_capacity = (initial_window_bytes as usize)
                    .div_ceil(32 * 1024)
                    .saturating_add(1)
                    .max(64);
                let (body_tx, body_rx) = mpsc::channel::<Frame>(body_capacity);
                let response_window = Arc::new(StreamSendWindow::new(initial_window_bytes));
                streams.insert(
                    frame.stream_id,
                    StreamDispatchTarget {
                        body_tx,
                        response_window: Arc::clone(&response_window),
                        handler: None,
                    },
                );
                active_handler_ids.insert(frame.stream_id);
                let request_headers_end_stream = frame.is_end_stream();

                let state_clone = Arc::clone(&state);
                let server_clone = Arc::clone(&server);
                let tx_clone = frame_tx.clone();
                let sid = frame.stream_id;
                let completion = StreamCompletion {
                    stream_id: sid,
                    finished_tx: handler_finished_tx.clone(),
                };
                let handle = handler_handles.spawn(async move {
                    let _completion = completion;
                    stream_handler::handle_stream(
                        state_clone,
                        server_clone,
                        sid,
                        meta,
                        body_rx,
                        tx_clone,
                        response_window,
                    )
                    .await;
                });
                streams.get_mut(&sid).expect("new stream exists").handler = Some(handle);

                if request_headers_end_stream {
                    if let Some(target) = streams.get(&sid) {
                        let _ = target.body_tx.try_send(Frame::new(
                            sid,
                            MsgType::StreamEnd,
                            0,
                            Bytes::new(),
                        ));
                    }
                }

                debug!(stream_id = frame.stream_id, "new stream started");
            }

            MsgType::RequestBody => {
                if let Some(target) = streams.get(&frame.stream_id).cloned() {
                    let is_end = frame.is_end_stream();
                    let sid = frame.stream_id;
                    let dispatch = dispatch_stream_frame(&target.body_tx, frame).await;
                    if dispatch == StreamDispatchStatus::Congested {
                        if let Some(target) = streams.remove(&sid) {
                            if let Some(handler) = target.handler {
                                handler.abort();
                            }
                        }
                        server.tunnel_metrics.record_error(
                            "stream_dispatch_timeout",
                            &format!("request body dispatch congested for stream {}", sid),
                        );
                        try_send_stream_error(
                            &frame_tx,
                            sid,
                            "tunnel request body dispatch stalled",
                        );
                        if is_end && draining && streams.is_empty() && active_handler_ids.is_empty()
                        {
                            info!("tunnel drained after request body completion");
                            break None;
                        }
                    }
                }
            }

            MsgType::StreamEnd => {
                if let Some(target) = streams.get(&frame.stream_id) {
                    if dispatch_stream_frame(&target.body_tx, frame.clone()).await
                        == StreamDispatchStatus::Congested
                    {
                        if let Some(handler) = &target.handler {
                            handler.abort();
                        }
                        try_send_stream_error(
                            &frame_tx,
                            frame.stream_id,
                            "tunnel request body dispatch stalled",
                        );
                    }
                }
            }

            MsgType::StreamError | MsgType::ResetStream => {
                // Client-side cancellation or end
                if let Some(target) = streams.remove(&frame.stream_id) {
                    if let Some(handler) = target.handler {
                        handler.abort();
                    }
                    if draining && streams.is_empty() && active_handler_ids.is_empty() {
                        info!("tunnel drained after stream termination");
                        break None;
                    }
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
                heartbeat.on_ack(frame.payload);
            }

            MsgType::GoAway => {
                info!("received GOAWAY");
                draining = true;
                let deadline_ms =
                    serde_json::from_slice::<aether_contracts::tunnel::GoAwayPayload>(
                        &frame.payload,
                    )
                    .map(|payload| payload.drain_deadline_ms)
                    .unwrap_or(state.config.tunnel_drain_deadline_ms)
                    .min(state.config.tunnel_drain_deadline_ms);
                drain_deadline.get_or_insert_with(|| {
                    tokio::time::Instant::now() + Duration::from_millis(deadline_ms)
                });
            }

            MsgType::WindowUpdate => {
                if let Ok(payload) = serde_json::from_slice::<
                    aether_contracts::tunnel::WindowUpdatePayload,
                >(&frame.payload)
                {
                    if let Some(target) = streams.get(&frame.stream_id) {
                        target.response_window.add_credit(payload.delta_bytes);
                    }
                }
                debug!(
                    msg_type = ?frame.msg_type,
                    stream_id = frame.stream_id,
                    "received tunnel protocol v3 WINDOW_UPDATE frame"
                );
            }

            MsgType::Settings => {
                if frame.stream_id != 0 || frame.flags != 0 {
                    break None;
                }
                let settings = serde_json::from_slice::<aether_contracts::tunnel::SettingsPayload>(
                    &frame.payload,
                )
                .ok()
                .filter(|settings| settings.is_valid());
                let Some(settings) = settings else {
                    warn!("invalid tunnel SETTINGS");
                    break None;
                };
                if !streams.is_empty()
                    && settings.initial_stream_window_bytes != initial_window_bytes
                {
                    warn!("tunnel SETTINGS changed with active streams");
                    break None;
                }
                initial_window_bytes = settings
                    .initial_stream_window_bytes
                    .min(state.config.tunnel_stream_initial_window_bytes);
            }

            MsgType::Hello | MsgType::LoadReport => {
                debug!(
                    msg_type = ?frame.msg_type,
                    stream_id = frame.stream_id,
                    "received tunnel protocol v3 control frame"
                );
            }

            MsgType::ConnectionClose => {
                info!("received CONNECTION_CLOSE");
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
            while handler_handles.try_join_next().is_some() {}
            frames_since_cleanup = 0;
            if draining && streams.is_empty() && active_handler_ids.is_empty() {
                info!("tunnel drained after cleanup");
                break None;
            }
        }
    };

    // Drop body senders so stream handlers waiting on body_rx will unblock
    streams.clear();

    handler_handles.shutdown().await;

    match read_err {
        Some(e) => Err(e.into()),
        None => Ok(()),
    }
}

async fn dispatch_stream_frame(tx: &mpsc::Sender<Frame>, frame: Frame) -> StreamDispatchStatus {
    let Some(frame) = attach_request_body_queue_budget(frame).await else {
        return StreamDispatchStatus::Congested;
    };
    match tx.try_send(frame) {
        Ok(()) => StreamDispatchStatus::Delivered,
        Err(mpsc::error::TrySendError::Closed(_)) => StreamDispatchStatus::Closed,
        Err(mpsc::error::TrySendError::Full(_)) => StreamDispatchStatus::Congested,
    }
}

async fn attach_request_body_queue_budget(frame: Frame) -> Option<Frame> {
    attach_request_body_queue_budget_with(
        frame,
        Arc::clone(&REQUEST_BODY_QUEUE_BUDGET),
        REQUEST_BODY_QUEUE_BUDGET_BYTES,
    )
    .await
}

async fn attach_request_body_queue_budget_with(
    mut frame: Frame,
    budget: Arc<Semaphore>,
    budget_bytes: usize,
) -> Option<Frame> {
    if frame.msg_type != MsgType::RequestBody {
        return Some(frame);
    }
    let permits = request_body_queue_permits(&frame, budget_bytes)?;
    let permit = budget.try_acquire_many_owned(permits).ok()?;
    frame.payload = Bytes::from_owner(BudgetedFramePayload {
        bytes: frame.payload,
        _permit: permit,
    });
    Some(frame)
}

fn request_body_queue_permits(frame: &Frame, budget_bytes: usize) -> Option<u32> {
    let decoded_budget = if frame.is_gzip() {
        aether_contracts::tunnel::MAX_TUNNEL_DECOMPRESSED_PAYLOAD_BYTES
    } else {
        0
    };
    let retained_bytes = frame
        .payload
        .len()
        .checked_add(decoded_budget)?
        .checked_add(size_of::<Frame>())?
        .max(1);
    if retained_bytes > budget_bytes {
        return None;
    }
    u32::try_from(retained_bytes).ok()
}

fn try_send_stream_error(frame_tx: &FrameSender, stream_id: u32, message: &'static str) {
    if frame_tx
        .try_send(Frame::new(
            stream_id,
            MsgType::StreamError,
            0,
            Bytes::from(message),
        ))
        .is_err()
    {
        frame_tx.close();
        warn!(
            stream_id,
            "writer channel full, StreamError dropped while aborting stalled stream"
        );
    }
}

#[cfg(test)]
fn prune_closed_stream_senders(streams: &mut HashMap<u32, StreamDispatchTarget>) -> usize {
    let before = streams.len();
    streams.retain(|_, target| !target.body_tx.is_closed());
    before.saturating_sub(streams.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_contracts::tunnel::{compress_payload, flags};
    use aether_runtime::bounded_queue;

    #[tokio::test]
    async fn dispatch_stream_frame_times_out_when_handler_stops_draining() {
        let (tx, mut rx) = mpsc::channel::<Frame>(1);
        tx.send(Frame::new(
            7,
            MsgType::RequestBody,
            0,
            Bytes::from_static(b"first"),
        ))
        .await
        .expect("first frame should enqueue");

        let stalled_send = tokio::spawn({
            let tx = tx.clone();
            async move {
                dispatch_stream_frame(
                    &tx,
                    Frame::new(7, MsgType::RequestBody, 0, Bytes::from_static(b"second")),
                )
                .await
            }
        });

        assert_eq!(
            stalled_send.await.expect("dispatch task should join"),
            StreamDispatchStatus::Congested
        );

        let retained = rx
            .recv()
            .await
            .expect("queued frame should still be present");
        assert_eq!(retained.payload, Bytes::from_static(b"first"));
    }

    #[tokio::test]
    async fn request_body_queue_budget_releases_when_frame_is_dropped() {
        const BUDGET_BYTES: usize = 4096;
        let budget = Arc::new(Semaphore::new(BUDGET_BYTES));
        let frame = Frame::new(
            7,
            MsgType::RequestBody,
            0,
            Bytes::from_static(b"request body"),
        );
        let permits = request_body_queue_permits(&frame, BUDGET_BYTES).expect("permit count");

        let frame = attach_request_body_queue_budget_with(frame, Arc::clone(&budget), BUDGET_BYTES)
            .await
            .expect("frame should fit the queue budget");
        assert_eq!(budget.available_permits(), BUDGET_BYTES - permits as usize);

        drop(frame);
        assert_eq!(budget.available_permits(), BUDGET_BYTES);
    }

    #[tokio::test]
    async fn gzip_request_body_budget_follows_decoded_payload_lifetime() {
        let (payload, frame_flags) = compress_payload(Bytes::from(vec![b'x'; 1024]));
        assert_eq!(frame_flags, flags::GZIP_COMPRESSED);
        let frame = Frame::new(7, MsgType::RequestBody, frame_flags, payload);
        let required = request_body_queue_permits(&frame, REQUEST_BODY_QUEUE_BUDGET_BYTES)
            .expect("gzip frame should fit the queue budget") as usize;
        let budget = Arc::new(Semaphore::new(required));
        let frame = attach_request_body_queue_budget_with(frame, Arc::clone(&budget), required)
            .await
            .expect("frame should acquire the entire local budget");
        assert_eq!(budget.available_permits(), 0);

        let decoded = stream_handler::decode_request_body_frame(frame)
            .expect("gzip request body should decode");
        assert_eq!(decoded, Bytes::from(vec![b'x'; 1024]));
        assert_eq!(budget.available_permits(), 0);

        drop(decoded);
        assert_eq!(budget.available_permits(), required);
    }

    #[tokio::test]
    async fn gzip_request_body_budget_releases_after_decode_error() {
        let frame = Frame::new(
            7,
            MsgType::RequestBody,
            flags::GZIP_COMPRESSED,
            Bytes::from_static(b"not gzip"),
        );
        let required = request_body_queue_permits(&frame, REQUEST_BODY_QUEUE_BUDGET_BYTES)
            .expect("gzip frame should fit the queue budget") as usize;
        let budget = Arc::new(Semaphore::new(required));
        let frame = attach_request_body_queue_budget_with(frame, Arc::clone(&budget), required)
            .await
            .expect("frame should acquire the entire local budget");
        assert_eq!(budget.available_permits(), 0);

        stream_handler::decode_request_body_frame(frame)
            .expect_err("invalid gzip request body should fail");
        assert_eq!(budget.available_permits(), required);
    }

    #[tokio::test]
    async fn try_send_stream_error_emits_stream_error_frame() {
        let (high_tx, mut high_rx) = bounded_queue::<Frame>(4);
        let (normal_tx, _normal_rx) = bounded_queue::<Frame>(4);
        let frame_tx = FrameSender::from_test_queues(high_tx, normal_tx);
        try_send_stream_error(&frame_tx, 9, "tunnel request body dispatch stalled");

        let frame = high_rx
            .recv()
            .await
            .expect("stream error frame should enqueue");
        assert_eq!(frame.stream_id, 9);
        assert_eq!(frame.msg_type, MsgType::StreamError);
        assert_eq!(
            frame.payload,
            Bytes::from_static(b"tunnel request body dispatch stalled")
        );
    }

    #[test]
    fn prune_closed_stream_senders_drops_streams_with_closed_receivers() {
        let (closed_tx, closed_rx) = mpsc::channel::<Frame>(1);
        let (open_tx, _open_rx) = mpsc::channel::<Frame>(1);
        drop(closed_rx);
        let mut streams = HashMap::from([
            (
                7,
                StreamDispatchTarget {
                    body_tx: closed_tx,
                    response_window: Arc::new(StreamSendWindow::new(1024)),
                    handler: None,
                },
            ),
            (
                9,
                StreamDispatchTarget {
                    body_tx: open_tx,
                    response_window: Arc::new(StreamSendWindow::new(1024)),
                    handler: None,
                },
            ),
        ]);

        let removed = prune_closed_stream_senders(&mut streams);

        assert_eq!(removed, 1);
        assert!(!streams.contains_key(&7));
        assert!(streams.contains_key(&9));
    }

    #[test]
    fn request_stream_id_rejects_zero_and_active_duplicates() {
        let (tx, _rx) = mpsc::channel::<Frame>(1);
        let streams = HashMap::from([(
            7,
            StreamDispatchTarget {
                body_tx: tx,
                response_window: Arc::new(StreamSendWindow::new(1024)),
                handler: None,
            },
        )]);
        let mut active_handler_ids = HashSet::from([7]);

        assert_eq!(
            validate_request_stream_id(&streams, &active_handler_ids, 0),
            Err("invalid stream id")
        );
        assert_eq!(
            validate_request_stream_id(&streams, &active_handler_ids, 7),
            Err("duplicate stream id")
        );
        assert_eq!(
            validate_request_stream_id(&streams, &active_handler_ids, 9),
            Ok(())
        );

        // The routing entry may be removed after a dispatch failure while the
        // handler is still running; its reservation must continue to reject
        // a new request with the same id.
        active_handler_ids.insert(11);
        assert_eq!(
            validate_request_stream_id(&HashMap::new(), &active_handler_ids, 11),
            Err("duplicate stream id")
        );
    }
}
