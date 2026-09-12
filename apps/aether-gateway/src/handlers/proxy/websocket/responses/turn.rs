//! Per-turn lifecycle accounting for the standard Responses WebSocket bridge.
//!
//! Every `response.create` remains a separate billable and auditable request,
//! including turns that cause the bridge to re-plan a changed model. This
//! module turns the connection-local JSON events back into the existing
//! Responses stream report surface without exposing the socket protocol to the
//! normal HTTP/SSE execution runtime.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_contracts::{
    ExecutionPlan, ExecutionStreamTerminalSummary, ExecutionTelemetry, ExecutionTimeouts,
    MAX_EXECUTION_REQUEST_TIMEOUT_MS, MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_MS,
};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_data_contracts::repository::usage::{
    UsageBodyCaptureState, PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY,
    WEBSOCKET_MODE_METADATA_KEY, WEBSOCKET_TRANSPORT_METADATA_KEY,
};
use aether_scheduler_core::SchedulerRequestCandidateStatusUpdate;
use aether_usage_runtime::{
    build_lifecycle_usage_seed, build_stream_terminal_usage_payload_seed,
    build_terminal_usage_context_seed, stream_report_represents_failure,
    DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
};
use axum::http::StatusCode;
use base64::Engine as _;
use serde_json::{json, Map, Value};
use tracing::warn;

use super::adapter::ResponsesWebSocketProtocolAdapter;
use super::admission::ResponsesWebSocketTurnAdmission;
use super::frame::ParsedResponsesWebSocketFrame;
use super::observation::ResponsesStructuredTerminalObserver;
use super::settlement::attempt_facts_for_outcome;
use crate::ai_serving::{build_openai_responses_stream_plan_from_decision, AiExecutionDecision};
use crate::clock::current_unix_ms;
use crate::control::{
    execution_plan_balance_capacity_rejection, GatewayControlDecision, GatewayLocalAuthRejection,
};
use crate::execution_runtime::attach_provider_response_headers_to_report_context;
use crate::execution_runtime::attempt_lifecycle::{
    attempt_billing_is_void, AttemptBodyCapture, AttemptClientDelivery, AttemptLifecycleSeed,
    AttemptProviderOutcome, AttemptStageGuard, AttemptTerminalFacts, AttemptTerminalFactsInput,
    ExecutionAttemptLifecycle,
};
use crate::orchestration::{
    apply_local_stream_failure_effects, apply_local_stream_success_effects,
    release_local_pool_key_lease, release_pool_key_lease_from_report_context,
    LocalExecutionEffectContext, LocalStreamFailureEffect,
};
use crate::plan_usage_policy::PlanUsagePolicySnapshot;
use crate::request_candidate_runtime::{
    ensure_execution_request_candidate_slot, record_local_request_candidate_status,
};
use crate::usage::{submit_stream_report, GatewayStreamReportRequest};
use crate::{AppState, GatewayError};

const WEBSOCKET_CONNECTION_TRACE_REPORT_CONTEXT_FIELD: &str = "websocket_connection_trace_id";
const WEBSOCKET_TURN_INDEX_REPORT_CONTEXT_FIELD: &str = "websocket_turn_index";
const WEBSOCKET_LOGICAL_TURN_ID_REPORT_CONTEXT_FIELD: &str = "websocket_logical_turn_id";
const WEBSOCKET_TURN_ATTEMPT_REPORT_CONTEXT_FIELD: &str = "websocket_turn_attempt";
const WEBSOCKET_CLIENT_DELIVERY_REPORT_CONTEXT_FIELD: &str = "websocket_client_delivery";
const WEBSOCKET_CLIENT_DELIVERY_ABORTED: &str = "aborted";
const WEBSOCKET_CLIENT_DELIVERY_REASON_REPORT_CONTEXT_FIELD: &str =
    "websocket_client_delivery_reason";
