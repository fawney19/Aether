use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use aether_ai_serving::{
    run_ai_attempt_loop, AiAttemptExecutionOutcome, AiAttemptLoopOutcome, AiAttemptLoopPort,
    AiAttemptRetryScope, AiExecutionAttempt,
};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_runtime::{AdmissionPermit, ConcurrencyPermit};
use aether_scheduler_core::{
    parse_request_candidate_report_context, SchedulerRequestCandidateStatusUpdate,
};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::Response;
use futures_util::StreamExt;
use tokio::time::{timeout, Duration, Instant};
use tracing::{debug, warn, Instrument};

use crate::ai_serving::LocalExecutionAttemptSource;
use crate::clock::current_unix_ms;
use crate::control::GatewayControlDecision;
use crate::execution_runtime::{
    acquire_upstream_execution_gate, build_transport_error_stop_response,
    execute_execution_runtime_stream_with_retry_scope,
    execute_execution_runtime_sync_with_retry_scope,
    mark_stream_candidate_watchdog_terminal_started, StreamCandidateWatchdogProgress,
    UpstreamExecutionGateProvider, UPSTREAM_EXECUTION_GATE_NAME,
};
use crate::executor::{
    attach_deferred_usage_context, build_local_execution_exhaustion,
    mark_deferred_upstream_response, LocalExecutionRequestOutcome,
};
use crate::handlers::shared::provider_pool::release_admin_provider_pool_key_lease;
use crate::log_ids::short_request_id;
use crate::orchestration::{
    local_execution_candidate_metadata_from_report_context,
    local_failover_policy_from_report_context, resolve_local_failover_policy,
    resolve_local_transport_failover_analysis_for_attempt, LocalFailoverDecision,
    LocalFailoverPolicy,
};
use crate::privacy::RedactionExecutionCandidateId;
use crate::request_candidate_runtime::{
    record_local_request_candidate_status, RequestCandidateRuntimeWriter,
};
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AppState, GatewayError};

const DEFAULT_STREAM_FIRST_BYTE_WATCHDOG_TIMEOUT_MS: u64 = 30_000;
const UPSTREAM_TARGET_GATE_NAME: &str = "gateway_upstream_target";
const UPSTREAM_EXECUTION_GATE_HOLD_STREAM_RESPONSE_ENV: &str =
    "AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_HOLD_STREAM_RESPONSE";
const UPSTREAM_EXECUTION_GATE_STREAM_HOLD_MODE_ENV: &str =
    "AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_STREAM_HOLD_MODE";

#[derive(Clone, Debug)]
pub(crate) struct BackgroundAdmissionPermit {
    _permit: AdmissionPermit,
}

impl BackgroundAdmissionPermit {
    pub(crate) fn new(permit: AdmissionPermit) -> Self {
        Self { _permit: permit }
    }
}

fn attach_redaction_execution_candidate(response: &mut Response<Body>, candidate_id: Option<&str>) {
    if let Some(candidate_id) = candidate_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        response
            .extensions_mut()
            .insert(RedactionExecutionCandidateId::new(candidate_id));
    }
}

