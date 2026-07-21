use std::collections::BTreeMap;
use std::sync::Arc;

use aether_contracts::ResolvedTransportProfile;
use serde_json::{json, Value};
use tracing::debug;

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
    request_requires_body_stream_field, resolve_upstream_is_stream_for_provider,
};
use crate::ai_serving::planner::gemini_cli::{
    build_gemini_cli_v1internal_provider_request, GeminiCliV1InternalRequestError,
    GeminiCliV1InternalRequestInput,
};
use crate::ai_serving::planner::redaction::{
    request_identity_response_encoding_when_redacted, resolve_provider_chat_pii_redaction,
};
use crate::ai_serving::planner::spec_metadata::local_openai_responses_spec_metadata;
use crate::ai_serving::planner::standard::{
    apply_codex_openai_special_headers, apply_deepseek_tool_call_thinking_compat,
    build_cross_format_openai_responses_request_body_with_codex_model_capabilities,
    build_cross_format_openai_responses_upstream_url,
    build_local_openai_responses_request_body_with_codex_model_capabilities,
    build_local_openai_responses_upstream_url, codex_model_capabilities_for_transport,
    request_body_build_failure_extra_data, request_conversion_failure_extra_data,
};
use crate::ai_serving::transport::antigravity::is_antigravity_provider_transport;
use crate::ai_serving::transport::auth::{
    resolve_local_gemini_auth, resolve_local_openai_bearer_auth, resolve_local_standard_auth,
};
use crate::ai_serving::transport::kiro::{
    build_kiro_provider_headers, build_kiro_provider_request_body,
    is_kiro_claude_messages_transport,
    local_kiro_request_transport_unsupported_reason_with_network, KiroProviderHeadersInput,
    KiroRequestAuth, KIRO_ENVELOPE_NAME,
};
use crate::ai_serving::transport::{
    apply_local_auth_config_header_overrides, build_grok_browser_headers, build_grok_upstream_url,
    build_kiro_cross_format_upstream_url, build_openai_image_headers,
    build_openai_image_upstream_url, build_standard_provider_request_headers,
    build_windsurf_cascade_headers, build_windsurf_cascade_request_body,
    build_windsurf_cascade_upstream_url, is_gemini_cli_provider_transport,
    is_windsurf_provider_transport, local_standard_transport_unsupported_reason_with_network,
    local_windsurf_request_transport_unsupported_reason_with_network,
    openai_image_transport_unsupported_reason, resolve_openai_image_auth, GrokHeaderInput,
    ProviderOpenAiImageHeadersInput, StandardProviderRequestHeadersInput,
    GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME, GROK_CHAT_PATH, WINDSURF_ENVELOPE_NAME,
};
use crate::ai_serving::{
    ai_local_execution_contract_for_formats, request_conversion_direct_auth,
    request_conversion_kind, CandidateFailureDiagnostic, GatewayProviderTransportSnapshot,
    LocalResolvedOAuthRequestAuth, OpenAiImageOperation, PlannerAppState,
};
use crate::ai_serving::{
    project_codex_openai_image_api_request_body, project_openai_image_api_request_body,
};
use crate::ai_serving::{ConversionMode, ExecutionStrategy};
use crate::{AppState, GatewayError};

use super::support::{
    mark_skipped_local_openai_responses_candidate,
    mark_skipped_local_openai_responses_candidate_with_extra_data,
    mark_skipped_local_openai_responses_candidate_with_failure_diagnostic,
    LocalOpenAiResponsesDecisionInput,
};
use super::LocalOpenAiResponsesSpec;

fn is_grok_text_provider_api_format(provider_api_format: &str) -> bool {
    matches!(
        crate::ai_serving::normalize_api_format_alias(provider_api_format).as_str(),
        "openai:chat" | "openai:responses" | "openai:responses:compact" | "claude:messages"
    )
}

