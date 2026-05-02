use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::ai_serving::planner::candidate_preparation::{
    prepare_header_authenticated_candidate, OauthPreparationContext,
};
use crate::ai_serving::planner::spec_metadata::local_openai_image_spec_metadata;
use crate::ai_serving::transport::{
    build_openai_image_headers, build_openai_image_upstream_url,
    openai_image_transport_unsupported_reason, resolve_openai_image_auth,
    ProviderOpenAiImageHeadersInput,
};
use crate::ai_serving::{
    apply_codex_openai_responses_special_body_edits, apply_codex_openai_responses_special_headers,
    build_openai_image_provider_request_body, default_model_for_openai_image_operation,
    normalize_openai_image_request, CandidateFailureDiagnostic, GatewayProviderTransportSnapshot,
    PlannerAppState,
};
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
    pub(super) provider_request_headers: BTreeMap<String, String>,
    pub(super) provider_request_body: Value,
    pub(super) upstream_url: String,
    pub(super) input_summary: Value,
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
                spec_metadata.api_format,
                "openai_image_request_normalize",
            ),
        )
        .await;
        return None;
    };

    let upstream_url = build_openai_image_upstream_url(transport, parts.uri.query());
    let mut provider_request_body = build_openai_image_provider_request_body(&normalized_request);
    apply_codex_openai_responses_special_body_edits(
        &mut provider_request_body,
        transport.provider.provider_type.as_str(),
        spec_metadata.api_format,
        transport.endpoint.body_rules.as_ref(),
        Some(candidate.key_id.as_str()),
    );

    let Some(mut provider_request_headers) =
        build_openai_image_headers(ProviderOpenAiImageHeadersInput {
            headers: &parts.headers,
            auth_header: &auth_header,
            auth_value: &auth_value,
            header_rules: transport.endpoint.header_rules.as_ref(),
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
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
                spec_metadata.api_format,
                "openai_image_header_rules",
            ),
        )
        .await;
        return None;
    };
    apply_codex_openai_responses_special_headers(
        &mut provider_request_headers,
        &provider_request_body,
        &parts.headers,
        transport.provider.provider_type.as_str(),
        spec_metadata.api_format,
        Some(trace_id),
        transport.key.decrypted_auth_config.as_deref(),
    );
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

    Some(LocalOpenAiImageCandidatePayloadParts {
        transport: Arc::clone(transport),
        auth_header,
        auth_value,
        requested_model,
        mapped_model,
        provider_request_headers,
        provider_request_body,
        upstream_url,
        input_summary: normalized_request.summary_json,
    })
}
