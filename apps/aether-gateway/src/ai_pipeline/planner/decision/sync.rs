use std::collections::BTreeMap;

use tracing::debug;
use url::Url;

use crate::ai_pipeline::planner::common::{
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION, GEMINI_FILES_DELETE_PLAN_KIND,
    GEMINI_FILES_GET_PLAN_KIND, GEMINI_FILES_LIST_PLAN_KIND, GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND,
    OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::ai_pipeline::planner::route::resolve_execution_runtime_sync_plan_kind;
use crate::ai_pipeline::{
    build_execution_runtime_auth_context, resolve_execution_runtime_auth_context, ConversionMode,
    ExecutionStrategy, GatewayControlDecision,
};
use crate::{AppState, GatewayControlSyncDecisionResponse, GatewayError};

pub(crate) async fn maybe_build_sync_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    let Some(plan_kind) = resolve_execution_runtime_sync_plan_kind(parts, decision) else {
        return Ok(None);
    };

    if let Some(payload) = maybe_build_local_video_task_follow_up_sync_decision_payload(
        state, parts, body_json, trace_id, decision, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_sync_local_video_decision_payload(
        state, parts, body_json, trace_id, decision, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_sync_local_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_sync_local_openai_cli_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_sync_local_standard_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_sync_local_same_format_provider_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if matches!(
        plan_kind,
        GEMINI_FILES_LIST_PLAN_KIND | GEMINI_FILES_GET_PLAN_KIND | GEMINI_FILES_DELETE_PLAN_KIND
    ) {
        if let Some(payload) = super::maybe_build_sync_local_gemini_files_decision_payload(
            state,
            parts,
            body_json,
            body_base64,
            body_is_empty,
            trace_id,
            decision,
            plan_kind,
        )
        .await?
        {
            return Ok(Some(payload));
        }
    }

    Ok(None)
}

async fn maybe_build_local_video_task_follow_up_sync_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    if !matches!(
        plan_kind,
        OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND
            | OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND
            | OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND
            | GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND
    ) {
        return Ok(None);
    }

    let _ = state
        .hydrate_video_task_for_route(decision.route_family.as_deref(), parts.uri.path())
        .await?;

    let auth_context = resolve_execution_runtime_auth_context(
        state,
        decision,
        &parts.headers,
        &parts.uri,
        trace_id,
    )
    .await?;
    let Some(auth_context) = auth_context else {
        return Ok(None);
    };
    let Some(follow_up) = state.video_tasks.prepare_follow_up_sync_plan(
        plan_kind,
        parts.uri.path(),
        Some(body_json),
        Some(&auth_context),
        trace_id,
    ) else {
        return Ok(None);
    };

    let auth_pair = extract_auth_header_pair(&follow_up.plan.headers);
    let execution_strategy =
        if follow_up.plan.provider_api_format == follow_up.plan.client_api_format {
            ExecutionStrategy::LocalSameFormat
        } else {
            ExecutionStrategy::LocalCrossFormat
        };
    let conversion_mode = if follow_up.plan.provider_api_format == follow_up.plan.client_api_format
    {
        ConversionMode::None
    } else {
        ConversionMode::Bidirectional
    };
    let upstream_base_url = infer_upstream_base_url(&follow_up.plan.url);

    debug!(
        event_name = "local_video_follow_up_sync_decision_payload_built",
        log_type = "debug",
        trace_id = %trace_id,
        request_id = %trace_id,
        candidate_id = ?follow_up.plan.candidate_id,
        provider_id = %follow_up.plan.provider_id,
        endpoint_id = %follow_up.plan.endpoint_id,
        key_id = %follow_up.plan.key_id,
        plan_kind,
        downstream_path = %parts.uri.path(),
        provider_api_format = %follow_up.plan.provider_api_format,
        client_api_format = %follow_up.plan.client_api_format,
        upstream_base_url = ?upstream_base_url,
        upstream_url = %follow_up.plan.url,
        "gateway built local video follow-up sync decision payload"
    );

    Ok(Some(GatewayControlSyncDecisionResponse {
        action: EXECUTION_RUNTIME_SYNC_DECISION_ACTION.to_string(),
        decision_kind: Some(plan_kind.to_string()),
        execution_strategy: Some(execution_strategy.as_str().to_string()),
        conversion_mode: Some(conversion_mode.as_str().to_string()),
        request_id: Some(trace_id.to_string()),
        candidate_id: follow_up.plan.candidate_id.clone(),
        provider_name: follow_up.plan.provider_name.clone(),
        provider_id: Some(follow_up.plan.provider_id.clone()),
        endpoint_id: Some(follow_up.plan.endpoint_id.clone()),
        key_id: Some(follow_up.plan.key_id.clone()),
        upstream_base_url,
        upstream_url: Some(follow_up.plan.url.clone()),
        provider_request_method: Some(follow_up.plan.method.clone()),
        auth_header: auth_pair.as_ref().map(|(name, _)| name.clone()),
        auth_value: auth_pair.as_ref().map(|(_, value)| value.clone()),
        provider_api_format: Some(follow_up.plan.provider_api_format.clone()),
        client_api_format: Some(follow_up.plan.client_api_format.clone()),
        provider_contract: Some(follow_up.plan.provider_api_format.clone()),
        client_contract: Some(follow_up.plan.client_api_format.clone()),
        model_name: follow_up.plan.model_name.clone(),
        mapped_model: None,
        prompt_cache_key: None,
        extra_headers: BTreeMap::new(),
        provider_request_headers: follow_up.plan.headers.clone(),
        provider_request_body: follow_up.plan.body.json_body.clone(),
        provider_request_body_base64: follow_up.plan.body.body_bytes_b64.clone(),
        content_type: follow_up.plan.content_type.clone(),
        proxy: follow_up.plan.proxy.clone(),
        tls_profile: follow_up.plan.tls_profile.clone(),
        timeouts: follow_up.plan.timeouts.clone(),
        upstream_is_stream: false,
        report_kind: follow_up.report_kind,
        report_context: follow_up.report_context,
        auth_context: Some(build_execution_runtime_auth_context(&auth_context)),
    }))
}

fn extract_auth_header_pair(headers: &BTreeMap<String, String>) -> Option<(String, String)> {
    [
        "authorization",
        "x-api-key",
        "api-key",
        "x-goog-api-key",
        "proxy-authorization",
    ]
    .into_iter()
    .find_map(|name| {
        headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(header_name, value)| (header_name.clone(), value.clone()))
    })
}

