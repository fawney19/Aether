use crate::ai_pipeline_api::{
    build_local_gemini_files_stream_plan_and_reports_for_kind,
    build_local_gemini_files_sync_plan_and_reports_for_kind,
    build_local_image_sync_plan_and_reports_for_kind,
    build_local_openai_chat_stream_plan_and_reports_for_kind,
    build_local_openai_chat_sync_plan_and_reports_for_kind,
    build_local_openai_cli_stream_plan_and_reports_for_kind,
    build_local_openai_cli_sync_plan_and_reports_for_kind,
    build_local_same_format_stream_plan_and_reports, build_local_same_format_sync_plan_and_reports,
    build_local_video_sync_plan_and_reports_for_kind,
    build_standard_family_stream_plan_and_reports, build_standard_family_sync_plan_and_reports,
    parse_direct_request_body, resolve_claude_stream_spec, resolve_claude_sync_spec,
    resolve_gemini_stream_spec, resolve_gemini_sync_spec, resolve_local_same_format_stream_spec,
    resolve_local_same_format_sync_spec, set_local_openai_chat_execution_exhausted_diagnostic,
    LocalStandardSpec, LocalStreamPlanAndReport, LocalSyncPlanAndReport,
    EXECUTION_RUNTIME_STREAM_DECISION_ACTION, EXECUTION_RUNTIME_SYNC_DECISION_ACTION,
};
use crate::control::GatewayControlDecision;
use crate::executor::candidate_loop::{
    execute_stream_plan_and_reports, execute_sync_plan_and_reports,
};
use crate::executor::LocalExecutionRequestOutcome;
use crate::{AppState, GatewayControlSyncDecisionResponse, GatewayError};

pub(crate) async fn maybe_execute_sync_local_path(
    state: &AppState,
    parts: &http::request::Parts,
    body_bytes: &axum::body::Bytes,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    super::maybe_execute_via_sync_decision_path(state, parts, body_bytes, trace_id, decision).await
}

pub(crate) async fn maybe_execute_stream_local_path(
    state: &AppState,
    parts: &http::request::Parts,
    body_bytes: &axum::body::Bytes,
    trace_id: &str,
    decision: &GatewayControlDecision,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    super::maybe_execute_via_stream_decision_path(state, parts, body_bytes, trace_id, decision)
        .await
}

pub(crate) async fn maybe_execute_sync_via_local_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let plan_and_reports = build_local_openai_chat_sync_plan_and_reports_for_kind(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let plan_count = plan_and_reports.len();
    let outcome = execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
    )
    .await?;

    if let LocalExecutionRequestOutcome::Exhausted(_) = &outcome {
        set_local_openai_chat_execution_exhausted_diagnostic(
            state, trace_id, decision, plan_kind, body_json, plan_count,
        );
    }

    Ok(outcome)
}

pub(crate) async fn maybe_execute_stream_via_local_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let plan_and_reports = build_local_openai_chat_stream_plan_and_reports_for_kind(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    let plan_count = plan_and_reports.len();
    let outcome =
        execute_stream_plan_and_reports(state, trace_id, decision, plan_kind, plan_and_reports)
            .await?;

    if let LocalExecutionRequestOutcome::Exhausted(_) = &outcome {
        set_local_openai_chat_execution_exhausted_diagnostic(
            state, trace_id, decision, plan_kind, body_json, plan_count,
        );
    }

    Ok(outcome)
}

pub(crate) async fn maybe_execute_sync_via_local_openai_cli_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let plan_and_reports: Vec<LocalSyncPlanAndReport> =
        build_local_openai_cli_sync_plan_and_reports_for_kind(
            state, parts, trace_id, decision, body_json, plan_kind,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
    )
    .await
}

pub(crate) async fn maybe_execute_stream_via_local_openai_cli_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let plan_and_reports: Vec<LocalStreamPlanAndReport> =
        build_local_openai_cli_stream_plan_and_reports_for_kind(
            state, parts, trace_id, decision, body_json, plan_kind,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_stream_plan_and_reports(state, trace_id, decision, plan_kind, plan_and_reports).await
}

pub(crate) async fn maybe_execute_sync_via_standard_family_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
    resolve_sync_spec: fn(&str) -> Option<LocalStandardSpec>,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some(spec) = resolve_sync_spec(plan_kind) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let plan_and_reports: Vec<LocalSyncPlanAndReport> =
        build_standard_family_sync_plan_and_reports(
            state, parts, trace_id, decision, body_json, spec,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
    )
    .await
}

pub(crate) async fn maybe_execute_stream_via_standard_family_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
    resolve_stream_spec: fn(&str) -> Option<LocalStandardSpec>,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some(spec) = resolve_stream_spec(plan_kind) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let plan_and_reports: Vec<LocalStreamPlanAndReport> =
        build_standard_family_stream_plan_and_reports(
            state, parts, trace_id, decision, body_json, spec,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_stream_plan_and_reports(state, trace_id, decision, plan_kind, plan_and_reports).await
}

