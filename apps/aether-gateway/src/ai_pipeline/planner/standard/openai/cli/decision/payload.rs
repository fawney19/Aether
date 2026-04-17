use serde_json::json;
use tracing::debug;

use crate::ai_pipeline::planner::candidate_metadata::build_request_trace_proxy_value;
use crate::ai_pipeline::planner::payload_metadata::{
    build_local_execution_decision_response, LocalExecutionDecisionResponseParts,
};
use crate::ai_pipeline::planner::report_context::{
    build_local_execution_report_context, LocalExecutionReportContextParts,
};
use crate::ai_pipeline::planner::spec_metadata::local_openai_cli_spec_metadata;
use crate::ai_pipeline::transport::{
    resolve_transport_execution_timeouts, resolve_transport_tls_profile,
};
use crate::{
    append_execution_contract_fields_to_value, append_local_failover_policy_to_value, AppState,
    GatewayControlSyncDecisionResponse,
};

use super::request::resolve_local_openai_cli_candidate_payload_parts;
use super::support::{LocalOpenAiCliCandidateAttempt, LocalOpenAiCliDecisionInput};
use super::LocalOpenAiCliSpec;

pub(crate) async fn maybe_build_local_openai_cli_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiCliDecisionInput,
    attempt: LocalOpenAiCliCandidateAttempt,
    spec: LocalOpenAiCliSpec,
) -> Option<GatewayControlSyncDecisionResponse> {
    let spec_metadata = local_openai_cli_spec_metadata(spec);
    let LocalOpenAiCliCandidateAttempt {
        eligible,
        candidate_index,
        candidate_id,
    } = attempt;
    let resolved = resolve_local_openai_cli_candidate_payload_parts(
        state,
        parts,
        trace_id,
        body_json,
        input,
        &eligible,
        candidate_index,
        &candidate_id,
        spec,
    )
    .await?;
    let candidate = &eligible.candidate;

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
    let timeouts = resolve_transport_execution_timeouts(&resolved.transport);
    let mut extra_fields = serde_json::Map::new();
    if let Some(proxy_value) =
        build_request_trace_proxy_value(Some(&resolved.transport), proxy.as_ref())
    {
        extra_fields.insert("proxy".to_string(), proxy_value);
    }
    if resolved.is_antigravity {
        extra_fields.insert("envelope_name".to_string(), json!("antigravity:v1internal"));
    }

    debug!(
        event_name = "local_openai_cli_decision_payload_built",
        log_type = "debug",
        trace_id = %trace_id,
        candidate_id = %candidate_id,
        candidate_index,
        provider_name = %resolved.transport.provider.name,
        provider_id = %candidate.provider_id,
        endpoint_id = %candidate.endpoint_id,
        key_id = %candidate.key_id,
        decision_kind = spec_metadata.decision_kind,
        execution_strategy = resolved.execution_strategy.as_str(),
        conversion_mode = resolved.conversion_mode.as_str(),
        client_api_format = spec_metadata.api_format,
        provider_api_format = %resolved.provider_api_format,
        request_path = %parts.uri.path(),
        request_query = ?parts.uri.query(),
        upstream_base_url = %resolved.transport.endpoint.base_url,
        upstream_url = %resolved.upstream_url,
        upstream_is_stream = resolved.upstream_is_stream,
        has_envelope = resolved.is_antigravity,
        "gateway built local openai cli decision payload"
    );

    Some(build_local_execution_decision_response(
        LocalExecutionDecisionResponseParts {
            decision_is_stream: spec_metadata.require_streaming,
            decision_kind: spec_metadata.decision_kind.to_string(),
            execution_strategy: resolved.execution_strategy,
            conversion_mode: resolved.conversion_mode,
            request_id: trace_id.to_string(),
            candidate_id: candidate_id.clone(),
            provider_name: resolved.transport.provider.name.clone(),
            provider_id: candidate.provider_id.clone(),
            endpoint_id: candidate.endpoint_id.clone(),
            key_id: candidate.key_id.clone(),
            upstream_base_url: resolved.transport.endpoint.base_url.clone(),
            upstream_url: resolved.upstream_url.clone(),
            provider_request_method: None,
            auth_header: Some(resolved.auth_header.clone()),
            auth_value: Some(resolved.auth_value.clone()),
            provider_api_format: resolved.provider_api_format.clone(),
            client_api_format: spec_metadata.api_format.to_string(),
            model_name: input.requested_model.clone(),
            mapped_model: resolved.mapped_model.clone(),
            prompt_cache_key,
            provider_request_headers: resolved.provider_request_headers.clone(),
            provider_request_body: Some(resolved.provider_request_body.clone()),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            proxy,
            tls_profile,
            timeouts,
            upstream_is_stream: resolved.upstream_is_stream,
            report_kind: spec_metadata.report_kind.map(ToOwned::to_owned),
            report_context: Some(append_local_failover_policy_to_value(
                append_execution_contract_fields_to_value(
                    build_local_execution_report_context(LocalExecutionReportContextParts {
                        auth_context: &input.auth_context,
                        request_id: trace_id,
                        candidate_id: &candidate_id,
                        candidate_index,
                        retry_index: 0,
                        model: &input.requested_model,
                        provider_name: &resolved.transport.provider.name,
                        provider_id: &candidate.provider_id,
                        endpoint_id: &candidate.endpoint_id,
                        key_id: &candidate.key_id,
                        key_name: Some(&candidate.key_name),
                        provider_api_format: &resolved.provider_api_format,
                        client_api_format: spec_metadata.api_format,
                        mapped_model: Some(&resolved.mapped_model),
                        upstream_url: Some(&resolved.upstream_url),
                        provider_request_method: Some(serde_json::Value::Null),
                        provider_request_headers: Some(&resolved.provider_request_headers),
                        original_headers: &parts.headers,
                        original_request_body: body_json,
                        has_envelope: resolved.is_antigravity,
                        needs_conversion: matches!(
                            resolved.conversion_mode,
                            crate::ai_pipeline::ConversionMode::Bidirectional
                        ),
                        extra_fields,
                    }),
                    resolved.execution_strategy,
                    resolved.conversion_mode,
                    spec_metadata.api_format,
                    candidate.endpoint_api_format.as_str(),
                ),
                &resolved.transport,
            )),
            auth_context: input.auth_context.clone(),
        },
    ))
}
