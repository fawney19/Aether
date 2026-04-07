use std::collections::BTreeMap;

use serde_json::json;
use tracing::warn;

use crate::ai_pipeline::control_facade::collect_control_headers;
use crate::ai_pipeline::execution_facade::{ConversionMode, ExecutionStrategy};
use crate::ai_pipeline::planner::candidate_runtime_facade::persist_skipped_local_candidate;
use crate::ai_pipeline::planner::transport_facade::{
    read_provider_transport_snapshot, resolve_local_oauth_request_auth,
    LocalResolvedOAuthRequestAuth,
};
use crate::ai_pipeline::provider_transport_facade::antigravity::{
    build_antigravity_safe_v1internal_request, build_antigravity_static_identity_headers,
    classify_local_antigravity_request_support, AntigravityEnvelopeRequestType,
    AntigravityRequestEnvelopeSupport, AntigravityRequestSideSupport,
};
use crate::ai_pipeline::provider_transport_facade::auth::{
    build_openai_passthrough_headers, resolve_local_gemini_auth, resolve_local_standard_auth,
};
use crate::ai_pipeline::provider_transport_facade::claude_code::{
    build_claude_code_passthrough_headers, supports_local_claude_code_transport_with_network,
};
use crate::ai_pipeline::provider_transport_facade::kiro::{
    build_kiro_provider_headers, supports_local_kiro_request_transport_with_network,
    KiroProviderHeadersInput, KIRO_ENVELOPE_NAME,
};
use crate::ai_pipeline::provider_transport_facade::policy::{
    supports_local_gemini_transport_with_network, supports_local_standard_transport_with_network,
};
use crate::ai_pipeline::provider_transport_facade::vertex::{
    resolve_local_vertex_api_key_query_auth,
    supports_local_vertex_api_key_gemini_transport_with_network,
};
use crate::ai_pipeline::provider_transport_facade::{
    apply_local_header_rules, build_passthrough_headers, ensure_upstream_auth_header,
    resolve_transport_execution_timeouts, resolve_transport_proxy_snapshot_with_tunnel_affinity,
    resolve_transport_tls_profile,
};
use crate::clock::current_unix_secs;
use crate::{
    append_execution_contract_fields_to_value, AppState, GatewayControlSyncDecisionResponse,
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, EXECUTION_RUNTIME_SYNC_DECISION_ACTION,
};
use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;

