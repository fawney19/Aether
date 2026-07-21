use std::collections::BTreeMap;
use std::sync::Arc;

use aether_contracts::ResolvedTransportProfile;
use serde_json::{json, Value};

use crate::ai_serving::planner::antigravity::{
    build_antigravity_v1internal_provider_request, AntigravityV1InternalRequestError,
    AntigravityV1InternalRequestInput, ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME,
};
use crate::ai_serving::planner::candidate_preparation::{
    prepare_header_authenticated_candidate, prepare_header_authenticated_candidate_from_auth,
    OauthPreparationContext,
};
use crate::ai_serving::planner::candidate_resolution::EligibleLocalExecutionCandidate;
use crate::ai_serving::planner::common::{
    endpoint_config_forces_body_stream_field, enforce_provider_body_stream_policy,
    request_requires_body_stream_field, OPENAI_CHAT_STREAM_PLAN_KIND,
};
use crate::ai_serving::planner::gemini_cli::{
    build_gemini_cli_v1internal_provider_request, GeminiCliV1InternalRequestError,
    GeminiCliV1InternalRequestInput,
};
use crate::ai_serving::planner::redaction::{
    request_identity_response_encoding_when_redacted, resolve_provider_chat_pii_redaction,
};
use crate::ai_serving::planner::standard::{
    apply_codex_openai_responses_special_body_edits, apply_codex_openai_special_headers,
    apply_deepseek_tool_call_thinking_compat, build_cross_format_openai_chat_request_body,
    build_cross_format_openai_chat_upstream_url, build_local_openai_chat_request_body,
    build_local_openai_chat_upstream_url, codex_model_capabilities_for_transport,
    request_body_build_failure_extra_data, request_conversion_failure_extra_data,
};
use crate::ai_serving::transport::antigravity::is_antigravity_provider_transport;
use crate::ai_serving::transport::auth::resolve_local_openai_bearer_auth;
use crate::ai_serving::transport::kiro::{
    build_kiro_provider_headers, build_kiro_provider_request_body,
    is_kiro_claude_messages_transport, KiroProviderHeadersInput, KiroRequestAuth,
    KIRO_ENVELOPE_NAME,
};
use crate::ai_serving::transport::local_openai_chat_transport_unsupported_reason;
use crate::ai_serving::transport::windsurf::{
    build_windsurf_cascade_headers, build_windsurf_cascade_request_body,
    build_windsurf_cascade_upstream_url, is_windsurf_provider_transport,
    local_windsurf_request_transport_unsupported_reason_with_network,
    resolve_windsurf_cascade_auth, WINDSURF_ENVELOPE_NAME,
};
use crate::ai_serving::transport::{
    build_grok_browser_headers, build_grok_upstream_url, build_kiro_cross_format_upstream_url,
    build_openai_image_headers, build_openai_image_upstream_url,
    build_standard_provider_request_headers, is_gemini_cli_provider_transport,
    openai_image_transport_unsupported_reason, resolve_openai_image_auth, GrokHeaderInput,
    ProviderOpenAiImageHeadersInput, StandardProviderRequestHeadersInput,
    GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME, GROK_CHAT_PATH,
};
use crate::ai_serving::{
    ai_local_execution_contract_for_formats, request_conversion_direct_auth,
    request_conversion_kind, CandidateFailureDiagnostic, GatewayProviderTransportSnapshot,
    LocalResolvedOAuthRequestAuth, OpenAiImageOperation,
};
use crate::ai_serving::{
    project_codex_openai_image_api_request_body, project_openai_image_api_request_body,
};
use crate::ai_serving::{ConversionMode, ExecutionStrategy};
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AppState, GatewayError};

use super::support::{
    mark_skipped_local_openai_chat_candidate,
    mark_skipped_local_openai_chat_candidate_with_extra_data,
    mark_skipped_local_openai_chat_candidate_with_failure_diagnostic, LocalOpenAiChatDecisionInput,
};

pub(crate) struct LocalOpenAiChatCandidatePayloadParts {
    pub(super) client_api_format: String,
    pub(super) auth_header: String,
    pub(super) auth_value: String,
    pub(super) mapped_model: String,
    pub(super) provider_api_format: String,
    pub(super) provider_request_body: Value,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) upstream_url: String,
    pub(super) execution_strategy: ExecutionStrategy,
    pub(super) conversion_mode: ConversionMode,
    pub(super) report_kind: String,
    pub(super) envelope_name: Option<&'static str>,
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) request_redacted: bool,
    pub(super) transport_profile: Option<ResolvedTransportProfile>,
    pub(super) image_request_summary: Option<Value>,
}

#[derive(Default)]
pub(crate) struct LocalOpenAiChatRequestPreparation;

fn is_grok_text_provider_api_format(provider_api_format: &str) -> bool {
    matches!(
        crate::ai_serving::normalize_api_format_alias(provider_api_format).as_str(),
        "openai:chat" | "openai:responses" | "openai:responses:compact" | "claude:messages"
    )
}

