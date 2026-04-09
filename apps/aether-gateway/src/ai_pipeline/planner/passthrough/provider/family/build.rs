use crate::ai_pipeline::GatewayControlDecision;
use crate::{AppState, GatewayControlSyncDecisionResponse, GatewayError};

use super::super::plans::{resolve_stream_spec, resolve_sync_spec};
use super::candidates::{
    materialize_local_same_format_provider_candidate_attempts,
    resolve_local_same_format_provider_decision_input,
};
use super::payload::maybe_build_local_same_format_provider_decision_payload_for_candidate;

pub(crate) async fn maybe_build_sync_local_same_format_provider_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_same_format_provider_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        return Ok(None);
    };

    let attempts =
        materialize_local_same_format_provider_candidate_attempts(state, trace_id, &input, spec)
            .await?;

    for attempt in attempts {
        if let Some(payload) =
            maybe_build_local_same_format_provider_decision_payload_for_candidate(
                state, parts, trace_id, body_json, &input, attempt, spec,
            )
            .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

pub(crate) async fn maybe_build_stream_local_same_format_provider_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_same_format_provider_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        return Ok(None);
    };

    let attempts =
        materialize_local_same_format_provider_candidate_attempts(state, trace_id, &input, spec)
            .await?;

    for attempt in attempts {
        if let Some(payload) =
            maybe_build_local_same_format_provider_decision_payload_for_candidate(
                state, parts, trace_id, body_json, &input, attempt, spec,
            )
            .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}
