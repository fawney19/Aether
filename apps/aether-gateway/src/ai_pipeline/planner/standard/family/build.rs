use tracing::warn;

use crate::ai_pipeline::planner::plan_builders::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_standard_stream_plan_from_decision, build_standard_sync_plan_from_decision,
    LocalStreamPlanAndReport, LocalSyncPlanAndReport,
};
use crate::ai_pipeline::GatewayControlDecision;
use crate::{AppState, GatewayControlSyncDecisionResponse, GatewayError};

use super::candidates::{
    materialize_local_standard_candidate_attempts, resolve_local_standard_decision_input,
};
use super::payload::maybe_build_local_standard_decision_payload_for_candidate;
use super::{LocalStandardSourceFamily, LocalStandardSpec};

pub(crate) async fn maybe_build_sync_via_standard_family_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
    resolve_sync_spec: fn(&str) -> Option<LocalStandardSpec>,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) =
        resolve_local_standard_decision_input(state, parts, trace_id, decision, body_json, spec)
            .await
    else {
        return Ok(None);
    };

    let attempts =
        materialize_local_standard_candidate_attempts(state, trace_id, &input, spec).await?;

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_standard_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

pub(crate) async fn maybe_build_stream_via_standard_family_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
    resolve_stream_spec: fn(&str) -> Option<LocalStandardSpec>,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) =
        resolve_local_standard_decision_input(state, parts, trace_id, decision, body_json, spec)
            .await
    else {
        return Ok(None);
    };

    let attempts =
        materialize_local_standard_candidate_attempts(state, trace_id, &input, spec).await?;

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_standard_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

pub(crate) async fn build_local_sync_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Result<Vec<LocalSyncPlanAndReport>, GatewayError> {
    let Some(input) =
        resolve_local_standard_decision_input(state, parts, trace_id, decision, body_json, spec)
            .await
    else {
        return Ok(Vec::new());
    };
    let attempts =
        materialize_local_standard_candidate_attempts(state, trace_id, &input, spec).await?;
    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_standard_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };
        let built = match spec.family {
            LocalStandardSourceFamily::Standard => {
                build_standard_sync_plan_from_decision(parts, body_json, payload)
            }
            LocalStandardSourceFamily::Gemini => {
                build_gemini_sync_plan_from_decision(parts, body_json, payload)
            }
        };
        match built {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec.api_format,
                    error = ?err,
                    "gateway local standard sync plan build failed"
                );
            }
        }
    }
    Ok(plans)
}

pub(crate) async fn build_local_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Result<Vec<LocalStreamPlanAndReport>, GatewayError> {
    let Some(input) =
        resolve_local_standard_decision_input(state, parts, trace_id, decision, body_json, spec)
            .await
    else {
        return Ok(Vec::new());
    };
    let attempts =
        materialize_local_standard_candidate_attempts(state, trace_id, &input, spec).await?;
    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_standard_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };
        let built = match spec.family {
            LocalStandardSourceFamily::Standard => {
                build_standard_stream_plan_from_decision(parts, body_json, payload, false)
            }
            LocalStandardSourceFamily::Gemini => {
                build_gemini_stream_plan_from_decision(parts, body_json, payload)
            }
        };
        match built {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec.api_format,
                    error = ?err,
                    "gateway local standard stream plan build failed"
                );
            }
        }
    }
    Ok(plans)
}
