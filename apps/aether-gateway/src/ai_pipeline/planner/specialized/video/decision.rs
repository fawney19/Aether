use std::collections::BTreeMap;

use serde_json::{json, Value};
use tracing::warn;

use crate::ai_pipeline::planner::common::EXECUTION_RUNTIME_SYNC_DECISION_ACTION;
use crate::ai_pipeline::transport::auth::{
    build_passthrough_headers_with_auth, resolve_local_gemini_auth, resolve_local_openai_chat_auth,
};
use crate::ai_pipeline::transport::policy::{
    supports_local_gemini_transport_with_network, supports_local_standard_transport_with_network,
};
use crate::ai_pipeline::transport::url::{
    build_gemini_video_predict_long_running_url, build_passthrough_path_url,
};
use crate::ai_pipeline::transport::{
    apply_local_body_rules, apply_local_header_rules, resolve_transport_execution_timeouts,
    resolve_transport_proxy_snapshot_with_tunnel_affinity, resolve_transport_tls_profile,
};
use crate::ai_pipeline::{
    collect_control_headers, ConversionMode, ExecutionStrategy, GatewayProviderTransportSnapshot,
    PlannerAppState,
};
use crate::{AppState, GatewayControlSyncDecisionResponse};

use super::support::{
    mark_skipped_local_video_candidate, LocalVideoCreateCandidateAttempt,
    LocalVideoCreateDecisionInput,
};
use super::{LocalVideoCreateFamily, LocalVideoCreateSpec};

pub(super) async fn maybe_build_local_video_create_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    input: &LocalVideoCreateDecisionInput,
    attempt: LocalVideoCreateCandidateAttempt,
    spec: LocalVideoCreateSpec,
) -> Option<GatewayControlSyncDecisionResponse> {
    let planner_state = PlannerAppState::new(state);
    let LocalVideoCreateCandidateAttempt {
        candidate,
        candidate_index,
        candidate_id,
    } = attempt;
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
            mark_skipped_local_video_candidate(
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
                decision_kind = spec.decision_kind,
                error = ?err,
                "gateway local video decision provider transport read failed"
            );
            mark_skipped_local_video_candidate(
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

    let transport_supported = match spec.family {
        LocalVideoCreateFamily::OpenAi => {
            supports_local_standard_transport_with_network(&transport, spec.api_format)
        }
        LocalVideoCreateFamily::Gemini => {
            supports_local_gemini_transport_with_network(&transport, spec.api_format)
        }
    };
    if !transport_supported {
        mark_skipped_local_video_candidate(
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

    let auth = match spec.family {
        LocalVideoCreateFamily::OpenAi => resolve_local_openai_chat_auth(&transport),
        LocalVideoCreateFamily::Gemini => resolve_local_gemini_auth(&transport),
    };
    let Some((auth_header, auth_value)) = auth else {
        mark_skipped_local_video_candidate(
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
    };

    let mapped_model = candidate.selected_provider_model_name.trim().to_string();
    if mapped_model.is_empty() {
        mark_skipped_local_video_candidate(
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

    let Some(upstream_url) =
        build_video_upstream_url(parts, &transport, &mapped_model, spec.family)
    else {
        mark_skipped_local_video_candidate(
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
            &candidate,
            candidate_index,
            &candidate_id,
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
            &candidate,
            candidate_index,
            &candidate_id,
            "transport_header_rules_apply_failed",
        )
        .await;
        return None;
    }
    let proxy =
        resolve_transport_proxy_snapshot_with_tunnel_affinity(planner_state.app(), &transport)
            .await;
    let tls_profile = resolve_transport_tls_profile(&transport);

    Some(GatewayControlSyncDecisionResponse {
        action: EXECUTION_RUNTIME_SYNC_DECISION_ACTION.to_string(),
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
        upstream_url: Some(upstream_url),
        provider_request_method: Some(parts.method.to_string()),
        auth_header: Some(auth_header),
        auth_value: Some(auth_value),
        provider_api_format: Some(spec.api_format.to_string()),
        client_api_format: Some(spec.api_format.to_string()),
        provider_contract: Some(spec.api_format.to_string()),
        client_contract: Some(spec.api_format.to_string()),
        model_name: Some(input.requested_model.clone()),
        mapped_model: Some(mapped_model.clone()),
        prompt_cache_key: None,
        extra_headers: BTreeMap::new(),
        provider_request_headers,
        provider_request_body: Some(provider_request_body),
        provider_request_body_base64: None,
        content_type: parts
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        proxy,
        tls_profile,
        timeouts: resolve_transport_execution_timeouts(&transport),
        upstream_is_stream: false,
        report_kind: Some(spec.report_kind.to_string()),
        report_context: Some(json!({
            "user_id": input.auth_context.user_id.clone(),
            "api_key_id": input.auth_context.api_key_id.clone(),
            "request_id": trace_id,
            "candidate_id": candidate_id,
            "candidate_index": candidate_index,
            "retry_index": 0,
            "model": input.requested_model.clone(),
            "provider_name": transport.provider.name.clone(),
            "provider_id": candidate.provider_id.clone(),
            "endpoint_id": candidate.endpoint_id.clone(),
            "key_id": candidate.key_id.clone(),
            "provider_api_format": spec.api_format,
            "client_api_format": spec.api_format,
            "mapped_model": mapped_model,
            "original_headers": collect_control_headers(&parts.headers),
            "original_request_body": body_json,
            "has_envelope": false,
            "needs_conversion": false,
        })),
        auth_context: Some(input.auth_context.clone()),
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
