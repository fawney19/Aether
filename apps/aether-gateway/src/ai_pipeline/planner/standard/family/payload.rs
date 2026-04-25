use crate::ai_pipeline::planner::candidate_materialization::mark_skipped_local_execution_candidate;
use crate::ai_pipeline::planner::candidate_metadata::build_request_trace_proxy_value;
use crate::ai_pipeline::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_pipeline::planner::payload_metadata::{
    build_local_execution_decision_response, LocalExecutionDecisionResponseParts,
};
use crate::ai_pipeline::planner::report_context::{
    build_local_execution_report_context, LocalExecutionReportContextParts,
};
use crate::ai_pipeline::planner::spec_metadata::local_standard_spec_metadata;
use crate::ai_pipeline::transport::{
    resolve_transport_execution_timeouts, resolve_transport_tls_profile,
};
use crate::ai_pipeline::{ConversionMode, ExecutionStrategy};
use crate::{
    append_execution_contract_fields_to_value, append_local_failover_policy_to_value, AppState,
    GatewayControlSyncDecisionResponse,
};

use super::request::resolve_local_standard_candidate_payload_parts;
use super::{LocalStandardCandidateAttempt, LocalStandardDecisionInput, LocalStandardSpec};

pub(super) async fn maybe_build_local_standard_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalStandardDecisionInput,
    attempt: LocalStandardCandidateAttempt,
    spec: LocalStandardSpec,
) -> Option<GatewayControlSyncDecisionResponse> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let LocalStandardCandidateAttempt {
        eligible,
        candidate_index,
        candidate_id,
        ..
    } = &attempt;
    let candidate = &eligible.candidate;
    let resolved = resolve_local_standard_candidate_payload_parts(
        state, parts, trace_id, body_json, input, &attempt, spec,
    )
    .await?;
    let proxy = state
        .resolve_transport_proxy_snapshot_with_tunnel_affinity(&resolved.transport)
        .await;
    let mut extra_fields = serde_json::Map::new();
    if let Some(proxy_value) =
        build_request_trace_proxy_value(Some(&resolved.transport), proxy.as_ref())
    {
        extra_fields.insert("proxy".to_string(), proxy_value);
    }
    if let Some(envelope_name) = resolved.envelope_name {
        extra_fields.insert(
            "envelope_name".to_string(),
            serde_json::Value::String(envelope_name.to_string()),
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
                provider_name: &candidate.provider_name,
                provider_id: &candidate.provider_id,
                endpoint_id: &candidate.endpoint_id,
                key_id: &candidate.key_id,
                key_name: Some(&candidate.key_name),
                model_id: Some(&candidate.model_id),
                global_model_id: Some(&candidate.global_model_id),
                global_model_name: Some(&candidate.global_model_name),
                provider_api_format: &resolved.provider_api_format,
                client_api_format: spec_metadata.api_format,
                mapped_model: Some(&resolved.mapped_model),
                candidate_group_id: eligible.orchestration.candidate_group_id.as_deref(),
                upstream_url: Some(&resolved.upstream_url),
                header_rules: resolved.transport.endpoint.header_rules.as_ref(),
                body_rules: resolved.transport.endpoint.body_rules.as_ref(),
                provider_request_method: Some(serde_json::Value::Null),
                provider_request_headers: Some(&resolved.provider_request_headers),
                original_headers: &parts.headers,
                original_request_body_json: Some(body_json),
                original_request_body_base64: None,
                client_requested_stream: body_json
                    .get("stream")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                upstream_is_stream: resolved.upstream_is_stream,
                has_envelope: resolved.envelope_name.is_some(),
                needs_conversion: true,
                extra_fields,
            }),
            ExecutionStrategy::LocalCrossFormat,
            ConversionMode::Bidirectional,
            spec_metadata.api_format,
            candidate.endpoint_api_format.as_str(),
        ),
        &resolved.transport,
    );
    let tls_profile = resolve_transport_tls_profile(&resolved.transport);
    let timeouts = resolve_transport_execution_timeouts(&resolved.transport);
    let super::request::LocalStandardCandidatePayloadParts {
        auth_header,
        auth_value,
        mapped_model,
        provider_api_format,
        provider_request_body,
        provider_request_headers,
        upstream_url,
        upstream_is_stream,
        envelope_name: _,
        transport,
    } = resolved;

    Some(build_local_execution_decision_response(
        LocalExecutionDecisionResponseParts {
            decision_is_stream: spec_metadata.require_streaming,
            decision_kind: spec_metadata.decision_kind.to_string(),
            execution_strategy: ExecutionStrategy::LocalCrossFormat,
            conversion_mode: ConversionMode::Bidirectional,
            request_id: trace_id.to_string(),
            candidate_id: candidate_id.to_string(),
            provider_name: candidate.provider_name.clone(),
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
            upstream_base_url: transport.endpoint.base_url.clone(),
            upstream_url,
            provider_request_method: None,
            auth_header: Some(auth_header),
            auth_value: Some(auth_value),
            provider_api_format,
            client_api_format: spec_metadata.api_format.to_string(),
            model_name: input.requested_model.clone(),
            mapped_model,
            prompt_cache_key: None,
            provider_request_headers,
            provider_request_body: Some(provider_request_body),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            proxy,
            tls_profile,
            timeouts,
            upstream_is_stream,
            report_kind: spec_metadata.report_kind.map(ToOwned::to_owned),
            report_context: Some(report_context),
            auth_context: input.auth_context.clone(),
        },
    ))
}

pub(super) async fn mark_skipped_local_standard_candidate(
    state: &AppState,
    input: &LocalStandardDecisionInput,
    trace_id: &str,
    candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::StandardDecision,
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
