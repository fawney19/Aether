use std::collections::BTreeSet;

use aether_ai_serving::{
    run_ai_attempt_loop, AiAttemptLoopOutcome, AiAttemptLoopPort, AiExecutionAttempt,
};
use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::{
    parse_request_candidate_report_context, SchedulerRequestCandidateStatusUpdate,
};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::Response;
use tokio::time::{timeout, Duration};
use tracing::{debug, info, warn, Instrument};

use crate::ai_serving::LocalExecutionAttemptSource;
use crate::cache::{CachedPoolCandidates, PoolCandidateCacheKey};
use crate::clock::current_unix_ms;
use crate::control::GatewayControlDecision;
use crate::execution_runtime::{execute_execution_runtime_stream, execute_execution_runtime_sync};
use crate::executor::{build_local_execution_exhaustion, LocalExecutionRequestOutcome};
use crate::handlers::shared::provider_pool::admin_provider_pool_config_from_config_value;
use crate::log_ids::short_request_id;
use crate::maintenance::pool_preheat_probe_provider_is_supported;
use crate::orchestration::local_execution_candidate_metadata_from_report_context;
use crate::request_candidate_runtime::{
    record_local_request_candidate_status, RequestCandidateRuntimeWriter,
};
use crate::task_runtime::{spawn_fire_and_forget, submit_pool_preheat_probe_task};
use crate::{AppState, GatewayError};

const DEFAULT_STREAM_CANDIDATE_WATCHDOG_TIMEOUT_MS: u64 = 300_000;
const CANDIDATE_POOL_PREHEAT_HOOK_TASK: &str = "candidate-loop-pool-preheat-probe-hook";

fn candidate_loop_gateway_error_kind(error: &GatewayError) -> &'static str {
    match error {
        GatewayError::UpstreamUnavailable { .. } => "upstream_unavailable",
        GatewayError::ControlUnavailable { .. } => "control_unavailable",
        GatewayError::Internal(_) => "internal",
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
        run_dynamic_attempt_loop(Some(state), trace_id, plan_kind, &port, &mut source).await
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
        let plan = attempt.execution_plan().clone();
        let report_kind = attempt.report_kind();
        let report_context = attempt.report_context();
        maybe_spawn_candidate_pool_preheat_probe_hook(self.state, &plan, report_context.as_ref());
        execute_execution_runtime_sync(
            self.state,
            self.parts.uri.path(),
            plan,
            self.trace_id,
            self.decision,
            self.plan_kind,
            report_kind,
            report_context,
        )
        .await
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
        run_dynamic_attempt_loop(Some(state), trace_id, plan_kind, &port, &mut source).await
    }
    .instrument(span)
    .await
}

async fn run_dynamic_attempt_loop<Port, Source, Attempt>(
    state: Option<&AppState>,
    trace_id: &str,
    plan_kind: &str,
    port: &Port,
    source: &mut Source,
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
    let mut attempted_key_ids = BTreeSet::new();

    while let Some(attempt) = source.next_execution_attempt().await? {
        let plan = attempt.execution_plan().clone();
        let report_context = attempt.report_context();
        attempted_key_ids.insert(plan.key_id.clone());
        last_attempted = Some((plan.clone(), report_context.clone()));
        if let Some(response) = port.execute_attempt(&attempt).await? {
            let remaining = source.drain_execution_attempts().await?;
            port.mark_unused_attempts(remaining).await?;
            return Ok(LocalExecutionRequestOutcome::responded(response));
        }
        if let Some(pool_group_id) = hedge_swap_pool_group_id(report_context.as_ref()) {
            if let Some((previous_next_key_id, next_key_id, promoted_key_ids)) = source
                .promote_hedge_preheated_pool_candidates(
                    plan.provider_id.as_str(),
                    pool_group_id.as_str(),
                    &attempted_key_ids,
                )
                .await?
            {
                if let Some(state) = state {
                    state.pool_preheat_metrics().record_hedge_swap("fast_fail");
                }
                info!(
                    event_name = "hedge_swap",
                    event = "hedge_swap",
                    log_type = "event",
                    worker = "candidate_loop",
                    trace_id = %trace_id,
                    plan_kind,
                    reason = "fast_fail",
                    request_id = %short_request_id(plan.request_id.as_str()),
                    provider_id = %plan.provider_id,
                    pool_group_id = %pool_group_id,
                    from_key_id = %plan.key_id,
                    to_key_id = %next_key_id,
                    failed_key_id = %plan.key_id,
                    previous_next_key_id = previous_next_key_id.as_deref().unwrap_or("-"),
                    next_key_id = %next_key_id,
                    promoted_key_ids = ?promoted_key_ids,
                    "gateway hedge swap promoted preheated pool candidates after fast failure"
                );
            }
        }
    }

    let Some((last_plan, last_report_context)) = last_attempted else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    Ok(LocalExecutionRequestOutcome::Exhausted(
        port.build_exhaustion(last_plan, last_report_context)
            .await?,
    ))
}

