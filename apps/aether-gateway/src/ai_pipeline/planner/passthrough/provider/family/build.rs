use crate::ai_pipeline::GatewayControlDecision;
use crate::{
    AppState, GatewayControlSyncDecisionResponse, GatewayError, LocalExecutionRuntimeMissDiagnostic,
};

use super::super::plans::{resolve_stream_spec, resolve_sync_spec};
use super::candidates::{
    materialize_local_same_format_provider_candidate_attempts,
    resolve_local_same_format_provider_decision_input,
};
use super::payload::maybe_build_local_same_format_provider_decision_payload_for_candidate;

fn extract_requested_model(
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    spec: crate::ai_pipeline::LocalSameFormatProviderSpec,
) -> Option<String> {
    match spec.family {
        crate::ai_pipeline::LocalSameFormatProviderFamily::Standard => body_json
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        crate::ai_pipeline::LocalSameFormatProviderFamily::Gemini => {
            let marker = "/models/";
            let start = parts.uri.path().find(marker)? + marker.len();
            let tail = &parts.uri.path()[start..];
            let end = tail.find(':').unwrap_or(tail.len());
            let model = tail[..end].trim();
            if model.is_empty() {
                None
            } else {
                Some(model.to_string())
            }
        }
    }
}

fn build_local_same_format_miss_diagnostic(
    decision: &GatewayControlDecision,
    spec: crate::ai_pipeline::LocalSameFormatProviderSpec,
    requested_model: Option<&str>,
    reason: &str,
) -> LocalExecutionRuntimeMissDiagnostic {
    LocalExecutionRuntimeMissDiagnostic {
        reason: reason.to_string(),
        route_family: decision.route_family.clone(),
        route_kind: decision.route_kind.clone(),
        public_path: Some(decision.public_path.clone()),
        plan_kind: Some(spec.decision_kind.to_string()),
        requested_model: requested_model.map(ToOwned::to_owned),
        candidate_count: None,
        skipped_candidate_count: None,
        skip_reasons: std::collections::BTreeMap::new(),
    }
}

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
        state.set_local_execution_runtime_miss_diagnostic(
            trace_id,
            build_local_same_format_miss_diagnostic(
                decision,
                spec,
                extract_requested_model(parts, body_json, spec).as_deref(),
                "decision_input_unavailable",
            ),
        );
        return Ok(None);
    };

    state.set_local_execution_runtime_miss_diagnostic(
        trace_id,
        build_local_same_format_miss_diagnostic(
            decision,
            spec,
            Some(input.requested_model.as_str()),
            "candidate_evaluation_incomplete",
        ),
    );
    let (attempts, candidate_count) =
        materialize_local_same_format_provider_candidate_attempts(state, trace_id, &input, spec)
            .await?;
    let preserve_existing_candidate_signal = candidate_count == 0
        && state.local_execution_runtime_miss_diagnostic_has_candidate_signal(trace_id);
    if !preserve_existing_candidate_signal {
        state.mutate_local_execution_runtime_miss_diagnostic(trace_id, |diagnostic| {
            diagnostic.candidate_count = Some(candidate_count);
            diagnostic.reason = if candidate_count == 0 {
                "candidate_list_empty".to_string()
            } else {
                "candidate_evaluation_incomplete".to_string()
            };
        });
    }

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

    state.mutate_local_execution_runtime_miss_diagnostic(trace_id, |diagnostic| {
        let candidate_count = diagnostic.candidate_count.unwrap_or(0);
        let skipped_candidate_count = diagnostic.skipped_candidate_count.unwrap_or(0);
        diagnostic.reason = if candidate_count == 0 {
            "candidate_list_empty".to_string()
        } else if skipped_candidate_count >= candidate_count {
            "all_candidates_skipped".to_string()
        } else {
            "no_local_sync_plans".to_string()
        };
    });

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
        state.set_local_execution_runtime_miss_diagnostic(
            trace_id,
            build_local_same_format_miss_diagnostic(
                decision,
                spec,
                extract_requested_model(parts, body_json, spec).as_deref(),
                "decision_input_unavailable",
            ),
        );
        return Ok(None);
    };

    state.set_local_execution_runtime_miss_diagnostic(
        trace_id,
        build_local_same_format_miss_diagnostic(
            decision,
            spec,
            Some(input.requested_model.as_str()),
            "candidate_evaluation_incomplete",
        ),
    );
    let (attempts, candidate_count) =
        materialize_local_same_format_provider_candidate_attempts(state, trace_id, &input, spec)
            .await?;
    let preserve_existing_candidate_signal = candidate_count == 0
        && state.local_execution_runtime_miss_diagnostic_has_candidate_signal(trace_id);
    if !preserve_existing_candidate_signal {
        state.mutate_local_execution_runtime_miss_diagnostic(trace_id, |diagnostic| {
            diagnostic.candidate_count = Some(candidate_count);
            diagnostic.reason = if candidate_count == 0 {
                "candidate_list_empty".to_string()
            } else {
                "candidate_evaluation_incomplete".to_string()
            };
        });
    }

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

    state.mutate_local_execution_runtime_miss_diagnostic(trace_id, |diagnostic| {
        let candidate_count = diagnostic.candidate_count.unwrap_or(0);
        let skipped_candidate_count = diagnostic.skipped_candidate_count.unwrap_or(0);
        diagnostic.reason = if candidate_count == 0 {
            "candidate_list_empty".to_string()
        } else if skipped_candidate_count >= candidate_count {
            "all_candidates_skipped".to_string()
        } else {
            "no_local_stream_plans".to_string()
        };
    });

    Ok(None)
}
