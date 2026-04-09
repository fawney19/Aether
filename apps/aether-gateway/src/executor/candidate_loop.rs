use axum::body::Body;
use axum::http::Response;

use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_scheduler_core::SchedulerRequestCandidateStatusUpdate;

use crate::ai_pipeline_api::{LocalStreamPlanAndReport, LocalSyncPlanAndReport};
use crate::control::GatewayControlDecision;
use crate::execution_runtime::{execute_execution_runtime_stream, execute_execution_runtime_sync};
use crate::request_candidate_runtime::record_local_request_candidate_status;
use crate::{AppState, GatewayError};

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
) -> Result<Option<Response<Body>>, GatewayError>
where
    T: LocalPlanAndReport,
{
    let mut remaining = plan_and_reports.into_iter();
    while let Some(plan_and_report) = remaining.next() {
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
            return Ok(Some(response));
        }
    }

    Ok(None)
}

pub(crate) async fn execute_stream_plan_and_reports<T>(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    plan_and_reports: Vec<T>,
) -> Result<Option<Response<Body>>, GatewayError>
where
    T: LocalPlanAndReport,
{
    let mut remaining = plan_and_reports.into_iter();
    while let Some(plan_and_report) = remaining.next() {
        if let Some(response) = execute_execution_runtime_stream(
            state,
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
            return Ok(Some(response));
        }
    }

    Ok(None)
}

pub(crate) async fn mark_unused_local_candidates<T>(state: &AppState, remaining: Vec<T>)
where
    T: LocalPlanAndReport,
{
    for plan_and_report in remaining {
        record_local_request_candidate_status(
            state,
            plan_and_report.plan(),
            plan_and_report.report_context().as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Unused,
                status_code: None,
                error_type: None,
                error_message: None,
                latency_ms: None,
                started_at_unix_secs: None,
                finished_at_unix_secs: None,
            },
        )
        .await;
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
        record_local_request_candidate_status(
            state,
            plan(&item),
            report_context(&item),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Unused,
                status_code: None,
                error_type: None,
                error_message: None,
                latency_ms: None,
                started_at_unix_secs: None,
                finished_at_unix_secs: None,
            },
        )
        .await;
    }
}
