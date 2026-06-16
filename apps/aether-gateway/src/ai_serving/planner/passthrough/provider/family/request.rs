use std::collections::BTreeMap;
use std::sync::Arc;

use aether_contracts::ResolvedTransportProfile;
use serde_json::Value;

use crate::ai_serving::planner::common::{
    enforce_provider_body_stream_policy, request_requires_body_stream_field,
};
use crate::ai_serving::planner::redaction::{
    request_identity_response_encoding_when_redacted, resolve_provider_chat_pii_redaction,
};
use crate::ai_serving::transport::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestEnvelopeSupport, AntigravityRequestSideSupport,
};
use crate::ai_serving::transport::{
    build_gemini_cli_v1internal_request, build_grok_browser_headers, build_grok_upstream_url,
    build_same_format_provider_headers, resolve_local_gemini_cli_request_auth,
    GeminiCliRequestAuth, GeminiCliRequestAuthSupport, GeminiCliRequestEnvelopeSupport,
    GrokHeaderInput, SameFormatProviderCompatibilityEdit,
    SameFormatProviderCompatibilityEditAction, SameFormatProviderHeadersInput,
    GEMINI_CLI_USER_AGENT, GROK_CHAT_PATH,
};
use crate::ai_serving::{CandidateFailureDiagnostic, GatewayProviderTransportSnapshot};
use crate::{AppState, GatewayError};

mod policy;
mod prepare;

use self::prepare::prepare_local_same_format_provider_candidate;
use super::payload::{
    mark_skipped_local_same_format_provider_candidate,
    mark_skipped_local_same_format_provider_candidate_with_extra_data,
    mark_skipped_local_same_format_provider_candidate_with_failure_diagnostic,
};
use super::{
    LocalSameFormatProviderCandidateAttempt, LocalSameFormatProviderDecisionInput,
    LocalSameFormatProviderSpec,
};
use crate::ai_serving::planner::standard::same_format_provider_request_body_failure_extra_data;

pub(crate) fn resolve_same_format_provider_transport_unsupported_reason_for_trace(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> Option<&'static str> {
    let provider_api_format =
        match crate::ai_serving::normalize_api_format_alias(provider_api_format).as_str() {
            "openai:chat" => "openai:chat",
            "openai:responses" => "openai:responses",
            "openai:responses:compact" => "openai:responses:compact",
            "openai:embedding" => "openai:embedding",
            "openai:rerank" => "openai:rerank",
            "claude:messages" => "claude:messages",
            "gemini:generate_content" => "gemini:generate_content",
            "gemini:embedding" => "gemini:embedding",
            "jina:embedding" => "jina:embedding",
            "jina:rerank" => "jina:rerank",
            "doubao:embedding" => "doubao:embedding",
            "aliyun:multimodal_embedding" => "aliyun:multimodal_embedding",
            _ => return Some("transport_api_format_unsupported"),
        };
    let behavior = policy::classify_same_format_provider_request_behavior(
        transport,
        provider_api_format,
        crate::ai_serving::planner::spec_metadata::LocalExecutionSurfaceSpecMetadata {
            api_format: provider_api_format,
            require_streaming: false,
            requested_model_family: None,
            decision_kind: "trace_candidate_metadata",
            report_kind: Some("trace_candidate_metadata"),
        },
    );
    if !behavior.is_antigravity
        && !behavior.is_claude_code
        && !behavior.is_gemini_cli
        && !behavior.is_vertex
        && !behavior.is_kiro
    {
        return None;
    }

    let family = if provider_api_format.starts_with("gemini:") {
        crate::ai_serving::LocalSameFormatProviderFamily::Gemini
    } else {
        crate::ai_serving::LocalSameFormatProviderFamily::Standard
    };
    policy::same_format_provider_transport_unsupported_reason(
        &behavior,
        transport,
        family,
        provider_api_format,
    )
}

pub(crate) struct LocalSameFormatProviderCandidatePayloadParts {
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) is_antigravity: bool,
    pub(super) is_gemini_cli: bool,
    pub(super) is_kiro: bool,
    pub(super) auth_header: Option<String>,
    pub(super) auth_value: Option<String>,
    pub(super) provider_api_format: String,
    pub(super) mapped_model: String,
    pub(super) report_kind: &'static str,
    pub(super) upstream_is_stream: bool,
    pub(super) upstream_url: String,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) provider_request_body: Value,
    pub(super) transport_profile: Option<ResolvedTransportProfile>,
    pub(super) compatibility_edits: Vec<SameFormatProviderCompatibilityEdit>,
    pub(super) request_redacted: bool,
}

