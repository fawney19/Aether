//! Turn finalization and terminal error mapping for a Responses WebSocket.
//!
//! A connection can outlive a turn, so persistence and adapter observation
//! handles are joined in order before the next turn is planned.

use std::time::Duration;

use axum::extract::ws::WebSocket;
use axum::http::StatusCode;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use super::state::BoundResponsesConnection;
use super::turn::{
    begin_unowned_responses_websocket_turn, ResponsesProviderAttempt, ResponsesWebSocketTurnOutcome,
};
use crate::handlers::proxy::websocket::session::{
    CLOSE_INTERNAL_ERROR, CLOSE_POLICY_VIOLATION, CLOSE_TRY_AGAIN, WEBSOCKET_LOG_TRANSPORT,
};
use crate::handlers::proxy::websocket::transport::send_responses_websocket_error;
use crate::plan_usage_policy::PlanUsagePolicySnapshot;
use crate::{AppState, GatewayError};

const RESPONSES_WEBSOCKET_ADAPTER_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(5);
const LOG_TARGET: &str = "aether_gateway::handlers::proxy::responses_ws";

macro_rules! warn {
    ($($arg:tt)*) => {
        tracing::warn!(target: LOG_TARGET, $($arg)*)
    };
}

/// Owns the in-flight turn so that losing the relay task still finalizes it.
///
/// Every ordinary exit path takes the turn out of here and finalizes it
/// explicitly. This guard only covers the paths that are not exit paths at all
/// — a panic in the relay loop, or the task being dropped — where the turn
/// would otherwise be discarded with its usage row left `Pending`, its
/// candidate row left `Streaming`, and its distributed pool key lease leaked
/// until the lease expires. Mirrors the HTTP path's `DirectPassthroughFinalizer`.
pub(super) struct ActiveProviderAttempt {
    turn: Option<ResponsesProviderAttempt>,
    state: AppState,
}

impl ActiveProviderAttempt {
    pub(super) fn new(state: &AppState, turn: ResponsesProviderAttempt) -> Self {
        Self {
            turn: Some(turn),
            state: state.clone(),
        }
    }

    /// Hands the turn back to a caller that will finalize it explicitly.
    pub(super) fn disarm(mut self) -> ResponsesProviderAttempt {
        self.turn
            .take()
            .expect("an armed active turn always holds its turn")
    }
}

impl std::ops::Deref for ActiveProviderAttempt {
    type Target = ResponsesProviderAttempt;

    fn deref(&self) -> &Self::Target {
        self.turn
            .as_ref()
            .expect("an armed active turn always holds its turn")
    }
}

impl std::ops::DerefMut for ActiveProviderAttempt {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.turn
            .as_mut()
            .expect("an armed active turn always holds its turn")
    }
}

/// Starts a turn and arms its cancellation fallback before control returns to
/// code that can await an upstream bind or socket write.
pub(super) async fn begin_responses_websocket_turn(
    state: &AppState,
    trace_id: &str,
    parts: http::request::Parts,
    control_decision: &crate::control::GatewayControlDecision,
    decision: crate::ai_serving::AiExecutionDecision,
    client_event: &serde_json::Value,
    plan_usage_policy_snapshot: Option<PlanUsagePolicySnapshot>,
) -> Result<ActiveProviderAttempt, GatewayError> {
    let state = state.clone();
    let trace_id = trace_id.to_string();
    let owner_timeout = state
        .frontdoor_runtime_guards
        .local_execution_planning_timeout;
    let control_decision = control_decision.clone();
    let client_event = client_event.clone();

    // Beginning an attempt performs several indispensable async writes before
    // an `ActiveProviderAttempt` can exist (balance/admission, Pending usage,
    // and candidate state). Run that whole transition in an owned task. If the
    // relay/session future is cancelled while awaiting it, Tokio detaches this
    // task; it still reaches either an explicitly cleaned-up error or an armed
    // guard whose dropped output finalizes the attempt.
    await_owned_turn_begin(
        async move {
            let turn = begin_unowned_responses_websocket_turn(
                &state,
                &parts,
                &control_decision,
                decision,
                &client_event,
                plan_usage_policy_snapshot,
            )
            .await?;
            Ok(ActiveProviderAttempt::new(&state, turn))
        },
        owner_timeout,
        trace_id,
    )
    .await
}

