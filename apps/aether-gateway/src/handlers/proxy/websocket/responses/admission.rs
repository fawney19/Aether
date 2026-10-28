//! Per-turn resource admission for the Responses WebSocket bridge.
//!
//! A WebSocket connection may live for a long time, but each `response.create`
//! is still one active upstream execution.  Keep the resource leases attached
//! to the turn instead of the socket so idle connections do not consume
//! upstream capacity.

use std::time::Instant;

use aether_contracts::ExecutionPlan;

use crate::execution_runtime::acquire_upstream_execution_gate;
use crate::provider_pool_demand::{
    acquire_provider_pool_execution_guard, acquire_provider_pool_execution_guard_unobserved,
    ProviderPoolInFlightAdmission, ProviderPoolInFlightGuard,
};
use crate::upstream_admission::UpstreamTargetAdmissionPermit;
use crate::{AppState, GatewayError};

pub(crate) struct ResponsesWebSocketTurnAdmission {
    upstream_execution: Option<aether_runtime::ConcurrencyPermit>,
    upstream_target: Option<UpstreamTargetAdmissionPermit>,
    provider_pool: Option<ProviderPoolInFlightGuard>,
    acquired_at: Instant,
}

impl ResponsesWebSocketTurnAdmission {
    pub(crate) async fn acquire(
        state: &AppState,
        plan: &ExecutionPlan,
        trace_id: &str,
        observation_context: Option<&serde_json::Value>,
    ) -> Result<Self, GatewayError> {
        let provider_admission = match observation_context {
            Some(context) => {
                acquire_provider_pool_execution_guard(state, plan, Some(context)).await?
            }
            None => acquire_provider_pool_execution_guard_unobserved(state, plan).await?,
        };
        let provider_pool = match provider_admission {
            ProviderPoolInFlightAdmission::Acquired(guard) => guard,
            ProviderPoolInFlightAdmission::Rejected {
                status, message, ..
            } => {
                return Err(GatewayError::Client {
                    status: http::StatusCode::from_u16(status)
                        .unwrap_or(http::StatusCode::SERVICE_UNAVAILABLE),
                    message,
                });
            }
        };
        let upstream_execution = acquire_upstream_execution_gate(state, trace_id).await?;
        let upstream_target = state
            .upstream_target_admission
            .acquire(plan, trace_id)
            .await?;

        Ok(Self {
            upstream_execution,
            upstream_target,
            provider_pool,
            acquired_at: Instant::now(),
        })
    }

    /// Release the distributed provider token before the turn's persistence
    /// work. The remaining permits are local RAII guards and are dropped with
    /// this value.
    pub(crate) async fn release(mut self) {
        if let Some(provider_pool) = self.provider_pool.take() {
            provider_pool.release().await;
        }
        drop(self.upstream_target.take());
        drop(self.upstream_execution.take());
    }
}

impl Drop for ResponsesWebSocketTurnAdmission {
    fn drop(&mut self) {
        crate::stage_metrics::observe_gateway_stage_ms(
            "websocket_turn_admission_held",
            self.acquired_at.elapsed().as_millis() as u64,
        );
    }
}
