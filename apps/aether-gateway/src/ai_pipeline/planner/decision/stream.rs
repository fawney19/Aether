use std::collections::BTreeMap;

use crate::ai_pipeline::planner::common::{
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, OPENAI_VIDEO_CONTENT_PLAN_KIND,
};
use crate::ai_pipeline::planner::route::{
    is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
};
use crate::ai_pipeline::{
    resolve_decision_execution_runtime_auth_context, ConversionMode, ExecutionStrategy,
    GatewayControlDecision,
};
use crate::{AppState, GatewayControlSyncDecisionResponse, GatewayError};

pub(crate) async fn maybe_build_stream_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    let Some(plan_kind) = resolve_execution_runtime_stream_plan_kind(parts, decision) else {
        return Ok(None);
    };

    if !is_matching_stream_request(plan_kind, parts, body_json, body_base64) {
        return Ok(None);
    }

    if let Some(payload) = maybe_build_local_video_task_content_stream_decision_payload(
        state, parts, trace_id, decision, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_stream_local_image_decision_payload(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
        plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_stream_local_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_stream_local_openai_cli_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_stream_local_standard_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_stream_local_same_format_provider_decision_payload(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    if let Some(payload) = super::maybe_build_stream_local_gemini_files_decision_payload(
        state, parts, trace_id, decision, plan_kind,
    )
    .await?
    {
        return Ok(Some(payload));
    }

    Ok(None)
}

async fn maybe_build_local_video_task_content_stream_decision_payload(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<GatewayControlSyncDecisionResponse>, GatewayError> {
    if plan_kind != OPENAI_VIDEO_CONTENT_PLAN_KIND
        || decision.route_family.as_deref() != Some("openai")
    {
        return Ok(None);
    }

    let _ = state
        .hydrate_video_task_for_route(decision.route_family.as_deref(), parts.uri.path())
        .await?;

    let Some(action) = state.video_tasks.prepare_openai_content_stream_action(
        parts.uri.path(),
        parts.uri.query(),
        trace_id,
    ) else {
        return Ok(None);
    };

    let crate::video_tasks::LocalVideoTaskContentAction::StreamPlan(plan) = action else {
        return Ok(None);
    };
    let plan = *plan;
    let provider_contract = plan.provider_api_format.clone();
    let client_contract = plan.client_api_format.clone();
    let execution_strategy = if plan.provider_api_format == plan.client_api_format {
        ExecutionStrategy::LocalSameFormat
    } else {
        ExecutionStrategy::LocalCrossFormat
    };
    let conversion_mode = if plan.provider_api_format == plan.client_api_format {
        ConversionMode::None
    } else {
        ConversionMode::Bidirectional
    };

    Ok(Some(GatewayControlSyncDecisionResponse {
        action: EXECUTION_RUNTIME_STREAM_DECISION_ACTION.to_string(),
        decision_kind: Some(plan_kind.to_string()),
        execution_strategy: Some(execution_strategy.as_str().to_string()),
        conversion_mode: Some(conversion_mode.as_str().to_string()),
        request_id: Some(plan.request_id),
        candidate_id: plan.candidate_id,
        provider_name: plan.provider_name,
        provider_id: Some(plan.provider_id),
        endpoint_id: Some(plan.endpoint_id),
        key_id: Some(plan.key_id),
        upstream_base_url: None,
        upstream_url: Some(plan.url),
        provider_request_method: Some(plan.method),
        auth_header: None,
        auth_value: None,
        provider_api_format: Some(plan.provider_api_format),
        client_api_format: Some(plan.client_api_format),
        provider_contract: Some(provider_contract),
        client_contract: Some(client_contract),
        model_name: plan.model_name,
        mapped_model: None,
        prompt_cache_key: None,
        extra_headers: BTreeMap::new(),
        provider_request_headers: plan.headers,
        provider_request_body: None,
        provider_request_body_base64: None,
        content_type: plan.content_type,
        proxy: plan.proxy,
        tls_profile: plan.tls_profile,
        timeouts: plan.timeouts,
        upstream_is_stream: true,
        report_kind: None,
        report_context: None,
        auth_context: resolve_decision_execution_runtime_auth_context(decision),
    }))
}