fn finalize_openai_chat_provider_request_body(
    provider_request_body: &mut Value,
    custom_directive_mapping: Option<&Value>,
    provider_api_format: &str,
    upstream_is_stream: bool,
    force_body_stream_field: bool,
    original_body: &Value,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
) -> bool {
    if let Some(mapping) = custom_directive_mapping {
        crate::ai_serving::apply_model_directive_mapping_patch(provider_request_body, mapping);
    }

    // Mapping and endpoint body rules can both write `stream`. The resolved transport
    // policy is authoritative and therefore runs after every body mutation.
    enforce_provider_body_stream_policy(
        provider_request_body,
        provider_api_format,
        upstream_is_stream,
        request_requires_body_stream_field(original_body, force_body_stream_field),
    );
    apply_deepseek_tool_call_thinking_compat(
        provider_request_body,
        transport.provider.provider_type.as_str(),
        transport.endpoint.base_url.as_str(),
        provider_api_format,
        Some(original_body),
    );
    let source_model = original_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(mapped_model);
    let codex_model_capabilities = codex_model_capabilities_for_transport(
        transport,
        provider_api_format,
        mapped_model,
        source_model,
    );
    crate::ai_serving::finalize_openai_provider_request_with_codex_model_capabilities(
        provider_request_body,
        crate::ai_serving::OpenAiProviderRequestFinalization {
            source_api_format: "openai:chat",
            provider_api_format,
            provider_type: transport.provider.provider_type.as_str(),
            provider_model: mapped_model,
            source_model,
            body_rules: transport.endpoint.body_rules.as_ref(),
            upstream_is_stream,
            require_body_stream_field: request_requires_body_stream_field(
                original_body,
                force_body_stream_field,
            ),
        },
        codex_model_capabilities.as_ref(),
    )
    .is_ok()
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_local_openai_chat_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    _preparation: Option<&mut LocalOpenAiChatRequestPreparation>,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    decision_kind: &str,
    report_kind: &str,
    upstream_is_stream: bool,
) -> Result<Option<LocalOpenAiChatCandidatePayloadParts>, GatewayError> {
    let prepare_started_at = std::time::Instant::now();
    let planner_state = crate::ai_serving::PlannerAppState::new(state);
    let candidate = &eligible.candidate;
    let provider_api_format = eligible.provider_api_format.as_str();
    let transport = &eligible.transport;
    let transport_profile = crate::ai_serving::transport::resolve_transport_profile(transport);
    let force_body_stream_field =
        endpoint_config_forces_body_stream_field(transport.endpoint.config.as_ref());
    let model_directives_started_at = std::time::Instant::now();
    let model_directive_resolution = input
        .model_directive_policy
        .resolve_reasoning(provider_api_format, Some(&input.requested_model));
    observe_gateway_stage_ms(
        "openai_chat_payload_model_directives",
        model_directives_started_at.elapsed().as_millis() as u64,
    );
    let redaction_started_at = std::time::Instant::now();
    let redaction = resolve_provider_chat_pii_redaction(
        state,
        parts,
        body_json,
        &input.auth_context,
        "openai:chat",
        candidate_id,
    )
    .await?;
    observe_gateway_stage_ms(
        "openai_chat_payload_redaction",
        redaction_started_at.elapsed().as_millis() as u64,
    );
    let body_json = redaction.body_json.as_ref();
    let effective_headers = input.effective_headers(&parts.headers);
    let is_grok = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("grok");
    let is_antigravity = is_antigravity_provider_transport(transport);
    let is_gemini_cli = is_gemini_cli_provider_transport(transport);

    if is_grok && is_grok_text_provider_api_format(provider_api_format) {
        let prepared_candidate = match prepare_header_authenticated_candidate(
            planner_state,
            transport,
            candidate,
            crate::ai_serving::transport::resolve_grok_session_auth(transport),
            OauthPreparationContext {
                trace_id,
                api_format: provider_api_format,
                operation: "openai_chat_same_format",
            },
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(skip_reason) => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    skip_reason,
                )
                .await;
                return Ok(None);
            }
        };
        let model_directive_mapping = match model_directive_resolution
            .mapping_patch_for_mapped_model(&prepared_candidate.mapped_model)
        {
            Ok(mapping) => mapping,
            Err(skip_reason) => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    skip_reason,
                )
                .await;
                return Ok(None);
            }
        };

        let Some(mut provider_request_body) = build_local_openai_chat_request_body(
            body_json,
            &prepared_candidate.mapped_model,
            upstream_is_stream,
            force_body_stream_field,
            transport.endpoint.body_rules.as_ref(),
            effective_headers,
            false,
        ) else {
            mark_skipped_local_openai_chat_candidate_with_extra_data(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                request_body_build_failure_extra_data(
                    body_json,
                    "openai:chat",
                    provider_api_format,
                ),
            )
            .await;
            return Ok(None);
        };
        if !finalize_openai_chat_provider_request_body(
            &mut provider_request_body,
            model_directive_mapping.as_ref(),
            provider_api_format,
            upstream_is_stream,
            force_body_stream_field,
            body_json,
            transport,
            &prepared_candidate.mapped_model,
        ) {
            mark_skipped_local_openai_chat_candidate_with_extra_data(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                request_body_build_failure_extra_data(
                    body_json,
                    "openai:chat",
                    provider_api_format,
                ),
            )
            .await;
            return Ok(None);
        }

        let upstream_url = build_grok_upstream_url(transport, GROK_CHAT_PATH);
        let Some(mut provider_request_headers) = build_grok_browser_headers(GrokHeaderInput {
            transport,
            transport_profile: transport_profile.as_ref(),
            request_headers: Some(effective_headers),
            content_type: "application/json",
            accept: "text/event-stream",
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
        }) else {
            mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    "openai:chat",
                    provider_api_format,
                    "grok_openai_chat_headers",
                ),
            )
            .await;
            return Ok(None);
        };

        let (execution_strategy, conversion_mode) =
            ai_local_execution_contract_for_formats("openai:chat", provider_api_format);
        let resolved_report_kind =
            if decision_kind == OPENAI_CHAT_STREAM_PLAN_KIND || !upstream_is_stream {
                report_kind.to_string()
            } else {
                "openai_chat_sync_finalize".to_string()
            };

        request_identity_response_encoding_when_redacted(
            &mut provider_request_headers,
            redaction.redacted,
        );

        let result = Ok(Some(LocalOpenAiChatCandidatePayloadParts {
            client_api_format: "openai:chat".to_string(),
            auth_header: prepared_candidate.auth_header,
            auth_value: prepared_candidate.auth_value,
            mapped_model: prepared_candidate.mapped_model,
            provider_api_format: provider_api_format.to_string(),
            provider_request_body,
            provider_request_headers,
            upstream_url,
            execution_strategy,
            conversion_mode,
            report_kind: resolved_report_kind,
            envelope_name: None,
            transport: Arc::clone(transport),
            request_redacted: redaction.redacted,
            transport_profile,
            image_request_summary: None,
        }));
        observe_gateway_stage_ms(
            "openai_chat_payload_parts_prepare",
            prepare_started_at.elapsed().as_millis() as u64,
        );
        return result;
    }

    if provider_api_format == "openai:chat" && is_windsurf_provider_transport(transport) {
        return build_windsurf_openai_chat_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            decision_kind,
            report_kind,
            transport,
            upstream_is_stream,
            redaction.redacted,
        )
        .await;
    }

    if provider_api_format == "openai:chat" {
        if let Some(skip_reason) = local_openai_chat_transport_unsupported_reason(transport) {
            mark_skipped_local_openai_chat_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                skip_reason,
            )
            .await;
            return Ok(None);
        };

        let auth_prepare_started_at = std::time::Instant::now();
        let prepared_candidate = match prepare_header_authenticated_candidate(
            planner_state,
            transport,
            candidate,
            resolve_local_openai_bearer_auth(transport),
            OauthPreparationContext {
                trace_id,
                api_format: "openai:chat",
                operation: "openai_chat_same_format",
            },
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(skip_reason) => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    skip_reason,
                )
                .await;
                return Ok(None);
            }
        };
        let model_directive_mapping = match model_directive_resolution
            .mapping_patch_for_mapped_model(&prepared_candidate.mapped_model)
        {
            Ok(mapping) => mapping,
            Err(skip_reason) => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    skip_reason,
                )
                .await;
                return Ok(None);
            }
        };
        observe_gateway_stage_ms(
            "openai_chat_payload_auth_prepare",
            auth_prepare_started_at.elapsed().as_millis() as u64,
        );

        let body_build_started_at = std::time::Instant::now();
        let Some(mut provider_request_body) = build_local_openai_chat_request_body(
            body_json,
            &prepared_candidate.mapped_model,
            upstream_is_stream,
            force_body_stream_field,
            transport.endpoint.body_rules.as_ref(),
            effective_headers,
            false,
        ) else {
            mark_skipped_local_openai_chat_candidate_with_extra_data(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                request_body_build_failure_extra_data(
                    body_json,
                    "openai:chat",
                    provider_api_format,
                ),
            )
            .await;
            return Ok(None);
        };
        observe_gateway_stage_ms(
            "openai_chat_payload_body_build",
            body_build_started_at.elapsed().as_millis() as u64,
        );
        if !finalize_openai_chat_provider_request_body(
            &mut provider_request_body,
            model_directive_mapping.as_ref(),
            "openai:chat",
            upstream_is_stream,
            force_body_stream_field,
            body_json,
            transport,
            &prepared_candidate.mapped_model,
        ) {
            mark_skipped_local_openai_chat_candidate_with_extra_data(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                request_body_build_failure_extra_data(
                    body_json,
                    "openai:chat",
                    provider_api_format,
                ),
            )
            .await;
            return Ok(None);
        }

        let Some(upstream_url) = build_local_openai_chat_upstream_url(parts, transport) else {
            mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "upstream_url_missing",
                CandidateFailureDiagnostic::upstream_url_missing(
                    "openai:chat",
                    provider_api_format,
                    "openai_chat_same_format_url",
                ),
            )
            .await;
            return Ok(None);
        };

        let Some(resolved_headers) =
            build_standard_provider_request_headers(StandardProviderRequestHeadersInput {
                transport,
                provider_api_format,
                same_format: true,
                headers: effective_headers,
                auth_header: &prepared_candidate.auth_header,
                auth_value: &prepared_candidate.auth_value,
                extra_headers: &BTreeMap::new(),
                header_rules: transport.endpoint.header_rules.as_ref(),
                provider_request_body: &provider_request_body,
                original_request_body: body_json,
                upstream_is_stream,
            })
        else {
            mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    "openai:chat",
                    provider_api_format,
                    "openai_chat_same_format_headers",
                ),
            )
            .await;
            return Ok(None);
        };
        let mut provider_request_headers = resolved_headers.headers;
        apply_codex_openai_special_headers(
            &mut provider_request_headers,
            &provider_request_body,
            effective_headers,
            transport.provider.provider_type.as_str(),
            transport.endpoint.api_format.as_str(),
            Some(trace_id),
            transport.key.decrypted_auth_config.as_deref(),
        );
        let (execution_strategy, conversion_mode) =
            ai_local_execution_contract_for_formats("openai:chat", "openai:chat");
        let resolved_report_kind =
            if decision_kind == OPENAI_CHAT_STREAM_PLAN_KIND || !upstream_is_stream {
                report_kind.to_string()
            } else {
                "openai_chat_sync_finalize".to_string()
            };

        request_identity_response_encoding_when_redacted(
            &mut provider_request_headers,
            redaction.redacted,
        );

        return Ok(Some(LocalOpenAiChatCandidatePayloadParts {
            client_api_format: "openai:chat".to_string(),
            auth_header: resolved_headers.auth_header,
            auth_value: resolved_headers.auth_value,
            mapped_model: prepared_candidate.mapped_model,
            provider_api_format: "openai:chat".to_string(),
            provider_request_body,
            provider_request_headers,
            upstream_url,
            execution_strategy,
            conversion_mode,
            report_kind: resolved_report_kind,
            envelope_name: None,
            transport: Arc::clone(transport),
            request_redacted: redaction.redacted,
            transport_profile,
            image_request_summary: None,
        }));
    };

    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    let normalized_provider_api_format =
        crate::ai_serving::normalize_api_format_alias(provider_api_format.as_str());
    if provider_api_format == "openai:image" {
        return resolve_openai_chat_to_openai_image_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            upstream_is_stream,
        )
        .await;
    }

    let Some(conversion_kind) =
        request_conversion_kind("openai:chat", provider_api_format.as_str())
    else {
        mark_skipped_local_openai_chat_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "transport_api_format_unsupported",
        )
        .await;
        return Ok(None);
    };
    if let Some(skip_reason) = crate::ai_serving::request_conversion_transport_unsupported_reason(
        transport,
        conversion_kind,
    ) {
        if !((is_antigravity || is_gemini_cli)
            && normalized_provider_api_format == "gemini:generate_content")
        {
            mark_skipped_local_openai_chat_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                skip_reason,
            )
            .await;
            return Ok(None);
        }
    }
    let is_kiro_claude_cli =
        is_kiro_claude_messages_transport(transport, provider_api_format.as_str());
    let oauth_context = OauthPreparationContext {
        trace_id,
        api_format: provider_api_format.as_str(),
        operation: "openai_chat_cross_format",
    };
    let kiro_auth = if is_kiro_claude_cli {
        match crate::ai_serving::planner::candidate_preparation::resolve_candidate_oauth_auth(
            planner_state,
            transport,
            oauth_context,
        )
        .await
        {
            Some(LocalResolvedOAuthRequestAuth::Kiro(auth)) => Some(auth),
            _ => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "transport_auth_unavailable",
                )
                .await;
                return Ok(None);
            }
        }
    } else {
        None
    };
    let prepared_candidate = if let Some(kiro_auth) = kiro_auth.as_ref() {
        match prepare_header_authenticated_candidate_from_auth(
            candidate,
            kiro_auth.name.to_string(),
            kiro_auth.value.clone(),
        ) {
            Ok(prepared) => prepared,
            Err(skip_reason) => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    skip_reason,
                )
                .await;
                return Ok(None);
            }
        }
    } else {
        match prepare_header_authenticated_candidate(
            planner_state,
            transport,
            candidate,
            request_conversion_direct_auth(transport, conversion_kind),
            oauth_context,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(skip_reason) => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    skip_reason,
                )
                .await;
                return Ok(None);
            }
        }
    };
    let model_directive_mapping = match model_directive_resolution
        .mapping_patch_for_mapped_model(&prepared_candidate.mapped_model)
    {
        Ok(mapping) => mapping,
        Err(skip_reason) => {
            mark_skipped_local_openai_chat_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                skip_reason,
            )
            .await;
            return Ok(None);
        }
    };

    let Some(mut provider_request_body) = build_cross_format_openai_chat_request_body(
        body_json,
        &prepared_candidate.mapped_model,
        transport.provider.provider_type.as_str(),
        provider_api_format.as_str(),
        upstream_is_stream,
        force_body_stream_field,
        if is_kiro_claude_cli {
            None
        } else {
            transport.endpoint.body_rules.as_ref()
        },
        Some(input.auth_context.api_key_id.as_str()),
        effective_headers,
        false,
    ) else {
        mark_skipped_local_openai_chat_candidate_with_extra_data(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "provider_request_body_build_failed",
            request_conversion_failure_extra_data(
                body_json,
                "openai:chat",
                provider_api_format.as_str(),
                Some(prepared_candidate.mapped_model.as_str()),
                Some(parts.uri.path()),
                upstream_is_stream,
                "openai_chat_request_conversion",
            ),
        )
        .await;
        return Ok(None);
    };
    if !finalize_openai_chat_provider_request_body(
        &mut provider_request_body,
        model_directive_mapping.as_ref(),
        provider_api_format.as_str(),
        upstream_is_stream,
        force_body_stream_field,
        body_json,
        transport,
        &prepared_candidate.mapped_model,
    ) {
        mark_skipped_local_openai_chat_candidate_with_extra_data(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "provider_request_body_build_failed",
            request_conversion_failure_extra_data(
                body_json,
                "openai:chat",
                provider_api_format.as_str(),
                Some(prepared_candidate.mapped_model.as_str()),
                Some(parts.uri.path()),
                upstream_is_stream,
                "openai_chat_request_conversion",
            ),
        )
        .await;
        return Ok(None);
    }

    if let Some(kiro_auth) = kiro_auth.as_ref() {
        return Ok(build_kiro_openai_chat_cross_format_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            decision_kind,
            transport,
            provider_api_format.as_str(),
            prepared_candidate.mapped_model,
            prepared_candidate.auth_header,
            prepared_candidate.auth_value,
            provider_request_body,
            upstream_is_stream,
            kiro_auth,
            redaction.redacted,
        )
        .await);
    }
    if provider_api_format == "gemini:generate_content"
        && is_gemini_cli_provider_transport(transport)
    {
        return Ok(build_gemini_cli_openai_chat_cross_format_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            decision_kind,
            transport,
            provider_api_format.as_str(),
            prepared_candidate.mapped_model,
            prepared_candidate.auth_header,
            prepared_candidate.auth_value,
            provider_request_body,
            upstream_is_stream,
            redaction.redacted,
        )
        .await);
    }
    if provider_api_format == "gemini:generate_content" && is_antigravity {
        return Ok(build_antigravity_openai_chat_cross_format_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            decision_kind,
            transport,
            provider_api_format.as_str(),
            prepared_candidate.mapped_model,
            prepared_candidate.auth_header,
            prepared_candidate.auth_value,
            provider_request_body,
            upstream_is_stream,
            redaction.redacted,
        )
        .await);
    }

    let Some(upstream_url) = build_cross_format_openai_chat_upstream_url(
        parts,
        transport,
        &prepared_candidate.mapped_model,
        provider_api_format.as_str(),
        upstream_is_stream,
    ) else {
        mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "upstream_url_missing",
            CandidateFailureDiagnostic::upstream_url_missing(
                "openai:chat",
                provider_api_format.as_str(),
                "openai_chat_cross_format_url",
            ),
        )
        .await;
        return Ok(None);
    };
    let Some(resolved_headers) =
        build_standard_provider_request_headers(StandardProviderRequestHeadersInput {
            transport,
            provider_api_format: provider_api_format.as_str(),
            same_format: false,
            headers: effective_headers,
            auth_header: &prepared_candidate.auth_header,
            auth_value: &prepared_candidate.auth_value,
            extra_headers: &BTreeMap::new(),
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
            upstream_is_stream,
        })
    else {
        mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "transport_header_rules_apply_failed",
            CandidateFailureDiagnostic::header_rules_apply_failed(
                "openai:chat",
                provider_api_format.as_str(),
                "openai_chat_cross_format_headers",
            ),
        )
        .await;
        return Ok(None);
    };
    let mut provider_request_headers = resolved_headers.headers;
    apply_codex_openai_special_headers(
        &mut provider_request_headers,
        &provider_request_body,
        effective_headers,
        transport.provider.provider_type.as_str(),
        provider_api_format.as_str(),
        Some(trace_id),
        transport.key.decrypted_auth_config.as_deref(),
    );
    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        redaction.redacted,
    );

    let resolved_report_kind = if decision_kind == OPENAI_CHAT_STREAM_PLAN_KIND {
        "openai_chat_stream_success".to_string()
    } else {
        "openai_chat_sync_finalize".to_string()
    };
    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats("openai:chat", provider_api_format.as_str());

    Ok(Some(LocalOpenAiChatCandidatePayloadParts {
        client_api_format: "openai:chat".to_string(),
        auth_header: resolved_headers.auth_header,
        auth_value: resolved_headers.auth_value,
        mapped_model: prepared_candidate.mapped_model,
        provider_api_format,
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        report_kind: resolved_report_kind,
        envelope_name: None,
        transport: Arc::clone(transport),
        request_redacted: redaction.redacted,
        transport_profile: None,
        image_request_summary: None,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn build_antigravity_openai_chat_cross_format_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    decision_kind: &str,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    gemini_request_body: Value,
    upstream_is_stream: bool,
    request_redacted: bool,
) -> Option<LocalOpenAiChatCandidatePayloadParts> {
    let candidate = &eligible.candidate;
    let effective_headers = input.effective_headers(&parts.headers);
    let resolved =
        match build_antigravity_v1internal_provider_request(AntigravityV1InternalRequestInput {
            state,
            parts,
            transport,
            trace_id,
            mapped_model: &mapped_model,
            provider_api_format,
            auth_header: &auth_header,
            auth_value: &auth_value,
            request_headers: effective_headers,
            original_request_body: original_body_json,
            gemini_request_body: &gemini_request_body,
            upstream_is_stream,
            same_format: false,
        })
        .await
        {
            Ok(resolved) => resolved,
            Err(AntigravityV1InternalRequestError::TransportUnsupported) => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "transport_unsupported",
                )
                .await;
                return None;
            }
            Err(AntigravityV1InternalRequestError::EnvelopeUnsupported) => {
                mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "provider_request_body_build_failed",
                    CandidateFailureDiagnostic::envelope_build_failed(
                        "openai:chat",
                        provider_api_format,
                        "openai_chat_antigravity_envelope",
                    ),
                )
                .await;
                return None;
            }
            Err(AntigravityV1InternalRequestError::UpstreamUrlUnavailable) => {
                mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "upstream_url_missing",
                    CandidateFailureDiagnostic::upstream_url_missing(
                        "openai:chat",
                        provider_api_format,
                        "openai_chat_antigravity_url",
                    ),
                )
                .await;
                return None;
            }
            Err(AntigravityV1InternalRequestError::HeaderRulesApplyFailed) => {
                mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "transport_header_rules_apply_failed",
                    CandidateFailureDiagnostic::header_rules_apply_failed(
                        "openai:chat",
                        provider_api_format,
                        "openai_chat_antigravity_headers",
                    ),
                )
                .await;
                return None;
            }
        };
    let mut provider_request_headers = resolved.headers.headers;
    apply_codex_openai_special_headers(
        &mut provider_request_headers,
        &resolved.body,
        effective_headers,
        resolved.transport.provider.provider_type.as_str(),
        provider_api_format,
        Some(trace_id),
        resolved.transport.key.decrypted_auth_config.as_deref(),
    );
    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        request_redacted,
    );

    let resolved_report_kind = if decision_kind == OPENAI_CHAT_STREAM_PLAN_KIND {
        "openai_chat_stream_success".to_string()
    } else {
        "openai_chat_sync_finalize".to_string()
    };
    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats("openai:chat", provider_api_format);

    Some(LocalOpenAiChatCandidatePayloadParts {
        client_api_format: "openai:chat".to_string(),
        auth_header: resolved.headers.auth_header,
        auth_value: resolved.headers.auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body: resolved.body,
        provider_request_headers,
        upstream_url: resolved.upstream_url,
        execution_strategy,
        conversion_mode,
        report_kind: resolved_report_kind,
        envelope_name: Some(ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME),
        transport: resolved.transport,
        request_redacted,
        transport_profile: None,
        image_request_summary: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn build_gemini_cli_openai_chat_cross_format_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    decision_kind: &str,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    gemini_request_body: Value,
    upstream_is_stream: bool,
    request_redacted: bool,
) -> Option<LocalOpenAiChatCandidatePayloadParts> {
    let candidate = &eligible.candidate;
    let effective_headers = input.effective_headers(&parts.headers);
    let resolved =
        match build_gemini_cli_v1internal_provider_request(GeminiCliV1InternalRequestInput {
            state,
            parts,
            transport,
            trace_id,
            mapped_model: &mapped_model,
            provider_api_format,
            auth_header: &auth_header,
            auth_value: &auth_value,
            request_headers: effective_headers,
            original_request_body: original_body_json,
            gemini_request_body: &gemini_request_body,
            upstream_is_stream,
        })
        .await
        {
            Ok(resolved) => resolved,
            Err(GeminiCliV1InternalRequestError::ProjectUnavailable) => {
                mark_skipped_local_openai_chat_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "transport_auth_unavailable",
                )
                .await;
                return None;
            }
            Err(GeminiCliV1InternalRequestError::EnvelopeUnsupported) => {
                mark_skipped_local_openai_chat_candidate_with_extra_data(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "provider_request_body_build_failed",
                    request_body_build_failure_extra_data(
                        original_body_json,
                        "openai:chat",
                        provider_api_format,
                    ),
                )
                .await;
                return None;
            }
            Err(GeminiCliV1InternalRequestError::UpstreamUrlUnavailable) => {
                mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "upstream_url_missing",
                    CandidateFailureDiagnostic::upstream_url_missing(
                        "openai:chat",
                        provider_api_format,
                        "openai_chat_gemini_cli_url",
                    ),
                )
                .await;
                return None;
            }
            Err(GeminiCliV1InternalRequestError::HeaderRulesApplyFailed) => {
                mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "transport_header_rules_apply_failed",
                    CandidateFailureDiagnostic::header_rules_apply_failed(
                        "openai:chat",
                        provider_api_format,
                        "openai_chat_gemini_cli_headers",
                    ),
                )
                .await;
                return None;
            }
        };
    let mut provider_request_headers = resolved.headers.headers;
    apply_codex_openai_special_headers(
        &mut provider_request_headers,
        &resolved.body,
        effective_headers,
        resolved.transport.provider.provider_type.as_str(),
        provider_api_format,
        Some(trace_id),
        resolved.transport.key.decrypted_auth_config.as_deref(),
    );
    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        request_redacted,
    );

    let resolved_report_kind = if decision_kind == OPENAI_CHAT_STREAM_PLAN_KIND {
        "openai_chat_stream_success".to_string()
    } else {
        "openai_chat_sync_finalize".to_string()
    };
    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats("openai:chat", provider_api_format);

    Some(LocalOpenAiChatCandidatePayloadParts {
        client_api_format: "openai:chat".to_string(),
        auth_header: resolved.headers.auth_header,
        auth_value: resolved.headers.auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body: resolved.body,
        provider_request_headers,
        upstream_url: resolved.upstream_url,
        execution_strategy,
        conversion_mode,
        report_kind: resolved_report_kind,
        envelope_name: Some(GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME),
        transport: resolved.transport,
        request_redacted,
        transport_profile: None,
        image_request_summary: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn resolve_openai_chat_to_openai_image_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    upstream_is_stream: bool,
) -> Result<Option<LocalOpenAiChatCandidatePayloadParts>, GatewayError> {
    let candidate = &eligible.candidate;
    let transport = &eligible.transport;
    let provider_api_format = "openai:image";
    if let Some(skip_reason) =
        openai_image_transport_unsupported_reason(transport, provider_api_format)
    {
        mark_skipped_local_openai_chat_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            skip_reason,
        )
        .await;
        return Ok(None);
    }

    let prepared_candidate = match prepare_header_authenticated_candidate(
        crate::ai_serving::PlannerAppState::new(state),
        transport,
        candidate,
        resolve_openai_image_auth(transport),
        OauthPreparationContext {
            trace_id,
            api_format: provider_api_format,
            operation: "openai_chat_image_bridge",
        },
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(skip_reason) => {
            mark_skipped_local_openai_chat_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                skip_reason,
            )
            .await;
            return Ok(None);
        }
    };

    let is_chatgpt_web = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("chatgpt_web");
    let is_codex = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex");
    let upstream_is_stream =
        crate::ai_serving::planner::common::resolve_upstream_is_stream_for_provider(
            transport.endpoint.config.as_ref(),
            transport.provider.provider_type.as_str(),
            provider_api_format,
            upstream_is_stream,
            false,
        );
    let Some((mut provider_request_body, image_request_summary)) = (if is_chatgpt_web {
        build_chatgpt_web_image_provider_body_from_openai_chat_body(
            body_json,
            &input.requested_model,
        )
    } else {
        build_openai_image_provider_body_from_openai_chat_body(
            body_json,
            &prepared_candidate.mapped_model,
            upstream_is_stream,
        )
    }) else {
        mark_skipped_local_openai_chat_candidate_with_extra_data(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "provider_request_body_build_failed",
            request_body_build_failure_extra_data(body_json, "openai:chat", provider_api_format),
        )
        .await;
        return Ok(None);
    };
    let Some(operation) = openai_image_operation_from_summary(&image_request_summary) else {
        return Ok(None);
    };
    if !is_chatgpt_web {
        let Some(projected) = project_openai_image_api_request_body(
            &provider_request_body,
            &prepared_candidate.mapped_model,
            operation,
            crate::image_capabilities::openai_image_provider_max_generation_count_for_model(
                transport.provider.provider_type.as_str(),
                Some(prepared_candidate.mapped_model.as_str()),
            ),
        ) else {
            mark_skipped_local_openai_chat_candidate_with_extra_data(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                request_body_build_failure_extra_data(
                    body_json,
                    "openai:chat",
                    provider_api_format,
                ),
            )
            .await;
            return Ok(None);
        };
        provider_request_body = projected;
    }
    if is_codex {
        let Some(projected) =
            project_codex_openai_image_api_request_body(&provider_request_body, operation)
        else {
            mark_skipped_local_openai_chat_candidate_with_extra_data(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                request_body_build_failure_extra_data(
                    body_json,
                    "openai:chat",
                    provider_api_format,
                ),
            )
            .await;
            return Ok(None);
        };
        provider_request_body = projected;
    }

    let upstream_url = if is_chatgpt_web {
        chatgpt_web_image_internal_url(&transport.endpoint.base_url)
    } else {
        let request_path = match operation {
            OpenAiImageOperation::Generate => "/v1/images/generations",
            OpenAiImageOperation::Edit => "/v1/images/edits",
        };
        build_openai_image_upstream_url(transport, Some(request_path), parts.uri.query())
    };
    let Some(mut provider_request_headers) =
        build_openai_image_headers(ProviderOpenAiImageHeadersInput {
            transport,
            headers: &parts.headers,
            auth_header: &prepared_candidate.auth_header,
            auth_value: &prepared_candidate.auth_value,
            accept: if is_codex {
                None
            } else if upstream_is_stream {
                Some("text/event-stream")
            } else {
                Some("application/json")
            },
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
        })
    else {
        mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "transport_header_rules_apply_failed",
            CandidateFailureDiagnostic::header_rules_apply_failed(
                "openai:chat",
                provider_api_format,
                "openai_chat_image_bridge_headers",
            ),
        )
        .await;
        return Ok(None);
    };
    if is_chatgpt_web {
        provider_request_headers.insert("x-aether-chatgpt-web-image".to_string(), "1".to_string());
    } else {
        apply_codex_openai_special_headers(
            &mut provider_request_headers,
            &provider_request_body,
            &parts.headers,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            Some(trace_id),
            transport.key.decrypted_auth_config.as_deref(),
        );
    }

    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats("openai:chat", provider_api_format);

    Ok(Some(LocalOpenAiChatCandidatePayloadParts {
        client_api_format: "openai:chat".to_string(),
        auth_header: prepared_candidate.auth_header,
        auth_value: prepared_candidate.auth_value,
        mapped_model: prepared_candidate.mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        report_kind: "openai_chat_stream_success".to_string(),
        envelope_name: None,
        transport: Arc::clone(transport),
        request_redacted: false,
        transport_profile: None,
        image_request_summary: Some(image_request_summary),
    }))
}

