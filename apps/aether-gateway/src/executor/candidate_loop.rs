use aether_ai_serving::{
    run_ai_attempt_loop, AiAttemptLoopOutcome, AiAttemptLoopPort, AiExecutionAttempt,
};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_runtime::ConcurrencyPermit;
use aether_scheduler_core::{
    parse_request_candidate_report_context, SchedulerRequestCandidateStatusUpdate,
};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::Response;
use futures_util::StreamExt;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn, Instrument};

use crate::ai_serving::LocalExecutionAttemptSource;
use crate::clock::current_unix_ms;
use crate::control::GatewayControlDecision;
use crate::execution_runtime::{execute_execution_runtime_stream, execute_execution_runtime_sync};
use crate::executor::{build_local_execution_exhaustion, LocalExecutionRequestOutcome};
use crate::handlers::shared::provider_pool::release_admin_provider_pool_key_lease;
use crate::log_ids::short_request_id;
use crate::orchestration::local_execution_candidate_metadata_from_report_context;
use crate::privacy::RedactionExecutionCandidateId;
use crate::request_candidate_runtime::{
    record_local_request_candidate_status, RequestCandidateRuntimeWriter,
};
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AppState, GatewayError};

const DEFAULT_STREAM_FIRST_BYTE_WATCHDOG_TIMEOUT_MS: u64 = 30_000;
const UPSTREAM_EXECUTION_GATE_NAME: &str = "gateway_upstream_execution";
const UPSTREAM_TARGET_GATE_NAME: &str = "gateway_upstream_target";
const UPSTREAM_EXECUTION_GATE_HOLD_STREAM_RESPONSE_ENV: &str =
    "AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_HOLD_STREAM_RESPONSE";
const UPSTREAM_EXECUTION_GATE_STREAM_HOLD_MODE_ENV: &str =
    "AETHER_GATEWAY_UPSTREAM_EXECUTION_GATE_STREAM_HOLD_MODE";

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
        };
        match run_ai_attempt_loop(&port, plan_and_reports).await? {
            AiAttemptLoopOutcome::Responded(response) => {
                Ok(LocalExecutionRequestOutcome::responded(response))
            }
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
    mut source: S,
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
        };
        run_dynamic_attempt_loop(
            &port,
            &mut source,
            trace_id,
            plan_kind,
            state
                .frontdoor_runtime_guards
                .local_execution_planning_timeout,
        )
        .await
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
}

#[async_trait]
impl<T> AiAttemptLoopPort<T> for SyncAttemptLoopPort<'_>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
{
    type Response = Response<Body>;
    type Exhaustion = crate::executor::LocalExecutionExhaustion;
    type Error = GatewayError;

