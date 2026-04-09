use std::collections::BTreeSet;

use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

use crate::ai_pipeline::planner::candidate_affinity::prefer_local_tunnel_owner_candidates;
use crate::ai_pipeline::{
    resolve_local_decision_execution_runtime_auth_context, ConversionMode, ExecutionStrategy,
    GatewayControlDecision,
};
use crate::ai_pipeline::{GatewayAuthApiKeySnapshot, PlannerAppState};
use crate::clock::current_unix_secs;
use crate::{append_execution_contract_fields_to_value, AppState, GatewayError};

use super::{
    LocalStandardCandidateAttempt, LocalStandardDecisionInput, LocalStandardSourceFamily,
    LocalStandardSourceMode, LocalStandardSpec,
};

pub(super) async fn resolve_local_standard_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Option<LocalStandardDecisionInput> {
    let planner_state = PlannerAppState::new(state);
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return None;
    };

    let requested_model = match spec.family {
        LocalStandardSourceFamily::Standard => body_json
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)?,
        LocalStandardSourceFamily::Gemini => extract_gemini_model_from_path(parts.uri.path())?,
    };

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
                api_format = spec.api_format,
                error = ?err,
                "gateway local standard decision auth snapshot read failed"
            );
            return None;
        }
    };

    Some(LocalStandardDecisionInput {
        auth_context,
        requested_model,
        auth_snapshot,
    })
}

pub(super) async fn materialize_local_standard_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalStandardDecisionInput,
    spec: LocalStandardSpec,
) -> Result<Vec<LocalStandardCandidateAttempt>, GatewayError> {
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
                auth_snapshot_allows_cross_format_candidate(
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
        let candidate_id = Uuid::new_v4().to_string();
        let provider_api_format = candidate.endpoint_api_format.trim().to_ascii_lowercase();
        let execution_strategy = if provider_api_format == spec.api_format {
            ExecutionStrategy::LocalSameFormat
        } else {
            ExecutionStrategy::LocalCrossFormat
        };
        let conversion_mode = if crate::ai_pipeline::conversion::request_conversion_kind(
            spec.api_format,
            provider_api_format.as_str(),
        )
        .is_some()
        {
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

        let stored_candidate_id = planner_state
            .persist_available_local_candidate(
                trace_id,
                &input.auth_context.user_id,
                &input.auth_context.api_key_id,
                &candidate,
                candidate_index as u32,
                &candidate_id,
                Some(extra_data),
                created_at_unix_secs,
                "gateway local standard decision request candidate upsert failed",
            )
            .await;

        attempts.push(LocalStandardCandidateAttempt {
            candidate,
            candidate_index: candidate_index as u32,
            candidate_id: stored_candidate_id,
        });
    }

    Ok(attempts)
}

fn auth_snapshot_allows_cross_format_candidate(
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

fn candidate_api_formats_for_spec(spec: LocalStandardSpec) -> &'static [&'static str] {
    match spec.mode {
        LocalStandardSourceMode::Chat | LocalStandardSourceMode::Cli => &[
            "openai:chat",
            "openai:cli",
            "openai:compact",
            "claude:chat",
            "claude:cli",
            "gemini:chat",
            "gemini:cli",
        ],
    }
}

fn extract_gemini_model_from_path(path: &str) -> Option<String> {
    let marker = "/models/";
    let start = path.find(marker)? + marker.len();
    let tail = &path[start..];
    let end = tail.find(':').unwrap_or(tail.len());
    let model = tail[..end].trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}