fn build_openai_image_provider_body_from_openai_chat_body(
    body_json: &Value,
    requested_model: &str,
    upstream_is_stream: bool,
) -> Option<(Value, Value)> {
    let (prompt, images) = collect_openai_chat_image_prompt_and_images(body_json)?;
    let operation = if images.is_empty() {
        "generate"
    } else {
        "edit"
    };
    let mut image_options = serde_json::Map::new();
    copy_openai_chat_image_option(body_json, &mut image_options, "size");
    copy_openai_chat_image_option(body_json, &mut image_options, "quality");
    copy_openai_chat_image_option(body_json, &mut image_options, "background");
    copy_openai_chat_image_option(body_json, &mut image_options, "output_format");
    copy_openai_chat_image_option(body_json, &mut image_options, "output_compression");
    copy_openai_chat_image_option(body_json, &mut image_options, "moderation");
    copy_openai_chat_image_option(body_json, &mut image_options, "input_fidelity");
    copy_openai_chat_image_option(body_json, &mut image_options, "partial_images");

    let mut body = serde_json::Map::new();
    let requested_model = requested_model.trim();
    if requested_model.is_empty() {
        return None;
    }
    body.insert(
        "model".to_string(),
        Value::String(requested_model.to_string()),
    );
    body.insert("prompt".to_string(), Value::String(prompt));
    body.extend(image_options.clone());
    if operation == "edit" {
        let image_urls = openai_image_inputs_as_api_urls(&images);
        if image_urls.len() != images.len() {
            return None;
        }
        body.insert("images".to_string(), Value::Array(image_urls));
    }
    if upstream_is_stream {
        body.insert("stream".to_string(), Value::Bool(true));
    }
    if let Some(user) = body_json
        .get("user")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.insert("user".to_string(), Value::String(user.to_string()));
    }

    let mut summary = serde_json::Map::new();
    summary.insert(
        "operation".to_string(),
        Value::String(operation.to_string()),
    );
    for key in ["output_format", "partial_images", "size", "quality"] {
        if let Some(value) = image_options.get(key) {
            summary.insert(key.to_string(), value.clone());
        }
    }
    Some((Value::Object(body), Value::Object(summary)))
}