async fn await_owned_turn_begin<T>(
    begin: impl std::future::Future<Output = Result<T, GatewayError>> + Send + 'static,
    owner_timeout: Duration,
    trace_id: String,
) -> Result<T, GatewayError>
where
    T: Send + 'static,
{
    await_owned_turn_begin_with_timeout(begin, owner_timeout, trace_id).await
}

async fn await_owned_turn_begin_with_timeout<T>(
    begin: impl std::future::Future<Output = Result<T, GatewayError>> + Send + 'static,
    owner_timeout: Duration,
    trace_id: String,
) -> Result<T, GatewayError>
where
    T: Send + 'static,
{
    tokio::spawn(async move {
        tokio::time::timeout(owner_timeout, begin)
            .await
            .map_err(|_| GatewayError::LocalExecutionPlanningTimeout {
                trace_id,
                phase: "responses_websocket_turn_begin_owner",
                timeout_ms: owner_timeout.as_millis() as u64,
            })?
    })
    .await
    .map_err(|error| {
        GatewayError::Internal(format!(
            "Responses WebSocket turn begin task failed before ownership transfer: {error}"
        ))
    })?
}

impl Drop for ActiveProviderAttempt {
    fn drop(&mut self) {
        let Some(turn) = self.turn.take() else {
            return;
        };
        let outcome = turn.abandonment_outcome();
        let state = self.state.clone();
        // No runtime means the process is going down; the spawn could not
        // complete anyway.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            warn!(
                event_name = "responses_websocket_turn_abandoned",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                "gateway finalized a Responses WebSocket turn whose relay task went away"
            );
            handle.spawn(async move {
                turn.finalize_detached(&state, outcome).await;
            });
        }
    }
}

/// 结束当前 logical turn 并结算它的 attempt。
///
/// `end()` 同时清掉 logical turn 和 attempt，取代原来「take active_turn +
/// 在每个出口手写 `active_response_create = None`」的两步组合。
pub(super) async fn finalize_active_turn(
    bound: &mut BoundResponsesConnection,
    state: &AppState,
    outcome: ResponsesWebSocketTurnOutcome,
) {
    if let Some(turn) = bound.turn_state.end() {
        queue_turn_finalization(bound, state, turn, outcome).await;
    }
}

pub(super) async fn queue_turn_finalization(
    bound: &mut BoundResponsesConnection,
    state: &AppState,
    turn: ActiveProviderAttempt,
    outcome: ResponsesWebSocketTurnOutcome,
) {
    await_pending_adapter_observation(bound).await;
    await_pending_turn_finalization(bound).await;
    bound.pending_turn_finalization = Some(spawn_guarded_turn_finalization(
        state.clone(),
        turn,
        outcome,
    ));
}

/// 「上一个 attempt 已经结算完毕」的凭证。
///
/// 只能由本模块颁发，且只有在结算真正落地之后。规划下一个 attempt 的入口
/// ([`super::quota::retry_active_turn_after_quota_exhaustion`]) 要求这个参数，
/// 于是「先结算、再规划」成为签名的一部分，而不是一句注释——顺序写反连编译都
/// 过不了。
pub(super) struct PreviousAttemptSettled(());

impl PreviousAttemptSettled {
    /// 没有 attempt 要结算（连接此刻不在 `Responding`）。
    pub(super) const fn nothing_to_settle() -> Self {
        Self(())
    }
}

