use std::collections::BTreeMap;
use std::sync::Arc;

use aether_contracts::ResolvedTransportProfile;
use serde_json::Value;

use crate::ai_serving::planner::candidate_preparation::{
    prepare_header_authenticated_candidate, prepare_header_authenticated_candidate_from_auth,
    OauthPreparationContext,
};
use crate::ai_serving::planner::common::{
    endpoint_config_forces_body_stream_field, enforce_provider_body_stream_policy,
    request_requires_body_stream_field, resolve_upstream_is_stream_for_provider,
};
use crate::ai_serving::planner::spec_metadata::local_standard_spec_metadata;
use crate::ai_serving::planner::standard::{
    apply_codex_openai_responses_special_headers, request_body_build_failure_extra_data,
};
use crate::ai_serving::transport::kiro::{
    build_kiro_provider_headers, build_kiro_provider_request_body,
    is_kiro_claude_messages_transport, KiroProviderHeadersInput, KiroRequestAuth,
    KIRO_ENVELOPE_NAME,
};
use crate::ai_serving::transport::{
    build_grok_browser_headers, build_grok_upstream_url, build_kiro_cross_format_upstream_url,
    build_openai_image_headers, build_openai_image_upstream_url,
    build_standard_provider_request_headers, build_windsurf_cascade_headers,
    build_windsurf_cascade_request_body, build_windsurf_cascade_upstream_url,
    is_windsurf_provider_transport,
    local_windsurf_request_transport_unsupported_reason_with_network,
    openai_image_transport_unsupported_reason, resolve_grok_session_auth,
    resolve_openai_image_auth, GrokHeaderInput, ProviderOpenAiImageHeadersInput,
    StandardProviderRequestHeadersInput, GROK_CHAT_PATH, WINDSURF_ENVELOPE_NAME,
};
use crate::ai_serving::{
    build_openai_image_request_body_from_gemini_image_request, gemini_request_is_image_generation,
    CandidateFailureDiagnostic, GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth,
};
use crate::AppState;

use super::payload::{
    mark_skipped_local_standard_candidate, mark_skipped_local_standard_candidate_with_extra_data,
    mark_skipped_local_standard_candidate_with_failure_diagnostic,
};
use super::{LocalStandardCandidateAttempt, LocalStandardDecisionInput, LocalStandardSpec};

pub(crate) struct LocalStandardCandidatePayloadParts {
    pub(super) auth_header: String,
    pub(super) auth_value: String,
    pub(super) mapped_model: String,
    pub(super) provider_api_format: String,
    pub(super) provider_request_body: Value,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) upstream_url: String,
    pub(super) upstream_is_stream: bool,
    pub(super) envelope_name: Option<&'static str>,
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) transport_profile: Option<ResolvedTransportProfile>,
}

fn is_grok_text_provider_api_format(provider_api_format: &str) -> bool {
    matches!(
        crate::ai_serving::normalize_api_format_alias(provider_api_format).as_str(),
        "openai:chat" | "openai:responses" | "openai:responses:compact" | "claude:messages"
    )
}

fn provider_preserves_claude_thinking_signatures(provider_type: &str, base_url: &str) -> bool {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    let base_url = base_url.trim().to_ascii_lowercase();
    let is_bedrock_runtime_url = base_url.contains("bedrock-runtime")
        && (base_url.contains("amazonaws.com")
            || base_url.contains("amazonaws.com.cn")
            || base_url.contains("api.aws"));
    matches!(
        provider_type.as_str(),
        "anthropic" | "claude_code" | "bedrock" | "aws_bedrock" | "amazon_bedrock"
    ) || base_url.contains("api.anthropic.com")
        || is_bedrock_runtime_url
}

fn sanitize_claude_thinking_block(block: Value) -> (Option<Value>, bool) {
    let Some(object) = block.as_object() else {
        return (Some(block), false);
    };
    let block_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();

    match block_type {
        "thinking" => {
            let thinking_text = object
                .get("thinking")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if thinking_text.is_empty() {
                (None, true)
            } else {
                (
                    Some(serde_json::json!({
                        "type": "text",
                        "text": thinking_text,
                    })),
                    true,
                )
            }
        }
        "redacted_thinking" => (None, true),
        _ => (Some(block), false),
    }
}

