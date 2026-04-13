use serde_json::json;
use tracing::warn;
use uuid::Uuid;

use crate::ai_pipeline::planner::candidate_affinity::{
    rank_local_execution_candidates, remember_scheduler_affinity_for_candidate,
};
use crate::ai_pipeline::{
    resolve_local_decision_execution_runtime_auth_context, ConversionMode, ExecutionStrategy,
    GatewayControlDecision, PlannerAppState,
};
use crate::clock::{current_unix_ms, current_unix_secs};
use crate::{append_execution_contract_fields_to_value, AppState, GatewayError};

use super::{
    LocalSameFormatProviderCandidateAttempt, LocalSameFormatProviderDecisionInput,
    LocalSameFormatProviderFamily, LocalSameFormatProviderSpec,
};

pub(crate) async fn resolve_local_same_format_provider_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Option<LocalSameFormatProviderDecisionInput> {
    let planner_state = PlannerAppState::new(state);
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return None;
    };

    let requested_model = match spec.family {
        LocalSameFormatProviderFamily::Standard => body_json
            .get("model")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)?,
        LocalSameFormatProviderFamily::Gemini => {
            super::super::request::extract_gemini_model_from_path(parts.uri.path())?
        }
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
                "gateway local same-format decision auth snapshot read failed"
            );
            return None;
        }
    };

    let required_capabilities = planner_state
        .resolve_request_candidate_required_capabilities(
            &auth_context.user_id,
            &auth_context.api_key_id,
            Some(requested_model.as_str()),
            None,
        )
        .await;

    Some(LocalSameFormatProviderDecisionInput {
        auth_context,
        requested_model,
        auth_snapshot,
        required_capabilities,
    })
}

pub(crate) async fn materialize_local_same_format_provider_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalSameFormatProviderDecisionInput,
    spec: LocalSameFormatProviderSpec,
) -> Result<(Vec<LocalSameFormatProviderCandidateAttempt>, usize), GatewayError> {
    let planner_state = PlannerAppState::new(state);
    let candidates = planner_state
        .list_selectable_candidates(
            spec.api_format,
            &input.requested_model,
            spec.require_streaming,
            input.required_capabilities.as_ref(),
            Some(&input.auth_snapshot),
            current_unix_secs(),
        )
        .await?;
    let candidates = rank_local_execution_candidates(
        planner_state,
        candidates,
        spec.api_format,
        input.required_capabilities.as_ref(),
    )
    .await;
    let candidate_count = candidates.len();

    let created_at_unix_ms = current_unix_ms();
    let mut attempts = Vec::with_capacity(candidates.len());
    let mut affinity_remembered = false;
    for (candidate_index, candidate) in candidates.into_iter().enumerate() {
        let generated_candidate_id = Uuid::new_v4().to_string();
        if !affinity_remembered {
            remember_scheduler_affinity_for_candidate(
                planner_state,
                Some(&input.auth_snapshot),
                spec.api_format,
                &input.requested_model,
                &candidate,
            );
            affinity_remembered = true;
        }
        let extra_data = append_execution_contract_fields_to_value(
            json!({
                "provider_api_format": spec.api_format,
                "client_api_format": spec.api_format,
                "global_model_id": candidate.global_model_id.clone(),
                "global_model_name": candidate.global_model_name.clone(),
                "model_id": candidate.model_id.clone(),
                "selected_provider_model_name": candidate.selected_provider_model_name.clone(),
                "mapping_matched_model": candidate.mapping_matched_model.clone(),
                "provider_name": candidate.provider_name.clone(),
                "key_name": candidate.key_name.clone(),
            }),
            ExecutionStrategy::LocalSameFormat,
            ConversionMode::None,
            spec.api_format,
            spec.api_format,
        );

        let candidate_id = planner_state
            .persist_available_local_candidate(
                trace_id,
                &input.auth_context.user_id,
                &input.auth_context.api_key_id,
                &candidate,
                candidate_index as u32,
                &generated_candidate_id,
                input.required_capabilities.as_ref(),
                Some(extra_data),
                created_at_unix_ms,
                "gateway local same-format decision request candidate upsert failed",
            )
            .await;

        attempts.push(LocalSameFormatProviderCandidateAttempt {
            candidate,
            candidate_index: candidate_index as u32,
            candidate_id,
        });
    }

    Ok((attempts, candidate_count))
}
