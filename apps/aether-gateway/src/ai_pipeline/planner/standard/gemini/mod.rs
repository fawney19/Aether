use crate::ai_pipeline::GatewayControlDecision;
use crate::ai_pipeline::{
    resolve_gemini_stream_spec as resolve_pipeline_stream_spec,
    resolve_gemini_sync_spec as resolve_pipeline_sync_spec,
};
use crate::{AppState, GatewayControlSyncDecisionResponse, GatewayError};

use super::family::{
    maybe_build_stream_via_standard_family_payload, maybe_build_sync_via_standard_family_payload,
};
pub(crate) use crate::ai_pipeline::normalize_gemini_request_to_openai_chat_request;

pub(crate) fn resolve_sync_spec(plan_kind: &str) -> Option<super::family::LocalStandardSpec> {
    resolve_pipeline_sync_spec(plan_kind)
}

pub(crate) fn resolve_stream_spec(plan_kind: &str) -> Option<super::family::LocalStandardSpec> {
    resolve_pipeline_stream_spec(plan_kind)
}

pub(crate) async fn maybe_build_sync_local_gemini_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
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

pub(crate) async fn maybe_build_stream_local_gemini_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
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