fn sanitize_claude_message_content_for_non_native_thinking(content: &mut Value) -> bool {
    const OMITTED_THINKING_TEXT: &str = "Previous thinking omitted.";

    if content.is_object() {
        let original = std::mem::take(content);
        let (sanitized, changed) = sanitize_claude_thinking_block(original);
        if changed {
            *content = sanitized.unwrap_or_else(|| {
                serde_json::json!({
                    "type": "text",
                    "text": OMITTED_THINKING_TEXT,
                })
            });
        }
        return changed;
    }

    let Some(blocks) = content.as_array_mut() else {
        return false;
    };
    let original_blocks = std::mem::take(blocks);
    let mut changed = false;
    let mut sanitized_blocks = Vec::with_capacity(original_blocks.len());
    for block in original_blocks {
        let (sanitized, block_changed) = sanitize_claude_thinking_block(block);
        changed |= block_changed;
        if let Some(sanitized) = sanitized {
            sanitized_blocks.push(sanitized);
        }
    }
    if changed && sanitized_blocks.is_empty() {
        sanitized_blocks.push(serde_json::json!({
            "type": "text",
            "text": OMITTED_THINKING_TEXT,
        }));
    }
    *blocks = sanitized_blocks;
    changed
}

fn sanitize_claude_request_thinking_signatures_for_non_native(body_json: &mut Value) -> bool {
    body_json
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .map(|messages| {
            messages.iter_mut().fold(false, |changed, message| {
                let is_assistant = message
                    .get("role")
                    .and_then(Value::as_str)
                    .is_some_and(|role| role.trim().eq_ignore_ascii_case("assistant"));
                if !is_assistant {
                    return changed;
                }
                let content_changed = message
                    .get_mut("content")
                    .is_some_and(sanitize_claude_message_content_for_non_native_thinking);
                changed || content_changed
            })
        })
        .unwrap_or(false)
}

fn apply_non_native_claude_thinking_signature_compat(
    provider_request_body: &mut Value,
    provider_api_format: &str,
    transport: &GatewayProviderTransportSnapshot,
) {
    if crate::ai_serving::normalize_api_format_alias(provider_api_format) != "claude:messages" {
        return;
    }
    if provider_preserves_claude_thinking_signatures(
        transport.provider.provider_type.as_str(),
        transport.endpoint.base_url.as_str(),
    ) {
        return;
    }

    let _ = sanitize_claude_request_thinking_signatures_for_non_native(provider_request_body);
}

