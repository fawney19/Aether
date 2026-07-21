use tracing::warn;

use crate::ai_serving::planner::candidate_materialization::{
    build_local_execution_candidate_attempt_source_with_serving,
    mark_skipped_local_execution_candidate,
    mark_skipped_local_execution_candidate_with_failure_diagnostic,
    materialize_local_execution_candidates_with_serving, LocalCandidateResolutionMode,
};
use crate::ai_serving::planner::candidate_metadata::{
    build_local_execution_candidate_metadata,
    build_local_execution_candidate_metadata_for_candidate, LocalExecutionCandidateMetadataParts,
};
use crate::ai_serving::planner::candidate_resolution::SkippedLocalExecutionCandidate;
use crate::ai_serving::planner::candidate_source::auth_snapshot_allows_cross_format_candidate;
use crate::ai_serving::planner::decision_input::{
    attach_routing_policy_to_local_requested_model_input,
    build_local_requested_model_decision_input, resolve_local_authenticated_decision_input,
};
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::planner::spec_metadata::local_openai_image_spec_metadata;
use crate::ai_serving::{
    extract_pool_sticky_session_token, request_candidate_api_formats,
    resolve_local_decision_execution_runtime_auth_context, CandidateFailureDiagnostic,
    ExecutionRuntimeAuthContext, GatewayControlDecision, PlannerAppState,
};
use crate::client_session_affinity::client_session_affinity_from_parts;
use crate::clock::current_unix_secs;
use crate::{AppState, GatewayError};
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

pub(super) use crate::ai_serving::planner::candidate_materialization::LocalExecutionCandidateAttempt as LocalOpenAiImageCandidateAttempt;
pub(super) use crate::ai_serving::planner::candidate_materialization::LocalExecutionCandidateAttemptSource as LocalOpenAiImageCandidateAttemptSource;
pub(super) use crate::ai_serving::planner::decision_input::LocalRequestedModelDecisionInput as LocalOpenAiImageDecisionInput;

use super::request::resolve_requested_image_model_for_request;

pub(super) async fn resolve_local_openai_image_decision_input(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<Option<LocalOpenAiImageDecisionInput>, GatewayError> {
    let Some(auth_context) = resolve_local_openai_image_auth_context(decision) else {
        return Ok(None);
    };

    let Some(requested_model) =
        resolve_requested_image_model_for_request(parts, body_json, body_base64)
    else {
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
                error = ?err,
                "gateway local openai image decision auth snapshot read failed"
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
        "openai:image",
    )
    .await
    {
        warn!(
            trace_id = %trace_id,
            error = ?err,
            "gateway local openai image decision routing profile resolution failed"
        );
        return Err(err);
    }
    Ok(Some(input))
}

fn resolve_local_openai_image_auth_context(
    decision: &GatewayControlDecision,
) -> Option<ExecutionRuntimeAuthContext> {
    resolve_local_decision_execution_runtime_auth_context(decision)
}

pub(super) async fn list_local_openai_image_candidate_attempts(
    state: &AppState,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    body_json: &serde_json::Value,
    api_format: &str,
    decision_kind: &str,
) -> Option<Vec<LocalOpenAiImageCandidateAttempt>> {
    let candidate_api_formats = image_candidate_api_formats(api_format);
    let mut attempts = Vec::new();
    for candidate_api_format in candidate_api_formats {
        let matches_client_format = candidate_api_format == api_format;
        let planner_state = PlannerAppState::new(state);
        let (mut candidates, preselection_skipped) = match planner_state
            .list_selectable_candidates_with_skip_reasons(
                candidate_api_format,
                &input.requested_model,
                false,
                input.required_capabilities.as_ref(),
                matches_client_format.then_some(&input.auth_snapshot),
                input.client_session_affinity.as_ref(),
                current_unix_secs(),
                false,
            )
            .await
        {
            Ok(candidates) => candidates,
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    decision_kind,
                    api_format = candidate_api_format,
                    error = ?err,
                    "gateway local openai image decision scheduler selection failed"
                );
                continue;
            }
        };
        if !matches_client_format {
            candidates.retain(|candidate| {
                auth_snapshot_allows_cross_format_candidate(
                    &input.auth_snapshot,
                    &input.requested_model,
                    None,
                    candidate,
                )
            });
        }
        attempts.extend(
            materialize_local_openai_image_candidate_attempts(
                planner_state,
                trace_id,
                input,
                body_json,
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
                api_format,
            )
            .await,
        );
    }

    Some(attempts)
}

