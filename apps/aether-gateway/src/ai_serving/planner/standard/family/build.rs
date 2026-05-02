use tracing::warn;

use crate::ai_serving::planner::common::extract_requested_model_from_request;
use crate::ai_serving::planner::plan_builders::{AiStreamAttempt, AiSyncAttempt};
use crate::ai_serving::planner::runtime_miss::{
    apply_local_runtime_candidate_evaluation_progress,
    apply_local_runtime_candidate_terminal_reason, set_local_runtime_miss_diagnostic_reason,
};
use crate::ai_serving::planner::spec_metadata::{
    build_stream_plan_from_requested_model_family, build_sync_plan_from_requested_model_family,
    local_standard_spec_metadata,
};
use crate::ai_serving::GatewayControlDecision;
use crate::{AiExecutionDecision, AppState, GatewayError};

use super::candidates::{
    materialize_local_standard_candidate_attempts, resolve_local_standard_decision_input,
};
use super::payload::maybe_build_local_standard_decision_payload_for_candidate;
use super::LocalStandardSpec;

pub(crate) async fn maybe_build_sync_via_standard_family_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
    resolve_sync_spec: fn(&str) -> Option<LocalStandardSpec>,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };
    let spec_metadata = local_standard_spec_metadata(spec);

    let Some(input) =
        resolve_local_standard_decision_input(state, parts, trace_id, decision, body_json, spec)
            .await
    else {
        return Ok(None);
    };

    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        spec_metadata.decision_kind,
        Some(input.requested_model.as_str()),
        "candidate_evaluation_incomplete",
    );
    let (attempts, candidate_count) =
        materialize_local_standard_candidate_attempts(state, trace_id, &input, body_json, spec)
            .await?;
    apply_local_runtime_candidate_evaluation_progress(state, trace_id, candidate_count);

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_standard_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_sync_plans");

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
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };
    let spec_metadata = local_standard_spec_metadata(spec);

    let Some(input) =
        resolve_local_standard_decision_input(state, parts, trace_id, decision, body_json, spec)
            .await
    else {
        return Ok(None);
    };

    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        spec_metadata.decision_kind,
        Some(input.requested_model.as_str()),
        "candidate_evaluation_incomplete",
    );
    let (attempts, candidate_count) =
        materialize_local_standard_candidate_attempts(state, trace_id, &input, body_json, spec)
            .await?;
    apply_local_runtime_candidate_evaluation_progress(state, trace_id, candidate_count);

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_standard_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_stream_plans");

    Ok(None)
}

pub(crate) async fn build_local_sync_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let requested_model_family = spec_metadata
        .requested_model_family
        .expect("standard spec metadata should include requested-model family");
    let Some(input) =
        resolve_local_standard_decision_input(state, parts, trace_id, decision, body_json, spec)
            .await
    else {
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            spec_metadata.decision_kind,
            extract_requested_model_from_request(parts, body_json, requested_model_family)
                .as_deref(),
            "decision_input_unavailable",
        );
        return Ok(Vec::new());
    };
    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        spec_metadata.decision_kind,
        Some(input.requested_model.as_str()),
        "candidate_evaluation_incomplete",
    );
    let (attempts, candidate_count) =
        materialize_local_standard_candidate_attempts(state, trace_id, &input, body_json, spec)
            .await?;
    apply_local_runtime_candidate_evaluation_progress(state, trace_id, candidate_count);
    if candidate_count == 0 {
        return Ok(Vec::new());
    }
    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_standard_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };
        let built = build_sync_plan_from_requested_model_family(
            requested_model_family,
            parts,
            body_json,
            payload,
        );
        match built {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec_metadata.api_format,
                    error = ?err,
                    "gateway local standard sync plan build failed"
                );
            }
        }
    }
    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_sync_plans");
    Ok(plans)
}

pub(crate) async fn build_local_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let requested_model_family = spec_metadata
        .requested_model_family
        .expect("standard spec metadata should include requested-model family");
    let Some(input) =
        resolve_local_standard_decision_input(state, parts, trace_id, decision, body_json, spec)
            .await
    else {
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            spec_metadata.decision_kind,
            extract_requested_model_from_request(parts, body_json, requested_model_family)
                .as_deref(),
            "decision_input_unavailable",
        );
        return Ok(Vec::new());
    };
    set_local_runtime_miss_diagnostic_reason(
        state,
        trace_id,
        decision,
        spec_metadata.decision_kind,
        Some(input.requested_model.as_str()),
        "candidate_evaluation_incomplete",
    );
    let (attempts, candidate_count) =
        materialize_local_standard_candidate_attempts(state, trace_id, &input, body_json, spec)
            .await?;
    apply_local_runtime_candidate_evaluation_progress(state, trace_id, candidate_count);
    if candidate_count == 0 {
        return Ok(Vec::new());
    }
    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_standard_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };
        let built = build_stream_plan_from_requested_model_family(
            requested_model_family,
            parts,
            body_json,
            payload,
        );
        match built {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec_metadata.api_format,
                    error = ?err,
                    "gateway local standard stream plan build failed"
                );
            }
        }
    }
    apply_local_runtime_candidate_terminal_reason(state, trace_id, "no_local_stream_plans");
    Ok(plans)
}