pub(crate) async fn execute_sync_plan_and_reports<T>(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    plan_and_reports: Vec<T>,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
{
    let transfer_tracker = ProviderTransferTracker::for_request(parts);
    execute_sync_plan_and_reports_with_transfer_tracker(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
        &transfer_tracker,
    )
    .await
}

pub(crate) async fn execute_sync_plan_and_reports_with_transfer_tracker<T>(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    plan_and_reports: Vec<T>,
    transfer_tracker: &ProviderTransferTracker,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
{
    let candidate_count = plan_and_reports.len();
    let first_provider = plan_and_reports
        .first()
        .and_then(|item| item.execution_plan().provider_name.as_deref())
        .unwrap_or("-")
        .to_string();
    let span = tracing::debug_span!(
        "candidates",
        trace_id = %trace_id,
        plan_kind,
        candidate_count,
    );

    async move {
        tracing::debug!(
            event_name = "candidate_loop_started",
            log_type = "event",
            trace_id = %trace_id,
            plan_kind,
            candidate_count,
            first_provider = first_provider.as_str(),
            "candidate loop started"
        );

        let port = SyncAttemptLoopPort {
            state,
            parts,
            trace_id,
            decision,
            plan_kind,
            transfer_tracker,
        };
        let loop_result = run_ai_attempt_loop(&port, plan_and_reports).await;
        if loop_result.is_err() {
            release_active_plan_usage_policy_cost_best_effort(
                state,
                decision,
                transfer_tracker,
                "candidate_loop_error",
            )
            .await;
        }
        match loop_result? {
            AiAttemptLoopOutcome::Responded(response) => {
                Ok(LocalExecutionRequestOutcome::responded(response))
            }
            AiAttemptLoopOutcome::Deferred(response) => Ok(
                LocalExecutionRequestOutcome::responded(mark_deferred_upstream_response(response)),
            ),
            AiAttemptLoopOutcome::Exhausted(exhaustion) => {
                Ok(LocalExecutionRequestOutcome::Exhausted(exhaustion))
            }
            AiAttemptLoopOutcome::NoPath => Ok(LocalExecutionRequestOutcome::NoPath),
        }
    }
    .instrument(span)
    .await
}

pub(crate) async fn execute_sync_attempt_source<T, S>(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    source: S,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
    S: LocalExecutionAttemptSource<T>,
{
    let transfer_tracker = ProviderTransferTracker::for_request(parts);
    execute_sync_attempt_source_with_transfer_tracker(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        source,
        &transfer_tracker,
    )
    .await
}

pub(crate) async fn execute_sync_attempt_source_with_transfer_tracker<T, S>(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    mut source: S,
    transfer_tracker: &ProviderTransferTracker,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
    S: LocalExecutionAttemptSource<T>,
{
    let span = tracing::debug_span!("candidates", trace_id = %trace_id, plan_kind);

    async move {
        tracing::debug!(
            event_name = "candidate_loop_started",
            log_type = "event",
            trace_id = %trace_id,
            plan_kind,
            "dynamic candidate loop started"
        );

        let port = SyncAttemptLoopPort {
            state,
            parts,
            trace_id,
            decision,
            plan_kind,
            transfer_tracker,
        };
        let loop_result = run_dynamic_attempt_loop(
            &port,
            &mut source,
            trace_id,
            plan_kind,
            state
                .frontdoor_runtime_guards
                .local_execution_planning_timeout,
        )
        .await;
        if loop_result.is_err() {
            release_active_plan_usage_policy_cost_best_effort(
                state,
                decision,
                transfer_tracker,
                "dynamic_candidate_loop_error",
            )
            .await;
        }
        loop_result
    }
    .instrument(span)
    .await
}

struct SyncAttemptLoopPort<'a> {
    state: &'a AppState,
    parts: &'a http::request::Parts,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &'a str,
    transfer_tracker: &'a ProviderTransferTracker,
}

#[async_trait]
impl<T> AiAttemptLoopPort<T> for SyncAttemptLoopPort<'_>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
{
    type Response = Response<Body>;
    type Exhaustion = crate::executor::LocalExecutionExhaustion;
    type Error = GatewayError;

    async fn should_skip_attempt(&self, attempt: &T) -> Result<bool, Self::Error> {
        Ok(should_skip_provider_transfer_attempt(
            self.transfer_tracker,
            self.trace_id,
            self.plan_kind,
            attempt,
        )
        .await)
    }

    async fn record_attempt_started(&self, attempt: &T) -> Result<(), Self::Error> {
        record_provider_transfer_attempt_started(self.transfer_tracker, attempt).await;
        Ok(())
    }

    async fn next_same_key_retry(&self, attempt: &T) -> Result<Option<T>, Self::Error> {
        Ok(crate::orchestration::next_same_key_retry_attempt(attempt))
    }

    async fn record_attempt_failed(&self, attempt: &T) -> Result<(), Self::Error> {
        record_provider_transfer_attempt_failed(
            self.state,
            self.transfer_tracker,
            self.trace_id,
            self.plan_kind,
            attempt,
        )
        .await;
        Ok(())
    }

    async fn execute_attempt(
        &self,
        attempt: &T,
    ) -> Result<AiAttemptExecutionOutcome<Self::Response>, Self::Error> {
        let plan = attempt.execution_plan();
        let report_context = attach_plan_usage_reservation_token(
            attempt.report_context(),
            self.transfer_tracker.usage_policy_reservation_token(),
        );
        let balance_response = execution_plan_balance_capacity_response(
            self.state,
            self.trace_id,
            self.decision,
            plan,
            report_context.as_ref(),
        )
        .await;
        let balance_response = match balance_response {
            Ok(response) => response,
            Err(error) => {
                release_plan_usage_policy_cost_best_effort(
                    self.state,
                    self.decision,
                    plan,
                    self.transfer_tracker,
                    "balance_capacity_error",
                )
                .await;
                return Err(error);
            }
        };
        if let Some(response) = balance_response {
            release_plan_usage_policy_cost_best_effort(
                self.state,
                self.decision,
                plan,
                self.transfer_tracker,
                "balance_capacity_rejected",
            )
            .await;
            return Ok(AiAttemptExecutionOutcome::Responded(response));
        }
        prewarm_direct_reqwest_candidate_client(plan);
        let _permit = match acquire_upstream_execution_gate(self.state, self.trace_id).await {
            Ok(permit) => permit,
            Err(error) => {
                release_plan_usage_policy_cost_best_effort(
                    self.state,
                    self.decision,
                    plan,
                    self.transfer_tracker,
                    "upstream_admission_error",
                )
                .await;
                return Err(error);
            }
        };
        if let Some(response) = execution_plan_cost_capacity_response(
            self.state,
            self.trace_id,
            self.decision,
            plan,
            report_context.as_ref(),
            self.transfer_tracker,
        )
        .await?
        {
            return Ok(AiAttemptExecutionOutcome::Responded(response));
        }
        let upstream_execution_gate_held_started_at = std::time::Instant::now();
        let deferred_report_context = report_context.clone();
        let execution = execute_execution_runtime_sync_with_retry_scope(
            self.state,
            self.parts.uri.path(),
            plan.clone(),
            self.trace_id,
            self.decision,
            self.plan_kind,
            attempt.report_kind(),
            report_context,
        )
        .await;
        let execution = match execution {
            Ok(execution) => execution,
            Err(error) => {
                release_plan_usage_policy_cost_best_effort(
                    self.state,
                    self.decision,
                    plan,
                    self.transfer_tracker,
                    "sync_execution_error",
                )
                .await;
                return Err(error);
            }
        };
        observe_gateway_stage_ms(
            "upstream_execution_gate_held",
            upstream_execution_gate_held_started_at
                .elapsed()
                .as_millis() as u64,
        );
        let mut execution = execution;
        match &mut execution {
            AiAttemptExecutionOutcome::Responded(response) => {
                attach_redaction_execution_candidate(response, plan.candidate_id.as_deref());
            }
            AiAttemptExecutionOutcome::Retry {
                fallback_response: Some(response),
                ..
            } => {
                attach_redaction_execution_candidate(response, plan.candidate_id.as_deref());
                attach_deferred_usage_context(response, plan, deferred_report_context.as_ref());
            }
            AiAttemptExecutionOutcome::Retry {
                fallback_response: None,
                ..
            } => {}
        }
        Ok(execution)
    }

    async fn mark_unused_attempts(&self, attempts: Vec<T>) -> Result<(), Self::Error> {
        mark_unused_local_candidates(self.state, attempts).await;
        Ok(())
    }

    async fn build_exhaustion(
        &self,
        last_plan: aether_contracts::ExecutionPlan,
        last_report_context: Option<serde_json::Value>,
    ) -> Result<Self::Exhaustion, Self::Error> {
        warn!(
            event_name = "candidate_loop_exhausted",
            log_type = "ops",
            trace_id = %self.trace_id,
            plan_kind = self.plan_kind,
            request_id = %short_request_id(last_plan.request_id.as_str()),
            candidate_id = ?last_plan.candidate_id,
            provider_name = last_plan.provider_name.as_deref().unwrap_or("-"),
            endpoint_id = %last_plan.endpoint_id,
            key_id = %last_plan.key_id,
            model_name = last_plan.model_name.as_deref().unwrap_or("-"),
            "candidate loop exhausted local sync candidates"
        );
        Ok(build_local_execution_exhaustion(
            self.state,
            &last_plan,
            attach_plan_usage_reservation_token(
                last_report_context,
                self.transfer_tracker.usage_policy_reservation_token(),
            )
            .as_ref(),
        )
        .await)
    }
}

pub(crate) async fn execute_stream_plan_and_reports<T>(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    plan_and_reports: Vec<T>,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
{
    let transfer_tracker = ProviderTransferTracker::default();
    execute_stream_plan_and_reports_with_transfer_tracker(
        state,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
        &transfer_tracker,
    )
    .await
}

pub(crate) async fn execute_stream_plan_and_reports_with_transfer_tracker<T>(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    plan_and_reports: Vec<T>,
    transfer_tracker: &ProviderTransferTracker,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
{
    let candidate_count = plan_and_reports.len();
    let first_provider = plan_and_reports
        .first()
        .and_then(|item| item.execution_plan().provider_name.as_deref())
        .unwrap_or("-")
        .to_string();
    let span = tracing::debug_span!(
        "candidates",
        trace_id = %trace_id,
        plan_kind,
        candidate_count,
    );

    async move {
        tracing::debug!(
            event_name = "candidate_loop_started",
            log_type = "event",
            trace_id = %trace_id,
            plan_kind,
            candidate_count,
            first_provider = first_provider.as_str(),
            "candidate loop started"
        );

        let port = StreamAttemptLoopPort {
            state,
            trace_id,
            decision,
            plan_kind,
            transfer_tracker,
        };
        let loop_result = run_ai_attempt_loop(&port, plan_and_reports).await;
        if loop_result.is_err() {
            release_active_plan_usage_policy_cost_best_effort(
                state,
                decision,
                transfer_tracker,
                "candidate_loop_error",
            )
            .await;
        }
        match loop_result? {
            AiAttemptLoopOutcome::Responded(response) => {
                Ok(LocalExecutionRequestOutcome::responded(response))
            }
            AiAttemptLoopOutcome::Deferred(response) => Ok(
                LocalExecutionRequestOutcome::responded(mark_deferred_upstream_response(response)),
            ),
            AiAttemptLoopOutcome::Exhausted(exhaustion) => {
                Ok(LocalExecutionRequestOutcome::Exhausted(exhaustion))
            }
            AiAttemptLoopOutcome::NoPath => Ok(LocalExecutionRequestOutcome::NoPath),
        }
    }
    .instrument(span)
    .await
}

pub(crate) async fn execute_stream_attempt_source<T, S>(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    source: S,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
    S: LocalExecutionAttemptSource<T>,
{
    let transfer_tracker = ProviderTransferTracker::default();
    execute_stream_attempt_source_with_transfer_tracker(
        state,
        trace_id,
        decision,
        plan_kind,
        source,
        &transfer_tracker,
    )
    .await
}

pub(crate) async fn execute_stream_attempt_source_with_transfer_tracker<T, S>(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    mut source: S,
    transfer_tracker: &ProviderTransferTracker,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
    S: LocalExecutionAttemptSource<T>,
{
    let span = tracing::debug_span!("candidates", trace_id = %trace_id, plan_kind);

    async move {
        tracing::debug!(
            event_name = "candidate_loop_started",
            log_type = "event",
            trace_id = %trace_id,
            plan_kind,
            "dynamic candidate loop started"
        );

        let port = StreamAttemptLoopPort {
            state,
            trace_id,
            decision,
            plan_kind,
            transfer_tracker,
        };
        let loop_result = run_dynamic_attempt_loop(
            &port,
            &mut source,
            trace_id,
            plan_kind,
            state
                .frontdoor_runtime_guards
                .local_execution_planning_timeout,
        )
        .await;
        if loop_result.is_err() {
            release_active_plan_usage_policy_cost_best_effort(
                state,
                decision,
                transfer_tracker,
                "dynamic_candidate_loop_error",
            )
            .await;
        }
        loop_result
    }
    .instrument(span)
    .await
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ProviderTransferLimits {
    max_transfer_count: u64,
    max_transfer_timeout_seconds: u64,
}

impl From<&LocalFailoverPolicy> for ProviderTransferLimits {
    fn from(policy: &LocalFailoverPolicy) -> Self {
        Self {
            max_transfer_count: policy.max_transfer_count,
            max_transfer_timeout_seconds: policy.max_transfer_timeout_seconds,
        }
    }
}

#[derive(Debug)]
struct ProviderTransferState {
    first_attempt_started_at: Instant,
    last_key_id: String,
    transfer_count: u64,
    limits: Option<ProviderTransferLimits>,
}

#[derive(Debug, Default)]
struct ProviderTransferStateTracker {
    by_provider: BTreeMap<String, ProviderTransferState>,
    exhausted_provider_ids: BTreeSet<String>,
    global: GlobalTransferState,
}

#[derive(Debug, Default)]
struct GlobalTransferState {
    first_attempt_started_at: Option<Instant>,
    last_candidate: Option<(String, String, String)>,
    transfer_count: u64,
    limits: Option<ProviderTransferLimits>,
    exhausted: bool,
}

impl GlobalTransferState {
    fn load_policy(&mut self, report_context: Option<&serde_json::Value>) {
        if self.limits.is_none() {
            if let Some(policy) =
                crate::orchestration::routing_execution_policy_from_report_context(report_context)
            {
                self.limits = Some(ProviderTransferLimits {
                    max_transfer_count: policy.max_transfer_count,
                    max_transfer_timeout_seconds: policy.max_transfer_timeout_seconds,
                });
            }
        }
    }

    fn changes_candidate(&self, plan: &aether_contracts::ExecutionPlan) -> bool {
        self.last_candidate
            .as_ref()
            .is_some_and(|(provider, endpoint, key)| {
                provider != &plan.provider_id
                    || endpoint != &plan.endpoint_id
                    || key != &plan.key_id
            })
    }

    fn record_attempt_started(&mut self, plan: &aether_contracts::ExecutionPlan, now: Instant) {
        self.first_attempt_started_at.get_or_insert(now);
        if self.changes_candidate(plan) {
            self.transfer_count = self.transfer_count.saturating_add(1);
        }
        self.last_candidate = Some((
            plan.provider_id.clone(),
            plan.endpoint_id.clone(),
            plan.key_id.clone(),
        ));
    }

    fn check_before_attempt(
        &mut self,
        plan: &aether_contracts::ExecutionPlan,
        now: Instant,
    ) -> Option<(bool, bool)> {
        let limits = self.limits?;
        let started_at = self.first_attempt_started_at?;
        let count_reached = self.changes_candidate(plan)
            && limits.max_transfer_count > 0
            && self.transfer_count >= limits.max_transfer_count;
        let timeout_reached = limits.max_transfer_timeout_seconds > 0
            && now.saturating_duration_since(started_at)
                >= Duration::from_secs(limits.max_transfer_timeout_seconds);
        if !count_reached && !timeout_reached {
            return None;
        }
        self.exhausted = true;
        Some((count_reached, timeout_reached))
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ProviderTransferTracker {
    state: std::sync::Arc<tokio::sync::Mutex<ProviderTransferStateTracker>>,
    usage_policy_reservation: Option<crate::plan_usage_policy::PlanUsageReservationContext>,
    usage_policy_reservation_plan:
        std::sync::Arc<std::sync::Mutex<Option<aether_contracts::ExecutionPlan>>>,
    usage_policy_cost_reserved: std::sync::Arc<AtomicBool>,
    _background_admission_permit: Option<BackgroundAdmissionPermit>,
}

impl ProviderTransferTracker {
    pub(crate) fn for_request(parts: &http::request::Parts) -> Self {
        let background_admission_permit =
            parts.extensions.get::<BackgroundAdmissionPermit>().cloned();
        let usage_policy_reservation = parts
            .extensions
            .get::<crate::plan_usage_policy::PlanUsageReservationContext>()
            .cloned();
        Self {
            state: Default::default(),
            usage_policy_reservation,
            usage_policy_reservation_plan: Default::default(),
            usage_policy_cost_reserved: Default::default(),
            _background_admission_permit: background_admission_permit,
        }
    }

    fn usage_policy_reservation_token(&self) -> Option<&str> {
        self.usage_policy_reservation
            .as_ref()
            .map(crate::plan_usage_policy::PlanUsageReservationContext::token)
    }

    fn record_usage_policy_reservation_plan(&self, plan: &aether_contracts::ExecutionPlan) {
        *self
            .usage_policy_reservation_plan
            .lock()
            .expect("usage policy reservation plan lock") = Some(plan.clone());
    }

    fn usage_policy_reservation_plan(&self) -> Option<aether_contracts::ExecutionPlan> {
        self.usage_policy_reservation_plan
            .lock()
            .expect("usage policy reservation plan lock")
            .clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderTransferLimitReached {
    provider_id: String,
    transfer_count: u64,
    elapsed_ms: u64,
    limits: ProviderTransferLimits,
    count_reached: bool,
    timeout_reached: bool,
}

impl ProviderTransferStateTracker {
    fn record_attempt_started(&mut self, plan: &aether_contracts::ExecutionPlan, now: Instant) {
        self.global.record_attempt_started(plan, now);
        match self.by_provider.entry(plan.provider_id.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(ProviderTransferState {
                    first_attempt_started_at: now,
                    last_key_id: plan.key_id.clone(),
                    transfer_count: 0,
                    limits: None,
                });
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let state = entry.get_mut();
                if state.last_key_id != plan.key_id {
                    state.transfer_count = state.transfer_count.saturating_add(1);
                    state.last_key_id.clone_from(&plan.key_id);
                }
            }
        }
    }

    fn needs_limits(&self, provider_id: &str) -> bool {
        self.by_provider
            .get(provider_id)
            .is_some_and(|state| state.limits.is_none())
    }

    fn set_limits(&mut self, provider_id: &str, limits: ProviderTransferLimits) {
        if let Some(state) = self.by_provider.get_mut(provider_id) {
            state.limits = Some(limits);
        }
    }

    fn check_before_attempt(
        &mut self,
        plan: &aether_contracts::ExecutionPlan,
        now: Instant,
    ) -> Option<ProviderTransferLimitReached> {
        if self.exhausted_provider_ids.contains(&plan.provider_id) {
            return self.reached_snapshot(plan.provider_id.as_str(), now, false, false);
        }

        let state = self.by_provider.get(&plan.provider_id)?;
        let limits = state.limits?;
        let elapsed = now.saturating_duration_since(state.first_attempt_started_at);
        let timeout_reached = limits.max_transfer_timeout_seconds > 0
            && elapsed >= Duration::from_secs(limits.max_transfer_timeout_seconds);
        let count_reached = state.last_key_id != plan.key_id
            && limits.max_transfer_count > 0
            && state.transfer_count >= limits.max_transfer_count;
        if !count_reached && !timeout_reached {
            return None;
        }

        let reached = self.reached_snapshot(
            plan.provider_id.as_str(),
            now,
            count_reached,
            timeout_reached,
        )?;
        self.exhausted_provider_ids.insert(plan.provider_id.clone());
        Some(reached)
    }

    fn check_timeout_after_failure(
        &mut self,
        provider_id: &str,
        now: Instant,
    ) -> Option<ProviderTransferLimitReached> {
        if self.exhausted_provider_ids.contains(provider_id) {
            return None;
        }
        let state = self.by_provider.get(provider_id)?;
        let limits = state.limits?;
        let elapsed = now.saturating_duration_since(state.first_attempt_started_at);
        let timeout_reached = limits.max_transfer_timeout_seconds > 0
            && elapsed >= Duration::from_secs(limits.max_transfer_timeout_seconds);
        if !timeout_reached {
            return None;
        }

        let reached = self.reached_snapshot(provider_id, now, false, true)?;
        self.exhausted_provider_ids.insert(provider_id.to_string());
        Some(reached)
    }

    fn reached_snapshot(
        &self,
        provider_id: &str,
        now: Instant,
        count_reached: bool,
        timeout_reached: bool,
    ) -> Option<ProviderTransferLimitReached> {
        let state = self.by_provider.get(provider_id)?;
        let limits = state.limits?;
        let elapsed = now.saturating_duration_since(state.first_attempt_started_at);
        Some(ProviderTransferLimitReached {
            provider_id: provider_id.to_string(),
            transfer_count: state.transfer_count,
            elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
            limits,
            count_reached,
            timeout_reached,
        })
    }
}

async fn load_provider_transfer_limits<Attempt>(
    state: &AppState,
    tracker: &mut ProviderTransferStateTracker,
    attempt: &Attempt,
) where
    Attempt: AiExecutionAttempt + Send + Sync + 'static,
{
    let plan = attempt.execution_plan();
    if !tracker.needs_limits(plan.provider_id.as_str()) {
        return;
    }
    let owned_report_context = if attempt.report_context_ref().is_none() {
        attempt.report_context()
    } else {
        None
    };
    let report_context = attempt
        .report_context_ref()
        .or(owned_report_context.as_ref());
    let embedded_policy_has_transfer_limits = report_context
        .and_then(serde_json::Value::as_object)
        .and_then(|object| object.get("local_failover_policy"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|policy| {
            policy.contains_key("max_transfer_count")
                || policy.contains_key("max_transfer_timeout_seconds")
        });
    let policy = if embedded_policy_has_transfer_limits {
        local_failover_policy_from_report_context(report_context).unwrap_or_default()
    } else {
        resolve_local_failover_policy(state, plan, report_context).await
    };
    tracker.set_limits(
        plan.provider_id.as_str(),
        ProviderTransferLimits::from(&policy),
    );
}

async fn provider_transfer_timeout_after_failure<Attempt>(
    state: &AppState,
    tracker: &mut ProviderTransferStateTracker,
    attempt: &Attempt,
) -> Option<ProviderTransferLimitReached>
where
    Attempt: AiExecutionAttempt + Send + Sync + 'static,
{
    let plan = attempt.execution_plan();
    load_provider_transfer_limits(state, tracker, attempt).await;
    tracker.check_timeout_after_failure(plan.provider_id.as_str(), Instant::now())
}

fn log_provider_transfer_limit_reached(
    trace_id: &str,
    plan_kind: &str,
    reached: &ProviderTransferLimitReached,
) {
    warn!(
        event_name = "provider_transfer_limit_reached",
        log_type = "event",
        trace_id,
        plan_kind,
        provider_id = %reached.provider_id,
        transfer_count = reached.transfer_count,
        elapsed_ms = reached.elapsed_ms,
        max_transfer_count = reached.limits.max_transfer_count,
        max_transfer_timeout_seconds = reached.limits.max_transfer_timeout_seconds,
        count_reached = reached.count_reached,
        timeout_reached = reached.timeout_reached,
        "gateway exhausted the provider transfer budget and will skip its remaining candidates"
    );
}

async fn should_skip_provider_transfer_attempt<Attempt>(
    tracker: &ProviderTransferTracker,
    trace_id: &str,
    plan_kind: &str,
    attempt: &Attempt,
) -> bool
where
    Attempt: AiExecutionAttempt + Send + Sync + 'static,
{
    let owned_report_context = attempt
        .report_context_ref()
        .is_none()
        .then(|| attempt.report_context())
        .flatten();
    let report_context = attempt
        .report_context_ref()
        .or(owned_report_context.as_ref());
    let mut tracker = tracker.state.lock().await;
    tracker.global.load_policy(report_context);
    if tracker.global.exhausted {
        return true;
    }
    let now = Instant::now();
    if let Some((count_reached, timeout_reached)) = tracker
        .global
        .check_before_attempt(attempt.execution_plan(), now)
    {
        warn!(
            event_name = "routing_transfer_limit_reached",
            log_type = "event",
            trace_id,
            plan_kind,
            transfer_count = tracker.global.transfer_count,
            elapsed_ms = tracker
                .global
                .first_attempt_started_at
                .map(|started| now.saturating_duration_since(started).as_millis() as u64)
                .unwrap_or(0),
            count_reached,
            timeout_reached,
            "gateway exhausted the routing strategy transfer budget"
        );
        return true;
    }
    let reached = tracker.check_before_attempt(attempt.execution_plan(), now);
    let Some(reached) = reached else {
        return false;
    };
    if reached.count_reached || reached.timeout_reached {
        log_provider_transfer_limit_reached(trace_id, plan_kind, &reached);
    }
    true
}

async fn record_provider_transfer_attempt_started<Attempt>(
    tracker: &ProviderTransferTracker,
    attempt: &Attempt,
) where
    Attempt: AiExecutionAttempt + Send + Sync + 'static,
{
    tracker
        .state
        .lock()
        .await
        .record_attempt_started(attempt.execution_plan(), Instant::now());
}

async fn record_provider_transfer_attempt_failed<Attempt>(
    state: &AppState,
    tracker: &ProviderTransferTracker,
    trace_id: &str,
    plan_kind: &str,
    attempt: &Attempt,
) where
    Attempt: AiExecutionAttempt + Send + Sync + 'static,
{
    let mut tracker = tracker.state.lock().await;
    let reached = provider_transfer_timeout_after_failure(state, &mut tracker, attempt).await;
    if let Some(reached) = reached {
        log_provider_transfer_limit_reached(trace_id, plan_kind, &reached);
    }
}

async fn run_dynamic_attempt_loop<Port, Source, Attempt>(
    port: &Port,
    source: &mut Source,
    trace_id: &str,
    plan_kind: &str,
    planning_timeout: Duration,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    Port: AiAttemptLoopPort<
        Attempt,
        Response = Response<Body>,
        Exhaustion = crate::executor::LocalExecutionExhaustion,
        Error = GatewayError,
    >,
    Source: LocalExecutionAttemptSource<Attempt>,
    Attempt: AiExecutionAttempt + Send + Sync + 'static,
{
    let mut last_attempted = None;
    let mut fallback_response = None;
    // A same-key retry derived after a candidate-scoped failure runs before
    // the source is asked for the next candidate.
    let mut pending_same_key_retry: Option<Attempt> = None;

    loop {
        let attempt = match pending_same_key_retry.take() {
            Some(attempt) => attempt,
            None => {
                let next_started_at = std::time::Instant::now();
                let next_attempt = next_execution_attempt_with_timeout(
                    source,
                    trace_id,
                    plan_kind,
                    planning_timeout,
                )
                .await?;
                observe_gateway_stage_ms(
                    "stream_candidate_next",
                    next_started_at.elapsed().as_millis() as u64,
                );
                let Some(attempt) = next_attempt else {
                    break;
                };
                attempt
            }
        };
        if port.should_skip_attempt(&attempt).await? {
            let provider_id = attempt.execution_plan().provider_id.clone();
            port.mark_unused_attempts(vec![attempt]).await?;
            source.skip_provider(provider_id.as_str()).await?;
            continue;
        }
        port.record_attempt_started(&attempt).await?;
        let execute_started_at = std::time::Instant::now();
        let execution = match port.execute_attempt(&attempt).await {
            Ok(execution) => execution,
            Err(err) => {
                let remaining = source.drain_execution_attempts().await?;
                port.mark_unused_attempts(remaining).await?;
                return Err(err);
            }
        };
        observe_gateway_stage_ms(
            "stream_candidate_execute",
            execute_started_at.elapsed().as_millis() as u64,
        );
        match execution {
            AiAttemptExecutionOutcome::Responded(response) => {
                let remaining = source.drain_execution_attempts().await?;
                let unused_started_at = std::time::Instant::now();
                port.mark_unused_attempts(remaining).await?;
                observe_gateway_stage_ms(
                    "stream_candidate_unused",
                    unused_started_at.elapsed().as_millis() as u64,
                );
                return Ok(LocalExecutionRequestOutcome::responded(response));
            }
            AiAttemptExecutionOutcome::Retry {
                scope,
                fallback_response: attempt_fallback_response,
            } => {
                if attempt_fallback_response.is_some() {
                    fallback_response = attempt_fallback_response;
                }
                if scope == AiAttemptRetryScope::Candidate {
                    pending_same_key_retry = port.next_same_key_retry(&attempt).await?;
                }
                apply_attempt_retry_scope(source, &attempt, scope).await?;
            }
        }

        port.record_attempt_failed(&attempt).await?;
        if port.should_skip_attempt(&attempt).await? {
            source
                .skip_provider(attempt.execution_plan().provider_id.as_str())
                .await?;
        }

        // Only retain a deep plan/context snapshot when this candidate really
        // failed and exhaustion reporting will need it.
        last_attempted = Some((attempt.execution_plan().clone(), attempt.report_context()));
    }

    if let Some(response) = fallback_response {
        return Ok(LocalExecutionRequestOutcome::responded(
            mark_deferred_upstream_response(response),
        ));
    }

    let Some((last_plan, last_report_context)) = last_attempted else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    Ok(LocalExecutionRequestOutcome::Exhausted(
        port.build_exhaustion(last_plan, last_report_context)
            .await?,
    ))
}

async fn apply_attempt_retry_scope<Source, Attempt>(
    source: &mut Source,
    attempt: &Attempt,
    scope: AiAttemptRetryScope,
) -> Result<(), GatewayError>
where
    Source: LocalExecutionAttemptSource<Attempt>,
    Attempt: AiExecutionAttempt,
{
    let plan = attempt.execution_plan();
    match scope {
        AiAttemptRetryScope::Candidate => Ok(()),
        AiAttemptRetryScope::Credential => source.skip_credential(plan.key_id.as_str()).await,
        AiAttemptRetryScope::Endpoint => source.skip_endpoint(plan.endpoint_id.as_str()).await,
        AiAttemptRetryScope::Provider => source.skip_provider(plan.provider_id.as_str()).await,
    }
}

async fn next_execution_attempt_with_timeout<Source, Attempt>(
    source: &mut Source,
    trace_id: &str,
    plan_kind: &str,
    planning_timeout: Duration,
) -> Result<Option<Attempt>, GatewayError>
where
    Source: LocalExecutionAttemptSource<Attempt>,
{
    match timeout(planning_timeout, source.next_execution_attempt()).await {
        Ok(result) => result,
        Err(_) => {
            let timeout_ms = planning_timeout.as_millis() as u64;
            warn!(
                event_name = "local_execution_candidate_planning_timeout",
                log_type = "ops",
                trace_id,
                plan_kind,
                timeout_ms,
                phase = "next_execution_attempt",
                "gateway timed out while planning the next local execution candidate"
            );
            Err(GatewayError::LocalExecutionPlanningTimeout {
                trace_id: trace_id.to_string(),
                phase: "next_execution_attempt",
                timeout_ms,
            })
        }
    }
}

struct StreamAttemptLoopPort<'a> {
    state: &'a AppState,
    trace_id: &'a str,
    decision: &'a GatewayControlDecision,
    plan_kind: &'a str,
    transfer_tracker: &'a ProviderTransferTracker,
}

#[async_trait]
impl<T> AiAttemptLoopPort<T> for StreamAttemptLoopPort<'_>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
{
    type Response = Response<Body>;
    type Exhaustion = crate::executor::LocalExecutionExhaustion;
    type Error = GatewayError;

    async fn should_skip_attempt(&self, attempt: &T) -> Result<bool, Self::Error> {
        Ok(should_skip_provider_transfer_attempt(
            self.transfer_tracker,
            self.trace_id,
            self.plan_kind,
            attempt,
        )
        .await)
    }

    async fn record_attempt_started(&self, attempt: &T) -> Result<(), Self::Error> {
        record_provider_transfer_attempt_started(self.transfer_tracker, attempt).await;
        Ok(())
    }

    async fn next_same_key_retry(&self, attempt: &T) -> Result<Option<T>, Self::Error> {
        Ok(crate::orchestration::next_same_key_retry_attempt(attempt))
    }

    async fn record_attempt_failed(&self, attempt: &T) -> Result<(), Self::Error> {
        record_provider_transfer_attempt_failed(
            self.state,
            self.transfer_tracker,
            self.trace_id,
            self.plan_kind,
            attempt,
        )
        .await;
        Ok(())
    }

    async fn execute_attempt(
        &self,
        attempt: &T,
    ) -> Result<AiAttemptExecutionOutcome<Self::Response>, Self::Error> {
        let plan = attempt.execution_plan();
        let report_context = attach_plan_usage_reservation_token(
            attempt.report_context(),
            self.transfer_tracker.usage_policy_reservation_token(),
        );
        let candidate_index = parse_request_candidate_report_context(report_context.as_ref())
            .and_then(|context| context.candidate_index)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        debug!(
            event_name = "candidate_loop_attempt_started",
            log_type = "debug",
            trace_id = %self.trace_id,
            plan_kind = self.plan_kind,
            request_id = %short_request_id(plan.request_id.as_str()),
            candidate_id = ?plan.candidate_id,
            provider_name = plan.provider_name.as_deref().unwrap_or("-"),
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            model_name = plan.model_name.as_deref().unwrap_or("-"),
            candidate_index = candidate_index.as_str(),
            "candidate loop attempting stream execution candidate"
        );
        let balance_response = execution_plan_balance_capacity_response(
            self.state,
            self.trace_id,
            self.decision,
            plan,
            report_context.as_ref(),
        )
        .await;
        let balance_response = match balance_response {
            Ok(response) => response,
            Err(error) => {
                release_plan_usage_policy_cost_best_effort(
                    self.state,
                    self.decision,
                    plan,
                    self.transfer_tracker,
                    "balance_capacity_error",
                )
                .await;
                return Err(error);
            }
        };
        if let Some(response) = balance_response {
            release_plan_usage_policy_cost_best_effort(
                self.state,
                self.decision,
                plan,
                self.transfer_tracker,
                "balance_capacity_rejected",
            )
            .await;
            return Ok(AiAttemptExecutionOutcome::Responded(response));
        }
        prewarm_direct_reqwest_candidate_client(plan);
        let watchdog_report_context_owned = report_context.clone();
        let watchdog_report_context = watchdog_report_context_owned.as_ref();
        let execution_state = self.state.clone();
        let execution_trace_id = self.trace_id.to_string();
        let execution_plan_kind = self.plan_kind.to_string();
        let execution_decision = self.decision.clone();
        let execution_report_kind = attempt.report_kind();
        let execution_plan = plan.clone();
        let execution_transfer_tracker = self.transfer_tracker.clone();
        let stop_on_transport_errors = matches!(
            resolve_local_transport_failover_analysis_for_attempt(
                self.state,
                plan,
                watchdog_report_context,
            )
            .await
            .decision,
            LocalFailoverDecision::StopLocalFailover
        );
        let watchdog_started_at = std::time::Instant::now();
        let execution = execute_stream_candidate_with_watchdog(
            self.state,
            self.trace_id,
            self.plan_kind,
            plan,
            watchdog_report_context,
            stop_on_transport_errors,
            move || async move {
                if let Some(response) = execution_plan_cost_capacity_response(
                    &execution_state,
                    execution_trace_id.as_str(),
                    &execution_decision,
                    &execution_plan,
                    report_context.as_ref(),
                    &execution_transfer_tracker,
                )
                .await?
                {
                    return Ok(AiAttemptExecutionOutcome::Responded(response));
                }
                execute_execution_runtime_stream_with_retry_scope(
                    &execution_state,
                    execution_plan,
                    execution_trace_id.as_str(),
                    &execution_decision,
                    execution_plan_kind.as_str(),
                    execution_report_kind,
                    report_context,
                )
                .await
            },
        )
        .await;
        let execution = match execution {
            Ok(execution) => execution,
            Err(error) => {
                release_plan_usage_policy_cost_best_effort(
                    self.state,
                    self.decision,
                    plan,
                    self.transfer_tracker,
                    "stream_execution_error",
                )
                .await;
                return Err(error);
            }
        };
        let mut execution = match execution {
            StreamCandidateWatchdogOutcome::TransportTimeout => {
                AiAttemptExecutionOutcome::Responded(
                    build_transport_error_stop_response(
                        self.state,
                        plan,
                        watchdog_report_context,
                        self.trace_id,
                        self.decision,
                        http::StatusCode::GATEWAY_TIMEOUT.as_u16(),
                        "local_stream_candidate_watchdog_timeout",
                        stream_candidate_watchdog_timeout_message(),
                        watchdog_started_at.elapsed().as_millis() as u64,
                    )
                    .await?,
                )
            }
            StreamCandidateWatchdogOutcome::Executed(execution) => execution,
        };
        match &mut execution {
            AiAttemptExecutionOutcome::Responded(response) => {
                attach_redaction_execution_candidate(response, plan.candidate_id.as_deref());
            }
            AiAttemptExecutionOutcome::Retry {
                fallback_response: Some(response),
                ..
            } => {
                attach_redaction_execution_candidate(response, plan.candidate_id.as_deref());
                attach_deferred_usage_context(response, plan, watchdog_report_context);
            }
            AiAttemptExecutionOutcome::Retry {
                fallback_response: None,
                ..
            } => {}
        }
        Ok(execution)
    }

    async fn mark_unused_attempts(&self, attempts: Vec<T>) -> Result<(), Self::Error> {
        mark_unused_local_candidates(self.state, attempts).await;
        Ok(())
    }

    async fn build_exhaustion(
        &self,
        last_plan: aether_contracts::ExecutionPlan,
        last_report_context: Option<serde_json::Value>,
    ) -> Result<Self::Exhaustion, Self::Error> {
        warn!(
            event_name = "candidate_loop_exhausted",
            log_type = "ops",
            trace_id = %self.trace_id,
            plan_kind = self.plan_kind,
            request_id = %short_request_id(last_plan.request_id.as_str()),
            candidate_id = ?last_plan.candidate_id,
            provider_name = last_plan.provider_name.as_deref().unwrap_or("-"),
            endpoint_id = %last_plan.endpoint_id,
            key_id = %last_plan.key_id,
            model_name = last_plan.model_name.as_deref().unwrap_or("-"),
            "candidate loop exhausted local stream candidates"
        );
        Ok(build_local_execution_exhaustion(
            self.state,
            &last_plan,
            attach_plan_usage_reservation_token(
                last_report_context,
                self.transfer_tracker.usage_policy_reservation_token(),
            )
            .as_ref(),
        )
        .await)
    }
}

fn prewarm_direct_reqwest_candidate_client(plan: &aether_contracts::ExecutionPlan) {
    let started_at = std::time::Instant::now();
    crate::execution_runtime::transport::prewarm_direct_reqwest_client_cache_for_plan(plan);
    observe_gateway_stage_ms(
        "direct_reqwest_client_prewarm",
        started_at.elapsed().as_millis() as u64,
    );
}

fn attach_plan_usage_reservation_token(
    report_context: Option<serde_json::Value>,
    reservation_token: Option<&str>,
) -> Option<serde_json::Value> {
    match (report_context, reservation_token) {
        (Some(serde_json::Value::Object(mut context)), Some(reservation_token)) => {
            context.insert(
                "plan_usage_reservation_token".to_string(),
                serde_json::Value::String(reservation_token.to_string()),
            );
            context.insert(
                aether_data_contracts::repository::usage::PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY
                    .to_string(),
                serde_json::Value::Bool(false),
            );
            Some(serde_json::Value::Object(context))
        }
        (Some(serde_json::Value::Object(mut context)), None) => {
            context.remove("plan_usage_reservation_token");
            context.remove(
                aether_data_contracts::repository::usage::PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY,
            );
            Some(serde_json::Value::Object(context))
        }
        (_, Some(reservation_token)) => {
            let mut context = serde_json::Map::new();
            context.insert(
                "plan_usage_reservation_token".to_string(),
                serde_json::Value::String(reservation_token.to_string()),
            );
            context.insert(
                aether_data_contracts::repository::usage::PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY
                    .to_string(),
                serde_json::Value::Bool(false),
            );
            Some(serde_json::Value::Object(context))
        }
        (report_context, None) => report_context,
    }
}

async fn execution_plan_balance_capacity_response(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let rejection = match crate::control::execution_plan_balance_capacity_rejection(
        state,
        decision,
        plan,
        report_context,
    )
    .await
    {
        Ok(rejection) => rejection,
        Err(err) => {
            mark_unused_local_candidate(state, plan, report_context).await;
            return Err(err);
        }
    };
    let Some(rejection) = rejection else {
        return Ok(None);
    };
    mark_unused_local_candidate(state, plan, report_context).await;
    let mut response = crate::api::response::build_local_auth_rejection_response(
        trace_id,
        Some(decision),
        &rejection,
    )?;
    attach_redaction_execution_candidate(&mut response, plan.candidate_id.as_deref());
    Ok(Some(response))
}

async fn execution_plan_cost_capacity_response(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    transfer_tracker: &ProviderTransferTracker,
) -> Result<Option<Response<Body>>, GatewayError> {
    let outcome = match crate::plan_usage_policy::reserve_admitted_http_plan_usage_policy_cost(
        state,
        decision,
        plan,
        report_context,
        transfer_tracker.usage_policy_reservation.as_ref(),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            release_plan_usage_policy_cost_best_effort(
                state,
                decision,
                plan,
                transfer_tracker,
                "reservation_error",
            )
            .await;
            mark_unused_local_candidate(state, plan, report_context).await;
            return Err(err);
        }
    };
    let rejection = match outcome {
        crate::plan_usage_policy::PlanUsageCostReservationOutcome::NotRequired => return Ok(None),
        crate::plan_usage_policy::PlanUsageCostReservationOutcome::Reserved => {
            transfer_tracker.record_usage_policy_reservation_plan(plan);
            transfer_tracker
                .usage_policy_cost_reserved
                .store(true, Ordering::Release);
            return Ok(None);
        }
        crate::plan_usage_policy::PlanUsageCostReservationOutcome::Rejected(rejection) => rejection,
    };
    release_plan_usage_policy_cost_best_effort(
        state,
        decision,
        plan,
        transfer_tracker,
        "reservation_rejected",
    )
    .await;
    mark_unused_local_candidate(state, plan, report_context).await;
    let mut response = crate::api::response::build_local_plan_usage_limited_response(
        trace_id,
        Some(decision),
        &rejection,
    )?;
    attach_redaction_execution_candidate(&mut response, plan.candidate_id.as_deref());
    Ok(Some(response))
}

async fn release_plan_usage_policy_cost_best_effort(
    state: &AppState,
    decision: &GatewayControlDecision,
    plan: &aether_contracts::ExecutionPlan,
    transfer_tracker: &ProviderTransferTracker,
    reason: &'static str,
) {
    if !transfer_tracker
        .usage_policy_cost_reserved
        .swap(false, Ordering::AcqRel)
    {
        return;
    }
    if let Err(error) = crate::plan_usage_policy::release_plan_usage_policy_cost(
        state,
        decision,
        plan,
        transfer_tracker
            .usage_policy_reservation_token()
            .expect("a reserved plan cost always has a reservation token"),
        current_unix_ms() / 1_000,
    )
    .await
    {
        warn!(
            event_name = "plan_usage_cost_reservation_release_failed",
            log_type = "ops",
            request_id = %plan.request_id,
            candidate_id = ?plan.candidate_id,
            reservation_token = transfer_tracker
                .usage_policy_reservation_token()
                .unwrap_or("-"),
            reason,
            error = ?error,
            "gateway failed to release plan usage cost reservation after terminal capacity failure"
        );
        transfer_tracker
            .usage_policy_cost_reserved
            .store(true, Ordering::Release);
    }
}

async fn release_active_plan_usage_policy_cost_best_effort(
    state: &AppState,
    decision: &GatewayControlDecision,
    transfer_tracker: &ProviderTransferTracker,
    reason: &'static str,
) {
    let Some(plan) = transfer_tracker.usage_policy_reservation_plan() else {
        return;
    };
    release_plan_usage_policy_cost_best_effort(state, decision, &plan, transfer_tracker, reason)
        .await;
}

pub(crate) async fn mark_unused_local_candidates<T>(state: &AppState, remaining: Vec<T>)
where
    T: AiExecutionAttempt,
{
    for plan_and_report in remaining {
        let report_context = plan_and_report.report_context();
        mark_unused_local_candidate(
            state,
            plan_and_report.execution_plan(),
            report_context.as_ref(),
        )
        .await;
    }
}

async fn mark_unused_local_candidate(
    state: &AppState,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) {
    let metadata = local_execution_candidate_metadata_from_report_context(report_context);
    if let Some(lease) = metadata.pool_key_lease.as_ref() {
        if let Err(err) =
            release_admin_provider_pool_key_lease(state.runtime_state.as_ref(), lease).await
        {
            warn!(
                error = ?err,
                "gateway candidate loop: failed to release unused pool key lease"
            );
        }
    }
    if should_skip_unused_persistence_from_metadata(&metadata) {
        return;
    }
    record_local_request_candidate_status(
        state,
        plan,
        report_context,
        SchedulerRequestCandidateStatusUpdate {
            status: RequestCandidateStatus::Unused,
            status_code: None,
            error_type: None,
            error_message: None,
            latency_ms: None,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
        },
    )
    .await;
}

fn should_skip_unused_persistence(report_context: Option<&serde_json::Value>) -> bool {
    let metadata = local_execution_candidate_metadata_from_report_context(report_context);
    should_skip_unused_persistence_from_metadata(&metadata)
}

fn should_skip_unused_persistence_from_metadata(
    metadata: &crate::orchestration::LocalExecutionCandidateMetadata,
) -> bool {
    metadata.candidate_group_id.is_some() && metadata.pool_key_index.is_some()
}

fn resolve_stream_candidate_watchdog_timeout(
    plan: &aether_contracts::ExecutionPlan,
    _report_context: Option<&serde_json::Value>,
) -> Duration {
    let timeout_ms = plan
        .timeouts
        .as_ref()
        .and_then(|timeouts| timeouts.first_byte_ms)
        .unwrap_or(DEFAULT_STREAM_FIRST_BYTE_WATCHDOG_TIMEOUT_MS)
        .max(1);
    Duration::from_millis(timeout_ms)
}

fn stream_candidate_watchdog_timeout_message() -> &'static str {
    "Stream first byte timeout"
}

fn admission_timeout_gate(error: &GatewayError) -> Option<&'static str> {
    match error {
        GatewayError::AdmissionTimeout { gate, .. } => Some(*gate),
        _ => None,
    }
}

fn admission_timeout_message(error: &GatewayError) -> String {
    match error {
        GatewayError::AdmissionTimeout {
            gate,
            queue_budget_ms,
            ..
        } => {
            format!("gateway admission gate {gate} timed out after {queue_budget_ms}ms")
        }
        other => format!("{other:?}"),
    }
}

fn is_candidate_level_admission_timeout(error: &GatewayError) -> bool {
    matches!(
        admission_timeout_gate(error),
        Some(UPSTREAM_EXECUTION_GATE_NAME | UPSTREAM_TARGET_GATE_NAME)
    )
}

fn should_record_candidate_admission_timeout(error: &GatewayError) -> bool {
    matches!(
        admission_timeout_gate(error),
        Some(UPSTREAM_EXECUTION_GATE_NAME)
    )
}

async fn record_stream_candidate_admission_timeout(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    candidate_started_unix_ms: u64,
    error: &GatewayError,
) {
    let terminal_unix_ms = current_unix_ms();
    record_local_request_candidate_status(
        state,
        plan,
        report_context,
        SchedulerRequestCandidateStatusUpdate {
            status: RequestCandidateStatus::Failed,
            status_code: Some(http::StatusCode::TOO_MANY_REQUESTS.as_u16()),
            error_type: Some("gateway_admission_timeout".to_string()),
            error_message: Some(admission_timeout_message(error)),
            latency_ms: Some(terminal_unix_ms.saturating_sub(candidate_started_unix_ms)),
            started_at_unix_ms: Some(candidate_started_unix_ms),
            finished_at_unix_ms: Some(terminal_unix_ms),
        },
    )
    .await;
}

fn log_stream_candidate_admission_timeout(
    trace_id: &str,
    plan_kind: &str,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    error: &GatewayError,
) {
    let provider_name = plan.provider_name.as_deref().unwrap_or("-");
    let model_name = plan.model_name.as_deref().unwrap_or("-");
    let candidate_index = parse_request_candidate_report_context(report_context)
        .and_then(|context| context.candidate_index)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let (gate, queue_budget_ms) = match error {
        GatewayError::AdmissionTimeout {
            gate,
            queue_budget_ms,
            ..
        } => (*gate, *queue_budget_ms),
        _ => ("-", 0),
    };
    warn!(
        event_name = "local_stream_candidate_admission_timeout",
        log_type = "event",
        trace_id = %trace_id,
        plan_kind,
        request_id = %short_request_id(plan.request_id.as_str()),
        candidate_id = ?plan.candidate_id,
        provider_name,
        endpoint_id = %plan.endpoint_id,
        key_id = %plan.key_id,
        model_name,
        candidate_index = candidate_index.as_str(),
        gate,
        queue_budget_ms,
        "gateway local stream candidate admission timed out; retrying next candidate"
    );
}

#[derive(Debug)]
enum StreamCandidateWatchdogOutcome {
    Executed(AiAttemptExecutionOutcome<Response<Body>>),
    TransportTimeout,
}

async fn execute_stream_candidate_with_watchdog<Fut>(
    state: &(impl RequestCandidateRuntimeWriter + UpstreamExecutionGateProvider + ?Sized),
    trace_id: &str,
    plan_kind: &str,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    stop_on_transport_errors: bool,
    execute: impl FnOnce() -> Fut,
) -> Result<StreamCandidateWatchdogOutcome, GatewayError>
where
    Fut: std::future::Future<
            Output = Result<AiAttemptExecutionOutcome<Response<Body>>, GatewayError>,
        > + Send,
{
    let timeout_duration = resolve_stream_candidate_watchdog_timeout(plan, report_context);
    let candidate_started_at = std::time::Instant::now();
    let candidate_started_unix_ms = current_unix_ms();
    let permit = match acquire_upstream_execution_gate(state, trace_id).await {
        Ok(permit) => permit,
        Err(err) if is_candidate_level_admission_timeout(&err) => {
            record_stream_candidate_admission_timeout(
                state,
                plan,
                report_context,
                candidate_started_unix_ms,
                &err,
            )
            .await;
            log_stream_candidate_admission_timeout(trace_id, plan_kind, plan, report_context, &err);
            return Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::retry(AiAttemptRetryScope::Candidate),
            ));
        }
        Err(err) => return Err(err),
    };
    let permit_hold = permit.map(UpstreamExecutionPermitHold::new);
    let watchdog_started_at = std::time::Instant::now();
    let watchdog_progress = StreamCandidateWatchdogProgress::shared();
    let execution = watchdog_progress.clone().scope(execute());
    tokio::pin!(execution);
    let deadline = tokio::time::sleep(timeout_duration);
    tokio::pin!(deadline);
    let execution_result = tokio::select! {
        biased;
        result = &mut execution => Some(result),
        () = &mut deadline => {
            if watchdog_progress.terminal_started() {
                Some(execution.await)
            } else {
                None
            }
        }
    };
    let outcome = match execution_result {
        Some(result) => result.map(StreamCandidateWatchdogOutcome::Executed),
        None => {
            // The abandoned attempt is dropped when this function returns.
            // Claim its settlement before that so its cancellation guard does
            // not race the watchdog rows written just below.
            watchdog_progress.mark_abandoned();
            let finished_at_unix_ms = current_unix_ms();
            let request_id = short_request_id(plan.request_id.as_str());
            let provider_name = plan.provider_name.as_deref().unwrap_or("-");
            let model_name = plan.model_name.as_deref().unwrap_or("-");
            let candidate_index = parse_request_candidate_report_context(report_context)
                .and_then(|context| context.candidate_index)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            let timeout_ms = u64::try_from(timeout_duration.as_millis()).unwrap_or(u64::MAX);
            record_local_request_candidate_status(
                state,
                plan,
                report_context,
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("local_stream_candidate_watchdog_timeout".to_string()),
                    error_message: Some(stream_candidate_watchdog_timeout_message().to_string()),
                    latency_ms: Some(candidate_started_at.elapsed().as_millis() as u64),
                    started_at_unix_ms: Some(candidate_started_unix_ms),
                    finished_at_unix_ms: Some(finished_at_unix_ms),
                },
            )
            .await;
            warn!(
                event_name = "local_stream_candidate_watchdog_timed_out",
                log_type = "event",
                trace_id = %trace_id,
                plan_kind,
                request_id = %request_id,
                candidate_id = ?plan.candidate_id,
                provider_name,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                model_name,
                candidate_index = candidate_index.as_str(),
                timeout_ms,
                "gateway local stream candidate watchdog timed out"
            );
            if stop_on_transport_errors {
                Ok(StreamCandidateWatchdogOutcome::TransportTimeout)
            } else {
                Ok(StreamCandidateWatchdogOutcome::Executed(
                    AiAttemptExecutionOutcome::retry(AiAttemptRetryScope::Candidate),
                ))
            }
        }
    };
    observe_gateway_stage_ms(
        "stream_candidate_watchdog_inline",
        watchdog_started_at.elapsed().as_millis() as u64,
    );
    match outcome {
        Ok(StreamCandidateWatchdogOutcome::Executed(AiAttemptExecutionOutcome::Responded(
            response,
        ))) => {
            let response = maybe_hold_upstream_execution_permit(Some(response), permit_hold)
                .expect("responded stream attempt must retain its response");
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::Responded(response),
            ))
        }
        Ok(StreamCandidateWatchdogOutcome::Executed(AiAttemptExecutionOutcome::Retry {
            scope,
            fallback_response,
        })) => {
            drop(permit_hold);
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::Retry {
                    scope,
                    fallback_response,
                },
            ))
        }
        Ok(StreamCandidateWatchdogOutcome::TransportTimeout) => {
            drop(permit_hold);
            Ok(StreamCandidateWatchdogOutcome::TransportTimeout)
        }
        Err(err) if is_candidate_level_admission_timeout(&err) => {
            drop(permit_hold);
            if should_record_candidate_admission_timeout(&err) {
                record_stream_candidate_admission_timeout(
                    state,
                    plan,
                    report_context,
                    candidate_started_unix_ms,
                    &err,
                )
                .await;
            }
            log_stream_candidate_admission_timeout(trace_id, plan_kind, plan, report_context, &err);
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::retry(AiAttemptRetryScope::Candidate),
            ))
        }
        Err(err) => {
            drop(permit_hold);
            Err(err)
        }
    }
}

