use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::{
    parse_request_candidate_report_context, SchedulerRequestCandidateStatusUpdate,
};
use axum::body::Body;
use axum::http::Response;
use tokio::time::{timeout, Duration};
use tracing::{debug, warn, Instrument};

use crate::ai_pipeline_api::{LocalStreamPlanAndReport, LocalSyncPlanAndReport};
use crate::clock::current_unix_ms;
use crate::control::GatewayControlDecision;
use crate::execution_runtime::{execute_execution_runtime_stream, execute_execution_runtime_sync};
use crate::executor::{build_local_execution_exhaustion, LocalExecutionRequestOutcome};
use crate::log_ids::short_request_id;
use crate::orchestration::local_execution_candidate_metadata_from_report_context;
use crate::request_candidate_runtime::{
    record_local_request_candidate_status, RequestCandidateRuntimeWriter,
};
use crate::{AppState, GatewayError};

const DEFAULT_STREAM_CANDIDATE_WATCHDOG_TIMEOUT_MS: u64 = 300_000;

pub(crate) trait LocalPlanAndReport {
    fn plan(&self) -> &aether_contracts::ExecutionPlan;

    fn report_kind(&self) -> Option<String>;

    fn report_context(&self) -> Option<serde_json::Value>;
}

impl LocalPlanAndReport for LocalSyncPlanAndReport {
    fn plan(&self) -> &aether_contracts::ExecutionPlan {
        &self.plan
    }

    fn report_kind(&self) -> Option<String> {
        self.report_kind.clone()
    }

    fn report_context(&self) -> Option<serde_json::Value> {
        self.report_context.clone()
    }
}

impl LocalPlanAndReport for LocalStreamPlanAndReport {
    fn plan(&self) -> &aether_contracts::ExecutionPlan {
        &self.plan
    }

    fn report_kind(&self) -> Option<String> {
        self.report_kind.clone()
    }

    fn report_context(&self) -> Option<serde_json::Value> {
        self.report_context.clone()
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
    T: LocalPlanAndReport,
{
    let candidate_count = plan_and_reports.len();
    let first_provider = plan_and_reports
        .first()
        .and_then(|item| item.plan().provider_name.as_deref())
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

        let mut remaining = plan_and_reports.into_iter();
        let mut last_attempted = None;
        while let Some(plan_and_report) = remaining.next() {
            last_attempted = Some((
                plan_and_report.plan().clone(),
                plan_and_report.report_context(),
            ));
            if let Some(response) = execute_execution_runtime_sync(
                state,
                parts.uri.path(),
                plan_and_report.plan().clone(),
                trace_id,
                decision,
                plan_kind,
                plan_and_report.report_kind(),
                plan_and_report.report_context(),
            )
            .await?
            {
                mark_unused_local_candidates(state, remaining.collect()).await;
                return Ok(LocalExecutionRequestOutcome::responded(response));
            }
        }

        let Some((plan, report_context)) = last_attempted else {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        };
        Ok(LocalExecutionRequestOutcome::Exhausted(
            build_local_execution_exhaustion(state, &plan, report_context.as_ref()).await,
        ))
    }
    .instrument(span)
    .await
}

pub(crate) async fn execute_stream_plan_and_reports<T>(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    plan_and_reports: Vec<T>,
) -> Result<LocalExecutionRequestOutcome, GatewayError>
where
    T: LocalPlanAndReport,
{
    let candidate_count = plan_and_reports.len();
    let first_provider = plan_and_reports
        .first()
        .and_then(|item| item.plan().provider_name.as_deref())
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

        let mut remaining = plan_and_reports.into_iter();
        let mut last_attempted = None;
        while let Some(plan_and_report) = remaining.next() {
            let plan = plan_and_report.plan().clone();
            let report_context = plan_and_report.report_context();
            let candidate_index = parse_request_candidate_report_context(report_context.as_ref())
                .and_then(|context| context.candidate_index)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".to_string());
            debug!(
                event_name = "candidate_loop_attempt_started",
                log_type = "debug",
                trace_id = %trace_id,
                plan_kind,
                request_id = %short_request_id(plan.request_id.as_str()),
                candidate_id = ?plan.candidate_id,
                provider_name = plan.provider_name.as_deref().unwrap_or("-"),
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                model_name = plan.model_name.as_deref().unwrap_or("-"),
                candidate_index = candidate_index.as_str(),
                "candidate loop attempting stream execution candidate"
            );
            last_attempted = Some((plan.clone(), report_context.clone()));
            let watchdog_plan = plan.clone();
            let watchdog_report_context = report_context.clone();
            let execution_state = state.clone();
            let execution_trace_id = trace_id.to_string();
            let execution_plan_kind = plan_kind.to_string();
            let execution_decision = decision.clone();
            let execution_report_kind = plan_and_report.report_kind();
            if let Some(response) = execute_stream_candidate_with_watchdog(
                state,
                trace_id,
                plan_kind,
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
            .await?
            {
                mark_unused_local_candidates(state, remaining.collect()).await;
                return Ok(LocalExecutionRequestOutcome::responded(response));
            }
        }

        let Some((plan, report_context)) = last_attempted else {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        };
        warn!(
            event_name = "candidate_loop_exhausted",
            log_type = "ops",
            trace_id = %trace_id,
            plan_kind,
            request_id = %short_request_id(plan.request_id.as_str()),
            candidate_id = ?plan.candidate_id,
            provider_name = plan.provider_name.as_deref().unwrap_or("-"),
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            model_name = plan.model_name.as_deref().unwrap_or("-"),
            "candidate loop exhausted local stream candidates"
        );
        Ok(LocalExecutionRequestOutcome::Exhausted(
            build_local_execution_exhaustion(state, &plan, report_context.as_ref()).await,
        ))
    }
    .instrument(span)
    .await
}

pub(crate) async fn mark_unused_local_candidates<T>(state: &AppState, remaining: Vec<T>)
where
    T: LocalPlanAndReport,
{
    for plan_and_report in remaining {
        let report_context = plan_and_report.report_context();
        if should_skip_unused_persistence(report_context.as_ref()) {
            continue;
        }
        record_local_request_candidate_status(
            state,
            plan_and_report.plan(),
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
    metadata.candidate_group_id.is_some()
        && metadata
            .pool_key_index
            .is_some_and(|pool_key_index| pool_key_index > 0)
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
    use std::sync::Arc;

    use aether_contracts::{ExecutionPlan, ExecutionTimeouts, RequestBody};
    use aether_data_contracts::repository::candidates::{
        RequestCandidateStatus, UpsertRequestCandidateRecord,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Debug, Default)]
    struct TestRequestCandidateWriter {
        records: Mutex<Vec<UpsertRequestCandidateRecord>>,
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
            client_api_format: "claude:cli".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-test".to_string()),
            proxy: None,
            tls_profile: None,
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
            "pool_key_index": 1,
        }))));
        assert!(!should_skip_unused_persistence(Some(&json!({
            "candidate_group_id": "pool-group",
            "pool_key_index": 0,
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
            .is_some_and(|message| message.contains("25ms")));
        assert_eq!(record.candidate_index, 2);
    }
}
