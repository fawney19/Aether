use crate::ai_serving::GatewayControlDecision;
use crate::ai_serving::{
    resolve_claude_stream_spec as resolve_surface_stream_spec,
    resolve_claude_sync_spec as resolve_surface_sync_spec,
};
use crate::{AiExecutionDecision, AppState, GatewayError};

use super::family::{
    maybe_build_stream_via_standard_family_payload, maybe_build_sync_via_standard_family_payload,
};
pub(crate) use crate::ai_serving::normalize_claude_request_to_openai_chat_request;

pub(crate) fn resolve_sync_spec(plan_kind: &str) -> Option<super::family::LocalStandardSpec> {
    resolve_surface_sync_spec(plan_kind)
}

pub(crate) fn resolve_stream_spec(plan_kind: &str) -> Option<super::family::LocalStandardSpec> {
    resolve_surface_stream_spec(plan_kind)
}

pub(crate) async fn maybe_build_sync_local_claude_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    maybe_build_sync_via_standard_family_payload(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        plan_kind,
        resolve_sync_spec,
    )
    .await
}

pub(crate) async fn maybe_build_stream_local_claude_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    maybe_build_stream_via_standard_family_payload(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        plan_kind,
        resolve_stream_spec,
    )
    .await
}
