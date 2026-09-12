//! Cancellation-safe ownership handoff for WebSocket planning leases.
//!
//! The relay races every turn against connection and response deadlines. A
//! planner future therefore cannot directly own a distributed pool-key lease:
//! losing the race would drop the future between scheduler selection and turn
//! startup, leaving that key unavailable until the lease TTL elapsed.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;
use tokio::task::JoinHandle;

use super::lifecycle::{begin_responses_websocket_turn, ActiveProviderAttempt};
use crate::ai_serving::{
    maybe_build_responses_websocket_decision, AiExecutionDecision, GatewayAuthApiKeySnapshot,
    ResponsesWebSocketDecision, ResponsesWebSocketPinnedCandidate,
};
use crate::control::GatewayControlDecision;
use crate::orchestration::release_pool_key_lease_from_report_context;
use crate::plan_usage_policy::PlanUsagePolicySnapshot;
use crate::{AppState, GatewayError};

/// Owns a selected pool-key lease until the attempt lifecycle has taken over
/// the decision report context.
pub(super) struct PlannedPoolKeyLeaseGuard {
    state: AppState,
    report_context: Option<Value>,
}

/// Planner output coupled to both its request parts and lease guard.
pub(super) struct OwnedResponsesWebSocketDecision {
    pub(super) planned: ResponsesWebSocketDecision,
    pub(super) planning_parts: http::request::Parts,
    pub(super) planned_lease: PlannedPoolKeyLeaseGuard,
}

/// Runs planning in an owner task. Dropping the caller's waiter detaches this
/// task; an unobserved successful output drops its guard and releases the
/// selected pool-key lease.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_owned_responses_websocket_plan(
    state: AppState,
    parts: http::request::Parts,
    trace_id: String,
    control_decision: GatewayControlDecision,
    auth_snapshot: Option<GatewayAuthApiKeySnapshot>,
    client_event: Value,
    excluded_key_ids: Option<BTreeSet<String>>,
    excluded_codex_account_ids: Option<BTreeSet<String>>,
    pinned_candidate: Option<ResponsesWebSocketPinnedCandidate>,
) -> JoinHandle<Result<Option<OwnedResponsesWebSocketDecision>, GatewayError>> {
    let owner_timeout = state
        .frontdoor_runtime_guards
        .local_execution_planning_timeout;
    tokio::spawn(async move {
        let planned = await_owned_planning_deadline(
            maybe_build_responses_websocket_decision(
                &state,
                &parts,
                &trace_id,
                &control_decision,
                auth_snapshot.as_ref(),
                &client_event,
                excluded_key_ids.as_ref(),
                excluded_codex_account_ids.as_ref(),
                pinned_candidate.as_ref(),
            ),
            owner_timeout,
        )
        .await
        .map_err(|_| GatewayError::LocalExecutionPlanningTimeout {
            trace_id: trace_id.clone(),
            phase: "responses_websocket_plan_owner",
            timeout_ms: owner_timeout.as_millis() as u64,
        })??;

        Ok(planned.map(|planned| {
            let planned_lease =
                PlannedPoolKeyLeaseGuard::new(&state, planned.execution.report_context.as_ref());
            OwnedResponsesWebSocketDecision {
                planned,
                planning_parts: parts,
                planned_lease,
            }
        }))
    })
}

async fn await_owned_planning_deadline<F, T>(
    planning: F,
    deadline: Duration,
) -> Result<T, tokio::time::error::Elapsed>
where
    F: std::future::Future<Output = T>,
{
    tokio::time::timeout(deadline, planning).await
}

pub(super) async fn await_owned_responses_websocket_plan(
    handle: JoinHandle<Result<Option<OwnedResponsesWebSocketDecision>, GatewayError>>,
) -> Result<Option<OwnedResponsesWebSocketDecision>, GatewayError> {
    handle.await.map_err(|error| {
        GatewayError::Internal(format!(
            "Responses WebSocket planning task failed before ownership transfer: {error}"
        ))
    })?
}

impl PlannedPoolKeyLeaseGuard {
    fn new(state: &AppState, report_context: Option<&Value>) -> Self {
        Self {
            state: state.clone(),
            report_context: report_context.cloned(),
        }
    }

    pub(super) async fn release(mut self) {
        release_pool_key_lease_from_report_context(&self.state, self.report_context.as_ref()).await;
        self.report_context = None;
    }

    fn disarm(&mut self) {
        self.report_context = None;
    }
}

impl Drop for PlannedPoolKeyLeaseGuard {
    fn drop(&mut self) {
        let Some(report_context) = self.report_context.take() else {
            return;
        };
        let state = self.state.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                release_pool_key_lease_from_report_context(&state, Some(&report_context)).await;
            });
        }
    }
}

/// Keeps the planning guard in the same detached owner task as lifecycle
/// startup. If the relay loses a deadline race while awaiting startup, the
/// task completes the handoff (or releases the lease on failure) without a
/// cancellation gap.
pub(super) async fn begin_responses_websocket_turn_with_planned_lease(
    state: &AppState,
    trace_id: &str,
    parts: http::request::Parts,
    control_decision: &GatewayControlDecision,
    decision: AiExecutionDecision,
    client_event: &Value,
    plan_usage_policy_snapshot: Option<PlanUsagePolicySnapshot>,
    mut planned_lease: PlannedPoolKeyLeaseGuard,
) -> Result<ActiveProviderAttempt, GatewayError> {
    let state = state.clone();
    let trace_id = trace_id.to_string();
    let control_decision = control_decision.clone();
    let client_event = client_event.clone();
    tokio::spawn(async move {
        let turn = begin_responses_websocket_turn(
            &state,
            &trace_id,
            parts,
            &control_decision,
            decision,
            &client_event,
            plan_usage_policy_snapshot,
        )
        .await?;
        // ActiveProviderAttempt now owns the report context containing the
        // lease. No await occurs between that handoff and disarming the guard.
        planned_lease.disarm();
        Ok(turn)
    })
    .await
    .map_err(|error| {
        GatewayError::Internal(format!(
            "Responses WebSocket guarded turn startup task failed: {error}"
        ))
    })?
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    struct DropProbe(Arc<AtomicUsize>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn dropping_a_planning_waiter_detaches_the_owner_and_drops_its_output() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicUsize::new(0));
        let task_started = Arc::clone(&started);
        let task_release = Arc::clone(&release);
        let task_dropped = Arc::clone(&dropped);
        let owner = tokio::spawn(async move {
            task_started.notify_one();
            task_release.notified().await;
            DropProbe(task_dropped)
        });
        started.notified().await;

        let waiter = tokio::spawn(async move {
            let _ = owner.await;
        });
        waiter.abort();
        let _ = waiter.await;
        release.notify_one();

        tokio::time::timeout(Duration::from_secs(1), async {
            while dropped.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached owner output should be dropped after it finishes");
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn planning_owner_deadline_drops_stalled_work_and_its_guards() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let task_dropped = Arc::clone(&dropped);
        let planning = async move {
            let _probe = DropProbe(task_dropped);
            std::future::pending::<()>().await;
        };

        let result =
            super::await_owned_planning_deadline(planning, Duration::from_millis(20)).await;

        assert!(
            result.is_err(),
            "stalled planning must hit its owner deadline"
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}
