use serde_json::json;

use aether_ai_serving::{AdaptationMode, AiRequestGzipPolicy, OriginalRequestPayload};
use aether_contracts::{ExecutionResponseBodyMode, EXECUTION_RESPONSE_BODY_MODE_HEADER};

use crate::ai_serving::ai_local_execution_contract_for_formats;
use crate::ai_serving::build_request_trace_proxy_value;
use crate::ai_serving::planner::candidate_materialization::{
    mark_skipped_local_execution_candidate, mark_skipped_local_execution_candidate_with_extra_data,
    mark_skipped_local_execution_candidate_with_failure_diagnostic,
};
use crate::ai_serving::planner::decision_input::apply_provider_request_routing_policy_to_decision;
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::planner::report_context::{
    build_local_execution_report_context, insert_native_client_envelope_name,
    LocalExecutionReportContextParts,
};
use crate::ai_serving::planner::spec_metadata::local_same_format_provider_spec_metadata;
use crate::ai_serving::planner::CandidateFailureDiagnostic;
use crate::ai_serving::planner::{
    build_ai_execution_decision_response, resolve_transport_request_encoding_policy,
    AiExecutionDecisionResponseParts,
};
use crate::ai_serving::transport::{
    resolve_transport_execution_timeouts, resolve_transport_profile,
};
use crate::{
    append_execution_contract_fields_to_value, append_local_failover_policy_to_value,
    AiExecutionDecision, AppState, GatewayError,
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
) -> Result<Option<AiExecutionDecision>, GatewayError> {
    let spec_metadata = local_same_format_provider_spec_metadata(spec);
    let LocalSameFormatProviderCandidateAttempt {
        eligible,
        candidate_index,
        candidate_id,
        ..
    } = &attempt;
    let candidate = &eligible.candidate;
    let Some(resolved) = resolve_local_same_format_provider_candidate_payload_parts(
        state, parts, trace_id, body_json, input, &attempt, spec,
    )
    .await?
    else {
        return Ok(None);
    };
    let request_redacted = resolved.request_redacted;
    let compatibility_edits_empty = resolved.compatibility_edits.is_empty();
    let original_request_body_json = if resolved.request_redacted {
        Some(&resolved.provider_request_body)
    } else {
        Some(body_json)
    };

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
    let transport_profile = resolved
        .transport_profile
        .clone()
        .or_else(|| resolve_transport_profile(&resolved.transport));
    let mut extra_fields = serde_json::Map::new();
    extra_fields.insert(
        "provider_type".to_string(),
        json!(resolved.transport.provider.provider_type.as_str()),
    );
    if let Some(operation) = spec.operation {
        extra_fields.insert("api_operation".to_string(), json!(operation.as_str()));
    }
    if let Some(client_surface) = input.client_surface {
        extra_fields.insert("client_surface".to_string(), json!(client_surface.as_str()));
    }
    if let Some(carrier) = input.gateway_credential_carrier {
        extra_fields.insert(
            "gateway_credential_carrier".to_string(),
            json!(carrier.as_str()),
        );
    }
    extra_fields.insert(
        "upstream_credential_mode".to_string(),
        json!(resolved.transport.key.auth_type.trim().to_ascii_lowercase()),
    );
    let mut adaptation_mode = if resolved.compatibility_edits.is_empty() {
        AdaptationMode::NativeTransparent
    } else {
        AdaptationMode::SameFormatCompat
    };
    if crate::ai_serving::normalize_api_format_alias(&resolved.provider_api_format)
        == "claude:messages"
    {
        let compatibility_profile =
            crate::ai_serving::transport::resolve_anthropic_compatibility_profile(
                &resolved.transport,
                &resolved.provider_api_format,
            );
        extra_fields.insert(
            "anthropic_compatibility_profile".to_string(),
            json!(compatibility_profile.as_str()),
        );
        if compatibility_profile.uses_claude_code_compatibility() {
            adaptation_mode = AdaptationMode::SameFormatCompat;
        }
    }
    extra_fields.insert(
        "adaptation_mode".to_string(),
        json!(adaptation_mode.as_str()),
    );
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
        insert_native_client_envelope_name(
            &mut extra_fields,
            super::super::ANTIGRAVITY_ENVELOPE_NAME,
            parts.uri.path(),
        );
    } else if resolved.is_gemini_cli {
        extra_fields.insert(
            "envelope_name".to_string(),
            json!(crate::ai_serving::transport::GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME),
        );
    }
    if !resolved.compatibility_edits.is_empty() {
        if let Ok(value) = serde_json::to_value(&resolved.compatibility_edits) {
            extra_fields.insert("request_body_compatibility_edits".to_string(), value);
        }
    }
    let provider_api_format = resolved.provider_api_format.clone();
    let (execution_strategy, conversion_mode) = ai_local_execution_contract_for_formats(
        spec_metadata.api_format,
        provider_api_format.as_str(),
    );
    let effective_headers = input.effective_headers(&parts.headers);
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
                provider_api_format: provider_api_format.as_str(),
                client_api_format: spec_metadata.api_format,
                mapped_model: Some(&resolved.mapped_model),
                candidate_group_id: eligible.orchestration.candidate_group_id.as_deref(),
                pool_key_lease: eligible.orchestration.pool_key_lease.as_ref(),
                ranking: eligible.ranking.as_ref(),
                upstream_url: Some(&resolved.upstream_url),
                header_rules: resolved.transport.endpoint.header_rules.as_ref(),
                body_rules: resolved.transport.endpoint.body_rules.as_ref(),
                provider_request_method: Some(serde_json::Value::Null),
                provider_request_headers: Some(&resolved.provider_request_headers),
                original_headers: effective_headers,
                request_path: Some(parts.uri.path()),
                request_query_string: parts.uri.query(),
                request_origin: Some(crate::ai_serving::request_origin_from_parts(parts)),
                original_request_body_json,
                original_request_body_base64: None,
                client_session_affinity: input.client_session_affinity.as_ref(),
                routing_policy: input.routing_policy.as_ref(),
                scheduler_affinity_epoch: eligible.orchestration.scheduler_affinity_epoch,
                client_requested_stream: body_json
                    .get("stream")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                upstream_is_stream: resolved.upstream_is_stream,
                has_envelope: resolved.is_kiro || resolved.is_antigravity || resolved.is_gemini_cli,
                needs_conversion: matches!(
                    conversion_mode,
                    crate::ai_serving::ConversionMode::Bidirectional
                ),
                extra_fields,
            }),
            execution_strategy,
            conversion_mode,
            spec_metadata.api_format,
            provider_api_format.as_str(),
        ),
        &resolved.transport,
    );
    let super::request::LocalSameFormatProviderCandidatePayloadParts {
        transport,
        is_antigravity: _,
        is_gemini_cli: _,
        is_kiro: _,
        auth_header,
        auth_value,
        provider_api_format,
        mapped_model,
        report_kind,
        upstream_is_stream,
        upstream_url,
        provider_request_headers,
        provider_request_body,
        transport_profile: _,
        compatibility_edits: _,
        request_redacted: _,
    } = resolved;
    let request_encoding = resolve_transport_request_encoding_policy(&transport);
    let content_type = if transport
        .provider
        .provider_type
        .eq_ignore_ascii_case("kiro")
    {
        provider_request_headers.get("content-type").cloned()
    } else {
        Some("application/json".to_string())
    };

    let mut decision = build_ai_execution_decision_response(AiExecutionDecisionResponseParts {
        decision_is_stream: spec_metadata.require_streaming,
        decision_kind: spec_metadata.decision_kind.to_string(),
        execution_strategy,
        conversion_mode,
        request_id: trace_id.to_string(),
        candidate_id: candidate_id.to_string(),
        provider_name: transport.provider.name.clone(),
        provider_type: transport.provider.provider_type.clone(),
        provider_id: candidate.provider_id.clone(),
        endpoint_id: candidate.endpoint_id.clone(),
        key_id: candidate.key_id.clone(),
        upstream_base_url: transport.endpoint.base_url.clone(),
        upstream_url,
        provider_request_method: None,
        auth_header,
        auth_value,
        provider_api_format,
        client_api_format: spec_metadata.api_format.to_string(),
        model_name: input.requested_model.clone(),
        mapped_model,
        prompt_cache_key,
        provider_request_headers,
        provider_request_body: Some(provider_request_body),
        provider_request_body_base64: None,
        content_type,
        content_encoding: request_encoding.content_encoding,
        request_gzip: request_encoding.request_gzip,
        proxy,
        transport_profile,
        timeouts: resolve_transport_execution_timeouts(&transport),
        upstream_is_stream,
        report_kind: Some(report_kind.to_string()),
        report_context: Some(report_context),
        auth_context: input.auth_context.clone(),
    });
    apply_provider_request_routing_policy_to_decision(
        input,
        &mut decision,
        Some(transport.as_ref()),
    )?;
    enforce_provider_api_operation_invariants(
        spec.operation,
        decision.provider_request_body.as_mut(),
        &mut decision.provider_request_headers,
    );
    decision.provider_request_body_base64 = original_request_body_base64(
        parts,
        decision.provider_request_body.as_ref(),
        adaptation_mode,
        request_redacted,
        compatibility_edits_empty,
        decision.content_encoding.as_deref(),
        decision.request_gzip.as_ref(),
    );
    decision
        .provider_request_headers
        .retain(|name, _| !name.eq_ignore_ascii_case(EXECUTION_RESPONSE_BODY_MODE_HEADER));
    if !spec_metadata.require_streaming && decision.provider_request_body_base64.is_some() {
        decision.provider_request_headers.insert(
            EXECUTION_RESPONSE_BODY_MODE_HEADER.to_string(),
            ExecutionResponseBodyMode::PreserveBytes
                .as_str()
                .to_string(),
        );
    }
    Ok(Some(decision))
}

