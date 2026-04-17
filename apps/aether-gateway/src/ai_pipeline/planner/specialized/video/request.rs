use std::collections::BTreeMap;

use serde_json::Value;

use crate::ai_pipeline::planner::candidate_preparation::resolve_candidate_mapped_model;
use crate::ai_pipeline::planner::spec_metadata::local_video_create_spec_metadata;
use crate::ai_pipeline::transport::auth::{
    build_passthrough_headers_with_auth, resolve_local_gemini_auth,
    resolve_local_openai_bearer_auth,
};
use crate::ai_pipeline::transport::url::{
    build_gemini_video_predict_long_running_url, build_passthrough_path_url,
};
use crate::ai_pipeline::transport::{
    apply_local_body_rules, apply_local_header_rules,
    local_gemini_transport_unsupported_reason_with_network,
    local_standard_transport_unsupported_reason_with_network,
};
use crate::ai_pipeline::GatewayProviderTransportSnapshot;
use crate::AppState;

use super::support::{
    mark_skipped_local_video_candidate, LocalVideoCreateCandidateAttempt,
    LocalVideoCreateDecisionInput,
};
use super::{LocalVideoCreateFamily, LocalVideoCreateSpec};

pub(super) struct LocalVideoCreateCandidatePayloadParts {
    pub(super) transport: GatewayProviderTransportSnapshot,
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

    let transport_unsupported_reason = match spec.family {
        LocalVideoCreateFamily::OpenAi => local_standard_transport_unsupported_reason_with_network(
            transport,
            spec_metadata.api_format,
        ),
        LocalVideoCreateFamily::Gemini => local_gemini_transport_unsupported_reason_with_network(
            transport,
            spec_metadata.api_format,
        ),
    };
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

    let auth = match spec.family {
        LocalVideoCreateFamily::OpenAi => resolve_local_openai_bearer_auth(transport),
        LocalVideoCreateFamily::Gemini => resolve_local_gemini_auth(transport),
    };
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

    let Some(upstream_url) = build_video_upstream_url(parts, transport, &mapped_model, spec.family)
    else {
        mark_skipped_local_video_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "upstream_url_missing",
        )
        .await;
        return None;
    };

    let Some(provider_request_body) = build_provider_request_body(
        body_json,
        spec.family,
        &mapped_model,
        transport.endpoint.body_rules.as_ref(),
    ) else {
        mark_skipped_local_video_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "provider_request_body_missing",
        )
        .await;
        return None;
    };

    let mut provider_request_headers = build_passthrough_headers_with_auth(
        &parts.headers,
        &auth_header,
        &auth_value,
        &BTreeMap::new(),
    );
    if !apply_local_header_rules(
        &mut provider_request_headers,
        transport.endpoint.header_rules.as_ref(),
        &[&auth_header, "content-type"],
        &provider_request_body,
        Some(body_json),
    ) {
        mark_skipped_local_video_candidate(
            state,
            input,
            trace_id,
            candidate,
            attempt.candidate_index,
            &attempt.candidate_id,
            "transport_header_rules_apply_failed",
        )
        .await;
        return None;
    }

    Some(LocalVideoCreateCandidatePayloadParts {
        transport: transport.clone(),
        auth_header,
        auth_value,
        mapped_model,
        provider_request_headers,
        provider_request_body,
        upstream_url,
    })
}

fn build_provider_request_body(
    body_json: &serde_json::Value,
    family: LocalVideoCreateFamily,
    mapped_model: &str,
    body_rules: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let mut provider_request_body = match family {
        LocalVideoCreateFamily::OpenAi => {
            let mut provider_request_body = body_json.as_object().cloned().unwrap_or_default();
            provider_request_body
                .insert("model".to_string(), Value::String(mapped_model.to_string()));
            serde_json::Value::Object(provider_request_body)
        }
        LocalVideoCreateFamily::Gemini => body_json.clone(),
    };
    if !apply_local_body_rules(&mut provider_request_body, body_rules, Some(body_json)) {
        return None;
    }
    Some(provider_request_body)
}

fn build_video_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    family: LocalVideoCreateFamily,
) -> Option<String> {
    let custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(path) = custom_path {
        let blocked_keys = match family {
            LocalVideoCreateFamily::OpenAi => &[][..],
            LocalVideoCreateFamily::Gemini => &["key"][..],
        };
        return build_passthrough_path_url(
            &transport.endpoint.base_url,
            path,
            parts.uri.query(),
            blocked_keys,
        );
    }

    match family {
        LocalVideoCreateFamily::OpenAi => build_passthrough_path_url(
            &transport.endpoint.base_url,
            parts.uri.path(),
            parts.uri.query(),
            &[],
        ),
        LocalVideoCreateFamily::Gemini => build_gemini_video_predict_long_running_url(
            &transport.endpoint.base_url,
            mapped_model,
            parts.uri.query(),
        ),
    }
}