pub(crate) async fn resolve_local_same_format_provider_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalSameFormatProviderDecisionInput,
    attempt: &LocalSameFormatProviderCandidateAttempt,
    spec: LocalSameFormatProviderSpec,
) -> Result<Option<LocalSameFormatProviderCandidatePayloadParts>, GatewayError> {
    let candidate = &attempt.eligible.candidate;
    let Some(prepared) = prepare_local_same_format_provider_candidate(
        state,
        trace_id,
        input,
        &attempt.eligible,
        attempt.candidate_index,
        &attempt.candidate_id,
        spec,
    )
    .await
    else {
        return Ok(None);
    };
    let enable_model_directives =
        crate::system_features::reasoning_model_directive_enabled_for_api_format_and_model(
            state,
            spec.api_format,
            Some(&input.requested_model),
        )
        .await;
    let effective_headers = input.effective_headers(&parts.headers);
    let redaction = resolve_provider_chat_pii_redaction(
        state,
        parts,
        body_json,
        &input.auth_context,
        spec.api_format,
        &attempt.candidate_id,
    )
    .await?;
    let body_json = redaction.body_json.as_ref();
    let mut transport = Arc::clone(&prepared.transport);

    let Some(base_provider_request) =
        super::super::request::build_same_format_provider_request_body_with_compatibility_report(
            body_json,
            prepared.provider_api_format.as_str(),
            &prepared.mapped_model,
            spec,
            prepared.transport.endpoint.body_rules.as_ref(),
            Some(effective_headers),
            prepared.upstream_is_stream,
            prepared.force_body_stream_field,
            prepared.kiro_auth.as_ref(),
            prepared.is_claude_code,
            enable_model_directives,
        )
    else {
        mark_skipped_local_same_format_provider_candidate_with_extra_data(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "provider_request_body_missing",
            same_format_provider_request_body_failure_extra_data(
                body_json,
                attempt.eligible.provider_api_format.as_str(),
                prepared.transport.endpoint.body_rules.as_ref(),
                if prepared.kiro_auth.is_some() {
                    "kiro_envelope"
                } else {
                    "same_format"
                },
            ),
        )
        .await;
        return Ok(None);
    };
    let mut base_provider_request_body = base_provider_request.body;
    let mut compatibility_edits = base_provider_request.compatibility_edits;
    if let Some(mapping) =
        crate::system_features::reasoning_model_directive_mapping_for_api_format_and_model(
            state,
            spec.api_format,
            Some(&input.requested_model),
        )
        .await
    {
        let before_mapping = base_provider_request_body.clone();
        crate::ai_serving::apply_model_directive_mapping_patch(
            &mut base_provider_request_body,
            &mapping,
        );
        if before_mapping != base_provider_request_body {
            compatibility_edits.push(SameFormatProviderCompatibilityEdit {
                field: "model_directive_mapping".to_string(),
                action: SameFormatProviderCompatibilityEditAction::RuntimeRewrite,
                detail: "applied configured model directive mapping patch".to_string(),
            });
        }
        // Directive mapping is a deep-merge patch and may overwrite/add `stream`;
        // re-enforce stream-field policy afterward.
        // Kiro behavior classification already hard-requires upstream streaming,
        // and the Kiro envelope does not use a top-level body stream field.
        if prepared.kiro_auth.is_none() {
            enforce_provider_body_stream_policy(
                &mut base_provider_request_body,
                prepared.provider_api_format.as_str(),
                prepared.upstream_is_stream,
                request_requires_body_stream_field(body_json, prepared.force_body_stream_field),
            );
        }
    }

    let antigravity_auth = if prepared.is_antigravity {
        match classify_local_antigravity_request_support(
            &transport,
            &base_provider_request_body,
            AntigravityEnvelopeRequestType::Agent,
        ) {
            AntigravityRequestSideSupport::Supported(spec) => Some(spec.auth),
            AntigravityRequestSideSupport::Unsupported(_) => {
                mark_skipped_local_same_format_provider_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    "transport_unsupported",
                )
                .await;
                return Ok(None);
            }
        }
    } else {
        None
    };
    let gemini_cli_auth = if prepared.behavior.is_gemini_cli {
        let mut auth = match resolve_local_gemini_cli_request_auth(&transport) {
            GeminiCliRequestAuthSupport::Supported(auth) => auth,
            GeminiCliRequestAuthSupport::Unsupported(_) => {
                mark_skipped_local_same_format_provider_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    "transport_auth_unavailable",
                )
                .await;
                return Ok(None);
            }
        };
        if auth.project_id.is_none() {
            auth = match state
                .hydrate_gemini_cli_project_metadata_for_transport(&transport)
                .await
            {
                Some(hydrated) => {
                    transport = Arc::new(hydrated);
                    match resolve_local_gemini_cli_request_auth(&transport) {
                        GeminiCliRequestAuthSupport::Supported(auth) => auth,
                        GeminiCliRequestAuthSupport::Unsupported(_) => {
                            GeminiCliRequestAuth::default()
                        }
                    }
                }
                None => GeminiCliRequestAuth::default(),
            };
        }
        Some(auth)
    } else {
        None
    };
    let provider_request_body = if let Some(antigravity_auth) = antigravity_auth.as_ref() {
        match build_antigravity_safe_v1internal_request(
            antigravity_auth,
            trace_id,
            &prepared.mapped_model,
            &base_provider_request_body,
            AntigravityEnvelopeRequestType::Agent,
        ) {
            AntigravityRequestEnvelopeSupport::Supported(envelope) => envelope,
            AntigravityRequestEnvelopeSupport::Unsupported(_) => {
                mark_skipped_local_same_format_provider_candidate_with_extra_data(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    "provider_request_body_missing",
                    same_format_provider_request_body_failure_extra_data(
                        body_json,
                        attempt.eligible.provider_api_format.as_str(),
                        prepared.transport.endpoint.body_rules.as_ref(),
                        "antigravity_envelope",
                    ),
                )
                .await;
                return Ok(None);
            }
        }
    } else if let Some(gemini_cli_auth) = gemini_cli_auth.as_ref() {
        match build_gemini_cli_v1internal_request(
            gemini_cli_auth,
            trace_id,
            &prepared.mapped_model,
            &base_provider_request_body,
        ) {
            GeminiCliRequestEnvelopeSupport::Supported(envelope) => envelope,
            GeminiCliRequestEnvelopeSupport::Unsupported(_) => {
                mark_skipped_local_same_format_provider_candidate_with_extra_data(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    "provider_request_body_missing",
                    same_format_provider_request_body_failure_extra_data(
                        body_json,
                        attempt.eligible.provider_api_format.as_str(),
                        prepared.transport.endpoint.body_rules.as_ref(),
                        "gemini_cli_v1internal_envelope",
                    ),
                )
                .await;
                return Ok(None);
            }
        }
    } else {
        base_provider_request_body
    };

    let is_grok = prepared
        .transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("grok");
    let transport_profile = crate::ai_serving::transport::resolve_transport_profile(&transport);
    let upstream_url = if is_grok {
        Some(build_grok_upstream_url(&transport, GROK_CHAT_PATH))
    } else {
        super::super::request::build_same_format_upstream_url(
            parts,
            &transport,
            &prepared.mapped_model,
            prepared.provider_api_format.as_str(),
            spec,
            prepared.upstream_is_stream,
            prepared.kiro_auth.as_ref(),
            Some(&provider_request_body),
        )
    };
    let Some(upstream_url) = upstream_url else {
        mark_skipped_local_same_format_provider_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "upstream_url_missing",
            CandidateFailureDiagnostic::upstream_url_missing(
                attempt.eligible.provider_api_format.as_str(),
                attempt.eligible.provider_api_format.as_str(),
                "same_format_provider_url",
            ),
        )
        .await;
        return Ok(None);
    };

    let mut extra_headers = antigravity_auth
        .as_ref()
        .map(build_antigravity_static_identity_headers)
        .unwrap_or_default();
    if prepared.behavior.is_gemini_cli {
        extra_headers.insert("user-agent".to_string(), GEMINI_CLI_USER_AGENT.to_string());
    }
    let Some(mut provider_request_headers) = (if is_grok {
        build_grok_browser_headers(GrokHeaderInput {
            transport: &transport,
            transport_profile: transport_profile.as_ref(),
            request_headers: Some(effective_headers),
            content_type: "application/json",
            accept: "text/event-stream",
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
        })
    } else {
        build_same_format_provider_headers(SameFormatProviderHeadersInput {
            headers: effective_headers,
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
            header_rules: transport.endpoint.header_rules.as_ref(),
            behavior: prepared.behavior,
            auth_header: prepared.auth_header.as_deref(),
            auth_value: prepared.auth_value.as_deref(),
            extra_headers: &extra_headers,
            key_fingerprint: transport.key.fingerprint.as_ref(),
            kiro_auth_config: prepared.kiro_auth.as_ref().map(|auth| &auth.auth_config),
            kiro_machine_id: prepared
                .kiro_auth
                .as_ref()
                .map(|auth| auth.machine_id.as_str()),
        })
    }) else {
        mark_skipped_local_same_format_provider_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_header_rules_apply_failed",
            CandidateFailureDiagnostic::header_rules_apply_failed(
                attempt.eligible.provider_api_format.as_str(),
                attempt.eligible.provider_api_format.as_str(),
                "same_format_provider_headers",
            ),
        )
        .await;
        return Ok(None);
    };
    request_identity_response_encoding_when_redacted(
        &mut provider_request_headers,
        redaction.redacted,
    );

    Ok(Some(LocalSameFormatProviderCandidatePayloadParts {
        transport,
        is_antigravity: prepared.is_antigravity,
        is_gemini_cli: prepared.behavior.is_gemini_cli,
        is_kiro: prepared.is_kiro,
        auth_header: prepared.auth_header,
        auth_value: prepared.auth_value,
        provider_api_format: prepared.provider_api_format,
        mapped_model: prepared.mapped_model,
        report_kind: prepared.report_kind,
        upstream_is_stream: prepared.upstream_is_stream,
        upstream_url,
        provider_request_headers,
        provider_request_body,
        transport_profile,
        compatibility_edits,
        request_redacted: redaction.redacted,
    }))
}
