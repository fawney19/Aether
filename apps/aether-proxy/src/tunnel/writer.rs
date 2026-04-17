//! Dedicated WebSocket writer task.
//!
//! All frame writes go through an mpsc channel to a single writer task,
//! avoiding contention on the WebSocket sink.  The writer also sends
//! periodic WebSocket Ping frames to keep the connection alive through
//! intermediary proxies (Nginx, Cloudflare, etc.).

use std::time::Duration;

use aether_contracts::tunnel::MsgType;
#[cfg(test)]
use aether_runtime::QueueSnapshot;
use aether_runtime::{bounded_queue, BoundedQueueSender, QueueSendError};
use futures_util::SinkExt;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, trace};

use super::protocol::Frame;

const HIGH_PRIORITY_QUEUE_CAPACITY: usize = 64;
const NORMAL_PRIORITY_QUEUE_CAPACITY: usize = 256;

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
}

impl FrameSender {
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
        Self { high_tx, normal_tx }
    }
}

/// Spawn the writer task. Returns the sender and a JoinHandle for cleanup.
///
/// `ping_interval` controls WebSocket-level Ping frequency (typically 15s).
/// This keeps the connection alive through intermediary proxies/load-balancers.
pub fn spawn_writer<S>(mut sink: S, ping_interval: Duration) -> (FrameSender, JoinHandle<()>)
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send + 'static,
{
    let (high_tx, mut high_rx) = bounded_queue::<Frame>(HIGH_PRIORITY_QUEUE_CAPACITY);
    let (normal_tx, mut normal_rx) = bounded_queue::<Frame>(NORMAL_PRIORITY_QUEUE_CAPACITY);
    let tx = FrameSender { high_tx, normal_tx };

    let handle = tokio::spawn(async move {
        let mut ping_ticker = tokio::time::interval(ping_interval);
        let mut high_open = true;
        let mut normal_open = true;
        ping_ticker.tick().await; // skip first immediate tick

        loop {
            if let Ok(frame) = high_rx.try_recv() {
                if !write_frame(&mut sink, frame).await {
                    break;
                }
                continue;
            }
            if !high_open && !normal_open {
                break;
            }

            tokio::select! {
                biased;
                frame = high_rx.recv(), if high_open => {
                    match frame {
                        Some(frame) => {
                            if !write_frame(&mut sink, frame).await {
                                break;
                            }
                        }
                        None => high_open = false,
                    }
                }
                _ = ping_ticker.tick(), if high_open || normal_open => {
                    if let Err(e) = sink.send(Message::Ping(vec![])).await {
                        error!(error = %e, "failed to send WebSocket ping");
                        break;
                    }
                    trace!("sent WebSocket ping");
                }
                frame = normal_rx.recv(), if normal_open => {
                    match frame {
                        Some(frame) => {
                            if !write_frame(&mut sink, frame).await {
                                break;
                            }
                        }
                        None => normal_open = false,
                    }
                }
            }
        }
        debug!("writer task exiting");
        let _ = sink.close().await;
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
        | MsgType::HeartbeatAck => FramePriority::High,
        MsgType::RequestHeaders
        | MsgType::RequestBody
        | MsgType::ResponseBody
        | MsgType::StreamEnd => FramePriority::Normal,
    }
}

async fn write_frame<S>(sink: &mut S, frame: Frame) -> bool
where
    S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin + Send + 'static,
{
    let data = frame.encode();
    if let Err(e) = sink.send(Message::Binary(data.into())).await {
        error!(error = %e, "failed to write frame to WebSocket");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
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