/// 首个可计费事件到达时记在 usage/candidate 上的状态码。WS 的首事件本身不带
/// HTTP 状态，沿用 HTTP 流式「已开始流」的 200。
const STREAM_STARTED_STATUS_CODE: u16 = 200;
const DEFAULT_WEBSOCKET_FIRST_EVENT_TIMEOUT_MS: u64 = 30_000;
const RESPONSES_WEBSOCKET_LIFECYCLE_STAGE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponsesWebSocketTurnObservation {
    Started,
    Terminal(ResponsesWebSocketTurnOutcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponsesWebSocketTurnTimeoutPhase {
    AwaitingFirstEvent,
    AwaitingTerminal,
}

impl ResponsesWebSocketTurnTimeoutPhase {
    pub(super) const fn error_code(self) -> &'static str {
        match self {
            Self::AwaitingFirstEvent => "responses_websocket_first_event_timeout",
            Self::AwaitingTerminal => "responses_websocket_turn_timeout",
        }
    }

    pub(super) const fn client_message(self) -> &'static str {
        match self {
            Self::AwaitingFirstEvent => {
                "Provider did not emit a response event before the configured timeout"
            }
            Self::AwaitingTerminal => {
                "Provider did not finish the response before the configured timeout"
            }
        }
    }

    pub(super) const fn outcome(self) -> ResponsesWebSocketTurnOutcome {
        match self {
            Self::AwaitingFirstEvent => ResponsesWebSocketTurnOutcome::first_event_timeout(),
            Self::AwaitingTerminal => ResponsesWebSocketTurnOutcome::terminal_timeout(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ResponsesWebSocketTurnDeadline {
    pub(super) phase: ResponsesWebSocketTurnTimeoutPhase,
    pub(super) deadline: Instant,
    pub(super) timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponsesWebSocketTurnOutcome {
    ProviderTerminal {
        status_code: u16,
        cancelled: bool,
    },
    Cancelled {
        reason: &'static str,
    },
    Failure {
        status_code: u16,
        reason: &'static str,
    },
}

impl ResponsesWebSocketTurnOutcome {
    pub(super) const fn client_disconnected() -> Self {
        Self::Cancelled {
            reason: "client disconnected before provider terminal event",
        }
    }

    pub(super) const fn connection_limit_reached() -> Self {
        Self::Cancelled {
            reason: "gateway WebSocket connection duration limit reached",
        }
    }

    pub(super) const fn connection_admission_lost() -> Self {
        // This gateway-owned cancellation is also used when a logical turn's
        // subscription-plan permit becomes unhealthy. Keeping it Cancelled
        // prevents the settlement layer from projecting a provider failure;
        // the client-visible 503 is emitted separately by plan_admission.
        Self::Cancelled {
            reason: "gateway WebSocket connection admission became unhealthy",
        }
    }

    pub(super) const fn upstream_closed() -> Self {
        Self::Failure {
            status_code: 502,
            reason: "upstream WebSocket closed before provider terminal event",
        }
    }

    pub(super) const fn upstream_receive_failed() -> Self {
        Self::Failure {
            status_code: 502,
            reason: "upstream WebSocket receive failed before provider terminal event",
        }
    }

    pub(super) const fn upstream_send_failed() -> Self {
        Self::Failure {
            status_code: 502,
            reason: "gateway could not forward response.create to the upstream",
        }
    }

    pub(super) const fn upstream_connect_failed(reason: &'static str) -> Self {
        Self::Failure {
            status_code: 502,
            reason,
        }
    }

    pub(super) const fn provider_quota_exhausted() -> Self {
        Self::Failure {
            status_code: 429,
            reason: "provider reported exhausted quota before closing the WebSocket",
        }
    }

    pub(super) const fn first_event_timeout() -> Self {
        Self::Failure {
            status_code: 504,
            reason: "upstream WebSocket did not emit a response event before timeout",
        }
    }

    pub(super) const fn terminal_timeout() -> Self {
        Self::Failure {
            status_code: 504,
            reason: "upstream WebSocket did not finish the response before timeout",
        }
    }

    pub(super) const fn relay_task_abandoned() -> Self {
        Self::Failure {
            status_code: 500,
            reason: "gateway relay task went away before the response finished",
        }
    }

    pub(super) const fn relay_task_abandoned_before_upstream_send() -> Self {
        Self::Cancelled {
            reason: "gateway abandoned the turn before sending response.create upstream",
        }
    }

    pub(super) const fn relay_task_abandonment(upstream_request_sent: bool) -> Self {
        if upstream_request_sent {
            Self::relay_task_abandoned()
        } else {
            Self::relay_task_abandoned_before_upstream_send()
        }
    }

    const fn status_code(self) -> u16 {
        match self {
            Self::ProviderTerminal { status_code, .. } | Self::Failure { status_code, .. } => {
                status_code
            }
            Self::Cancelled { .. } => 499,
        }
    }
}

/// 一次 provider attempt：一条上游执行，也是一条独立的 usage/candidate 记录。
///
/// 与 [`super::turn_state::LogicalTurn`] 分工明确：logical turn 是客户端看到的
/// 一轮请求（可能包含多个 attempt），attempt 只负责这一次上游执行的记账事实。
pub(super) struct ResponsesProviderAttempt {
    /// 记账三段（pending / started / terminal）由共享的 transport 中立生命周期负责。
    lifecycle: ExecutionAttemptLifecycle,
    started_at: Instant,
    provider_request_started_at_unix_ms: u64,
    provider_request_order_id: String,
    provider_headers: BTreeMap<String, String>,
    observer: ResponsesStructuredTerminalObserver,
    provider_capture: AttemptBodyCapture,
    client_capture: AttemptBodyCapture,
    upstream_bytes: u64,
    first_event_elapsed_ms: Option<u64>,
    first_event_timeout: Duration,
    terminal_timeout: Duration,
    admission: Option<ResponsesWebSocketTurnAdmission>,
    /// A provider attempt gets its own server-issued cost reservation. The
    /// trusted token also lives in the lifecycle report context so the normal
    /// terminal usage path can reconcile actual cost. We retain the control
    /// snapshot here only for the quota-retry path, which must release the old
    /// attempt synchronously before reserving the replacement.
    plan_usage_cost_reservation: Option<ResponsesPlanUsageCostReservation>,
    terminal_error_body: Option<String>,
    /// 观察到的 provider 终态事实，与「为什么现在结算」这个信号分开保存。
    /// 客户端投递失败不会把它擦掉。
    provider_outcome: Option<AttemptProviderOutcome>,
    /// 这一个 attempt 的内容是否完整交付给了客户端。与 provider 终态正交。
    client_delivery: AttemptClientDelivery,
    /// Tracks the irreversible transport handoff separately from flush
    /// confirmation. Once start_send succeeds, a later cancellation cannot
    /// prove the provider did not receive or execute response.create.
    upstream_request_state: UpstreamRequestState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpstreamRequestState {
    NotStarted,
    PossiblySent,
    Sent,
}

impl UpstreamRequestState {
    const fn may_have_reached_provider(self) -> bool {
        !matches!(self, Self::NotStarted)
    }
}

struct ResponsesPlanUsageCostReservation {
    control_decision: GatewayControlDecision,
    reservation_token: String,
}

struct OwnedResponsesPlanUsageCostReservation {
    control_decision: GatewayControlDecision,
    plan: ExecutionPlan,
    reservation_token: String,
}

/// Owns a successful durable reservation until an attempt lifecycle owns the
/// trusted report context. If startup is cancelled anywhere in between, Drop
/// releases the reservation instead of leaving a false 429 until its TTL.
struct ResponsesPlanUsageCostReservationGuard {
    state: AppState,
    reservation: Option<OwnedResponsesPlanUsageCostReservation>,
}

impl ResponsesPlanUsageCostReservationGuard {
    fn new(
        state: &AppState,
        control_decision: GatewayControlDecision,
        plan: ExecutionPlan,
        reservation_token: String,
    ) -> Self {
        Self {
            state: state.clone(),
            reservation: Some(OwnedResponsesPlanUsageCostReservation {
                control_decision,
                plan,
                reservation_token,
            }),
        }
    }

    fn reservation_token(&self) -> &str {
        self.reservation
            .as_ref()
            .expect("an armed plan cost reservation guard owns its reservation")
            .reservation_token
            .as_str()
    }

    fn disarm(mut self) -> ResponsesPlanUsageCostReservation {
        let reservation = self
            .reservation
            .take()
            .expect("an armed plan cost reservation guard owns its reservation");
        ResponsesPlanUsageCostReservation {
            control_decision: reservation.control_decision,
            reservation_token: reservation.reservation_token,
        }
    }
}

impl Drop for ResponsesPlanUsageCostReservationGuard {
    fn drop(&mut self) {
        let Some(reservation) = self.reservation.take() else {
            return;
        };
        let state = self.state.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                release_responses_plan_usage_cost_best_effort(
                    &state,
                    &reservation.control_decision,
                    &reservation.plan,
                    reservation.reservation_token.as_str(),
                    "turn_start_cancelled",
                )
                .await;
            });
        }
    }
}

enum ResponsesPlanUsageCostReservationStart {
    NotRequired,
    Reserved(ResponsesPlanUsageCostReservationGuard),
    Rejected(crate::plan_usage_policy::PlanUsagePolicyRejection),
}

async fn reserve_responses_plan_usage_cost_owned(
    state: &AppState,
    control_decision: &GatewayControlDecision,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    plan_usage_policy_snapshot: Option<&PlanUsagePolicySnapshot>,
    reservation_token: String,
) -> Result<ResponsesPlanUsageCostReservationStart, GatewayError> {
    let outcome = crate::plan_usage_policy::reserve_admitted_plan_usage_policy_cost(
        state,
        control_decision,
        plan,
        report_context,
        plan_usage_policy_snapshot,
        reservation_token.as_str(),
    )
    .await?;
    match outcome {
        crate::plan_usage_policy::PlanUsageCostReservationOutcome::NotRequired => {
            Ok(ResponsesPlanUsageCostReservationStart::NotRequired)
        }
        crate::plan_usage_policy::PlanUsageCostReservationOutcome::Reserved => {
            Ok(ResponsesPlanUsageCostReservationStart::Reserved(
                ResponsesPlanUsageCostReservationGuard::new(
                    state,
                    control_decision.clone(),
                    plan.clone(),
                    reservation_token,
                ),
            ))
        }
        crate::plan_usage_policy::PlanUsageCostReservationOutcome::Rejected(rejection) => {
            Ok(ResponsesPlanUsageCostReservationStart::Rejected(rejection))
        }
    }
}

fn attach_plan_usage_reservation_token(
    report_context: Option<Value>,
    reservation_token: &str,
) -> Option<Value> {
    let mut object = match report_context {
        Some(Value::Object(object)) => object,
        Some(other) => Map::from_iter([("seed".to_string(), other)]),
        None => Map::new(),
    };
    // Always overwrite a client-derived/seed value with the server-issued
    // token. Terminal reconciliation must never trust caller-controlled JSON.
    object.insert(
        "plan_usage_reservation_token".to_string(),
        Value::String(reservation_token.to_string()),
    );
    // The token and its reconciliation controls are server-owned. A seed or
    // client event must never be able to defer reconciliation on its own.
    object.insert(
        PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY.to_string(),
        Value::Bool(false),
    );
    Some(Value::Object(object))
}

fn websocket_plan_usage_rejection_error(
    rejection: &crate::plan_usage_policy::PlanUsagePolicyRejection,
) -> GatewayError {
    GatewayError::PlanUsageLimited(rejection.clone())
}

async fn release_responses_plan_usage_cost(
    state: &AppState,
    control_decision: &GatewayControlDecision,
    plan: &ExecutionPlan,
    reservation_token: &str,
    _reason: &'static str,
) -> Result<(), GatewayError> {
    crate::plan_usage_policy::release_plan_usage_policy_cost(
        state,
        control_decision,
        plan,
        reservation_token,
        current_unix_ms() / 1_000,
    )
    .await
}

async fn release_responses_plan_usage_cost_best_effort(
    state: &AppState,
    control_decision: &GatewayControlDecision,
    plan: &ExecutionPlan,
    reservation_token: &str,
    reason: &'static str,
) {
    if let Err(error) =
        release_responses_plan_usage_cost(state, control_decision, plan, reservation_token, reason)
            .await
    {
        warn!(
            event_name = "responses_websocket_plan_usage_cost_release_failed",
            log_type = "ops",
            request_id = %plan.request_id,
            candidate_id = ?plan.candidate_id,
            reservation_token,
            reason,
            error = ?error,
            "gateway failed to release a Responses WebSocket plan cost reservation"
        );
    }
}

/// 组装一轮 turn 的 decision。
///
/// `effective_client_event` 必须是**已经过请求侧脱敏**的客户端事件（见
/// `super::redaction`），`provider_event` 由它派生。审计里的 `original_request_body`
/// 直接用它覆盖 seed：continuation 的 seed 来自绑定那一轮的 report_context，不覆盖
/// 会记成上一轮的 body；但覆盖成 raw 事件又会把已脱敏的审计内容换回原文，等于
/// 脱敏在审计侧失效。
pub(super) fn prepare_responses_websocket_turn_decision(
    template: &AiExecutionDecision,
    request_id: String,
    reuse_selected_candidate: bool,
    effective_client_event: &Value,
    provider_event: &Value,
    connection_trace_id: &str,
    turn_index: u64,
    logical_turn_id: &str,
    turn_attempt: u32,
) -> AiExecutionDecision {
    let mut decision = template.clone();
    decision.request_id = Some(request_id.clone());
    if !reuse_selected_candidate {
        decision.candidate_id = None;
    }
    decision.provider_request_body = Some(provider_event.clone());
    decision.provider_request_body_base64 = None;
    decision.report_context = Some(prepare_websocket_report_context(
        decision.report_context.take(),
        request_id.as_str(),
        reuse_selected_candidate,
        effective_client_event,
        provider_event,
        connection_trace_id,
        turn_index,
        logical_turn_id,
        turn_attempt,
    ));
    decision
}

pub(super) async fn begin_unowned_responses_websocket_turn(
    state: &AppState,
    parts: &http::request::Parts,
    control_decision: &GatewayControlDecision,
    decision: AiExecutionDecision,
    client_event: &Value,
    plan_usage_policy_snapshot: Option<PlanUsagePolicySnapshot>,
) -> Result<ResponsesProviderAttempt, GatewayError> {
    let planned_report_context = decision.report_context.clone();
    let attempt = match build_openai_responses_stream_plan_from_decision(
        parts,
        client_event,
        decision,
        false,
    ) {
        Ok(Some(attempt)) => attempt,
        Ok(None) => {
            release_pool_key_lease_from_report_context(state, planned_report_context.as_ref())
                .await;
            return Err(GatewayError::Internal(
                "Responses WebSocket request could not build a usage/audit stream plan".to_string(),
            ));
        }
        Err(error) => {
            release_pool_key_lease_from_report_context(state, planned_report_context.as_ref())
                .await;
            return Err(error);
        }
    };
    let mut plan = attempt.plan;
    let (first_event_timeout, terminal_timeout) =
        resolve_responses_websocket_turn_timeouts(plan.timeouts.as_ref());
    let report_kind = match attempt.report_kind {
        Some(report_kind) => report_kind,
        None => {
            release_local_pool_key_lease(
                state,
                LocalExecutionEffectContext {
                    plan: &plan,
                    report_context: attempt.report_context.as_ref(),
                },
            )
            .await;
            return Err(GatewayError::Internal(
                "Responses WebSocket request is missing an execution report kind".to_string(),
            ));
        }
    };
    let mut report_context = attempt.report_context;

    let balance_rejection = execution_plan_balance_capacity_rejection(
        state,
        control_decision,
        &plan,
        report_context.as_ref(),
    )
    .await;
    let balance_rejection = match balance_rejection {
        Ok(rejection) => rejection,
        Err(error) => {
            release_local_pool_key_lease(
                state,
                LocalExecutionEffectContext {
                    plan: &plan,
                    report_context: report_context.as_ref(),
                },
            )
            .await;
            return Err(error);
        }
    };
    if let Some(rejection) = balance_rejection {
        release_local_pool_key_lease(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
        )
        .await;
        return Err(websocket_auth_rejection_error(rejection));
    }

    let candidate_started_at_unix_ms = current_unix_ms();
    ensure_execution_request_candidate_slot(state, &mut plan, &mut report_context).await;
    let admission = match ResponsesWebSocketTurnAdmission::acquire(
        state,
        &plan,
        plan.request_id.as_str(),
    )
    .await
    {
        Ok(admission) => admission,
        Err(error) => {
            release_then_record_responses_websocket_admission_failure(
                release_local_pool_key_lease(
                    state,
                    LocalExecutionEffectContext {
                        plan: &plan,
                        report_context: report_context.as_ref(),
                    },
                ),
                record_responses_websocket_admission_failure(
                    state,
                    &plan,
                    report_context.as_ref(),
                    candidate_started_at_unix_ms,
                    &error,
                ),
            )
            .await;
            return Err(error);
        }
    };

    let reservation_token = uuid::Uuid::new_v4().to_string();
    let cost_reservation = match reserve_responses_plan_usage_cost_owned(
        state,
        control_decision,
        &plan,
        report_context.as_ref(),
        plan_usage_policy_snapshot.as_ref(),
        reservation_token,
    )
    .await
    {
        Ok(ResponsesPlanUsageCostReservationStart::NotRequired) => None,
        Ok(ResponsesPlanUsageCostReservationStart::Reserved(guard)) => {
            report_context =
                attach_plan_usage_reservation_token(report_context, guard.reservation_token());
            Some(guard)
        }
        Ok(ResponsesPlanUsageCostReservationStart::Rejected(rejection)) => {
            let error = websocket_plan_usage_rejection_error(&rejection);
            admission.release().await;
            release_then_record_responses_websocket_admission_failure(
                release_local_pool_key_lease(
                    state,
                    LocalExecutionEffectContext {
                        plan: &plan,
                        report_context: report_context.as_ref(),
                    },
                ),
                record_responses_websocket_admission_failure(
                    state,
                    &plan,
                    report_context.as_ref(),
                    candidate_started_at_unix_ms,
                    &error,
                ),
            )
            .await;
            return Err(error);
        }
        Err(error) => {
            admission.release().await;
            release_then_record_responses_websocket_admission_failure(
                release_local_pool_key_lease(
                    state,
                    LocalExecutionEffectContext {
                        plan: &plan,
                        report_context: report_context.as_ref(),
                    },
                ),
                record_responses_websocket_admission_failure(
                    state,
                    &plan,
                    report_context.as_ref(),
                    candidate_started_at_unix_ms,
                    &error,
                ),
            )
            .await;
            return Err(error);
        }
    };

    let lifecycle = ExecutionAttemptLifecycle::begin(
        state,
        AttemptLifecycleSeed {
            plan,
            report_kind,
            report_context,
            // relay loop 是单任务：一段慢依赖会拖住整条连接的收发，所以每段
            // 记账 I/O 都要有等待上界。
            stage_guard: AttemptStageGuard::Bounded(RESPONSES_WEBSOCKET_LIFECYCLE_STAGE_TIMEOUT),
        },
    )
    .await;
    let plan_usage_cost_reservation = cost_reservation.map(|guard| guard.disarm());

    Ok(ResponsesProviderAttempt {
        lifecycle,
        started_at: Instant::now(),
        provider_request_started_at_unix_ms: current_unix_ms(),
        provider_request_order_id: uuid::Uuid::now_v7().to_string(),
        provider_headers: BTreeMap::new(),
        observer: ResponsesStructuredTerminalObserver::default(),
        provider_capture: AttemptBodyCapture::default(),
        client_capture: AttemptBodyCapture::default(),
        upstream_bytes: 0,
        first_event_elapsed_ms: None,
        first_event_timeout,
        terminal_timeout,
        admission: Some(admission),
        plan_usage_cost_reservation,
        terminal_error_body: None,
        provider_outcome: None,
        client_delivery: AttemptClientDelivery::Complete,
        upstream_request_state: UpstreamRequestState::NotStarted,
    })
}

async fn release_then_record_responses_websocket_admission_failure(
    release_pool_lease: impl Future<Output = ()>,
    record_candidate_failure: impl Future<Output = ()>,
) {
    // Lease cleanup protects live routing capacity and must not sit behind a
    // slow candidate writer. The candidate write still follows immediately so
    // the seeded row reaches a terminal state on the ordinary error path.
    release_pool_lease.await;
    record_candidate_failure.await;
}

async fn record_responses_websocket_admission_failure(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    candidate_started_at_unix_ms: u64,
    error: &GatewayError,
) {
    let terminal_at_unix_ms = current_unix_ms();
    record_local_request_candidate_status(
        state,
        plan,
        report_context,
        responses_websocket_admission_failure_update(
            candidate_started_at_unix_ms,
            terminal_at_unix_ms,
            error,
        ),
    )
    .await;
}

fn responses_websocket_admission_failure_update(
    candidate_started_at_unix_ms: u64,
    terminal_at_unix_ms: u64,
    error: &GatewayError,
) -> SchedulerRequestCandidateStatusUpdate {
    let (status_code, error_type, error_message) = match error {
        GatewayError::AdmissionTimeout {
            gate,
            queue_budget_ms,
            ..
        } => (
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "gateway_admission_timeout",
            format!("gateway admission gate {gate} timed out after {queue_budget_ms}ms"),
        ),
        GatewayError::PlanUsageLimited(_) => (
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "plan_usage_limit_exceeded",
            "subscription plan usage limit exceeded".to_string(),
        ),
        other => (
            StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            "gateway_admission_failed",
            format!("{other:?}"),
        ),
    };
    SchedulerRequestCandidateStatusUpdate {
        status: RequestCandidateStatus::Failed,
        status_code: Some(status_code),
        error_type: Some(error_type.to_string()),
        error_message: Some(error_message),
        latency_ms: Some(terminal_at_unix_ms.saturating_sub(candidate_started_at_unix_ms)),
        started_at_unix_ms: Some(candidate_started_at_unix_ms),
        finished_at_unix_ms: Some(terminal_at_unix_ms),
    }
}

fn websocket_auth_rejection_error(rejection: GatewayLocalAuthRejection) -> GatewayError {
    let (status, message) = match rejection {
        GatewayLocalAuthRejection::InvalidApiKey => {
            (StatusCode::UNAUTHORIZED, "The API key is invalid")
        }
        GatewayLocalAuthRejection::LockedApiKey => (
            StatusCode::FORBIDDEN,
            "The API key is locked and cannot be used",
        ),
        GatewayLocalAuthRejection::WalletUnavailable => {
            (StatusCode::FORBIDDEN, "The account wallet is unavailable")
        }
        GatewayLocalAuthRejection::BalanceDenied { remaining } => {
            let message = match remaining {
                Some(remaining) => format!("Insufficient balance (remaining: ${remaining:.2})"),
                None => "Insufficient balance".to_string(),
            };
            return GatewayError::Client {
                status: StatusCode::TOO_MANY_REQUESTS,
                message,
            };
        }
        GatewayLocalAuthRejection::ProviderNotAllowed { .. } => (
            StatusCode::FORBIDDEN,
            "The provider is not allowed for this API key",
        ),
        GatewayLocalAuthRejection::ApiFormatNotAllowed { .. } => (
            StatusCode::FORBIDDEN,
            "The API format is not allowed for this API key",
        ),
        GatewayLocalAuthRejection::ModelNotAllowed { .. } => (
            StatusCode::FORBIDDEN,
            "The requested model is not allowed for this API key",
        ),
        GatewayLocalAuthRejection::IpNotAllowed { .. } => (
            StatusCode::UNAUTHORIZED,
            "The current IP is not allowed for this API key",
        ),
    };
    GatewayError::Client {
        status,
        message: message.to_string(),
    }
}

impl ResponsesProviderAttempt {
    pub(super) fn cancel_on_client_disconnect(&self) -> bool {
        crate::orchestration::routing_execution_policy_from_report_context(
            self.lifecycle.report_context(),
        )
        .is_some_and(|policy| policy.cancel_on_client_disconnect)
    }

    /// Releases all per-turn capacity before terminal persistence starts.
    /// Provider-pool runtime tokens normally use an awaited removal. The
    /// bounded wait prevents a broken runtime backend from stalling the relay;
    /// the guard's `Drop` path remains the timeout fallback.
    pub(super) async fn release_admission(&mut self) {
        if let Some(admission) = self.admission.take() {
            let _ = self
                .lifecycle
                .stage_guard()
                .await_stage(
                    self.lifecycle.trace_id(),
                    "turn_admission_release",
                    admission.release(),
                )
                .await;
        }
    }

    /// Releases this attempt's reserved cost before a transparent retry. The
    /// terminal usage write that follows may reconcile the same token again;
    /// repository reconciliation is idempotent, so the first Released state
    /// wins and the replacement attempt can reserve independently.
    pub(super) async fn release_plan_usage_cost_for_retry(
        &mut self,
        state: &AppState,
    ) -> Result<(), GatewayError> {
        if self.upstream_request_state.may_have_reached_provider()
            && self.provider_outcome.is_none()
        {
            return Err(GatewayError::Internal(
                "cannot release an unresolved plan cost reservation after the upstream request may have been sent"
                    .to_string(),
            ));
        }
        let Some(reservation) = self.plan_usage_cost_reservation.take() else {
            return Ok(());
        };
        let release = release_responses_plan_usage_cost(
            state,
            &reservation.control_decision,
            self.lifecycle.plan(),
            reservation.reservation_token.as_str(),
            "transparent_retry",
        )
        .await;
        if release.is_err() {
            self.plan_usage_cost_reservation = Some(reservation);
        }
        release
    }

    /// A transparent replacement failed before its request reached an
    /// upstream. No terminal usage event can safely own the still-reserved
    /// estimate, so release it explicitly before the orphan attempt is
    /// finalized or dropped.
    pub(super) async fn release_plan_usage_cost_before_upstream_send(
        &mut self,
        state: &AppState,
        reason: &'static str,
    ) {
        if self.upstream_request_state.may_have_reached_provider() {
            return;
        }
        let Some(reservation) = self.plan_usage_cost_reservation.take() else {
            return;
        };
        release_responses_plan_usage_cost_best_effort(
            state,
            &reservation.control_decision,
            self.lifecycle.plan(),
            reservation.reservation_token.as_str(),
            reason,
        )
        .await;
    }

    pub(super) fn set_provider_response_headers(&mut self, headers: BTreeMap<String, String>) {
        let observed_at_unix_ms = current_unix_ms();
        let report_context = attach_provider_response_headers_to_report_context(
            self.lifecycle.take_report_context(),
            &headers,
            self.provider_request_started_at_unix_ms,
            observed_at_unix_ms,
            &self.provider_request_order_id,
        );
        self.lifecycle.set_report_context(report_context);
        self.provider_headers = headers;
    }

    /// Starts the per-turn response deadlines only after the corresponding
    /// `response.create` has been accepted by the upstream socket writer.
    pub(super) fn record_upstream_request_state(&mut self, state: UpstreamRequestState) {
        match state {
            UpstreamRequestState::NotStarted => {
                debug_assert!(
                    false,
                    "upstream request state cannot move back to not started"
                );
            }
            UpstreamRequestState::PossiblySent => {
                debug_assert_eq!(
                    self.upstream_request_state,
                    UpstreamRequestState::NotStarted
                );
                self.upstream_request_state = state;
                self.started_at = Instant::now();
                self.provider_request_started_at_unix_ms = current_unix_ms();
                self.provider_request_order_id = uuid::Uuid::now_v7().to_string();
                self.first_event_elapsed_ms = None;
            }
            UpstreamRequestState::Sent => {
                debug_assert_eq!(
                    self.upstream_request_state,
                    UpstreamRequestState::PossiblySent
                );
                self.upstream_request_state = state;
            }
        }
    }

    /// Selects a cancellation-safe fallback for an attempt whose owner task
    /// disappeared. Before the upstream write this is a void cancellation;
    /// after the write it remains a gateway relay failure because provider
    /// work may already have started.
    pub(super) const fn abandonment_outcome(&self) -> ResponsesWebSocketTurnOutcome {
        ResponsesWebSocketTurnOutcome::relay_task_abandonment(
            self.upstream_request_state.may_have_reached_provider(),
        )
    }

    pub(super) fn deadline(&self) -> ResponsesWebSocketTurnDeadline {
        let (phase, timeout) = if self.first_event_elapsed_ms.is_some() {
            (
                ResponsesWebSocketTurnTimeoutPhase::AwaitingTerminal,
                self.terminal_timeout,
            )
        } else {
            (
                ResponsesWebSocketTurnTimeoutPhase::AwaitingFirstEvent,
                self.first_event_timeout.min(self.terminal_timeout),
            )
        };
        ResponsesWebSocketTurnDeadline {
            phase,
            deadline: self.started_at + timeout,
            timeout,
        }
    }

    pub(super) fn observe_upstream_frame(
        &mut self,
        frame: &ParsedResponsesWebSocketFrame<'_>,
        adapter: &dyn ResponsesWebSocketProtocolAdapter,
    ) -> Option<ResponsesWebSocketTurnObservation> {
        self.upstream_bytes = self
            .upstream_bytes
            .saturating_add(frame.raw_text().len() as u64);
        if self.first_event_elapsed_ms.is_none() {
            self.first_event_elapsed_ms = Some(elapsed_ms(self.started_at));
        }

        // 一帧可以带多个协议事件（批量帧），必须拆开：观测器按事件推进状态机，
        // 整帧当一个事件喂会丢掉批量里最后那个 completed 的 usage。
        let events = frame.protocol_events();
        let mut report_context = self.lifecycle.take_report_context();
        for event in &events {
            // 观测已经走结构化入口，但捕获仍然必须是 SSE 形状：usage runtime 按
            // `data:` 行解析被捕获的 body 判定终态。
            self.capture_sse_event(event);
            adapter.decorate_turn_report_context(&mut report_context, event);
        }
        self.lifecycle.set_report_context(report_context);
        let fallback_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
        });
        let report_context = self.lifecycle.report_context().unwrap_or(&fallback_context);
        self.observer.observe_events(report_context, &events);

        let event_type = frame.event_type().unwrap_or_default();
        if matches!(event_type, "error" | "response.failed") {
            self.terminal_error_body = frame
                .terminal_event()
                .and_then(|event| serde_json::to_string(event).ok());
        }
        if let Some(outcome) = provider_terminal_outcome(frame) {
            // provider 的终态是独立事实：先记下来，之后即使客户端投递失败、
            // 结算信号变成 Cancelled，这条事实也不会被擦掉。
            self.provider_outcome.get_or_insert(
                attempt_facts_for_outcome(None, AttemptClientDelivery::Complete, outcome).provider,
            );
            return Some(ResponsesWebSocketTurnObservation::Terminal(outcome));
        }
        if frame.is_started() {
            return Some(ResponsesWebSocketTurnObservation::Started);
        }
        None
    }

    pub(super) fn observe_invalid_upstream_text(
        &mut self,
        text: &str,
    ) -> Option<ResponsesWebSocketTurnObservation> {
        self.upstream_bytes = self.upstream_bytes.saturating_add(text.len() as u64);
        if self.first_event_elapsed_ms.is_none() {
            self.first_event_elapsed_ms = Some(elapsed_ms(self.started_at));
        }
        self.capture_sse_event(&json!({
            "type": "error",
            "error": {
                "type": "gateway_protocol_error",
                "message": "upstream Responses WebSocket event was not valid JSON"
            }
        }));
        self.observer
            .disable_with_error("upstream Responses WebSocket event was not valid JSON");
        Some(ResponsesWebSocketTurnObservation::Terminal(
            ResponsesWebSocketTurnOutcome::Failure {
                status_code: 502,
                reason: "upstream Responses WebSocket event was not valid JSON",
            },
        ))
    }

    /// 记录「这一个 attempt 的内容没能完整交付给客户端」。
    ///
    /// 与 provider 终态分开记录：供应商已经给出终态时，这条事实只影响
    /// candidate 的错误分类和审计里的投递标记，不作废账单。
    pub(super) fn record_client_delivery_aborted(&mut self, reason: &'static str) {
        if matches!(self.client_delivery, AttemptClientDelivery::Complete) {
            self.client_delivery = AttemptClientDelivery::Aborted { reason };
        }
    }

    pub(super) fn capture_client_frame(&mut self, event: &Value) {
        self.client_capture
            .append(&websocket_event_as_sse_line(event));
    }

    pub(super) async fn mark_stream_started(&mut self, state: &AppState) {
        let telemetry = self.telemetry();
        self.lifecycle
            .mark_started(state, STREAM_STARTED_STATUS_CODE, &telemetry)
            .await;
    }

    /// 结算这一个 attempt。
    ///
    /// `outcome` 是「为什么现在结算」的信号，不是供应商事实本身：
    /// [`attempt_facts_for_outcome`] 把它和已观察到的 provider 终态、已记录的
    /// 投递结果一起，拆成 provider outcome 与 client delivery 两个正交事实。
    /// 之后的四段记账（usage terminal → candidate terminal → provider 效果 →
    /// execution report）由共享的 [`ExecutionAttemptLifecycle::settle`] 负责，
    /// 这里只提供 WS 观察到的终态事实。
    async fn settle(mut self, state: &AppState, outcome: ResponsesWebSocketTurnOutcome) {
        let facts = attempt_facts_for_outcome(self.provider_outcome, self.client_delivery, outcome);
        if self.upstream_request_state.may_have_reached_provider()
            && !facts.provider.is_terminal()
            && self.plan_usage_cost_reservation.is_some()
        {
            let report_context =
                attach_plan_usage_reservation_deferred(self.lifecycle.take_report_context());
            self.lifecycle.set_report_context(report_context);
        }
        if let Some(reason) = facts.delivery.aborted_reason() {
            let report_context = attach_client_delivery_to_report_context(
                self.lifecycle.take_report_context(),
                reason,
            );
            self.lifecycle.set_report_context(report_context);
        }
        let summary = self.finish_summary(facts);
        let telemetry = self.telemetry();
        let terminal_error_body = self.terminal_error_body.take();

        // 终态载荷完整了才释放准入：usage/审计写入期间不再占着 gateway/供应商容量。
        if let Some(admission) = self.admission.take() {
            let _ = self
                .lifecycle
                .stage_guard()
                .await_stage(
                    self.lifecycle.trace_id(),
                    "turn_admission_release",
                    admission.release(),
                )
                .await;
        }

        self.lifecycle
            .settle(
                state,
                AttemptTerminalFactsInput {
                    facts,
                    terminal_summary: summary,
                    telemetry,
                    provider_headers: std::mem::take(&mut self.provider_headers),
                    provider_body: &self.provider_capture,
                    client_body: &self.client_capture,
                    provider_error_body: terminal_error_body.as_deref(),
                    reason: facts.reason(),
                },
            )
            .await;
    }

    fn capture_sse_event(&mut self, event: &Value) {
        self.provider_capture
            .append(&websocket_event_as_sse_line(event));
    }

    /// 终态摘要。
    ///
    /// 只消费两个正交事实：`forced_error` 只在「供应商没给出终态且内容已完整
    /// 交付」时补 parser_error，`cancelled` 覆盖供应商声明取消与客户端投递失败
    /// 两种情形——与拆分前 `outcome.forced_error()` / `outcome.cancelled()` 的
    /// 取值逐一对应。
    fn finish_summary(&mut self, facts: AttemptTerminalFacts) -> ExecutionStreamTerminalSummary {
        let fallback_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
        });
        let report_context = self.lifecycle.report_context().unwrap_or(&fallback_context);
        let mut summary = self.observer.finish(report_context);
        if let Some(reason) = facts.forced_error() {
            if summary.parser_error.is_none() {
                summary.parser_error = Some(reason.to_string());
            }
        }
        // 只有作废账单的那一侧才把摘要改写成 cancelled。provider 终态已到达时
        // 摘要必须保留真实的 finish_reason 和 usage，否则计费记录会被写坏。
        if attempt_billing_is_void(facts) {
            summary.observed_finish = true;
            if summary.finish_reason.is_none() {
                summary.finish_reason = Some("cancelled".to_string());
            }
        } else if !summary.observed_finish && summary.parser_error.is_none() {
            summary.parser_error = Some(
                "upstream Responses WebSocket ended before a provider terminal event".to_string(),
            );
        }
        summary
    }

    fn telemetry(&self) -> ExecutionTelemetry {
        ExecutionTelemetry {
            ttfb_ms: self.first_event_elapsed_ms,
            elapsed_ms: Some(elapsed_ms(self.started_at)),
            upstream_bytes: Some(self.upstream_bytes),
        }
    }
}