pub(crate) struct LocalOpenAiResponsesCandidatePayloadParts {
    pub(super) auth_header: String,
    pub(super) auth_value: String,
    pub(super) mapped_model: String,
    pub(super) provider_api_format: String,
    pub(super) provider_request_body: Value,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) upstream_url: String,
    pub(super) execution_strategy: ExecutionStrategy,
    pub(super) conversion_mode: ConversionMode,
    pub(super) is_antigravity: bool,
    pub(super) envelope_name: Option<&'static str>,
    pub(super) upstream_is_stream: bool,
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) transport_profile: Option<ResolvedTransportProfile>,
    pub(super) image_request_summary: Option<Value>,
    pub(super) request_redacted: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_local_openai_responses_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiResponsesDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    spec: LocalOpenAiResponsesSpec,
) -> Result<Option<LocalOpenAiResponsesCandidatePayloadParts>, GatewayError> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let client_api_format = spec_metadata.api_format.trim().to_ascii_lowercase();
    let planner_state = PlannerAppState::new(state);
    let candidate = &eligible.candidate;
    let provider_api_format = eligible.provider_api_format.as_str();
    let normalized_provider_api_format =
        crate::ai_serving::normalize_api_format_alias(provider_api_format);
    let transport = Arc::clone(&eligible.transport);
    let transport_profile = crate::ai_serving::transport::resolve_transport_profile(&transport);
    let is_antigravity = is_antigravity_provider_transport(&transport);
    let is_gemini_cli = is_gemini_cli_provider_transport(&transport);
    let is_kiro_claude_cli = is_kiro_claude_messages_transport(&transport, provider_api_format);
    let is_grok = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("grok");

    if !is_grok && provider_api_format.eq_ignore_ascii_case("openai:image") {
        return Ok(resolve_openai_responses_to_openai_image_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            spec,
        )
        .await);
    }
    let is_windsurf_cascade =
        provider_api_format == "openai:chat" && is_windsurf_provider_transport(&transport);

    let same_format = api_format_alias_matches(provider_api_format, &client_api_format);
    let conversion_kind = request_conversion_kind(spec_metadata.api_format, provider_api_format);
    let transport_unsupported_reason = if is_grok
        && is_grok_text_provider_api_format(provider_api_format)
    {
        None
    } else if same_format && is_kiro_claude_cli {
        local_kiro_request_transport_unsupported_reason_with_network(&transport)
    } else if same_format {
        local_standard_transport_unsupported_reason_with_network(&transport, provider_api_format)
    } else if is_windsurf_cascade {
        local_windsurf_request_transport_unsupported_reason_with_network(&transport)
    } else {
        match conversion_kind {
            Some(_)
                if (is_antigravity || is_gemini_cli)
                    && normalized_provider_api_format == "gemini:generate_content" =>
            {
                None
            }
            Some(kind) => {
                crate::ai_serving::request_conversion_transport_unsupported_reason(&transport, kind)
            }
            None => Some("transport_api_format_unsupported"),
        }
    };
    if let Some(skip_reason) = transport_unsupported_reason {
        mark_skipped_local_openai_responses_candidate(
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

    let oauth_context = OauthPreparationContext {
        trace_id,
        api_format: provider_api_format,
        operation: "openai_responses_candidate_request",
    };
    let kiro_auth = if is_kiro_claude_cli {
        match crate::ai_serving::planner::candidate_preparation::resolve_candidate_oauth_auth(
            planner_state,
            &transport,
            oauth_context,
        )
        .await
        {
            Some(LocalResolvedOAuthRequestAuth::Kiro(auth)) => Some(auth),
            _ => {
                mark_skipped_local_openai_responses_candidate(
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

    let direct_auth = if is_grok && is_grok_text_provider_api_format(provider_api_format) {
        crate::ai_serving::transport::resolve_grok_session_auth(&transport)
    } else if kiro_auth.is_some() {
        None
    } else if same_format {
        match crate::ai_serving::normalize_api_format_alias(provider_api_format).as_str() {
            "gemini:generate_content" => resolve_local_gemini_auth(&transport),
            "claude:messages" => resolve_local_standard_auth(&transport),
            "openai:responses" | "openai:responses:compact" => {
                resolve_local_openai_bearer_auth(&transport)
            }
            _ => None,
        }
    } else {
        conversion_kind.and_then(|kind| request_conversion_direct_auth(&transport, kind))
    };
    let prepared_candidate = if let Some(kiro_auth) = kiro_auth.as_ref() {
        match prepare_header_authenticated_candidate_from_auth(
            candidate,
            kiro_auth.name.to_string(),
            kiro_auth.value.clone(),
        ) {
            Ok(prepared) => prepared,
            Err(skip_reason) => {
                mark_skipped_local_openai_responses_candidate(
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
            &transport,
            candidate,
            direct_auth,
            oauth_context,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(skip_reason) => {
                mark_skipped_local_openai_responses_candidate(
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
    let auth_header = prepared_candidate.auth_header;
    let auth_value = prepared_candidate.auth_value;
    let mapped_model = prepared_candidate.mapped_model;
    let model_directive_resolution = input
        .model_directive_policy
        .resolve_reasoning(provider_api_format, Some(&input.requested_model));
    let model_directive_mapping =
        match model_directive_resolution.mapping_patch_for_mapped_model(&mapped_model) {
            Ok(mapping) => mapping,
            Err(skip_reason) => {
                mark_skipped_local_openai_responses_candidate(
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
    let redaction = resolve_provider_chat_pii_redaction(
        state,
        parts,
        body_json,
        &input.auth_context,
        spec_metadata.api_format,
        candidate_id,
    )
    .await?;
    let body_json = redaction.body_json.as_ref();

    let needs_bidirectional_conversion = !same_format && conversion_kind.is_some();
    let upstream_is_stream = resolve_upstream_is_stream_for_provider(
        transport.endpoint.config.as_ref(),
        transport.provider.provider_type.as_str(),
        provider_api_format,
        spec_metadata.require_streaming,
        is_antigravity || is_kiro_claude_cli,
    );
    let force_body_stream_field =
        endpoint_config_forces_body_stream_field(transport.endpoint.config.as_ref());
    let effective_headers = input.effective_headers(&parts.headers);
    let source_model = body_json
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(input.requested_model.as_str());
    let codex_model_capabilities = codex_model_capabilities_for_transport(
        &transport,
        provider_api_format,
        mapped_model.as_str(),
        source_model,
    );
    let Some(mut base_provider_request_body) =
        (if is_grok && is_grok_text_provider_api_format(provider_api_format) {
            build_local_openai_responses_request_body_with_codex_model_capabilities(
                body_json,
                &mapped_model,
                upstream_is_stream,
                force_body_stream_field,
                transport.provider.provider_type.as_str(),
                spec_metadata.api_format,
                transport.endpoint.body_rules.as_ref(),
                effective_headers,
                codex_model_capabilities.as_ref(),
                false,
            )
        } else if needs_bidirectional_conversion {
            build_cross_format_openai_responses_request_body_with_codex_model_capabilities(
                body_json,
                &mapped_model,
                spec_metadata.api_format,
                provider_api_format,
                upstream_is_stream,
                force_body_stream_field,
                transport.provider.provider_type.as_str(),
                if is_kiro_claude_cli || is_windsurf_cascade {
                    None
                } else {
                    transport.endpoint.body_rules.as_ref()
                },
                effective_headers,
                codex_model_capabilities.as_ref(),
                false,
            )
        } else {
            build_local_openai_responses_request_body_with_codex_model_capabilities(
                body_json,
                &mapped_model,
                upstream_is_stream,
                force_body_stream_field,
                transport.provider.provider_type.as_str(),
                provider_api_format,
                if is_kiro_claude_cli || is_windsurf_cascade {
                    None
                } else {
                    transport.endpoint.body_rules.as_ref()
                },
                effective_headers,
                codex_model_capabilities.as_ref(),
                false,
            )
        })
    else {
        mark_skipped_local_openai_responses_candidate_with_extra_data(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "provider_request_body_build_failed",
            request_conversion_failure_extra_data(
                body_json,
                spec_metadata.api_format,
                provider_api_format,
                Some(mapped_model.as_str()),
                Some(parts.uri.path()),
                upstream_is_stream,
                "openai_responses_request_conversion",
            ),
        )
        .await;
        return Ok(None);
    };
    if let Some(mapping) = model_directive_mapping.as_ref() {
        crate::ai_serving::apply_model_directive_mapping_patch(
            &mut base_provider_request_body,
            mapping,
        );
        // Directive mapping is a deep-merge patch and may overwrite/add `stream`;
        // re-enforce stream-field policy afterward.
        enforce_provider_body_stream_policy(
            &mut base_provider_request_body,
            provider_api_format,
            upstream_is_stream,
            request_requires_body_stream_field(body_json, force_body_stream_field),
        );
    }
    apply_deepseek_tool_call_thinking_compat(
        &mut base_provider_request_body,
        transport.provider.provider_type.as_str(),
        transport.endpoint.base_url.as_str(),
        provider_api_format,
        Some(body_json),
    );
    if crate::ai_serving::finalize_openai_provider_request_with_codex_model_capabilities(
        &mut base_provider_request_body,
        crate::ai_serving::OpenAiProviderRequestFinalization {
            source_api_format: spec_metadata.api_format,
            provider_api_format,
            provider_type: transport.provider.provider_type.as_str(),
            provider_model: mapped_model.as_str(),
            source_model,
            body_rules: transport.endpoint.body_rules.as_ref(),
            upstream_is_stream,
            require_body_stream_field: request_requires_body_stream_field(
                body_json,
                force_body_stream_field,
            ),
        },
        codex_model_capabilities.as_ref(),
    )
    .is_err()
    {
        mark_skipped_local_openai_responses_candidate_with_extra_data(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "provider_request_body_build_failed",
            request_conversion_failure_extra_data(
                body_json,
                spec_metadata.api_format,
                provider_api_format,
                Some(mapped_model.as_str()),
                Some(parts.uri.path()),
                upstream_is_stream,
                "openai_responses_request_conversion",
            ),
        )
        .await;
        return Ok(None);
    }
    let provider_request_body = base_provider_request_body;

    if let Some(kiro_auth) = kiro_auth.as_ref() {
        return build_kiro_openai_responses_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            spec_metadata.api_format,
            &transport,
            provider_api_format,
            mapped_model,
            auth_header,
            auth_value,
            provider_request_body,
            upstream_is_stream,
            needs_bidirectional_conversion,
            kiro_auth,
            redaction.redacted,
        )
        .await;
    }
    if is_windsurf_cascade {
        return Ok(build_windsurf_openai_responses_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            spec_metadata.api_format,
            &transport,
            provider_api_format,
            mapped_model,
            auth_header,
            auth_value,
            provider_request_body,
            upstream_is_stream,
            redaction.redacted,
        )
        .await);
    }
    if provider_api_format == "gemini:generate_content"
        && is_gemini_cli_provider_transport(&transport)
    {
        return Ok(build_gemini_cli_openai_responses_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            spec_metadata.api_format,
            &transport,
            provider_api_format,
            mapped_model,
            auth_header,
            auth_value,
            provider_request_body,
            upstream_is_stream,
            redaction.redacted,
        )
        .await);
    }
    if is_antigravity {
        return Ok(build_antigravity_openai_responses_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            eligible,
            candidate_index,
            candidate_id,
            spec_metadata.api_format,
            &transport,
            provider_api_format,
            mapped_model,
            auth_header,
            auth_value,
            provider_request_body,
            upstream_is_stream,
            redaction.redacted,
        )
        .await);
    }

    let Some(upstream_url) = (if is_grok && is_grok_text_provider_api_format(provider_api_format) {
        Some(build_grok_upstream_url(&transport, GROK_CHAT_PATH))
    } else if needs_bidirectional_conversion {
        build_cross_format_openai_responses_upstream_url(
            parts,
            &transport,
            &mapped_model,
            spec_metadata.api_format,
            provider_api_format,
            upstream_is_stream,
        )
    } else {
        build_local_openai_responses_upstream_url(
            parts,
            &transport,
            api_format_alias_matches(provider_api_format, "openai:responses:compact"),
        )
    }) else {
        mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "upstream_url_missing",
            CandidateFailureDiagnostic::upstream_url_missing(
                spec_metadata.api_format,
                provider_api_format,
                "openai_responses_url",
            ),
        )
        .await;
        return Ok(None);
    };
    let extra_headers = BTreeMap::new();
    let resolved_headers = if is_grok && is_grok_text_provider_api_format(provider_api_format) {
        let Some(headers) = build_grok_browser_headers(GrokHeaderInput {
            transport: &transport,
            transport_profile: transport_profile.as_ref(),
            request_headers: Some(effective_headers),
            content_type: "application/json",
            accept: "text/event-stream",
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
        }) else {
            mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    spec_metadata.api_format,
                    provider_api_format,
                    "grok_openai_responses_headers",
                ),
            )
            .await;
            return Ok(None);
        };
        crate::ai_serving::transport::StandardProviderRequestHeaders {
            headers,
            auth_header: auth_header.clone(),
            auth_value: auth_value.clone(),
        }
    } else {
        let Some(resolved_headers) =
            build_standard_provider_request_headers(StandardProviderRequestHeadersInput {
                transport: &transport,
                provider_api_format,
                same_format,
                headers: effective_headers,
                auth_header: &auth_header,
                auth_value: &auth_value,
                extra_headers: &extra_headers,
                header_rules: transport.endpoint.header_rules.as_ref(),
                provider_request_body: &provider_request_body,
                original_request_body: body_json,
                upstream_is_stream,
            })
        else {
            mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    spec_metadata.api_format,
                    provider_api_format,
                    "openai_responses_headers",
                ),
            )
            .await;
            return Ok(None);
        };
        resolved_headers
    };
    let mut provider_request_headers = resolved_headers.headers;
    if !is_grok {
        apply_local_auth_config_header_overrides(
            &mut provider_request_headers,
            transport.key.decrypted_auth_config.as_deref(),
        );
        apply_codex_openai_special_headers(
            &mut provider_request_headers,
            &provider_request_body,
            effective_headers,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            Some(trace_id),
            transport.key.decrypted_auth_config.as_deref(),
        );
        crate::ai_serving::apply_codex_openai_responses_lite_header_for_request_body_with_capabilities(
            &mut provider_request_headers,
            Some(&provider_request_body),
            transport.provider.provider_type.as_str(),
            provider_api_format,
            mapped_model.as_str(),
            source_model,
            codex_model_capabilities.as_ref(),
        );
    }
    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        redaction.redacted,
    );

    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats(spec_metadata.api_format, provider_api_format);

    debug!(
        event_name = "local_openai_responses_upstream_url_resolved",
        log_type = "debug",
        trace_id = %trace_id,
        candidate_id = %candidate_id,
        candidate_index,
        provider_id = %candidate.provider_id,
        endpoint_id = %candidate.endpoint_id,
        key_id = %candidate.key_id,
        provider_type = %transport.provider.provider_type,
        client_api_format = spec_metadata.api_format,
        provider_api_format = %provider_api_format,
        execution_strategy = execution_strategy.as_str(),
        conversion_mode = conversion_mode.as_str(),
        base_url = %transport.endpoint.base_url,
        custom_path = ?transport.endpoint.custom_path,
        request_path = %parts.uri.path(),
        request_query = ?parts.uri.query(),
        mapped_model = %mapped_model,
        upstream_url = %upstream_url,
        upstream_is_stream,
        "gateway resolved local openai responses upstream url"
    );

    Ok(Some(LocalOpenAiResponsesCandidatePayloadParts {
        auth_header: resolved_headers.auth_header,
        auth_value: resolved_headers.auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        is_antigravity: false,
        envelope_name: None,
        upstream_is_stream,
        transport: Arc::clone(&transport),
        transport_profile,
        image_request_summary: None,
        request_redacted: redaction.redacted,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn build_antigravity_openai_responses_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalOpenAiResponsesDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    client_api_format: &str,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    gemini_request_body: Value,
    upstream_is_stream: bool,
    request_redacted: bool,
) -> Option<LocalOpenAiResponsesCandidatePayloadParts> {
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
            same_format: api_format_alias_matches(provider_api_format, client_api_format),
        })
        .await
        {
            Ok(resolved) => resolved,
            Err(AntigravityV1InternalRequestError::TransportUnsupported) => {
                mark_skipped_local_openai_responses_candidate(
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
                mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "provider_request_body_build_failed",
                    CandidateFailureDiagnostic::envelope_build_failed(
                        client_api_format,
                        provider_api_format,
                        "openai_responses_antigravity_envelope",
                    ),
                )
                .await;
                return None;
            }
            Err(AntigravityV1InternalRequestError::UpstreamUrlUnavailable) => {
                mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "upstream_url_missing",
                    CandidateFailureDiagnostic::upstream_url_missing(
                        client_api_format,
                        provider_api_format,
                        "openai_responses_antigravity_url",
                    ),
                )
                .await;
                return None;
            }
            Err(AntigravityV1InternalRequestError::HeaderRulesApplyFailed) => {
                mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "transport_header_rules_apply_failed",
                    CandidateFailureDiagnostic::header_rules_apply_failed(
                        client_api_format,
                        provider_api_format,
                        "openai_responses_antigravity_headers",
                    ),
                )
                .await;
                return None;
            }
        };
    let mut provider_request_headers = resolved.headers.headers;
    apply_local_auth_config_header_overrides(
        &mut provider_request_headers,
        resolved.transport.key.decrypted_auth_config.as_deref(),
    );
    apply_codex_openai_special_headers(
        &mut provider_request_headers,
        &resolved.body,
        effective_headers,
        resolved.transport.provider.provider_type.as_str(),
        provider_api_format,
        Some(trace_id),
        resolved.transport.key.decrypted_auth_config.as_deref(),
    );
    provider_request_headers.insert("accept".to_string(), "text/event-stream".to_string());
    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        request_redacted,
    );

    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats(client_api_format, provider_api_format);

    Some(LocalOpenAiResponsesCandidatePayloadParts {
        auth_header: resolved.headers.auth_header,
        auth_value: resolved.headers.auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body: resolved.body,
        provider_request_headers,
        upstream_url: resolved.upstream_url,
        execution_strategy,
        conversion_mode,
        is_antigravity: true,
        envelope_name: Some(ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME),
        upstream_is_stream,
        transport: Arc::clone(&resolved.transport),
        transport_profile: crate::ai_serving::transport::resolve_transport_profile(
            &resolved.transport,
        ),
        image_request_summary: None,
        request_redacted,
    })
}

#[allow(clippy::too_many_arguments)]
async fn build_gemini_cli_openai_responses_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalOpenAiResponsesDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    client_api_format: &str,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    gemini_request_body: Value,
    upstream_is_stream: bool,
    request_redacted: bool,
) -> Option<LocalOpenAiResponsesCandidatePayloadParts> {
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
                mark_skipped_local_openai_responses_candidate(
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
                mark_skipped_local_openai_responses_candidate_with_extra_data(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "provider_request_body_build_failed",
                    request_body_build_failure_extra_data(
                        original_body_json,
                        client_api_format,
                        provider_api_format,
                    ),
                )
                .await;
                return None;
            }
            Err(GeminiCliV1InternalRequestError::UpstreamUrlUnavailable) => {
                mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "upstream_url_missing",
                    CandidateFailureDiagnostic::upstream_url_missing(
                        client_api_format,
                        provider_api_format,
                        "openai_responses_gemini_cli_url",
                    ),
                )
                .await;
                return None;
            }
            Err(GeminiCliV1InternalRequestError::HeaderRulesApplyFailed) => {
                mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "transport_header_rules_apply_failed",
                    CandidateFailureDiagnostic::header_rules_apply_failed(
                        client_api_format,
                        provider_api_format,
                        "openai_responses_gemini_cli_headers",
                    ),
                )
                .await;
                return None;
            }
        };
    let mut provider_request_headers = resolved.headers.headers;
    apply_local_auth_config_header_overrides(
        &mut provider_request_headers,
        resolved.transport.key.decrypted_auth_config.as_deref(),
    );
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

    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats(client_api_format, provider_api_format);

    Some(LocalOpenAiResponsesCandidatePayloadParts {
        auth_header: resolved.headers.auth_header,
        auth_value: resolved.headers.auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body: resolved.body,
        provider_request_headers,
        upstream_url: resolved.upstream_url,
        execution_strategy,
        conversion_mode,
        is_antigravity: false,
        envelope_name: Some(GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME),
        upstream_is_stream,
        transport: resolved.transport,
        transport_profile: None,
        image_request_summary: None,
        request_redacted,
    })
}

