use axum::body::{Body, Bytes};
use axum::http::Response;
use std::collections::BTreeMap;

use crate::ai_pipeline_api::{
    is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
    supports_stream_scheduler_decision_kind, OPENAI_VIDEO_CONTENT_PLAN_KIND,
};
use crate::api::response::build_client_response_from_parts;
use crate::control::GatewayControlDecision;
use crate::execution_runtime::execute_execution_runtime_stream;
use crate::{AppState, GatewayError, GatewayFallbackReason};

use super::{
    build_direct_plan_bypass_cache_key, maybe_execute_stream_via_local_decision,
    maybe_execute_stream_via_local_gemini_files_decision,
    maybe_execute_stream_via_local_openai_cli_decision,
    maybe_execute_stream_via_local_same_format_provider_decision,
    maybe_execute_stream_via_local_standard_decision, maybe_execute_stream_via_plan_fallback,
    maybe_execute_stream_via_remote_decision, parse_local_request_body, should_skip_direct_plan,
};

pub(crate) async fn maybe_execute_via_stream_decision_path(
    state: &AppState,
    parts: &http::request::Parts,
    body_bytes: &Bytes,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<Option<Response<Body>>, GatewayError> {
    let Some(plan_kind) = resolve_execution_runtime_stream_plan_kind(parts, decision) else {
        return Ok(None);
    };

    let Some((body_json, body_base64)) = parse_local_request_body(parts, body_bytes) else {
        return Ok(None);
    };

    if !is_matching_stream_request(plan_kind, parts, &body_json) {
        return Ok(None);
    }

    let bypass_cache_key =
        build_direct_plan_bypass_cache_key(plan_kind, parts, body_bytes, decision);
    if should_skip_direct_plan(state, &bypass_cache_key) {
        return Ok(None);
    }

    if let Some(response) =
        maybe_execute_local_video_task_content_stream(state, parts, trace_id, decision, plan_kind)
            .await?
    {
        return Ok(Some(response));
    }

    if supports_stream_scheduler_decision_kind(plan_kind) {
        if let Some(response) = maybe_execute_stream_via_local_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }

        if let Some(response) = maybe_execute_stream_via_local_openai_cli_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }

        if let Some(response) = maybe_execute_stream_via_local_standard_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }

        if let Some(response) = maybe_execute_stream_via_local_same_format_provider_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }

        if let Some(response) = maybe_execute_stream_via_local_gemini_files_decision(
            state, parts, trace_id, decision, plan_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }

        if let Some(response) = maybe_execute_stream_via_remote_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            return Ok(Some(response));
        }
    }

    maybe_execute_stream_via_plan_fallback(
        state,
        parts,
        trace_id,
        decision,
        &body_json,
        body_base64,
        plan_kind,
        bypass_cache_key,
        if supports_stream_scheduler_decision_kind(plan_kind) {
            GatewayFallbackReason::RemoteDecisionMiss
        } else {
            GatewayFallbackReason::SchedulerDecisionUnsupported
        },
    )
    .await
}

async fn maybe_execute_local_video_task_content_stream(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<Option<Response<Body>>, GatewayError> {
    if plan_kind != OPENAI_VIDEO_CONTENT_PLAN_KIND
        || decision.route_family.as_deref() != Some("openai")
    {
        return Ok(None);
    }

    let _ = state
        .hydrate_video_task_for_route(decision.route_family.as_deref(), parts.uri.path())
        .await?;

    if let Some(task_id) =
        crate::video_tasks::extract_openai_task_id_from_content_path(parts.uri.path())
    {
        let refresh_path = format!("/v1/videos/{task_id}");
        if let Some(refresh_plan) = state.video_tasks.prepare_read_refresh_sync_plan(
            Some("openai"),
            &refresh_path,
            trace_id,
        ) {
            state.execute_video_task_refresh_plan(&refresh_plan).await?;
        }
    }

    let Some(action) = state.video_tasks.prepare_openai_content_stream_action(
        parts.uri.path(),
        parts.uri.query(),
        trace_id,
    ) else {
        return Ok(None);
    };

    match action {
        crate::video_tasks::LocalVideoTaskContentAction::Immediate {
            status_code,
            body_json,
        } => Ok(Some(build_json_response(
            trace_id,
            decision,
            status_code,
            &body_json,
        )?)),
        crate::video_tasks::LocalVideoTaskContentAction::StreamPlan(plan) => {
            execute_execution_runtime_stream(
                state, *plan, trace_id, decision, plan_kind, None, None,
            )
            .await
        }
    }
}

fn build_json_response(
    trace_id: &str,
    decision: &GatewayControlDecision,
    status_code: u16,
    body_json: &serde_json::Value,
) -> Result<Response<Body>, GatewayError> {
    let body_bytes =
        serde_json::to_vec(body_json).map_err(|err| GatewayError::Internal(err.to_string()))?;
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("content-length".to_string(), body_bytes.len().to_string());
    build_client_response_from_parts(
        status_code,
        &headers,
        Body::from(body_bytes),
        trace_id,
        Some(decision),
    )
}