fn infer_upstream_base_url(upstream_url: &str) -> Option<String> {
    let parsed = Url::parse(upstream_url).ok()?;
    let host = parsed.host_str()?;
    let mut base = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        base.push(':');
        base.push_str(port.to_string().as_str());
    }
    let base_path = infer_upstream_base_path(parsed.path());
    if !base_path.is_empty() {
        base.push_str(base_path);
    }
    Some(base)
}

fn infer_upstream_base_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return "";
    }

    for suffix in [
        "/responses/compact",
        "/responses",
        "/chat/completions",
        "/messages",
    ] {
        if let Some(prefix) = trimmed.strip_suffix(suffix) {
            return normalize_inferred_base_path(prefix);
        }
    }

    for marker in ["/v1/videos", "/v1beta/"] {
        if let Some((prefix, _)) = trimmed.split_once(marker) {
            return normalize_inferred_base_path(prefix);
        }
    }

    normalize_inferred_base_path(trimmed)
}

fn normalize_inferred_base_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        ""
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::infer_upstream_base_url;

    #[test]
    fn infer_upstream_base_url_preserves_codex_base_path() {
        assert_eq!(
            infer_upstream_base_url("https://tiger.bookapi.cc/codex/responses").as_deref(),
            Some("https://tiger.bookapi.cc/codex")
        );
        assert_eq!(
            infer_upstream_base_url("https://chatgpt.com/backend-api/codex/responses").as_deref(),
            Some("https://chatgpt.com/backend-api/codex")
        );
    }

    #[test]
    fn infer_upstream_base_url_preserves_nested_v1_prefix() {
        assert_eq!(
            infer_upstream_base_url("https://api.openai.example/custom/v1/chat/completions?mode=1")
                .as_deref(),
            Some("https://api.openai.example/custom/v1")
        );
    }

    #[test]
    fn infer_upstream_base_url_strips_video_operation_path() {
        assert_eq!(
            infer_upstream_base_url("https://video.example/nested/v1/videos/task-123/content")
                .as_deref(),
            Some("https://video.example/nested")
        );
    }
}
