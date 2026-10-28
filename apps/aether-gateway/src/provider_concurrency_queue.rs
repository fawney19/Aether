//! Bounded, cancellation-safe FIFO admission per upstream credential.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use aether_runtime_state::{RuntimeSemaphore, RuntimeSemaphoreError, RuntimeSemaphorePermit};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use tokio::time::{sleep, timeout, Instant};

mod observation;
pub(crate) use observation::queued_request_ids;
pub(crate) use observation::QueueCancellationGuard;

pub(crate) fn is_concurrency_skip_reason(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some("provider_concurrency_limit_reached" | "provider_key_concurrency_limit_reached")
    )
}

#[derive(Debug)]
pub(crate) struct ProviderConcurrencyQueue {
    timeout: Duration,
    capacity: usize,
    lanes: Mutex<HashMap<String, Weak<Lane>>>,
}

#[derive(Debug)]
struct Lane {
    turn: AsyncMutex<()>,
    waiting: Semaphore,
}

impl ProviderConcurrencyQueue {
    pub(crate) fn from_env() -> Self {
        let timeout_ms = std::env::var("AETHER_GATEWAY_PROVIDER_CONCURRENCY_QUEUE_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(15_000)
            .min(120_000);
        let capacity = std::env::var("AETHER_GATEWAY_PROVIDER_CONCURRENCY_QUEUE_CAPACITY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(100)
            .min(10_000);
        Self::new(Duration::from_millis(timeout_ms), capacity)
    }

    pub(crate) fn new(timeout: Duration, capacity: usize) -> Self {
        Self {
            timeout,
            capacity,
            lanes: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        !self.timeout.is_zero() && self.capacity > 0
    }

    pub(crate) fn is_waiting(&self, resource: &str) -> bool {
        self.lanes
            .lock()
            .ok()
            .and_then(|lanes| lanes.get(resource).and_then(Weak::upgrade))
            .is_some_and(|lane| lane.waiting.available_permits() < self.capacity)
    }

    /// FIFO is local to a credential; the permits enforce shared runtime limits.
    /// Dropping this future removes its place and releases partial admissions.
    #[cfg(test)]
    pub(crate) async fn acquire(
        &self,
        gates: &[RuntimeSemaphore],
        resource: &str,
    ) -> Result<Vec<RuntimeSemaphorePermit>, RuntimeSemaphoreError> {
        self.acquire_observed(gates, resource, true, None).await
    }

    pub(crate) async fn acquire_observed(
        &self,
        gates: &[RuntimeSemaphore],
        resource: &str,
        queue_enabled: bool,
        request: Option<(
            &crate::AppState,
            &aether_contracts::ExecutionPlan,
            Option<&serde_json::Value>,
        )>,
    ) -> Result<Vec<RuntimeSemaphorePermit>, RuntimeSemaphoreError> {
        let Some(first_gate) = gates.first() else {
            return Ok(Vec::new());
        };
        if !queue_enabled || !self.enabled() {
            return try_acquire_all(gates).await;
        }
        let lane = {
            let mut lanes =
                self.lanes
                    .lock()
                    .map_err(|error| RuntimeSemaphoreError::Unavailable {
                        gate: first_gate.gate(),
                        limit: first_gate.limit(),
                        message: error.to_string(),
                    })?;
            lanes.retain(|_, lane| lane.strong_count() > 0);
            match lanes.get(resource).and_then(Weak::upgrade) {
                Some(lane) => lane,
                None => {
                    let lane = Arc::new(Lane {
                        turn: AsyncMutex::new(()),
                        waiting: Semaphore::new(self.capacity),
                    });
                    lanes.insert(resource.to_string(), Arc::downgrade(&lane));
                    lane
                }
            }
        };
        let saturated = || RuntimeSemaphoreError::Saturated {
            gate: first_gate.gate(),
            limit: first_gate.limit(),
        };
        // Register every arrival before it can touch runtime capacity. New
        // requests cannot bypass an existing waiter even when a slot opens.
        let _place = lane.waiting.try_acquire().map_err(|_| saturated())?;
        let started = Instant::now();
        let mut last_saturated = saturated();
        let mut observation = None;
        let result = timeout(self.timeout, async {
            let _turn = match lane.turn.try_lock() {
                Ok(turn) => turn,
                Err(_) => {
                    if let Some((state, plan, context)) = request {
                        observation = Some(observation::QueueObservation::begin(
                            state,
                            plan,
                            context,
                            self.timeout,
                        ));
                    }
                    lane.turn.lock().await
                }
            };
            loop {
                match try_acquire_all(gates).await {
                    Ok(permits) => return Ok(permits),
                    Err(error @ RuntimeSemaphoreError::Saturated { .. }) => {
                        last_saturated = error;
                        if observation.is_none() {
                            if let Some((state, plan, context)) = request {
                                observation = Some(observation::QueueObservation::begin(
                                    state,
                                    plan,
                                    context,
                                    self.timeout,
                                ));
                            }
                        }
                        sleep(Duration::from_millis(50)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
        })
        .await
        .unwrap_or_else(|_| Err(last_saturated));
        if let Some(observation) = observation {
            observation.finish().await;
        }
        crate::stage_metrics::observe_gateway_stage_ms(
            "provider_concurrency_queue_wait",
            u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        );
        result
    }
}

async fn try_acquire_all(
    gates: &[RuntimeSemaphore],
) -> Result<Vec<RuntimeSemaphorePermit>, RuntimeSemaphoreError> {
    let mut permits = Vec::with_capacity(gates.len());
    for gate in gates {
        match gate.try_acquire().await {
            Ok(permit) => permits.push(permit),
            Err(error) => {
                for permit in permits {
                    permit.release().await?;
                }
                return Err(error);
            }
        }
    }
    Ok(permits)
}

#[cfg(test)]
mod tests;
