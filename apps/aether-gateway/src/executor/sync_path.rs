use axum::body::{Body, Bytes};
use axum::http::Response;
use std::collections::BTreeMap;

use crate::ai_pipeline_api::{
    is_matching_stream_request, resolve_execution_runtime_stream_plan_kind,
    resolve_execution_runtime_sync_plan_kind, supports_sync_scheduler_decision_kind,
    LocalSyncPlanAndReport, GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND, OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND,
    OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND, OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND,
};
use crate::api::response::build_client_response_from_parts;
use crate::control::resolve_execution_runtime_auth_context;
use crate::control::GatewayControlDecision;
use crate::{AppState, GatewayError, GatewayFallbackReason};

use super::{
    build_direct_plan_bypass_cache_key, execute_sync_plan_and_reports,
    maybe_execute_sync_via_local_decision, maybe_execute_sync_via_local_gemini_files_decision,
    maybe_execute_sync_via_local_image_decision, maybe_execute_sync_via_local_openai_cli_decision,
    maybe_execute_sync_via_local_same_format_provider_decision,
    maybe_execute_sync_via_local_standard_decision, maybe_execute_sync_via_local_video_decision,
    maybe_execute_sync_via_plan_fallback, maybe_execute_sync_via_remote_decision,
    parse_local_request_body, should_skip_direct_plan, LocalExecutionRequestOutcome,
};

pub(crate) async fn maybe_execute_via_sync_decision_path(
    state: &AppState,
    parts: &http::request::Parts,
    body_bytes: &Bytes,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    if let LocalExecutionRequestOutcome::Responded(response) =
        maybe_build_local_video_task_read_response(state, parts, trace_id, decision).await?
    {
        return Ok(LocalExecutionRequestOutcome::Responded(response));
    }

    let Some(plan_kind) = resolve_execution_runtime_sync_plan_kind(parts, decision) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let Some((body_json, body_base64)) = parse_local_request_body(parts, body_bytes) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    if let Some(stream_plan_kind) = resolve_execution_runtime_stream_plan_kind(parts, decision) {
        if is_matching_stream_request(stream_plan_kind, parts, &body_json) {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        }
    }

    let bypass_cache_key =
        build_direct_plan_bypass_cache_key(plan_kind, parts, body_bytes, decision);
    if should_skip_direct_plan(state, &bypass_cache_key) {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let mut exhausted = None;

    match maybe_execute_local_video_task_follow_up_sync(
        state, parts, &body_json, trace_id, decision, plan_kind,
    )
    .await?
    {
        LocalExecutionRequestOutcome::Responded(response) => {
            return Ok(LocalExecutionRequestOutcome::Responded(response));
        }
        LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
        LocalExecutionRequestOutcome::NoPath => {}
    }

    if supports_sync_scheduler_decision_kind(plan_kind) {
        match maybe_execute_sync_via_local_video_decision(
            state, parts, &body_json, trace_id, decision, plan_kind,
        )
        .await?
        {
            LocalExecutionRequestOutcome::Responded(response) => {
                return Ok(LocalExecutionRequestOutcome::Responded(response));
            }
            LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
            LocalExecutionRequestOutcome::NoPath => {}
        }

        match maybe_execute_sync_via_local_image_decision(
            state,
            parts,
            &body_json,
            body_base64.as_deref(),
            trace_id,
            decision,
            plan_kind,
        )
        .await?
        {
            LocalExecutionRequestOutcome::Responded(response) => {
                return Ok(LocalExecutionRequestOutcome::Responded(response));
            }
            LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
            LocalExecutionRequestOutcome::NoPath => {}
        }

        match maybe_execute_sync_via_local_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            LocalExecutionRequestOutcome::Responded(response) => {
                return Ok(LocalExecutionRequestOutcome::Responded(response));
            }
            LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
            LocalExecutionRequestOutcome::NoPath => {}
        }

        match maybe_execute_sync_via_local_openai_cli_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            LocalExecutionRequestOutcome::Responded(response) => {
                return Ok(LocalExecutionRequestOutcome::Responded(response));
            }
            LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
            LocalExecutionRequestOutcome::NoPath => {}
        }

        match maybe_execute_sync_via_local_standard_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            LocalExecutionRequestOutcome::Responded(response) => {
                return Ok(LocalExecutionRequestOutcome::Responded(response));
            }
            LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
            LocalExecutionRequestOutcome::NoPath => {}
        }

        match maybe_execute_sync_via_local_same_format_provider_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            LocalExecutionRequestOutcome::Responded(response) => {
                return Ok(LocalExecutionRequestOutcome::Responded(response));
            }
            LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
            LocalExecutionRequestOutcome::NoPath => {}
        }

        match maybe_execute_sync_via_local_gemini_files_decision(
            state,
            parts,
            &body_json,
            body_base64.as_deref(),
            body_bytes.is_empty(),
            trace_id,
            decision,
            plan_kind,
        )
        .await?
        {
            LocalExecutionRequestOutcome::Responded(response) => {
                return Ok(LocalExecutionRequestOutcome::Responded(response));
            }
            LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
            LocalExecutionRequestOutcome::NoPath => {}
        }

        if let Some(response) = maybe_execute_sync_via_remote_decision(
            state, parts, trace_id, decision, &body_json, plan_kind,
        )
        .await?
        {
            return Ok(LocalExecutionRequestOutcome::Responded(response));
        }
    }

    match maybe_execute_sync_via_plan_fallback(
        state,
        parts,
        trace_id,
        decision,
        &body_json,
        body_base64,
        plan_kind,
        bypass_cache_key,
        if supports_sync_scheduler_decision_kind(plan_kind) {
            GatewayFallbackReason::RemoteDecisionMiss
        } else {
            GatewayFallbackReason::SchedulerDecisionUnsupported
        },
    )
    .await?
    {
        LocalExecutionRequestOutcome::Responded(response) => {
            Ok(LocalExecutionRequestOutcome::Responded(response))
        }
        LocalExecutionRequestOutcome::Exhausted(outcome) => {
            Ok(LocalExecutionRequestOutcome::Exhausted(outcome))
        }
        LocalExecutionRequestOutcome::NoPath => Ok(exhausted
            .map(LocalExecutionRequestOutcome::Exhausted)
            .unwrap_or(LocalExecutionRequestOutcome::NoPath)),
    }
}

