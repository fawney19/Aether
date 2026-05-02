use crate::ai_serving::build_request_trace_proxy_value;
use crate::ai_serving::planner::candidate_materialization::{
    mark_skipped_local_execution_candidate, mark_skipped_local_execution_candidate_with_extra_data,
    mark_skipped_local_execution_candidate_with_failure_diagnostic,
};
use crate::ai_serving::planner::materialization_policy::{
    build_local_candidate_persistence_policy, LocalCandidatePersistencePolicyKind,
};
use crate::ai_serving::planner::passthrough::maybe_build_local_same_format_provider_decision_payload_for_candidate;
use crate::ai_serving::planner::report_context::{
    build_local_execution_report_context, LocalExecutionReportContextParts,
};
use crate::ai_serving::planner::spec_metadata::local_standard_spec_metadata;
use crate::ai_serving::planner::CandidateFailureDiagnostic;
use crate::ai_serving::planner::{
    build_ai_execution_decision_response, AiExecutionDecisionResponseParts,
};
use crate::ai_serving::transport::{
    resolve_transport_execution_timeouts, resolve_transport_tls_profile,
};
use crate::ai_serving::{
    ai_local_execution_contract_for_formats, api_format_alias_matches,
    resolve_local_same_format_stream_spec, resolve_local_same_format_sync_spec,
};
use crate::{
    append_execution_contract_fields_to_value, append_local_failover_policy_to_value,
    AiExecutionDecision, AppState,
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
) -> Option<AiExecutionDecision> {
    let spec_metadata = local_standard_spec_metadata(spec);
    if api_format_alias_matches(
        &attempt.eligible.provider_api_format,
        spec_metadata.api_format,
    ) {
        let same_format_spec = if spec_metadata.require_streaming {
            resolve_local_same_format_stream_spec(spec_metadata.decision_kind)
        } else {
            resolve_local_same_format_sync_spec(spec_metadata.decision_kind)
        };
        if let Some(same_format_spec) = same_format_spec {
            return maybe_build_local_same_format_provider_decision_payload_for_candidate(
                state,
                parts,
                trace_id,
                body_json,
                input,
                attempt,
                same_format_spec,
            )
            .await;
        }
    }

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
    let (execution_strategy, conversion_mode) = ai_local_execution_contract_for_formats(
        spec_metadata.api_format,
        resolved.provider_api_format.as_str(),
    );
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
                has_envelope: resolved.envelope_name.is_some(),
                needs_conversion: true,
                extra_fields,
            }),
            execution_strategy,
            conversion_mode,
            spec_metadata.api_format,
            resolved.provider_api_format.as_str(),
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

    Some(build_ai_execution_decision_response(
        AiExecutionDecisionResponseParts {
            decision_is_stream: spec_metadata.require_streaming,
            decision_kind: spec_metadata.decision_kind.to_string(),
            execution_strategy,
            conversion_mode,
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn mark_skipped_local_standard_candidate_with_extra_data(
    state: &AppState,
    input: &LocalStandardDecisionInput,
    trace_id: &str,
    candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    extra_data: Option<serde_json::Value>,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::StandardDecision,
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
pub(super) async fn mark_skipped_local_standard_candidate_with_failure_diagnostic(
    state: &AppState,
    input: &LocalStandardDecisionInput,
    trace_id: &str,
    candidate: &aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
    diagnostic: CandidateFailureDiagnostic,
) {
    let persistence_policy = build_local_candidate_persistence_policy(
        &input.auth_context,
        input.required_capabilities.as_ref(),
        LocalCandidatePersistencePolicyKind::StandardDecision,
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
    use std::sync::Arc;

    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
    use serde_json::json;

    use super::maybe_build_local_standard_decision_payload_for_candidate;
    use crate::ai_serving::planner::candidate_materialization::LocalExecutionCandidateAttempt;
    use crate::ai_serving::planner::candidate_resolution::EligibleLocalExecutionCandidate;
    use crate::ai_serving::planner::decision_input::LocalRequestedModelDecisionInput;
    use crate::ai_serving::{
        ExecutionRuntimeAuthContext, GatewayAuthApiKeySnapshot, LocalStandardSourceFamily,
        LocalStandardSourceMode, LocalStandardSpec,
    };
    use crate::orchestration::LocalExecutionCandidateMetadata;

    fn sample_auth_snapshot() -> GatewayAuthApiKeySnapshot {
        GatewayAuthApiKeySnapshot {
            user_id: "user-1".to_string(),
            username: "alice".to_string(),
            email: None,
            user_role: "user".to_string(),
            user_auth_source: "local".to_string(),
            user_is_active: true,
            user_is_deleted: false,
            user_rate_limit: None,
            user_allowed_providers: None,
            user_allowed_api_formats: None,
            user_allowed_models: None,
            api_key_id: "api-key-1".to_string(),
            api_key_name: Some("default".to_string()),
            api_key_is_active: true,
            api_key_is_locked: false,
            api_key_is_standalone: false,
            api_key_rate_limit: None,
            api_key_concurrent_limit: None,
            api_key_expires_at_unix_secs: None,
            api_key_allowed_providers: None,
            api_key_allowed_api_formats: None,
            api_key_allowed_models: None,
            currently_usable: true,
        }
    }

    fn sample_input() -> LocalRequestedModelDecisionInput {
        LocalRequestedModelDecisionInput {
            auth_context: ExecutionRuntimeAuthContext {
                user_id: "user-1".to_string(),
                api_key_id: "api-key-1".to_string(),
                username: Some("alice".to_string()),
                api_key_name: Some("default".to_string()),
                balance_remaining: Some(10.0),
                access_allowed: true,
                api_key_is_standalone: false,
            },
            requested_model: "claude-sonnet-4-5".to_string(),
            auth_snapshot: sample_auth_snapshot(),
            required_capabilities: None,
            request_auth_channel: None,
        }
    }

    fn sample_transport(api_format: &str, endpoint_id: &str) -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "provider".to_string(),
                provider_type: "custom".to_string(),
                website: None,
                is_active: true,
                keep_priority_on_conversion: false,
                enable_format_conversion: true,
                concurrent_limit: None,
                max_retries: None,
                proxy: None,
                request_timeout_secs: None,
                stream_first_byte_timeout_secs: None,
                config: None,
            },
            endpoint: GatewayProviderTransportEndpoint {
                id: endpoint_id.to_string(),
                provider_id: "provider-1".to_string(),
                api_format: api_format.to_string(),
                api_family: Some(
                    api_format
                        .split_once(':')
                        .map(|(family, _)| family)
                        .unwrap_or(api_format)
                        .to_string(),
                ),
                endpoint_kind: Some("chat".to_string()),
                is_active: true,
                base_url: "https://api.example.test".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: None,
                config: None,
                format_acceptance_config: if api_format == "openai:chat" {
                    Some(json!({
                        "enabled": true,
                        "accept_formats": ["claude:messages"],
                    }))
                } else {
                    None
                },
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "api_key".to_string(),
                is_active: true,
                api_formats: Some(vec![
                    "claude:messages".to_string(),
                    "openai:chat".to_string(),
                ]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: Some(json!({
                    "claude:messages": 1,
                    "openai:chat": 1,
                })),
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                decrypted_api_key: "sk-upstream".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    fn sample_candidate(
        api_format: &str,
        endpoint_id: &str,
    ) -> SchedulerMinimalCandidateSelectionCandidate {
        SchedulerMinimalCandidateSelectionCandidate {
            provider_id: "provider-1".to_string(),
            provider_name: "provider".to_string(),
            provider_type: "custom".to_string(),
            provider_priority: 1,
            endpoint_id: endpoint_id.to_string(),
            endpoint_api_format: api_format.to_string(),
            key_id: "key-1".to_string(),
            key_name: "key".to_string(),
            key_auth_type: "api_key".to_string(),
            key_internal_priority: 1,
            key_global_priority_for_format: Some(1),
            key_capabilities: None,
            model_id: format!("model-{endpoint_id}"),
            global_model_id: "global-model-1".to_string(),
            global_model_name: "claude-sonnet-4-5".to_string(),
            selected_provider_model_name: if api_format == "claude:messages" {
                "claude-sonnet-4-5-upstream".to_string()
            } else {
                "gpt-4o-upstream".to_string()
            },
            mapping_matched_model: None,
        }
    }

    fn sample_attempt(
        api_format: &str,
        endpoint_id: &str,
        candidate_index: u32,
    ) -> LocalExecutionCandidateAttempt {
        LocalExecutionCandidateAttempt {
            eligible: EligibleLocalExecutionCandidate {
                candidate: sample_candidate(api_format, endpoint_id),
                transport: Arc::new(sample_transport(api_format, endpoint_id)),
                provider_api_format: api_format.to_string(),
                orchestration: LocalExecutionCandidateMetadata::default(),
                ranking: None,
            },
            candidate_index,
            retry_index: 0,
            candidate_id: format!("candidate-{candidate_index}"),
        }
    }

    fn claude_stream_spec() -> LocalStandardSpec {
        LocalStandardSpec {
            api_format: "claude:messages",
            decision_kind: "claude_chat_stream",
            report_kind: "claude_chat_stream_success",
            family: LocalStandardSourceFamily::Standard,
            mode: LocalStandardSourceMode::Chat,
            require_streaming: true,
        }
    }

    #[tokio::test]
    async fn standard_family_builds_same_format_candidate_before_cross_format_candidate() {
        let state = crate::AppState::new().expect("state should build");
        let request = http::Request::builder()
            .method("POST")
            .uri("/v1/messages?beta=true")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let body_json = json!({
            "model": "claude-sonnet-4-5",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 32,
            "stream": true
        });
        let input = sample_input();

        let payload = maybe_build_local_standard_decision_payload_for_candidate(
            &state,
            &parts,
            "trace-standard-same-format-first",
            &body_json,
            &input,
            sample_attempt("claude:messages", "endpoint-claude", 0),
            claude_stream_spec(),
        )
        .await
        .expect("same-format candidate should build a standard-family payload");

        assert_eq!(payload.endpoint_id.as_deref(), Some("endpoint-claude"));
        assert_eq!(
            payload.execution_strategy.as_deref(),
            Some("local_same_format")
        );
        assert_eq!(payload.conversion_mode.as_deref(), Some("none"));
        assert_eq!(
            payload.provider_api_format.as_deref(),
            Some("claude:messages")
        );
        assert_eq!(
            payload.client_api_format.as_deref(),
            Some("claude:messages")
        );
        assert_eq!(
            payload
                .provider_request_body
                .as_ref()
                .and_then(|body| body.get("model"))
                .and_then(serde_json::Value::as_str),
            Some("claude-sonnet-4-5-upstream")
        );

        let cross_format_payload = maybe_build_local_standard_decision_payload_for_candidate(
            &state,
            &parts,
            "trace-standard-same-format-first",
            &body_json,
            &input,
            sample_attempt("openai:chat", "endpoint-openai-chat", 1),
            claude_stream_spec(),
        )
        .await
        .expect("cross-format candidate should still build after the same-format candidate");

        assert_eq!(
            cross_format_payload.endpoint_id.as_deref(),
            Some("endpoint-openai-chat")
        );
        assert_eq!(
            cross_format_payload.execution_strategy.as_deref(),
            Some("local_cross_format")
        );
        assert_eq!(
            cross_format_payload.conversion_mode.as_deref(),
            Some("bidirectional")
        );
    }
}
