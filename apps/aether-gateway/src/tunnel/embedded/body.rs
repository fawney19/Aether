use std::collections::VecDeque;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use parking_lot::Mutex;
use tokio::sync::Notify;

const CHUNK_BYTES: usize = 32 * 1024;

#[derive(Debug)]
pub enum LocalBodyEvent {
    Chunk(Bytes),
    End,
    Error(String),
}

#[derive(Default)]
struct BufferState {
    chunks: VecDeque<BytesMut>,
    bytes: usize,
    terminal: Option<Result<(), String>>,
    receiver_taken: bool,
    receiver_closed: bool,
}

pub(super) struct ResponseBuffer {
    state: Mutex<BufferState>,
    notify: Notify,
    capacity: usize,
}

impl ResponseBuffer {
    pub(super) fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(BufferState::default()),
            notify: Notify::new(),
            capacity,
        })
    }

    pub(super) fn take_receiver(self: &Arc<Self>) -> Option<BodyReceiver> {
        let mut state = self.state.lock();
        if state.receiver_taken {
            return None;
        }
        state.receiver_taken = true;
        Some(BodyReceiver {
            buffer: Arc::clone(self),
            finished: false,
        })
    }

    pub(super) fn push(&self, mut payload: Bytes) -> bool {
        let mut state = self.state.lock();
        if state.terminal.is_some()
            || state.receiver_closed
            || payload.len() > self.capacity.saturating_sub(state.bytes)
        {
            return false;
        }
        state.bytes += payload.len();
        while !payload.is_empty() {
            if let Some(tail) = state
                .chunks
                .back_mut()
                .filter(|chunk| chunk.len() < CHUNK_BYTES)
            {
                let count = payload.len().min(CHUNK_BYTES - tail.len());
                tail.extend_from_slice(&payload.split_to(count));
            } else {
                let count = payload.len().min(CHUNK_BYTES);
                let chunk = payload.split_to(count);
                state.chunks.push_back(
                    chunk
                        .try_into_mut()
                        .unwrap_or_else(|chunk| BytesMut::from(chunk.as_ref())),
                );
            }
        }
        drop(state);
        self.notify.notify_waiters();
        true
    }

    pub(super) fn finish(&self, result: Result<(), String>) {
        let mut state = self.state.lock();
        if state.terminal.is_none() {
            state.terminal = Some(result);
        }
        drop(state);
        self.notify.notify_waiters();
    }
}

pub(super) struct BodyReceiver {
    buffer: Arc<ResponseBuffer>,
    finished: bool,
}

impl BodyReceiver {
    pub(super) async fn recv(&mut self) -> Option<LocalBodyEvent> {
        if self.finished {
            return None;
        }
        loop {
            let notified = self.buffer.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.buffer.state.lock();
                if let Some(chunk) = state.chunks.pop_front() {
                    state.bytes -= chunk.len();
                    return Some(LocalBodyEvent::Chunk(chunk.freeze()));
                }
                if let Some(terminal) = state.terminal.take() {
                    self.finished = true;
                    state.receiver_closed = true;
                    return Some(match terminal {
                        Ok(()) => LocalBodyEvent::End,
                        Err(error) => LocalBodyEvent::Error(error),
                    });
                }
            }
            notified.await;
        }
    }
}

impl Drop for BodyReceiver {
    fn drop(&mut self) {
        let mut state = self.buffer.state.lock();
        state.receiver_closed = true;
        state.chunks.clear();
        state.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn error_survives_a_full_buffer() {
        let buffer = ResponseBuffer::new(CHUNK_BYTES);
        let mut receiver = buffer.take_receiver().unwrap();
        assert!(buffer.push(Bytes::from(vec![b'x'; CHUNK_BYTES])));
        buffer.finish(Err("proxy disconnected".into()));
        assert!(matches!(
            receiver.recv().await,
            Some(LocalBodyEvent::Chunk(_))
        ));
        assert!(
            matches!(receiver.recv().await, Some(LocalBodyEvent::Error(error)) if error == "proxy disconnected")
        );
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn small_frames_are_coalesced_within_the_byte_budget() {
        let buffer = ResponseBuffer::new(4096);
        let mut receiver = buffer.take_receiver().unwrap();
        for _ in 0..4096 {
            assert!(buffer.push(Bytes::from_static(b"x")));
        }
        assert!(!buffer.push(Bytes::from_static(b"x")));
        assert_eq!(buffer.state.lock().chunks.len(), 1);
        buffer.finish(Ok(()));
        assert!(
            matches!(receiver.recv().await, Some(LocalBodyEvent::Chunk(chunk)) if chunk.len() == 4096)
        );
        assert!(matches!(receiver.recv().await, Some(LocalBodyEvent::End)));
    }

    #[tokio::test]
    async fn terminal_wakes_an_empty_receiver_and_is_not_overwritten() {
        let buffer = ResponseBuffer::new(1024);
        let mut receiver = buffer.take_receiver().unwrap();
        let task = tokio::spawn(async move { receiver.recv().await });
        tokio::task::yield_now().await;
        buffer.finish(Err("cancelled".into()));
        buffer.finish(Ok(()));
        assert!(
            matches!(task.await.unwrap(), Some(LocalBodyEvent::Error(error)) if error == "cancelled")
        );
    }
}
