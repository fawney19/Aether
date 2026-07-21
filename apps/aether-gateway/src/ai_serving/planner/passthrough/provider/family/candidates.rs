use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::{
    build_local_execution_candidate_attempt_source_with_serving,
    materialize_local_execution_candidates_with_serving, LocalCandidateResolutionMode,
    LocalExecutionCandidateAttemptSource,
};
use crate::ai_serving::planner::candidate_metadata::{
    build_local_execution_candidate_contract_metadata,
    build_local_execution_candidate_contract_metadata_for_candidate,
    LocalExecutionCandidateMetadataParts,
};
use crate::ai_serving::planner::candidate_resolution::SkippedLocalExecutionCandidate;
use crate::ai_serving::planner::common::extract_requested_model_from_request;
use crate::ai_serving::planner::decision_input::{
    attach_routing_policy_to_local_requested_model_input,
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::planner::spec_metadata::local_same_format_provider_spec_metadata;
use crate::ai_serving::{
    ai_local_execution_contract_for_formats, extract_pool_sticky_session_token,
    resolve_local_decision_execution_runtime_auth_context, GatewayControlDecision, PlannerAppState,
};
use crate::client_session_affinity::client_session_affinity_from_api_request;
use crate::clock::current_unix_secs;
use crate::{AppState, GatewayError};

use super::{
    LocalSameFormatProviderCandidateAttempt, LocalSameFormatProviderDecisionInput,
    LocalSameFormatProviderSpec,
};

pub(crate) async fn resolve_local_same_format_provider_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<Option<LocalSameFormatProviderDecisionInput>, GatewayError> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        return Ok(None);
    };

    let Some(requested_model) = extract_requested_model_from_request(
        parts,
        body_json,
        spec_metadata
            .requested_model_family
            .expect("same-format provider specs should declare requested-model family"),
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
                "gateway local same-format decision auth snapshot read failed"
            );
            return Err(err);
        }
    };

    let mut input = build_local_requested_model_decision_input(resolved_input, requested_model);
    input.request_auth_channel = decision.request_auth_channel.clone();
    input.client_session_affinity = client_session_affinity_from_api_request(
        spec_metadata.api_format,
        &parts.headers,
        Some(body_json),
    );
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
            "gateway local same-format decision routing profile resolution failed"
        );
        return Err(err);
    }
    Ok(Some(input))
}

pub(crate) async fn materialize_local_same_format_provider_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalSameFormatProviderDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<(Vec<LocalSameFormatProviderCandidateAttempt>, usize), GatewayError> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::SameFormatProviderDecision,
    );
    let model_directive_resolution = input
        .model_directive_policy
        .resolve_reasoning(spec_metadata.api_format, Some(&input.requested_model));
    let routing_model = model_directive_resolution
        .base_model()
        .unwrap_or(&input.requested_model);
    let (candidates, preselection_skipped) = planner_state
        .list_selectable_candidates_with_skip_reasons(
            spec_metadata.api_format,
            routing_model,
            spec_metadata.require_streaming,
            input.required_capabilities.as_ref(),
            Some(&input.auth_snapshot),
            input.client_session_affinity.as_ref(),
            current_unix_secs(),
            false,
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
        preselection_skipped
            .into_iter()
            .map(|item| SkippedLocalExecutionCandidate {
                candidate: item.candidate,
                skip_reason: item.skip_reason,
                transport: None,
                ranking: None,
                extra_data: None,
            })
            .collect(),
        LocalCandidateResolutionMode::Standard,
        |eligible| {
            let (execution_strategy, conversion_mode) = ai_local_execution_contract_for_formats(
                spec_metadata.api_format,
                spec_metadata.api_format,
            );
            Some(build_local_execution_candidate_contract_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: spec_metadata.api_format,
                    client_api_format: spec_metadata.api_format,
                    extra_fields: serde_json::Map::new(),
                },
                execution_strategy,
                conversion_mode,
                spec_metadata.api_format,
            ))
        },
        |mut skipped_candidate| {
            let provider_api_format = skipped_candidate
                .transport
                .as_ref()
                .map(|transport| transport.endpoint.api_format.trim().to_ascii_lowercase())
                .unwrap_or_else(|| spec_metadata.api_format.to_string());
            let (execution_strategy, conversion_mode) = ai_local_execution_contract_for_formats(
                spec_metadata.api_format,
                provider_api_format.as_str(),
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

pub(crate) async fn build_local_same_format_provider_candidate_attempt_source<'a>(
    state: &'a AppState,
    trace_id: &str,
    input: &LocalSameFormatProviderDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalSameFormatProviderSpec,
) -> Result<(LocalExecutionCandidateAttemptSource<'a>, usize), GatewayError> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::SameFormatProviderDecision,
    );
    let model_directive_resolution = input
        .model_directive_policy
        .resolve_reasoning(spec_metadata.api_format, Some(&input.requested_model));
    let routing_model = model_directive_resolution
        .base_model()
        .unwrap_or(&input.requested_model);
    let (candidates, preselection_skipped) = planner_state
        .list_selectable_candidates_with_skip_reasons(
            spec_metadata.api_format,
            routing_model,
            spec_metadata.require_streaming,
            input.required_capabilities.as_ref(),
            Some(&input.auth_snapshot),
            input.client_session_affinity.as_ref(),
            current_unix_secs(),
            false,
        )
        .await?;

    Ok(build_local_execution_candidate_attempt_source_with_serving(
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
        preselection_skipped
            .into_iter()
            .map(|item| SkippedLocalExecutionCandidate {
                candidate: item.candidate,
                skip_reason: item.skip_reason,
                transport: None,
                ranking: None,
                extra_data: None,
            })
            .collect(),
        LocalCandidateResolutionMode::Standard,
        |eligible| {
            let (execution_strategy, conversion_mode) = ai_local_execution_contract_for_formats(
                spec_metadata.api_format,
                spec_metadata.api_format,
            );
            Some(build_local_execution_candidate_contract_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: spec_metadata.api_format,
                    client_api_format: spec_metadata.api_format,
                    extra_fields: serde_json::Map::new(),
                },
                execution_strategy,
                conversion_mode,
                spec_metadata.api_format,
            ))
        },
        |mut skipped_candidate| {
            let provider_api_format = skipped_candidate
                .transport
                .as_ref()
                .map(|transport| transport.endpoint.api_format.trim().to_ascii_lowercase())
                .unwrap_or_else(|| spec_metadata.api_format.to_string());
            let (execution_strategy, conversion_mode) = ai_local_execution_contract_for_formats(
                spec_metadata.api_format,
                provider_api_format.as_str(),
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
    .await)
}
