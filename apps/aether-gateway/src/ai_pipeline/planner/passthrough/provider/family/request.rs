use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::ai_pipeline::transport::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestEnvelopeSupport, AntigravityRequestSideSupport,
};
use crate::ai_pipeline::transport::auth::{
    build_complete_passthrough_headers, build_complete_passthrough_headers_with_auth,
};
use crate::ai_pipeline::transport::claude_code::build_claude_code_passthrough_headers;
use crate::ai_pipeline::transport::kiro::{build_kiro_provider_headers, KiroProviderHeadersInput};
use crate::ai_pipeline::transport::{apply_local_header_rules, ensure_upstream_auth_header};
use crate::ai_pipeline::{CandidateFailureDiagnostic, GatewayProviderTransportSnapshot};
use crate::AppState;

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
use crate::ai_pipeline::planner::standard::same_format_provider_request_body_failure_extra_data;

pub(crate) fn resolve_same_format_provider_transport_unsupported_reason_for_trace(
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> Option<&'static str> {
    let provider_api_format = match crate::ai_pipeline::normalize_legacy_openai_format_alias(
        provider_api_format,
    )
    .as_str()
    {
        "openai:chat" => "openai:chat",
        "openai:responses" => "openai:responses",
        "openai:responses:compact" => "openai:responses:compact",
        "claude:chat" => "claude:chat",
        "claude:cli" => "claude:cli",
        "gemini:chat" => "gemini:chat",
        "gemini:cli" => "gemini:cli",
        _ => return Some("transport_api_format_unsupported"),
    };
    let behavior = policy::classify_same_format_provider_request_behavior(
        transport,
        crate::ai_pipeline::planner::spec_metadata::LocalExecutionSurfaceSpecMetadata {
            api_format: provider_api_format,
            require_streaming: false,
            requested_model_family: None,
            decision_kind: "trace_candidate_metadata",
            report_kind: Some("trace_candidate_metadata"),
        },
    );
    if !behavior.is_antigravity
        && !behavior.is_claude_code
        && !behavior.is_vertex
        && !behavior.is_kiro
    {
        return None;
    }

    let family = if provider_api_format.starts_with("gemini:") {
        crate::ai_pipeline::LocalSameFormatProviderFamily::Gemini
    } else {
        crate::ai_pipeline::LocalSameFormatProviderFamily::Standard
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
    pub(super) is_kiro: bool,
    pub(super) auth_header: Option<String>,
    pub(super) auth_value: Option<String>,
    pub(super) mapped_model: String,
    pub(super) report_kind: &'static str,
    pub(super) upstream_is_stream: bool,
    pub(super) upstream_url: String,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) provider_request_body: Value,
}

pub(crate) async fn resolve_local_same_format_provider_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalSameFormatProviderDecisionInput,
    attempt: &LocalSameFormatProviderCandidateAttempt,
    spec: LocalSameFormatProviderSpec,
) -> Option<LocalSameFormatProviderCandidatePayloadParts> {
    let candidate = &attempt.eligible.candidate;
    let prepared = prepare_local_same_format_provider_candidate(
        state,
        trace_id,
        input,
        &attempt.eligible,
        attempt.candidate_index,
        &attempt.candidate_id,
        spec,
    )
    .await?;

    let Some(base_provider_request_body) =
        super::super::request::build_same_format_provider_request_body(
            body_json,
            &prepared.mapped_model,
            spec,
            prepared.transport.endpoint.body_rules.as_ref(),
            prepared.upstream_is_stream,
            prepared.kiro_auth.as_ref(),
            prepared.is_claude_code,
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
        return None;
    };

    let antigravity_auth = if prepared.is_antigravity {
        match classify_local_antigravity_request_support(
            &prepared.transport,
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
                return None;
            }
        }
    } else {
        base_provider_request_body
    };

    let Some(upstream_url) = super::super::request::build_same_format_upstream_url(
        parts,
        &prepared.transport,
        &prepared.mapped_model,
        spec,
        prepared.upstream_is_stream,
        prepared.kiro_auth.as_ref(),
    ) else {
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
        return None;
    };

    let Some(provider_request_headers) = (if let Some(kiro_auth) = prepared.kiro_auth.as_ref() {
        build_kiro_provider_headers(KiroProviderHeadersInput {
            headers: &parts.headers,
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
            header_rules: prepared.transport.endpoint.header_rules.as_ref(),
            auth_header: prepared.auth_header.as_deref().unwrap_or_default(),
            auth_value: prepared.auth_value.as_deref().unwrap_or_default(),
            auth_config: &kiro_auth.auth_config,
            machine_id: kiro_auth.machine_id.as_str(),
        })
    } else {
        let extra_headers = antigravity_auth
            .as_ref()
            .map(build_antigravity_static_identity_headers)
            .unwrap_or_default();
        let mut provider_request_headers = if prepared.is_claude_code {
            build_claude_code_passthrough_headers(
                &parts.headers,
                prepared.auth_header.as_deref().unwrap_or_default(),
                prepared.auth_value.as_deref().unwrap_or_default(),
                &extra_headers,
                prepared.upstream_is_stream,
                prepared.transport.key.fingerprint.as_ref(),
            )
        } else if prepared.is_vertex {
            build_complete_passthrough_headers(
                &parts.headers,
                &extra_headers,
                Some("application/json"),
            )
        } else {
            build_complete_passthrough_headers_with_auth(
                &parts.headers,
                prepared.auth_header.as_deref().unwrap_or_default(),
                prepared.auth_value.as_deref().unwrap_or_default(),
                &extra_headers,
                Some("application/json"),
            )
        };
        let protected_headers = prepared
            .auth_header
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| vec![value, "content-type"])
            .unwrap_or_else(|| vec!["content-type"]);
        if !apply_local_header_rules(
            &mut provider_request_headers,
            prepared.transport.endpoint.header_rules.as_ref(),
            &protected_headers,
            &provider_request_body,
            Some(body_json),
        ) {
            None
        } else {
            if let (Some(auth_header), Some(auth_value)) = (
                prepared.auth_header.as_deref(),
                prepared.auth_value.as_deref(),
            ) {
                ensure_upstream_auth_header(&mut provider_request_headers, auth_header, auth_value);
            }
            if prepared.upstream_is_stream {
                provider_request_headers
                    .insert("accept".to_string(), "text/event-stream".to_string());
            }
            Some(provider_request_headers)
        }
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
        return None;
    };

    Some(LocalSameFormatProviderCandidatePayloadParts {
        transport: prepared.transport,
        is_antigravity: prepared.is_antigravity,
        is_kiro: prepared.is_kiro,
        auth_header: prepared.auth_header,
        auth_value: prepared.auth_value,
        mapped_model: prepared.mapped_model,
        report_kind: prepared.report_kind,
        upstream_is_stream: prepared.upstream_is_stream,
        upstream_url,
        provider_request_headers,
        provider_request_body,
    })
}