use super::types::{
    LocalSameFormatProviderCandidateAttempt, LocalSameFormatProviderDecisionInput,
    LocalSameFormatProviderFamily, LocalSameFormatProviderSpec,
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
    let LocalSameFormatProviderCandidateAttempt {
        candidate,
        candidate_index,
        candidate_id,
    } = attempt;

    let transport = match read_provider_transport_snapshot(
        state,
        &candidate.provider_id,
        &candidate.endpoint_id,
        &candidate.key_id,
    )
    .await
    {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            mark_skipped_local_same_format_provider_candidate(
                state,
                input,
                trace_id,
                &candidate,
                candidate_index,
                &candidate_id,
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
                "gateway local same-format decision provider transport read failed"
            );
            mark_skipped_local_same_format_provider_candidate(
                state,
                input,
                trace_id,
                &candidate,
                candidate_index,
                &candidate_id,
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
    let is_claude_code = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("claude_code");
    let is_vertex = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("vertex_ai");
    let transport_supported = match spec.family {
        _ if transport
            .provider
            .provider_type
            .trim()
            .eq_ignore_ascii_case("kiro") =>
        {
            supports_local_kiro_request_transport_with_network(&transport)
        }
        _ if is_antigravity => true,
        _ if is_claude_code => {
            supports_local_claude_code_transport_with_network(&transport, spec.api_format)
        }
        _ if is_vertex => supports_local_vertex_api_key_gemini_transport_with_network(&transport),
        LocalSameFormatProviderFamily::Standard => {
            supports_local_standard_transport_with_network(&transport, spec.api_format)
        }
        LocalSameFormatProviderFamily::Gemini => {
            supports_local_gemini_transport_with_network(&transport, spec.api_format)
        }
    };
    if !transport_supported {
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

    let is_kiro = transport
        .provider
        .provider_type
        .trim()
        .eq_ignore_ascii_case("kiro");
    let vertex_query_auth = if is_vertex {
        resolve_local_vertex_api_key_query_auth(&transport)
    } else {
        None
    };
    let should_try_oauth_auth = is_kiro
        || matches!(spec.family, LocalSameFormatProviderFamily::Standard)
            && resolve_local_standard_auth(&transport).is_none()
        || matches!(spec.family, LocalSameFormatProviderFamily::Gemini)
            && !is_vertex
            && resolve_local_gemini_auth(&transport).is_none();
    let oauth_auth = if should_try_oauth_auth {
        match resolve_local_oauth_request_auth(state, &transport).await {
            Ok(Some(LocalResolvedOAuthRequestAuth::Kiro(auth))) => {
                Some(LocalResolvedOAuthRequestAuth::Kiro(auth))
            }
            Ok(Some(LocalResolvedOAuthRequestAuth::Header { name, value })) => {
                Some(LocalResolvedOAuthRequestAuth::Header { name, value })
            }
            Ok(None) => None,
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    api_format = spec.api_format,
                    provider_type = %transport.provider.provider_type,
                    error = ?err,
                    "gateway local same-format oauth auth resolution failed"
                );
                None
            }
        }
    } else {
        None
    };
    let kiro_auth = match oauth_auth.as_ref() {
        Some(LocalResolvedOAuthRequestAuth::Kiro(auth)) => Some(auth),
        _ => None,
    };
    let auth = if let Some(auth) = kiro_auth.as_ref() {
        Some((auth.name.to_string(), auth.value.clone()))
    } else if let Some(LocalResolvedOAuthRequestAuth::Header { name, value }) = oauth_auth.as_ref()
    {
        Some((name.clone(), value.clone()))
    } else if is_vertex {
        None
    } else {
        match spec.family {
            LocalSameFormatProviderFamily::Standard => resolve_local_standard_auth(&transport),
            LocalSameFormatProviderFamily::Gemini => resolve_local_gemini_auth(&transport),
        }
    };
    let (auth_header, auth_value) = match auth {
        Some((name, value)) => (Some(name), Some(value)),
        None if is_vertex && vertex_query_auth.is_some() => (None, None),
        None => {
            mark_skipped_local_same_format_provider_candidate(
                state,
                input,
                trace_id,
                &candidate,
                candidate_index,
                &candidate_id,
                "transport_auth_unavailable",
            )
            .await;
            return None;
        }
    };
    if is_vertex && vertex_query_auth.is_none() {
        mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            &candidate,
            candidate_index,
            &candidate_id,
            "transport_auth_unavailable",
        )
        .await;
        return None;
    }
    let mapped_model = candidate.selected_provider_model_name.trim().to_string();
    if mapped_model.is_empty() {
        mark_skipped_local_same_format_provider_candidate(
            state,
            input,
            trace_id,
            &candidate,
            candidate_index,
            &candidate_id,
            "mapped_model_missing",
        )
        .await;
        return None;
    }

    let Some(base_provider_request_body) =
        super::super::request::build_same_format_provider_request_body(
            body_json,
            &mapped_model,
            spec,
            transport.endpoint.body_rules.as_ref(),
            is_kiro || is_antigravity || spec.require_streaming,
            kiro_auth,
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
    let upstream_is_stream = is_kiro || is_antigravity || spec.require_streaming;
    let report_kind = if is_kiro && !spec.require_streaming {
        "claude_cli_sync_finalize"
    } else if is_antigravity && !spec.require_streaming {
        match spec.api_format {
            "gemini:chat" => "gemini_chat_sync_finalize",
            "gemini:cli" => "gemini_cli_sync_finalize",
            _ => spec.report_kind,
        }
    } else {
        spec.report_kind
    };

    let Some(upstream_url) = super::super::request::build_same_format_upstream_url(
        parts,
        &transport,
        &mapped_model,
        spec,
        upstream_is_stream,
        kiro_auth,
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
            build_passthrough_headers(&parts.headers, &extra_headers, Some("application/json"))
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
    let proxy = resolve_transport_proxy_snapshot_with_tunnel_affinity(state, &transport).await;
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
    persist_skipped_local_candidate(
        state,
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
