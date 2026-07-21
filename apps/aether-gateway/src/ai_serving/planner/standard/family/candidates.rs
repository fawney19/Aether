use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::{
    build_lazy_requested_model_execution_candidate_attempt_source_with_serving,
    materialize_local_execution_candidates_with_serving, LocalCandidateResolutionMode,
    LocalExecutionCandidateAttemptSource,
};
use crate::ai_serving::planner::candidate_metadata::{
    build_local_execution_candidate_contract_metadata,
    build_local_execution_candidate_contract_metadata_for_candidate,
    LocalExecutionCandidateMetadataParts,
};
use crate::ai_serving::planner::candidate_source::{
    preselect_local_execution_candidates_for_api_formats_with_serving,
    preselect_local_execution_candidates_with_serving, LocalCandidatePreselectionKeyMode,
};
use crate::ai_serving::planner::common::extract_requested_model_from_request;
use crate::ai_serving::planner::decision_input::{
    attach_routing_policy_to_local_requested_model_input,
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::planner::spec_metadata::local_standard_spec_metadata;
use crate::ai_serving::{
    ai_local_execution_contract_for_formats, extract_pool_sticky_session_token,
    gemini_request_is_image_generation, resolve_local_decision_execution_runtime_auth_context,
    GatewayControlDecision, PlannerAppState,
};
use crate::client_session_affinity::client_session_affinity_from_parts;
use crate::{AppState, GatewayError};

use super::{LocalStandardCandidateAttempt, LocalStandardDecisionInput, LocalStandardSpec};

pub(super) async fn resolve_local_standard_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Result<Option<LocalStandardDecisionInput>, GatewayError> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return Ok(None);
    };

    let Some(requested_model) = extract_requested_model_from_request(
        parts,
        body_json,
        spec_metadata
            .requested_model_family
            .expect("standard specs should declare requested-model family"),
    ) else {
        return Ok(None);
    };

    let resolved_input = match resolve_local_authenticated_decision_input(
        state,
        auth_context,
        Some(requested_model.as_str()),
        decision.auth_endpoint_signature.as_deref(),
        None,
        &decision.model_directive_policy,
    )
    .await
    {
        Ok(Some(resolved_input)) => resolved_input,
        Ok(None) => return Ok(None),
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                api_format = spec_metadata.api_format,
                error = ?err,
                "gateway local standard decision auth snapshot read failed"
            );
            return Err(err);
        }
    };

    let mut input = build_local_requested_model_decision_input(resolved_input, requested_model);
    input.request_auth_channel = decision.request_auth_channel.clone();
    input.client_session_affinity = client_session_affinity_from_parts(parts, Some(body_json));
    if let Err(err) = attach_routing_policy_to_local_requested_model_input(
        state,
        parts,
        &mut input,
        body_json,
        spec_metadata.api_format,
    )
    .await
    {
        warn!(
            trace_id = %trace_id,
            api_format = spec_metadata.api_format,
            error = ?err,
            "gateway local standard decision routing profile resolution failed"
        );
        return Err(err);
    }
    Ok(Some(input))
}

pub(super) async fn materialize_local_standard_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalStandardDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Result<(Vec<LocalStandardCandidateAttempt>, usize), GatewayError> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let is_gemini_image_bridge = spec_metadata.api_format == "gemini:generate_content"
        && gemini_request_is_image_generation(body_json);
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::StandardDecision,
    );
    let preselection = preselect_local_execution_candidates_with_serving(
        planner_state,
        &input.model_directive_policy,
        spec_metadata.api_format,
        &input.requested_model,
        None,
        false,
        input.required_capabilities.as_ref(),
        &input.auth_snapshot,
        input.routing_policy.as_ref(),
        input.client_session_affinity.as_ref(),
        false,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
    )
    .await?;
    let (candidates, skipped_candidates) = maybe_append_gemini_image_openai_image_preselection(
        state,
        planner_state,
        trace_id,
        input,
        body_json,
        spec,
        preselection.candidates,
        preselection.skipped_candidates,
    )
    .await?;
    let outcome = materialize_local_execution_candidates_with_serving(
        planner_state,
        trace_id,
        spec_metadata.api_format,
        Some(&input.requested_model),
        Some(&input.auth_snapshot),
        input.client_session_affinity.as_ref(),
        input.required_capabilities.as_ref(),
        input.routing_policy.as_ref(),
        sticky_session_token.as_deref(),
        input.request_auth_channel.as_deref(),
        persistence_policy,
        candidates,
        skipped_candidates,
        if is_gemini_image_bridge {
            LocalCandidateResolutionMode::WithoutTransportPairGate
        } else {
            LocalCandidateResolutionMode::Standard
        },
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

pub(super) async fn build_local_standard_candidate_attempt_source<'a>(
    state: &'a AppState,
    trace_id: &str,
    input: &LocalStandardDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
) -> Result<(LocalExecutionCandidateAttemptSource<'a>, usize), GatewayError> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::StandardDecision,
    );
    if spec_metadata.api_format == "gemini:generate_content"
        && gemini_request_is_image_generation(body_json)
    {
        let (attempts, candidate_count) =
            materialize_local_standard_candidate_attempts(state, trace_id, input, body_json, spec)
                .await?;
        let source =
            LocalExecutionCandidateAttemptSource::from_static_attempts_for_image_bridge(attempts);
        return Ok((source, candidate_count));
    }

    let (source, candidate_count) =
        build_lazy_requested_model_execution_candidate_attempt_source_with_serving(
            planner_state,
            &input.model_directive_policy,
            trace_id,
            spec_metadata.api_format,
            &input.requested_model,
            None,
            spec_metadata.require_streaming,
            &input.auth_snapshot,
            input.client_session_affinity.as_ref(),
            input.required_capabilities.as_ref(),
            input.routing_policy.as_ref(),
            sticky_session_token.as_deref(),
            input.request_auth_channel.as_deref(),
            persistence_policy,
            false,
            LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
            LocalCandidateResolutionMode::Standard,
            move |eligible| {
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
            move |mut skipped_candidate| {
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
    Ok((source, candidate_count))
}

#[allow(clippy::too_many_arguments)]
async fn maybe_append_gemini_image_openai_image_preselection(
    state: &AppState,
    planner_state: PlannerAppState<'_>,
    trace_id: &str,
    input: &LocalStandardDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalStandardSpec,
    mut candidates: Vec<aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate>,
    mut skipped_candidates: Vec<
        crate::ai_serving::planner::candidate_resolution::SkippedLocalExecutionCandidate,
    >,
) -> Result<
    (
        Vec<aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate>,
        Vec<crate::ai_serving::planner::candidate_resolution::SkippedLocalExecutionCandidate>,
    ),
    GatewayError,
> {
    let spec_metadata = local_standard_spec_metadata(spec);
    if spec_metadata.api_format != "gemini:generate_content"
        || !gemini_request_is_image_generation(body_json)
    {
        return Ok((candidates, skipped_candidates));
    }

    let image_preselection = preselect_local_execution_candidates_for_api_formats_with_serving(
        planner_state,
        &input.model_directive_policy,
        spec_metadata.api_format,
        &input.requested_model,
        None,
        spec_metadata.require_streaming,
        input.required_capabilities.as_ref(),
        &input.auth_snapshot,
        input.routing_policy.as_ref(),
        input.client_session_affinity.as_ref(),
        false,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
        vec!["openai:image".to_string()],
    )
    .await?;
    candidates.extend(image_preselection.candidates);
    skipped_candidates.extend(image_preselection.skipped_candidates);
    let _ = (state, trace_id);
    Ok((candidates, skipped_candidates))
}