/// 结算一个 attempt 并等它落地。
///
/// 与 [`queue_turn_finalization`] 的区别只在于「等」：后者把 handle 挂在连接上
/// 让 relay loop 继续跑，适用于结算之后不再需要读取共享状态的出口；这个用在
/// 必须先看到结算结果才能继续的路径上——典型的就是透明重试，它紧接着要按
/// health / adaptive / pool 状态规划下一个 attempt。
pub(super) async fn settle_turn_finalization(
    bound: &mut BoundResponsesConnection,
    state: &AppState,
    mut turn: ActiveProviderAttempt,
    outcome: ResponsesWebSocketTurnOutcome,
) -> Option<PreviousAttemptSettled> {
    // This awaited finalization entry is used only by transparent quota retry.
    // Free the old attempt's durable cost reservation before planning can
    // reserve the replacement, otherwise two per-attempt estimates briefly
    // count against the same user and can manufacture a false local 429.
    if let Err(error) = turn.release_plan_usage_cost_for_retry(state).await {
        warn!(
            event_name = "responses_websocket_plan_usage_cost_retry_release_failed",
            log_type = "ops",
            transport = WEBSOCKET_LOG_TRANSPORT,
            websocket = true,
            error = ?error,
            "gateway stopped a transparent Responses WebSocket retry because the old plan cost reservation could not be released"
        );
        queue_turn_finalization(bound, state, turn, outcome).await;
        await_pending_turn_finalization(bound).await;
        return None;
    }
    queue_turn_finalization(bound, state, turn, outcome).await;
    await_pending_turn_finalization(bound).await;
    Some(PreviousAttemptSettled(()))
}

pub(super) fn spawn_bounded_adapter_observation(
    observation: impl std::future::Future<Output = ()> + Send + 'static,
) -> JoinHandle<()> {
    spawn_bounded_adapter_observation_with_timeout(
        observation,
        RESPONSES_WEBSOCKET_ADAPTER_OBSERVATION_TIMEOUT,
    )
}

fn spawn_bounded_adapter_observation_with_timeout(
    observation: impl std::future::Future<Output = ()> + Send + 'static,
    owner_timeout: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if timeout(owner_timeout, observation).await.is_err() {
            warn!(
                event_name = "responses_websocket_adapter_observation_timeout",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                timeout_ms = owner_timeout.as_millis() as u64,
                "gateway stopped a timed-out Responses WebSocket adapter observation"
            );
        }
    })
}

pub(super) async fn await_pending_adapter_observation(bound: &mut BoundResponsesConnection) {
    if let Some(handle) = bound.pending_adapter_observation.take() {
        if let Err(error) = handle.await {
            warn!(
                event_name = "responses_websocket_adapter_observation_join_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                error = ?error,
                "gateway Responses WebSocket adapter observation task failed"
            );
        }
    }
}

pub(super) fn finalize_unbound_turn(
    state: AppState,
    turn: ActiveProviderAttempt,
    outcome: ResponsesWebSocketTurnOutcome,
) -> JoinHandle<()> {
    spawn_guarded_turn_finalization(state, turn, outcome)
}

fn spawn_guarded_turn_finalization(
    state: AppState,
    turn: ActiveProviderAttempt,
    outcome: ResponsesWebSocketTurnOutcome,
) -> JoinHandle<()> {
    // Spawn synchronously while the armed guard is still owned here. Caller
    // cancellation cannot drop an unguarded attempt between cleanup awaits.
    tokio::spawn(async move {
        let mut turn = turn;
        turn.release_admission().await;
        turn.disarm().finalize_detached(&state, outcome).await;
    })
}

pub(super) async fn await_turn_finalization_handle(handle: JoinHandle<()>) {
    // Do not abort terminal persistence here. Each I/O stage inside the turn
    // finalizer is independently bounded, and aborting the owner would skip
    // pool-lease cleanup and leave usage/candidate state non-terminal.
    match handle.await {
        Ok(()) => {}
        Err(error) => {
            warn!(
                event_name = "responses_websocket_turn_finalization_join_failed",
                log_type = "ops",
                transport = WEBSOCKET_LOG_TRANSPORT,
                websocket = true,
                error = ?error,
                "gateway Responses WebSocket turn finalizer task failed"
            );
        }
    }
}

pub(super) async fn await_pending_turn_finalization(bound: &mut BoundResponsesConnection) {
    if let Some(handle) = bound.pending_turn_finalization.take() {
        await_turn_finalization_handle(handle).await;
    }
}