    async fn execute_attempt(&self, attempt: &T) -> Result<Option<Self::Response>, Self::Error> {
        let plan = attempt.execution_plan();
        let report_context = attempt.report_context();
        if let Some(response) = execution_plan_balance_capacity_response(
            self.state,
            self.trace_id,
            self.decision,
            plan,
            report_context.as_ref(),
        )
        .await?
        {
            return Ok(Some(response));
        }
        prewarm_direct_reqwest_candidate_client(plan);
        let _permit = acquire_upstream_execution_gate(self.state, self.trace_id).await?;
        let upstream_execution_gate_held_started_at = std::time::Instant::now();
        let mut response = execute_execution_runtime_sync(
            self.state,
            self.parts.uri.path(),
            plan.clone(),
            self.trace_id,
            self.decision,
            self.plan_kind,
            attempt.report_kind(),
            report_context,
        )
        .await?;
        observe_gateway_stage_ms(
            "upstream_execution_gate_held",
            upstream_execution_gate_held_started_at
                .elapsed()
                .as_millis() as u64,
        );
        if let Some(response) = response.as_mut() {
            attach_redaction_execution_candidate(response, plan.candidate_id.as_deref());
        }
        Ok(response)
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
        Ok(
            build_local_execution_exhaustion(self.state, &last_plan, last_report_context.as_ref())
                .await,
        )
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
        };
        match run_ai_attempt_loop(&port, plan_and_reports).await? {
            AiAttemptLoopOutcome::Responded(response) => {
                Ok(LocalExecutionRequestOutcome::responded(response))
            }
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
    mut source: S,
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
        };
        run_dynamic_attempt_loop(
            &port,
            &mut source,
            trace_id,
            plan_kind,
            state
                .frontdoor_runtime_guards
                .local_execution_planning_timeout,
        )
        .await
    }
    .instrument(span)
    .await
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

    loop {
        let next_started_at = std::time::Instant::now();
        let next_attempt =
            next_execution_attempt_with_timeout(source, trace_id, plan_kind, planning_timeout)
                .await?;
        observe_gateway_stage_ms(
            "stream_candidate_next",
            next_started_at.elapsed().as_millis() as u64,
        );
        let Some(attempt) = next_attempt else {
            break;
        };
        let execute_started_at = std::time::Instant::now();
        let response = match port.execute_attempt(&attempt).await {
            Ok(response) => response,
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
        if let Some(response) = response {
            let remaining = source.drain_execution_attempts().await?;
            let unused_started_at = std::time::Instant::now();
            port.mark_unused_attempts(remaining).await?;
            observe_gateway_stage_ms(
                "stream_candidate_unused",
                unused_started_at.elapsed().as_millis() as u64,
            );
            return Ok(LocalExecutionRequestOutcome::responded(response));
        }

        // Only retain a deep plan/context snapshot when this candidate really
        // failed and exhaustion reporting will need it.
        last_attempted = Some((attempt.execution_plan().clone(), attempt.report_context()));
    }

    let Some((last_plan, last_report_context)) = last_attempted else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    Ok(LocalExecutionRequestOutcome::Exhausted(
        port.build_exhaustion(last_plan, last_report_context)
            .await?,
    ))
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
}

