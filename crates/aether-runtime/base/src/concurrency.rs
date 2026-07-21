use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::metrics::{MetricKind, MetricLabel, MetricSample};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConcurrencyError {
    #[error("concurrency gate {gate} is saturated at {limit}")]
    Saturated { gate: &'static str, limit: usize },
    #[error("concurrency gate {gate} is closed")]
    Closed { gate: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcurrencySnapshot {
    pub limit: usize,
    pub in_flight: usize,
    pub available_permits: usize,
    pub high_watermark: usize,
    pub rejected: u64,
}

impl ConcurrencySnapshot {
    pub fn to_metric_samples(&self, gate: &'static str) -> Vec<MetricSample> {
        let labels = vec![MetricLabel::new("gate", gate)];
        vec![
            MetricSample::new(
                "concurrency_in_flight",
                "Current number of in-flight operations guarded by the concurrency gate.",
                MetricKind::Gauge,
                self.in_flight as u64,
            )
            .with_labels(labels.clone()),
            MetricSample::new(
                "concurrency_available_permits",
                "Currently available permits for the concurrency gate.",
                MetricKind::Gauge,
                self.available_permits as u64,
            )
            .with_labels(labels.clone()),
            MetricSample::new(
                "concurrency_high_watermark",
                "Highest observed in-flight count for the concurrency gate.",
                MetricKind::Gauge,
                self.high_watermark as u64,
            )
            .with_labels(labels.clone()),
            MetricSample::new(
                "concurrency_rejected_total",
                "Number of operations rejected by the concurrency gate.",
                MetricKind::Counter,
                self.rejected,
            )
            .with_labels(labels),
        ]
    }
}

#[derive(Debug)]
struct ConcurrencyState {
    gate: &'static str,
    limit: usize,
    semaphore: Arc<Semaphore>,
    in_flight: AtomicUsize,
    high_watermark: AtomicUsize,
    rejected: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct ConcurrencyGate {
    state: Arc<ConcurrencyState>,
}

impl ConcurrencyGate {
    pub fn new(gate: &'static str, limit: usize) -> Self {
        assert!(limit > 0, "concurrency gate limit must be positive");
        Self {
            state: Arc::new(ConcurrencyState {
                gate,
                limit,
                semaphore: Arc::new(Semaphore::new(limit)),
                in_flight: AtomicUsize::new(0),
                high_watermark: AtomicUsize::new(0),
                rejected: AtomicU64::new(0),
            }),
        }
    }

    pub async fn acquire(&self) -> Result<ConcurrencyPermit, ConcurrencyError> {
        let permit = self
            .state
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ConcurrencyError::Closed {
                gate: self.state.gate,
            })?;
        Ok(ConcurrencyPermit::new(self.state.clone(), permit))
    }

    pub fn try_acquire(&self) -> Result<ConcurrencyPermit, ConcurrencyError> {
        match self.state.semaphore.clone().try_acquire_owned() {
            Ok(permit) => Ok(ConcurrencyPermit::new(self.state.clone(), permit)),
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                self.state.rejected.fetch_add(1, Ordering::Relaxed);
                Err(ConcurrencyError::Saturated {
                    gate: self.state.gate,
                    limit: self.state.limit,
                })
            }
            Err(tokio::sync::TryAcquireError::Closed) => Err(ConcurrencyError::Closed {
                gate: self.state.gate,
            }),
        }
    }

    pub fn snapshot(&self) -> ConcurrencySnapshot {
        ConcurrencySnapshot {
            limit: self.state.limit,
            in_flight: self.state.in_flight.load(Ordering::Relaxed),
            available_permits: self.state.semaphore.available_permits(),
            high_watermark: self.state.high_watermark.load(Ordering::Relaxed),
            rejected: self.state.rejected.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
pub struct ConcurrencyPermit {
    state: Arc<ConcurrencyState>,
    _permit: OwnedSemaphorePermit,
}

impl ConcurrencyPermit {
    fn new(state: Arc<ConcurrencyState>, permit: OwnedSemaphorePermit) -> Self {
        let in_flight = state.in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        let mut observed = state.high_watermark.load(Ordering::Acquire);
        while in_flight > observed {
            match state.high_watermark.compare_exchange_weak(
                observed,
                in_flight,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(next) => observed = next,
            }
        }
        Self {
            state,
            _permit: permit,
        }
    }
}

impl Drop for ConcurrencyPermit {
    fn drop(&mut self) {
        self.state.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::{ConcurrencyError, ConcurrencyGate};

    #[tokio::test]
    async fn tracks_in_flight_and_high_watermark() {
        let gate = ConcurrencyGate::new("test", 2);

        let permit_a = gate.acquire().await.expect("permit a");
        let permit_b = gate.acquire().await.expect("permit b");
        let snapshot = gate.snapshot();

        assert_eq!(snapshot.in_flight, 2);
        assert_eq!(snapshot.high_watermark, 2);
        assert_eq!(snapshot.available_permits, 0);

        drop((permit_a, permit_b));
        assert_eq!(gate.snapshot().in_flight, 0);
    }

    #[test]
    fn rejects_when_saturated() {
        let gate = ConcurrencyGate::new("test", 1);
        let _permit = gate.try_acquire().expect("first permit");

        let error = gate.try_acquire().expect_err("second permit should fail");
        assert_eq!(
            error,
            ConcurrencyError::Saturated {
                gate: "test",
                limit: 1,
            }
        );
        assert_eq!(gate.snapshot().rejected, 1);
    }
}