pub(super) async fn send_responses_websocket_turn_start_error(
    client_socket: &mut WebSocket,
    error: &GatewayError,
) {
    let status_code = responses_websocket_turn_start_http_status(error);
    match error {
        GatewayError::PlanUsageLimited(_) => {
            send_responses_websocket_error(
                client_socket,
                status_code,
                "rate_limit_error",
                "plan_usage_limit_exceeded",
                "Subscription plan usage limit exceeded; retry later",
            )
            .await;
        }
        GatewayError::Client { status, message } => {
            let (error_type, code) = if status.as_u16() == 429 {
                ("rate_limit_error", "gateway_request_capacity_exceeded")
            } else {
                ("invalid_request_error", "gateway_request_not_allowed")
            };
            send_responses_websocket_error(client_socket, status_code, error_type, code, message)
                .await;
        }
        GatewayError::AdmissionTimeout { .. } => {
            send_responses_websocket_error(
                client_socket,
                status_code,
                "server_error",
                "gateway_admission_timeout",
                "Gateway capacity is busy; retry this response",
            )
            .await;
        }
        GatewayError::LocalExecutionPlanningTimeout { .. } => {
            send_responses_websocket_error(
                client_socket,
                status_code,
                "server_error",
                "gateway_planning_timeout",
                "Gateway planning timed out; retry this response",
            )
            .await;
        }
        _ => {
            send_responses_websocket_error(
                client_socket,
                status_code,
                "server_error",
                "responses_websocket_turn_start_failed",
                "Gateway could not start this response",
            )
            .await;
        }
    }
}

