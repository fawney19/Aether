use std::collections::BTreeMap;
use std::sync::Arc;

use aether_contracts::ResolvedTransportProfile;
use serde_json::Value;

use crate::ai_serving::planner::candidate_preparation::{
    prepare_header_authenticated_candidate, OauthPreparationContext,
};
use crate::ai_serving::planner::spec_metadata::local_openai_image_spec_metadata;
use crate::ai_serving::pure::normalize_openai_image_request_with_options;
use crate::ai_serving::transport::{
    build_grok_browser_headers, build_grok_upstream_url, build_openai_image_headers,
    build_openai_image_upstream_url, build_standard_provider_request_headers,
    openai_image_transport_unsupported_reason, resolve_openai_image_auth, GrokHeaderInput,
    ProviderOpenAiImageHeadersInput, StandardProviderRequestHeadersInput, GROK_CHAT_PATH,
};
use crate::ai_serving::{
    apply_codex_openai_responses_special_body_edits, apply_codex_openai_responses_special_headers,
    build_chatgpt_web_image_request_body,
    build_gemini_image_request_body_from_openai_image_request,
    build_openai_image_api_provider_request_body, build_openai_image_provider_request_body,
    default_model_for_openai_image_operation, normalize_openai_image_request,
    request_conversion_direct_auth, CandidateFailureDiagnostic, GatewayProviderTransportSnapshot,
    PlannerAppState, RequestConversionKind,
};
use crate::image_capabilities::openai_image_normalize_options_for_provider;
use crate::AppState;

use super::support::{
    mark_skipped_local_openai_image_candidate,
    mark_skipped_local_openai_image_candidate_with_failure_diagnostic,
    LocalOpenAiImageCandidateAttempt, LocalOpenAiImageDecisionInput,
};
use super::LocalOpenAiImageSpec;

pub(super) use crate::ai_serving::resolve_requested_openai_image_model_for_request as resolve_requested_image_model_for_request;

pub(super) struct LocalOpenAiImageCandidatePayloadParts {
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) auth_header: String,
    pub(super) auth_value: String,
    pub(super) requested_model: String,
    pub(super) mapped_model: String,
    pub(super) provider_api_format: String,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) provider_request_body: Value,
    pub(super) upstream_url: String,
    pub(super) input_summary: Value,
    pub(super) transport_profile: Option<ResolvedTransportProfile>,
}

