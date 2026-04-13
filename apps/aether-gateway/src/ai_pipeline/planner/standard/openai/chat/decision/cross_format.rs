use std::collections::BTreeMap;

use aether_scheduler_core::SchedulerMinimalCandidateSelectionCandidate;
use serde_json::json;
use tracing::warn;

use crate::ai_pipeline::collect_control_headers;
use crate::ai_pipeline::conversion::{
    request_conversion_direct_auth, request_conversion_kind,
    request_conversion_requires_enable_flag, request_conversion_transport_supported,
    request_pair_allowed_for_transport,
};
use crate::ai_pipeline::planner::common::OPENAI_CHAT_STREAM_PLAN_KIND;
use crate::ai_pipeline::planner::standard::{
    apply_codex_openai_cli_special_headers, build_cross_format_openai_chat_request_body,
    build_cross_format_openai_chat_upstream_url,
};
use crate::ai_pipeline::transport::auth::{
    build_claude_passthrough_headers, build_openai_passthrough_headers, ensure_upstream_auth_header,
};
use crate::ai_pipeline::transport::{
    apply_local_header_rules, resolve_transport_execution_timeouts,
    resolve_transport_proxy_snapshot_with_tunnel_affinity, resolve_transport_tls_profile,
};
use crate::ai_pipeline::{ConversionMode, ExecutionStrategy, PlannerAppState};
use crate::ai_pipeline::{GatewayProviderTransportSnapshot, LocalResolvedOAuthRequestAuth};
use crate::{
    append_execution_contract_fields_to_value, append_local_failover_policy_to_value, AppState,
    GatewayControlSyncDecisionResponse,
};

use super::support::{mark_skipped_local_openai_chat_candidate, LocalOpenAiChatDecisionInput};

