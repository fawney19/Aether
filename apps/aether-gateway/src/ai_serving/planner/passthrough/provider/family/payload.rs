use serde_json::json;

use crate::ai_serving::ai_local_execution_contract_for_formats;
use crate::ai_serving::build_request_trace_proxy_value;
use crate::ai_serving::planner::candidate_materialization::{
    mark_skipped_local_execution_candidate, mark_skipped_local_execution_candidate_with_extra_data,
    mark_skipped_local_execution_candidate_with_failure_diagnostic,
};
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::planner::report_context::{
    build_local_execution_report_context, LocalExecutionReportContextParts,
};
use crate::ai_serving::planner::spec_metadata::local_same_format_provider_spec_metadata;
use crate::ai_serving::planner::CandidateFailureDiagnostic;
use crate::ai_serving::planner::{
    build_ai_execution_decision_response, AiExecutionDecisionResponseParts,
};
use crate::ai_serving::transport::{
    resolve_transport_execution_timeouts, resolve_transport_tls_profile,
};
use crate::{
    append_execution_contract_fields_to_value, append_local_failover_policy_to_value,
    AiExecutionDecision, AppState,
};
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use super::request::resolve_local_same_format_provider_candidate_payload_parts;
use super::{
    LocalSameFormatProviderCandidateAttempt, LocalSameFormatProviderDecisionInput,
    LocalSameFormatProviderSpec,
};

pub(crate) async fn maybe_build_local_same_format_provider_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalSameFormatProviderDecisionInput,
    attempt: LocalSameFormatProviderCandidateAttempt,
    spec: LocalSameFormatProviderSpec,
) -> Option<AiExecutionDecision> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let LocalSameFormatProviderCandidateAttempt {
        eligible,
        candidate_index,
        candidate_id,
        ..
    } = &attempt;
    let candidate = &eligible.candidate;
    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats(spec_metadata.api_format, spec_metadata.api_format);
    let resolved = resolve_local_same_format_provider_candidate_payload_parts(
        state, parts, trace_id, body_json, input, &attempt, spec,
    )
    .await?;

    let prompt_cache_key = resolved
        .provider_request_body
        .get("prompt_cache_key")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let proxy = state
        .resolve_transport_proxy_snapshot_with_tunnel_affinity(&resolved.transport)
        .await;
    let tls_profile = resolve_transport_tls_profile(&resolved.transport);
    let mut extra_fields = serde_json::Map::new();
    if let Some(proxy_value) =
        build_request_trace_proxy_value(Some(&resolved.transport), proxy.as_ref())
    {
        extra_fields.insert("proxy".to_string(), proxy_value);
    }
    if resolved.is_kiro {
        extra_fields.insert(
            "envelope_name".to_string(),
            json!(crate::ai_serving::transport::kiro::KIRO_ENVELOPE_NAME),
        );
    } else if resolved.is_antigravity {
        extra_fields.insert(
            "envelope_name".to_string(),
            json!(super::super::ANTIGRAVITY_ENVELOPE_NAME),
        );
    }
    let report_context = append_local_failover_policy_to_value(
        append_execution_contract_fields_to_value(
            build_local_execution_report_context(LocalExecutionReportContextParts {
                auth_context: &input.auth_context,
                request_id: trace_id,
                candidate_id,
                attempt_identity: attempt.attempt_identity(),
                model: &input.requested_model,
                provider_name: &resolved.transport.provider.name,
                provider_id: &candidate.provider_id,
                endpoint_id: &candidate.endpoint_id,
                key_id: &candidate.key_id,
                key_name: Some(&candidate.key_name),
                model_id: Some(&candidate.model_id),
                global_model_id: Some(&candidate.global_model_id),
                global_model_name: Some(&candidate.global_model_name),
                provider_api_format: spec_metadata.api_format,
                client_api_format: spec_metadata.api_format,
                mapped_model: Some(&resolved.mapped_model),
                candidate_group_id: eligible.orchestration.candidate_group_id.as_deref(),
                ranking: eligible.ranking.as_ref(),
                upstream_url: Some(&resolved.upstream_url),
                header_rules: resolved.transport.endpoint.header_rules.as_ref(),
                body_rules: resolved.transport.endpoint.body_rules.as_ref(),
                provider_request_method: Some(serde_json::Value::Null),
                provider_request_headers: Some(&resolved.provider_request_headers),
                original_headers: &parts.headers,
                request_origin: Some(crate::ai_serving::request_origin_from_parts(parts)),
                original_request_body_json: Some(body_json),
                original_request_body_base64: None,
                client_requested_stream: body_json
                    .get("stream")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                upstream_is_stream: resolved.upstream_is_stream,
                has_envelope: resolved.is_kiro || resolved.is_antigravity,
                needs_conversion: false,
                extra_fields,
            }),
            execution_strategy,
            conversion_mode,
            spec_metadata.api_format,
            spec_metadata.api_format,
        ),
        &resolved.transport,
    );
    let super::request::LocalSameFormatProviderCandidatePayloadParts {
        transport,
        is_antigravity: _,
        is_kiro: _,
        auth_header,
        auth_value,
        mapped_model,
        report_kind,
        upstream_is_stream,
        upstream_url,
        provider_request_headers,
        provider_request_body,
    } = resolved;

    Some(build_ai_execution_decision_response(
        AiExecutionDecisionResponseParts {
            decision_is_stream: spec_metadata.require_streaming,
            decision_kind: spec_metadata.decision_kind.to_string(),
            execution_strategy,
            conversion_mode,
            request_id: trace_id.to_string(),
            candidate_id: candidate_id.to_string(),
            provider_name: transport.provider.name.clone(),
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
            upstream_base_url: transport.endpoint.base_url.clone(),
            upstream_url,
            provider_request_method: None,
            auth_header,
            auth_value,
            provider_api_format: spec_metadata.api_format.to_string(),
            client_api_format: spec_metadata.api_format.to_string(),
            model_name: input.requested_model.clone(),
            mapped_model,
            prompt_cache_key,
            provider_request_headers,
            provider_request_body: Some(provider_request_body),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            proxy,
            tls_profile,
            timeouts: resolve_transport_execution_timeouts(&transport),
            upstream_is_stream,
            report_kind: Some(report_kind.to_string()),
            report_context: Some(report_context),
            auth_context: input.auth_context.clone(),
        },
    ))
}

pub(super) async fn mark_skipped_local_same_format_provider_candidate(
    state: &AppState,
    input: &LocalSameFormatProviderDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::SameFormatProviderDecision,
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
pub(super) async fn mark_skipped_local_same_format_provider_candidate_with_extra_data(
    state: &AppState,
    input: &LocalSameFormatProviderDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    extra_data: Option<serde_json::Value>,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::SameFormatProviderDecision,
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
pub(super) async fn mark_skipped_local_same_format_provider_candidate_with_failure_diagnostic(
    state: &AppState,
    input: &LocalSameFormatProviderDecisionInput,
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
        LocalCandidatePersistencePolicyKind::SameFormatProviderDecision,
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