fn hedge_swap_pool_group_id(report_context: Option<&serde_json::Value>) -> Option<String> {
    if !report_context_hedge_eligible(report_context) {
        return None;
    }
    let metadata = local_execution_candidate_metadata_from_report_context(report_context);
    if metadata.pool_key_index != Some(0) {
        return None;
    }
    if parse_request_candidate_report_context(report_context)?.retry_index != 0 {
        return None;
    }

    let pool_group_id = metadata.candidate_group_id?.trim().to_string();
    (!pool_group_id.is_empty()).then_some(pool_group_id)
}

fn report_context_hedge_eligible(report_context: Option<&serde_json::Value>) -> bool {
    report_context
        .and_then(|value| value.get("hedge_eligible"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || report_context
            .and_then(|value| value.get("error_flow"))
            .and_then(|value| value.get("hedge_eligible"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
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
        let plan = attempt.execution_plan().clone();
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
        let watchdog_plan = plan.clone();
        let watchdog_report_context = report_context.clone();
        let execution_state = self.state.clone();
        let execution_trace_id = self.trace_id.to_string();
        let execution_plan_kind = self.plan_kind.to_string();
        let execution_decision = self.decision.clone();
        let execution_report_kind = attempt.report_kind();
        maybe_spawn_candidate_pool_preheat_probe_hook(
            self.state,
            &watchdog_plan,
            watchdog_report_context.as_ref(),
        );
        execute_stream_candidate_with_watchdog(
            self.state,
            self.trace_id,
            self.plan_kind,
            &watchdog_plan,
            watchdog_report_context.as_ref(),
            move || async move {
                execute_execution_runtime_stream(
                    &execution_state,
                    plan,
                    execution_trace_id.as_str(),
                    &execution_decision,
                    execution_plan_kind.as_str(),
                    execution_report_kind,
                    report_context,
                )
                .await
            },
        )
        .await
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidatePoolPreheatProbeHookInput {
    provider_id: String,
    pool_group_id: String,
    current_key_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CandidatePoolPreheatProbeSubmission {
    provider_id: String,
    pool_group_id: String,
    top_n_key_ids: Vec<String>,
}

fn maybe_spawn_candidate_pool_preheat_probe_hook(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) {
    let Some(input) = candidate_pool_preheat_probe_hook_input(plan, report_context) else {
        return;
    };
    let app = state.clone();
    spawn_candidate_pool_preheat_probe_hook_task(async move {
        let provider_id = input.provider_id.clone();
        let pool_group_id = input.pool_group_id.clone();
        match run_candidate_pool_preheat_probe_hook_once_with_input(&app, input).await {
            Ok(Some(run_id)) => {
                debug!(
                    event_name = "candidate_pool_preheat_probe_hook_submitted",
                    log_type = "event",
                    provider_id = %provider_id,
                    pool_group_id = %pool_group_id,
                    run_id = %run_id,
                    "candidate pool preheat probe hook submitted"
                );
            }
            Ok(None) => {
                debug!(
                    event_name = "candidate_pool_preheat_probe_hook_skipped",
                    log_type = "event",
                    provider_id = %provider_id,
                    pool_group_id = %pool_group_id,
                    "candidate pool preheat probe hook skipped"
                );
            }
            Err(error) => {
                debug!(
                    event_name = "candidate_pool_preheat_probe_hook_failed",
                    log_type = "event",
                    provider_id = %provider_id,
                    pool_group_id = %pool_group_id,
                    error_kind = candidate_loop_gateway_error_kind(&error),
                    "candidate pool preheat probe hook failed"
                );
            }
        }
    });
}

fn spawn_candidate_pool_preheat_probe_hook_task(
    future: impl std::future::Future<Output = ()> + Send + 'static,
) {
    spawn_fire_and_forget(CANDIDATE_POOL_PREHEAT_HOOK_TASK, future);
}

fn candidate_pool_preheat_probe_hook_input(
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) -> Option<CandidatePoolPreheatProbeHookInput> {
    let metadata = local_execution_candidate_metadata_from_report_context(report_context);
    if metadata.pool_key_index != Some(0) {
        return None;
    }
    if parse_request_candidate_report_context(report_context)?.retry_index != 0 {
        return None;
    }

    let provider_id = plan.provider_id.trim();
    let current_key_id = plan.key_id.trim();
    let pool_group_id = metadata.candidate_group_id?.trim().to_string();
    if provider_id.is_empty() || current_key_id.is_empty() || pool_group_id.is_empty() {
        return None;
    }

    Some(CandidatePoolPreheatProbeHookInput {
        provider_id: provider_id.to_string(),
        pool_group_id,
        current_key_id: current_key_id.to_string(),
    })
}

async fn run_candidate_pool_preheat_probe_hook_once_with_input(
    app: &AppState,
    input: CandidatePoolPreheatProbeHookInput,
) -> Result<Option<String>, GatewayError> {
    let Some(submission) = build_candidate_pool_preheat_probe_submission(app, input).await? else {
        return Ok(None);
    };
    submit_pool_preheat_probe_task(
        app,
        &submission.provider_id,
        &submission.pool_group_id,
        submission.top_n_key_ids,
    )
    .await
}

#[cfg(test)]
async fn run_candidate_pool_preheat_probe_hook_once(
    app: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
) -> Result<Option<String>, GatewayError> {
    let Some(input) = candidate_pool_preheat_probe_hook_input(plan, report_context) else {
        return Ok(None);
    };
    run_candidate_pool_preheat_probe_hook_once_with_input(app, input).await
}

async fn build_candidate_pool_preheat_probe_submission(
    app: &AppState,
    input: CandidatePoolPreheatProbeHookInput,
) -> Result<Option<CandidatePoolPreheatProbeSubmission>, GatewayError> {
    if !app.has_provider_catalog_data_reader() {
        return Ok(None);
    }
    let Some(provider) = app
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&input.provider_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    if !pool_preheat_probe_provider_is_supported(&provider.provider_type) {
        return Ok(None);
    }
    let Some(pool_config) = admin_provider_pool_config_from_config_value(provider.config.as_ref())
    else {
        return Ok(None);
    };
    if !pool_config.pre_probe.enabled {
        return Ok(None);
    }
    let Some(cache_key) =
        PoolCandidateCacheKey::from_pool_group_id(&input.provider_id, &input.pool_group_id)
    else {
        return Ok(None);
    };
    let current_config_version = app.effective_pool_candidate_config_version().await;
    let cache_ttl = Duration::from_secs(pool_config.pre_probe.cache_ttl_seconds.max(1));
    let Some(cached) =
        app.pool_candidate_cache()
            .get_fresh(&cache_key, cache_ttl, current_config_version)
    else {
        app.pool_preheat_metrics()
            .record_candidate_cache_operation("miss");
        return Ok(None);
    };
    app.pool_preheat_metrics()
        .record_candidate_cache_operation("hit");
    let top_n_key_ids = collect_candidate_pool_preheat_probe_key_ids(
        cached,
        &input.current_key_id,
        pool_config.pre_probe.top_n,
    );
    if top_n_key_ids.is_empty() {
        return Ok(None);
    }

    Ok(Some(CandidatePoolPreheatProbeSubmission {
        provider_id: input.provider_id,
        pool_group_id: input.pool_group_id,
        top_n_key_ids,
    }))
}

fn collect_candidate_pool_preheat_probe_key_ids(
    cached: CachedPoolCandidates,
    current_key_id: &str,
    top_n: u32,
) -> Vec<String> {
    let current_key_id = current_key_id.trim();
    let mut seen = BTreeSet::new();
    cached
        .rows
        .into_iter()
        .filter_map(|row| {
            let key_id = row.key_id.trim().to_string();
            if key_id.is_empty() || key_id == current_key_id || !seen.insert(key_id.clone()) {
                None
            } else {
                Some(key_id)
            }
        })
        .take(top_n.max(1) as usize)
        .collect()
}

pub(crate) async fn mark_unused_local_candidates<T>(state: &AppState, remaining: Vec<T>)
where
    T: AiExecutionAttempt,
{
    for plan_and_report in remaining {
        let report_context = plan_and_report.report_context();
        if should_skip_unused_persistence(report_context.as_ref()) {
            continue;
        }
        record_local_request_candidate_status(
            state,
            plan_and_report.execution_plan(),
            report_context.as_ref(),
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

fn should_skip_unused_persistence(report_context: Option<&serde_json::Value>) -> bool {
    let metadata = local_execution_candidate_metadata_from_report_context(report_context);
    metadata.candidate_group_id.is_some() && metadata.pool_key_index.is_some()
}

fn resolve_stream_candidate_watchdog_timeout(plan: &aether_contracts::ExecutionPlan) -> Duration {
    let timeout_ms = plan
        .timeouts
        .as_ref()
        .and_then(|timeouts| timeouts.first_byte_ms.or(timeouts.total_ms))
        .unwrap_or(DEFAULT_STREAM_CANDIDATE_WATCHDOG_TIMEOUT_MS)
        .max(1);
    Duration::from_millis(timeout_ms)
}

async fn execute_stream_candidate_with_watchdog<Fut>(
    state: &(impl RequestCandidateRuntimeWriter + ?Sized),
    trace_id: &str,
    plan_kind: &str,
    plan: &aether_contracts::ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    execute: impl FnOnce() -> Fut,
) -> Result<Option<Response<Body>>, GatewayError>
where
    Fut:
        std::future::Future<Output = Result<Option<Response<Body>>, GatewayError>> + Send + 'static,
{
    let timeout_duration = resolve_stream_candidate_watchdog_timeout(plan);
    let candidate_started_unix_ms = current_unix_ms();
    let mut join_handle = tokio::spawn(execute());
    match timeout(timeout_duration, &mut join_handle).await {
        Ok(Ok(result)) => result,
        Ok(Err(join_error)) => Err(GatewayError::Internal(format!(
            "local stream candidate task join failed: {join_error}"
        ))),
        Err(_) => {
            join_handle.abort();
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
                    error_message: Some(format!(
                        "local stream candidate attempt exceeded watchdog timeout of {timeout_ms}ms"
                    )),
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
    use std::collections::VecDeque;
    use std::sync::Arc;

    use aether_ai_serving::AiExecutionAttempt;
    use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
    use aether_data::repository::background_tasks::InMemoryBackgroundTaskRepository;
    use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
    use aether_data_contracts::repository::candidate_selection::StoredMinimalCandidateSelectionRow;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, UpsertRequestCandidateRecord,
    };
    use aether_data_contracts::repository::provider_catalog::{
        StoredProviderCatalogEndpoint, StoredProviderCatalogProvider,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::{oneshot, Mutex};

    use super::*;

    const PREHEAT_POOL_GROUP_ID: &str = "provider=provider-preheat|endpoint=endpoint-preheat|model=model-preheat|selected_model=gpt-5|api_format=openai:responses|singleton_key=*";

    #[derive(Debug, Default)]
    struct TestRequestCandidateWriter {
        records: Mutex<Vec<UpsertRequestCandidateRecord>>,
    }

    #[derive(Debug, Clone)]
    struct TestDynamicAttempt {
        plan: ExecutionPlan,
        report_context: Option<serde_json::Value>,
    }

    impl AiExecutionAttempt for TestDynamicAttempt {
        fn execution_plan(&self) -> &ExecutionPlan {
            &self.plan
        }

        fn report_kind(&self) -> Option<String> {
            None
        }

        fn report_context(&self) -> Option<serde_json::Value> {
            self.report_context.clone()
        }
    }

    struct TestDynamicAttemptSource {
        attempts: VecDeque<TestDynamicAttempt>,
        promoted_key_id: &'static str,
        promote_calls: usize,
    }

    #[async_trait]
    impl LocalExecutionAttemptSource<TestDynamicAttempt> for TestDynamicAttemptSource {
        async fn next_execution_attempt(
            &mut self,
        ) -> Result<Option<TestDynamicAttempt>, GatewayError> {
            Ok(self.attempts.pop_front())
        }

        async fn drain_execution_attempts(
            &mut self,
        ) -> Result<Vec<TestDynamicAttempt>, GatewayError> {
            Ok(self.attempts.drain(..).collect())
        }

        async fn promote_hedge_preheated_pool_candidates(
            &mut self,
            provider_id: &str,
            pool_group_id: &str,
            attempted_key_ids: &BTreeSet<String>,
        ) -> Result<Option<(Option<String>, String, Vec<String>)>, GatewayError> {
            self.promote_calls += 1;
            if provider_id != "provider-preheat"
                || pool_group_id != PREHEAT_POOL_GROUP_ID
                || !attempted_key_ids.contains("key-1")
            {
                return Ok(None);
            }
            let previous_next_key_id = self
                .attempts
                .front()
                .map(|attempt| attempt.plan.key_id.clone());
            let Some(position) = self
                .attempts
                .iter()
                .position(|attempt| attempt.plan.key_id == self.promoted_key_id)
            else {
                return Ok(None);
            };
            let promoted = self
                .attempts
                .remove(position)
                .expect("promoted key position should remain valid");
            let next_key_id = promoted.plan.key_id.clone();
            self.attempts.push_front(promoted);
            Ok(Some((
                previous_next_key_id,
                next_key_id,
                vec![self.promoted_key_id.to_string()],
            )))
        }
    }

    struct TestDynamicAttemptPort {
        state: AppState,
        responding_key_id: &'static str,
        attempted_key_ids: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl AiAttemptLoopPort<TestDynamicAttempt> for TestDynamicAttemptPort {
        type Response = Response<Body>;
        type Exhaustion = crate::executor::LocalExecutionExhaustion;
        type Error = GatewayError;

        async fn execute_attempt(
            &self,
            attempt: &TestDynamicAttempt,
        ) -> Result<Option<Self::Response>, Self::Error> {
            let key_id = attempt.execution_plan().key_id.clone();
            self.attempted_key_ids.lock().await.push(key_id.clone());
            if key_id == self.responding_key_id {
                Ok(Some(Response::new(Body::empty())))
            } else {
                Ok(None)
            }
        }

        async fn mark_unused_attempts(
            &self,
            _attempts: Vec<TestDynamicAttempt>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn build_exhaustion(
            &self,
            last_plan: ExecutionPlan,
            last_report_context: Option<serde_json::Value>,
        ) -> Result<Self::Exhaustion, Self::Error> {
            Ok(build_local_execution_exhaustion(
                &self.state,
                &last_plan,
                last_report_context.as_ref(),
            )
            .await)
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

    fn preheat_provider(
        provider_type: &str,
        pre_probe_enabled: bool,
        top_n: u32,
    ) -> StoredProviderCatalogProvider {
        StoredProviderCatalogProvider::new(
            "provider-preheat".to_string(),
            "preheat".to_string(),
            Some("https://example.com".to_string()),
            provider_type.to_string(),
        )
        .expect("provider should build")
        .with_transport_fields(
            true,
            false,
            false,
            None,
            Some(1),
            None,
            None,
            None,
            Some(json!({
                "pool_advanced": {
                    "pre_probe": {
                        "enabled": pre_probe_enabled,
                        "top_n": top_n,
                        "required_healthy": 1,
                        "cache_ttl_seconds": 300
                    }
                }
            })),
        )
    }

    fn preheat_endpoint() -> StoredProviderCatalogEndpoint {
        StoredProviderCatalogEndpoint::new(
            "endpoint-preheat".to_string(),
            "provider-preheat".to_string(),
            "openai:responses".to_string(),
            Some("openai".to_string()),
            Some("responses".to_string()),
            true,
        )
        .expect("endpoint should build")
        .with_transport_fields(
            "https://example.com".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("endpoint transport should build")
    }

    fn preheat_state(provider_type: &str, pre_probe_enabled: bool, top_n: u32) -> AppState {
        let provider_catalog_repository = Arc::new(InMemoryProviderCatalogReadRepository::seed(
            vec![preheat_provider(provider_type, pre_probe_enabled, top_n)],
            vec![preheat_endpoint()],
            vec![],
        ));
        let background_task_repository = Arc::new(InMemoryBackgroundTaskRepository::default());
        AppState::new()
            .expect("gateway state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_provider_catalog_repository_for_tests(
                    provider_catalog_repository,
                )
                .attach_background_task_repository_for_tests(background_task_repository),
            )
    }

    fn preheat_cache_key() -> PoolCandidateCacheKey {
        PoolCandidateCacheKey::new(
            "provider-preheat",
            "endpoint-preheat",
            "model-preheat",
            "gpt-5",
            "openai:responses",
        )
    }

    fn preheat_candidate_row(key_id: &str) -> StoredMinimalCandidateSelectionRow {
        StoredMinimalCandidateSelectionRow {
            provider_id: "provider-preheat".to_string(),
            provider_name: "preheat".to_string(),
            provider_type: "codex".to_string(),
            provider_priority: 10,
            provider_is_active: true,
            endpoint_id: "endpoint-preheat".to_string(),
            endpoint_api_format: "openai:responses".to_string(),
            endpoint_api_family: Some("openai".to_string()),
            endpoint_kind: Some("responses".to_string()),
            endpoint_is_active: true,
            key_id: key_id.to_string(),
            key_name: key_id.to_string(),
            key_auth_type: "oauth".to_string(),
            key_is_active: true,
            key_api_formats: Some(vec!["openai:responses".to_string()]),
            key_allowed_models: None,
            key_capabilities: None,
            key_internal_priority: 1,
            key_global_priority_by_format: None,
            model_id: "model-preheat".to_string(),
            global_model_id: "global-model-preheat".to_string(),
            global_model_name: "gpt-5".to_string(),
            global_model_mappings: None,
            global_model_supports_streaming: Some(true),
            model_provider_model_name: "gpt-5".to_string(),
            model_provider_model_mappings: None,
            model_supports_streaming: Some(true),
            model_is_active: true,
            model_is_available: true,
        }
    }

    fn insert_preheat_cache(state: &AppState, key_ids: &[&str]) {
        let rows = key_ids
            .iter()
            .map(|key_id| preheat_candidate_row(key_id))
            .collect::<Vec<_>>();
        state.pool_candidate_cache().insert(
            preheat_cache_key(),
            CachedPoolCandidates::new(
                rows,
                state.pool_candidate_config_version(),
                key_ids.len() as u32,
            ),
            Duration::from_secs(300),
            16,
            16,
        );
    }

    fn preheat_plan(key_id: &str) -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req_preheat".to_string(),
            candidate_id: Some("cand_preheat".to_string()),
            provider_name: Some("preheat".to_string()),
            provider_id: "provider-preheat".to_string(),
            endpoint_id: "endpoint-preheat".to_string(),
            key_id: key_id.to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/responses".to_string(),
            headers: Default::default(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: "openai:responses".to_string(),
            provider_api_format: "openai:responses".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn preheat_report_context(pool_key_index: Option<u32>, retry_index: u32) -> serde_json::Value {
        let mut context = json!({
            "request_id": "req_preheat",
            "candidate_id": "cand_preheat",
            "candidate_index": 0,
            "retry_index": retry_index,
            "candidate_group_id": PREHEAT_POOL_GROUP_ID,
        });
        if let Some(pool_key_index) = pool_key_index {
            context["pool_key_index"] = json!(pool_key_index);
        }
        context
    }

    fn hedge_swap_report_context(hedge_eligible: bool, pool: bool) -> Option<serde_json::Value> {
        let mut context = json!({
            "request_id": "req_preheat",
            "candidate_id": "cand_preheat",
            "candidate_index": 0,
            "retry_index": 0,
        });
        if hedge_eligible {
            context["hedge_eligible"] = json!(true);
        }
        if pool {
            context["candidate_group_id"] = json!(PREHEAT_POOL_GROUP_ID);
            context["pool_key_index"] = json!(0);
        }
        Some(context)
    }

    fn dynamic_attempt(
        key_id: &str,
        report_context: Option<serde_json::Value>,
    ) -> TestDynamicAttempt {
        TestDynamicAttempt {
            plan: preheat_plan(key_id),
            report_context,
        }
    }

    async fn run_test_dynamic_loop(
        first_report_context: Option<serde_json::Value>,
        responding_key_id: &'static str,
        promoted_key_id: &'static str,
    ) -> (Vec<String>, usize) {
        let attempted_key_ids = Arc::new(Mutex::new(Vec::new()));
        let port = TestDynamicAttemptPort {
            state: AppState::new().expect("state should build"),
            responding_key_id,
            attempted_key_ids: attempted_key_ids.clone(),
        };
        let mut source = TestDynamicAttemptSource {
            attempts: VecDeque::from([
                dynamic_attempt("key-1", first_report_context),
                dynamic_attempt("key-2", hedge_swap_report_context(false, true)),
                dynamic_attempt("key-3", hedge_swap_report_context(false, true)),
            ]),
            promoted_key_id,
            promote_calls: 0,
        };

        let outcome = run_dynamic_attempt_loop(
            Some(&port.state),
            "trace-hedge-swap",
            "openai_responses_sync",
            &port,
            &mut source,
        )
        .await
        .expect("dynamic loop should succeed");
        assert!(matches!(
            outcome,
            LocalExecutionRequestOutcome::Responded(_)
        ));
        let attempts = attempted_key_ids.lock().await.clone();
        (attempts, source.promote_calls)
    }

    #[tokio::test]
    async fn hedge_swap_fast_fail_promotes_stamped_key_next() {
        let (attempts, promote_calls) =
            run_test_dynamic_loop(hedge_swap_report_context(true, true), "key-3", "key-3").await;

        assert_eq!(attempts, vec!["key-1", "key-3"]);
        assert_eq!(promote_calls, 1);
    }

    #[tokio::test]
    async fn hedge_swap_no_healthy_stamp_preserves_original_order() {
        let (attempts, promote_calls) = run_test_dynamic_loop(
            hedge_swap_report_context(true, true),
            "key-2",
            "missing-key",
        )
        .await;

        assert_eq!(attempts, vec!["key-1", "key-2"]);
        assert_eq!(promote_calls, 1);
    }

    #[tokio::test]
    async fn hedge_swap_non_pool_or_non_hedge_path_does_not_reorder() {
        let (non_hedge_attempts, non_hedge_promote_calls) =
            run_test_dynamic_loop(hedge_swap_report_context(false, true), "key-2", "key-3").await;
        let (non_pool_attempts, non_pool_promote_calls) =
            run_test_dynamic_loop(hedge_swap_report_context(true, false), "key-2", "key-3").await;

        assert_eq!(non_hedge_attempts, vec!["key-1", "key-2"]);
        assert_eq!(non_pool_attempts, vec!["key-1", "key-2"]);
        assert_eq!(non_hedge_promote_calls, 0);
        assert_eq!(non_pool_promote_calls, 0);
    }

    #[test]
    fn stream_candidate_watchdog_prefers_first_byte_timeout() {
        let timeout =
            resolve_stream_candidate_watchdog_timeout(&test_plan(Some(ExecutionTimeouts {
                first_byte_ms: Some(12_345),
                total_ms: Some(90_000),
                ..ExecutionTimeouts::default()
            })));

        assert_eq!(timeout, Duration::from_millis(12_345));
    }

    #[test]
    fn stream_candidate_watchdog_uses_default_when_timeouts_missing() {
        let timeout = resolve_stream_candidate_watchdog_timeout(&test_plan(None));

        assert_eq!(
            timeout,
            Duration::from_millis(DEFAULT_STREAM_CANDIDATE_WATCHDOG_TIMEOUT_MS)
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

    mod oauth_pool_preheat_integration {
        use super::*;
        use base64::Engine as _;

        #[tokio::test]
        async fn q4_probe_starts_before_k1_first_byte_and_concurrent_with_dispatch() {
            let (started_tx, started_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let (completed_tx, mut completed_rx) = oneshot::channel();
            let (first_byte_tx, mut first_byte_rx) = oneshot::channel();

            spawn_candidate_pool_preheat_probe_hook_task(async move {
                let _ = started_tx.send(());
                let _ = release_rx.await;
                let _ = completed_tx.send(());
            });

            started_rx
                .await
                .expect("preheat hook should start before K1 first byte");
            first_byte_tx
                .send(())
                .expect("K1 first byte should be observable after hook start");
            assert!(matches!(first_byte_rx.try_recv(), Ok(())));
            assert!(matches!(
                completed_rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            release_tx.send(()).expect("hook release should send");
            completed_rx
                .await
                .expect("hook should complete after release");
        }

        #[tokio::test]
        async fn q8_hedge_swap_on_k1_401_saves_request() {
            let mut context =
                hedge_swap_report_context(false, true).expect("base report context should build");
            context = crate::executor::with_hedge_eligibility_report_context(
                Some(&context),
                crate::executor::hedge_fast_failure_status_tag(401)
                    .expect("401 should be hedge eligible"),
            )
            .expect("401 report context should be tagged");

            let (attempts, promote_calls) =
                run_test_dynamic_loop(Some(context), "key-3", "key-3").await;

            assert_eq!(attempts, vec!["key-1", "key-3"]);
            assert_eq!(promote_calls, 1);
        }

        #[tokio::test]
        async fn q9_hedge_no_healthy_falls_back_original_loop() {
            let (attempts, promote_calls) = run_test_dynamic_loop(
                hedge_swap_report_context(true, true),
                "key-2",
                "missing-key",
            )
            .await;

            assert_eq!(attempts, vec!["key-1", "key-2"]);
            assert_eq!(promote_calls, 1);
        }

        #[tokio::test]
        async fn q10_body_1mb_eligibility_check() {
            struct BodyReplayPort {
                state: AppState,
                responding_key_id: &'static str,
                observed_bodies: Arc<Mutex<Vec<(String, usize)>>>,
            }

            #[async_trait]
            impl AiAttemptLoopPort<TestDynamicAttempt> for BodyReplayPort {
                type Response = Response<Body>;
                type Exhaustion = crate::executor::LocalExecutionExhaustion;
                type Error = GatewayError;

                async fn execute_attempt(
                    &self,
                    attempt: &TestDynamicAttempt,
                ) -> Result<Option<Self::Response>, Self::Error> {
                    let plan = attempt.execution_plan();
                    let body_len = plan
                        .body
                        .body_bytes_b64
                        .as_deref()
                        .map(|body| {
                            base64::engine::general_purpose::STANDARD
                                .decode(body)
                                .expect("body replay payload should decode")
                                .len()
                        })
                        .unwrap_or_default();
                    self.observed_bodies
                        .lock()
                        .await
                        .push((plan.key_id.clone(), body_len));
                    if plan.key_id == self.responding_key_id {
                        Ok(Some(Response::new(Body::empty())))
                    } else {
                        Ok(None)
                    }
                }

                async fn mark_unused_attempts(
                    &self,
                    _attempts: Vec<TestDynamicAttempt>,
                ) -> Result<(), Self::Error> {
                    Ok(())
                }

                async fn build_exhaustion(
                    &self,
                    last_plan: ExecutionPlan,
                    last_report_context: Option<serde_json::Value>,
                ) -> Result<Self::Exhaustion, Self::Error> {
                    Ok(build_local_execution_exhaustion(
                        &self.state,
                        &last_plan,
                        last_report_context.as_ref(),
                    )
                    .await)
                }
            }

            fn replay_attempt(
                key_id: &str,
                report_context: Option<serde_json::Value>,
                body_bytes_b64: &str,
            ) -> TestDynamicAttempt {
                let mut attempt = dynamic_attempt(key_id, report_context);
                attempt.plan.body = RequestBody {
                    json_body: None,
                    body_bytes_b64: Some(body_bytes_b64.to_string()),
                    body_ref: None,
                };
                attempt
            }

            let body = vec![b'x'; 1024 * 1024];
            let body_bytes_b64 = base64::engine::general_purpose::STANDARD.encode(&body);
            let mut context =
                hedge_swap_report_context(false, true).expect("base report context should build");
            context = crate::executor::with_hedge_eligibility_report_context(
                Some(&context),
                crate::executor::hedge_fast_failure_status_tag(401)
                    .expect("401 should be hedge eligible"),
            )
            .expect("401 report context should be tagged");
            let observed_bodies = Arc::new(Mutex::new(Vec::new()));
            let port = BodyReplayPort {
                state: AppState::new().expect("state should build"),
                responding_key_id: "key-3",
                observed_bodies: observed_bodies.clone(),
            };
            let mut source = TestDynamicAttemptSource {
                attempts: VecDeque::from([
                    replay_attempt("key-1", Some(context), &body_bytes_b64),
                    replay_attempt(
                        "key-2",
                        hedge_swap_report_context(false, true),
                        &body_bytes_b64,
                    ),
                    replay_attempt(
                        "key-3",
                        hedge_swap_report_context(false, true),
                        &body_bytes_b64,
                    ),
                ]),
                promoted_key_id: "key-3",
                promote_calls: 0,
            };

            let outcome = run_dynamic_attempt_loop(
                Some(&port.state),
                "trace-body-replay",
                "openai_responses_sync",
                &port,
                &mut source,
            )
            .await
            .expect("body replay loop should succeed");

            assert!(matches!(
                outcome,
                LocalExecutionRequestOutcome::Responded(_)
            ));
            assert_eq!(source.promote_calls, 1);
            assert_eq!(
                observed_bodies.lock().await.as_slice(),
                &[
                    ("key-1".to_string(), 1024 * 1024),
                    ("key-3".to_string(), 1024 * 1024)
                ]
            );
        }
    }

    #[tokio::test]
    async fn candidate_pool_preheat_probe_hook_switch_off_does_not_submit() {
        let state = preheat_state("codex", false, 2);
        insert_preheat_cache(&state, &["key-1", "key-2", "key-3"]);
        let report_context = preheat_report_context(Some(0), 0);

        let run_id = run_candidate_pool_preheat_probe_hook_once(
            &state,
            &preheat_plan("key-1"),
            Some(&report_context),
        )
        .await
        .expect("hook should not error");

        assert!(run_id.is_none());
    }

    #[tokio::test]
    async fn candidate_pool_preheat_probe_hook_non_oauth_provider_does_not_submit() {
        let state = preheat_state("openai", true, 2);
        insert_preheat_cache(&state, &["key-1", "key-2", "key-3"]);
        let report_context = preheat_report_context(Some(0), 0);

        let run_id = run_candidate_pool_preheat_probe_hook_once(
            &state,
            &preheat_plan("key-1"),
            Some(&report_context),
        )
        .await
        .expect("hook should not error");

        assert!(run_id.is_none());
    }

    #[tokio::test]
    async fn candidate_pool_preheat_probe_hook_non_pool_candidate_does_not_submit() {
        let state = preheat_state("codex", true, 2);
        insert_preheat_cache(&state, &["key-1", "key-2", "key-3"]);
        let report_context = preheat_report_context(None, 0);

        let run_id = run_candidate_pool_preheat_probe_hook_once(
            &state,
            &preheat_plan("key-1"),
            Some(&report_context),
        )
        .await
        .expect("hook should not error");

        assert!(run_id.is_none());
    }

    #[tokio::test]
    async fn candidate_pool_preheat_probe_hook_missing_cache_does_not_submit() {
        let state = preheat_state("codex", true, 2);
        let report_context = preheat_report_context(Some(0), 0);

        let run_id = run_candidate_pool_preheat_probe_hook_once(
            &state,
            &preheat_plan("key-1"),
            Some(&report_context),
        )
        .await
        .expect("hook should not error");

        assert!(run_id.is_none());
    }

    #[tokio::test]
    async fn candidate_pool_preheat_probe_hook_oauth_enabled_submits() {
        let state = preheat_state("codex", true, 2);
        insert_preheat_cache(&state, &["key-1", "key-2", "key-3", "key-4"]);
        let report_context = preheat_report_context(Some(0), 0);

        let run_id = run_candidate_pool_preheat_probe_hook_once(
            &state,
            &preheat_plan("key-1"),
            Some(&report_context),
        )
        .await
        .expect("hook should not error")
        .expect("enabled OAuth pool should submit");
        let run = state
            .find_background_task_run(&run_id)
            .await
            .expect("run lookup should succeed")
            .expect("queued run should exist");

        assert_eq!(
            run.payload_json
                .as_ref()
                .and_then(|value| value.get("top_n_key_ids")),
            Some(&json!(["key-2", "key-3"]))
        );
    }

    #[tokio::test]
    async fn candidate_pool_preheat_probe_hook_spawn_does_not_await_probe_future() {
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let (completed_tx, mut completed_rx) = oneshot::channel();

        spawn_candidate_pool_preheat_probe_hook_task(async move {
            let _ = started_tx.send(());
            let _ = release_rx.await;
            let _ = completed_tx.send(());
        });

        started_rx.await.expect("hook task should start");
        assert!(matches!(
            completed_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        release_tx.send(()).expect("release should send");
        completed_rx.await.expect("hook task should complete");
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
            .is_some_and(|message| message.contains("25ms")));
        assert_eq!(record.candidate_index, 2);
    }
}