struct UpstreamExecutionPermitHold {
    _permit: ConcurrencyPermit,
    started_at: std::time::Instant,
}

impl UpstreamExecutionPermitHold {
    fn new(permit: ConcurrencyPermit) -> Self {
        Self {
            _permit: permit,
            started_at: std::time::Instant::now(),
        }
    }
}

impl Drop for UpstreamExecutionPermitHold {
    fn drop(&mut self) {
        observe_gateway_stage_ms(
            "upstream_execution_gate_held",
            self.started_at.elapsed().as_millis() as u64,
        );
    }
}

fn maybe_hold_upstream_execution_permit(
    response: Option<Response<Body>>,
    permit_hold: Option<UpstreamExecutionPermitHold>,
) -> Option<Response<Body>> {
    match upstream_execution_gate_stream_hold_mode() {
        UpstreamExecutionStreamHoldMode::Headers => {
            drop(permit_hold);
            response
        }
        UpstreamExecutionStreamHoldMode::FirstBody => match (response, permit_hold) {
            (Some(response), Some(permit_hold)) => Some(
                hold_response_upstream_execution_permit_until_first_body(response, permit_hold),
            ),
            (response, _permit_hold) => response,
        },
        UpstreamExecutionStreamHoldMode::Response => match (response, permit_hold) {
            (Some(response), Some(permit_hold)) => Some(hold_response_upstream_execution_permit(
                response,
                permit_hold,
            )),
            (response, _permit_hold) => response,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpstreamExecutionStreamHoldMode {
    Headers,
    FirstBody,
    Response,
}

fn upstream_execution_gate_stream_hold_mode() -> UpstreamExecutionStreamHoldMode {
    if std::env::var(UPSTREAM_EXECUTION_GATE_HOLD_STREAM_RESPONSE_ENV)
        .ok()
        .is_some_and(|value| parse_env_bool(value.as_str()))
    {
        return UpstreamExecutionStreamHoldMode::Response;
    }
    std::env::var(UPSTREAM_EXECUTION_GATE_STREAM_HOLD_MODE_ENV)
        .ok()
        .as_deref()
        .map(parse_upstream_execution_stream_hold_mode)
        .unwrap_or(UpstreamExecutionStreamHoldMode::FirstBody)
}

fn parse_upstream_execution_stream_hold_mode(value: &str) -> UpstreamExecutionStreamHoldMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "headers" | "header" | "off" | "none" | "disabled" | "disable" | "0" => {
            UpstreamExecutionStreamHoldMode::Headers
        }
        "response" | "full" | "body" | "stream" | "1" => UpstreamExecutionStreamHoldMode::Response,
        _ => UpstreamExecutionStreamHoldMode::FirstBody,
    }
}

fn parse_env_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn hold_response_upstream_execution_permit_until_first_body(
    response: Response<Body>,
    permit_hold: UpstreamExecutionPermitHold,
) -> Response<Body> {
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let mut permit_hold = Some(permit_hold);
        let mut body_stream = body.into_data_stream();
        while let Some(item) = body_stream.next().await {
            drop(permit_hold.take());
            yield item;
        }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

fn hold_response_upstream_execution_permit(
    response: Response<Body>,
    permit_hold: UpstreamExecutionPermitHold,
) -> Response<Body> {
    let (parts, body) = response.into_parts();
    let stream = async_stream::stream! {
        let _permit_hold = permit_hold;
        let mut body_stream = body.into_data_stream();
        while let Some(item) = body_stream.next().await {
            yield item;
        }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

pub(crate) async fn mark_unused_local_candidate_items<T, FPlan, FContext>(
    state: &AppState,
    remaining: Vec<T>,
    plan: FPlan,
    report_context: FContext,
) where
    FPlan: Fn(&T) -> &aether_contracts::ExecutionPlan,
    FContext: Fn(&T) -> Option<&serde_json::Value>,
{
    for item in remaining {
        let report_context = report_context(&item);
        if should_skip_unused_persistence(report_context) {
            continue;
        }
        record_local_request_candidate_status(
            state,
            plan(&item),
            report_context,
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Unused,
                status_code: None,
                error_type: None,
                error_message: None,
                latency_ms: None,
                started_at_unix_ms: None,
                finished_at_unix_ms: None,
            },
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use aether_contracts::{
        ExecutionPlan, ExecutionResult, ExecutionTimeouts, RequestBody, ResponseBody,
    };
    use aether_data::repository::settlement::{
        InMemorySettlementRepository, ReserveUsagePolicyCostInput, ReserveUsagePolicyCostOutcome,
        SettlementWriteRepository, UsagePolicyCostReservationState, UsagePolicyCostWindow,
    };
    use aether_data_contracts::repository::billing::{
        BillingReadRepository, StoredBillingModelContext, UserPlanEntitlementRecord,
    };
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, UpsertRequestCandidateRecord,
    };
    use aether_data_contracts::DataLayerError;
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;

    struct TestRequestCandidateWriter {
        records: Mutex<Vec<UpsertRequestCandidateRecord>>,
        upstream_gate: Option<aether_runtime::ConcurrencyGate>,
        upstream_queue_budget: Duration,
    }

    impl Default for TestRequestCandidateWriter {
        fn default() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
                upstream_gate: None,
                upstream_queue_budget: Duration::from_millis(250),
            }
        }
    }

    impl TestRequestCandidateWriter {
        fn with_upstream_gate(limit: usize, queue_budget: Duration) -> Self {
            Self {
                records: Mutex::new(Vec::new()),
                upstream_gate: Some(aether_runtime::ConcurrencyGate::new(
                    UPSTREAM_EXECUTION_GATE_NAME,
                    limit,
                )),
                upstream_queue_budget: queue_budget,
            }
        }
    }

    #[async_trait]
    impl RequestCandidateRuntimeWriter for TestRequestCandidateWriter {
        fn has_request_candidate_data_writer(&self) -> bool {
            true
        }

        async fn upsert_request_candidate(
            &self,
            candidate: UpsertRequestCandidateRecord,
        ) -> Result<
            Option<aether_data_contracts::repository::candidates::StoredRequestCandidate>,
            GatewayError,
        > {
            self.records.lock().await.push(candidate);
            Ok(None)
        }
    }

    impl UpstreamExecutionGateProvider for TestRequestCandidateWriter {
        fn upstream_execution_gate(&self) -> Option<&aether_runtime::ConcurrencyGate> {
            self.upstream_gate.as_ref()
        }

        fn upstream_execution_gate_queue_budget(&self) -> Duration {
            self.upstream_queue_budget
        }
    }

    struct PendingAttemptSource;

    #[async_trait]
    impl LocalExecutionAttemptSource<()> for PendingAttemptSource {
        async fn next_execution_attempt(&mut self) -> Result<Option<()>, GatewayError> {
            std::future::pending::<()>().await;
            Ok(None)
        }

        async fn drain_execution_attempts(&mut self) -> Result<Vec<()>, GatewayError> {
            Ok(Vec::new())
        }

        async fn skip_credential(&mut self, _key_id: &str) -> Result<(), GatewayError> {
            Ok(())
        }

        async fn skip_endpoint(&mut self, _endpoint_id: &str) -> Result<(), GatewayError> {
            Ok(())
        }

        async fn skip_provider(&mut self, _provider_id: &str) -> Result<(), GatewayError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct CostReservationBillingRepository {
        model_context: StoredBillingModelContext,
        entitlement: UserPlanEntitlementRecord,
    }

    #[async_trait]
    impl BillingReadRepository for CostReservationBillingRepository {
        async fn find_model_context(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _global_model_name: &str,
        ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
            Ok(Some(self.model_context.clone()))
        }

        async fn find_model_context_by_model_id(
            &self,
            _provider_id: &str,
            _provider_api_key_id: Option<&str>,
            _model_id: &str,
        ) -> Result<Option<StoredBillingModelContext>, DataLayerError> {
            Ok(Some(self.model_context.clone()))
        }

        async fn list_user_plan_entitlements(
            &self,
            user_id: &str,
        ) -> Result<Option<Vec<UserPlanEntitlementRecord>>, DataLayerError> {
            Ok(Some(
                (self.entitlement.user_id == user_id)
                    .then(|| self.entitlement.clone())
                    .into_iter()
                    .collect(),
            ))
        }
    }

    struct PlanningErrorAfterFirstSyncAttemptSource {
        first: Option<crate::ai_serving::AiSyncAttempt>,
    }

    #[async_trait]
    impl LocalExecutionAttemptSource<crate::ai_serving::AiSyncAttempt>
        for PlanningErrorAfterFirstSyncAttemptSource
    {
        async fn next_execution_attempt(
            &mut self,
        ) -> Result<Option<crate::ai_serving::AiSyncAttempt>, GatewayError> {
            match self.first.take() {
                Some(first) => Ok(Some(first)),
                None => Err(GatewayError::Internal(
                    "synthetic next-candidate planning failure".to_string(),
                )),
            }
        }

        async fn drain_execution_attempts(
            &mut self,
        ) -> Result<Vec<crate::ai_serving::AiSyncAttempt>, GatewayError> {
            Ok(self.first.take().into_iter().collect())
        }

        async fn skip_credential(&mut self, _key_id: &str) -> Result<(), GatewayError> {
            Ok(())
        }

        async fn skip_endpoint(&mut self, _endpoint_id: &str) -> Result<(), GatewayError> {
            Ok(())
        }

        async fn skip_provider(&mut self, _provider_id: &str) -> Result<(), GatewayError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct TransferTestAttempt {
        label: &'static str,
        plan: ExecutionPlan,
        report_context: serde_json::Value,
    }

    impl AiExecutionAttempt for TransferTestAttempt {
        fn execution_plan(&self) -> &ExecutionPlan {
            &self.plan
        }

        fn report_kind(&self) -> Option<String> {
            None
        }

        fn report_context(&self) -> Option<serde_json::Value> {
            Some(self.report_context.clone())
        }

        fn report_context_ref(&self) -> Option<&serde_json::Value> {
            Some(&self.report_context)
        }
    }

    struct TransferTestPort<'a> {
        state: &'a AppState,
        tracker: ProviderTransferTracker,
        retry_scope: AiAttemptRetryScope,
        executed: StdMutex<Vec<&'static str>>,
        unused: StdMutex<Vec<&'static str>>,
    }

    impl<'a> TransferTestPort<'a> {
        fn new(state: &'a AppState) -> Self {
            Self::with_tracker(state, ProviderTransferTracker::default())
        }

        fn with_tracker(state: &'a AppState, tracker: ProviderTransferTracker) -> Self {
            Self {
                state,
                tracker,
                retry_scope: AiAttemptRetryScope::Candidate,
                executed: StdMutex::new(Vec::new()),
                unused: StdMutex::new(Vec::new()),
            }
        }

        fn with_retry_scope(state: &'a AppState, retry_scope: AiAttemptRetryScope) -> Self {
            Self {
                state,
                tracker: ProviderTransferTracker::default(),
                retry_scope,
                executed: StdMutex::new(Vec::new()),
                unused: StdMutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AiAttemptLoopPort<TransferTestAttempt> for TransferTestPort<'_> {
        type Response = Response<Body>;
        type Exhaustion = crate::executor::LocalExecutionExhaustion;
        type Error = GatewayError;

        async fn should_skip_attempt(
            &self,
            attempt: &TransferTestAttempt,
        ) -> Result<bool, Self::Error> {
            Ok(should_skip_provider_transfer_attempt(
                &self.tracker,
                "trace-transfer-test",
                "transfer_test",
                attempt,
            )
            .await)
        }

        async fn record_attempt_started(
            &self,
            attempt: &TransferTestAttempt,
        ) -> Result<(), Self::Error> {
            record_provider_transfer_attempt_started(&self.tracker, attempt).await;
            Ok(())
        }

        async fn record_attempt_failed(
            &self,
            attempt: &TransferTestAttempt,
        ) -> Result<(), Self::Error> {
            record_provider_transfer_attempt_failed(
                self.state,
                &self.tracker,
                "trace-transfer-test",
                "transfer_test",
                attempt,
            )
            .await;
            Ok(())
        }

        async fn execute_attempt(
            &self,
            attempt: &TransferTestAttempt,
        ) -> Result<AiAttemptExecutionOutcome<Self::Response>, Self::Error> {
            self.executed.lock().unwrap().push(attempt.label);
            Ok(if attempt.plan.provider_id == "provider-b" {
                AiAttemptExecutionOutcome::Responded(Response::new(Body::from("ok")))
            } else {
                AiAttemptExecutionOutcome::retry(self.retry_scope)
            })
        }

        async fn mark_unused_attempts(
            &self,
            attempts: Vec<TransferTestAttempt>,
        ) -> Result<(), Self::Error> {
            self.unused
                .lock()
                .unwrap()
                .extend(attempts.into_iter().map(|attempt| attempt.label));
            Ok(())
        }

        async fn build_exhaustion(
            &self,
            last_plan: ExecutionPlan,
            last_report_context: Option<serde_json::Value>,
        ) -> Result<Self::Exhaustion, Self::Error> {
            Ok(build_local_execution_exhaustion(
                self.state,
                &last_plan,
                last_report_context.as_ref(),
            )
            .await)
        }
    }

    struct TransferTestAttemptSource {
        attempts: std::collections::VecDeque<TransferTestAttempt>,
        skipped_providers: Vec<String>,
    }

    #[async_trait]
    impl LocalExecutionAttemptSource<TransferTestAttempt> for TransferTestAttemptSource {
        async fn next_execution_attempt(
            &mut self,
        ) -> Result<Option<TransferTestAttempt>, GatewayError> {
            Ok(self.attempts.pop_front())
        }

        async fn drain_execution_attempts(
            &mut self,
        ) -> Result<Vec<TransferTestAttempt>, GatewayError> {
            Ok(self.attempts.drain(..).collect())
        }

        async fn skip_credential(&mut self, key_id: &str) -> Result<(), GatewayError> {
            self.attempts
                .retain(|attempt| attempt.plan.key_id != key_id);
            Ok(())
        }

        async fn skip_endpoint(&mut self, endpoint_id: &str) -> Result<(), GatewayError> {
            self.attempts
                .retain(|attempt| attempt.plan.endpoint_id != endpoint_id);
            Ok(())
        }

        async fn skip_provider(&mut self, provider_id: &str) -> Result<(), GatewayError> {
            self.skipped_providers.push(provider_id.to_string());
            self.attempts
                .retain(|attempt| attempt.plan.provider_id != provider_id);
            Ok(())
        }
    }

    fn transfer_test_attempts() -> Vec<TransferTestAttempt> {
        fn attempt(label: &'static str, provider_id: &str, key_id: &str) -> TransferTestAttempt {
            let mut plan = test_plan(None);
            plan.provider_id = provider_id.to_string();
            plan.key_id = key_id.to_string();
            TransferTestAttempt {
                label,
                plan,
                report_context: json!({
                    "local_failover_policy": {
                        "max_transfer_count": 1,
                        "max_transfer_timeout_seconds": 0
                    }
                }),
            }
        }

        vec![
            attempt("a-key1-retry0", "provider-a", "key-1"),
            attempt("a-key2-retry0", "provider-a", "key-2"),
            attempt("a-key2-retry1", "provider-a", "key-2"),
            attempt("a-key3-retry0", "provider-a", "key-3"),
            attempt("b-key1-retry0", "provider-b", "key-b"),
        ]
    }

    #[tokio::test]
    async fn static_loop_allows_same_key_retries_then_skips_next_transfer() {
        let state = AppState::new().expect("state should build");
        let port = TransferTestPort::new(&state);

        let outcome = run_ai_attempt_loop(&port, transfer_test_attempts())
            .await
            .expect("attempt loop should succeed");

        assert!(matches!(outcome, AiAttemptLoopOutcome::Responded(_)));
        assert_eq!(
            port.executed.lock().unwrap().as_slice(),
            [
                "a-key1-retry0",
                "a-key2-retry0",
                "a-key2-retry1",
                "b-key1-retry0"
            ]
        );
        assert_eq!(port.unused.lock().unwrap().as_slice(), ["a-key3-retry0"]);
    }

    #[tokio::test]
    async fn routing_transfer_budget_counts_switches_across_providers_not_same_key_retries() {
        for (limit, succeeds) in [(1, false), (2, true)] {
            let state = AppState::new().unwrap();
            let port = TransferTestPort::new(&state);
            let mut attempts = transfer_test_attempts();
            for attempt in &mut attempts {
                attempt.report_context["routing_execution_policy"] =
                    json!({ "max_transfer_count": limit });
            }
            let outcome = run_ai_attempt_loop(&port, attempts).await.unwrap();
            assert_eq!(
                matches!(outcome, AiAttemptLoopOutcome::Responded(_)),
                succeeds
            );
            {
                let executed = port.executed.lock().unwrap();
                assert_eq!(
                    &executed[..3],
                    ["a-key1-retry0", "a-key2-retry0", "a-key2-retry1"]
                );
                assert_eq!(executed.len(), if succeeds { 4 } else { 3 });
            }
            assert_eq!(port.tracker.state.lock().await.global.transfer_count, limit);
        }
    }

    #[tokio::test]
    async fn dynamic_loop_honors_global_transfer_budget_across_providers() {
        let state = AppState::new().unwrap();
        let port = TransferTestPort::new(&state);
        let mut attempts = transfer_test_attempts();
        for attempt in &mut attempts {
            attempt.report_context["routing_execution_policy"] = json!({ "max_transfer_count": 1 });
        }
        let mut source = TransferTestAttemptSource {
            attempts: attempts.into(),
            skipped_providers: Vec::new(),
        };
        let outcome = run_dynamic_attempt_loop(
            &port,
            &mut source,
            "global-budget",
            "test",
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            LocalExecutionRequestOutcome::Exhausted(_)
        ));
        assert_eq!(
            port.executed.lock().unwrap().as_slice(),
            ["a-key1-retry0", "a-key2-retry0", "a-key2-retry1"]
        );
        assert_eq!(source.skipped_providers, ["provider-a", "provider-b"]);
    }

    #[test]
    fn routing_time_budget_is_cumulative_and_zero_is_unlimited() {
        let mut global = super::GlobalTransferState::default();
        global.load_policy(Some(
            &json!({ "routing_execution_policy": { "max_transfer_timeout_seconds": 60 } }),
        ));
        let now = tokio::time::Instant::now();
        let plan = test_plan(None);
        global.record_attempt_started(&plan, now);
        global.record_attempt_started(&plan, now + Duration::from_secs(40));
        assert_eq!(global.transfer_count, 0);
        assert_eq!(
            global.check_before_attempt(&plan, now + Duration::from_secs(59)),
            None
        );
        assert_eq!(
            global.check_before_attempt(&plan, now + Duration::from_secs(60)),
            Some((false, true))
        );
        let mut unlimited = super::GlobalTransferState::default();
        unlimited.load_policy(Some(&json!({ "routing_execution_policy": {} })));
        unlimited.record_attempt_started(&plan, now);
        assert_eq!(
            unlimited.check_before_attempt(&plan, now + Duration::from_secs(86_400)),
            None
        );
    }

    #[tokio::test]
    async fn cloned_tracker_preserves_global_budget_across_candidate_loops() {
        let state = AppState::new().unwrap();
        let tracker = ProviderTransferTracker::default();
        let mut attempts = transfer_test_attempts();
        for attempt in &mut attempts {
            attempt.report_context["routing_execution_policy"] = json!({ "max_transfer_count": 1 });
        }
        let remaining = attempts.split_off(3);
        let first_port = TransferTestPort::with_tracker(&state, tracker.clone());
        let first_outcome = run_ai_attempt_loop(&first_port, attempts).await.unwrap();
        assert!(matches!(first_outcome, AiAttemptLoopOutcome::Exhausted(_)));
        assert_eq!(tracker.state.lock().await.global.transfer_count, 1);

        let second_port = TransferTestPort::with_tracker(&state, tracker.clone());
        let mut source = TransferTestAttemptSource {
            attempts: remaining.into(),
            skipped_providers: Vec::new(),
        };
        let second_outcome = run_dynamic_attempt_loop(
            &second_port,
            &mut source,
            "global-budget-across-loops",
            "test",
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(matches!(
            second_outcome,
            LocalExecutionRequestOutcome::NoPath
        ));
        assert!(second_port.executed.lock().unwrap().is_empty());
        assert_eq!(source.skipped_providers, ["provider-a", "provider-b"]);
        assert!(tracker.state.lock().await.global.exhausted);
    }

    #[tokio::test]
    async fn cloned_tracker_preserves_transfer_budget_across_candidate_loops() {
        let state = AppState::new().expect("state should build");
        let tracker = ProviderTransferTracker::default();
        let mut attempts = transfer_test_attempts();
        let provider_b = attempts.pop().expect("provider-b attempt should exist");
        let key_3 = attempts.pop().expect("third provider-a key should exist");
        let first_port = TransferTestPort::with_tracker(&state, tracker.clone());

        let first_outcome = run_ai_attempt_loop(&first_port, attempts)
            .await
            .expect("first candidate loop should exhaust");
        assert!(matches!(first_outcome, AiAttemptLoopOutcome::Exhausted(_)));

        let second_port = TransferTestPort::with_tracker(&state, tracker);
        let second_outcome = run_ai_attempt_loop(&second_port, vec![key_3, provider_b])
            .await
            .expect("second candidate loop should succeed");

        assert!(matches!(second_outcome, AiAttemptLoopOutcome::Responded(_)));
        assert_eq!(
            second_port.executed.lock().unwrap().as_slice(),
            ["b-key1-retry0"]
        );
        assert_eq!(
            second_port.unused.lock().unwrap().as_slice(),
            ["a-key3-retry0"]
        );
    }

    #[tokio::test]
    async fn dynamic_loop_skips_exhausted_provider_at_candidate_source() {
        let state = AppState::new().expect("state should build");
        let port = TransferTestPort::new(&state);
        let mut source = TransferTestAttemptSource {
            attempts: transfer_test_attempts().into(),
            skipped_providers: Vec::new(),
        };

        let outcome = run_dynamic_attempt_loop(
            &port,
            &mut source,
            "trace-transfer-test",
            "transfer_test",
            Duration::from_secs(1),
        )
        .await
        .expect("dynamic attempt loop should succeed");

        assert!(matches!(
            outcome,
            LocalExecutionRequestOutcome::Responded(_)
        ));
        assert_eq!(
            port.executed.lock().unwrap().as_slice(),
            [
                "a-key1-retry0",
                "a-key2-retry0",
                "a-key2-retry1",
                "b-key1-retry0"
            ]
        );
        assert_eq!(source.skipped_providers, ["provider-a"]);
    }

    #[tokio::test]
    async fn dynamic_loop_applies_provider_scoped_retry_to_candidate_source() {
        let state = AppState::new().expect("state should build");
        let port = TransferTestPort::with_retry_scope(&state, AiAttemptRetryScope::Provider);
        let mut source = TransferTestAttemptSource {
            attempts: transfer_test_attempts().into(),
            skipped_providers: Vec::new(),
        };

        let outcome = run_dynamic_attempt_loop(
            &port,
            &mut source,
            "trace-provider-scope-test",
            "provider_scope_test",
            Duration::from_secs(1),
        )
        .await
        .expect("dynamic attempt loop should succeed");

        assert!(matches!(
            outcome,
            LocalExecutionRequestOutcome::Responded(_)
        ));
        assert_eq!(
            port.executed.lock().unwrap().as_slice(),
            ["a-key1-retry0", "b-key1-retry0"]
        );
        assert_eq!(source.skipped_providers, ["provider-a"]);
    }

    #[test]
    fn transfer_timeout_is_checked_at_candidate_boundary_and_zero_disables_limits() {
        let started_at = Instant::now();
        let mut first = test_plan(None);
        first.provider_id = "provider-a".to_string();
        first.key_id = "key-1".to_string();

        let mut timeout_tracker = ProviderTransferStateTracker::default();
        timeout_tracker.record_attempt_started(&first, started_at);
        timeout_tracker.set_limits(
            "provider-a",
            ProviderTransferLimits {
                max_transfer_count: 0,
                max_transfer_timeout_seconds: 60,
            },
        );
        assert!(timeout_tracker
            .check_before_attempt(&first, started_at + Duration::from_secs(59))
            .is_none());
        let reached = timeout_tracker
            .check_before_attempt(&first, started_at + Duration::from_secs(60))
            .expect("timeout should stop the provider at the next candidate boundary");
        assert!(reached.timeout_reached);
        assert!(!reached.count_reached);

        let mut count_tracker = ProviderTransferStateTracker::default();
        count_tracker.record_attempt_started(&first, started_at);
        count_tracker.set_limits(
            "provider-a",
            ProviderTransferLimits {
                max_transfer_count: 1,
                max_transfer_timeout_seconds: 60,
            },
        );
        let mut second_key = first.clone();
        second_key.key_id = "key-2".to_string();
        assert!(count_tracker
            .check_before_attempt(&second_key, started_at + Duration::from_secs(1))
            .is_none());
        count_tracker.record_attempt_started(&second_key, started_at + Duration::from_secs(1));
        let mut third_key = first.clone();
        third_key.key_id = "key-3".to_string();
        let reached = count_tracker
            .check_before_attempt(&third_key, started_at + Duration::from_secs(2))
            .expect("count should stop the provider before another key transfer");
        assert!(reached.count_reached);
        assert!(!reached.timeout_reached);

        let mut unlimited_tracker = ProviderTransferStateTracker::default();
        unlimited_tracker.record_attempt_started(&first, started_at);
        unlimited_tracker.set_limits("provider-a", ProviderTransferLimits::default());
        let mut another_key = first.clone();
        another_key.key_id = "key-2".to_string();
        assert!(unlimited_tracker
            .check_before_attempt(&another_key, started_at + Duration::from_secs(3_600))
            .is_none());
    }

    #[test]
    fn transfer_count_and_timeout_limits_use_or_semantics() {
        let started_at = Instant::now();
        let mut first = test_plan(None);
        first.provider_id = "provider-a".to_string();
        first.key_id = "key-1".to_string();
        let limits = ProviderTransferLimits {
            max_transfer_count: 1,
            max_transfer_timeout_seconds: 60,
        };

        let mut count_first = ProviderTransferStateTracker::default();
        count_first.record_attempt_started(&first, started_at);
        count_first.set_limits("provider-a", limits);
        let mut second = first.clone();
        second.key_id = "key-2".to_string();
        count_first.record_attempt_started(&second, started_at + Duration::from_secs(1));
        let mut third = first.clone();
        third.key_id = "key-3".to_string();
        let count_reached = count_first
            .check_before_attempt(&third, started_at + Duration::from_secs(2))
            .expect("count should independently exhaust a provider before timeout");
        assert!(count_reached.count_reached);
        assert!(!count_reached.timeout_reached);

        let mut timeout_first = ProviderTransferStateTracker::default();
        timeout_first.record_attempt_started(&first, started_at);
        timeout_first.set_limits("provider-a", limits);
        let timeout_reached = timeout_first
            .check_before_attempt(&first, started_at + Duration::from_secs(60))
            .expect("timeout should independently exhaust a provider before count");
        assert!(!timeout_reached.count_reached);
        assert!(timeout_reached.timeout_reached);
    }

    fn test_plan(timeouts: Option<ExecutionTimeouts>) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req_watchdog".to_string(),
            candidate_id: Some("cand_watchdog".to_string()),
            provider_name: Some("provider".to_string()),
            provider_id: "provider_id".to_string(),
            endpoint_id: "endpoint_id".to_string(),
            key_id: "key_id".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/messages".to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-test"})),
            stream: true,
            client_api_format: "claude:messages".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-test".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts,
        }
    }

    #[tokio::test]
    async fn next_execution_attempt_times_out_instead_of_waiting_forever() {
        let mut source = PendingAttemptSource;

        let err = next_execution_attempt_with_timeout(
            &mut source,
            "trace-planning-timeout",
            "openai_responses_sync",
            Duration::from_millis(5),
        )
        .await
        .expect_err("pending candidate planning should time out");

        match err {
            GatewayError::LocalExecutionPlanningTimeout {
                trace_id,
                phase,
                timeout_ms,
            } => {
                assert_eq!(trace_id, "trace-planning-timeout");
                assert_eq!(phase, "next_execution_attempt");
                assert_eq!(timeout_ms, 5);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn dynamic_sync_planning_error_releases_reserved_http_plan_cost() {
        let now_unix_secs = current_unix_ms() / 1_000;
        let request_id = "req-http-reservation-planning-error";
        let subject_id = "user-http-reservation-planning-error";
        let settlement = Arc::new(InMemorySettlementRepository::default());
        let billing = Arc::new(CostReservationBillingRepository {
            model_context: StoredBillingModelContext::new(
                "provider-1".to_string(),
                None,
                Some("key-1".to_string()),
                None,
                None,
                "global-model-1".to_string(),
                "gpt-5".to_string(),
                None,
                Some(0.25),
                None,
                Some("model-1".to_string()),
                Some("gpt-5".to_string()),
                None,
                None,
                None,
            )
            .expect("billing context should build"),
            entitlement: UserPlanEntitlementRecord {
                id: "ent-http-reservation-planning-error".to_string(),
                user_id: subject_id.to_string(),
                plan_id: "plan-http-reservation-planning-error".to_string(),
                payment_order_id: "order-http-reservation-planning-error".to_string(),
                status: "active".to_string(),
                starts_at_unix_secs: now_unix_secs.saturating_sub(60),
                expires_at_unix_secs: now_unix_secs.saturating_add(3_600),
                entitlements_snapshot: json!([{
                    "type": "usage_policy",
                    "rules": [{
                        "metric": "actual_cost_usd",
                        "window": {"kind": "rolling", "seconds": 3_600},
                        "limit": 10.0
                    }]
                }]),
                created_at_unix_secs: now_unix_secs.saturating_sub(60),
                updated_at_unix_secs: now_unix_secs.saturating_sub(60),
            },
        });
        let data = crate::data::GatewayDataState::with_billing_reader_for_tests(billing)
            .with_settlement_writer_for_tests(settlement.clone());
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(data)
            .with_usage_runtime_for_tests(crate::usage::UsageRuntimeConfig {
                enabled: true,
                ..crate::usage::UsageRuntimeConfig::default()
            })
            .with_execution_runtime_sync_override_for_tests(|plan| {
                Ok(ExecutionResult {
                    request_id: plan.request_id.clone(),
                    candidate_id: plan.candidate_id.clone(),
                    status_code: http::StatusCode::TOO_MANY_REQUESTS.as_u16(),
                    headers: Default::default(),
                    response_observation: None,
                    body: Some(ResponseBody {
                        json_body: Some(json!({"error": {"message": "retry elsewhere"}})),
                        body_bytes_b64: None,
                    }),
                    telemetry: None,
                    error: None,
                })
            });

        let mut decision = GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        )
        .with_execution_runtime_candidate(true);
        decision.auth_context = Some(crate::control::GatewayControlAuthContext {
            user_id: subject_id.to_string(),
            api_key_id: "api-key-http-reservation-planning-error".to_string(),
            username: None,
            api_key_name: None,
            balance_remaining: None,
            access_allowed: true,
            user_rate_limit: None,
            api_key_rate_limit: None,
            api_key_is_standalone: false,
            admin_bypass_limits: false,
            local_rejection: None,
            allowed_models: None,
            ip_rules: None,
            verified_api_key_hash: None,
        });
        let admission = crate::plan_usage_policy::check_and_acquire_http_plan_usage_policy(
            &state,
            Some(&decision),
            request_id,
            now_unix_secs.saturating_mul(1_000),
        )
        .await
        .expect("HTTP plan admission should succeed");
        let reservation = admission
            .reservation_context
            .expect("cost policy should freeze a reservation context");
        let reservation_token = reservation.token().to_string();
        let admitted_at_unix_secs = reservation.admitted_at_unix_secs();

        let mut request = http::Request::builder()
            .uri("/v1/chat/completions")
            .body(())
            .expect("request should build");
        request.extensions_mut().insert(reservation);
        let (parts, _) = request.into_parts();
        let mut plan = test_plan(None);
        plan.request_id = request_id.to_string();
        plan.candidate_id = Some("cand-http-reservation-planning-error".to_string());
        plan.provider_id = "provider-1".to_string();
        plan.endpoint_id = "endpoint-1".to_string();
        plan.key_id = "key-1".to_string();
        plan.client_api_format = "openai:chat".to_string();
        plan.provider_api_format = "openai:chat".to_string();
        plan.model_name = Some("gpt-5".to_string());
        plan.stream = false;
        plan.body = RequestBody::from_json(json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 16
        }));
        let source = PlanningErrorAfterFirstSyncAttemptSource {
            first: Some(crate::ai_serving::AiSyncAttempt {
                plan,
                report_kind: None,
                report_context: Some(json!({
                    "candidate_index": 0,
                    "retry_index": 0,
                    "model_id": "model-1",
                    "global_model_name": "gpt-5"
                })),
            }),
        };

        let error = execute_sync_attempt_source::<crate::ai_serving::AiSyncAttempt, _>(
            &state,
            &parts,
            "trace-http-reservation-planning-error",
            &decision,
            "openai_chat_sync",
            source,
        )
        .await
        .expect_err("second candidate planning should fail");
        assert!(matches!(
            error,
            GatewayError::Internal(message)
                if message == "synthetic next-candidate planning failure"
        ));

        let probe = settlement
            .reserve_usage_policy_cost(ReserveUsagePolicyCostInput {
                request_id: request_id.to_string(),
                subject_id: subject_id.to_string(),
                reservation_token,
                admitted_at_unix_secs,
                reserved_cost_units: 1,
                reservation_expires_at_unix_secs: admitted_at_unix_secs.saturating_add(86_400),
                retain_until_unix_secs: admitted_at_unix_secs.saturating_add(32 * 86_400),
                windows: vec![UsagePolicyCostWindow {
                    window_id: "release-state-probe".to_string(),
                    starts_at_unix_secs: admitted_at_unix_secs.saturating_sub(1),
                    ends_at_unix_secs: admitted_at_unix_secs.saturating_add(1),
                    limit_cost_units: 1_000_000_000,
                }],
            })
            .await
            .expect("reservation state probe should succeed");
        assert_eq!(
            probe,
            ReserveUsagePolicyCostOutcome::AlreadyTerminal {
                state: UsagePolicyCostReservationState::Released,
            }
        );
    }

    fn test_report_context() -> serde_json::Value {
        json!({
            "request_id": "req_watchdog",
            "candidate_id": "cand_watchdog",
            "candidate_index": 2,
            "retry_index": 0,
            "user_id": "user_1",
            "api_key_id": "api_key_1",
        })
    }

    #[test]
    fn request_tracker_preserves_server_reservation_identity_across_clones() {
        let mut request = http::Request::new(());
        let gate = aether_runtime::ConcurrencyGate::new("image_heartbeat_request", 1);
        let admission = AdmissionPermit::from(gate.try_acquire().expect("request admission"));
        request
            .extensions_mut()
            .insert(BackgroundAdmissionPermit::new(admission.clone()));
        request.extensions_mut().insert(
            crate::plan_usage_policy::PlanUsageReservationContext::for_test(
                "user-1",
                "server-token",
                12_345,
                crate::plan_usage_policy::EffectivePlanUsagePolicy::default(),
            ),
        );
        let (parts, _) = request.into_parts();

        let tracker = ProviderTransferTracker::for_request(&parts);
        let cloned = tracker.clone();

        let reservation = tracker
            .usage_policy_reservation
            .as_ref()
            .expect("request reservation snapshot");
        let cloned_reservation = cloned
            .usage_policy_reservation
            .as_ref()
            .expect("cloned reservation snapshot");
        assert_eq!(reservation.admitted_at_unix_secs(), 12_345);
        assert_eq!(reservation.subject_id(), "user-1");
        assert_eq!(reservation.token(), "server-token");
        assert_eq!(cloned_reservation.token(), "server-token");
        assert!(std::ptr::eq(
            reservation.policy(),
            cloned_reservation.policy()
        ));

        drop((parts, admission, tracker));
        assert_eq!(gate.snapshot().in_flight, 1);
        assert!(
            gate.try_acquire().is_err(),
            "image heartbeat tracker clone should retain request admission"
        );
        drop(cloned);
        assert_eq!(gate.snapshot().in_flight, 0);
    }

    #[test]
    fn server_reservation_token_overrides_report_context_value() {
        let context = attach_plan_usage_reservation_token(
            Some(json!({
                "candidate_index": 2,
                "plan_usage_reservation_token": "client-controlled-token"
            })),
            Some("server-token"),
        )
        .expect("reservation context");

        assert_eq!(context["candidate_index"], 2);
        assert_eq!(context["plan_usage_reservation_token"], "server-token");
        assert_eq!(context["plan_usage_reservation_deferred"], false);
    }

    #[test]
    fn missing_reservation_does_not_create_or_propagate_a_token() {
        let request = http::Request::new(());
        let (parts, _) = request.into_parts();
        let tracker = ProviderTransferTracker::for_request(&parts);
        assert!(tracker.usage_policy_reservation.is_none());
        assert!(tracker.usage_policy_reservation_token().is_none());

        let original = json!({
            "candidate_index": 2,
            "plan_usage_reservation_token": "untrusted-seed-token",
            "plan_usage_reservation_deferred": true
        });
        let expected = json!({"candidate_index": 2});
        assert_eq!(
            attach_plan_usage_reservation_token(Some(original), None),
            Some(expected)
        );
        assert_eq!(attach_plan_usage_reservation_token(None, None), None);
    }

    #[test]
    fn stream_candidate_watchdog_prefers_first_byte_timeout() {
        let report_context = json!({"upstream_is_stream": true});
        let timeout = resolve_stream_candidate_watchdog_timeout(
            &test_plan(Some(ExecutionTimeouts {
                first_byte_ms: Some(12_345),
                total_ms: Some(90_000),
                ..ExecutionTimeouts::default()
            })),
            Some(&report_context),
        );

        assert_eq!(timeout, Duration::from_millis(12_345));
    }

    #[test]
    fn stream_candidate_watchdog_uses_default_when_timeouts_missing() {
        let timeout = resolve_stream_candidate_watchdog_timeout(&test_plan(None), None);

        assert_eq!(
            timeout,
            Duration::from_millis(DEFAULT_STREAM_FIRST_BYTE_WATCHDOG_TIMEOUT_MS)
        );
    }

    #[test]
    fn stream_candidate_watchdog_ignores_total_timeout_for_stream_upstream() {
        let report_context = json!({"upstream_is_stream": true});
        let timeout = resolve_stream_candidate_watchdog_timeout(
            &test_plan(Some(ExecutionTimeouts {
                total_ms: Some(90_000),
                ..ExecutionTimeouts::default()
            })),
            Some(&report_context),
        );

        assert_eq!(
            timeout,
            Duration::from_millis(DEFAULT_STREAM_FIRST_BYTE_WATCHDOG_TIMEOUT_MS)
        );
    }

    #[test]
    fn stream_candidate_watchdog_prefers_first_byte_timeout_when_upstream_non_stream() {
        let report_context = json!({"upstream_is_stream": false});
        let timeout = resolve_stream_candidate_watchdog_timeout(
            &test_plan(Some(ExecutionTimeouts {
                first_byte_ms: Some(12_345),
                total_ms: Some(599_000),
                ..ExecutionTimeouts::default()
            })),
            Some(&report_context),
        );

        assert_eq!(timeout, Duration::from_millis(12_345));
    }

    #[test]
    fn stream_candidate_watchdog_ignores_total_timeout_when_upstream_non_stream() {
        let report_context = json!({"upstream_is_stream": false});
        let timeout = resolve_stream_candidate_watchdog_timeout(
            &test_plan(Some(ExecutionTimeouts {
                total_ms: Some(599_000),
                ..ExecutionTimeouts::default()
            })),
            Some(&report_context),
        );

        assert_eq!(
            timeout,
            Duration::from_millis(DEFAULT_STREAM_FIRST_BYTE_WATCHDOG_TIMEOUT_MS)
        );
    }

    #[test]
    fn stream_candidate_watchdog_defaults_to_streaming_when_flag_missing() {
        let report_context = json!({});
        let timeout = resolve_stream_candidate_watchdog_timeout(
            &test_plan(Some(ExecutionTimeouts {
                first_byte_ms: Some(12_345),
                total_ms: Some(90_000),
                ..ExecutionTimeouts::default()
            })),
            Some(&report_context),
        );

        assert_eq!(timeout, Duration::from_millis(12_345));
    }

    #[test]
    fn upstream_execution_stream_hold_mode_defaults_to_first_body() {
        assert_eq!(
            parse_upstream_execution_stream_hold_mode(""),
            UpstreamExecutionStreamHoldMode::FirstBody
        );
        assert_eq!(
            parse_upstream_execution_stream_hold_mode("first_body"),
            UpstreamExecutionStreamHoldMode::FirstBody
        );
        assert_eq!(
            parse_upstream_execution_stream_hold_mode("off"),
            UpstreamExecutionStreamHoldMode::Headers
        );
        assert_eq!(
            parse_upstream_execution_stream_hold_mode("response"),
            UpstreamExecutionStreamHoldMode::Response
        );
    }

    #[test]
    fn unused_persistence_skips_pool_internal_candidates() {
        assert!(should_skip_unused_persistence(Some(&json!({
            "candidate_group_id": "pool-group",
            "pool_key_index": 0,
        }))));
        assert!(should_skip_unused_persistence(Some(&json!({
            "candidate_group_id": "pool-group",
            "pool_key_index": 1,
        }))));
        assert!(!should_skip_unused_persistence(Some(&json!({
            "candidate_group_id": "pool-group",
        }))));
        assert!(!should_skip_unused_persistence(Some(&json!({
            "candidate_index": 1,
        }))));
    }

    #[tokio::test]
    async fn stream_candidate_watchdog_marks_failed_candidate_and_continues() {
        let writer = Arc::new(TestRequestCandidateWriter::default());
        let plan = test_plan(Some(ExecutionTimeouts {
            first_byte_ms: Some(25),
            ..ExecutionTimeouts::default()
        }));
        let report_context = test_report_context();
        let writer_for_task = writer.clone();

        let task = tokio::spawn(async move {
            execute_stream_candidate_with_watchdog(
                writer_for_task.as_ref(),
                "trace_watchdog",
                "claude_cli_stream",
                &plan,
                Some(&report_context),
                false,
                || {
                    std::future::pending::<
                        Result<AiAttemptExecutionOutcome<Response<Body>>, GatewayError>,
                    >()
                },
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(40)).await;
        let result = task.await.expect("watchdog task should join");
        assert!(matches!(
            result,
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::Retry {
                    scope: AiAttemptRetryScope::Candidate,
                    fallback_response: None,
                }
            ))
        ));

        let records = writer.records.lock().await;
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.status, RequestCandidateStatus::Failed);
        assert_eq!(record.status_code, None);
        assert_eq!(
            record.error_type.as_deref(),
            Some("local_stream_candidate_watchdog_timeout")
        );
        assert!(record
            .error_message
            .as_deref()
            .is_some_and(|message| message == "Stream first byte timeout"));
        assert_eq!(record.candidate_index, 2);
    }

    async fn assert_stream_candidate_retry_gets_fresh_first_byte_budget(
        provider_id: &str,
        key_id: &str,
        first_byte_ms: u64,
    ) {
        let writer = TestRequestCandidateWriter::default();
        let plan = test_plan(Some(ExecutionTimeouts {
            first_byte_ms: Some(100),
            ..ExecutionTimeouts::default()
        }));
        let report_context = test_report_context();

        let result = execute_stream_candidate_with_watchdog(
            &writer,
            "trace_watchdog_retry_budget",
            "claude_cli_stream",
            &plan,
            Some(&report_context),
            false,
            || {
                std::future::pending::<
                    Result<AiAttemptExecutionOutcome<Response<Body>>, GatewayError>,
                >()
            },
        )
        .await;
        assert!(matches!(
            result,
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::Retry {
                    scope: AiAttemptRetryScope::Candidate,
                    fallback_response: None,
                }
            ))
        ));

        let mut next_plan = plan.clone();
        next_plan.candidate_id = Some("cand_watchdog_retry".to_string());
        next_plan.provider_id = provider_id.to_string();
        next_plan.key_id = key_id.to_string();
        next_plan.timeouts = Some(ExecutionTimeouts {
            first_byte_ms: Some(first_byte_ms),
            ..ExecutionTimeouts::default()
        });
        let mut next_report_context = report_context.clone();
        next_report_context["candidate_id"] = json!("cand_watchdog_retry");
        next_report_context["candidate_index"] = json!(3);

        let result = execute_stream_candidate_with_watchdog(
            &writer,
            "trace_watchdog_retry_budget",
            "claude_cli_stream",
            &next_plan,
            Some(&next_report_context),
            false,
            || async {
                tokio::time::sleep(Duration::from_millis(60)).await;
                Ok(AiAttemptExecutionOutcome::Responded(Response::new(
                    Body::from("retry succeeded"),
                )))
            },
        )
        .await;
        assert!(
            matches!(
                result,
                Ok(StreamCandidateWatchdogOutcome::Executed(
                    AiAttemptExecutionOutcome::Responded(_)
                ))
            ),
            "candidate {provider_id}/{key_id} must receive its own {first_byte_ms} ms budget"
        );

        let records = writer.records.lock().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, plan.candidate_id.as_deref().unwrap());
        assert_eq!(records[0].status, RequestCandidateStatus::Failed);
        assert_eq!(
            records[0].error_type.as_deref(),
            Some("local_stream_candidate_watchdog_timeout")
        );
    }

    #[tokio::test]
    async fn stream_candidate_watchdog_failover_gets_fresh_first_byte_budget() {
        for first_byte_ms in [100, 75, 150] {
            assert_stream_candidate_retry_gets_fresh_first_byte_budget(
                "provider_next",
                "key_next",
                first_byte_ms,
            )
            .await;
        }
    }

    #[tokio::test]
    async fn stream_candidate_watchdog_same_provider_retries_get_fresh_first_byte_budget() {
        for key_id in ["key_next", "key_id"] {
            assert_stream_candidate_retry_gets_fresh_first_byte_budget("provider_id", key_id, 100)
                .await;
        }
    }

    #[tokio::test]
    async fn stream_candidate_watchdog_starts_first_byte_budget_after_admission() {
        let writer = TestRequestCandidateWriter::with_upstream_gate(1, Duration::from_secs(1));
        let held_permit = writer
            .upstream_gate
            .as_ref()
            .expect("test gate should exist")
            .try_acquire()
            .expect("test gate permit should acquire");
        let plan = test_plan(Some(ExecutionTimeouts {
            first_byte_ms: Some(50),
            ..ExecutionTimeouts::default()
        }));
        let report_context = test_report_context();

        let (result, ()) = tokio::join!(
            execute_stream_candidate_with_watchdog(
                &writer,
                "trace_watchdog_admission_budget",
                "claude_cli_stream",
                &plan,
                Some(&report_context),
                false,
                || async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok(AiAttemptExecutionOutcome::Responded(Response::new(
                        Body::from("admitted candidate succeeded"),
                    )))
                },
            ),
            async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                drop(held_permit);
            },
        );

        assert!(matches!(
            result,
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::Responded(_)
            ))
        ));
        assert!(writer.records.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stream_candidate_watchdog_can_stop_on_transport_error() {
        let writer = Arc::new(TestRequestCandidateWriter::default());
        let plan = test_plan(Some(ExecutionTimeouts {
            first_byte_ms: Some(5),
            ..ExecutionTimeouts::default()
        }));
        let report_context = test_report_context();

        let result = execute_stream_candidate_with_watchdog(
            writer.as_ref(),
            "trace_watchdog_stop",
            "claude_cli_stream",
            &plan,
            Some(&report_context),
            true,
            || {
                std::future::pending::<
                    Result<AiAttemptExecutionOutcome<Response<Body>>, GatewayError>,
                >()
            },
        )
        .await;

        assert!(matches!(
            result,
            Ok(StreamCandidateWatchdogOutcome::TransportTimeout)
        ));
        let records = writer.records.lock().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status_code, None);
        assert_eq!(
            records[0].error_type.as_deref(),
            Some("local_stream_candidate_watchdog_timeout")
        );
    }

    #[tokio::test]
    async fn stream_candidate_watchdog_does_not_cancel_started_terminalization() {
        let writer = Arc::new(TestRequestCandidateWriter::default());
        let plan = test_plan(Some(ExecutionTimeouts {
            first_byte_ms: Some(5),
            ..ExecutionTimeouts::default()
        }));
        let report_context = test_report_context();

        let result = execute_stream_candidate_with_watchdog(
            writer.as_ref(),
            "trace_terminalization",
            "claude_cli_stream",
            &plan,
            Some(&report_context),
            true,
            || async {
                mark_stream_candidate_watchdog_terminal_started();
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(AiAttemptExecutionOutcome::Responded(Response::new(
                    Body::from("terminal response"),
                )))
            },
        )
        .await;

        assert!(matches!(
            result,
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::Responded(_)
            ))
        ));
        assert!(writer.records.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stream_candidate_watchdog_does_not_relabel_execution_error_as_timeout() {
        let writer = Arc::new(TestRequestCandidateWriter::default());
        let plan = test_plan(None);
        let report_context = test_report_context();

        let result = execute_stream_candidate_with_watchdog(
            writer.as_ref(),
            "trace_execution_error",
            "claude_cli_stream",
            &plan,
            Some(&report_context),
            true,
            || async {
                Err(GatewayError::UpstreamUnavailable {
                    trace_id: "trace_execution_error".to_string(),
                    message: "upstream connect failed".to_string(),
                })
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(GatewayError::UpstreamUnavailable { message, .. })
                if message == "upstream connect failed"
        ));
        assert!(writer.records.lock().await.is_empty());
    }

    #[tokio::test]
    async fn stream_candidate_upstream_execution_admission_timeout_marks_failed_and_continues() {
        let writer = Arc::new(TestRequestCandidateWriter::with_upstream_gate(
            1,
            Duration::from_millis(1),
        ));
        let _held_permit = writer
            .upstream_gate
            .as_ref()
            .expect("test gate should exist")
            .try_acquire()
            .expect("test gate permit should acquire");
        let plan = test_plan(None);
        let report_context = test_report_context();

        let result = execute_stream_candidate_with_watchdog(
            writer.as_ref(),
            "trace_admission",
            "claude_cli_stream",
            &plan,
            Some(&report_context),
            false,
            || async {
                panic!("execute future should not run while upstream execution gate is saturated")
            },
        )
        .await;

        assert!(matches!(
            result,
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::Retry {
                    scope: AiAttemptRetryScope::Candidate,
                    fallback_response: None,
                }
            ))
        ));
        let records = writer.records.lock().await;
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.status, RequestCandidateStatus::Failed);
        assert_eq!(
            record.status_code,
            Some(http::StatusCode::TOO_MANY_REQUESTS.as_u16())
        );
        assert_eq!(
            record.error_type.as_deref(),
            Some("gateway_admission_timeout")
        );
        assert!(record
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains(UPSTREAM_EXECUTION_GATE_NAME)));
        assert_eq!(record.candidate_index, 2);
    }

    #[tokio::test]
    async fn stream_candidate_target_admission_timeout_continues_without_duplicate_record() {
        let writer = Arc::new(TestRequestCandidateWriter::default());
        let plan = test_plan(None);
        let report_context = test_report_context();

        let result = execute_stream_candidate_with_watchdog(
            writer.as_ref(),
            "trace_target_admission",
            "claude_cli_stream",
            &plan,
            Some(&report_context),
            false,
            || async {
                Err(GatewayError::AdmissionTimeout {
                    trace_id: "trace_target_admission".to_string(),
                    gate: UPSTREAM_TARGET_GATE_NAME,
                    queue_budget_ms: 5,
                })
            },
        )
        .await;

        assert!(matches!(
            result,
            Ok(StreamCandidateWatchdogOutcome::Executed(
                AiAttemptExecutionOutcome::Retry {
                    scope: AiAttemptRetryScope::Candidate,
                    fallback_response: None,
                }
            ))
        ));
        assert!(writer.records.lock().await.is_empty());
    }
}