impl ResponsesProviderAttempt {
    /// Finalizes a turn whose owner is already gone, releasing admission first.
    ///
    /// The normal path releases admission before spawning the finalizer; a
    /// turn reclaimed from a lost relay task has to do both itself.
    pub(super) async fn finalize_detached(
        mut self,
        state: &AppState,
        outcome: ResponsesWebSocketTurnOutcome,
    ) {
        self.release_admission().await;
        self.settle(state, outcome).await;
    }
}

/// 把一轮 turn 的事实写进审计/用量 report context。
///
/// `effective_client_event` 是脱敏后的客户端事件（未启用脱敏时就是原事件）。
/// HTTP 路径的约定是「脱敏生效时审计记录脱敏后的 body」
/// （`ai_serving/planner/standard/openai/responses/decision/payload.rs`），
/// WS 这里必须保持一致，否则上游收到的是脱敏内容、审计里却留着原始 PII。
fn prepare_websocket_report_context(
    report_context: Option<Value>,
    request_id: &str,
    reuse_selected_candidate: bool,
    effective_client_event: &Value,
    provider_event: &Value,
    connection_trace_id: &str,
    turn_index: u64,
    logical_turn_id: &str,
    turn_attempt: u32,
) -> Value {
    let mut object = match report_context {
        Some(Value::Object(object)) => object,
        Some(other) => Map::from_iter([("seed".to_string(), other)]),
        None => Map::new(),
    };
    object.insert(
        "request_id".to_string(),
        Value::String(request_id.to_string()),
    );
    if !reuse_selected_candidate {
        for field in [
            "candidate_id",
            "candidate_index",
            "retry_index",
            "pool_key_index",
            "candidate_group_id",
            "pool_key_lease_key",
            "pool_key_lease_owner",
            "pool_key_lease_token",
            "pool_key_lease_fencing_token",
            "pool_key_lease_ttl_ms",
            "scheduler_affinity_epoch",
        ] {
            object.remove(field);
        }
    }
    object.insert(
        "original_request_body".to_string(),
        effective_client_event.clone(),
    );
    if let Some(model) = effective_client_event
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        object.insert("model".to_string(), Value::String(model.to_string()));
    }
    if let Some(mapped_model) = provider_event
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        object.insert(
            "mapped_model".to_string(),
            Value::String(mapped_model.to_string()),
        );
    }
    object.insert(WEBSOCKET_MODE_METADATA_KEY.to_string(), Value::Bool(true));
    object.insert(
        WEBSOCKET_CONNECTION_TRACE_REPORT_CONTEXT_FIELD.to_string(),
        Value::String(connection_trace_id.to_string()),
    );
    object.insert(
        WEBSOCKET_TURN_INDEX_REPORT_CONTEXT_FIELD.to_string(),
        Value::Number(turn_index.into()),
    );
    object.insert(
        WEBSOCKET_LOGICAL_TURN_ID_REPORT_CONTEXT_FIELD.to_string(),
        Value::String(logical_turn_id.to_string()),
    );
    object.insert(
        WEBSOCKET_TURN_ATTEMPT_REPORT_CONTEXT_FIELD.to_string(),
        Value::Number(turn_attempt.into()),
    );
    object.insert(
        WEBSOCKET_TRANSPORT_METADATA_KEY.to_string(),
        Value::String("responses".to_string()),
    );
    Value::Object(object)
}

