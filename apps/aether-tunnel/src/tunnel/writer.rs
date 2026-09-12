//! Dedicated WebSocket writer task.
//!
//! All frame writes go through an mpsc channel to a single writer task,
//! avoiding contention on the WebSocket sink.  The writer also sends
//! periodic WebSocket Ping frames to keep the connection alive through
//! intermediary proxies (Nginx, Cloudflare, etc.).

use std::sync::Arc;
use std::time::Duration;

use aether_contracts::tunnel::{MsgType, HEADER_SIZE};
#[cfg(test)]
use aether_runtime::QueueSnapshot;
use aether_runtime::{bounded_queue, BoundedQueueSender, QueueSendError};
use futures_util::SinkExt;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, trace};

use crate::state::TunnelMetrics;

use super::protocol::Frame;
use aether_contracts::tunnel_security::SecureFrameCodec;

const HIGH_PRIORITY_QUEUE_CAPACITY: usize = 64;
const NORMAL_PRIORITY_QUEUE_CAPACITY: usize = 256;
const WRITE_TIMEOUT: Duration = Duration::from_secs(15);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FramePriority {
    High,
    Normal,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameQueueSnapshots {
    pub high: QueueSnapshot,
    pub normal: QueueSnapshot,
}

/// Sender half — cloned by stream handlers and heartbeat.
#[derive(Debug, Clone)]
pub struct FrameSender {
    high_tx: BoundedQueueSender<Frame>,
    normal_tx: BoundedQueueSender<Frame>,
    close_tx: watch::Sender<bool>,
}

impl FrameSender {
    pub fn close(&self) {
        let _ = self.close_tx.send(true);
    }

    pub(super) fn subscribe_close(&self) -> watch::Receiver<bool> {
        self.close_tx.subscribe()
    }

    pub async fn send(&self, frame: Frame) -> Result<(), QueueSendError<Frame>> {
        match classify_frame_priority(&frame) {
            FramePriority::High => self.high_tx.send(frame).await,
            FramePriority::Normal => self.normal_tx.send(frame).await,
        }
    }

    pub fn try_send(&self, frame: Frame) -> Result<(), QueueSendError<Frame>> {
        match classify_frame_priority(&frame) {
            FramePriority::High => self.high_tx.try_send(frame),
            FramePriority::Normal => self.normal_tx.try_send(frame),
        }
    }

    #[cfg(test)]
    pub fn snapshots(&self) -> FrameQueueSnapshots {
        FrameQueueSnapshots {
            high: self.high_tx.snapshot(),
            normal: self.normal_tx.snapshot(),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_queues(
        high_tx: BoundedQueueSender<Frame>,
        normal_tx: BoundedQueueSender<Frame>,
    ) -> Self {
        let (close_tx, _) = watch::channel(false);
        Self {
            high_tx,
            normal_tx,
            close_tx,
        }
    }
}

/// Spawn the writer task. Returns the sender and a JoinHandle for cleanup.
///
/// `ping_interval` controls WebSocket-level Ping frequency (typically 15s).
/// This keeps the connection alive through intermediary proxies/load-balancers.
#[cfg(test)]
pub fn spawn_writer<S>(sink: S, ping_interval: Duration) -> (FrameSender, JoinHandle<()>)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send + 'static,
{
    spawn_writer_with_metrics(sink, ping_interval, None)
}

/// Spawn the writer task with optional tunnel metrics instrumentation.
#[allow(dead_code)]
pub fn spawn_writer_with_metrics<S>(
    sink: S,
    ping_interval: Duration,
    tunnel_metrics: Option<Arc<TunnelMetrics>>,
) -> (FrameSender, JoinHandle<()>)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send + 'static,
{
    spawn_writer_with_metrics_and_security(sink, ping_interval, tunnel_metrics, None)
}

pub fn spawn_writer_with_metrics_and_security<S>(
    mut sink: S,
    ping_interval: Duration,
    tunnel_metrics: Option<Arc<TunnelMetrics>>,
    security: Option<Arc<SecureFrameCodec>>,
) -> (FrameSender, JoinHandle<()>)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send + 'static,
{
    let (high_tx, mut high_rx) = bounded_queue::<Frame>(HIGH_PRIORITY_QUEUE_CAPACITY);
    let (normal_tx, mut normal_rx) = bounded_queue::<Frame>(NORMAL_PRIORITY_QUEUE_CAPACITY);
    let (close_tx, mut close_rx) = watch::channel(false);
    let tx = FrameSender {
        high_tx,
        normal_tx,
        close_tx,
    };

    let handle = tokio::spawn(async move {
        let mut ping_ticker = tokio::time::interval(ping_interval);
        let mut high_open = true;
        let mut normal_open = true;
        let mut close_open = true;
        ping_ticker.tick().await; // skip first immediate tick

        loop {
            if *close_rx.borrow() {
                break;
            }
            if let Ok(frame) = high_rx.try_recv() {
                if !write_frame(
                    &mut sink,
                    frame,
                    tunnel_metrics.as_deref(),
                    security.as_deref(),
                )
                .await
                {
                    break;
                }
                continue;
            }
            if !high_open && !normal_open {
                break;
            }

            tokio::select! {
                biased;
                changed = close_rx.changed(), if close_open => {
                    if changed.is_err() { close_open = false; }
                    if *close_rx.borrow() { break; }
                },
                frame = high_rx.recv(), if high_open => {
                    match frame {
                        Some(frame) => {
                            if !write_frame(&mut sink, frame, tunnel_metrics.as_deref(), security.as_deref()).await {
                                break;
                            }
                        }
                        None => high_open = false,
                    }
                }
                _ = ping_ticker.tick(), if high_open || normal_open => {
                    if let Err(e) = send_message(&mut sink, Message::Ping(vec![])).await {
                        error!(error = %e, "failed to send WebSocket ping");
                        if let Some(metrics) = tunnel_metrics.as_deref() {
                            metrics.record_error("ws_ping_error", &e.to_string());
                        }
                        break;
                    }
                    trace!("sent WebSocket ping");
                }
                frame = normal_rx.recv(), if normal_open => {
                    match frame {
                        Some(frame) => {
                            if !write_frame(&mut sink, frame, tunnel_metrics.as_deref(), security.as_deref()).await {
                                break;
                            }
                        }
                        None => normal_open = false,
                    }
                }
            }
        }
        debug!("writer task exiting");
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, sink.close()).await;
    });

    (tx, handle)
}

fn classify_frame_priority(frame: &Frame) -> FramePriority {
    match frame.msg_type {
        MsgType::ResponseHeaders
        | MsgType::StreamError
        | MsgType::Ping
        | MsgType::Pong
        | MsgType::GoAway
        | MsgType::HeartbeatData
        | MsgType::HeartbeatAck
        | MsgType::Hello
        | MsgType::Settings
        | MsgType::WindowUpdate
        | MsgType::ResetStream
        | MsgType::ConnectionClose
        | MsgType::LoadReport => FramePriority::High,
        MsgType::RequestHeaders
        | MsgType::RequestBody
        | MsgType::ResponseBody
        | MsgType::StreamEnd => FramePriority::Normal,
    }
}

async fn write_frame<S>(
    sink: &mut S,
    frame: Frame,
    tunnel_metrics: Option<&TunnelMetrics>,
    security: Option<&SecureFrameCodec>,
) -> bool
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send + 'static,
{
    let stream_id = frame.stream_id;
    let msg_type = frame.msg_type;
    let flags = frame.flags;
    let data = match security {
        Some(codec) => match codec.encrypt_frame(frame) {
            Ok(data) => data,
            Err(e) => {
                error!(error = %e, "failed to encrypt tunnel frame");
                if let Some(metrics) = tunnel_metrics {
                    metrics.record_error("secure_frame_encrypt_error", &e.to_string());
                }
                return false;
            }
        },
        None => frame.encode(),
    };
    let wire_len = data.len().max(HEADER_SIZE);
    if let Err(e) = send_message(sink, Message::Binary(data.into())).await {
        error!(
            stream_id = stream_id,
            msg_type = ?msg_type,
            flags = flags,
            wire_len = wire_len,
            error = %e,
            "failed to write frame to WebSocket"
        );
        if let Some(metrics) = tunnel_metrics {
            metrics.record_error("ws_write_error", &e.to_string());
        }
        return false;
    }
    if let Some(metrics) = tunnel_metrics {
        metrics.record_ws_outgoing_frame(wire_len);
    }
    true
}

async fn send_message<S>(
    sink: &mut S,
    message: Message,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    tokio::time::timeout(WRITE_TIMEOUT, sink.send(message))
        .await
        .map_err(|_| {
            tokio_tungstenite::tungstenite::Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "tunnel WebSocket write timed out",
            ))
        })?
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn dropping_last_sender_flushes_queued_body_and_end_frames() {
        let sink = VecSink::default();
        let sent = Arc::clone(&sink.sent);
        let (sender, task) = spawn_writer(sink, Duration::from_secs(60));
        sender
            .send(Frame::new(
                7,
                MsgType::ResponseBody,
                0,
                bytes::Bytes::from_static(b"late"),
            ))
            .await
            .unwrap();
        sender
            .send(Frame::new(7, MsgType::StreamEnd, 0, bytes::Bytes::new()))
            .await
            .unwrap();
        drop(sender);
        task.await.unwrap();
        let frames = sent.lock().unwrap();
        assert_eq!(frames.len(), 2);
        let Message::Binary(body) = &frames[0] else {
            panic!("expected body")
        };
        assert_eq!(
            Frame::decode(body.clone().into()).unwrap().payload,
            b"late".as_slice()
        );
    }

    struct StalledSink;

    impl futures_util::Sink<Message> for StalledSink {
        type Error = Error;
        fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
            Poll::Pending
        }
        fn start_send(self: Pin<&mut Self>, _: Message) -> Result<(), Error> {
            Ok(())
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
            Poll::Pending
        }
        fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Error>> {
            Poll::Pending
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_socket_write_and_close_are_bounded() {
        let (sender, task) = spawn_writer(StalledSink, Duration::from_secs(60));
        sender
            .send(Frame::new(
                1,
                MsgType::ResponseBody,
                0,
                bytes::Bytes::from_static(b"data"),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("writer should time out")
            .unwrap();
    }

    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use futures_util::Sink;
    use tokio_tungstenite::tungstenite::{Error, Message};

    use super::spawn_writer;
    use crate::tunnel::protocol::Frame;
    use aether_contracts::tunnel::MsgType;

    #[derive(Clone, Default)]
    struct VecSink {
        sent: Arc<Mutex<Vec<Message>>>,
    }

    impl Sink<Message> for VecSink {
        type Error = Error;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.sent.lock().expect("sink lock").push(item);
            Ok(())
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn prioritizes_control_frames_ahead_of_buffered_body_frames() {
        let sink = VecSink::default();
        let sent = Arc::clone(&sink.sent);
        let (sender, handle) = spawn_writer(sink, Duration::from_secs(60));

        for idx in 0..8u8 {
            sender
                .try_send(Frame::new(
                    7,
                    MsgType::ResponseBody,
                    0,
                    bytes::Bytes::from(vec![idx; 32]),
                ))
                .expect("frame send should succeed");
        }
        sender
            .try_send(Frame::new(
                7,
                MsgType::StreamError,
                0,
                bytes::Bytes::from_static(b"boom"),
            ))
            .expect("frame send should succeed");
        let snapshots = sender.snapshots();
        assert!(snapshots.high.enqueued_total >= 1);
        assert!(snapshots.normal.enqueued_total >= 8);

        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(sender);
        handle.await.expect("writer should exit cleanly");

        let sent = sent.lock().expect("sink lock");
        assert!(
            sent.len() >= 2,
            "writer should flush both body and control frames"
        );
        let first = match &sent[0] {
            Message::Binary(data) => {
                Frame::decode(data.clone().into()).expect("frame should decode")
            }
            other => panic!("unexpected first message: {other:?}"),
        };
        let second = match &sent[1] {
            Message::Binary(data) => {
                Frame::decode(data.clone().into()).expect("frame should decode")
            }
            other => panic!("unexpected second message: {other:?}"),
        };
        assert_eq!(first.msg_type, MsgType::StreamError);
        assert_eq!(second.msg_type, MsgType::ResponseBody);
    }
}
