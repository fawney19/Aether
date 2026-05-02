use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::{
    materialize_local_execution_candidates_with_serving, LocalCandidateResolutionMode,
};
use crate::ai_serving::planner::candidate_metadata::{
    build_local_execution_candidate_contract_metadata,
    build_local_execution_candidate_contract_metadata_for_candidate,
    LocalExecutionCandidateMetadataParts,
};
use crate::ai_serving::planner::candidate_source::{
    preselect_local_execution_candidates_with_serving, LocalCandidatePreselectionKeyMode,
};
use crate::ai_serving::planner::common::extract_requested_model_from_request;
use crate::ai_serving::planner::decision_input::{
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::planner::spec_metadata::local_standard_spec_metadata;
use crate::ai_serving::{
    ai_local_execution_contract_for_formats, extract_pool_sticky_session_token,
    resolve_local_decision_execution_runtime_auth_context, GatewayControlDecision, PlannerAppState,
};
use crate::{AppState, GatewayError};

use super::{LocalStandardCandidateAttempt, LocalStandardDecisionInput, LocalStandardSpec};

pub(super) async fn resolve_local_standard_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Option<LocalStandardDecisionInput> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return None;
    };

    let requested_model = extract_requested_model_from_request(
        parts,
        body_json,
        spec_metadata
            .requested_model_family
            .expect("standard specs should declare requested-model family"),
    )?;

    let resolved_input = match resolve_local_authenticated_decision_input(
        state,
        auth_context,
        Some(requested_model.as_str()),
        None,
    )
    .await
    {
        Ok(Some(resolved_input)) => resolved_input,
        Ok(None) => return None,
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                api_format = spec_metadata.api_format,
                error = ?err,
                "gateway local standard decision auth snapshot read failed"
            );
            return None;
        }
    };

    let mut input = build_local_requested_model_decision_input(resolved_input, requested_model);
    input.request_auth_channel = decision.request_auth_channel.clone();
    Some(input)
}

pub(super) async fn materialize_local_standard_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalStandardDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Result<(Vec<LocalStandardCandidateAttempt>, usize), GatewayError> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::StandardDecision,
    );
    let preselection = preselect_local_execution_candidates_with_serving(
        planner_state,
        spec_metadata.api_format,
        &input.requested_model,
        spec_metadata.require_streaming,
        input.required_capabilities.as_ref(),
        &input.auth_snapshot,
        false,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
    )
    .await?;
    let outcome = materialize_local_execution_candidates_with_serving(
        planner_state,
        trace_id,
        spec_metadata.api_format,
        Some(&input.requested_model),
        Some(&input.auth_snapshot),
        input.required_capabilities.as_ref(),
        sticky_session_token.as_deref(),
        input.request_auth_channel.as_deref(),
        persistence_policy,
        preselection.candidates,
        preselection.skipped_candidates,
        LocalCandidateResolutionMode::Standard,
        |eligible| {
            let provider_api_format = eligible.provider_api_format.clone();
            let (execution_strategy, conversion_mode) = ai_local_execution_contract_for_formats(
                spec_metadata.api_format,
                &provider_api_format,
            );
            Some(build_local_execution_candidate_contract_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: provider_api_format.as_str(),
                    client_api_format: spec_metadata.api_format,
                    extra_fields: serde_json::Map::new(),
                },
                execution_strategy,
                conversion_mode,
                eligible.candidate.endpoint_api_format.as_str(),
            ))
        },
        |mut skipped_candidate| {
            let provider_api_format = skipped_candidate
                .transport
                .as_ref()
                .map(|transport| transport.endpoint.api_format.trim().to_ascii_lowercase())
                .unwrap_or_else(|| {
                    skipped_candidate
                        .candidate
                        .endpoint_api_format
                        .trim()
                        .to_ascii_lowercase()
                });
            let (execution_strategy, conversion_mode) = ai_local_execution_contract_for_formats(
                spec_metadata.api_format,
                &provider_api_format,
            );
            skipped_candidate.extra_data = Some(
                build_local_execution_candidate_contract_metadata_for_candidate(
                    &skipped_candidate.candidate,
                    skipped_candidate.transport_ref(),
                    provider_api_format.as_str(),
                    spec_metadata.api_format,
                    serde_json::Map::new(),
                    execution_strategy,
                    conversion_mode,
                    provider_api_format.as_str(),
                ),
            );
            skipped_candidate
        },
    )
    .await;

    Ok((outcome.attempts, outcome.candidate_count))
}
