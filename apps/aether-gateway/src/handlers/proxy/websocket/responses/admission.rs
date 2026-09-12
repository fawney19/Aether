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
    acquire_provider_pool_execution_guard, ProviderPoolInFlightAdmission, ProviderPoolInFlightGuard,
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
    ) -> Result<Self, GatewayError> {
        let upstream_execution = acquire_upstream_execution_gate(state, trace_id).await?;
        let upstream_target = match state
            .upstream_target_admission
            .acquire(plan, trace_id)
            .await
        {
            Ok(permit) => permit,
            Err(error) => {
                drop(upstream_execution);
                return Err(error);
            }
        };
        let provider_pool = match acquire_provider_pool_execution_guard(state, plan).await? {
            ProviderPoolInFlightAdmission::Acquired(guard) => guard,
            ProviderPoolInFlightAdmission::Saturated { limit } => {
                drop(upstream_target);
                drop(upstream_execution);
                return Err(GatewayError::Client {
                    status: http::StatusCode::TOO_MANY_REQUESTS,
                    message: format!("上游账号并发已达上限 ({limit})"),
                });
            }
        };

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