/// 在审计/用量 report context 上标记这一 attempt 的内容没能交付给客户端。
///
/// 只增字段，不改既有字段：账单本身按 provider 终态计，投递失败作为独立事实
/// 留在记录里，便于事后区分「客户端拿到了」和「客户端没拿到但已计费」。
fn attach_client_delivery_to_report_context(
    report_context: Option<Value>,
    reason: &str,
) -> Option<Value> {
    let mut object = match report_context {
        Some(Value::Object(object)) => object,
        Some(other) => Map::from_iter([("seed".to_string(), other)]),
        None => Map::new(),
    };
    object.insert(
        WEBSOCKET_CLIENT_DELIVERY_REPORT_CONTEXT_FIELD.to_string(),
        Value::String(WEBSOCKET_CLIENT_DELIVERY_ABORTED.to_string()),
    );
    object.insert(
        WEBSOCKET_CLIENT_DELIVERY_REASON_REPORT_CONTEXT_FIELD.to_string(),
        Value::String(reason.to_string()),
    );
    Some(Value::Object(object))
}

fn attach_plan_usage_reservation_deferred(report_context: Option<Value>) -> Option<Value> {
    let mut object = match report_context {
        Some(Value::Object(object)) => object,
        Some(other) => Map::from_iter([("seed".to_string(), other)]),
        None => Map::new(),
    };
    object.insert(
        PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY.to_string(),
        Value::Bool(true),
    );
    Some(Value::Object(object))
}

