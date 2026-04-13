use tracing::warn;

pub(crate) use crate::ai_pipeline::{
    resolve_local_same_format_stream_spec as resolve_stream_spec,
    resolve_local_same_format_sync_spec as resolve_sync_spec,
};

use super::{
    materialize_local_same_format_provider_candidate_attempts,
    maybe_build_local_same_format_provider_decision_payload_for_candidate,
    resolve_local_same_format_provider_decision_input, AppState, GatewayControlDecision,
    GatewayError, LocalSameFormatProviderFamily, LocalSameFormatProviderSpec,
    LocalStreamPlanAndReport, LocalSyncPlanAndReport,
};
use crate::ai_pipeline::planner::plan_builders::{
    build_gemini_stream_plan_from_decision, build_gemini_sync_plan_from_decision,
    build_standard_stream_plan_from_decision, build_standard_sync_plan_from_decision,
};
use crate::LocalExecutionRuntimeMissDiagnostic;

fn extract_requested_model(
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Option<String> {
    match spec.family {
        LocalSameFormatProviderFamily::Standard => body_json
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        LocalSameFormatProviderFamily::Gemini => {
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
    spec: LocalSameFormatProviderSpec,
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

pub(crate) async fn build_local_sync_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<Vec<LocalSyncPlanAndReport>, GatewayError> {
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
        return Ok(Vec::new());
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
    if candidate_count == 0 {
        return Ok(Vec::new());
    }

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_same_format_provider_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };

        let built = match spec.family {
            LocalSameFormatProviderFamily::Standard => {
                build_standard_sync_plan_from_decision(parts, body_json, payload)
            }
            LocalSameFormatProviderFamily::Gemini => {
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
                    "gateway local same-format sync decision plan build failed"
                );
            }
        }
    }

    state.mutate_local_execution_runtime_miss_diagnostic(trace_id, |diagnostic| {
        let candidate_count = diagnostic.candidate_count.unwrap_or(0);
        let skipped_candidate_count = diagnostic.skipped_candidate_count.unwrap_or(0);
        diagnostic.reason = if candidate_count > 0 && skipped_candidate_count >= candidate_count {
            "all_candidates_skipped".to_string()
        } else {
            "no_local_sync_plans".to_string()
        };
    });

    Ok(plans)
}

pub(crate) async fn build_local_stream_plan_and_reports(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<Vec<LocalStreamPlanAndReport>, GatewayError> {
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
        return Ok(Vec::new());
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
    if candidate_count == 0 {
        return Ok(Vec::new());
    }

    let mut plans = Vec::new();
    for attempt in attempts {
        let Some(payload) = maybe_build_local_same_format_provider_decision_payload_for_candidate(
            state, parts, trace_id, body_json, &input, attempt, spec,
        )
        .await
        else {
            continue;
        };

        let built = match spec.family {
            LocalSameFormatProviderFamily::Standard => {
                build_standard_stream_plan_from_decision(parts, body_json, payload, false)
            }
            LocalSameFormatProviderFamily::Gemini => {
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
                    "gateway local same-format stream decision plan build failed"
                );
            }
        }
    }

    state.mutate_local_execution_runtime_miss_diagnostic(trace_id, |diagnostic| {
        let candidate_count = diagnostic.candidate_count.unwrap_or(0);
        let skipped_candidate_count = diagnostic.skipped_candidate_count.unwrap_or(0);
        diagnostic.reason = if candidate_count > 0 && skipped_candidate_count >= candidate_count {
            "all_candidates_skipped".to_string()
        } else {
            "no_local_stream_plans".to_string()
        };
    });

    Ok(plans)
}
