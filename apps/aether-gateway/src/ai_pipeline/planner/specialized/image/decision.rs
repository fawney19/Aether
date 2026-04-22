use crate::ai_pipeline::planner::candidate_metadata::build_request_trace_proxy_value;
use crate::ai_pipeline::planner::payload_metadata::{
    build_local_execution_decision_response, LocalExecutionDecisionResponseParts,
};
use crate::ai_pipeline::planner::report_context::{
    build_local_execution_report_context, LocalExecutionReportContextParts,
};
use crate::ai_pipeline::planner::spec_metadata::local_openai_image_spec_metadata;
use crate::ai_pipeline::transport::{
    resolve_transport_execution_timeouts, resolve_transport_tls_profile,
};
use crate::ai_pipeline::{ConversionMode, ExecutionStrategy, PlannerAppState};
use crate::{AppState, GatewayControlSyncDecisionResponse};

use super::request::resolve_local_openai_image_candidate_payload_parts;
use super::support::{LocalOpenAiImageCandidateAttempt, LocalOpenAiImageDecisionInput};
use super::LocalOpenAiImageSpec;

pub(super) async fn maybe_build_local_openai_image_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    attempt: LocalOpenAiImageCandidateAttempt,
    spec: LocalOpenAiImageSpec,
) -> Option<GatewayControlSyncDecisionResponse> {
    let spec_metadata = local_openai_image_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let attempt_identity = attempt.attempt_identity();
    let resolved = resolve_local_openai_image_candidate_payload_parts(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        input,
        &attempt,
        spec,
    )
    .await?;
    let LocalOpenAiImageCandidateAttempt {
        eligible,
        candidate_id,
        ..
    } = attempt;
    let candidate = eligible.candidate;
    let transport = resolved.transport;
    let proxy = planner_state
        .app()
        .resolve_transport_proxy_snapshot_with_tunnel_affinity(&transport)
        .await;
    let tls_profile = resolve_transport_tls_profile(&transport);
    let mut extra_fields = serde_json::Map::new();
    if let Some(proxy_value) = build_request_trace_proxy_value(Some(&transport), proxy.as_ref()) {
        extra_fields.insert("proxy".to_string(), proxy_value);
    }
    extra_fields.insert("image_request".to_string(), resolved.input_summary.clone());
    let report_context = build_local_execution_report_context(LocalExecutionReportContextParts {
        auth_context: &input.auth_context,
        request_id: trace_id,
        candidate_id: &candidate_id,
        attempt_identity,
        model: &resolved.requested_model,
        provider_name: &transport.provider.name,
        provider_id: &candidate.provider_id,
        endpoint_id: &candidate.endpoint_id,
        key_id: &candidate.key_id,
        key_name: None,
        provider_api_format: spec_metadata.api_format,
        client_api_format: spec_metadata.api_format,
        mapped_model: Some(&resolved.mapped_model),
        candidate_group_id: eligible.orchestration.candidate_group_id.as_deref(),
        upstream_url: Some(&resolved.upstream_url),
        provider_request_method: Some(serde_json::Value::String(parts.method.to_string())),
        provider_request_headers: Some(&resolved.provider_request_headers),
        original_headers: &parts.headers,
        original_request_body_json: Some(body_json),
        original_request_body_base64: body_base64,
        has_envelope: false,
        needs_conversion: false,
        extra_fields,
    });

    Some(build_local_execution_decision_response(
        LocalExecutionDecisionResponseParts {
            decision_is_stream: false,
            decision_kind: spec_metadata.decision_kind.to_string(),
            execution_strategy: ExecutionStrategy::LocalSameFormat,
            conversion_mode: ConversionMode::None,
            request_id: trace_id.to_string(),
            candidate_id: candidate_id.clone(),
            provider_name: transport.provider.name.clone(),
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
            upstream_base_url: transport.endpoint.base_url.clone(),
            upstream_url: resolved.upstream_url,
            provider_request_method: Some(parts.method.to_string()),
            auth_header: Some(resolved.auth_header),
            auth_value: Some(resolved.auth_value),
            provider_api_format: spec_metadata.api_format.to_string(),
            client_api_format: spec_metadata.api_format.to_string(),
            model_name: resolved.requested_model,
            mapped_model: resolved.mapped_model,
            prompt_cache_key: None,
            provider_request_headers: resolved.provider_request_headers,
            provider_request_body: Some(resolved.provider_request_body),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
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
