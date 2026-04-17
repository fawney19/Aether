use std::collections::BTreeMap;

use serde_json::Value;
use tracing::debug;

use crate::ai_pipeline::conversion::{request_conversion_direct_auth, request_conversion_kind};
use crate::ai_pipeline::planner::candidate_eligibility::EligibleLocalExecutionCandidate;
use crate::ai_pipeline::planner::candidate_preparation::{
    prepare_header_authenticated_candidate, OauthPreparationContext,
};
use crate::ai_pipeline::planner::common::force_upstream_streaming_for_provider;
use crate::ai_pipeline::planner::spec_metadata::local_openai_cli_spec_metadata;
use crate::ai_pipeline::planner::standard::{
    apply_codex_openai_cli_special_headers, build_cross_format_openai_cli_request_body,
    build_cross_format_openai_cli_upstream_url, build_local_openai_cli_request_body,
    build_local_openai_cli_upstream_url,
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
use crate::ai_pipeline::transport::local_standard_transport_unsupported_reason_with_network;
use crate::ai_pipeline::{ConversionMode, ExecutionStrategy};
use crate::ai_pipeline::{GatewayProviderTransportSnapshot, PlannerAppState};
use crate::AppState;

use super::support::{mark_skipped_local_openai_cli_candidate, LocalOpenAiCliDecisionInput};
use super::LocalOpenAiCliSpec;

const ANTIGRAVITY_ENVELOPE_NAME: &str = "antigravity:v1internal";

pub(crate) struct LocalOpenAiCliCandidatePayloadParts {
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
    pub(super) upstream_is_stream: bool,
    pub(super) transport: GatewayProviderTransportSnapshot,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_local_openai_cli_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiCliDecisionInput,
    eligible: &EligibleLocalExecutionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    spec: LocalOpenAiCliSpec,
) -> Option<LocalOpenAiCliCandidatePayloadParts> {
    let spec_metadata = local_openai_cli_spec_metadata(spec);
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

    let same_format = provider_api_format == client_api_format;
    let conversion_kind = request_conversion_kind(spec_metadata.api_format, provider_api_format);
    let transport_unsupported_reason = if same_format {
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
        mark_skipped_local_openai_cli_candidate(
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

    let direct_auth = if same_format {
        match provider_api_format {
            "gemini:cli" => resolve_local_gemini_auth(transport),
            "claude:cli" => resolve_local_standard_auth(transport),
            "openai:cli" | "openai:compact" => resolve_local_openai_bearer_auth(transport),
            _ => None,
        }
    } else {
        conversion_kind.and_then(|kind| request_conversion_direct_auth(transport, kind))
    };
    let prepared_candidate = match prepare_header_authenticated_candidate(
        planner_state,
        transport,
        candidate,
        direct_auth,
        OauthPreparationContext {
            trace_id,
            api_format: provider_api_format,
            operation: "openai_cli_candidate_request",
        },
    )
    .await
    {
        Ok(prepared) => prepared,
        Err(skip_reason) => {
            mark_skipped_local_openai_cli_candidate(
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
        build_cross_format_openai_cli_request_body(
            body_json,
            &mapped_model,
            spec_metadata.api_format,
            provider_api_format,
            upstream_is_stream,
            transport.provider.provider_type.as_str(),
            transport.endpoint.body_rules.as_ref(),
            Some(input.auth_context.api_key_id.as_str()),
        )
    } else {
        build_local_openai_cli_request_body(
            body_json,
            &mapped_model,
            upstream_is_stream,
            transport.provider.provider_type.as_str(),
            provider_api_format,
            transport.endpoint.body_rules.as_ref(),
            Some(input.auth_context.api_key_id.as_str()),
        )
    }) else {
        mark_skipped_local_openai_cli_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "provider_request_body_missing",
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
                mark_skipped_local_openai_cli_candidate(
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
                mark_skipped_local_openai_cli_candidate(
                    state,
                    input,
                    trace_id,
                    candidate,
                    candidate_index,
                    candidate_id,
                    "provider_request_body_missing",
                )
                .await;
                return None;
            }
        }
    } else {
        base_provider_request_body
    };

    let Some(upstream_url) = (if needs_bidirectional_conversion {
        build_cross_format_openai_cli_upstream_url(
            parts,
            transport,
            &mapped_model,
            spec_metadata.api_format,
            provider_api_format,
            upstream_is_stream,
        )
    } else {
        build_local_openai_cli_upstream_url(
            parts,
            transport,
            provider_api_format == "openai:compact",
        )
    }) else {
        mark_skipped_local_openai_cli_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "upstream_url_missing",
        )
        .await;
        return None;
    };

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
    if !apply_local_header_rules(
        &mut provider_request_headers,
        transport.endpoint.header_rules.as_ref(),
        &[&auth_header, "content-type"],
        &provider_request_body,
        Some(body_json),
    ) {
        mark_skipped_local_openai_cli_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "transport_header_rules_apply_failed",
        )
        .await;
        return None;
    }
    apply_codex_openai_cli_special_headers(
        &mut provider_request_headers,
        &provider_request_body,
        &parts.headers,
        transport.provider.provider_type.as_str(),
        provider_api_format,
        Some(trace_id),
        transport.key.decrypted_auth_config.as_deref(),
    );
    ensure_upstream_auth_header(&mut provider_request_headers, &auth_header, &auth_value);
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
        event_name = "local_openai_cli_upstream_url_resolved",
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
        "gateway resolved local openai cli upstream url"
    );

    Some(LocalOpenAiCliCandidatePayloadParts {
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
        upstream_is_stream,
        transport: transport.clone(),
    })
}