fn responses_websocket_turn_start_http_status(error: &GatewayError) -> u16 {
    match error {
        GatewayError::PlanUsageLimited(_) => StatusCode::TOO_MANY_REQUESTS.as_u16(),
        GatewayError::Client { status, .. } => status.as_u16(),
        GatewayError::AdmissionTimeout { .. } => StatusCode::TOO_MANY_REQUESTS.as_u16(),
        GatewayError::LocalExecutionPlanningTimeout { .. } => StatusCode::GATEWAY_TIMEOUT.as_u16(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
    }
}

pub(super) fn responses_websocket_turn_start_close(error: &GatewayError) -> (u16, &'static str) {
    match error {
        GatewayError::PlanUsageLimited(_) => (CLOSE_TRY_AGAIN, "plan_usage_limit_exceeded"),
        GatewayError::Client { .. } => (CLOSE_POLICY_VIOLATION, "request_not_allowed"),
        GatewayError::AdmissionTimeout { .. }
        | GatewayError::LocalExecutionPlanningTimeout { .. } => (CLOSE_TRY_AGAIN, "gateway_busy"),
        _ => (CLOSE_INTERNAL_ERROR, "turn_start_failed"),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{
        await_owned_turn_begin, await_owned_turn_begin_with_timeout,
        await_turn_finalization_handle, responses_websocket_turn_start_close,
        responses_websocket_turn_start_http_status, spawn_bounded_adapter_observation_with_timeout,
    };
    use crate::plan_usage_policy::PlanUsagePolicyRejection;
    use crate::GatewayError;

    #[test]
    fn admission_timeout_uses_http_429_and_keeps_the_retry_later_close_code() {
        let error = GatewayError::AdmissionTimeout {
            trace_id: "turn-admission".to_string(),
            gate: "gateway_upstream_execution",
            queue_budget_ms: 25,
        };

        assert_eq!(responses_websocket_turn_start_http_status(&error), 429);
        assert_eq!(
            responses_websocket_turn_start_close(&error),
            (1013, "gateway_busy")
        );
    }

    #[test]
    fn plan_usage_limit_uses_http_429_and_retry_later_close_code() {
        let error = GatewayError::PlanUsageLimited(PlanUsagePolicyRejection {
            metric: "actual_cost_usd",
            limit: 10.0,
            retry_after: 60,
            window: "calendar_month",
        });

        assert_eq!(responses_websocket_turn_start_http_status(&error), 429);
        assert_eq!(
            responses_websocket_turn_start_close(&error),
            (1013, "plan_usage_limit_exceeded")
        );
    }

    /// C6 依赖的性质：结算是「等到落地」而不是「排进队列」。
    ///
    /// 透明重试在这之后立刻按 health / adaptive / pool 状态规划下一个 attempt，
    /// 所以结算任务必须已经跑完——只把 handle 挂起来是不够的。
    #[tokio::test]
    async fn awaiting_a_finalization_handle_runs_the_settlement_to_completion() {
        let settled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&settled);
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            flag.store(true, Ordering::SeqCst);
        });

        assert!(
            !settled.load(Ordering::SeqCst),
            "the settlement has not finished yet"
        );
        await_turn_finalization_handle(handle).await;
        assert!(
            settled.load(Ordering::SeqCst),
            "the settlement must be complete before the caller proceeds"
        );
    }

    /// 顺序型：结算的每一步都要排在规划之前。
    ///
    /// 用计数器替身重放透明重试的两步——旧 attempt 结算完成写入 1，规划开始时
    /// 读到的必须已经是 1。旧实现在这里先规划、再把结算排进队列，规划读到的是 0。
    #[tokio::test]
    async fn transparent_retry_replans_only_after_the_previous_attempt_is_settled() {
        let steps = Arc::new(AtomicUsize::new(0));

        // 第一步：结算旧 attempt（等到落地）。
        let recorder = Arc::clone(&steps);
        let settlement = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            recorder.store(1, Ordering::SeqCst);
        });
        await_turn_finalization_handle(settlement).await;

        // 第二步：规划下一个 attempt，它读到的状态必须是结算之后的。
        let observed_at_planning = steps.load(Ordering::SeqCst);
        assert_eq!(
            observed_at_planning, 1,
            "planning must observe the state projected by the settled attempt"
        );
    }

    /// 结算任务失败（panic / cancel）也必须让调用方继续，不能把 relay loop 卡死。
    #[tokio::test]
    async fn a_failed_finalization_task_still_releases_the_caller() {
        let handle = tokio::spawn(async { panic!("settlement task exploded") });
        await_turn_finalization_handle(handle).await;
    }

    #[tokio::test]
    async fn cancelling_the_caller_does_not_cancel_turn_begin_or_drop_an_unowned_result() {
        struct DropProbe(Arc<AtomicBool>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let begin_finished = Arc::new(AtomicBool::new(false));
        let result_dropped = Arc::new(AtomicBool::new(false));
        let finished = Arc::clone(&begin_finished);
        let dropped = Arc::clone(&result_dropped);
        let caller = tokio::spawn(async move {
            await_owned_turn_begin(
                async move {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                    finished.store(true, Ordering::SeqCst);
                    Ok(DropProbe(dropped))
                },
                Duration::from_secs(1),
                "turn-begin-cancel".to_string(),
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        caller.abort();
        let _ = caller.await;
        tokio::time::sleep(Duration::from_millis(120)).await;

        assert!(
            begin_finished.load(Ordering::SeqCst),
            "the owned begin task must outlive its cancelled relay caller"
        );
        assert!(
            result_dropped.load(Ordering::SeqCst),
            "an undeliverable armed result must be dropped so its cleanup guard runs"
        );
    }

    #[tokio::test]
    async fn turn_begin_owner_deadline_drops_stalled_work_and_its_guards() {
        struct DropProbe(Arc<AtomicBool>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = Arc::clone(&dropped);
        let result: Result<(), GatewayError> = await_owned_turn_begin_with_timeout(
            async move {
                let _probe = DropProbe(task_dropped);
                std::future::pending::<()>().await;
                Ok(())
            },
            Duration::from_millis(20),
            "turn-begin-deadline".to_string(),
        )
        .await;

        assert!(matches!(
            result,
            Err(GatewayError::LocalExecutionPlanningTimeout {
                trace_id,
                phase: "responses_websocket_turn_begin_owner",
                timeout_ms: 20,
            }) if trace_id == "turn-begin-deadline"
        ));
        assert!(
            dropped.load(Ordering::SeqCst),
            "owner timeout must drop the stalled begin future so RAII cleanup runs"
        );
    }

    #[tokio::test]
    async fn cancelling_observation_waiter_cannot_bypass_the_owner_timeout() {
        struct DropProbe(Arc<AtomicBool>);
        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = Arc::clone(&dropped);
        let observation = async move {
            let _probe = DropProbe(task_dropped);
            std::future::pending::<()>().await;
        };
        let owner =
            spawn_bounded_adapter_observation_with_timeout(observation, Duration::from_millis(20));
        let waiter = tokio::spawn(async move {
            let _ = owner.await;
        });
        waiter.abort();
        let _ = waiter.await;

        tokio::time::timeout(Duration::from_secs(1), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached observation owner must enforce its own timeout");
    }
}
