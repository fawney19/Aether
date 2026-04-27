use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;
use tracing::debug;

use crate::ai_pipeline::conversion::{request_conversion_direct_auth, request_conversion_kind};
use crate::ai_pipeline::planner::candidate_preparation::{
    prepare_header_authenticated_candidate, OauthPreparationContext,
};
use crate::ai_pipeline::planner::candidate_resolution::EligibleLocalExecutionCandidate;
use crate::ai_pipeline::planner::common::force_upstream_streaming_for_provider;
use crate::ai_pipeline::planner::spec_metadata::local_openai_responses_spec_metadata;
use crate::ai_pipeline::planner::standard::{
    apply_codex_openai_responses_special_headers, build_cross_format_openai_responses_request_body,
    build_cross_format_openai_responses_upstream_url, build_local_openai_responses_request_body,
    build_local_openai_responses_upstream_url, request_body_build_failure_extra_data,
};
use crate::ai_pipeline::transport::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestEnvelopeSupport, AntigravityRequestSideSupport,
};
use crate::ai_pipeline::transport::apply_local_header_rules;
use crate::ai_pipeline::transport::auth::{
    build_claude_passthrough_headers, build_complete_passthrough_headers_with_auth,
    build_openai_passthrough_headers, ensure_upstream_auth_header, resolve_local_gemini_auth,
    resolve_local_openai_bearer_auth, resolve_local_standard_auth,
};
use crate::ai_pipeline::transport::kiro::{
    build_kiro_provider_headers, build_kiro_provider_request_body,
    local_kiro_request_transport_unsupported_reason_with_network, KiroProviderHeadersInput,
    KiroRequestAuth, KIRO_ENVELOPE_NAME,
};
use crate::ai_pipeline::transport::local_standard_transport_unsupported_reason_with_network;
use crate::ai_pipeline::transport::vertex::uses_vertex_api_key_query_auth;
use crate::ai_pipeline::{CandidateFailureDiagnostic, ConversionMode, ExecutionStrategy};
use crate::ai_pipeline::{
    GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth, PlannerAppState,
};
use crate::AppState;

use super::support::{
    mark_skipped_local_openai_responses_candidate,
    mark_skipped_local_openai_responses_candidate_with_extra_data,
    mark_skipped_local_openai_responses_candidate_with_failure_diagnostic,
    LocalOpenAiResponsesDecisionInput,
};
use super::LocalOpenAiResponsesSpec;

