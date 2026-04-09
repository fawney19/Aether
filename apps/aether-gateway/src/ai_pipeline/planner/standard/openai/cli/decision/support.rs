use std::collections::BTreeSet;

use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

use crate::ai_pipeline::contracts::ExecutionRuntimeAuthContext;
use crate::ai_pipeline::conversion::request_conversion_kind;
use crate::ai_pipeline::planner::candidate_affinity::prefer_local_tunnel_owner_candidates;
use crate::ai_pipeline::{
    resolve_local_decision_execution_runtime_auth_context, ConversionMode, ExecutionStrategy,
    GatewayControlDecision,
};
use crate::ai_pipeline::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_secs;
use crate::{append_execution_contract_fields_to_value, AppState, GatewayError};

use super::LocalOpenAiCliSpec;

#[derive(Debug, Clone)]
pub(crate) struct LocalOpenAiCliDecisionInput {
    pub(crate) auth_context: ExecutionRuntimeAuthContext,
    pub(crate) requested_model: String,
    pub(crate) auth_snapshot: GatewayAuthApiKeySnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalOpenAiCliCandidateAttempt {
    pub(crate) candidate: SchedulerMinimalCandidateSelectionCandidate,
    pub(crate) candidate_index: u32,
    pub(crate) candidate_id: String,
}

pub(crate) async fn resolve_local_openai_cli_decision_input(
    state: &AppState,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
) -> Option<LocalOpenAiCliDecisionInput> {
    let planner_state = PlannerAppState::new(state);
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return None;
    };

    let requested_model = body_json
        .get("model")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)?;

    let auth_snapshot = match planner_state
        .read_auth_api_key_snapshot(
            &auth_context.user_id,
            &auth_context.api_key_id,
            current_unix_secs(),
        )
        .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return None,
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                error = ?err,
                "gateway local openai cli decision auth snapshot read failed"
            );
            return None;
        }
    };

    Some(LocalOpenAiCliDecisionInput {
        auth_context,
        requested_model,
        auth_snapshot,
    })
}

pub(crate) async fn materialize_local_openai_cli_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalOpenAiCliDecisionInput,
    spec: LocalOpenAiCliSpec,
) -> Result<Vec<LocalOpenAiCliCandidateAttempt>, GatewayError> {
    let planner_state = PlannerAppState::new(state);
    let mut seen_candidates = BTreeSet::new();
    let mut candidates = Vec::new();
    for candidate_api_format in candidate_api_formats_for_spec(spec) {
        let auth_snapshot = if *candidate_api_format == spec.api_format {
            Some(&input.auth_snapshot)
        } else {
            None
        };
        let mut selected_candidates = planner_state
            .list_selectable_candidates(
                candidate_api_format,
                &input.requested_model,
                spec.require_streaming,
                auth_snapshot,
                current_unix_secs(),
            )
            .await?;
        if auth_snapshot.is_none() {
            selected_candidates.retain(|candidate| {
                auth_snapshot_allows_cross_format_openai_cli_candidate(
                    &input.auth_snapshot,
                    &input.requested_model,
                    candidate,
                )
            });
        }
        for candidate in selected_candidates {
            let candidate_key = format!(
                "{}:{}:{}:{}:{}:{}",
                candidate.provider_id,
                candidate.endpoint_id,
                candidate.key_id,
                candidate.model_id,
                candidate.selected_provider_model_name,
                candidate.endpoint_api_format,
            );
            if seen_candidates.insert(candidate_key) {
                candidates.push(candidate);
            }
        }
    }
    let candidates = prefer_local_tunnel_owner_candidates(planner_state, candidates).await;

    let created_at_unix_secs = current_unix_secs();
    let mut attempts = Vec::with_capacity(candidates.len());
    for (candidate_index, candidate) in candidates.into_iter().enumerate() {
        let generated_candidate_id = Uuid::new_v4().to_string();
        let provider_api_format = candidate.endpoint_api_format.trim().to_ascii_lowercase();
        let execution_strategy =
            if provider_api_format == spec.api_format.trim().to_ascii_lowercase() {
                ExecutionStrategy::LocalSameFormat
            } else {
                ExecutionStrategy::LocalCrossFormat
            };
        let conversion_mode =
            if request_conversion_kind(spec.api_format, provider_api_format.as_str()).is_some() {
                ConversionMode::Bidirectional
            } else {
                ConversionMode::None
            };
        let extra_data = append_execution_contract_fields_to_value(
            json!({
                "provider_api_format": provider_api_format,
                "client_api_format": spec.api_format,
                "global_model_id": candidate.global_model_id.clone(),
                "global_model_name": candidate.global_model_name.clone(),
                "model_id": candidate.model_id.clone(),
                "selected_provider_model_name": candidate.selected_provider_model_name.clone(),
                "mapping_matched_model": candidate.mapping_matched_model.clone(),
                "provider_name": candidate.provider_name.clone(),
                "key_name": candidate.key_name.clone(),
            }),
            execution_strategy,
            conversion_mode,
            spec.api_format,
            candidate.endpoint_api_format.as_str(),
        );

        let candidate_id = planner_state
            .persist_available_local_candidate(
                trace_id,
                &input.auth_context.user_id,
                &input.auth_context.api_key_id,
                &candidate,
                candidate_index as u32,
                &generated_candidate_id,
                Some(extra_data),
                created_at_unix_secs,
                "gateway local openai cli decision request candidate upsert failed",
            )
            .await;

        attempts.push(LocalOpenAiCliCandidateAttempt {
            candidate,
            candidate_index: candidate_index as u32,
            candidate_id,
        });
    }

    Ok(attempts)
}

fn auth_snapshot_allows_cross_format_openai_cli_candidate(
    auth_snapshot: &GatewayAuthApiKeySnapshot,
    requested_model: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
) -> bool {
    if let Some(allowed_providers) = auth_snapshot.effective_allowed_providers() {
        let provider_allowed = allowed_providers.iter().any(|value| {
            value
                .trim()
                .eq_ignore_ascii_case(candidate.provider_id.trim())
                || value
                    .trim()
                    .eq_ignore_ascii_case(candidate.provider_name.trim())
        });
        if !provider_allowed {
            return false;
        }
    }

    if let Some(allowed_models) = auth_snapshot.effective_allowed_models() {
        let model_allowed = allowed_models
            .iter()
            .any(|value| value == requested_model || value == &candidate.global_model_name);
        if !model_allowed {
            return false;
        }
    }

    true
}

fn candidate_api_formats_for_spec(spec: LocalOpenAiCliSpec) -> &'static [&'static str] {
    match spec.api_format {
        "openai:compact" => &["openai:compact", "openai:cli", "claude:cli", "gemini:cli"],
        "openai:cli" => &["openai:cli", "claude:cli", "gemini:cli"],
        _ => &[],
    }
}

pub(crate) async fn mark_skipped_local_openai_cli_candidate(
    state: &AppState,
    input: &LocalOpenAiCliDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    PlannerAppState::new(state)
        .persist_skipped_local_candidate(
            trace_id,
            &input.auth_context.user_id,
            &input.auth_context.api_key_id,
            candidate,
            candidate_index,
            candidate_id,
            skip_reason,
            current_unix_secs(),
            "gateway local openai cli decision failed to persist skipped candidate",
        )
        .await;
}