pub(super) async fn build_local_openai_image_candidate_attempt_source<'a>(
    state: &'a AppState,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    body_json: &serde_json::Value,
    api_format: &str,
    decision_kind: &str,
) -> Result<Option<(LocalOpenAiImageCandidateAttemptSource<'a>, usize)>, GatewayError> {
    let planner_state = PlannerAppState::new(state);
    let mut candidates = Vec::new();
    let mut preselection_skipped = Vec::new();
    for candidate_api_format in image_candidate_api_formats(api_format) {
        let matches_client_format = candidate_api_format == api_format;
        match planner_state
            .list_selectable_candidates_with_skip_reasons(
                candidate_api_format,
                &input.requested_model,
                false,
                input.required_capabilities.as_ref(),
                matches_client_format.then_some(&input.auth_snapshot),
                input.client_session_affinity.as_ref(),
                current_unix_secs(),
                false,
            )
            .await
        {
            Ok((mut format_candidates, mut format_skipped)) => {
                if !matches_client_format {
                    format_candidates.retain(|candidate| {
                        auth_snapshot_allows_cross_format_candidate(
                            &input.auth_snapshot,
                            &input.requested_model,
                            None,
                            candidate,
                        )
                    });
                    format_skipped.retain(|candidate| {
                        auth_snapshot_allows_cross_format_candidate(
                            &input.auth_snapshot,
                            &input.requested_model,
                            None,
                            &candidate.candidate,
                        )
                    });
                }
                candidates.append(&mut format_candidates);
                preselection_skipped.append(&mut format_skipped);
            }
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    decision_kind,
                    api_format = candidate_api_format,
                    error = ?err,
                    "gateway local openai image decision scheduler selection failed"
                );
                continue;
            }
        }
    }

    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::ImageDecision,
    );

    let (source, candidate_count) = build_local_execution_candidate_attempt_source_with_serving(
        planner_state,
        trace_id,
        api_format,
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
        LocalCandidateResolutionMode::WithoutTransportPairGate,
        |eligible| {
            Some(build_local_execution_candidate_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: eligible.provider_api_format.as_str(),
                    client_api_format: api_format,
                    extra_fields: serde_json::Map::new(),
                },
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
            skipped_candidate.extra_data =
                Some(build_local_execution_candidate_metadata_for_candidate(
                    &skipped_candidate.candidate,
                    skipped_candidate.transport_ref(),
                    provider_api_format.as_str(),
                    api_format,
                    serde_json::Map::new(),
                ));
            skipped_candidate
        },
    )
    .await;

    Ok(Some((source, candidate_count)))
}

async fn materialize_local_openai_image_candidate_attempts(
    state: PlannerAppState<'_>,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    body_json: &serde_json::Value,
    candidates: Vec<SchedulerMinimalCandidateSelectionCandidate>,
    preselection_skipped: Vec<SkippedLocalExecutionCandidate>,
    api_format: &str,
) -> Vec<LocalOpenAiImageCandidateAttempt> {
    let sticky_session_token = extract_pool_sticky_session_token(body_json);
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::ImageDecision,
    );
    let outcome = materialize_local_execution_candidates_with_serving(
        state,
        trace_id,
        api_format,
        Some(&input.requested_model),
        Some(&input.auth_snapshot),
        input.client_session_affinity.as_ref(),
        input.required_capabilities.as_ref(),
        input.routing_policy.as_ref(),
        sticky_session_token.as_deref(),
        input.request_auth_channel.as_deref(),
        persistence_policy,
        candidates,
        preselection_skipped,
        LocalCandidateResolutionMode::WithoutTransportPairGate,
        |eligible| {
            Some(build_local_execution_candidate_metadata(
                LocalExecutionCandidateMetadataParts {
                    eligible,
                    provider_api_format: eligible.provider_api_format.as_str(),
                    client_api_format: api_format,
                    extra_fields: serde_json::Map::new(),
                },
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
            skipped_candidate.extra_data =
                Some(build_local_execution_candidate_metadata_for_candidate(
                    &skipped_candidate.candidate,
                    skipped_candidate.transport_ref(),
                    provider_api_format.as_str(),
                    api_format,
                    serde_json::Map::new(),
                ));
            skipped_candidate
        },
    )
    .await;

    outcome.attempts
}

fn image_candidate_api_formats(api_format: &str) -> Vec<&'static str> {
    if api_format.trim().eq_ignore_ascii_case("openai:image") {
        vec!["openai:image", "gemini:generate_content"]
    } else {
        request_candidate_api_formats(api_format, false)
    }
}

pub(super) async fn mark_skipped_local_openai_image_candidate(
    state: &AppState,
    input: &LocalOpenAiImageDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::ImageDecision,
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

pub(super) async fn mark_skipped_local_openai_image_candidate_with_failure_diagnostic(
    state: &AppState,
    input: &LocalOpenAiImageDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    diagnostic: CandidateFailureDiagnostic,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::ImageDecision,
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
