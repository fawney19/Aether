mod decision;
mod support;

use tracing::warn;

use crate::ai_pipeline::planner::plan_builders::{
    build_passthrough_sync_plan_from_decision, LocalSyncPlanAndReport,
};
use crate::ai_pipeline::GatewayControlDecision;
use crate::ai_pipeline::{
    resolve_local_video_sync_spec as resolve_sync_spec, LocalVideoCreateFamily,
    LocalVideoCreateSpec,
};
use crate::{AppState, GatewayControlSyncDecisionResponse, GatewayError};

use self::decision::maybe_build_local_video_create_decision_payload_for_candidate;
use self::support::{
    list_local_video_create_candidate_attempts, resolve_local_video_create_decision_input,
};

pub(crate) async fn build_local_video_sync_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Vec<LocalSyncPlanAndReport>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_sync_plan_and_reports(state, parts, body_json, trace_id, decision, spec).await
}

pub(crate) async fn maybe_build_sync_local_video_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_video_create_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        return Ok(None);
    };

    let Some(attempts) = list_local_video_create_candidate_attempts(
        state,
        trace_id,
        &input,
        spec.api_format,
        spec.decision_kind,
    )
    .await
    else {
        return Ok(None);
    };

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_video_create_decision_payload_for_candidate(
            state, parts, body_json, trace_id, &input, attempt, spec,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

async fn build_local_sync_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    spec: LocalVideoCreateSpec,
) -> Result<Vec<LocalSyncPlanAndReport>, GatewayError> {
    let Some(input) = resolve_local_video_create_decision_input(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await
    else {
        return Ok(Vec::new());
    };

    let Some(attempts) = list_local_video_create_candidate_attempts(
        state,
        trace_id,
        &input,
        spec.api_format,
        spec.decision_kind,
    )
    .await
    else {
        return Ok(Vec::new());
    };

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_video_create_decision_payload_for_candidate(
            state, parts, body_json, trace_id, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };

        match build_passthrough_sync_plan_from_decision(parts, payload) {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    decision_kind = spec.decision_kind,
                    error = ?err,
                    "gateway local video sync decision plan build failed"
                );
            }
        }
    }

    Ok(plans)
}
