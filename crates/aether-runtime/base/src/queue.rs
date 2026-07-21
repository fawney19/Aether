use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::metrics::{MetricKind, MetricLabel, MetricSample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueSnapshot {
    pub capacity: usize,
    pub depth: usize,
    pub high_watermark: usize,
    pub enqueued_total: u64,
    pub rejected_full_total: u64,
    pub rejected_closed_total: u64,
}

impl QueueSnapshot {
    pub fn to_metric_samples(&self, queue: &'static str) -> Vec<MetricSample> {
        let labels = vec![MetricLabel::new("queue", queue)];
        vec![
            MetricSample::new(
                "queue_depth",
                "Current number of items buffered in the queue.",
                MetricKind::Gauge,
                self.depth as u64,
            )
            .with_labels(labels.clone()),
            MetricSample::new(
                "queue_high_watermark",
                "Highest observed queue depth.",
                MetricKind::Gauge,
                self.high_watermark as u64,
            )
            .with_labels(labels.clone()),
            MetricSample::new(
                "queue_enqueued_total",
                "Total number of items successfully enqueued.",
                MetricKind::Counter,
                self.enqueued_total,
            )
            .with_labels(labels.clone()),
            MetricSample::new(
                "queue_rejected_full_total",
                "Total number of items rejected because the queue was full.",
                MetricKind::Counter,
                self.rejected_full_total,
            )
            .with_labels(labels.clone()),
            MetricSample::new(
                "queue_rejected_closed_total",
                "Total number of items rejected because the queue was closed.",
                MetricKind::Counter,
                self.rejected_closed_total,
            )
            .with_labels(labels),
        ]
    }
}

#[derive(Debug)]
struct QueueState {
    capacity: usize,
    depth: AtomicUsize,
    high_watermark: AtomicUsize,
    enqueued_total: AtomicU64,
    rejected_full_total: AtomicU64,
    rejected_closed_total: AtomicU64,
}

#[derive(Debug)]
pub enum QueueSendError<T> {
    Full(T),
    Closed(T),
}

#[derive(Debug, Clone)]
pub struct BoundedQueueSender<T> {
    inner: mpsc::Sender<T>,
    state: Arc<QueueState>,
}

#[derive(Debug)]
pub struct BoundedQueueReceiver<T> {
    inner: mpsc::Receiver<T>,
    state: Arc<QueueState>,
}

pub fn bounded_queue<T>(capacity: usize) -> (BoundedQueueSender<T>, BoundedQueueReceiver<T>) {
    assert!(capacity > 0, "bounded queue capacity must be positive");
    let (tx, rx) = mpsc::channel(capacity);
    let state = Arc::new(QueueState {
        capacity,
        depth: AtomicUsize::new(0),
        high_watermark: AtomicUsize::new(0),
        enqueued_total: AtomicU64::new(0),
        rejected_full_total: AtomicU64::new(0),
        rejected_closed_total: AtomicU64::new(0),
    });
    (
        BoundedQueueSender {
            inner: tx,
            state: state.clone(),
        },
        BoundedQueueReceiver { inner: rx, state },
    )
}

impl<T> BoundedQueueSender<T> {
    pub async fn send(&self, value: T) -> Result<(), QueueSendError<T>> {
        let permit = match self.inner.reserve().await {
            Ok(permit) => permit,
            Err(_) => {
                self.state
                    .rejected_closed_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(QueueSendError::Closed(value));
            }
        };
        self.record_enqueue();
        permit.send(value);
        Ok(())
    }

    pub fn try_send(&self, value: T) -> Result<(), QueueSendError<T>> {
        let permit = match self.inner.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.state
                    .rejected_full_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(QueueSendError::Full(value));
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.state
                    .rejected_closed_total
                    .fetch_add(1, Ordering::Relaxed);
                return Err(QueueSendError::Closed(value));
            }
        };
        self.record_enqueue();
        permit.send(value);
        Ok(())
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            capacity: self.state.capacity,
            depth: self.state.depth.load(Ordering::Relaxed),
            high_watermark: self.state.high_watermark.load(Ordering::Relaxed),
            enqueued_total: self.state.enqueued_total.load(Ordering::Relaxed),
            rejected_full_total: self.state.rejected_full_total.load(Ordering::Relaxed),
            rejected_closed_total: self.state.rejected_closed_total.load(Ordering::Relaxed),
        }
    }

    pub fn capacity(&self) -> usize {
        self.state.capacity
    }

    fn record_enqueue(&self) {
        let depth = self.state.depth.fetch_add(1, Ordering::AcqRel) + 1;
        self.state.enqueued_total.fetch_add(1, Ordering::Relaxed);
        let mut observed = self.state.high_watermark.load(Ordering::Acquire);
        while depth > observed {
            match self.state.high_watermark.compare_exchange_weak(
                observed,
                depth,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(next) => observed = next,
            }
        }
    }
}

impl<T> BoundedQueueReceiver<T> {
    pub async fn recv(&mut self) -> Option<T> {
        let value = self.inner.recv().await?;
        self.state.depth.fetch_sub(1, Ordering::AcqRel);
        Some(value)
    }

    pub fn try_recv(&mut self) -> Result<T, mpsc::error::TryRecvError> {
        let value = self.inner.try_recv()?;
        self.state.depth.fetch_sub(1, Ordering::AcqRel);
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{bounded_queue, QueueSendError};

    #[tokio::test]
    async fn tracks_queue_depth_and_high_watermark() {
        let (tx, mut rx) = bounded_queue::<u32>(2);
        tx.send(1).await.expect("enqueue 1");
        tx.send(2).await.expect("enqueue 2");

        let snapshot = tx.snapshot();
        assert_eq!(snapshot.depth, 2);
        assert_eq!(snapshot.high_watermark, 2);
        assert_eq!(snapshot.enqueued_total, 2);

        assert_eq!(rx.recv().await, Some(1));
        assert_eq!(tx.snapshot().depth, 1);
    }

    #[test]
    fn counts_full_rejections() {
        let (tx, _rx) = bounded_queue::<u32>(1);
        tx.try_send(1).expect("first send should work");
        let error = tx.try_send(2).expect_err("second send should fail");
        assert!(matches!(error, QueueSendError::Full(2)));
        assert_eq!(tx.snapshot().rejected_full_total, 1);
    }

    #[tokio::test]
    async fn send_does_not_underflow_depth_when_receiver_races() {
        let (tx, mut rx) = bounded_queue::<u32>(1);
        let receiver = tokio::spawn(async move {
            for _ in 0..256 {
                assert!(rx.recv().await.is_some());
            }
        });

        for value in 0..256 {
            tx.send(value).await.expect("send should succeed");
        }

        receiver.await.expect("receiver task should join");
        let snapshot = tx.snapshot();
        assert_eq!(snapshot.depth, 0);
        assert!(snapshot.high_watermark <= 1);
        assert_eq!(snapshot.enqueued_total, 256);
    }
}
