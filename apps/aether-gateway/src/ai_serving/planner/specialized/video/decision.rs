use crate::ai_serving::build_request_trace_proxy_value;
use crate::ai_serving::planner::report_context::{
    build_local_execution_report_context, LocalExecutionReportContextParts,
};
use crate::ai_serving::planner::spec_metadata::local_video_create_spec_metadata;
use crate::ai_serving::planner::{
    build_ai_execution_decision_response, AiExecutionDecisionResponseParts,
};
use crate::ai_serving::transport::{
    resolve_transport_execution_timeouts, resolve_transport_tls_profile,
};
use crate::ai_serving::{ai_local_execution_contract_for_formats, PlannerAppState};
use crate::{AiExecutionDecision, AppState};

use super::request::resolve_local_video_create_candidate_payload_parts;
use super::support::{LocalVideoCreateCandidateAttempt, LocalVideoCreateDecisionInput};
use super::LocalVideoCreateSpec;

pub(super) async fn maybe_build_local_video_create_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    input: &LocalVideoCreateDecisionInput,
    attempt: LocalVideoCreateCandidateAttempt,
    spec: LocalVideoCreateSpec,
) -> Option<AiExecutionDecision> {
    let spec_metadata = local_video_create_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let attempt_identity = attempt.attempt_identity();
    let resolved = resolve_local_video_create_candidate_payload_parts(
        state, parts, body_json, trace_id, input, &attempt, spec,
    )
    .await?;
    let LocalVideoCreateCandidateAttempt {
        eligible,
        candidate_id,
        ..
    } = attempt;
    let candidate = eligible.candidate;
    let transport = resolved.transport;
    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats(spec_metadata.api_format, spec_metadata.api_format);
    let proxy = planner_state
        .app()
        .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
        .await;
    let tls_profile = resolve_transport_tls_profile(&transport);
    let mut extra_fields = serde_json::Map::new();
    if let Some(proxy_value) = build_request_trace_proxy_value(Some(&transport), proxy.as_ref()) {
        extra_fields.insert("proxy".to_string(), proxy_value);
    }
    let report_context = build_local_execution_report_context(LocalExecutionReportContextParts {
        auth_context: &input.auth_context,
        request_id: trace_id,
        candidate_id: &candidate_id,
        attempt_identity,
        model: &input.requested_model,
        provider_name: &transport.provider.name,
        provider_id: &candidate.provider_id,
        endpoint_id: &candidate.endpoint_id,
        key_id: &candidate.key_id,
        key_name: None,
        model_id: Some(&candidate.model_id),
        global_model_id: Some(&candidate.global_model_id),
        global_model_name: Some(&candidate.global_model_name),
        provider_api_format: spec_metadata.api_format,
        client_api_format: spec_metadata.api_format,
        mapped_model: Some(&resolved.mapped_model),
        candidate_group_id: eligible.orchestration.candidate_group_id.as_deref(),
        ranking: eligible.ranking.as_ref(),
        upstream_url: None,
        header_rules: transport.endpoint.header_rules.as_ref(),
        body_rules: transport.endpoint.body_rules.as_ref(),
        provider_request_method: None,
        provider_request_headers: None,
        original_headers: &parts.headers,
        request_origin: Some(crate::ai_serving::request_origin_from_parts(parts)),
        original_request_body_json: Some(body_json),
        original_request_body_base64: None,
        client_requested_stream: false,
        upstream_is_stream: false,
        has_envelope: false,
        needs_conversion: false,
        extra_fields,
    });
    let super::request::LocalVideoCreateCandidatePayloadParts {
        transport: _,
        auth_header,
        auth_value,
        mapped_model,
        provider_request_headers,
        provider_request_body,
        upstream_url,
    } = resolved;

    Some(build_ai_execution_decision_response(
        AiExecutionDecisionResponseParts {
            decision_is_stream: false,
            decision_kind: spec_metadata.decision_kind.to_string(),
            execution_strategy,
            conversion_mode,
            request_id: trace_id.to_string(),
            candidate_id: candidate_id.clone(),
            provider_name: transport.provider.name.clone(),
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
            upstream_base_url: transport.endpoint.base_url.clone(),
            upstream_url,
            provider_request_method: Some(parts.method.to_string()),
            auth_header: Some(auth_header),
            auth_value: Some(auth_value),
            provider_api_format: spec_metadata.api_format.to_string(),
            client_api_format: spec_metadata.api_format.to_string(),
            model_name: input.requested_model.clone(),
            mapped_model,
            prompt_cache_key: None,
            provider_request_headers,
            provider_request_body: Some(provider_request_body),
            provider_request_body_base64: None,
            content_type: parts
                .headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            proxy,
            tls_profile,
            timeouts: resolve_transport_execution_timeouts(&transport),
            upstream_is_stream: false,
            report_kind: spec_metadata.report_kind.map(ToOwned::to_owned),
            report_context: Some(report_context),
            auth_context: input.auth_context.clone(),
        },
    ))
}
