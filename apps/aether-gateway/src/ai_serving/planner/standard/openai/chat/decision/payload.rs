use crate::ai_serving::build_request_trace_proxy_value;
use crate::ai_serving::planner::report_context::{
    build_local_execution_report_context, insert_provider_stream_event_api_format,
    LocalExecutionReportContextParts,
};
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

use super::request::resolve_local_openai_chat_candidate_payload_parts;
use super::support::{LocalOpenAiChatCandidateAttempt, LocalOpenAiChatDecisionInput};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_build_local_openai_chat_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    attempt: LocalOpenAiChatCandidateAttempt,
    decision_kind: &str,
    report_kind: &str,
    upstream_is_stream: bool,
) -> Option<AiExecutionDecision> {
    let attempt_identity = attempt.attempt_identity();
    let LocalOpenAiChatCandidateAttempt {
        eligible,
        candidate_index,
        candidate_id,
        ..
    } = attempt;
    let resolved = resolve_local_openai_chat_candidate_payload_parts(
        state,
        parts,
        trace_id,
        body_json,
        input,
        &eligible,
        candidate_index,
        &candidate_id,
        decision_kind,
        report_kind,
        upstream_is_stream,
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
    if let Some(envelope_name) = resolved.envelope_name {
        extra_fields.insert(
            "envelope_name".to_string(),
            serde_json::Value::String(envelope_name.to_string()),
        );
    }
    insert_provider_stream_event_api_format(
        &mut extra_fields,
        resolved.transport.provider.provider_type.as_str(),
    );
    let report_context = append_local_failover_policy_to_value(
        append_execution_contract_fields_to_value(
            build_local_execution_report_context(LocalExecutionReportContextParts {
                auth_context: &input.auth_context,
                request_id: trace_id,
                candidate_id: &candidate_id,
                attempt_identity,
                model: &input.requested_model,
                provider_name: &resolved.transport.provider.name,
                provider_id: &candidate.provider_id,
                endpoint_id: &candidate.endpoint_id,
                key_id: &candidate.key_id,
                key_name: Some(&candidate.key_name),
                model_id: Some(&candidate.model_id),
                global_model_id: Some(&candidate.global_model_id),
                global_model_name: Some(&candidate.global_model_name),
                provider_api_format: &resolved.provider_api_format,
                client_api_format: "openai:chat",
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
                upstream_is_stream,
                has_envelope: resolved.envelope_name.is_some(),
                needs_conversion: matches!(
                    resolved.conversion_mode,
                    crate::ai_serving::ConversionMode::Bidirectional
                ),
                extra_fields,
            }),
            resolved.execution_strategy,
            resolved.conversion_mode,
            "openai:chat",
            candidate.endpoint_api_format.as_str(),
        ),
        &resolved.transport,
    );
    let super::request::LocalOpenAiChatCandidatePayloadParts {
        auth_header,
        auth_value,
        mapped_model,
        provider_api_format,
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        report_kind,
        envelope_name: _,
        transport,
    } = resolved;

    Some(build_ai_execution_decision_response(
        AiExecutionDecisionResponseParts {
            decision_is_stream: upstream_is_stream,
            decision_kind: decision_kind.to_string(),
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
            provider_request_method: None,
            auth_header: Some(auth_header),
            auth_value: Some(auth_value),
            provider_api_format,
            client_api_format: "openai:chat".to_string(),
            model_name: input.requested_model.clone(),
            mapped_model,
            prompt_cache_key,
            provider_request_headers,
            provider_request_body: Some(provider_request_body),
            provider_request_body_base64: None,
            content_type: Some("application/json".to_string()),
            proxy,
            tls_profile,
            timeouts,
            upstream_is_stream,
            report_kind: Some(report_kind),
            report_context: Some(report_context),
            auth_context: input.auth_context.clone(),
        },
    ))
}