#[async_trait]
impl<T> AiAttemptLoopPort<T> for StreamAttemptLoopPort<'_>
where
    T: AiExecutionAttempt + Send + Sync + 'static,
{
    type Response = Response<Body>;
    type Exhaustion = crate::executor::LocalExecutionExhaustion;
    type Error = GatewayError;

    async fn execute_attempt(&self, attempt: &T) -> Result<Option<Self::Response>, Self::Error> {
        let plan = attempt.execution_plan();
        let report_context = attempt.report_context();
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
        if let Some(response) = execution_plan_balance_capacity_response(
            self.state,
            self.trace_id,
            self.decision,
            plan,
            report_context.as_ref(),
        )
        .await?
        {
            return Ok(Some(response));
        }
        prewarm_direct_reqwest_candidate_client(plan);
        // The attempt owns the canonical report context. Borrow it for the
        // watchdog; only third-party/synthesized attempts using the default
        // trait implementation need an owned fallback clone.
        let watchdog_report_context_owned = if attempt.report_context_ref().is_none() {
            report_context.clone()
        } else {
            None
        };
        let watchdog_report_context = attempt
            .report_context_ref()
            .or(watchdog_report_context_owned.as_ref());
        let execution_state = self.state.clone();
        let execution_trace_id = self.trace_id.to_string();
        let execution_plan_kind = self.plan_kind.to_string();
        let execution_decision = self.decision.clone();
        let execution_report_kind = attempt.report_kind();
        let execution_plan = plan.clone();
        let mut response = execute_stream_candidate_with_watchdog(
            self.state,
            self.trace_id,
            self.plan_kind,
            plan,
            watchdog_report_context,
            move || async move {
                execute_execution_runtime_stream(
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
        .await?;
        if let Some(response) = response.as_mut() {
            attach_redaction_execution_candidate(response, plan.candidate_id.as_deref());
        }
        Ok(response)
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
        Ok(
            build_local_execution_exhaustion(self.state, &last_plan, last_report_context.as_ref())
                .await,
        )
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

async fn execute_stream_candidate_with_watchdog<Fut>(
    state: &(impl RequestCandidateRuntimeWriter + UpstreamExecutionGateProvider + ?Sized),
    trace_id: &str,
    plan_kind: &str,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    execute: impl FnOnce() -> Fut,
) -> Result<Option<Response<Body>>, GatewayError>
where
    Fut: std::future::Future<Output = Result<Option<Response<Body>>, GatewayError>> + Send,
{
    let timeout_duration = resolve_stream_candidate_watchdog_timeout(plan, report_context);
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
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    let permit_hold = permit.map(UpstreamExecutionPermitHold::new);
    let watchdog_started_at = std::time::Instant::now();
    let outcome = match timeout(timeout_duration, execute()).await {
        Ok(result) => result,
        Err(_) => {
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
                    status_code: Some(http::StatusCode::GATEWAY_TIMEOUT.as_u16()),
                    error_type: Some("local_stream_candidate_watchdog_timeout".to_string()),
                    error_message: Some(stream_candidate_watchdog_timeout_message().to_string()),
                    latency_ms: None,
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
            Ok(None)
        }
    };
    observe_gateway_stage_ms(
        "stream_candidate_watchdog_inline",
        watchdog_started_at.elapsed().as_millis() as u64,
    );
    match outcome {
        Ok(response) => Ok(maybe_hold_upstream_execution_permit(response, permit_hold)),
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
            Ok(None)
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

trait UpstreamExecutionGateProvider {
    fn upstream_execution_gate(&self) -> Option<&aether_runtime::ConcurrencyGate>;
    fn upstream_execution_gate_queue_budget(&self) -> Duration;
}

impl UpstreamExecutionGateProvider for AppState {
    fn upstream_execution_gate(&self) -> Option<&aether_runtime::ConcurrencyGate> {
        self.upstream_execution_gate.as_deref()
    }

    fn upstream_execution_gate_queue_budget(&self) -> Duration {
        self.frontdoor_runtime_guards.internal_gate_queue_budget
    }
}

async fn acquire_upstream_execution_gate(
    state: &(impl UpstreamExecutionGateProvider + ?Sized),
    trace_id: &str,
) -> Result<Option<ConcurrencyPermit>, GatewayError> {
    let Some(gate) = state.upstream_execution_gate() else {
        return Ok(None);
    };
    let budget = state.upstream_execution_gate_queue_budget();
    let gate_wait_started_at = std::time::Instant::now();
    match timeout(budget, gate.acquire()).await {
        Ok(Ok(permit)) => {
            observe_gateway_stage_ms(
                "upstream_execution_gate_wait",
                gate_wait_started_at.elapsed().as_millis() as u64,
            );
            Ok(Some(permit))
        }
        Ok(Err(err)) => Err(GatewayError::Internal(err.to_string())),
        Err(_) => Err(GatewayError::AdmissionTimeout {
            trace_id: trace_id.to_string(),
            gate: UPSTREAM_EXECUTION_GATE_NAME,
            queue_budget_ms: budget.as_millis() as u64,
        }),
    }
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
    use std::sync::Arc;

    use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, UpsertRequestCandidateRecord,
    };
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
                || std::future::pending::<Result<Option<Response<Body>>, GatewayError>>(),
            )
            .await
        });

        tokio::time::sleep(Duration::from_millis(40)).await;
        let result = task.await.expect("watchdog task should join");
        assert!(matches!(result, Ok(None)));

        let records = writer.records.lock().await;
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.status, RequestCandidateStatus::Failed);
        assert_eq!(
            record.status_code,
            Some(http::StatusCode::GATEWAY_TIMEOUT.as_u16())
        );
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
            || async {
                panic!("execute future should not run while upstream execution gate is saturated")
            },
        )
        .await;

        assert!(matches!(result, Ok(None)));
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
            || async {
                Err(GatewayError::AdmissionTimeout {
                    trace_id: "trace_target_admission".to_string(),
                    gate: UPSTREAM_TARGET_GATE_NAME,
                    queue_budget_ms: 5,
                })
            },
        )
        .await;

        assert!(matches!(result, Ok(None)));
        assert!(writer.records.lock().await.is_empty());
    }
}