pub(crate) async fn maybe_execute_sync_via_local_standard_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let mut exhausted = None;

    match maybe_execute_sync_via_standard_family_decision(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        plan_kind,
        resolve_claude_sync_spec,
    )
    .await?
    {
        LocalExecutionRequestOutcome::Responded(response) => {
            return Ok(LocalExecutionRequestOutcome::Responded(response));
        }
        LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
        LocalExecutionRequestOutcome::NoPath => {}
    }

    match maybe_execute_sync_via_standard_family_decision(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        plan_kind,
        resolve_gemini_sync_spec,
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

pub(crate) async fn maybe_execute_stream_via_local_standard_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let mut exhausted = None;

    match maybe_execute_stream_via_standard_family_decision(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        plan_kind,
        resolve_claude_stream_spec,
    )
    .await?
    {
        LocalExecutionRequestOutcome::Responded(response) => {
            return Ok(LocalExecutionRequestOutcome::Responded(response));
        }
        LocalExecutionRequestOutcome::Exhausted(outcome) => exhausted = Some(outcome),
        LocalExecutionRequestOutcome::NoPath => {}
    }

    match maybe_execute_stream_via_standard_family_decision(
        state,
        parts,
        trace_id,
        decision,
        body_json,
        plan_kind,
        resolve_gemini_stream_spec,
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

pub(crate) async fn maybe_execute_sync_via_local_same_format_provider_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some(spec) = resolve_local_same_format_sync_spec(plan_kind) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let plan_and_reports: Vec<LocalSyncPlanAndReport> =
        build_local_same_format_sync_plan_and_reports(
            state, parts, trace_id, decision, body_json, spec,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
    )
    .await
}

pub(crate) async fn maybe_execute_stream_via_local_same_format_provider_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some(spec) = resolve_local_same_format_stream_spec(plan_kind) else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let plan_and_reports: Vec<LocalStreamPlanAndReport> =
        build_local_same_format_stream_plan_and_reports(
            state, parts, trace_id, decision, body_json, spec,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_stream_plan_and_reports(state, trace_id, decision, plan_kind, plan_and_reports).await
}

pub(crate) async fn maybe_execute_sync_via_local_gemini_files_decision(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    body_is_empty: bool,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let plan_and_reports: Vec<LocalSyncPlanAndReport> =
        build_local_gemini_files_sync_plan_and_reports_for_kind(
            state,
            parts,
            body_json,
            body_base64,
            body_is_empty,
            trace_id,
            decision,
            plan_kind,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
    )
    .await
}

pub(crate) async fn maybe_execute_sync_via_local_image_decision(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let plan_and_reports: Vec<LocalSyncPlanAndReport> =
        build_local_image_sync_plan_and_reports_for_kind(
            state,
            parts,
            body_json,
            body_base64,
            trace_id,
            decision,
            plan_kind,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
    )
    .await
}

pub(crate) async fn maybe_execute_stream_via_local_gemini_files_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let plan_and_reports: Vec<LocalStreamPlanAndReport> =
        build_local_gemini_files_stream_plan_and_reports_for_kind(
            state, parts, trace_id, decision, plan_kind,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_stream_plan_and_reports(state, trace_id, decision, plan_kind, plan_and_reports).await
}

pub(crate) async fn maybe_execute_sync_via_local_video_decision(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let plan_and_reports: Vec<LocalSyncPlanAndReport> =
        build_local_video_sync_plan_and_reports_for_kind(
            state, parts, body_json, trace_id, decision, plan_kind,
        )
        .await?;
    if plan_and_reports.is_empty() {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    }

    execute_sync_plan_and_reports(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        plan_and_reports,
    )
    .await
}

pub(crate) async fn maybe_execute_sync_request(
    state: &AppState,
    parts: &http::request::Parts,
    body_bytes: &axum::body::Bytes,
    trace_id: &str,
    decision: Option<&GatewayControlDecision>,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some(decision) = decision else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };
    #[cfg(not(test))]
    {
        if parts.method != http::Method::POST {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        }
        return maybe_execute_sync_local_path(state, parts, body_bytes, trace_id, decision).await;
    }
    #[cfg(test)]
    {
        if state
            .execution_runtime_override_base_url()
            .unwrap_or_default()
            .is_empty()
            && parts.method != http::Method::POST
        {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        }
        maybe_execute_sync_local_path(state, parts, body_bytes, trace_id, decision).await
    }
}

pub(crate) async fn maybe_execute_stream_request(
    state: &AppState,
    parts: &http::request::Parts,
    body_bytes: &axum::body::Bytes,
    trace_id: &str,
    decision: Option<&GatewayControlDecision>,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some(decision) = decision else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };
    #[cfg(not(test))]
    {
        if parts.method != http::Method::POST {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        }
        return maybe_execute_stream_local_path(state, parts, body_bytes, trace_id, decision).await;
    }
    #[cfg(test)]
    {
        if state
            .execution_runtime_override_base_url()
            .unwrap_or_default()
            .is_empty()
            && parts.method != http::Method::POST
        {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        }
        maybe_execute_stream_local_path(state, parts, body_bytes, trace_id, decision).await
    }
}

pub(crate) fn planner_decision_action(action: &str) -> bool {
    matches!(
        action,
        EXECUTION_RUNTIME_SYNC_DECISION_ACTION | EXECUTION_RUNTIME_STREAM_DECISION_ACTION
    )
}

pub(crate) fn parse_local_request_body(
    parts: &http::request::Parts,
    body_bytes: &axum::body::Bytes,
) -> Option<(serde_json::Value, Option<String>)> {
    parse_direct_request_body(parts, body_bytes)
}

pub(crate) fn decision_payload_is_direct_execution(
    payload: &GatewayControlSyncDecisionResponse,
) -> bool {
    planner_decision_action(payload.action.as_str())
}