#[allow(clippy::too_many_arguments)]
async fn build_windsurf_openai_responses_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalOpenAiResponsesDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    client_api_format: &str,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    openai_chat_request_body: Value,
    upstream_is_stream: bool,
    request_redacted: bool,
) -> Option<LocalOpenAiResponsesCandidatePayloadParts> {
    let candidate = &eligible.candidate;
    let effective_headers = input.effective_headers(&parts.headers);
    let provider_request_body = match build_windsurf_cascade_request_body(
        &openai_chat_request_body,
        &mapped_model,
        &auth_value,
        transport.endpoint.body_rules.as_ref(),
        Some(effective_headers),
        upstream_is_stream,
    ) {
        Some(body) => body,
        None => {
            mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                CandidateFailureDiagnostic::envelope_build_failed(
                    client_api_format,
                    provider_api_format,
                    "openai_responses_windsurf_cascade",
                ),
            )
            .await;
            return None;
        }
    };
    let upstream_url = match build_windsurf_cascade_upstream_url(
        transport.endpoint.base_url.as_str(),
        parts.uri.query(),
    ) {
        Some(url) => url,
        None => {
            mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "upstream_url_missing",
                CandidateFailureDiagnostic::upstream_url_missing(
                    client_api_format,
                    provider_api_format,
                    "openai_responses_windsurf_url",
                ),
            )
            .await;
            return None;
        }
    };
    let mut provider_request_headers = match build_windsurf_cascade_headers(
        effective_headers,
        &provider_request_body,
        original_body_json,
        transport.endpoint.header_rules.as_ref(),
        &auth_header,
        &auth_value,
        upstream_is_stream,
    ) {
        Some(headers) => headers,
        None => {
            mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    client_api_format,
                    provider_api_format,
                    "openai_responses_windsurf_headers",
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
    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats(client_api_format, provider_api_format);

    Some(LocalOpenAiResponsesCandidatePayloadParts {
        auth_header,
        auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        is_antigravity: false,
        envelope_name: Some(WINDSURF_ENVELOPE_NAME),
        upstream_is_stream,
        transport: Arc::clone(transport),
        transport_profile: None,
        image_request_summary: None,
        request_redacted,
    })
}

fn api_format_alias_matches(left: &str, right: &str) -> bool {
    crate::ai_serving::api_format_alias_matches(left, right)
}

#[allow(clippy::too_many_arguments)]
async fn resolve_openai_responses_to_openai_image_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiResponsesDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    spec: LocalOpenAiResponsesSpec,
) -> Option<LocalOpenAiResponsesCandidatePayloadParts> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let candidate = &eligible.candidate;
    let transport = &eligible.transport;
    let provider_api_format = "openai:image";
    if let Some(skip_reason) =
        openai_image_transport_unsupported_reason(transport, provider_api_format)
    {
        mark_skipped_local_openai_responses_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            skip_reason,
        )
        .await;
        return None;
    }

    let prepared_candidate = match prepare_header_authenticated_candidate(
        PlannerAppState::new(state),
        transport,
        candidate,
        resolve_openai_image_auth(transport),
        OauthPreparationContext {
            trace_id,
            api_format: provider_api_format,
            operation: "openai_responses_image_bridge",
        },
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(skip_reason) => {
            mark_skipped_local_openai_responses_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                skip_reason,
            )
            .await;
            return None;
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
    let upstream_is_stream = resolve_upstream_is_stream_for_provider(
        transport.endpoint.config.as_ref(),
        transport.provider.provider_type.as_str(),
        provider_api_format,
        spec_metadata.require_streaming && candidate.supports_streaming,
        false,
    );
    let Some((mut provider_request_body, image_request_summary)) = (if is_chatgpt_web {
        build_chatgpt_web_image_provider_body_from_openai_responses_body(
            body_json,
            &input.requested_model,
        )
    } else {
        build_openai_image_provider_body_from_openai_responses_body(
            body_json,
            &prepared_candidate.mapped_model,
            upstream_is_stream,
        )
    }) else {
        mark_skipped_local_openai_responses_candidate_with_extra_data(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "provider_request_body_build_failed",
            request_body_build_failure_extra_data(
                body_json,
                spec_metadata.api_format,
                provider_api_format,
            ),
        )
        .await;
        return None;
    };
    let operation = openai_image_operation_from_summary(&image_request_summary)?;
    if !is_chatgpt_web {
        provider_request_body = project_openai_image_api_request_body(
            &provider_request_body,
            &prepared_candidate.mapped_model,
            operation,
            crate::image_capabilities::openai_image_provider_max_generation_count_for_model(
                transport.provider.provider_type.as_str(),
                Some(prepared_candidate.mapped_model.as_str()),
            ),
        )?;
    }
    if is_codex {
        provider_request_body =
            project_codex_openai_image_api_request_body(&provider_request_body, operation)?;
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
        mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "transport_header_rules_apply_failed",
            CandidateFailureDiagnostic::header_rules_apply_failed(
                spec_metadata.api_format,
                provider_api_format,
                "openai_responses_image_bridge_headers",
            ),
        )
        .await;
        return None;
    };
    if is_chatgpt_web {
        provider_request_headers.insert("x-aether-chatgpt-web-image".to_string(), "1".to_string());
    } else {
        apply_local_auth_config_header_overrides(
            &mut provider_request_headers,
            transport.key.decrypted_auth_config.as_deref(),
        );
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
        ai_local_execution_contract_for_formats(spec_metadata.api_format, provider_api_format);

    Some(LocalOpenAiResponsesCandidatePayloadParts {
        auth_header: prepared_candidate.auth_header,
        auth_value: prepared_candidate.auth_value,
        mapped_model: prepared_candidate.mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        is_antigravity: false,
        envelope_name: None,
        upstream_is_stream,
        transport: Arc::clone(transport),
        transport_profile: None,
        image_request_summary: Some(image_request_summary),
        request_redacted: false,
    })
}

