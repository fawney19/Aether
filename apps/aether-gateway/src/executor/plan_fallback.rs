use crate::ai_pipeline_api::{
    maybe_build_stream_plan_payload, maybe_build_sync_plan_payload, LocalStreamPlanAndReport,
    LocalSyncPlanAndReport,
};
use crate::control::GatewayControlDecision;
use crate::executor::{
    execute_stream_plan_and_reports, execute_sync_plan_and_reports, LocalExecutionRequestOutcome,
};
use crate::{AppState, GatewayControlPlanResponse, GatewayError, GatewayFallbackReason};

pub(crate) async fn maybe_execute_sync_via_plan_fallback(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<String>,
    _plan_kind: &str,
    _bypass_cache_key: String,
    _fallback_reason: GatewayFallbackReason,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let body_is_empty =
        body_base64.is_none() && body_json.as_object().is_some_and(|value| value.is_empty());
    let Some(payload) = maybe_build_sync_plan_payload(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64.as_deref(),
        body_is_empty,
    )
    .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let GatewayControlPlanResponse {
        action: _,
        plan_kind,
        plan,
        report_kind,
        report_context,
        auth_context: _,
    } = payload;

    let (Some(plan_kind), Some(plan)) = (plan_kind, plan) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind.as_str(),
        vec![LocalSyncPlanAndReport {
            plan,
            report_kind,
            report_context,
        }],
    )
    .await
}

pub(crate) async fn maybe_execute_stream_via_plan_fallback(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<String>,
    _plan_kind: &str,
    _bypass_cache_key: String,
    _fallback_reason: GatewayFallbackReason,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some(payload) = maybe_build_stream_plan_payload(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        body_base64.as_deref(),
    )
    .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let GatewayControlPlanResponse {
        action: _,
        plan_kind,
        plan,
        report_kind,
        report_context,
        auth_context: _,
    } = payload;

    let (Some(plan_kind), Some(plan)) = (plan_kind, plan) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_stream_plan_and_reports(
        state,
        trace_id,
        decision,
        plan_kind.as_str(),
        vec![LocalStreamPlanAndReport {
            plan,
            report_kind,
            report_context,
        }],
    )
    .await
}