#[allow(clippy::too_many_arguments)]
pub(super) async fn build_cross_format_local_openai_chat_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    body_json: &serde_json::Value,
    input: &LocalOpenAiChatDecisionInput,
    candidate: &SchedulerMinimalCandidateSelectionCandidate,
    candidate_index: u32,
    candidate_id: &str,
    decision_kind: &str,
    upstream_is_stream: bool,
    transport: &GatewayProviderTransportSnapshot,
    provider_api_format: &str,
) -> Option<GatewayControlSyncDecisionResponse> {
    let planner_state = PlannerAppState::new(state);
    let provider_api_format = provider_api_format.trim().to_ascii_lowercase();
    let Some(conversion_kind) =
        request_conversion_kind("openai:chat", provider_api_format.as_str())
    else {
        return None;
    };
    if !request_pair_allowed_for_transport(transport, "openai:chat", provider_api_format.as_str()) {
        let skip_reason =
            if request_conversion_requires_enable_flag("openai:chat", provider_api_format.as_str())
                && !transport.provider.enable_format_conversion
            {
                "format_conversion_disabled"
            } else {
                "transport_unsupported"
            };
        mark_skipped_local_openai_chat_candidate(
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
    if !request_conversion_transport_supported(transport, conversion_kind) {
        mark_skipped_local_openai_chat_candidate(
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
    let resolve_auth = request_conversion_direct_auth(transport, conversion_kind);
    let oauth_auth = if resolve_auth.is_none() {
        match planner_state
            .resolve_local_oauth_request_auth(transport)
            .await
        {
            Ok(Some(LocalResolvedOAuthRequestAuth::Header { name, value })) => Some((name, value)),
            Ok(Some(LocalResolvedOAuthRequestAuth::Kiro(_))) => None,
            Ok(None) => None,
            Err(err) => {
                warn!(
                    trace_id = %trace_id,
                    provider_type = %transport.provider.provider_type,
                    provider_api_format = %provider_api_format,
                    error = ?err,
                    "gateway local openai chat cross-format oauth auth resolution failed"
                );
                None
            }
        }
    } else {
        None
    };

    let Some((auth_header, auth_value)) = resolve_auth.or(oauth_auth) else {
        mark_skipped_local_openai_chat_candidate(
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
        mark_skipped_local_openai_chat_candidate(
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

    let Some(provider_request_body) = build_cross_format_openai_chat_request_body(
        body_json,
        &mapped_model,
        transport.provider.provider_type.as_str(),
        provider_api_format.as_str(),
        upstream_is_stream,
        transport.endpoint.body_rules.as_ref(),
        Some(input.auth_context.api_key_id.as_str()),
    ) else {
        mark_skipped_local_openai_chat_candidate(
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

    let Some(upstream_url) = build_cross_format_openai_chat_upstream_url(
        parts,
        transport,
        &mapped_model,
        provider_api_format.as_str(),
        upstream_is_stream,
    ) else {
        mark_skipped_local_openai_chat_candidate(
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

    let mut provider_request_headers = if provider_api_format.starts_with("claude:") {
        build_claude_passthrough_headers(
            &parts.headers,
            &auth_header,
            &auth_value,
            &BTreeMap::new(),
            Some("application/json"),
        )
    } else {
        build_openai_passthrough_headers(
            &parts.headers,
            &auth_header,
            &auth_value,
            &BTreeMap::new(),
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
        mark_skipped_local_openai_chat_candidate(
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

    let report_kind = if decision_kind == OPENAI_CHAT_STREAM_PLAN_KIND {
        "openai_chat_stream_success"
    } else {
        "openai_chat_sync_finalize"
    };
    let proxy =
        resolve_transport_proxy_snapshot_with_tunnel_affinity(planner_state.app(), transport).await;
    let tls_profile = resolve_transport_tls_profile(transport);
    let prompt_cache_key = provider_request_body
        .get("prompt_cache_key")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    Some(GatewayControlSyncDecisionResponse {
        action: if upstream_is_stream {
            crate::ai_pipeline::planner::common::EXECUTION_RUNTIME_STREAM_DECISION_ACTION
                .to_string()
        } else {
            crate::ai_pipeline::planner::common::EXECUTION_RUNTIME_SYNC_DECISION_ACTION.to_string()
        },
        decision_kind: Some(decision_kind.to_string()),
        execution_strategy: Some(ExecutionStrategy::LocalCrossFormat.as_str().to_string()),
        conversion_mode: Some(ConversionMode::Bidirectional.as_str().to_string()),
        request_id: Some(trace_id.to_string()),
        candidate_id: Some(candidate_id.to_string()),
        provider_name: Some(transport.provider.name.clone()),
        provider_id: Some(candidate.provider_id.clone()),
        endpoint_id: Some(candidate.endpoint_id.clone()),
        key_id: Some(candidate.key_id.clone()),
        upstream_base_url: Some(transport.endpoint.base_url.clone()),
        upstream_url: Some(upstream_url.clone()),
        provider_request_method: None,
        auth_header: Some(auth_header),
        auth_value: Some(auth_value),
        provider_api_format: Some(provider_api_format.clone()),
        client_api_format: Some("openai:chat".to_string()),
        provider_contract: Some(provider_api_format.clone()),
        client_contract: Some("openai:chat".to_string()),
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
        timeouts: resolve_transport_execution_timeouts(transport),
        upstream_is_stream,
        report_kind: Some(report_kind.to_string()),
        report_context: Some(append_local_failover_policy_to_value(
            append_execution_contract_fields_to_value(
                json!({
                    "user_id": input.auth_context.user_id,
                    "api_key_id": input.auth_context.api_key_id,
                    "username": input.auth_context.username,
                    "api_key_name": input.auth_context.api_key_name,
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
                    "provider_api_format": provider_api_format,
                    "client_api_format": "openai:chat",
                    "mapped_model": mapped_model,
                    "upstream_url": upstream_url,
                    "provider_request_method": serde_json::Value::Null,
                    "provider_request_headers": provider_request_headers,
                    "original_headers": collect_control_headers(&parts.headers),
                    "original_request_body": crate::ai_pipeline::build_report_context_original_request_echo(body_json),
                    "has_envelope": false,
                    "needs_conversion": true,
                }),
                ExecutionStrategy::LocalCrossFormat,
                ConversionMode::Bidirectional,
                "openai:chat",
                provider_api_format.as_str(),
            ),
            transport,
        )),
        auth_context: Some(input.auth_context.clone()),
    })
}