pub(crate) async fn resolve_local_standard_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalStandardDecisionInput,
    attempt: &LocalStandardCandidateAttempt,
    spec: LocalStandardSpec,
) -> Option<LocalStandardCandidatePayloadParts> {
    let spec_metadata = local_standard_spec_metadata(spec);
    let planner_state = crate::ai_serving::PlannerAppState::new(state);
    let candidate = &attempt.eligible.candidate;
    let transport = &attempt.eligible.transport;
    let transport_profile = crate::ai_serving::transport::resolve_transport_profile(transport);
    let provider_api_format = attempt.eligible.provider_api_format.as_str();
    let effective_headers = input.effective_headers(&parts.headers);
    let is_grok = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("grok");
    if spec_metadata.api_format == "gemini:generate_content"
        && provider_api_format == "openai:image"
        && gemini_request_is_image_generation(body_json)
    {
        return resolve_local_gemini_image_to_openai_image_candidate_payload_parts(
            state, parts, trace_id, body_json, input, attempt,
        )
        .await;
    }
    let is_kiro_claude_cli = is_kiro_claude_messages_transport(transport, provider_api_format);
    if is_grok && is_grok_text_provider_api_format(provider_api_format) {
        let prepared_candidate = match prepare_header_authenticated_candidate(
            planner_state,
            transport,
            candidate,
            resolve_grok_session_auth(transport),
            OauthPreparationContext {
                trace_id,
                api_format: provider_api_format,
                operation: "standard_family_grok_text_request",
            },
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(skip_reason) => {
                mark_skipped_local_standard_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    skip_reason,
                )
                .await;
                return None;
            }
        };

        let mut provider_request_body = body_json.clone();
        if let Some(object) = provider_request_body.as_object_mut() {
            object.insert(
                "model".to_string(),
                serde_json::Value::String(prepared_candidate.mapped_model.clone()),
            );
        }

        let upstream_is_stream = resolve_upstream_is_stream_for_provider(
            transport.endpoint.config.as_ref(),
            transport.provider.provider_type.as_str(),
            provider_api_format,
            spec_metadata.require_streaming,
            false,
        );
        let force_body_stream_field =
            endpoint_config_forces_body_stream_field(transport.endpoint.config.as_ref());
        enforce_provider_body_stream_policy(
            &mut provider_request_body,
            provider_api_format,
            upstream_is_stream,
            request_requires_body_stream_field(body_json, force_body_stream_field),
        );

        let upstream_url = build_grok_upstream_url(transport, GROK_CHAT_PATH);
        let Some(provider_request_headers) = build_grok_browser_headers(GrokHeaderInput {
            transport,
            transport_profile: transport_profile.as_ref(),
            request_headers: Some(effective_headers),
            content_type: "application/json",
            accept: "text/event-stream",
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
        }) else {
            mark_skipped_local_standard_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    spec_metadata.api_format,
                    provider_api_format,
                    "grok_standard_family_headers",
                ),
            )
            .await;
            return None;
        };

        return Some(LocalStandardCandidatePayloadParts {
            auth_header: prepared_candidate.auth_header,
            auth_value: prepared_candidate.auth_value,
            mapped_model: prepared_candidate.mapped_model,
            provider_api_format: provider_api_format.to_string(),
            provider_request_body,
            provider_request_headers,
            upstream_url,
            upstream_is_stream,
            envelope_name: None,
            transport: Arc::clone(transport),
            transport_profile,
        });
    }

    if !crate::ai_serving::request_pair_allowed_for_transport(
        transport,
        spec_metadata.api_format,
        provider_api_format,
    ) {
        return None;
    }

    let is_windsurf_cascade =
        provider_api_format == "openai:chat" && is_windsurf_provider_transport(transport);
    let transport_unsupported_reason = if is_windsurf_cascade {
        local_windsurf_request_transport_unsupported_reason_with_network(transport)
    } else {
        crate::ai_serving::request_pair_transport_unsupported_reason(
            transport,
            spec_metadata.api_format,
            provider_api_format,
        )
    };
    if let Some(skip_reason) = transport_unsupported_reason {
        mark_skipped_local_standard_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            skip_reason,
        )
        .await;
        return None;
    }

    let oauth_context = OauthPreparationContext {
        trace_id,
        api_format: provider_api_format,
        operation: "standard_family_cross_format",
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
                mark_skipped_local_standard_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    "transport_auth_unavailable",
                )
                .await;
                return None;
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
                mark_skipped_local_standard_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    skip_reason,
                )
                .await;
                return None;
            }
        }
    } else {
        match prepare_header_authenticated_candidate(
            planner_state,
            transport,
            candidate,
            crate::ai_serving::request_pair_direct_auth(transport, provider_api_format),
            oauth_context,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(skip_reason) => {
                mark_skipped_local_standard_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    skip_reason,
                )
                .await;
                return None;
            }
        }
    };

    let upstream_is_stream = resolve_upstream_is_stream_for_provider(
        transport.endpoint.config.as_ref(),
        transport.provider.provider_type.as_str(),
        provider_api_format,
        spec_metadata.require_streaming,
        is_kiro_claude_cli,
    );
    let force_body_stream_field =
        endpoint_config_forces_body_stream_field(transport.endpoint.config.as_ref());
    let enable_model_directives =
        crate::system_features::reasoning_model_directive_enabled_for_api_format_and_model(
            state,
            provider_api_format,
            Some(&input.requested_model),
        )
        .await;
    let mut provider_request_body =
        match crate::ai_serving::planner::standard::build_standard_request_body_with_model_directives_and_request_headers(
            body_json,
            spec_metadata.api_format,
            &prepared_candidate.mapped_model,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            parts.uri.path(),
            upstream_is_stream,
            if is_kiro_claude_cli || is_windsurf_cascade {
                None
            } else {
                transport.endpoint.body_rules.as_ref()
            },
            Some(input.auth_context.api_key_id.as_str()),
            Some(effective_headers),
            enable_model_directives,
        ) {
            Some(body) => body,
            None => {
                mark_skipped_local_standard_candidate_with_extra_data(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    "provider_request_body_build_failed",
                    request_body_build_failure_extra_data(
                        body_json,
                        spec_metadata.api_format,
                        provider_api_format,
                    ),
                )
                .await;
                return None;
            }
        };
    enforce_provider_body_stream_policy(
        &mut provider_request_body,
        provider_api_format,
        upstream_is_stream,
        request_requires_body_stream_field(body_json, force_body_stream_field),
    );
    if let Err(err) = apply_transport_request_body_semantics(
        &mut provider_request_body,
        transport,
        provider_api_format,
    ) {
        mark_skipped_local_standard_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_request_body_semantics_failed",
            CandidateFailureDiagnostic::request_conversion_failed(
                spec_metadata.api_format,
                provider_api_format,
                "standard_family_transport_body_semantics",
                err.to_string(),
            ),
        )
        .await;
        return None;
    }
    apply_non_native_claude_thinking_signature_compat(
        &mut provider_request_body,
        provider_api_format,
        transport,
    );
    if let Some(mapping) =
        crate::system_features::reasoning_model_directive_mapping_for_api_format_and_model(
            state,
            provider_api_format,
            Some(&input.requested_model),
        )
        .await
    {
        crate::ai_serving::apply_model_directive_mapping_patch(
            &mut provider_request_body,
            &mapping,
        );
        // Directive mapping is a deep-merge patch and may overwrite/add `stream`;
        // re-enforce stream-field policy afterward.
        enforce_provider_body_stream_policy(
            &mut provider_request_body,
            provider_api_format,
            upstream_is_stream,
            request_requires_body_stream_field(body_json, force_body_stream_field),
        );
        if let Err(err) = apply_transport_request_body_semantics(
            &mut provider_request_body,
            transport,
            provider_api_format,
        ) {
            mark_skipped_local_standard_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "transport_request_body_semantics_failed",
                CandidateFailureDiagnostic::request_conversion_failed(
                    spec_metadata.api_format,
                    provider_api_format,
                    "standard_family_transport_body_semantics_after_model_directives",
                    err.to_string(),
                ),
            )
            .await;
            return None;
        }
        apply_non_native_claude_thinking_signature_compat(
            &mut provider_request_body,
            provider_api_format,
            transport,
        );
    }

    if let Some(kiro_auth) = kiro_auth.as_ref() {
        return build_kiro_cross_format_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            attempt,
            transport,
            provider_api_format,
            prepared_candidate.mapped_model,
            prepared_candidate.auth_header,
            prepared_candidate.auth_value,
            provider_request_body,
            upstream_is_stream,
            kiro_auth,
        )
        .await;
    }
    if is_windsurf_cascade {
        return build_windsurf_cross_format_payload_parts(
            state,
            parts,
            trace_id,
            body_json,
            input,
            attempt,
            transport,
            provider_api_format,
            prepared_candidate.mapped_model,
            prepared_candidate.auth_header,
            prepared_candidate.auth_value,
            provider_request_body,
            upstream_is_stream,
        )
        .await;
    }

    let upstream_url = match crate::ai_serving::planner::standard::build_standard_upstream_url(
        parts,
        transport,
        &prepared_candidate.mapped_model,
        provider_api_format,
        upstream_is_stream,
        Some(&provider_request_body),
    ) {
        Some(url) => url,
        None => {
            mark_skipped_local_standard_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "upstream_url_missing",
                CandidateFailureDiagnostic::upstream_url_missing(
                    spec_metadata.api_format,
                    provider_api_format,
                    "standard_family_url",
                ),
            )
            .await;
            return None;
        }
    };
    let Some(resolved_headers) =
        build_standard_provider_request_headers(StandardProviderRequestHeadersInput {
            transport,
            provider_api_format,
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
        mark_skipped_local_standard_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_header_rules_apply_failed",
            CandidateFailureDiagnostic::header_rules_apply_failed(
                spec_metadata.api_format,
                provider_api_format,
                "standard_family_headers",
            ),
        )
        .await;
        return None;
    };
    let mut provider_request_headers = resolved_headers.headers;
    apply_codex_openai_responses_special_headers(
        &mut provider_request_headers,
        &provider_request_body,
        effective_headers,
        transport.provider.provider_type.as_str(),
        provider_api_format,
        Some(trace_id),
        transport.key.decrypted_auth_config.as_deref(),
    );

    Some(LocalStandardCandidatePayloadParts {
        auth_header: resolved_headers.auth_header,
        auth_value: resolved_headers.auth_value,
        mapped_model: prepared_candidate.mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        upstream_is_stream,
        envelope_name: None,
        transport: Arc::clone(transport),
        transport_profile: None,
    })
}

