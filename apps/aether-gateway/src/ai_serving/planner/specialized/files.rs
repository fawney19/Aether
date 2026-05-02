mod decision;
mod request;
mod support;

use tracing::warn;

use crate::ai_serving::planner::plan_builders::{
    build_passthrough_stream_plan_from_decision, build_passthrough_sync_plan_from_decision,
    AiStreamAttempt, AiSyncAttempt,
};
use crate::ai_serving::planner::spec_metadata::local_gemini_files_spec_metadata;
use crate::ai_serving::GatewayControlDecision;
use crate::ai_serving::{
    resolve_gemini_files_stream_spec as resolve_stream_spec,
    resolve_gemini_files_sync_spec as resolve_sync_spec, LocalGeminiFilesSpec,
};
use crate::{AiExecutionDecision, AppState, GatewayError};

use self::decision::maybe_build_local_gemini_files_decision_payload_for_candidate;
use self::support::{
    materialize_local_gemini_files_candidate_attempts, resolve_local_gemini_files_decision_input,
};

pub(crate) async fn build_local_gemini_files_sync_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_sync_plan_and_reports(
        state,
        parts,
        body_json,
        body_base64,
        body_is_empty,
        trace_id,
        decision,
        spec,
    )
    .await
}

pub(crate) async fn build_local_gemini_files_stream_plan_and_reports_for_kind(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(Vec::new());
    };

    build_local_stream_plan_and_reports(state, parts, trace_id, decision, spec).await
}

pub(crate) async fn maybe_build_sync_local_gemini_files_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_gemini_files_decision_input(state, trace_id, decision).await
    else {
        return Ok(None);
    };

    let attempts =
        materialize_local_gemini_files_candidate_attempts(state, trace_id, &input).await?;

    for attempt in attempts {
        if let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            state,
            parts,
            body_json,
            body_base64,
            body_is_empty,
            trace_id,
            &input,
            attempt,
            spec,
        )
        .await
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

pub(crate) async fn maybe_build_stream_local_gemini_files_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(None);
    };

    let Some(input) = resolve_local_gemini_files_decision_input(state, trace_id, decision).await
    else {
        return Ok(None);
    };

    let attempts =
        materialize_local_gemini_files_candidate_attempts(state, trace_id, &input).await?;

    let empty_body_json = serde_json::Value::Null;
    for attempt in attempts {
        if let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            state,
            parts,
            &empty_body_json,
            None,
            true,
            trace_id,
            &input,
            attempt,
            spec,
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
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    decision: &GatewayControlDecision,
    spec: LocalGeminiFilesSpec,
) -> Result<Vec<AiSyncAttempt>, GatewayError> {
    let spec_metadata = local_gemini_files_spec_metadata(spec);
    let Some(input) = resolve_local_gemini_files_decision_input(state, trace_id, decision).await
    else {
        return Ok(Vec::new());
    };

    let attempts =
        materialize_local_gemini_files_candidate_attempts(state, trace_id, &input).await?;

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            state,
            parts,
            body_json,
            body_base64,
            body_is_empty,
            trace_id,
            &input,
            attempt,
            spec,
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
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local gemini files sync decision plan build failed"
                );
            }
        }
    }

    Ok(plans)
}

async fn build_local_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    spec: LocalGeminiFilesSpec,
) -> Result<Vec<AiStreamAttempt>, GatewayError> {
    let spec_metadata = local_gemini_files_spec_metadata(spec);
    let Some(input) = resolve_local_gemini_files_decision_input(state, trace_id, decision).await
    else {
        return Ok(Vec::new());
    };

    let attempts =
        materialize_local_gemini_files_candidate_attempts(state, trace_id, &input).await?;

    let mut plans = Vec::new();
    let empty_body_json = serde_json::Value::Null;
    for attempt in attempts {
        let Some(payload) = maybe_build_local_gemini_files_decision_payload_for_candidate(
            state,
            parts,
            &empty_body_json,
            None,
            true,
            trace_id,
            &input,
            attempt,
            spec,
        )
        .await
        else {
            continue;
        };

        match build_passthrough_stream_plan_from_decision(parts, payload) {
            Ok(Some(value)) => plans.push(value),
            Ok(None) => {}
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    decision_kind = spec_metadata.decision_kind,
                    error = ?err,
                    "gateway local gemini files stream decision plan build failed"
                );
            }
        }
    }

    Ok(plans)
}