fn openai_image_operation_from_summary(summary: &Value) -> Option<OpenAiImageOperation> {
    match summary.get("operation")?.as_str()? {
        "generate" => Some(OpenAiImageOperation::Generate),
        "edit" => Some(OpenAiImageOperation::Edit),
        _ => None,
    }
}

fn build_chatgpt_web_image_provider_body_from_openai_chat_body(
    body_json: &Value,
    requested_model: &str,
) -> Option<(Value, Value)> {
    let (prompt, images) = collect_openai_chat_image_prompt_and_images(body_json)?;
    let operation = if images.is_empty() {
        "generate"
    } else {
        "edit"
    };
    let size = body_json
        .get("size")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("1024x1024");
    let output_format = body_json
        .get("output_format")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("png");
    let quality = body_json
        .get("quality")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("medium");
    let model = body_json
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| requested_model.trim());
    let web_model = body_json
        .get("web_model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("gpt-5-5-thinking");
    let image_urls = openai_image_inputs_as_urls(&images);

    let body = json!({
        "operation": operation,
        "model": if model.is_empty() { "gpt-image-2" } else { model },
        "web_model": web_model,
        "prompt": prompt,
        "size": size,
        "ratio": chatgpt_web_ratio_for_size(size),
        "output_format": output_format,
        "images": image_urls,
    });
    let summary = json!({
        "operation": operation,
        "output_format": output_format,
        "size": size,
        "quality": quality,
    });
    Some((body, summary))
}

