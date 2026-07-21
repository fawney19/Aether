use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::{
    build_lazy_requested_model_execution_candidate_attempt_source_with_serving,
    build_local_execution_candidate_attempt_source_with_serving,
    mark_skipped_local_execution_candidate, mark_skipped_local_execution_candidate_with_extra_data,
    mark_skipped_local_execution_candidate_with_failure_diagnostic,
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
use crate::ai_serving::planner::common::extract_standard_requested_model;
use crate::ai_serving::planner::decision_input::{
    attach_routing_policy_to_local_requested_model_input,
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::planner::runtime_miss::set_local_runtime_miss_diagnostic_reason;
use crate::ai_serving::planner::spec_metadata::local_openai_responses_spec_metadata;
use crate::ai_serving::planner::CandidateFailureDiagnostic;
use crate::ai_serving::{
    ai_local_execution_contract_for_formats, extract_pool_sticky_session_token,
    openai_responses_request_operation, resolve_local_decision_execution_runtime_auth_context,
    ExecutionRuntimeAuthContext, GatewayControlDecision, PlannerAppState,
};
use crate::client_session_affinity::client_session_affinity_from_parts;
use crate::{AppState, GatewayError};

use super::super::super::openai_request_is_image_generation_intent;
use super::LocalOpenAiResponsesSpec;

pub(crate) use crate::ai_serving::planner::candidate_materialization::LocalExecutionCandidateAttempt as LocalOpenAiResponsesCandidateAttempt;
pub(crate) use crate::ai_serving::planner::candidate_materialization::LocalExecutionCandidateAttemptSource as LocalOpenAiResponsesCandidateAttemptSource;
pub(crate) use crate::ai_serving::planner::decision_input::LocalRequestedModelDecisionInput as LocalOpenAiResponsesDecisionInput;

pub(crate) async fn resolve_local_openai_responses_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<Option<LocalOpenAiResponsesDecisionInput>, GatewayError> {
    let Some(auth_context) = resolve_local_decision_execution_runtime_auth_context(decision) else {
        warn!(
            trace_id = %trace_id,
            route_class = ?decision.route_class,
            route_family = ?decision.route_family,
            route_kind = ?decision.route_kind,
            "gateway local openai responses decision skipped: missing_auth_context"
        );
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            plan_kind,
            extract_standard_requested_model(body_json).as_deref(),
            "missing_auth_context",
        );
        return Ok(None);
    };

    let Some(requested_model) = extract_standard_requested_model(body_json) else {
        warn!(
            trace_id = %trace_id,
            "gateway local openai responses decision skipped: missing_requested_model"
        );
        set_local_runtime_miss_diagnostic_reason(
            state,
            trace_id,
            decision,
            plan_kind,
            None,
            "missing_requested_model",
        );
        return Ok(None);
    };

    let resolved_input = match resolve_local_authenticated_decision_input(
        state,
        auth_context.clone(),
        Some(requested_model.as_str()),
        decision.auth_endpoint_signature.as_deref(),
        None,
        &decision.model_directive_policy,
    )
    .await
    {
        Ok(Some(resolved_input)) => resolved_input,
        Ok(None) => {
            warn!(
                trace_id = %trace_id,
                user_id = %auth_context.user_id,
                api_key_id = %auth_context.api_key_id,
                "gateway local openai responses decision skipped: auth_snapshot_missing"
            );
            set_local_runtime_miss_diagnostic_reason(
                state,
                trace_id,
                decision,
                plan_kind,
                Some(requested_model.as_str()),
                "auth_snapshot_missing",
            );
            return Ok(None);
        }
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                error = ?err,
                "gateway local openai responses decision auth snapshot read failed"
            );
            set_local_runtime_miss_diagnostic_reason(
                state,
                trace_id,
                decision,
                plan_kind,
                Some(requested_model.as_str()),
                "auth_snapshot_read_failed",
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
        "openai:responses",
    )
    .await
    {
        warn!(
            trace_id = %trace_id,
            error = ?err,
            "gateway local openai responses decision routing profile resolution failed"
        );
        return Err(err);
    }
    Ok(Some(input))
}

pub(crate) async fn materialize_local_openai_responses_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalOpenAiResponsesDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalOpenAiResponsesSpec,
) -> Result<(Vec<LocalOpenAiResponsesCandidateAttempt>, usize), GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let request_operation = openai_responses_request_operation(spec_metadata.api_format, body_json);
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
    );
    let preselection = preselect_local_execution_candidates_with_serving(
        planner_state,
        &input.model_directive_policy,
        spec_metadata.api_format,
        &input.requested_model,
        request_operation,
        spec_metadata.require_streaming,
        input.required_capabilities.as_ref(),
        &input.auth_snapshot,
        input.routing_policy.as_ref(),
        input.client_session_affinity.as_ref(),
        true,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
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

pub(crate) async fn build_local_openai_responses_candidate_attempt_source<'a>(
    state: &'a AppState,
    trace_id: &str,
    input: &LocalOpenAiResponsesDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalOpenAiResponsesSpec,
) -> Result<(LocalOpenAiResponsesCandidateAttemptSource<'a>, usize), GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let request_operation = openai_responses_request_operation(spec_metadata.api_format, body_json);
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
    );
    if openai_request_is_image_generation_intent(&input.requested_model, body_json) {
        let (image_candidates, image_candidate_count) =
            build_local_openai_responses_image_candidate_attempt_source(
                state, trace_id, input, body_json, spec,
            )
            .await?;
        if image_candidate_count > 0 {
            return Ok((image_candidates, image_candidate_count));
        }
    }
    Ok(
        build_lazy_requested_model_execution_candidate_attempt_source_with_serving(
            planner_state,
            &input.model_directive_policy,
            trace_id,
            spec_metadata.api_format,
            &input.requested_model,
            request_operation,
            spec_metadata.require_streaming,
            &input.auth_snapshot,
            input.client_session_affinity.as_ref(),
            input.required_capabilities.as_ref(),
            input.routing_policy.as_ref(),
            sticky_session_token.as_deref(),
            input.request_auth_channel.as_deref(),
            persistence_policy,
            true,
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
        .await,
    )
}

