use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::ai_serving::transport::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestEnvelopeSupport, AntigravityRequestSideSupport,
};
use crate::ai_serving::transport::{
    build_same_format_provider_headers, SameFormatProviderHeadersInput,
};
use crate::ai_serving::{CandidateFailureDiagnostic, GatewayProviderTransportSnapshot};
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
use crate::ai_serving::planner::standard::same_format_provider_request_body_failure_extra_data;

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

    let extra_headers = antigravity_auth
        .as_ref()
        .map(build_antigravity_static_identity_headers)
        .unwrap_or_default();
    let Some(provider_request_headers) =
        build_same_format_provider_headers(SameFormatProviderHeadersInput {
            headers: &parts.headers,
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
            header_rules: prepared.transport.endpoint.header_rules.as_ref(),
            behavior: prepared.behavior,
            auth_header: prepared.auth_header.as_deref(),
            auth_value: prepared.auth_value.as_deref(),
            extra_headers: &extra_headers,
            key_fingerprint: prepared.transport.key.fingerprint.as_ref(),
            kiro_auth_config: prepared.kiro_auth.as_ref().map(|auth| &auth.auth_config),
            kiro_machine_id: prepared
                .kiro_auth
                .as_ref()
                .map(|auth| auth.machine_id.as_str()),
        })
    else {
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
