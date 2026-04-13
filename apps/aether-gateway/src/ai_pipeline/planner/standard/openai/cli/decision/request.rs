use std::collections::BTreeMap;

use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::Value;
use tracing::{debug, warn};

use crate::ai_pipeline::conversion::{
    request_conversion_direct_auth, request_conversion_kind,
    request_conversion_requires_enable_flag, request_conversion_transport_supported,
    request_pair_allowed_for_transport,
};
use crate::ai_pipeline::planner::common::force_upstream_streaming_for_provider;
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
    resolve_local_standard_auth,
};
use crate::ai_pipeline::transport::policy::supports_local_standard_transport_with_network;
use crate::ai_pipeline::{ConversionMode, ExecutionStrategy};
use crate::ai_pipeline::{
    GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth, PlannerAppState,
};
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
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    spec: LocalOpenAiCliSpec,
) -> Option<LocalOpenAiCliCandidatePayloadParts> {
    let planner_state = PlannerAppState::new(state);
    let provider_api_format = candidate.endpoint_api_format.trim().to_ascii_lowercase();

    let transport = match planner_state
        .read_provider_transport_snapshot(
            &candidate.provider_id,
            &candidate.endpoint_id,
            &candidate.key_id,
        )
        .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            mark_skipped_local_openai_cli_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_snapshot_missing",
            )
            .await;
            return None;
        }
        Err(err) => {
            warn!(
                trace_id = %trace_id,
                api_format = spec.api_format,
                error = ?err,
                "gateway local openai cli decision provider transport read failed"
            );
            mark_skipped_local_openai_cli_candidate(
                state,
                input,
                trace_id,
                candidate,
                candidate_index,
                candidate_id,
                "transport_snapshot_read_failed",
            )
            .await;
            return None;
        }
    };
    let is_antigravity = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("antigravity");

    let same_format = provider_api_format == spec.api_format.trim().to_ascii_lowercase();
    let conversion_kind = request_conversion_kind(spec.api_format, provider_api_format.as_str());
    if !same_format
        && !request_pair_allowed_for_transport(
            &transport,
            spec.api_format,
            provider_api_format.as_str(),
        )
    {
        let skip_reason = if conversion_kind.is_some()
            && request_conversion_requires_enable_flag(
                spec.api_format,
                provider_api_format.as_str(),
            )
            && !transport.provider.enable_format_conversion
        {
            "format_conversion_disabled"
        } else {
            "transport_unsupported"
        };
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
    let transport_supported = if same_format {
        supports_local_standard_transport_with_network(&transport, provider_api_format.as_str())
    } else {
        match conversion_kind {
            Some(_) if is_antigravity && provider_api_format == "gemini:cli" => true,
            Some(kind) => request_conversion_transport_supported(&transport, kind),
            None => false,
        }
    };
    if !transport_supported {
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

    let resolved_auth = if same_format {
        match provider_api_format.as_str() {
            "gemini:cli" => resolve_local_gemini_auth(&transport),
            "claude:cli" | "openai:cli" | "openai:compact" => {
                resolve_local_standard_auth(&transport)
            }
            _ => None,
        }
    } else {
        conversion_kind.and_then(|kind| request_conversion_direct_auth(&transport, kind))
    };
    let oauth_auth = if resolved_auth.is_none() {
        match planner_state
            .resolve_local_oauth_request_auth(&transport)
            .await
        {
            Ok(Some(LocalResolvedOAuthRequestAuth::Header { name, value })) => Some((name, value)),
            Ok(Some(LocalResolvedOAuthRequestAuth::Kiro(_))) => None,
            Ok(None) => None,
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec.api_format,
                    provider_type = %transport.provider.provider_type,
                    error = ?err,
                    "gateway local openai cli oauth auth resolution failed"
                );
                None
            }
        }
    } else {
        None
    };

    let Some((auth_header, auth_value)) = resolved_auth.or(oauth_auth) else {
        mark_skipped_local_openai_cli_candidate(
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
    };

    let mapped_model = candidate.selected_provider_model_name.trim().to_string();
    if mapped_model.is_empty() {
        mark_skipped_local_openai_cli_candidate(
            state,
            input,
            trace_id,
            candidate,
            candidate_index,
            candidate_id,
            "mapped_model_missing",
        )
        .await;
        return None;
    }

    let needs_bidirectional_conversion = !same_format && conversion_kind.is_some();
    let upstream_is_stream = spec.require_streaming
        || is_antigravity
        || force_upstream_streaming_for_provider(
            transport.provider.provider_type.as_str(),
            provider_api_format.as_str(),
        );
    let Some(base_provider_request_body) = (if needs_bidirectional_conversion {
        build_cross_format_openai_cli_request_body(
            body_json,
            &mapped_model,
            spec.api_format,
            provider_api_format.as_str(),
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
            provider_api_format.as_str(),
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
            &transport,
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
            &transport,
            &mapped_model,
            spec.api_format,
            provider_api_format.as_str(),
            upstream_is_stream,
        )
    } else {
        build_local_openai_cli_upstream_url(
            parts,
            &transport,
            provider_api_format.as_str() == "openai:compact",
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
        provider_api_format.as_str(),
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
        client_api_format = spec.api_format,
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
        provider_api_format,
        provider_request_body,
        provider_request_headers,
        upstream_url,
        execution_strategy,
        conversion_mode,
        is_antigravity: is_antigravity
            || antigravity_auth.is_some() && ANTIGRAVITY_ENVELOPE_NAME == "antigravity:v1internal",
        upstream_is_stream,
        transport,
    })
}
