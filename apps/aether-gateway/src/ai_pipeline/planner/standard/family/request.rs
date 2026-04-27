use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::ai_pipeline::planner::candidate_preparation::{
    prepare_header_authenticated_candidate, OauthPreparationContext,
};
use crate::ai_pipeline::planner::common::force_upstream_streaming_for_provider;
use crate::ai_pipeline::planner::spec_metadata::local_standard_spec_metadata;
use crate::ai_pipeline::planner::standard::{
    apply_codex_openai_responses_special_headers, request_body_build_failure_extra_data,
};
use crate::ai_pipeline::transport::apply_local_header_rules;
use crate::ai_pipeline::transport::auth::{
    build_claude_passthrough_headers, build_openai_passthrough_headers, ensure_upstream_auth_header,
};
use crate::ai_pipeline::transport::kiro::{
    build_kiro_provider_headers, build_kiro_provider_request_body, KiroProviderHeadersInput,
    KiroRequestAuth, KIRO_ENVELOPE_NAME,
};
use crate::ai_pipeline::transport::vertex::uses_vertex_api_key_query_auth;
use crate::ai_pipeline::{
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
    let planner_state = crate::ai_pipeline::PlannerAppState::new(state);
    let candidate = &attempt.eligible.candidate;
    let transport = &attempt.eligible.transport;
    let provider_api_format = attempt.eligible.provider_api_format.as_str();
    let is_kiro_claude_cli = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("kiro")
        && provider_api_format.eq_ignore_ascii_case("claude:cli");
    let Some(conversion_kind) = crate::ai_pipeline::conversion::request_conversion_kind(
        spec_metadata.api_format,
        provider_api_format,
    ) else {
        return None;
    };

    if let Some(skip_reason) =
        crate::ai_pipeline::conversion::request_conversion_transport_unsupported_reason(
            transport,
            conversion_kind,
        )
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

    let oauth_context = OauthPreparationContext {
        trace_id,
        api_format: provider_api_format,
        operation: "standard_family_cross_format",
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
        let mapped_model =
            match crate::ai_pipeline::planner::candidate_preparation::resolve_candidate_mapped_model(
                candidate,
            ) {
                Ok(mapped_model) => mapped_model,
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
            crate::ai_pipeline::conversion::request_conversion_direct_auth(
                transport,
                conversion_kind,
            ),
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

    let upstream_is_stream = spec_metadata.require_streaming
        || force_upstream_streaming_for_provider(
            transport.provider.provider_type.as_str(),
            provider_api_format,
        );
    let provider_request_body =
        match crate::ai_pipeline::planner::standard::build_standard_request_body(
            body_json,
            spec_metadata.api_format,
            &prepared_candidate.mapped_model,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            parts.uri.path(),
            upstream_is_stream,
            if is_kiro_claude_cli {
                None
            } else {
                transport.endpoint.body_rules.as_ref()
            },
            Some(input.auth_context.api_key_id.as_str()),
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

    let upstream_url = match crate::ai_pipeline::planner::standard::build_standard_upstream_url(
        parts,
        transport,
        &prepared_candidate.mapped_model,
        provider_api_format,
        upstream_is_stream,
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
    let uses_vertex_query_auth = uses_vertex_api_key_query_auth(transport, provider_api_format);

    let mut provider_request_headers = if provider_api_format.starts_with("claude:") {
        build_claude_passthrough_headers(
            &parts.headers,
            &prepared_candidate.auth_header,
            &prepared_candidate.auth_value,
            &BTreeMap::new(),
            Some("application/json"),
        )
    } else {
        build_openai_passthrough_headers(
            &parts.headers,
            &prepared_candidate.auth_header,
            &prepared_candidate.auth_value,
            &BTreeMap::new(),
            Some("application/json"),
        )
    };
    let protected_headers = if uses_vertex_query_auth {
        &["content-type"][..]
    } else {
        &[prepared_candidate.auth_header.as_str(), "content-type"][..]
    };
    if !apply_local_header_rules(
        &mut provider_request_headers,
        transport.endpoint.header_rules.as_ref(),
        protected_headers,
        &provider_request_body,
        Some(body_json),
    ) {
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
        ensure_upstream_auth_header(
            &mut provider_request_headers,
            &prepared_candidate.auth_header,
            &prepared_candidate.auth_value,
        );
        (
            prepared_candidate.auth_header.clone(),
            prepared_candidate.auth_value.clone(),
        )
    };
    if upstream_is_stream {
        provider_request_headers
            .entry("accept".to_string())
            .or_insert_with(|| "text/event-stream".to_string());
    }

    Some(LocalStandardCandidatePayloadParts {
        auth_header,
        auth_value,
        mapped_model: prepared_candidate.mapped_model,
        provider_api_format: provider_api_format.to_string(),
        provider_request_body,
        provider_request_headers,
        upstream_url,
        upstream_is_stream,
        envelope_name: None,
        transport: Arc::clone(transport),
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
    let provider_request_body = match build_kiro_provider_request_body(
        &claude_request_body,
        &mapped_model,
        &kiro_auth.auth_config,
        transport.endpoint.body_rules.as_ref(),
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
    })
}