fn apply_transport_request_body_semantics(
    provider_request_body: &mut Value,
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> Result<(), crate::ai_serving::transport::TransportRequestBodySemanticsError> {
    crate::ai_serving::transport::apply_transport_request_body_semantics(
        provider_request_body,
        transport,
        provider_api_format,
    )
}

#[allow(clippy::too_many_arguments)]
async fn build_windsurf_cross_format_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalStandardDecisionInput,
    attempt: &LocalStandardCandidateAttempt,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    openai_chat_request_body: Value,
    upstream_is_stream: bool,
) -> Option<LocalStandardCandidatePayloadParts> {
    let candidate = &attempt.eligible.candidate;
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
            mark_skipped_local_standard_candidate_with_extra_data(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "provider_request_body_build_failed",
                request_body_build_failure_extra_data(
                    &openai_chat_request_body,
                    provider_api_format,
                    provider_api_format,
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
            mark_skipped_local_standard_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "upstream_url_missing",
                CandidateFailureDiagnostic::upstream_url_missing(
                    provider_api_format,
                    provider_api_format,
                    "standard_family_windsurf_url",
                ),
            )
            .await;
            return None;
        }
    };
    let provider_request_headers = match build_windsurf_cascade_headers(
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
            mark_skipped_local_standard_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    provider_api_format,
                    provider_api_format,
                    "standard_family_windsurf_headers",
                ),
            )
            .await;
            return None;
        }
    };

    Some(LocalStandardCandidatePayloadParts {
        auth_header,
        auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        upstream_is_stream,
        envelope_name: Some(WINDSURF_ENVELOPE_NAME),
        transport: Arc::clone(transport),
        transport_profile: None,
    })
}

