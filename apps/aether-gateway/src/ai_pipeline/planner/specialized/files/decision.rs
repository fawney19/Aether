use std::collections::BTreeMap;

use serde_json::json;
use tracing::warn;

use crate::ai_pipeline::contracts::GEMINI_FILES_UPLOAD_PLAN_KIND;
use crate::ai_pipeline::planner::common::{
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, EXECUTION_RUNTIME_SYNC_DECISION_ACTION,
};
use crate::ai_pipeline::transport::auth::{
    build_passthrough_headers_with_auth, resolve_local_gemini_auth,
};
use crate::ai_pipeline::transport::policy::supports_local_gemini_transport_with_network;
use crate::ai_pipeline::transport::url::build_gemini_files_passthrough_url;
use crate::ai_pipeline::transport::{
    apply_local_body_rules, apply_local_header_rules, resolve_transport_execution_timeouts,
    resolve_transport_proxy_snapshot_with_tunnel_affinity, resolve_transport_tls_profile,
};
use crate::ai_pipeline::{
    collect_control_headers, ConversionMode, ExecutionStrategy, PlannerAppState,
};
use crate::{AppState, GatewayControlSyncDecisionResponse};

use super::support::{
    mark_skipped_local_gemini_files_candidate, LocalGeminiFilesCandidateAttempt,
    LocalGeminiFilesDecisionInput, GEMINI_FILES_CANDIDATE_API_FORMAT,
    GEMINI_FILES_CLIENT_API_FORMAT,
};
use super::LocalGeminiFilesSpec;

pub(super) async fn maybe_build_local_gemini_files_decision_payload_for_candidate(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    input: &LocalGeminiFilesDecisionInput,
    attempt: LocalGeminiFilesCandidateAttempt,
    spec: LocalGeminiFilesSpec,
) -> Option<GatewayControlSyncDecisionResponse> {
    let planner_state = PlannerAppState::new(state);
    let LocalGeminiFilesCandidateAttempt {
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
            mark_skipped_local_gemini_files_candidate(
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
                error = ?err,
                "gateway local gemini files provider transport read failed"
            );
            mark_skipped_local_gemini_files_candidate(
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

    if !supports_local_gemini_transport_with_network(&transport, GEMINI_FILES_CANDIDATE_API_FORMAT)
    {
        mark_skipped_local_gemini_files_candidate(
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

    let Some((auth_header, auth_value)) = resolve_local_gemini_auth(&transport) else {
        mark_skipped_local_gemini_files_candidate(
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

    let custom_path = transport
        .endpoint
        .custom_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let passthrough_path = custom_path.unwrap_or(parts.uri.path());
    let Some(upstream_url) = build_gemini_files_passthrough_url(
        &transport.endpoint.base_url,
        passthrough_path,
        parts.uri.query(),
    ) else {
        mark_skipped_local_gemini_files_candidate(
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

    let mut provider_request_body = if spec.decision_kind == GEMINI_FILES_UPLOAD_PLAN_KIND
        && !body_is_empty
        && body_base64.is_none()
    {
        Some(body_json.clone())
    } else {
        None
    };
    let provider_request_body_base64 = if spec.decision_kind == GEMINI_FILES_UPLOAD_PLAN_KIND {
        body_base64
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    } else {
        None
    };
    let original_request_body = if let Some(body_bytes_b64) = provider_request_body_base64.clone() {
        json!({"body_bytes_b64": body_bytes_b64})
    } else if !body_is_empty {
        body_json.clone()
    } else {
        serde_json::Value::Null
    };
    if provider_request_body_base64.is_some() && transport.endpoint.body_rules.is_some() {
        mark_skipped_local_gemini_files_candidate(
            state,
            input,
            trace_id,
            &candidate,
            candidate_index,
            &candidate_id,
            "transport_body_rules_unsupported_for_binary_upload",
        )
        .await;
        return None;
    }
    if let Some(body) = provider_request_body.as_mut() {
        if !apply_local_body_rules(
            body,
            transport.endpoint.body_rules.as_ref(),
            Some(body_json),
        ) {
            mark_skipped_local_gemini_files_candidate(
                state,
                input,
                trace_id,
                &candidate,
                candidate_index,
                &candidate_id,
                "transport_body_rules_apply_failed",
            )
            .await;
            return None;
        }
    }
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
        provider_request_body
            .as_ref()
            .unwrap_or(&original_request_body),
        Some(&original_request_body),
    ) {
        mark_skipped_local_gemini_files_candidate(
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
    let file_name = parts
        .uri
        .path()
        .trim_start_matches("/v1beta/")
        .trim()
        .to_string();
    let proxy =
        resolve_transport_proxy_snapshot_with_tunnel_affinity(planner_state.app(), &transport)
            .await;
    let tls_profile = resolve_transport_tls_profile(&transport);

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
        upstream_url: Some(upstream_url),
        provider_request_method: Some(parts.method.to_string()),
        auth_header: Some(auth_header),
        auth_value: Some(auth_value),
        provider_api_format: Some(GEMINI_FILES_CLIENT_API_FORMAT.to_string()),
        client_api_format: Some(GEMINI_FILES_CLIENT_API_FORMAT.to_string()),
        provider_contract: Some(GEMINI_FILES_CLIENT_API_FORMAT.to_string()),
        client_contract: Some(GEMINI_FILES_CLIENT_API_FORMAT.to_string()),
        model_name: Some("gemini-files".to_string()),
        mapped_model: Some(candidate.selected_provider_model_name.clone()),
        prompt_cache_key: None,
        extra_headers: BTreeMap::new(),
        provider_request_headers,
        provider_request_body,
        provider_request_body_base64,
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
        upstream_is_stream: spec.require_streaming,
        report_kind: spec.report_kind.map(ToOwned::to_owned),
        report_context: Some(json!({
            "user_id": input.auth_context.user_id,
            "api_key_id": input.auth_context.api_key_id,
            "username": input.auth_context.username,
            "api_key_name": input.auth_context.api_key_name,
            "request_id": trace_id,
            "candidate_id": candidate_id,
            "candidate_index": candidate_index,
            "retry_index": 0,
            "model": "gemini-files",
            "provider_name": transport.provider.name,
            "provider_id": candidate.provider_id,
            "endpoint_id": candidate.endpoint_id,
            "key_id": candidate.key_id,
            "file_key_id": candidate.key_id,
            "file_name": file_name,
            "provider_api_format": GEMINI_FILES_CLIENT_API_FORMAT,
            "client_api_format": GEMINI_FILES_CLIENT_API_FORMAT,
            "original_headers": collect_control_headers(&parts.headers),
            "original_request_body": crate::ai_pipeline::build_report_context_original_request_echo(&original_request_body),
            "has_envelope": false,
            "needs_conversion": false,
        })),
        auth_context: Some(input.auth_context.clone()),
    })
}