fn copy_openai_chat_image_option(
    body_json: &Value,
    image_options: &mut serde_json::Map<String, Value>,
    key: &str,
) {
    if let Some(value) = body_json.get(key) {
        image_options.insert(key.to_string(), value.clone());
    }
}

fn collect_openai_chat_image_prompt_and_images(body_json: &Value) -> Option<(String, Vec<Value>)> {
    let messages = body_json.get("messages").and_then(Value::as_array)?;
    let mut prompt_parts = Vec::new();
    let mut images = Vec::new();
    for message in messages.iter().filter_map(Value::as_object) {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let content = message.get("content");
        if matches!(role, "system" | "developer" | "user") {
            if let Some(text) = crate::ai_serving::extract_openai_text_content(content)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            {
                prompt_parts.push(text);
            }
        }
        if role == "user" {
            collect_openai_chat_image_inputs(content, &mut images);
        }
    }
    let prompt = prompt_parts.join("\n").trim().to_string();
    (!prompt.is_empty()).then_some((prompt, images))
}

fn collect_openai_chat_image_inputs(content: Option<&Value>, images: &mut Vec<Value>) {
    let Some(parts) = content.and_then(Value::as_array) else {
        return;
    };
    for part in parts.iter().filter_map(Value::as_object) {
        let part_type = part
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if matches!(part_type, "image_url" | "input_image") {
            if let Some(url) = part
                .get("image_url")
                .and_then(|value| {
                    value
                        .as_str()
                        .or_else(|| value.get("url").and_then(Value::as_str))
                })
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                images.push(serde_json::json!({
                    "type": "input_image",
                    "image_url": url,
                }));
            } else if let Some(file_id) = part
                .get("file_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                images.push(serde_json::json!({
                    "type": "input_image",
                    "file_id": file_id,
                }));
            }
        }
    }
}