async fn maybe_build_local_video_task_read_response(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    if parts.method != http::Method::GET || decision.route_kind.as_deref() != Some("video") {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let _ = state
        .hydrate_video_task_for_route(decision.route_family.as_deref(), parts.uri.path())
        .await?;

    let refresh_plan = state.video_tasks.prepare_read_refresh_sync_plan(
        decision.route_family.as_deref(),
        parts.uri.path(),
        trace_id,
    );

    if let Some(refresh_plan) = refresh_plan {
        state.execute_video_task_refresh_plan(&refresh_plan).await?;
    }

    let read_response = state
        .video_tasks
        .read_response(decision.route_family.as_deref(), parts.uri.path());
    let read_response = match read_response {
        Some(read_response) => Some(read_response),
        None => {
            state
                .read_data_backed_video_task_response(
                    decision.route_family.as_deref(),
                    parts.uri.path(),
                )
                .await?
        }
    };
    let Some(read_response) = read_response else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let body_bytes = serde_json::to_vec(&read_response.body_json)
        .map_err(|err| GatewayError::Internal(err.to_string()))?;
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    headers.insert("content-length".to_string(), body_bytes.len().to_string());

    Ok(LocalExecutionRequestOutcome::Responded(
        build_client_response_from_parts(
            read_response.status_code,
            &headers,
            Body::from(body_bytes),
            trace_id,
            Some(decision),
        )?,
    ))
}

async fn maybe_execute_local_video_task_follow_up_sync(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    if !matches!(
        plan_kind,
        OPENAI_VIDEO_REMIX_SYNC_PLAN_KIND
            | OPENAI_VIDEO_CANCEL_SYNC_PLAN_KIND
            | OPENAI_VIDEO_DELETE_SYNC_PLAN_KIND
            | GEMINI_VIDEO_CANCEL_SYNC_PLAN_KIND
    ) {
        return Ok(LocalExecutionRequestOutcome::NoPath);
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
    let Some(follow_up) = state.video_tasks.prepare_follow_up_sync_plan(
        plan_kind,
        parts.uri.path(),
        Some(body_json),
        auth_context.as_ref(),
        trace_id,
    ) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        vec![LocalSyncPlanAndReport {
            plan: follow_up.plan,
            report_kind: follow_up.report_kind,
            report_context: follow_up.report_context,
        }],
    )
    .await
}