fn provider_terminal_outcome(
    frame: &ParsedResponsesWebSocketFrame<'_>,
) -> Option<ResponsesWebSocketTurnOutcome> {
    frame
        .terminal()
        .map(|terminal| ResponsesWebSocketTurnOutcome::ProviderTerminal {
            status_code: terminal.status_code,
            cancelled: terminal.cancelled,
        })
}

fn resolve_responses_websocket_turn_timeouts(
    timeouts: Option<&ExecutionTimeouts>,
) -> (Duration, Duration) {
    let first_event_timeout_ms = timeouts
        .and_then(|timeouts| timeouts.first_byte_ms)
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_WEBSOCKET_FIRST_EVENT_TIMEOUT_MS)
        .min(MAX_EXECUTION_STREAM_FIRST_BYTE_TIMEOUT_MS);
    let terminal_timeout_ms = timeouts
        .and_then(|timeouts| timeouts.total_ms)
        .filter(|value| *value > 0)
        .unwrap_or(MAX_EXECUTION_REQUEST_TIMEOUT_MS)
        .min(MAX_EXECUTION_REQUEST_TIMEOUT_MS);
    (
        Duration::from_millis(first_event_timeout_ms),
        Duration::from_millis(terminal_timeout_ms),
    )
}

fn websocket_event_as_sse_line(event: &Value) -> Vec<u8> {
    let payload = serde_json::to_string(event).unwrap_or_else(|_| {
        json!({
            "type": "error",
            "error": {
                "type": "gateway_protocol_error",
                "message": "upstream Responses WebSocket event could not be serialized"
            }
        })
        .to_string()
    });
    format!("data: {payload}\n\n").into_bytes()
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use aether_contracts::ExecutionTimeouts;
    use aether_data_contracts::repository::candidates::RequestCandidateStatus;
    use serde_json::json;

    use super::super::observation::ResponsesStructuredTerminalObserver;

    use super::super::frame::ParsedResponsesWebSocketFrame;
    use super::super::settlement::{
        attempt_facts_for_outcome, settle_signal_for_client_delivery_failure,
    };
    use super::{
        attach_client_delivery_to_report_context, attach_plan_usage_reservation_deferred,
        attach_plan_usage_reservation_token, prepare_websocket_report_context,
        provider_terminal_outcome, release_then_record_responses_websocket_admission_failure,
        resolve_responses_websocket_turn_timeouts, responses_websocket_admission_failure_update,
        websocket_event_as_sse_line, ResponsesWebSocketTurnDeadline, ResponsesWebSocketTurnOutcome,
        ResponsesWebSocketTurnTimeoutPhase,
    };
    use crate::execution_runtime::attempt_lifecycle::{
        classify_attempt_settlement, AttemptBilling, AttemptCandidateError, AttemptCandidateStatus,
        AttemptClientDelivery, AttemptProviderEffect, AttemptSettlementInputs,
    };
    use crate::GatewayError;

    #[test]
    fn server_plan_usage_reservation_token_overrides_seed_value() {
        let context = attach_plan_usage_reservation_token(
            Some(json!({
                "candidate_index": 2,
                "plan_usage_reservation_token": "client-controlled-token"
            })),
            "server-reservation-token",
        )
        .expect("reservation context");

        assert_eq!(context["candidate_index"], 2);
        assert_eq!(
            context["plan_usage_reservation_token"],
            "server-reservation-token"
        );
        assert_eq!(context["plan_usage_reservation_deferred"], false);
    }

    #[test]
    fn gateway_can_defer_a_server_owned_reservation_after_transport_handoff() {
        let context = attach_plan_usage_reservation_deferred(Some(json!({
            "plan_usage_reservation_token": "server-reservation-token",
            "plan_usage_reservation_deferred": false
        })))
        .expect("reservation context");

        assert_eq!(context["plan_usage_reservation_deferred"], true);
    }

    #[tokio::test]
    async fn admission_failure_releases_pool_lease_before_recording_candidate_terminal() {
        let phase = Arc::new(AtomicU8::new(0));
        let release_phase = Arc::clone(&phase);
        let record_phase = Arc::clone(&phase);

        release_then_record_responses_websocket_admission_failure(
            async move {
                assert_eq!(release_phase.swap(1, Ordering::SeqCst), 0);
            },
            async move {
                assert_eq!(record_phase.swap(2, Ordering::SeqCst), 1);
            },
        )
        .await;

        assert_eq!(phase.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn admission_timeout_terminalizes_the_seeded_candidate() {
        let update = responses_websocket_admission_failure_update(
            1_000,
            1_025,
            &GatewayError::AdmissionTimeout {
                trace_id: "turn-1".to_string(),
                gate: "gateway_upstream_execution",
                queue_budget_ms: 25,
            },
        );

        assert_eq!(update.status, RequestCandidateStatus::Failed);
        assert_eq!(update.status_code, Some(429));
        assert_eq!(
            update.error_type.as_deref(),
            Some("gateway_admission_timeout")
        );
        assert_eq!(
            update.error_message.as_deref(),
            Some("gateway admission gate gateway_upstream_execution timed out after 25ms")
        );
        assert_eq!(update.latency_ms, Some(25));
        assert_eq!(update.started_at_unix_ms, Some(1_000));
        assert_eq!(update.finished_at_unix_ms, Some(1_025));
    }

    #[test]
    fn non_timeout_admission_failure_still_terminalizes_the_seeded_candidate() {
        let update = responses_websocket_admission_failure_update(
            50,
            40,
            &GatewayError::Internal("admission gate closed".to_string()),
        );

        assert_eq!(update.status, RequestCandidateStatus::Failed);
        assert_eq!(update.status_code, Some(500));
        assert_eq!(
            update.error_type.as_deref(),
            Some("gateway_admission_failed")
        );
        assert_eq!(update.latency_ms, Some(0));
        assert_eq!(update.started_at_unix_ms, Some(50));
        assert_eq!(update.finished_at_unix_ms, Some(40));
    }

    #[test]
    fn followup_context_uses_a_fresh_request_and_candidate() {
        let context = prepare_websocket_report_context(
            Some(json!({
                "request_id":"connection",
                "candidate_id":"candidate",
                "candidate_index": 0,
                "pool_key_lease_key": "lease",
                "original_request_body":{"model":"public"}
            })),
            "turn-2",
            false,
            &json!({"type":"response.create","model":"public"}),
            &json!({"type":"response.create","model":"provider-public"}),
            "connection",
            2,
            "logical-turn-2",
            1,
        );
        assert_eq!(context["request_id"], "turn-2");
        assert!(context.get("candidate_id").is_none());
        assert!(context.get("candidate_index").is_none());
        assert!(context.get("pool_key_lease_key").is_none());
        assert_eq!(context["original_request_body"]["type"], "response.create");
        assert_eq!(context["model"], "public");
        assert_eq!(context["mapped_model"], "provider-public");
        assert_eq!(context["websocket_mode"], true);
        assert_eq!(context["websocket_transport"], "responses");
        assert_eq!(context["websocket_logical_turn_id"], "logical-turn-2");
        assert_eq!(context["websocket_turn_attempt"], 1);
    }

    #[test]
    fn replanned_context_keeps_selected_candidate_and_records_the_new_client_model() {
        let context = prepare_websocket_report_context(
            Some(json!({
                "request_id": "prewarm",
                "candidate_id": "terra-candidate",
                "original_request_body": {"model": "gpt-5.6-sol", "generate": false}
            })),
            "turn-2",
            true,
            &json!({
                "type": "response.create",
                "model": "gpt-5.6-terra",
                "input": "hello"
            }),
            &json!({
                "type": "response.create",
                "model": "gpt-5.6-terra-provider",
                "input": "hello"
            }),
            "connection",
            2,
            "logical-turn-2",
            2,
        );

        assert_eq!(context["request_id"], "turn-2");
        assert_eq!(context["candidate_id"], "terra-candidate");
        assert_eq!(context["original_request_body"]["model"], "gpt-5.6-terra");
        assert_eq!(context["model"], "gpt-5.6-terra");
        assert_eq!(context["mapped_model"], "gpt-5.6-terra-provider");
        assert_eq!(context["websocket_mode"], true);
        assert_eq!(context["websocket_logical_turn_id"], "logical-turn-2");
        assert_eq!(context["websocket_turn_attempt"], 2);
    }

    #[test]
    fn completed_event_is_captured_as_a_responses_sse_terminal_event() {
        let event = json!({
            "type": "response.completed",
            "response": {
                "id": "resp_ws_usage_123",
                "model": "gpt-5.6",
                "usage": {
                    "input_tokens": 3,
                    "output_tokens": 5,
                    "total_tokens": 8
                }
            }
        });
        let raw = serde_json::to_string(&event).expect("event should serialize");
        let frame = ParsedResponsesWebSocketFrame::parse(&raw).expect("event should parse");
        let outcome = provider_terminal_outcome(&frame);
        assert_eq!(
            outcome,
            Some(ResponsesWebSocketTurnOutcome::ProviderTerminal {
                status_code: 200,
                cancelled: false
            })
        );
        let capture = String::from_utf8(websocket_event_as_sse_line(&event))
            .expect("capture should be UTF-8");
        assert_eq!(capture, format!("data: {event}\n\n"));

        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
        });
        let mut observer = ResponsesStructuredTerminalObserver::default();
        observer.observe_events(&report_context, &[&event]);
        let summary = observer.finish(&report_context);
        let usage = summary
            .standardized_usage
            .expect("response.completed usage must reach the terminal summary");
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.dimensions.get("total_tokens"), Some(&json!(8)));
    }

    #[test]
    fn a_legitimate_incomplete_is_a_successful_provider_terminal_that_keeps_its_usage() {
        // 写满 max_output_tokens 的 incomplete 是合法终态：状态码不再是 502，
        // usage 观测器照样能看到 finish 和 token，记账层不该把它当解析失败。
        let event = json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_ws_incomplete_123",
                "model": "gpt-5.6",
                "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"},
                "output": [],
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 7,
                    "total_tokens": 11
                }
            }
        });
        let raw = serde_json::to_string(&event).expect("event should serialize");
        let frame = ParsedResponsesWebSocketFrame::parse(&raw).expect("event should parse");
        let outcome = provider_terminal_outcome(&frame);
        assert_eq!(
            outcome,
            Some(ResponsesWebSocketTurnOutcome::ProviderTerminal {
                status_code: 200,
                cancelled: false
            })
        );
        let outcome = outcome.expect("incomplete should end the turn");
        let facts = attempt_facts_for_outcome(None, AttemptClientDelivery::Complete, outcome);
        assert!(!facts.provider.cancelled_by_provider());
        assert!(facts.forced_error().is_none());
        assert!(!facts.provider.stream_timeout());
        assert!(facts.provider.is_terminal());

        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
        });
        let mut observer = ResponsesStructuredTerminalObserver::default();
        observer.observe_events(&report_context, &[&event]);
        let summary = observer.finish(&report_context);
        assert!(summary.observed_finish);
        assert_eq!(summary.finish_reason.as_deref(), Some("length"));
        assert!(summary.parser_error.is_none());
        let usage = summary
            .standardized_usage
            .expect("incomplete usage must reach the terminal summary");
        assert_eq!(usage.input_tokens, 4);
        assert_eq!(usage.output_tokens, 7);

        // 结算表用这些事实决定是否投射供应商失败：合法 incomplete 即使被记账层
        // 判成失败，也必须落在「不扣健康分、只释放 lease」一侧，且账单照记。
        let settlement = classify_attempt_settlement(AttemptSettlementInputs {
            facts,
            report_represents_failure: true,
            observed_finish: summary.observed_finish,
            has_parser_error: summary.parser_error.is_some(),
        });
        assert_eq!(settlement.status_code, 200);
        assert_eq!(settlement.billing, AttemptBilling::Billed);
        assert_eq!(
            settlement.provider_effect,
            AttemptProviderEffect::ReleasePoolKeyLease
        );
        assert!(settlement.submit_execution_report);
    }

    #[test]
    fn an_incomplete_without_a_legitimate_reason_still_projects_a_provider_failure() {
        let raw = r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"error"}}}"#;
        let frame = ParsedResponsesWebSocketFrame::parse(raw).expect("event should parse");

        assert_eq!(
            provider_terminal_outcome(&frame),
            Some(ResponsesWebSocketTurnOutcome::ProviderTerminal {
                status_code: 502,
                cancelled: false
            })
        );
    }

    /// relay 级：provider 终态帧已经到达，但客户端 socket 已经关闭。
    ///
    /// 走的是 relay loop 写客户端失败时的完整决策链——真实帧解析 →
    /// 记录 provider 事实 → 记录投递失败 → 选结算信号 → 结算表。旧实现在这里
    /// 用 client_disconnected() 覆盖 outcome，于是一条已经产出 token 的响应被
    /// 记成 void billing、不投射供应商效果、也不提交 execution report。
    #[test]
    fn a_provider_terminal_that_reaches_a_closed_client_socket_is_still_billed() {
        let completed = json!({
            "type": "response.completed",
            "response": {
                "id": "resp_ws_delivery_failed",
                "model": "gpt-5.6",
                "usage": {"input_tokens": 3, "output_tokens": 5, "total_tokens": 8}
            }
        });
        let raw = serde_json::to_string(&completed).expect("event should serialize");
        let frame = ParsedResponsesWebSocketFrame::parse(&raw).expect("event should parse");

        // relay loop 观察到终态帧：attempt 记下 provider 事实。
        let observed = provider_terminal_outcome(&frame).expect("completed ends the turn");
        let recorded_provider =
            attempt_facts_for_outcome(None, AttemptClientDelivery::Complete, observed).provider;

        // 随后写客户端失败：记录投递失败，并按 provider 终态选结算信号。
        let delivery = AttemptClientDelivery::Aborted {
            reason: "gateway could not relay the provider event to the client",
        };
        let signal = settle_signal_for_client_delivery_failure(Some(observed));
        assert_eq!(
            signal, observed,
            "a reached terminal must remain the signal"
        );

        let facts = attempt_facts_for_outcome(Some(recorded_provider), delivery, signal);

        // 终态摘要保留真实 usage 与 finish_reason，不被改写成 cancelled。
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
        });
        let mut observer = ResponsesStructuredTerminalObserver::default();
        observer.observe_events(&report_context, &[&completed]);
        let summary = observer.finish(&report_context);
        assert!(summary.observed_finish);
        assert!(summary.parser_error.is_none());
        let usage = summary
            .standardized_usage
            .clone()
            .expect("usage must survive a delivery failure");
        assert_eq!(usage.input_tokens, 3);
        assert_eq!(usage.output_tokens, 5);

        let settlement = classify_attempt_settlement(AttemptSettlementInputs {
            facts,
            report_represents_failure: false,
            observed_finish: summary.observed_finish,
            has_parser_error: summary.parser_error.is_some(),
        });
        assert_eq!(settlement.billing, AttemptBilling::Billed);
        assert_eq!(settlement.status_code, 200);
        assert_eq!(settlement.candidate_status, AttemptCandidateStatus::Success);
        assert_eq!(
            settlement.provider_effect,
            AttemptProviderEffect::ProviderSuccess
        );
        assert!(settlement.submit_execution_report);
        // 投递失败仍然留痕。
        assert_eq!(
            settlement.candidate_error,
            AttemptCandidateError::ClientDeliveryFailed
        );
    }

    /// 同一条链路，但供应商还没给出终态：仍然作废账单、不提交 report。
    #[test]
    fn a_closed_client_socket_before_any_terminal_still_voids_the_bill() {
        let signal = settle_signal_for_client_delivery_failure(None);
        let facts = attempt_facts_for_outcome(
            None,
            AttemptClientDelivery::Aborted {
                reason: "gateway could not relay the provider event to the client",
            },
            signal,
        );
        let settlement = classify_attempt_settlement(AttemptSettlementInputs {
            facts,
            report_represents_failure: false,
            observed_finish: false,
            has_parser_error: false,
        });

        assert_eq!(settlement.billing, AttemptBilling::Void);
        assert_eq!(settlement.status_code, 499);
        assert_eq!(
            settlement.candidate_status,
            AttemptCandidateStatus::Cancelled
        );
        assert_eq!(
            settlement.provider_effect,
            AttemptProviderEffect::ReleasePoolKeyLease
        );
        assert!(!settlement.submit_execution_report);
    }

    /// 投递失败只往 report context 里加字段，不改既有字段。
    #[test]
    fn the_client_delivery_marker_only_adds_report_context_fields() {
        let context = attach_client_delivery_to_report_context(
            Some(json!({
                "request_id": "turn-2",
                "websocket_mode": true,
                "original_request_body": {"model": "public"},
            })),
            "gateway could not relay the provider event to the client",
        )
        .expect("marker should produce a report context");

        assert_eq!(context["websocket_client_delivery"], "aborted");
        assert_eq!(
            context["websocket_client_delivery_reason"],
            "gateway could not relay the provider event to the client"
        );
        assert_eq!(context["request_id"], "turn-2");
        assert_eq!(context["websocket_mode"], true);
        assert_eq!(context["original_request_body"]["model"], "public");
    }

    #[test]
    fn error_event_uses_the_top_level_status_code() {
        let event = json!({
            "type": "error",
            "status_code": 429,
            "error": {"type": "usage_limit_reached"},
        });
        let raw = serde_json::to_string(&event).expect("event should serialize");
        let frame = ParsedResponsesWebSocketFrame::parse(&raw).expect("event should parse");
        assert_eq!(
            provider_terminal_outcome(&frame),
            Some(ResponsesWebSocketTurnOutcome::ProviderTerminal {
                status_code: 429,
                cancelled: false,
            })
        );
    }

    #[test]
    fn quota_close_fallback_preserves_the_client_visible_status() {
        let outcome = ResponsesWebSocketTurnOutcome::provider_quota_exhausted();
        assert_eq!(outcome.status_code(), 429);
        assert!(matches!(
            outcome,
            ResponsesWebSocketTurnOutcome::Failure {
                status_code: 429,
                ..
            }
        ));
    }

    #[test]
    fn an_abandoned_turn_after_upstream_send_is_recorded_as_a_gateway_failure() {
        let outcome = ResponsesWebSocketTurnOutcome::relay_task_abandonment(true);
        let facts = attempt_facts_for_outcome(None, AttemptClientDelivery::Complete, outcome);
        let settlement = classify_attempt_settlement(AttemptSettlementInputs {
            facts,
            report_represents_failure: true,
            observed_finish: false,
            has_parser_error: false,
        });

        assert_eq!(settlement.status_code, 500);
        assert_eq!(settlement.billing, AttemptBilling::Billed);
        assert!(!facts.provider.cancelled_by_provider());
        assert!(facts.forced_error().is_some());
        assert!(settlement.submit_execution_report);
    }

    #[test]
    fn an_abandoned_turn_before_upstream_send_is_void_and_does_not_penalize_provider() {
        let outcome = ResponsesWebSocketTurnOutcome::relay_task_abandonment(false);
        let facts = attempt_facts_for_outcome(None, AttemptClientDelivery::Complete, outcome);
        let settlement = classify_attempt_settlement(AttemptSettlementInputs {
            facts,
            report_represents_failure: false,
            observed_finish: false,
            has_parser_error: false,
        });

        assert_eq!(settlement.status_code, 499);
        assert_eq!(settlement.billing, AttemptBilling::Void);
        assert_eq!(
            settlement.candidate_status,
            AttemptCandidateStatus::Cancelled
        );
        assert_eq!(settlement.candidate_error, AttemptCandidateError::Cancelled);
        assert_eq!(
            settlement.provider_effect,
            AttemptProviderEffect::ReleasePoolKeyLease
        );
        assert!(!settlement.submit_execution_report);
    }

    #[test]
    fn turn_timeouts_reuse_provider_first_byte_and_request_deadlines() {
        let (first_event, terminal) =
            resolve_responses_websocket_turn_timeouts(Some(&ExecutionTimeouts {
                first_byte_ms: Some(12_345),
                total_ms: Some(67_890),
                ..ExecutionTimeouts::default()
            }));

        assert_eq!(first_event, Duration::from_millis(12_345));
        assert_eq!(terminal, Duration::from_millis(67_890));
    }

    #[test]
    fn first_event_deadline_never_outlives_the_turn_deadline() {
        let started_at = Instant::now();
        let first_event = Duration::from_secs(30);
        let terminal = Duration::from_secs(10);
        let deadline = ResponsesWebSocketTurnDeadline {
            phase: ResponsesWebSocketTurnTimeoutPhase::AwaitingFirstEvent,
            deadline: started_at + first_event.min(terminal),
            timeout: first_event.min(terminal),
        };

        assert_eq!(
            deadline.phase,
            ResponsesWebSocketTurnTimeoutPhase::AwaitingFirstEvent
        );
        assert_eq!(deadline.timeout, Duration::from_secs(10));
        assert_eq!(deadline.deadline, started_at + Duration::from_secs(10));
    }
}
