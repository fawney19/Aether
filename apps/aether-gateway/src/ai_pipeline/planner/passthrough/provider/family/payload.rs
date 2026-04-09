use std::collections::BTreeMap;

use serde_json::json;

use crate::ai_pipeline::transport::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestEnvelopeSupport, AntigravityRequestSideSupport,
};
use crate::ai_pipeline::transport::auth::build_openai_passthrough_headers;
use crate::ai_pipeline::transport::claude_code::build_claude_code_passthrough_headers;
use crate::ai_pipeline::transport::kiro::{
    build_kiro_provider_headers, KiroProviderHeadersInput, KIRO_ENVELOPE_NAME,
};
use crate::ai_pipeline::transport::{
    apply_local_header_rules, ensure_upstream_auth_header, resolve_transport_execution_timeouts,
    resolve_transport_proxy_snapshot_with_tunnel_affinity, resolve_transport_tls_profile,
};
use crate::ai_pipeline::{
    collect_control_headers, ConversionMode, ExecutionStrategy, PlannerAppState,
};
use crate::clock::current_unix_secs;
use crate::{
    append_execution_contract_fields_to_value, AppState, GatewayControlSyncDecisionResponse,
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, EXECUTION_RUNTIME_SYNC_DECISION_ACTION,
};
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

#[path = "payload/prepare.rs"]
mod prepare;

use self::prepare::{
    prepare_local_same_format_provider_candidate, PreparedSameFormatProviderCandidate,
};
use super::{
    LocalSameFormatProviderCandidateAttempt, LocalSameFormatProviderDecisionInput,
    LocalSameFormatProviderSpec,
};