async fn resolve_local_gemini_image_to_openai_image_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalStandardDecisionInput,
    attempt: &LocalStandardCandidateAttempt,
) -> Option<LocalStandardCandidatePayloadParts> {
    let client_api_format = "gemini:generate_content";
    let provider_api_format = "openai:image";
    let planner_state = crate::ai_serving::PlannerAppState::new(state);
    let candidate = &attempt.eligible.candidate;
    let transport = &attempt.eligible.transport;

    if let Some(skip_reason) =
        openai_image_transport_unsupported_reason(transport, provider_api_format)
    {
        mark_skipped_local_standard_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            skip_reason,
        )
        .await;
        return None;
    }

    let prepared_candidate = match prepare_header_authenticated_candidate(
        planner_state,
        transport,
        candidate,
        resolve_openai_image_auth(transport),
        OauthPreparationContext {
            trace_id,
            api_format: provider_api_format,
            operation: "gemini_image_to_openai_image_candidate_request",
        },
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(skip_reason) => {
            mark_skipped_local_standard_candidate(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                skip_reason,
            )
            .await;
            return None;
        }
    };

    let Some(converted) = build_openai_image_request_body_from_gemini_image_request(
        body_json,
        parts.uri.path(),
        &prepared_candidate.mapped_model,
    ) else {
        mark_skipped_local_standard_candidate_with_extra_data(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "provider_request_body_build_failed",
            request_body_build_failure_extra_data(
                body_json,
                client_api_format,
                provider_api_format,
            ),
        )
        .await;
        return None;
    };

    let upstream_is_stream = true;
    let upstream_url =
        build_openai_image_upstream_url(transport, Some("/v1/images/generations"), None);
    let effective_headers = input.effective_headers(&parts.headers);
    let Some(mut provider_request_headers) =
        build_openai_image_headers(ProviderOpenAiImageHeadersInput {
            headers: effective_headers,
            auth_header: &prepared_candidate.auth_header,
            auth_value: &prepared_candidate.auth_value,
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &converted.body_json,
            original_request_body: body_json,
        })
    else {
        mark_skipped_local_standard_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_header_rules_apply_failed",
            CandidateFailureDiagnostic::header_rules_apply_failed(
                client_api_format,
                provider_api_format,
                "gemini_image_to_openai_image_headers",
            ),
        )
        .await;
        return None;
    };
    apply_codex_openai_responses_special_headers(
        &mut provider_request_headers,
        &converted.body_json,
        effective_headers,
        transport.provider.provider_type.as_str(),
        provider_api_format,
        Some(trace_id),
        transport.key.decrypted_auth_config.as_deref(),
    );

    Some(LocalStandardCandidatePayloadParts {
        auth_header: prepared_candidate.auth_header,
        auth_value: prepared_candidate.auth_value,
        mapped_model: converted.mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body: converted.body_json,
        provider_request_headers,
        upstream_url,
        upstream_is_stream,
        envelope_name: None,
        transport: Arc::clone(transport),
        transport_profile: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn build_kiro_cross_format_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    original_body_json: &serde_json::Value,
    input: &LocalStandardDecisionInput,
    attempt: &LocalStandardCandidateAttempt,
    transport: &Arc<GatewayProviderTransportSnapshot>,
    provider_api_format: &str,
    mapped_model: String,
    auth_header: String,
    auth_value: String,
    claude_request_body: Value,
    upstream_is_stream: bool,
    kiro_auth: &KiroRequestAuth,
) -> Option<LocalStandardCandidatePayloadParts> {
    let candidate = &attempt.eligible.candidate;
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
            mark_skipped_local_standard_candidate_with_extra_data(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "provider_request_body_build_failed",
                request_body_build_failure_extra_data(
                    &claude_request_body,
                    provider_api_format,
                    provider_api_format,
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
            mark_skipped_local_standard_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "upstream_url_missing",
                CandidateFailureDiagnostic::upstream_url_missing(
                    provider_api_format,
                    provider_api_format,
                    "standard_family_kiro_url",
                ),
            )
            .await;
            return None;
        }
    };
    let provider_request_headers = match build_kiro_provider_headers(KiroProviderHeadersInput {
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
            mark_skipped_local_standard_candidate_with_failure_diagnostic(
                state,
                input,
                trace_id,
                candidate,
                attempt.candidate_index,
                &attempt.candidate_id,
                "transport_header_rules_apply_failed",
                CandidateFailureDiagnostic::header_rules_apply_failed(
                    provider_api_format,
                    provider_api_format,
                    "standard_family_kiro_headers",
                ),
            )
            .await;
            return None;
        }
    };

    Some(LocalStandardCandidatePayloadParts {
        auth_header,
        auth_value,
        mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        upstream_is_stream,
        envelope_name: Some(KIRO_ENVELOPE_NAME),
        transport: Arc::clone(transport),
        transport_profile: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        provider_preserves_claude_thinking_signatures,
        sanitize_claude_request_thinking_signatures_for_non_native,
    };
    use serde_json::json;

    #[test]
    fn sanitizes_historical_claude_thinking_for_non_native_relays() {
        let mut body = json!({
            "model": "claude-opus-4-1",
            "messages": [{
                "role": "assistant",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "I should keep this short.",
                        "signature": "sig_123"
                    },
                    {
                        "type": "redacted_thinking",
                        "data": "opaque"
                    },
                    {
                        "type": "text",
                        "text": "Done."
                    }
                ]
            }]
        });

        assert!(sanitize_claude_request_thinking_signatures_for_non_native(
            &mut body
        ));
        assert_eq!(body["messages"][0]["content"][0]["type"], json!("text"));
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            json!("I should keep this short.")
        );
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][0]["content"][1]["text"], json!("Done."));
    }

    #[test]
    fn inserts_placeholder_when_only_redacted_thinking_would_remain() {
        let mut body = json!({
            "model": "claude-opus-4-1",
            "messages": [{
                "role": "assistant",
                "content": [{
                    "type": "redacted_thinking",
                    "data": "opaque"
                }]
            }]
        });

        assert!(sanitize_claude_request_thinking_signatures_for_non_native(
            &mut body
        ));
        assert_eq!(body["messages"][0]["content"][0]["type"], json!("text"));
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            json!("Previous thinking omitted.")
        );
    }

    #[test]
    fn official_claude_providers_preserve_thinking_signatures() {
        assert!(provider_preserves_claude_thinking_signatures(
            "anthropic",
            "https://relay.example.com"
        ));
        assert!(provider_preserves_claude_thinking_signatures(
            "custom",
            "https://api.anthropic.com"
        ));
        assert!(provider_preserves_claude_thinking_signatures(
            "aws",
            "https://bedrock-runtime.us-east-1.amazonaws.com"
        ));
        assert!(provider_preserves_claude_thinking_signatures(
            "amazon_bedrock",
            "https://relay.example.com"
        ));
        assert!(!provider_preserves_claude_thinking_signatures(
            "openai",
            "https://relay.example.com"
        ));
    }
}
