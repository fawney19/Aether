use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::ai_serving::planner::candidate_preparation::resolve_candidate_mapped_model;
use crate::ai_serving::planner::spec_metadata::local_video_create_spec_metadata;
use crate::ai_serving::transport::{
    build_video_create_headers, build_video_create_request_body, build_video_create_upstream_url,
    resolve_video_create_auth, video_create_transport_unsupported_reason,
    ProviderVideoCreateFamily, ProviderVideoCreateHeadersInput,
};
use crate::ai_serving::{CandidateFailureDiagnostic, GatewayProviderTransportSnapshot};
use crate::AppState;

use super::support::{
    mark_skipped_local_video_candidate, mark_skipped_local_video_candidate_with_failure_diagnostic,
    LocalVideoCreateCandidateAttempt, LocalVideoCreateDecisionInput,
};
use super::{LocalVideoCreateFamily, LocalVideoCreateSpec};

pub(super) struct LocalVideoCreateCandidatePayloadParts {
    pub(super) transport: Arc<GatewayProviderTransportSnapshot>,
    pub(super) auth_header: String,
    pub(super) auth_value: String,
    pub(super) mapped_model: String,
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) provider_request_body: Value,
    pub(super) upstream_url: String,
}

pub(super) async fn resolve_local_video_create_candidate_payload_parts(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    input: &LocalVideoCreateDecisionInput,
    attempt: &LocalVideoCreateCandidateAttempt,
    spec: LocalVideoCreateSpec,
) -> Option<LocalVideoCreateCandidatePayloadParts> {
    let spec_metadata = local_video_create_spec_metadata(spec);
    let candidate = &attempt.eligible.candidate;
    let transport = &attempt.eligible.transport;

    let provider_family = provider_video_create_family(spec.family);
    let transport_unsupported_reason = video_create_transport_unsupported_reason(
        transport,
        provider_family,
        spec_metadata.api_format,
    );
    if let Some(skip_reason) = transport_unsupported_reason {
        mark_skipped_local_video_candidate(
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

    let auth = resolve_video_create_auth(transport, provider_family);
    let Some((auth_header, auth_value)) = auth else {
        mark_skipped_local_video_candidate(
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
    };

    let mapped_model = match resolve_candidate_mapped_model(candidate) {
        Ok(mapped_model) => mapped_model,
        Err(skip_reason) => {
            mark_skipped_local_video_candidate(
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

    let Some(upstream_url) = build_video_create_upstream_url(
        transport,
        parts.uri.path(),
        parts.uri.query(),
        &mapped_model,
        provider_family,
    ) else {
        mark_skipped_local_video_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "upstream_url_missing",
            CandidateFailureDiagnostic::upstream_url_missing(
                spec_metadata.api_format,
                spec_metadata.api_format,
                "video_upstream_url",
            ),
        )
        .await;
        return None;
    };

    let Some(provider_request_body) = build_video_create_request_body(
        body_json,
        provider_family,
        &mapped_model,
        transport.endpoint.body_rules.as_ref(),
    ) else {
        mark_skipped_local_video_candidate_with_failure_diagnostic(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_body_rules_apply_failed",
            CandidateFailureDiagnostic::body_rules_apply_failed(
                spec_metadata.api_format,
                spec_metadata.api_format,
                "video_body_rules",
            ),
        )
        .await;
        return None;
    };

    let Some(provider_request_headers) =
        build_video_create_headers(ProviderVideoCreateHeadersInput {
            headers: &parts.headers,
            auth_header: &auth_header,
            auth_value: &auth_value,
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
        })
    else {
        mark_skipped_local_video_candidate_with_failure_diagnostic(
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
                "video_header_rules",
            ),
        )
        .await;
        return None;
    };

    Some(LocalVideoCreateCandidatePayloadParts {
        transport: Arc::clone(transport),
        auth_header,
        auth_value,
        mapped_model,
        provider_request_headers,
        provider_request_body,
        upstream_url,
    })
}

fn provider_video_create_family(family: LocalVideoCreateFamily) -> ProviderVideoCreateFamily {
    match family {
        LocalVideoCreateFamily::OpenAi => ProviderVideoCreateFamily::OpenAi,
        LocalVideoCreateFamily::Gemini => ProviderVideoCreateFamily::Gemini,
    }
}