pub(crate) async fn build_local_openai_responses_image_candidate_attempt_source<'a>(
    state: &'a AppState,
    trace_id: &str,
    input: &LocalOpenAiResponsesDecisionInput,
    body_json: &serde_json::Value,
    spec: LocalOpenAiResponsesSpec,
) -> Result<(LocalOpenAiResponsesCandidateAttemptSource<'a>, usize), GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
    );
    let preselection = preselect_local_execution_candidates_for_api_formats_with_serving(
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
        true,
        LocalCandidatePreselectionKeyMode::ProviderEndpointKeyModelAndApiFormat,
        vec!["openai:image".to_string()],
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
        preselection.candidates,
        preselection.skipped_candidates,
        LocalCandidateResolutionMode::WithoutTransportPairGate,
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
    .await)
}

pub(crate) async fn mark_skipped_local_openai_responses_candidate(
    state: &AppState,
    input: &LocalOpenAiResponsesDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
    );
    mark_skipped_local_execution_candidate(
        state,
        trace_id,
        persistence_policy.skipped,
        candidate,
        candidate_index,
        candidate_id,
        skip_reason,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn mark_skipped_local_openai_responses_candidate_with_extra_data(
    state: &AppState,
    input: &LocalOpenAiResponsesDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    extra_data: Option<serde_json::Value>,
) {
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
    );
    mark_skipped_local_execution_candidate_with_extra_data(
        state,
        trace_id,
        persistence_policy.skipped,
        candidate,
        candidate_index,
        candidate_id,
        skip_reason,
        extra_data,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
    state: &AppState,
    input: &LocalOpenAiResponsesDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    diagnostic: CandidateFailureDiagnostic,
) {
    let auth_context: &ExecutionRuntimeAuthContext = &input.auth_context;
    let persistence_policy = build_local_candidate_persistence_policy(
        auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::OpenAiResponsesDecision,
    );
    mark_skipped_local_execution_candidate_with_failure_diagnostic(
        state,
        trace_id,
        persistence_policy.skipped,
        candidate,
        candidate_index,
        candidate_id,
        skip_reason,
        diagnostic,
    )
    .await;
}