fn openai_image_inputs_as_urls(images: &[Value]) -> Vec<Value> {
    images
        .iter()
        .filter_map(|image| {
            image
                .get("image_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
        })
        .collect()
}

fn openai_image_inputs_as_api_urls(images: &[Value]) -> Vec<Value> {
    images
        .iter()
        .filter_map(|image| {
            image
                .get("image_url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| json!({ "image_url": value }))
        })
        .collect()
}

fn chatgpt_web_ratio_for_size(size: &str) -> String {
    let Some((width, height)) = size.split_once('x') else {
        return "1:1".to_string();
    };
    let Ok(width) = width.trim().parse::<u64>() else {
        return "1:1".to_string();
    };
    let Ok(height) = height.trim().parse::<u64>() else {
        return "1:1".to_string();
    };
    if width == 0 || height == 0 {
        return "1:1".to_string();
    }
    let divisor = gcd(width, height);
    format!("{}:{}", width / divisor, height / divisor)
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let next = left % right;
        left = right;
        right = next;
    }
    left.max(1)
}

fn chatgpt_web_image_internal_url(base_url: &str) -> String {
    let base_url = base_url.trim().trim_end_matches('/');
    let base_url = if base_url.is_empty() {
        "https://chatgpt.com"
    } else {
        base_url
    };
    format!("{base_url}/__aether/chatgpt-web-image")
}

#[allow(clippy::too_many_arguments)]
async fn build_windsurf_openai_chat_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    decision_kind: &str,
    report_kind: &str,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    upstream_is_stream: bool,
    request_redacted: bool,
) -> Result<Option<LocalOpenAiChatCandidatePayloadParts>, GatewayError> {
    let planner_state = crate::ai_serving::PlannerAppState::new(state);
    let candidate = &eligible.candidate;
    if let Some(skip_reason) =
        local_windsurf_request_transport_unsupported_reason_with_network(transport)
    {
        mark_skipped_local_openai_chat_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            skip_reason,
        )
        .await;
        return Ok(None);
    }

    let prepared_candidate = match prepare_header_authenticated_candidate(
        planner_state,
        transport,
        candidate,
        resolve_windsurf_cascade_auth(transport)
            .or_else(|| resolve_local_openai_bearer_auth(transport)),
        OauthPreparationContext {
            trace_id,
            api_format: "openai:chat",
            operation: "openai_chat_windsurf_cascade",
        },
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(skip_reason) => {
            mark_skipped_local_openai_chat_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                skip_reason,
            )
            .await;
            return Ok(None);
        }
    };

    let Some(provider_request_body) = build_windsurf_cascade_request_body(
        body_json,
        &prepared_candidate.mapped_model,
        &prepared_candidate.auth_value,
        transport.endpoint.body_rules.as_ref(),
        Some(&parts.headers),
        upstream_is_stream,
    ) else {
        mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "provider_request_body_build_failed",
            CandidateFailureDiagnostic::envelope_build_failed(
                "openai:chat",
                "openai:chat",
                "openai_chat_windsurf_cascade",
            ),
        )
        .await;
        return Ok(None);
    };

    let Some(upstream_url) = build_windsurf_cascade_upstream_url(
        transport.endpoint.base_url.as_str(),
        parts.uri.query(),
    ) else {
        mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "upstream_url_missing",
            CandidateFailureDiagnostic::upstream_url_missing(
                "openai:chat",
                "openai:chat",
                "openai_chat_windsurf_url",
            ),
        )
        .await;
        return Ok(None);
    };

    let mut provider_request_headers = match build_windsurf_cascade_headers(
        &parts.headers,
        &provider_request_body,
        body_json,
        transport.endpoint.header_rules.as_ref(),
        &prepared_candidate.auth_header,
        &prepared_candidate.auth_value,
        upstream_is_stream,
    ) {
        Some(headers) => headers,
        None => {
            mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    "openai:chat",
                    "openai:chat",
                    "openai_chat_windsurf_headers",
                ),
            )
            .await;
            return Ok(None);
        }
    };
    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        request_redacted,
    );

    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats("openai:chat", "openai:chat");
    let resolved_report_kind =
        if decision_kind == OPENAI_CHAT_STREAM_PLAN_KIND || !upstream_is_stream {
            report_kind.to_string()
        } else {
            "openai_chat_sync_finalize".to_string()
        };

    Ok(Some(LocalOpenAiChatCandidatePayloadParts {
        client_api_format: "openai:chat".to_string(),
        auth_header: prepared_candidate.auth_header,
        auth_value: prepared_candidate.auth_value,
        mapped_model: prepared_candidate.mapped_model,
        provider_api_format: "openai:chat".to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        report_kind: resolved_report_kind,
        envelope_name: Some(WINDSURF_ENVELOPE_NAME),
        transport: Arc::clone(transport),
        request_redacted,
        transport_profile: None,
        image_request_summary: None,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn build_kiro_openai_chat_cross_format_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    decision_kind: &str,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    claude_request_body: Value,
    upstream_is_stream: bool,
    kiro_auth: &KiroRequestAuth,
    request_redacted: bool,
) -> Option<LocalOpenAiChatCandidatePayloadParts> {
    let candidate = &eligible.candidate;
    let effective_headers = input.effective_headers(&parts.headers);
    let provider_request_body = match build_kiro_provider_request_body(
        &claude_request_body,
        &mapped_model,
        &kiro_auth.auth_config,
        transport.endpoint.body_rules.as_ref(),
        Some(effective_headers),
    ) {
        Some(body) => body,
        None => {
            mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                CandidateFailureDiagnostic::envelope_build_failed(
                    "openai:chat",
                    provider_api_format,
                    "openai_chat_kiro_envelope",
                ),
            )
            .await;
            return None;
        }
    };
    let upstream_url = match build_kiro_cross_format_upstream_url(
        transport,
        &mapped_model,
        provider_api_format,
        upstream_is_stream,
        parts.uri.query(),
        kiro_auth.auth_config.effective_api_region(),
    ) {
        Some(url) => url,
        None => {
            mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "upstream_url_missing",
                CandidateFailureDiagnostic::upstream_url_missing(
                    "openai:chat",
                    provider_api_format,
                    "openai_chat_kiro_url",
                ),
            )
            .await;
            return None;
        }
    };
    let mut provider_request_headers = match build_kiro_provider_headers(KiroProviderHeadersInput {
        headers: effective_headers,
        provider_request_body: &provider_request_body,
        original_request_body: original_body_json,
        header_rules: transport.endpoint.header_rules.as_ref(),
        auth_header: &auth_header,
        auth_value: &auth_value,
        auth_config: &kiro_auth.auth_config,
        machine_id: kiro_auth.machine_id.as_str(),
    }) {
        Some(headers) => headers,
        None => {
            mark_skipped_local_openai_chat_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    "openai:chat",
                    provider_api_format,
                    "openai_chat_kiro_headers",
                ),
            )
            .await;
            return None;
        }
    };
    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        request_redacted,
    );
    let resolved_report_kind = if decision_kind == OPENAI_CHAT_STREAM_PLAN_KIND {
        "openai_chat_stream_success".to_string()
    } else {
        "openai_chat_sync_finalize".to_string()
    };
    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats("openai:chat", provider_api_format);

    Some(LocalOpenAiChatCandidatePayloadParts {
        client_api_format: "openai:chat".to_string(),
        auth_header,
        auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        report_kind: resolved_report_kind,
        envelope_name: Some(KIRO_ENVELOPE_NAME),
        transport: Arc::clone(transport),
        request_redacted,
        transport_profile: None,
        image_request_summary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_provider_transport::snapshot::{
        GatewayProviderTransportEndpoint, GatewayProviderTransportKey,
        GatewayProviderTransportProvider, GatewayProviderTransportSnapshot,
    };
    use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

    fn sample_auth_snapshot() -> crate::ai_serving::GatewayAuthApiKeySnapshot {
        crate::ai_serving::GatewayAuthApiKeySnapshot {
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
            api_key_ip_rules: None,
            currently_usable: true,
        }
    }

    fn sample_input() -> LocalOpenAiChatDecisionInput {
        LocalOpenAiChatDecisionInput {
            auth_context: crate::ai_serving::ExecutionRuntimeAuthContext {
                user_id: "user-1".to_string(),
                api_key_id: "api-key-1".to_string(),
                username: Some("alice".to_string()),
                api_key_name: Some("default".to_string()),
                balance_remaining: Some(10.0),
                access_allowed: true,
                api_key_is_standalone: false,
            },
            requested_model: "gemini-2.5-pro".to_string(),
            auth_snapshot: sample_auth_snapshot(),
            required_capabilities: None,
            request_auth_channel: None,
            client_session_affinity: None,
            routing_policy: None,
            routing_trace_seed: None,
            routing_context: None,
            model_directive_policy: Default::default(),
        }
    }

    fn sample_gemini_cli_transport() -> GatewayProviderTransportSnapshot {
        GatewayProviderTransportSnapshot {
            provider: GatewayProviderTransportProvider {
                id: "provider-1".to_string(),
                name: "gemini".to_string(),
                provider_type: "gemini_cli".to_string(),
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
                id: "endpoint-1".to_string(),
                provider_id: "provider-1".to_string(),
                api_format: "gemini:generate_content".to_string(),
                api_family: Some("gemini".to_string()),
                endpoint_kind: Some("generate_content".to_string()),
                is_active: true,
                base_url: "https://cloudcode-pa.googleapis.com".to_string(),
                header_rules: None,
                body_rules: None,
                max_retries: None,
                custom_path: Some("/v1internal:{action}".to_string()),
                config: None,
                format_acceptance_config: None,
                proxy: None,
            },
            key: GatewayProviderTransportKey {
                id: "key-1".to_string(),
                provider_id: "provider-1".to_string(),
                name: "key".to_string(),
                auth_type: "bearer".to_string(),
                is_active: true,
                api_formats: Some(vec!["gemini:generate_content".to_string()]),
                auth_type_by_format: None,
                allow_auth_channel_mismatch_formats: None,
                allowed_models: None,
                capabilities: None,
                rate_multipliers: None,
                global_priority_by_format: Some(json!({
                    "gemini:generate_content": 1,
                })),
                expires_at_unix_secs: None,
                proxy: None,
                fingerprint: None,
                upstream_metadata: Some(json!({
                    "gemini_cli": {
                        "project_id": "test-project"
                    }
                })),
                decrypted_api_key: "oauth-access-token".to_string(),
                decrypted_auth_config: None,
            },
        }
    }

    fn sample_gemini_cli_eligible() -> EligibleLocalExecutionCandidate {
        EligibleLocalExecutionCandidate {
            kind: crate::ai_serving::planner::candidate_resolution::LocalExecutionCandidateKind::SingleKey,
            candidate: SchedulerMinimalCandidateSelectionCandidate {
                provider_id: "provider-1".to_string(),
                provider_name: "gemini".to_string(),
                provider_type: "gemini_cli".to_string(),
                provider_priority: 1,
                endpoint_id: "endpoint-1".to_string(),
                endpoint_api_format: "gemini:generate_content".to_string(),
                key_id: "key-1".to_string(),
                key_name: "key".to_string(),
                key_auth_type: "bearer".to_string(),
                key_internal_priority: 1,
                key_global_priority_for_format: Some(1),
                key_capabilities: None,
                model_id: "model-1".to_string(),
                global_model_id: "global-model-1".to_string(),
                global_model_name: "gemini-2.5-pro".to_string(),
                selected_provider_model_name: "gemini-2.5-pro".to_string(),
                supports_streaming: true,
                mapping_matched_model: None,
            },
            transport: Arc::new(sample_gemini_cli_transport()),
            provider_api_format: "gemini:generate_content".to_string(),
            orchestration: crate::orchestration::LocalExecutionCandidateMetadata::default(),
            ranking: None,
        }
    }

    fn sample_antigravity_transport() -> GatewayProviderTransportSnapshot {
        let mut transport = sample_gemini_cli_transport();
        transport.provider.name = "antigravity".to_string();
        transport.provider.provider_type = "antigravity".to_string();
        transport.endpoint.base_url = "https://antigravity.googleapis.com".to_string();
        transport.endpoint.custom_path = None;
        transport.key.auth_type = "oauth".to_string();
        transport.key.decrypted_api_key = "__placeholder__".to_string();
        transport.key.upstream_metadata = None;
        transport.key.decrypted_auth_config = Some(
            json!({
                "provider_type": "antigravity",
                "project_id": "test-antigravity-project",
                "client_version": "1.2.3",
                "session_id": "sess-antigravity-chat",
                "access_token_import_temporary": true,
                "headers": {
                    "Authorization": "Bearer imported-antigravity-token"
                }
            })
            .to_string(),
        );
        transport
    }

    fn sample_antigravity_eligible() -> EligibleLocalExecutionCandidate {
        let mut eligible = sample_gemini_cli_eligible();
        eligible.candidate.provider_name = "antigravity".to_string();
        eligible.candidate.provider_type = "antigravity".to_string();
        eligible.transport = Arc::new(sample_antigravity_transport());
        eligible
    }

    fn sample_openai_chat_eligible(provider_type: &str) -> EligibleLocalExecutionCandidate {
        let mut transport = sample_gemini_cli_transport();
        transport.provider.name = provider_type.to_string();
        transport.provider.provider_type = provider_type.to_string();
        transport.endpoint.api_format = "openai:chat".to_string();
        transport.endpoint.api_family = Some("openai".to_string());
        transport.endpoint.endpoint_kind = Some("chat_completions".to_string());
        transport.endpoint.base_url = if provider_type == "grok" {
            "https://grok.com".to_string()
        } else {
            "https://api.openai.test".to_string()
        };
        transport.endpoint.custom_path = None;
        transport.key.api_formats = Some(vec!["openai:chat".to_string()]);
        transport.key.upstream_metadata = None;
        if provider_type == "grok" {
            transport.key.auth_type = "oauth".to_string();
            transport.key.decrypted_api_key.clear();
            transport.key.decrypted_auth_config =
                Some(json!({ "sso_token": "test-session" }).to_string());
        } else {
            transport.key.auth_type = "bearer".to_string();
            transport.key.decrypted_api_key = "test-api-key".to_string();
            transport.key.decrypted_auth_config = None;
        }

        let mut eligible = sample_gemini_cli_eligible();
        eligible.candidate.provider_name = provider_type.to_string();
        eligible.candidate.provider_type = provider_type.to_string();
        eligible.candidate.endpoint_api_format = "openai:chat".to_string();
        eligible.candidate.global_model_name = "gpt-5.6-sol".to_string();
        eligible.candidate.selected_provider_model_name = "gpt-5.6-sol".to_string();
        eligible.transport = Arc::new(transport);
        eligible.provider_api_format = "openai:chat".to_string();
        eligible
    }

    fn sample_custom_directive_input() -> LocalOpenAiChatDecisionInput {
        let mut input = sample_input();
        input.requested_model = "gpt-5.6-sol-high".to_string();
        input.model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::from_config_values(
                Some(&json!(true)),
                Some(&json!({
                    "reasoning_effort": {
                        "api_formats": {
                            "openai:chat": {
                                "suffixes": ["high"],
                                "mappings": {
                                    "high": {
                                        "reasoning_effort": "low",
                                        "stream": false
                                    }
                                }
                            }
                        }
                    }
                })),
            );
        input
    }

    fn sample_alias_max_directive_input() -> LocalOpenAiChatDecisionInput {
        let mut input = sample_input();
        input.requested_model = "deployment-alias-max".to_string();
        input.model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::from_config_values(
                Some(&json!(true)),
                None,
            );
        input
    }

    #[tokio::test]
    async fn alias_reasoning_directive_is_constrained_by_the_mapped_openai_model() {
        let state = AppState::new().expect("state should build");
        let request = http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let body_json = json!({
            "model": "deployment-alias-max",
            "messages": [{"role": "user", "content": "hello"}]
        });

        let mut supported = sample_openai_chat_eligible("custom");
        supported.candidate.selected_provider_model_name = "gpt-5.6-sol".to_string();
        let payload = resolve_local_openai_chat_candidate_payload_parts(
            &state,
            &parts,
            "trace-alias-max-gpt-5.6-sol",
            &body_json,
            &sample_alias_max_directive_input(),
            None,
            &supported,
            0,
            "candidate-0",
            "openai_chat_sync",
            "openai_chat_sync_success",
            false,
        )
        .await
        .expect("candidate resolution should not fail")
        .expect("GPT-5.6 candidate should build a payload");
        assert_eq!(payload.provider_request_body["reasoning_effort"], "max");

        let mut unsupported = sample_openai_chat_eligible("custom");
        unsupported.candidate.selected_provider_model_name = "gpt-5.4".to_string();
        let payload = resolve_local_openai_chat_candidate_payload_parts(
            &state,
            &parts,
            "trace-alias-max-gpt-5.4",
            &body_json,
            &sample_alias_max_directive_input(),
            None,
            &unsupported,
            0,
            "candidate-0",
            "openai_chat_sync",
            "openai_chat_sync_success",
            false,
        )
        .await
        .expect("candidate resolution should not fail");
        assert!(payload.is_none(), "GPT-5.4 must reject the max directive");
    }

    #[tokio::test]
    async fn custom_policy_suffix_patch_is_applied_after_candidate_mapping() {
        let state = AppState::new().expect("state should build");
        let request = http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let body_json = json!({
            "model": "deployment-alias-VendorFuture",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let mut input = sample_input();
        input.requested_model = "deployment-alias-VendorFuture".to_string();
        input.model_directive_policy =
            crate::system_features::ModelDirectivePolicySnapshot::from_config_values(
                Some(&json!(true)),
                Some(&json!({
                    "reasoning_effort": {
                        "api_formats": {
                            "openai:chat": {
                                "suffixes": ["VendorFuture"],
                                "mappings": {
                                    "VendorFuture": {
                                        "reasoning_effort": "high"
                                    }
                                }
                            }
                        }
                    }
                })),
            );
        let payload = resolve_local_openai_chat_candidate_payload_parts(
            &state,
            &parts,
            "trace-custom-policy-suffix",
            &body_json,
            &input,
            None,
            &sample_openai_chat_eligible("custom"),
            0,
            "candidate-0",
            "openai_chat_sync",
            "openai_chat_sync_success",
            false,
        )
        .await
        .expect("candidate resolution should not fail")
        .expect("custom directive candidate should build a payload");

        assert_eq!(payload.provider_request_body["model"], "gpt-5.6-sol");
        assert_eq!(payload.provider_request_body["reasoning_effort"], "high");
    }

    #[tokio::test]
    async fn same_format_and_grok_chat_apply_the_same_custom_directive_finalization() {
        let state = AppState::new().expect("state should build");
        let request = http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let body_json = json!({
            "model": "gpt-5.6-sol-high",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": true
        });

        for provider_type in ["custom", "grok"] {
            let payload = resolve_local_openai_chat_candidate_payload_parts(
                &state,
                &parts,
                &format!("trace-directive-{provider_type}"),
                &body_json,
                &sample_custom_directive_input(),
                None,
                &sample_openai_chat_eligible(provider_type),
                0,
                "candidate-0",
                OPENAI_CHAT_STREAM_PLAN_KIND,
                "openai_chat_stream_success",
                true,
            )
            .await
            .expect("candidate resolution should not fail")
            .expect("same-format candidate should build a payload");

            assert_eq!(
                payload.provider_request_body["reasoning_effort"], "low",
                "custom mapping must be authoritative for {provider_type}"
            );
            assert_eq!(
                payload.provider_request_body["stream"], true,
                "stream policy must be re-applied after mapping for {provider_type}"
            );
        }
    }

    #[tokio::test]
    async fn openai_chat_to_gemini_cli_wraps_cross_format_body_in_v1internal_envelope() {
        let state = AppState::new().expect("state should build");
        let request = http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let body_json = json!({
            "model": "gemini-2.5-pro",
            "messages": [{"role": "user", "content": "hello"}],
            "generationConfig": {"temperature": 0.2},
            "stream": true
        });

        let payload = resolve_local_openai_chat_candidate_payload_parts(
            &state,
            &parts,
            "trace-openai-chat-gemini-cli",
            &body_json,
            &sample_input(),
            None,
            &sample_gemini_cli_eligible(),
            0,
            "candidate-0",
            OPENAI_CHAT_STREAM_PLAN_KIND,
            "openai_chat_stream_success",
            true,
        )
        .await
        .expect("candidate resolution should not fail")
        .expect("gemini_cli candidate should build a payload");

        assert_eq!(
            payload.upstream_url,
            "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            payload.envelope_name,
            Some(GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME)
        );
        assert_eq!(
            payload
                .provider_request_headers
                .get("user-agent")
                .map(String::as_str),
            Some(crate::ai_serving::transport::GEMINI_CLI_USER_AGENT)
        );
        assert_eq!(payload.provider_request_body["model"], "gemini-2.5-pro");
        assert_eq!(payload.provider_request_body["project"], "test-project");
        assert_eq!(
            payload.provider_request_body["user_prompt_id"],
            "trace-openai-chat-gemini-cli"
        );
        assert!(payload.provider_request_body.get("contents").is_none());
        assert!(payload
            .provider_request_body
            .get("generationConfig")
            .is_none());
        assert!(payload.provider_request_body["request"]
            .get("contents")
            .is_some());
    }

    #[tokio::test]
    async fn openai_chat_to_antigravity_wraps_cross_format_body_in_v1internal_envelope() {
        let state = AppState::new().expect("state should build");
        let request = http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(())
            .expect("request should build");
        let (parts, _) = request.into_parts();
        let body_json = json!({
            "model": "gemini-3.5-flash",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": true
        });

        let payload = resolve_local_openai_chat_candidate_payload_parts(
            &state,
            &parts,
            "trace-openai-chat-antigravity",
            &body_json,
            &sample_input(),
            None,
            &sample_antigravity_eligible(),
            0,
            "candidate-0",
            OPENAI_CHAT_STREAM_PLAN_KIND,
            "openai_chat_stream_success",
            true,
        )
        .await
        .expect("candidate resolution should not fail")
        .expect("antigravity candidate should build a payload");

        assert_eq!(
            payload.upstream_url,
            "https://antigravity.googleapis.com/v1internal:streamGenerateContent?alt=sse"
        );
        assert_eq!(payload.envelope_name, Some("antigravity:v1internal"));
        assert_eq!(
            payload
                .provider_request_headers
                .get("authorization")
                .map(String::as_str),
            Some("Bearer imported-antigravity-token")
        );
        assert_eq!(
            payload
                .provider_request_headers
                .get("x-client-name")
                .map(String::as_str),
            Some("antigravity")
        );
        assert_eq!(
            payload
                .provider_request_headers
                .get("x-client-version")
                .map(String::as_str),
            Some("1.2.3")
        );
        assert_eq!(
            payload
                .provider_request_headers
                .get("x-vscode-sessionid")
                .map(String::as_str),
            Some("sess-antigravity-chat")
        );
        assert_eq!(
            payload.provider_request_body["project"],
            "test-antigravity-project"
        );
        assert_eq!(
            payload.provider_request_body["requestId"],
            "trace-openai-chat-antigravity"
        );
        assert_eq!(payload.provider_request_body["model"], "gemini-2.5-pro");
        assert_eq!(
            payload.provider_request_body["userAgent"],
            "antigravity/cli/1.0.16 (aidev_client; os_type=linux; arch=arm64; auth_method=consumer)"
        );
        assert_eq!(payload.provider_request_body["requestType"], "agent");
        assert!(payload.provider_request_body.get("contents").is_none());
        assert!(payload.provider_request_body["request"]
            .get("contents")
            .is_some());
        assert!(payload.provider_request_body["request"]
            .get("model")
            .is_none());
    }

    #[test]
    fn chatgpt_web_chat_image_bridge_body_uses_internal_web_shape() {
        let body_json = json!({
            "model": "gpt-image-2",
            "messages": [
                {"role": "system", "content": "Use crisp vector-like shapes."},
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Draw a glass city"},
                        {"type": "image_url", "image_url": {"url": "https://example.com/ref.png"}}
                    ]
                }
            ],
            "size": "1536x1024",
            "output_format": "webp",
            "web_model": "gpt-5-image-test"
        });

        let (provider_body, summary) =
            build_chatgpt_web_image_provider_body_from_openai_chat_body(&body_json, "gpt-image-2")
                .expect("chat image body should convert");

        assert_eq!(provider_body["operation"], "edit");
        assert_eq!(provider_body["model"], "gpt-image-2");
        assert_eq!(provider_body["web_model"], "gpt-5-image-test");
        assert_eq!(
            provider_body["prompt"],
            "Use crisp vector-like shapes.\nDraw a glass city"
        );
        assert_eq!(provider_body["size"], "1536x1024");
        assert_eq!(provider_body["ratio"], "3:2");
        assert_eq!(provider_body["output_format"], "webp");
        assert_eq!(provider_body["images"][0], "https://example.com/ref.png");
        assert_eq!(summary["operation"], "edit");
        assert_eq!(summary["output_format"], "webp");
    }

    #[test]
    fn openai_chat_image_bridge_builds_images_api_body() {
        let body_json = json!({
            "model": "gpt-image-2",
            "messages": [
                {"role": "user", "content": "Draw a glass city"}
            ],
            "size": "1024x1024",
            "output_format": "png"
        });

        let (provider_body, summary) =
            build_openai_image_provider_body_from_openai_chat_body(&body_json, "gpt-image-2", true)
                .expect("chat image body should convert");

        assert_eq!(provider_body["model"], "gpt-image-2");
        assert_eq!(provider_body["prompt"], "Draw a glass city");
        assert_eq!(provider_body["size"], "1024x1024");
        assert_eq!(provider_body["output_format"], "png");
        assert_eq!(provider_body["stream"], true);
        assert!(provider_body.get("tools").is_none());
        assert!(provider_body.get("input").is_none());
        assert_eq!(summary["operation"], "generate");
        assert_eq!(summary["output_format"], "png");

        let (sync_provider_body, _) = build_openai_image_provider_body_from_openai_chat_body(
            &body_json,
            "gpt-image-2",
            false,
        )
        .expect("chat image body should convert for a sync upstream");
        assert!(sync_provider_body.get("stream").is_none());
    }
}