fn build_openai_image_provider_body_from_openai_responses_body(
    body_json: &Value,
    requested_model: &str,
    upstream_is_stream: bool,
) -> Option<(Value, Value)> {
    let object = body_json.as_object()?;
    let tool = openai_responses_image_generation_tool(object);
    let (prompt, images) = collect_openai_responses_image_prompt_and_images(object.get("input"))?;
    let operation = if images.is_empty() {
        OpenAiImageOperation::Generate
    } else {
        OpenAiImageOperation::Edit
    };
    if let Some(action) = tool
        .as_ref()
        .and_then(|tool| tool.get("action"))
        .and_then(Value::as_str)
    {
        let expected = operation.as_str();
        if !action.trim().eq_ignore_ascii_case(expected) {
            return None;
        }
    }

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
    for key in [
        "background",
        "quality",
        "size",
        "output_format",
        "output_compression",
        "moderation",
        "input_fidelity",
        "partial_images",
        "n",
        "user",
    ] {
        if let Some(value) = tool
            .as_ref()
            .and_then(|tool| tool.get(key))
            .or_else(|| object.get(key))
        {
            body.insert(key.to_string(), value.clone());
        }
    }
    if operation == OpenAiImageOperation::Edit {
        let image_urls = openai_image_inputs_as_api_urls(&images);
        if image_urls.len() != images.len() {
            return None;
        }
        body.insert("images".to_string(), Value::Array(image_urls));
    }
    if upstream_is_stream {
        body.insert("stream".to_string(), Value::Bool(true));
    }
    let mut summary = serde_json::Map::new();
    summary.insert(
        "operation".to_string(),
        Value::String(operation.as_str().to_string()),
    );
    for key in ["output_format", "partial_images", "size", "quality"] {
        let tool_value = tool.as_ref().and_then(|tool| tool.get(key));
        if let Some(value) = tool_value.or_else(|| object.get(key)) {
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

fn openai_responses_image_generation_tool(
    object: &serde_json::Map<String, Value>,
) -> Option<serde_json::Map<String, Value>> {
    object
        .get("tools")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_object)
        .find(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("image_generation"))
        })
        .cloned()
}

