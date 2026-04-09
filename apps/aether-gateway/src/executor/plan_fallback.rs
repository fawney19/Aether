use axum::body::Body;
use axum::http::Response;

use crate::ai_pipeline_api::{maybe_build_stream_plan_payload, maybe_build_sync_plan_payload};
use crate::control::GatewayControlDecision;
use crate::execution_runtime::{execute_execution_runtime_stream, execute_execution_runtime_sync};
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
) -> Result<Option<Response<Body>>, GatewayError> {
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
        return Ok(None);
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
        return Ok(None);
    };

    execute_execution_runtime_sync(
        state,
        parts.uri.path(),
        plan,
        trace_id,
        decision,
        plan_kind.as_str(),
        report_kind,
        report_context,
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
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(payload) =
        maybe_build_stream_plan_payload(state, parts, trace_id, decision, body_json).await?
    else {
        return Ok(None);
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
        return Ok(None);
    };

    execute_execution_runtime_stream(
        state,
        plan,
        trace_id,
        decision,
        plan_kind.as_str(),
        report_kind,
        report_context,
    )
    .await
}