pub(crate) async fn maybe_build_local_same_format_provider_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalSameFormatProviderDecisionInput,
    attempt: LocalSameFormatProviderCandidateAttempt,
    spec: LocalSameFormatProviderSpec,
) -> Option<GatewayControlSyncDecisionResponse> {
    let planner_state = PlannerAppState::new(state);
    let LocalSameFormatProviderCandidateAttempt {
        candidate,
        candidate_index,
        candidate_id,
    } = attempt;

    let PreparedSameFormatProviderCandidate {
        transport,
        is_antigravity,
        is_claude_code,
        is_vertex,
        is_kiro,
        kiro_auth,
        auth_header,
        auth_value,
        mapped_model,
        report_kind,
        upstream_is_stream,
    } = prepare_local_same_format_provider_candidate(
        planner_state.app(),
        trace_id,
        input,
        &candidate,
        candidate_index,
        &candidate_id,
        spec,
    )
    .await?;

    let Some(base_provider_request_body) =
        super::super::request::build_same_format_provider_request_body(
            body_json,
            &mapped_model,
            spec,
            transport.endpoint.body_rules.as_ref(),
            upstream_is_stream,
            kiro_auth.as_ref(),
            is_claude_code,
        )
    else {
        mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            &candidate,
            candidate_index,
            &candidate_id,
            "provider_request_body_missing",
        )
        .await;
        return None;
    };

    let antigravity_auth = if is_antigravity {
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
                    &candidate,
                    candidate_index,
                    &candidate_id,
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
                mark_skipped_local_same_format_provider_candidate(
                    state,
                    input,
                    trace_id,
                    &candidate,
                    candidate_index,
                    &candidate_id,
                    "provider_request_body_missing",
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
        &transport,
        &mapped_model,
        spec,
        upstream_is_stream,
        kiro_auth.as_ref(),
    ) else {
        mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            &candidate,
            candidate_index,
            &candidate_id,
            "upstream_url_missing",
        )
        .await;
        return None;
    };

    let Some(provider_request_headers) = (if let Some(kiro_auth) = kiro_auth.as_ref() {
        build_kiro_provider_headers(KiroProviderHeadersInput {
            headers: &parts.headers,
            provider_request_body: &provider_request_body,
            original_request_body: body_json,
            header_rules: transport.endpoint.header_rules.as_ref(),
            auth_header: auth_header.as_deref().unwrap_or_default(),
            auth_value: auth_value.as_deref().unwrap_or_default(),
            auth_config: &kiro_auth.auth_config,
            machine_id: kiro_auth.machine_id.as_str(),
        })
    } else {
        let extra_headers = antigravity_auth
            .as_ref()
            .map(build_antigravity_static_identity_headers)
            .unwrap_or_default();
        let mut provider_request_headers = if is_claude_code {
            build_claude_code_passthrough_headers(
                &parts.headers,
                auth_header.as_deref().unwrap_or_default(),
                auth_value.as_deref().unwrap_or_default(),
                &extra_headers,
                upstream_is_stream,
                transport.key.fingerprint.as_ref(),
            )
        } else if is_vertex {
            crate::ai_pipeline::transport::build_passthrough_headers(
                &parts.headers,
                &extra_headers,
                Some("application/json"),
            )
        } else {
            build_openai_passthrough_headers(
                &parts.headers,
                auth_header.as_deref().unwrap_or_default(),
                auth_value.as_deref().unwrap_or_default(),
                &extra_headers,
                Some("application/json"),
            )
        };
        let protected_headers = auth_header
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| vec![value, "content-type"])
            .unwrap_or_else(|| vec!["content-type"]);
        if !apply_local_header_rules(
            &mut provider_request_headers,
            transport.endpoint.header_rules.as_ref(),
            &protected_headers,
            &provider_request_body,
            Some(body_json),
        ) {
            None
        } else {
            if let (Some(auth_header), Some(auth_value)) =
                (auth_header.as_deref(), auth_value.as_deref())
            {
                ensure_upstream_auth_header(&mut provider_request_headers, auth_header, auth_value);
            }
            if upstream_is_stream {
                provider_request_headers
                    .insert("accept".to_string(), "text/event-stream".to_string());
            }
            Some(provider_request_headers)
        }
    }) else {
        mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            &candidate,
            candidate_index,
            &candidate_id,
            "transport_header_rules_apply_failed",
        )
        .await;
        return None;
    };

    let prompt_cache_key = provider_request_body
        .get("prompt_cache_key")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let proxy =
        resolve_transport_proxy_snapshot_with_tunnel_affinity(planner_state.app(), &transport)
            .await;
    let tls_profile = resolve_transport_tls_profile(&transport);
    let report_context = append_execution_contract_fields_to_value(
        json!({
            "user_id": input.auth_context.user_id,
            "api_key_id": input.auth_context.api_key_id,
            "request_id": trace_id,
            "candidate_id": candidate_id,
            "candidate_index": candidate_index,
            "retry_index": 0,
            "model": input.requested_model,
            "provider_name": transport.provider.name,
            "provider_id": candidate.provider_id,
            "endpoint_id": candidate.endpoint_id,
            "key_id": candidate.key_id,
            "key_name": candidate.key_name,
            "provider_api_format": spec.api_format,
            "client_api_format": spec.api_format,
            "mapped_model": mapped_model,
            "upstream_url": upstream_url,
            "provider_request_method": serde_json::Value::Null,
            "provider_request_headers": provider_request_headers,
            "provider_request_body": provider_request_body,
            "original_headers": collect_control_headers(&parts.headers),
            "original_request_body": body_json,
            "has_envelope": is_kiro || is_antigravity,
            "envelope_name": if is_kiro {
                Some(KIRO_ENVELOPE_NAME)
            } else if is_antigravity {
                Some(super::super::ANTIGRAVITY_ENVELOPE_NAME)
            } else {
                None
            },
            "needs_conversion": false,
        }),
        ExecutionStrategy::LocalSameFormat,
        ConversionMode::None,
        spec.api_format,
        spec.api_format,
    );

    Some(GatewayControlSyncDecisionResponse {
        action: if spec.require_streaming {
            EXECUTION_RUNTIME_STREAM_DECISION_ACTION.to_string()
        } else {
            EXECUTION_RUNTIME_SYNC_DECISION_ACTION.to_string()
        },
        decision_kind: Some(spec.decision_kind.to_string()),
        execution_strategy: Some(ExecutionStrategy::LocalSameFormat.as_str().to_string()),
        conversion_mode: Some(ConversionMode::None.as_str().to_string()),
        request_id: Some(trace_id.to_string()),
        candidate_id: Some(candidate_id.clone()),
        provider_name: Some(transport.provider.name.clone()),
        provider_id: Some(candidate.provider_id.clone()),
        endpoint_id: Some(candidate.endpoint_id.clone()),
        key_id: Some(candidate.key_id.clone()),
        upstream_base_url: Some(transport.endpoint.base_url.clone()),
        upstream_url: Some(upstream_url.clone()),
        provider_request_method: None,
        auth_header,
        auth_value,
        provider_api_format: Some(spec.api_format.to_string()),
        client_api_format: Some(spec.api_format.to_string()),
        provider_contract: Some(spec.api_format.to_string()),
        client_contract: Some(spec.api_format.to_string()),
        model_name: Some(input.requested_model.clone()),
        mapped_model: Some(mapped_model.clone()),
        prompt_cache_key,
        extra_headers: BTreeMap::new(),
        provider_request_headers: provider_request_headers.clone(),
        provider_request_body: Some(provider_request_body.clone()),
        provider_request_body_base64: None,
        content_type: Some("application/json".to_string()),
        proxy,
        tls_profile,
        timeouts: resolve_transport_execution_timeouts(&transport),
        upstream_is_stream,
        report_kind: Some(report_kind.to_string()),
        report_context: Some(report_context),
        auth_context: Some(input.auth_context.clone()),
    })
}

pub(super) async fn mark_skipped_local_same_format_provider_candidate(
    state: &AppState,
    input: &LocalSameFormatProviderDecisionInput,
    trace_id: &str,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    skip_reason: &'static str,
) {
    PlannerAppState::new(state)
        .persist_skipped_local_candidate(
            trace_id,
            &input.auth_context.user_id,
            &input.auth_context.api_key_id,
            candidate,
            candidate_index,
            candidate_id,
            skip_reason,
            current_unix_secs(),
            "gateway local same-format decision failed to persist skipped candidate",
        )
        .await;
}
