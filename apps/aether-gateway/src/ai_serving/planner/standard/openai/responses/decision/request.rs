use std::collections::BTreeMap;
use std::sync::Arc;

use aether_contracts::ResolvedTransportProfile;
use serde_json::{json, Value};
use tracing::debug;

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
    apply_codex_openai_responses_special_body_edits, apply_codex_openai_responses_special_headers,
    apply_deepseek_tool_call_thinking_compat, build_cross_format_openai_responses_request_body,
    build_cross_format_openai_responses_upstream_url, build_local_openai_responses_request_body,
    build_local_openai_responses_upstream_url, request_body_build_failure_extra_data,
};
use crate::ai_serving::transport::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, is_antigravity_provider_transport,
    AntigravityEnvelopeRequestType, AntigravityRequestEnvelopeSupport,
    AntigravityRequestSideSupport,
};
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
    build_grok_browser_headers, build_grok_upstream_url, build_kiro_cross_format_upstream_url,
    build_openai_image_headers, build_openai_image_upstream_url,
    build_standard_provider_request_headers, build_windsurf_cascade_headers,
    build_windsurf_cascade_request_body, build_windsurf_cascade_upstream_url,
    is_gemini_cli_provider_transport, is_windsurf_provider_transport,
    local_standard_transport_unsupported_reason_with_network,
    local_windsurf_request_transport_unsupported_reason_with_network,
    openai_image_transport_unsupported_reason, resolve_openai_image_auth, GrokHeaderInput,
    ProviderOpenAiImageHeadersInput, StandardProviderRequestHeadersInput,
    GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME, GROK_CHAT_PATH, WINDSURF_ENVELOPE_NAME,
};
use crate::ai_serving::{
    ai_local_execution_contract_for_formats, request_conversion_direct_auth,
    request_conversion_kind, CandidateFailureDiagnostic, GatewayProviderTransportSnapshot,
    LocalResolvedOAuthRequestAuth, PlannerAppState,
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

const ANTIGRAVITY_ENVELOPE_NAME: &str = "antigravity:v1internal";

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
    let transport = &eligible.transport;
    let transport_profile = crate::ai_serving::transport::resolve_transport_profile(transport);
    let is_antigravity = is_antigravity_provider_transport(transport);
    let is_gemini_cli = is_gemini_cli_provider_transport(transport);
    let is_kiro_claude_cli = is_kiro_claude_messages_transport(transport, provider_api_format);
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
        provider_api_format == "openai:chat" && is_windsurf_provider_transport(transport);

    let same_format = api_format_alias_matches(provider_api_format, &client_api_format);
    let conversion_kind = request_conversion_kind(spec_metadata.api_format, provider_api_format);
    let transport_unsupported_reason =
        if is_grok && is_grok_text_provider_api_format(provider_api_format) {
            None
        } else if same_format && is_kiro_claude_cli {
            local_kiro_request_transport_unsupported_reason_with_network(transport)
        } else if same_format {
            local_standard_transport_unsupported_reason_with_network(transport, provider_api_format)
        } else if is_windsurf_cascade {
            local_windsurf_request_transport_unsupported_reason_with_network(transport)
        } else {
            match conversion_kind {
                Some(_)
                    if (is_antigravity || is_gemini_cli)
                        && normalized_provider_api_format == "gemini:generate_content" =>
                {
                    None
                }
                Some(kind) => crate::ai_serving::request_conversion_transport_unsupported_reason(
                    transport, kind,
                ),
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
            transport,
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
        crate::ai_serving::transport::resolve_grok_session_auth(transport)
    } else if kiro_auth.is_some() {
        None
    } else if same_format {
        match crate::ai_serving::normalize_api_format_alias(provider_api_format).as_str() {
            "gemini:generate_content" => resolve_local_gemini_auth(transport),
            "claude:messages" => resolve_local_standard_auth(transport),
            "openai:responses" | "openai:responses:compact" => {
                resolve_local_openai_bearer_auth(transport)
            }
            _ => None,
        }
    } else {
        conversion_kind.and_then(|kind| request_conversion_direct_auth(transport, kind))
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
            transport,
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
    let enable_model_directives =
        crate::system_features::reasoning_model_directive_enabled_for_api_format_and_model(
            state,
            provider_api_format,
            Some(&input.requested_model),
        )
        .await;
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
    let Some(mut base_provider_request_body) =
        (if is_grok && is_grok_text_provider_api_format(provider_api_format) {
            build_local_openai_responses_request_body(
                body_json,
                &mapped_model,
                upstream_is_stream,
                force_body_stream_field,
                transport.provider.provider_type.as_str(),
                spec_metadata.api_format,
                transport.endpoint.body_rules.as_ref(),
                Some(input.auth_context.api_key_id.as_str()),
                effective_headers,
                enable_model_directives,
            )
        } else if needs_bidirectional_conversion {
            build_cross_format_openai_responses_request_body(
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
                Some(input.auth_context.api_key_id.as_str()),
                effective_headers,
                enable_model_directives,
            )
        } else {
            build_local_openai_responses_request_body(
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
                Some(input.auth_context.api_key_id.as_str()),
                effective_headers,
                enable_model_directives,
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
            request_body_build_failure_extra_data(
                body_json,
                spec_metadata.api_format,
                provider_api_format,
            ),
        )
        .await;
        return Ok(None);
    };
    if let Some(mapping) =
        crate::system_features::reasoning_model_directive_mapping_for_api_format_and_model(
            state,
            provider_api_format,
            Some(&input.requested_model),
        )
        .await
    {
        crate::ai_serving::apply_model_directive_mapping_patch(
            &mut base_provider_request_body,
            &mapping,
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
    let antigravity_auth = if is_antigravity {
        match classify_local_antigravity_request_support(
            transport,
            &base_provider_request_body,
            AntigravityEnvelopeRequestType::Agent,
        ) {
            AntigravityRequestSideSupport::Supported(spec) => Some(spec.auth),
            AntigravityRequestSideSupport::Unsupported(_) => {
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
                return Ok(None);
            }
        }
    } else {
        None
    };
    let provider_request_body = if let Some(antigravity_auth) = antigravity_auth.as_ref() {
        match build_antigravity_safe_v1internal_request(
            antigravity_auth,
            trace_id,
            &mapped_model,
            &base_provider_request_body,
            AntigravityEnvelopeRequestType::Agent,
        ) {
            AntigravityRequestEnvelopeSupport::Supported(envelope) => envelope,
            AntigravityRequestEnvelopeSupport::Unsupported(_) => {
                mark_skipped_local_openai_responses_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "provider_request_body_build_failed",
                    CandidateFailureDiagnostic::envelope_build_failed(
                        spec_metadata.api_format,
                        provider_api_format,
                        "openai_responses_antigravity_envelope",
                    ),
                )
                .await;
                return Ok(None);
            }
        }
    } else {
        base_provider_request_body
    };

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
            transport,
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
            transport,
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
        && is_gemini_cli_provider_transport(transport)
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
            transport,
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
        Some(build_grok_upstream_url(transport, GROK_CHAT_PATH))
    } else if needs_bidirectional_conversion {
        build_cross_format_openai_responses_upstream_url(
            parts,
            transport,
            &mapped_model,
            spec_metadata.api_format,
            provider_api_format,
            upstream_is_stream,
        )
    } else {
        build_local_openai_responses_upstream_url(
            parts,
            transport,
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
    let extra_headers = antigravity_auth
        .as_ref()
        .map(build_antigravity_static_identity_headers)
        .unwrap_or_default();
    let resolved_headers = if is_grok && is_grok_text_provider_api_format(provider_api_format) {
        let Some(headers) = build_grok_browser_headers(GrokHeaderInput {
            transport,
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
                transport,
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
        apply_codex_openai_responses_special_headers(
            &mut provider_request_headers,
            &provider_request_body,
            effective_headers,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            Some(trace_id),
            transport.key.decrypted_auth_config.as_deref(),
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
        is_antigravity: is_antigravity
            || antigravity_auth.is_some() && ANTIGRAVITY_ENVELOPE_NAME == "antigravity:v1internal",
        envelope_name: if is_antigravity || antigravity_auth.is_some() {
            Some(ANTIGRAVITY_ENVELOPE_NAME)
        } else {
            None
        },
        upstream_is_stream,
        transport: Arc::clone(transport),
        transport_profile,
        image_request_summary: None,
        request_redacted: redaction.redacted,
    }))
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
    apply_codex_openai_responses_special_headers(
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
    let upstream_is_stream = resolve_upstream_is_stream_for_provider(
        transport.endpoint.config.as_ref(),
        transport.provider.provider_type.as_str(),
        provider_api_format,
        spec_metadata.require_streaming,
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
            &input.requested_model,
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

    if !is_chatgpt_web {
        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            transport.endpoint.body_rules.as_ref(),
            Some(candidate.key_id.as_str()),
        );
    }

    let upstream_url = if is_chatgpt_web {
        chatgpt_web_image_internal_url(&transport.endpoint.base_url)
    } else {
        build_openai_image_upstream_url(
            transport,
            Some("/v1/images/generations"),
            parts.uri.query(),
        )
    };
    let Some(mut provider_request_headers) =
        build_openai_image_headers(ProviderOpenAiImageHeadersInput {
            headers: &parts.headers,
            auth_header: &prepared_candidate.auth_header,
            auth_value: &prepared_candidate.auth_value,
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
        apply_codex_openai_responses_special_headers(
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
    let input = object.get("input")?.clone();
    let tool = openai_responses_image_generation_tool(object);

    let mut body = serde_json::Map::new();
    body.insert("input".to_string(), input);
    if let Some(model) = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let requested_model = requested_model.trim();
            (!requested_model.is_empty()).then_some(requested_model)
        })
    {
        body.insert("model".to_string(), Value::String(model.to_string()));
    }
    for key in [
        "user",
        "metadata",
        "include",
        "parallel_tool_calls",
        "store",
    ] {
        if let Some(value) = object.get(key) {
            body.insert(key.to_string(), value.clone());
        }
    }
    if upstream_is_stream {
        body.insert("stream".to_string(), Value::Bool(true));
    } else if let Some(value) = object.get("stream") {
        body.insert("stream".to_string(), value.clone());
    }
    let image_tool = tool.clone().unwrap_or_else(|| {
        let mut tool = serde_json::Map::new();
        tool.insert(
            "type".to_string(),
            Value::String("image_generation".to_string()),
        );
        tool
    });
    body.insert(
        "tools".to_string(),
        Value::Array(vec![Value::Object(image_tool)]),
    );

    let mut summary = serde_json::Map::new();
    summary.insert(
        "operation".to_string(),
        tool.as_ref()
            .and_then(|tool| tool.get("action"))
            .cloned()
            .unwrap_or_else(|| json!("generate")),
    );
    for key in ["output_format", "partial_images", "size", "quality"] {
        let tool_value = tool.as_ref().and_then(|tool| tool.get(key));
        if let Some(value) = tool_value.or_else(|| object.get(key)) {
            summary.insert(key.to_string(), value.clone());
        }
    }

    Some((Value::Object(body), Value::Object(summary)))
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
    fn openai_responses_image_bridge_body_preserves_image_generation_tool() {
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

        assert_eq!(provider_body["tools"][0]["type"], "image_generation");
        assert_eq!(provider_body["tools"][0]["size"], "1024x1024");
        assert_eq!(provider_body["tools"][0]["output_format"], "png");
        assert_eq!(provider_body["model"], "gpt-image-2");
        assert_eq!(provider_body["input"], "Draw a glass city");
        assert_eq!(provider_body["stream"], true);
        assert_eq!(summary["operation"], "generate");
        assert_eq!(summary["output_format"], "png");
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