fn build_chatgpt_web_image_provider_body_from_openai_responses_body(
    body_json: &Value,
    requested_model: &str,
) -> Option<(Value, Value)> {
    let object = body_json.as_object()?;
    let (prompt, images) = collect_openai_responses_image_prompt_and_images(object.get("input"))?;
    let operation = if images.is_empty() {
        "generate"
    } else {
        "edit"
    };
    let tool = openai_responses_image_generation_tool(object);
    let size = image_option_string(tool.as_ref(), object, "size").unwrap_or("1024x1024");
    let output_format =
        image_option_string(tool.as_ref(), object, "output_format").unwrap_or("png");
    let quality = image_option_string(tool.as_ref(), object, "quality").unwrap_or("medium");
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| requested_model.trim());
    let web_model = image_option_string(tool.as_ref(), object, "web_model")
        .or_else(|| image_option_string(tool.as_ref(), object, "model"))
        .unwrap_or("gpt-5-5-thinking");
    let image_urls = openai_image_inputs_as_urls(&images);

    let mut body = json!({
        "operation": operation,
        "model": if model.is_empty() { "gpt-image-2" } else { model },
        "web_model": web_model,
        "prompt": prompt,
        "size": size,
        "ratio": chatgpt_web_ratio_for_size(size),
        "quality": quality,
        "output_format": output_format,
        "images": image_urls,
    });
    if let Some(partial_images) = tool
        .as_ref()
        .and_then(|tool| tool.get("partial_images"))
        .or_else(|| object.get("partial_images"))
        .cloned()
    {
        body.as_object_mut()?
            .insert("partial_images".to_string(), partial_images);
    }
    let mut summary = json!({
        "operation": operation,
        "output_format": output_format,
        "size": size,
        "quality": quality,
    });
    if let Some(partial_images) = tool
        .as_ref()
        .and_then(|tool| tool.get("partial_images"))
        .or_else(|| object.get("partial_images"))
        .cloned()
    {
        summary
            .as_object_mut()?
            .insert("partial_images".to_string(), partial_images);
    }
    Some((body, summary))
}