pub(super) async fn resolve_local_openai_image_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    attempt: &LocalOpenAiImageCandidateAttempt,
    spec: LocalOpenAiImageSpec,
) -> Option<LocalOpenAiImageCandidatePayloadParts> {
    let spec_metadata = local_openai_image_spec_metadata(spec);
    let candidate = &attempt.eligible.candidate;
    let transport = &attempt.eligible.transport;
    let provider_api_format = attempt.eligible.provider_api_format.as_str();
    let effective_headers = input.effective_headers(&parts.headers);

    if provider_api_format == "gemini:generate_content" {
        return resolve_local_openai_image_to_gemini_candidate_payload_parts(
            state,
            parts,
            body_json,
            body_base64,
            trace_id,
            input,
            attempt,
            spec,
        )
        .await;
    }

    if let Some(skip_reason) =
        openai_image_transport_unsupported_reason(transport, spec_metadata.api_format)
    {
        mark_skipped_local_openai_image_candidate(
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
        PlannerAppState::new(state),
        transport,
        candidate,
        resolve_openai_image_auth(transport),
        OauthPreparationContext {
            trace_id,
            api_format: spec_metadata.api_format,
            operation: "openai_image_candidate_request",
        },
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(skip_reason) => {
            mark_skipped_local_openai_image_candidate(
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
    let auth_header = prepared_candidate.auth_header;
    let auth_value = prepared_candidate.auth_value;

    let normalized_request = normalize_openai_image_request_with_options(
        parts,
        body_json,
        body_base64,
        openai_image_normalize_options_for_provider(&transport.provider.provider_type),
    );
    let Some(normalized_request) = normalized_request else {
        mark_skipped_local_openai_image_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "provider_request_body_missing",
            CandidateFailureDiagnostic::provider_request_body_missing(
                spec_metadata.api_format,
                spec_metadata.api_format,
                "openai_image_request_normalize",
            ),
        )
        .await;
        return None;
    };

    let is_chatgpt_web = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("chatgpt_web");
    let is_grok = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("grok");
    let is_codex = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("codex");
    let transport_profile = crate::ai_serving::transport::resolve_transport_profile(transport);
    let upstream_url = if is_chatgpt_web {
        chatgpt_web_image_internal_url(&transport.endpoint.base_url)
    } else if is_grok {
        build_grok_upstream_url(transport, GROK_CHAT_PATH)
    } else {
        build_openai_image_upstream_url(transport, Some(parts.uri.path()), parts.uri.query())
    };
    let mut provider_request_body = if is_chatgpt_web {
        match build_chatgpt_web_image_request_body(parts, body_json, body_base64) {
            Ok(body) => body,
            Err(err) => err.to_error_json(),
        }
    } else if is_codex || is_grok {
        build_openai_image_provider_request_body(&normalized_request)
    } else {
        build_openai_image_api_provider_request_body(
            &normalized_request,
            Some(prepared_candidate.mapped_model.as_str()),
        )
    };
    if !is_chatgpt_web {
        apply_codex_openai_responses_special_body_edits(
            &mut provider_request_body,
            transport.provider.provider_type.as_str(),
            spec_metadata.api_format,
            transport.endpoint.body_rules.as_ref(),
            Some(candidate.key_id.as_str()),
        );
    }

    let Some(mut provider_request_headers) = (if is_grok {
        build_grok_browser_headers(GrokHeaderInput {
            transport,
            transport_profile: transport_profile.as_ref(),
            request_headers: Some(effective_headers),
            content_type: "application/json",
            accept: "*/*",
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
        })
    } else {
        build_openai_image_headers(ProviderOpenAiImageHeadersInput {
            headers: effective_headers,
            auth_header: &auth_header,
            auth_value: &auth_value,
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
        })
    }) else {
        mark_skipped_local_openai_image_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_header_rules_apply_failed",
            CandidateFailureDiagnostic::header_rules_apply_failed(
                spec_metadata.api_format,
                spec_metadata.api_format,
                "openai_image_header_rules",
            ),
        )
        .await;
        return None;
    };
    if is_chatgpt_web {
        provider_request_headers.insert("x-aether-chatgpt-web-image".to_string(), "1".to_string());
    } else if is_grok {
    } else {
        apply_codex_openai_responses_special_headers(
            &mut provider_request_headers,
            &provider_request_body,
            effective_headers,
            transport.provider.provider_type.as_str(),
            spec_metadata.api_format,
            Some(trace_id),
            transport.key.decrypted_auth_config.as_deref(),
        );
    }
    let requested_model = normalized_request
        .requested_model
        .clone()
        .unwrap_or_else(|| {
            default_model_for_openai_image_operation(normalized_request.operation).to_string()
        });
    let mapped_model = provider_request_body
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string();

    let input_summary = if is_chatgpt_web || is_grok {
        provider_request_body.clone()
    } else {
        normalized_request.summary_json
    };

    Some(LocalOpenAiImageCandidatePayloadParts {
        transport: Arc::clone(transport),
        auth_header,
        auth_value,
        requested_model,
        mapped_model,
        provider_api_format: spec_metadata.api_format.to_string(),
        provider_request_headers,
        provider_request_body,
        upstream_url,
        input_summary,
        transport_profile,
    })
}

async fn resolve_local_openai_image_to_gemini_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &Value,
    body_base64: Option<&str>,
    trace_id: &str,
    input: &LocalOpenAiImageDecisionInput,
    attempt: &LocalOpenAiImageCandidateAttempt,
    spec: LocalOpenAiImageSpec,
) -> Option<LocalOpenAiImageCandidatePayloadParts> {
    let spec_metadata = local_openai_image_spec_metadata(spec);
    let candidate = &attempt.eligible.candidate;
    let transport = &attempt.eligible.transport;
    let provider_api_format = "gemini:generate_content";
    let effective_headers = input.effective_headers(&parts.headers);

    let prepared_candidate = match prepare_header_authenticated_candidate(
        PlannerAppState::new(state),
        transport,
        candidate,
        request_conversion_direct_auth(transport, RequestConversionKind::ToGeminiStandard),
        OauthPreparationContext {
            trace_id,
            api_format: provider_api_format,
            operation: "openai_image_to_gemini_candidate_request",
        },
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(skip_reason) => {
            mark_skipped_local_openai_image_candidate(
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

    let Some(normalized_request) = normalize_openai_image_request(parts, body_json, body_base64)
    else {
        mark_skipped_local_openai_image_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "provider_request_body_missing",
            CandidateFailureDiagnostic::provider_request_body_missing(
                spec_metadata.api_format,
                provider_api_format,
                "openai_image_request_normalize",
            ),
        )
        .await;
        return None;
    };

    let Some(mut converted) = build_gemini_image_request_body_from_openai_image_request(
        &normalized_request,
        &prepared_candidate.mapped_model,
    ) else {
        mark_skipped_local_openai_image_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "provider_request_body_missing",
            CandidateFailureDiagnostic::provider_request_body_missing(
                spec_metadata.api_format,
                provider_api_format,
                "openai_image_to_gemini_request_body",
            ),
        )
        .await;
        return None;
    };
    converted.body_json =
        match crate::ai_serving::transport::apply_standard_provider_request_body_rules_with_request_headers(
            converted.body_json,
            transport.endpoint.body_rules.as_ref(),
            body_json,
            effective_headers,
        ) {
            Some(body) => body,
            None => {
                mark_skipped_local_openai_image_candidate_with_failure_diagnostic(
                    state,
                    input,
                    trace_id,
                    candidate,
                    attempt.candidate_index,
                    &attempt.candidate_id,
                    "provider_request_body_missing",
                    CandidateFailureDiagnostic::provider_request_body_missing(
                        spec_metadata.api_format,
                        provider_api_format,
                        "openai_image_to_gemini_body_rules",
                    ),
                )
                .await;
                return None;
            }
        };
    let upstream_is_stream = spec_metadata.require_streaming;
    let Some(upstream_url) = crate::ai_serving::planner::standard::build_standard_upstream_url(
        parts,
        transport,
        &converted.mapped_model,
        provider_api_format,
        upstream_is_stream,
        Some(&converted.body_json),
    ) else {
        mark_skipped_local_openai_image_candidate_with_failure_diagnostic(
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
                "openai_image_to_gemini_url",
            ),
        )
        .await;
        return None;
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
            provider_request_body: &converted.body_json,
            original_request_body: body_json,
            upstream_is_stream,
        })
    else {
        mark_skipped_local_openai_image_candidate_with_failure_diagnostic(
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
                "openai_image_to_gemini_headers",
            ),
        )
        .await;
        return None;
    };

    Some(LocalOpenAiImageCandidatePayloadParts {
        transport: Arc::clone(transport),
        auth_header: resolved_headers.auth_header,
        auth_value: resolved_headers.auth_value,
        requested_model: converted.requested_model,
        mapped_model: converted.mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_headers: resolved_headers.headers,
        provider_request_body: converted.body_json,
        upstream_url,
        input_summary: converted.summary_json,
        transport_profile: None,
    })
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