const ANTIGRAVITY_ENVELOPE_NAME: &str = "antigravity:v1internal";

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
) -> Option<LocalOpenAiResponsesCandidatePayloadParts> {
    let spec_metadata = local_openai_responses_spec_metadata(spec);
    let client_api_format = spec_metadata.api_format.trim().to_ascii_lowercase();
    let planner_state = PlannerAppState::new(state);
    let candidate = &eligible.candidate;
    let provider_api_format = eligible.provider_api_format.as_str();
    let transport = &eligible.transport;
    let is_antigravity = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("antigravity");
    let is_kiro_claude_cli = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("kiro")
        && provider_api_format.eq_ignore_ascii_case("claude:cli");

    let same_format = api_format_alias_matches(provider_api_format, &client_api_format);
    let conversion_kind = request_conversion_kind(spec_metadata.api_format, provider_api_format);
    let transport_unsupported_reason = if same_format && is_kiro_claude_cli {
        local_kiro_request_transport_unsupported_reason_with_network(transport)
    } else if same_format {
        local_standard_transport_unsupported_reason_with_network(transport, provider_api_format)
    } else {
        match conversion_kind {
            Some(_) if is_antigravity && provider_api_format == "gemini:cli" => None,
            Some(kind) => {
                crate::ai_pipeline::conversion::request_conversion_transport_unsupported_reason(
                    transport, kind,
                )
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
        return None;
    }

    let oauth_context = OauthPreparationContext {
        trace_id,
        api_format: provider_api_format,
        operation: "openai_responses_candidate_request",
    };
    let kiro_auth = if is_kiro_claude_cli {
        match crate::ai_pipeline::planner::candidate_preparation::resolve_candidate_oauth_auth(
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
                return None;
            }
        }
    } else {
        None
    };

    let direct_auth = if kiro_auth.is_some() {
        None
    } else if same_format {
        match crate::ai_pipeline::normalize_legacy_openai_format_alias(provider_api_format).as_str()
        {
            "gemini:cli" => resolve_local_gemini_auth(transport),
            "claude:cli" => resolve_local_standard_auth(transport),
            "openai:responses" | "openai:responses:compact" => {
                resolve_local_openai_bearer_auth(transport)
            }
            _ => None,
        }
    } else {
        conversion_kind.and_then(|kind| request_conversion_direct_auth(transport, kind))
    };
    let prepared_candidate = if let Some(kiro_auth) = kiro_auth.as_ref() {
        let mapped_model =
            match crate::ai_pipeline::planner::candidate_preparation::resolve_candidate_mapped_model(
                candidate,
            ) {
                Ok(mapped_model) => mapped_model,
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
        crate::ai_pipeline::planner::candidate_preparation::PreparedHeaderAuthenticatedCandidate {
            auth_header: kiro_auth.name.to_string(),
            auth_value: kiro_auth.value.clone(),
            mapped_model,
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
                return None;
            }
        }
    };
    let auth_header = prepared_candidate.auth_header;
    let auth_value = prepared_candidate.auth_value;
    let mapped_model = prepared_candidate.mapped_model;

    let needs_bidirectional_conversion = !same_format && conversion_kind.is_some();
    let upstream_is_stream = spec_metadata.require_streaming
        || is_antigravity
        || force_upstream_streaming_for_provider(
            transport.provider.provider_type.as_str(),
            provider_api_format,
        );
    let Some(base_provider_request_body) = (if needs_bidirectional_conversion {
        build_cross_format_openai_responses_request_body(
            body_json,
            &mapped_model,
            spec_metadata.api_format,
            provider_api_format,
            upstream_is_stream,
            transport.provider.provider_type.as_str(),
            if is_kiro_claude_cli {
                None
            } else {
                transport.endpoint.body_rules.as_ref()
            },
            Some(input.auth_context.api_key_id.as_str()),
        )
    } else {
        build_local_openai_responses_request_body(
            body_json,
            &mapped_model,
            upstream_is_stream,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            if is_kiro_claude_cli {
                None
            } else {
                transport.endpoint.body_rules.as_ref()
            },
            Some(input.auth_context.api_key_id.as_str()),
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
                return None;
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
                return None;
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
        )
        .await;
    }

    let Some(upstream_url) = (if needs_bidirectional_conversion {
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
        return None;
    };
    let uses_vertex_query_auth = uses_vertex_api_key_query_auth(transport, provider_api_format);

    let extra_headers = antigravity_auth
        .as_ref()
        .map(build_antigravity_static_identity_headers)
        .unwrap_or_default();
    let mut provider_request_headers = if same_format {
        build_complete_passthrough_headers_with_auth(
            &parts.headers,
            &auth_header,
            &auth_value,
            &extra_headers,
            Some("application/json"),
        )
    } else if provider_api_format.starts_with("claude:") {
        build_claude_passthrough_headers(
            &parts.headers,
            &auth_header,
            &auth_value,
            &extra_headers,
            Some("application/json"),
        )
    } else {
        build_openai_passthrough_headers(
            &parts.headers,
            &auth_header,
            &auth_value,
            &extra_headers,
            Some("application/json"),
        )
    };
    let protected_headers = if uses_vertex_query_auth {
        &["content-type"][..]
    } else {
        &[auth_header.as_str(), "content-type"][..]
    };
    if !apply_local_header_rules(
        &mut provider_request_headers,
        transport.endpoint.header_rules.as_ref(),
        protected_headers,
        &provider_request_body,
        Some(body_json),
    ) {
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
        return None;
    }
    apply_codex_openai_responses_special_headers(
        &mut provider_request_headers,
        &provider_request_body,
        &parts.headers,
        transport.provider.provider_type.as_str(),
        provider_api_format,
        Some(trace_id),
        transport.key.decrypted_auth_config.as_deref(),
    );
    let (auth_header, auth_value) = if uses_vertex_query_auth {
        provider_request_headers.remove("x-goog-api-key");
        (String::new(), String::new())
    } else {
        ensure_upstream_auth_header(&mut provider_request_headers, &auth_header, &auth_value);
        (auth_header, auth_value)
    };
    if upstream_is_stream {
        provider_request_headers
            .entry("accept".to_string())
            .or_insert_with(|| "text/event-stream".to_string());
    }

    let execution_strategy = if same_format {
        ExecutionStrategy::LocalSameFormat
    } else {
        ExecutionStrategy::LocalCrossFormat
    };
    let conversion_mode = if needs_bidirectional_conversion {
        ConversionMode::Bidirectional
    } else {
        ConversionMode::None
    };

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
        is_antigravity: is_antigravity
            || antigravity_auth.is_some() && ANTIGRAVITY_ENVELOPE_NAME == "antigravity:v1internal",
        envelope_name: if is_antigravity || antigravity_auth.is_some() {
            Some(ANTIGRAVITY_ENVELOPE_NAME)
        } else {
            None
        },
        upstream_is_stream,
        transport: Arc::clone(transport),
    })
}

fn api_format_alias_matches(left: &str, right: &str) -> bool {
    crate::ai_pipeline::legacy_openai_format_alias_matches(left, right)
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
) -> Option<LocalOpenAiResponsesCandidatePayloadParts> {
    let candidate = &eligible.candidate;
    let provider_request_body = match build_kiro_provider_request_body(
        &claude_request_body,
        &mapped_model,
        &kiro_auth.auth_config,
        transport.endpoint.body_rules.as_ref(),
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
            return None;
        }
    };
    let upstream_url = match crate::ai_pipeline::build_provider_transport_request_url(
        transport,
        provider_api_format,
        Some(&mapped_model),
        upstream_is_stream,
        parts.uri.query(),
        Some(kiro_auth.auth_config.effective_api_region()),
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
            return None;
        }
    };
    let provider_request_headers = match build_kiro_provider_headers(KiroProviderHeadersInput {
        headers: &parts.headers,
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
            return None;
        }
    };
    let execution_strategy = if needs_bidirectional_conversion {
        ExecutionStrategy::LocalCrossFormat
    } else {
        ExecutionStrategy::LocalSameFormat
    };
    let conversion_mode = if needs_bidirectional_conversion {
        ConversionMode::Bidirectional
    } else {
        ConversionMode::None
    };

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
        envelope_name: Some(KIRO_ENVELOPE_NAME),
        upstream_is_stream,
        transport: Arc::clone(transport),
    })
}