fn image_option_string<'a>(
    tool: Option<&'a serde_json::Map<String, Value>>,
    object: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Option<&'a str> {
    tool.and_then(|tool| tool.get(key))
        .or_else(|| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn collect_openai_responses_image_prompt_and_images(
    input: Option<&Value>,
) -> Option<(String, Vec<Value>)> {
    let input = input?;
    let mut prompt_parts = Vec::new();
    let mut images = Vec::new();
    collect_openai_responses_image_input(input, &mut prompt_parts, &mut images);
    let prompt = prompt_parts.join("\n").trim().to_string();
    (!prompt.is_empty()).then_some((prompt, images))
}

fn collect_openai_responses_image_input(
    value: &Value,
    prompt_parts: &mut Vec<String>,
    images: &mut Vec<Value>,
) {
    match value {
        Value::String(text) => {
            let text = text.trim();
            if !text.is_empty() {
                prompt_parts.push(text.to_string());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_openai_responses_image_input(item, prompt_parts, images);
            }
        }
        Value::Object(object) => {
            let item_type = object
                .get("type")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if matches!(item_type, "input_text" | "text") {
                if let Some(text) = object
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    prompt_parts.push(text.to_string());
                }
            } else if matches!(item_type, "input_image" | "image_url") {
                collect_openai_image_input_object(object, images);
            }
            if let Some(content) = object.get("content") {
                collect_openai_responses_image_input(content, prompt_parts, images);
            }
        }
        _ => {}
    }
}