fn enforce_provider_api_operation_invariants(
    operation: Option<crate::ai_serving::ApiOperation>,
    provider_request_body: Option<&mut serde_json::Value>,
    provider_request_headers: &mut std::collections::BTreeMap<String, String>,
) {
    if operation != Some(crate::ai_serving::ApiOperation::ClaudeCountTokens) {
        return;
    }

    if let Some(provider_request_body) = provider_request_body {
        crate::ai_serving::transport::enforce_same_format_provider_api_operation_body_policy(
            provider_request_body,
            operation,
        );
    }
    for header_name in ["accept", "content-type"] {
        provider_request_headers.retain(|name, _| !name.eq_ignore_ascii_case(header_name));
        provider_request_headers.insert(header_name.to_string(), "application/json".to_string());
    }
}

fn original_request_body_base64(
    parts: &http::request::Parts,
    provider_request_body: Option<&serde_json::Value>,
    adaptation_mode: AdaptationMode,
    request_redacted: bool,
    compatibility_edits_empty: bool,
    content_encoding: Option<&str>,
    request_gzip: Option<&AiRequestGzipPolicy>,
) -> Option<String> {
    if adaptation_mode != AdaptationMode::NativeTransparent
        || request_redacted
        || !compatibility_edits_empty
        || content_encoding.is_some_and(|value| !value.trim().is_empty())
        || request_gzip.is_some_and(|policy| policy.enabled != Some(false))
    {
        return None;
    }

    parts
        .extensions
        .get::<OriginalRequestPayload>()?
        .body_bytes_base64_if_unchanged(provider_request_body?)
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use base64::Engine as _;

    use super::{
        enforce_provider_api_operation_invariants, original_request_body_base64, AdaptationMode,
        AiRequestGzipPolicy, OriginalRequestPayload,
    };
    use crate::ai_serving::ApiOperation;

    fn request_parts_with_original_payload(
        body_json: serde_json::Value,
        body_bytes: &[u8],
    ) -> http::request::Parts {
        let (mut parts, ()) = http::Request::new(()).into_parts();
        parts
            .extensions
            .insert(OriginalRequestPayload::from_parsed_json(
                body_json, body_bytes,
            ));
        parts
    }

    #[test]
    fn count_tokens_invariants_win_after_provider_routing_mutations() {
        let mut body = serde_json::json!({
            "model": "claude-sonnet-4",
            "messages": [],
            "stream": true
        });
        let mut headers = BTreeMap::from([
            ("Accept".to_string(), "text/event-stream".to_string()),
            ("Content-Type".to_string(), "text/plain".to_string()),
            ("x-provider-route".to_string(), "kept".to_string()),
        ]);

        enforce_provider_api_operation_invariants(
            Some(ApiOperation::ClaudeCountTokens),
            Some(&mut body),
            &mut headers,
        );

        assert!(body.get("stream").is_none());
        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            headers.get("x-provider-route").map(String::as_str),
            Some("kept")
        );
        assert_eq!(
            headers
                .keys()
                .filter(|name| name.eq_ignore_ascii_case("accept"))
                .count(),
            1
        );
        assert_eq!(
            headers
                .keys()
                .filter(|name| name.eq_ignore_ascii_case("content-type"))
                .count(),
            1
        );
    }

    #[test]
    fn unchanged_same_format_body_preserves_original_json_bytes() {
        let raw = br#"{ "unknown": {"enabled":true}, "messages": [], "model": "claude-sonnet-4" }"#;
        let body_json: serde_json::Value = serde_json::from_slice(raw).expect("body should parse");
        let parts = request_parts_with_original_payload(body_json.clone(), raw);

        let encoded = original_request_body_base64(
            &parts,
            Some(&body_json),
            AdaptationMode::NativeTransparent,
            false,
            true,
            None,
            None,
        )
        .expect("unchanged request should retain exact bytes");

        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .expect("body should decode"),
            raw
        );
    }

    #[test]
    fn request_edits_or_encoding_disable_original_json_bytes() {
        let raw = br#"{"model":"claude-sonnet-4","messages":[]}"#;
        let body_json: serde_json::Value = serde_json::from_slice(raw).expect("body should parse");
        let parts = request_parts_with_original_payload(body_json.clone(), raw);
        let changed_body = serde_json::json!({
            "model": "claude-sonnet-4-5",
            "messages": []
        });

        assert!(original_request_body_base64(
            &parts,
            Some(&changed_body),
            AdaptationMode::NativeTransparent,
            false,
            true,
            None,
            None,
        )
        .is_none());
        assert!(original_request_body_base64(
            &parts,
            Some(&body_json),
            AdaptationMode::NativeTransparent,
            true,
            true,
            None,
            None,
        )
        .is_none());
        assert!(original_request_body_base64(
            &parts,
            Some(&body_json),
            AdaptationMode::NativeTransparent,
            false,
            false,
            None,
            None,
        )
        .is_none());
        assert!(original_request_body_base64(
            &parts,
            Some(&body_json),
            AdaptationMode::SameFormatCompat,
            false,
            true,
            None,
            None,
        )
        .is_none());
        assert!(original_request_body_base64(
            &parts,
            Some(&body_json),
            AdaptationMode::NativeTransparent,
            false,
            true,
            Some("gzip"),
            None,
        )
        .is_none());
        assert!(original_request_body_base64(
            &parts,
            Some(&body_json),
            AdaptationMode::NativeTransparent,
            false,
            true,
            None,
            Some(&AiRequestGzipPolicy {
                enabled: Some(true),
                min_bytes: Some(1),
            }),
        )
        .is_none());
    }
}
