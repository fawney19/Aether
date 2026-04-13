use std::collections::BTreeMap;

use serde_json::json;
use tracing::debug;

use crate::ai_pipeline::collect_control_headers;
use crate::ai_pipeline::transport::{
    resolve_transport_execution_timeouts, resolve_transport_proxy_snapshot_with_tunnel_affinity,
    resolve_transport_tls_profile,
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
    let LocalOpenAiCliCandidateAttempt {
        candidate,
        candidate_index,
        candidate_id,
    } = attempt;
    let resolved = resolve_local_openai_cli_candidate_payload_parts(
        state,
        parts,
        trace_id,
        body_json,
        input,
        &candidate,
        candidate_index,
        &candidate_id,
        spec,
    )
    .await?;

    let prompt_cache_key = resolved
        .provider_request_body
        .get("prompt_cache_key")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let proxy =
        resolve_transport_proxy_snapshot_with_tunnel_affinity(state, &resolved.transport).await;
    let tls_profile = resolve_transport_tls_profile(&resolved.transport);
    let timeouts = resolve_transport_execution_timeouts(&resolved.transport);

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
        decision_kind = spec.decision_kind,
        execution_strategy = resolved.execution_strategy.as_str(),
        conversion_mode = resolved.conversion_mode.as_str(),
        client_api_format = spec.api_format,
        provider_api_format = %resolved.provider_api_format,
        request_path = %parts.uri.path(),
        request_query = ?parts.uri.query(),
        upstream_base_url = %resolved.transport.endpoint.base_url,
        upstream_url = %resolved.upstream_url,
        upstream_is_stream = resolved.upstream_is_stream,
        has_envelope = resolved.is_antigravity,
        "gateway built local openai cli decision payload"
    );

    Some(GatewayControlSyncDecisionResponse {
        action: if spec.require_streaming {
            crate::ai_pipeline::planner::common::EXECUTION_RUNTIME_STREAM_DECISION_ACTION
                .to_string()
        } else {
            crate::ai_pipeline::planner::common::EXECUTION_RUNTIME_SYNC_DECISION_ACTION.to_string()
        },
        decision_kind: Some(spec.decision_kind.to_string()),
        execution_strategy: Some(resolved.execution_strategy.as_str().to_string()),
        conversion_mode: Some(resolved.conversion_mode.as_str().to_string()),
        request_id: Some(trace_id.to_string()),
        candidate_id: Some(candidate_id.clone()),
        provider_name: Some(resolved.transport.provider.name.clone()),
        provider_id: Some(candidate.provider_id.clone()),
        endpoint_id: Some(candidate.endpoint_id.clone()),
        key_id: Some(candidate.key_id.clone()),
        upstream_base_url: Some(resolved.transport.endpoint.base_url.clone()),
        upstream_url: Some(resolved.upstream_url.clone()),
        provider_request_method: None,
        auth_header: Some(resolved.auth_header.clone()),
        auth_value: Some(resolved.auth_value.clone()),
        provider_api_format: Some(resolved.provider_api_format.clone()),
        client_api_format: Some(spec.api_format.to_string()),
        provider_contract: Some(resolved.provider_api_format.clone()),
        client_contract: Some(spec.api_format.to_string()),
        model_name: Some(input.requested_model.clone()),
        mapped_model: Some(resolved.mapped_model.clone()),
        prompt_cache_key,
        extra_headers: BTreeMap::new(),
        provider_request_headers: resolved.provider_request_headers.clone(),
        provider_request_body: Some(resolved.provider_request_body.clone()),
        provider_request_body_base64: None,
        content_type: Some("application/json".to_string()),
        proxy,
        tls_profile,
        timeouts,
        upstream_is_stream: resolved.upstream_is_stream,
        report_kind: Some(spec.report_kind.to_string()),
        report_context: Some(append_local_failover_policy_to_value(
            append_execution_contract_fields_to_value(
                json!({
                    "user_id": input.auth_context.user_id,
                    "api_key_id": input.auth_context.api_key_id,
                    "username": input.auth_context.username,
                    "api_key_name": input.auth_context.api_key_name,
                    "request_id": trace_id,
                    "candidate_id": candidate_id,
                    "candidate_index": candidate_index,
                    "retry_index": 0,
                    "model": input.requested_model,
                    "provider_name": resolved.transport.provider.name,
                    "provider_id": candidate.provider_id,
                    "endpoint_id": candidate.endpoint_id,
                    "key_id": candidate.key_id,
                    "key_name": candidate.key_name,
                    "provider_api_format": resolved.provider_api_format,
                    "client_api_format": spec.api_format,
                    "mapped_model": resolved.mapped_model,
                    "upstream_url": resolved.upstream_url,
                    "provider_request_method": serde_json::Value::Null,
                    "provider_request_headers": resolved.provider_request_headers,
                    "original_headers": collect_control_headers(&parts.headers),
                    "original_request_body": crate::ai_pipeline::build_report_context_original_request_echo(body_json),
                    "has_envelope": resolved.is_antigravity,
                    "envelope_name": if resolved.is_antigravity {
                        Some("antigravity:v1internal")
                    } else {
                        None
                    },
                    "needs_conversion": matches!(resolved.conversion_mode, crate::ai_pipeline::ConversionMode::Bidirectional),
                }),
                resolved.execution_strategy,
                resolved.conversion_mode,
                spec.api_format,
                candidate.endpoint_api_format.as_str(),
            ),
            &resolved.transport,
        )),
        auth_context: Some(input.auth_context.clone()),
    })
}