fn collect_openai_image_input_object(
    object: &serde_json::Map<String, Value>,
    images: &mut Vec<Value>,
) {
    if let Some(url) = object
        .get("image_url")
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("url").and_then(Value::as_str))
        })
        .or_else(|| object.get("url").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        images.push(json!({
            "type": "input_image",
            "image_url": url,
        }));
    } else if let Some(file_id) = object
        .get("file_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        images.push(json!({
            "type": "input_image",
            "file_id": file_id,
        }));
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
async fn build_kiro_openai_responses_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalOpenAiResponsesDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    client_api_format: &str,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    claude_request_body: Value,
    upstream_is_stream: bool,
    needs_bidirectional_conversion: bool,
    kiro_auth: &KiroRequestAuth,
    request_redacted: bool,
) -> Result<Option<LocalOpenAiResponsesCandidatePayloadParts>, GatewayError> {
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
            mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "provider_request_body_build_failed",
                CandidateFailureDiagnostic::envelope_build_failed(
                    client_api_format,
                    provider_api_format,
                    "openai_responses_kiro_envelope",
                ),
            )
            .await;
            return Ok(None);
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
            mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "upstream_url_missing",
                CandidateFailureDiagnostic::upstream_url_missing(
                    client_api_format,
                    provider_api_format,
                    "openai_responses_kiro_url",
                ),
            )
            .await;
            return Ok(None);
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
            mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    client_api_format,
                    provider_api_format,
                    "openai_responses_kiro_headers",
                ),
            )
            .await;
            return Ok(None);
        }
    };
    let (execution_strategy, conversion_mode) =
        ai_local_execution_contract_for_formats(client_api_format, provider_api_format);

    debug!(
        event_name = "local_openai_responses_kiro_upstream_url_resolved",
        log_type = "debug",
        trace_id = %trace_id,
        candidate_id = %candidate_id,
        candidate_index,
        provider_id = %candidate.provider_id,
        endpoint_id = %candidate.endpoint_id,
        key_id = %candidate.key_id,
        provider_type = %transport.provider.provider_type,
        client_api_format = client_api_format,
        provider_api_format = %provider_api_format,
        execution_strategy = execution_strategy.as_str(),
        conversion_mode = conversion_mode.as_str(),
        upstream_url = %upstream_url,
        upstream_is_stream,
        "gateway resolved local openai responses kiro upstream url"
    );

    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        request_redacted,
    );

    Ok(Some(LocalOpenAiResponsesCandidatePayloadParts {
        auth_header,
        auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        is_antigravity: false,
        envelope_name: Some(KIRO_ENVELOPE_NAME),
        upstream_is_stream,
        transport: Arc::clone(transport),
        transport_profile: None,
        image_request_summary: None,
        request_redacted,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_responses_image_bridge_builds_images_api_body() {
        let body_json = json!({
            "model": "gpt-image-2",
            "input": "Draw a glass city",
            "tools": [
                {
                    "type": "image_generation",
                    "size": "1024x1024",
                    "output_format": "png"
                }
            ],
            "tool_choice": {
                "type": "image_generation"
            }
        });

        let (provider_body, summary) = build_openai_image_provider_body_from_openai_responses_body(
            &body_json,
            "gpt-image-2",
            true,
        )
        .expect("responses image body should convert");

        assert_eq!(provider_body["model"], "gpt-image-2");
        assert_eq!(provider_body["prompt"], "Draw a glass city");
        assert_eq!(provider_body["size"], "1024x1024");
        assert_eq!(provider_body["output_format"], "png");
        assert_eq!(provider_body["stream"], true);
        assert!(provider_body.get("tools").is_none());
        assert!(provider_body.get("input").is_none());
        assert_eq!(summary["operation"], "generate");
        assert_eq!(summary["output_format"], "png");

        let (sync_provider_body, _) = build_openai_image_provider_body_from_openai_responses_body(
            &body_json,
            "gpt-image-2",
            false,
        )
        .expect("responses image body should convert for a sync upstream");
        assert!(sync_provider_body.get("stream").is_none());
    }

    #[test]
    fn responses_image_bridge_uses_the_shared_mapped_model_projection() {
        let body_json = json!({
            "model": "image-alias",
            "input": "Draw a glass city",
            "tools": [{
                "type": "image_generation",
                "quality": "high",
                "n": 2
            }],
            "tool_choice": {"type": "image_generation"}
        });
        let (body, _) = build_openai_image_provider_body_from_openai_responses_body(
            &body_json, "dall-e-3", false,
        )
        .expect("Responses image body should convert before provider projection");

        assert!(project_openai_image_api_request_body(
            &body,
            "dall-e-3",
            OpenAiImageOperation::Generate,
            1,
        )
        .is_none());
        let single = json!({
            "model": "dall-e-3",
            "prompt": "Draw a glass city",
            "quality": "high",
            "n": 1
        });
        let projected = project_openai_image_api_request_body(
            &single,
            "dall-e-3",
            OpenAiImageOperation::Generate,
            1,
        )
        .expect("DALL-E 3 single image request should project");
        assert_eq!(projected["quality"], "hd");

        let codex_overflow = json!({
            "model": "gpt-image-2",
            "prompt": "Draw a glass city",
            "n": 11
        });
        assert!(project_codex_openai_image_api_request_body(
            &codex_overflow,
            OpenAiImageOperation::Generate
        )
        .is_none());
    }

    #[test]
    fn chatgpt_web_responses_image_body_preserves_usage_options() {
        let body_json = json!({
            "model": "gpt-image-2",
            "input": "Draw a glass city",
            "tools": [
                {
                    "type": "image_generation",
                    "size": "1024x1024",
                    "quality": "high",
                    "output_format": "png",
                    "partial_images": 2
                }
            ],
            "tool_choice": {
                "type": "image_generation"
            }
        });

        let (provider_body, summary) =
            build_chatgpt_web_image_provider_body_from_openai_responses_body(
                &body_json,
                "gpt-image-2",
            )
            .expect("responses image body should convert");

        assert_eq!(provider_body["quality"], "high");
        assert_eq!(provider_body["partial_images"], 2);
        assert_eq!(summary["quality"], "high");
        assert_eq!(summary["partial_images"], 2);
    }
}
