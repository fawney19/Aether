use std::sync::Arc;

use crate::concurrency::{ConcurrencyGate, ConcurrencyPermit};
use crate::metrics::{MetricKind, MetricLabel, MetricSample};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DistributedConcurrencyError {
    #[error("distributed concurrency gate {gate} is saturated at {limit}")]
    Saturated { gate: &'static str, limit: usize },
    #[error("distributed concurrency gate {gate} is unavailable: {message}")]
    Unavailable {
        gate: &'static str,
        limit: usize,
        message: String,
    },
    #[error("{0}")]
    InvalidConfiguration(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DistributedConcurrencySnapshot {
    pub limit: usize,
    pub in_flight: usize,
    pub available_permits: usize,
    pub high_watermark: usize,
    pub rejected: u64,
}

impl DistributedConcurrencySnapshot {
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
struct DistributedConcurrencyState {
    gate: &'static str,
    limit: usize,
    gate_impl: Arc<ConcurrencyGate>,
}

#[derive(Debug, Clone)]
pub struct DistributedConcurrencyGate {
    state: Arc<DistributedConcurrencyState>,
}

impl DistributedConcurrencyGate {
    pub fn new_in_memory(gate: &'static str, limit: usize) -> Self {
        assert!(
            limit > 0,
            "distributed concurrency gate limit must be positive"
        );
        Self {
            state: Arc::new(DistributedConcurrencyState {
                gate,
                limit,
                gate_impl: Arc::new(ConcurrencyGate::new(gate, limit)),
            }),
        }
    }

    pub fn gate(&self) -> &'static str {
        self.state.gate
    }

    pub fn limit(&self) -> usize {
        self.state.limit
    }

    pub async fn try_acquire(
        &self,
    ) -> Result<DistributedConcurrencyPermit, DistributedConcurrencyError> {
        self.state
            .gate_impl
            .try_acquire()
            .map(|permit| DistributedConcurrencyPermit { _permit: permit })
            .map_err(|err| match err {
                crate::ConcurrencyError::Saturated { gate, limit } => {
                    DistributedConcurrencyError::Saturated { gate, limit }
                }
                crate::ConcurrencyError::Closed { gate } => {
                    DistributedConcurrencyError::Unavailable {
                        gate,
                        limit: self.state.limit,
                        message: "in-memory distributed concurrency gate is closed".to_string(),
                    }
                }
            })
    }

    pub async fn snapshot(
        &self,
    ) -> Result<DistributedConcurrencySnapshot, DistributedConcurrencyError> {
        let snapshot = self.state.gate_impl.snapshot();
        Ok(DistributedConcurrencySnapshot {
            limit: snapshot.limit,
            in_flight: snapshot.in_flight,
            available_permits: snapshot.available_permits,
            high_watermark: snapshot.high_watermark,
            rejected: snapshot.rejected,
        })
    }
}

#[derive(Debug)]
pub struct DistributedConcurrencyPermit {
    _permit: ConcurrencyPermit,
}

#[cfg(test)]
mod tests {
    use super::{DistributedConcurrencyError, DistributedConcurrencyGate};

    #[tokio::test]
    async fn shared_in_memory_gate_rejects_second_acquire() {
        let gate = DistributedConcurrencyGate::new_in_memory("shared", 1);
        let permit = gate.try_acquire().await.expect("first permit");

        let error = gate
            .try_acquire()
            .await
            .expect_err("second permit should fail");
        assert_eq!(
            error,
            DistributedConcurrencyError::Saturated {
                gate: "shared",
                limit: 1,
            }
        );

        let snapshot = gate.snapshot().await.expect("snapshot should build");
        assert_eq!(snapshot.in_flight, 1);
        assert_eq!(snapshot.available_permits, 0);
        assert_eq!(snapshot.high_watermark, 1);
        assert_eq!(snapshot.rejected, 1);

        drop(permit);
        let snapshot = gate.snapshot().await.expect("snapshot should build");
        assert_eq!(snapshot.in_flight, 0);
    }
}
