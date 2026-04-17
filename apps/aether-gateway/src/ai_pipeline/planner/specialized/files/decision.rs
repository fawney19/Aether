use serde_json::json;

use crate::ai_pipeline::planner::candidate_metadata::build_request_trace_proxy_value;
use crate::ai_pipeline::planner::payload_metadata::{
    build_local_execution_decision_response, LocalExecutionDecisionResponseParts,
};
use crate::ai_pipeline::planner::report_context::{
    build_local_execution_report_context, LocalExecutionReportContextParts,
};
use crate::ai_pipeline::planner::spec_metadata::local_gemini_files_spec_metadata;
use crate::ai_pipeline::transport::{
    resolve_transport_execution_timeouts, resolve_transport_tls_profile,
};
use crate::ai_pipeline::{ConversionMode, ExecutionStrategy, PlannerAppState};
use crate::{AppState, GatewayControlSyncDecisionResponse};

use super::request::resolve_local_gemini_files_candidate_payload_parts;
use super::support::{
    LocalGeminiFilesCandidateAttempt, LocalGeminiFilesDecisionInput, GEMINI_FILES_CLIENT_API_FORMAT,
};
use super::LocalGeminiFilesSpec;

#[allow(clippy::too_many_arguments)]
pub(super) async fn maybe_build_local_gemini_files_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    input: &LocalGeminiFilesDecisionInput,
    attempt: LocalGeminiFilesCandidateAttempt,
    spec: LocalGeminiFilesSpec,
) -> Option<GatewayControlSyncDecisionResponse> {
    let spec_metadata = local_gemini_files_spec_metadata(spec);
    let planner_state = PlannerAppState::new(state);
    let resolved = resolve_local_gemini_files_candidate_payload_parts(
        state,
        parts,
        body_json,
        body_base64,
        body_is_empty,
        trace_id,
        input,
        &attempt,
        spec,
    )
    .await?;
    let LocalGeminiFilesCandidateAttempt {
        eligible,
        candidate_index,
        candidate_id,
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
    extra_fields.insert("file_key_id".to_string(), json!(candidate.key_id));
    extra_fields.insert("file_name".to_string(), json!(resolved.file_name));

    Some(build_local_execution_decision_response(
        LocalExecutionDecisionResponseParts {
            decision_is_stream: spec_metadata.require_streaming,
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
            provider_api_format: GEMINI_FILES_CLIENT_API_FORMAT.to_string(),
            client_api_format: GEMINI_FILES_CLIENT_API_FORMAT.to_string(),
            model_name: "gemini-files".to_string(),
            mapped_model: candidate.selected_provider_model_name.clone(),
            prompt_cache_key: None,
            provider_request_headers: resolved.provider_request_headers,
            provider_request_body: resolved.provider_request_body,
            provider_request_body_base64: resolved.provider_request_body_base64,
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
            upstream_is_stream: spec_metadata.require_streaming,
            report_kind: spec_metadata.report_kind.map(ToOwned::to_owned),
            report_context: Some(build_local_execution_report_context(
                LocalExecutionReportContextParts {
                    auth_context: &input.auth_context,
                    request_id: trace_id,
                    candidate_id: &candidate_id,
                    candidate_index,
                    retry_index: 0,
                    model: "gemini-files",
                    provider_name: &transport.provider.name,
                    provider_id: &candidate.provider_id,
                    endpoint_id: &candidate.endpoint_id,
                    key_id: &candidate.key_id,
                    key_name: None,
                    provider_api_format: GEMINI_FILES_CLIENT_API_FORMAT,
                    client_api_format: GEMINI_FILES_CLIENT_API_FORMAT,
                    mapped_model: None,
                    upstream_url: None,
                    provider_request_method: None,
                    provider_request_headers: None,
                    original_headers: &parts.headers,
                    original_request_body: &resolved.original_request_body,
                    has_envelope: false,
                    needs_conversion: false,
                    extra_fields,
                },
            )),
            auth_context: input.auth_context.clone(),
        },
    ))
}
