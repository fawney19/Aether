use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::io::Error as IoError;
use std::pin::Pin;
use std::time::Instant;

use axum::body::{to_bytes, Body, Bytes};
use axum::http::header::{CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderName, HeaderValue, Response, StatusCode};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::ai_serving::api::{
    build_core_error_body_for_client_format,
    build_local_gemini_files_stream_attempt_source_for_kind,
    build_local_gemini_files_sync_attempt_source_for_kind,
    build_local_image_stream_attempt_source_for_kind,
    build_local_image_sync_attempt_source_for_kind,
    build_local_openai_chat_stream_attempt_source_for_kind,
    build_local_openai_chat_stream_plan_and_reports_for_kind,
    build_local_openai_chat_sync_attempt_source_for_kind,
    build_local_openai_chat_sync_plan_and_reports_for_kind,
    build_local_openai_responses_stream_attempt_source_for_kind,
    build_local_openai_responses_stream_plan_and_reports_for_kind,
    build_local_openai_responses_sync_attempt_source_for_kind,
    build_local_openai_responses_sync_plan_and_reports_for_kind,
    build_local_same_format_stream_attempt_source, build_local_same_format_stream_plan_and_reports,
    build_local_same_format_sync_attempt_source, build_local_same_format_sync_plan_and_reports,
    build_local_video_sync_attempt_source_for_kind, build_standard_family_stream_attempt_source,
    build_standard_family_sync_attempt_source, parse_direct_request_body,
    resolve_claude_stream_spec, resolve_claude_sync_spec, resolve_gemini_stream_spec,
    resolve_gemini_sync_spec, resolve_local_same_format_stream_spec,
    resolve_local_same_format_sync_spec, set_local_openai_chat_execution_exhausted_diagnostic,
    set_local_openai_image_execution_exhausted_diagnostic, AiStreamAttempt, AiSyncAttempt,
    LocalCoreSyncErrorKind, LocalStandardSpec, EXECUTION_RUNTIME_STREAM_DECISION_ACTION,
    EXECUTION_RUNTIME_SYNC_DECISION_ACTION,
};
use crate::ai_serving::LocalExecutionAttemptSource;
use crate::api::response::{
    attach_control_metadata_headers, build_client_response_from_parts_with_mutator,
};
use crate::constants::{CONTROL_CANDIDATE_ID_HEADER, EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS};
use crate::control::GatewayControlDecision;
use crate::execution_runtime::sync::{
    build_openai_image_sync_json_whitespace_heartbeat_stream,
    build_sync_json_whitespace_heartbeat_stream, execute_execution_runtime_sync,
};
use crate::executor::candidate_loop::{
    execute_stream_attempt_source, execute_sync_attempt_source, execute_sync_plan_and_reports,
    mark_unused_local_candidates,
};
use crate::executor::{
    build_local_execution_exhaustion, record_failed_usage_for_exhausted_request,
    LocalExecutionExhaustion, LocalExecutionRequestOutcome,
};
use crate::handlers::shared::system_config_bool;
use crate::stage_metrics::observe_gateway_stage_ms;
use crate::{AiExecutionDecision, AppState, GatewayError};

const ENABLE_OPENAI_IMAGE_SYNC_HEARTBEAT_CONFIG_KEY: &str = "enable_openai_image_sync_heartbeat";
const ENABLE_STANDARD_TEXT_SYNC_HEARTBEAT_CONFIG_KEY: &str = "enable_standard_text_sync_heartbeat";
const OPENAI_IMAGE_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS: u16 = 502;
const OPENAI_IMAGE_SYNC_HEARTBEAT_EXHAUSTED_STATUS: u16 = 503;
const OPENAI_IMAGE_SYNC_HEARTBEAT_ERROR_MESSAGE_LIMIT: usize = 4096;
const STANDARD_TEXT_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS: u16 = 502;
const STANDARD_TEXT_SYNC_HEARTBEAT_EXHAUSTED_STATUS: u16 = 503;
const STANDARD_TEXT_SYNC_HEARTBEAT_ERROR_MESSAGE_LIMIT: usize = 4096;

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
    let Some((attempt_source, candidate_count)) =
        build_local_openai_chat_sync_attempt_source_for_kind(
            state, parts, trace_id, decision, body_json, plan_kind,
        )
        .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    if standard_text_sync_heartbeat_should_wrap(state, plan_kind).await {
        let parts_for_task = parts.clone();
        let body_json_for_task = body_json.clone();
        return Ok(LocalExecutionRequestOutcome::responded(
            build_standard_text_sync_heartbeat_shell_response(
                state.clone(),
                parts_for_task,
                trace_id.to_string(),
                decision.clone(),
                plan_kind.to_string(),
                move |state, parts, trace_id, decision, plan_kind, started_at| async move {
                    let Some((attempt_source, candidate_count)) =
                        build_local_openai_chat_sync_attempt_source_for_kind(
                            &state,
                            &parts,
                            trace_id.as_str(),
                            &decision,
                            &body_json_for_task,
                            plan_kind.as_str(),
                        )
                        .await?
                    else {
                        return Ok(LocalExecutionRequestOutcome::NoPath);
                    };

                    let outcome = execute_sync_attempt_source::<AiSyncAttempt, _>(
                        &state,
                        &parts,
                        trace_id.as_str(),
                        &decision,
                        plan_kind.as_str(),
                        attempt_source,
                    )
                    .await?;
                    match outcome {
                        LocalExecutionRequestOutcome::Exhausted(exhaustion) => {
                            set_local_openai_chat_execution_exhausted_diagnostic(
                                &state,
                                trace_id.as_str(),
                                &decision,
                                plan_kind.as_str(),
                                &body_json_for_task,
                                candidate_count,
                            );
                            record_standard_text_sync_heartbeat_exhaustion(
                                &state,
                                exhaustion,
                                &started_at,
                            )
                            .await;
                            Ok(LocalExecutionRequestOutcome::NoPath)
                        }
                        outcome => Ok(outcome),
                    }
                },
            )?,
        ));
    }

    let outcome = execute_sync_attempt_source::<AiSyncAttempt, _>(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await?;

    if let LocalExecutionRequestOutcome::Exhausted(_) = &outcome {
        set_local_openai_chat_execution_exhausted_diagnostic(
            state,
            trace_id,
            decision,
            plan_kind,
            body_json,
            candidate_count,
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
    let attempt_source_started_at = std::time::Instant::now();
    let attempt_source = build_local_openai_chat_stream_attempt_source_for_kind(
        state, parts, trace_id, decision, body_json, plan_kind,
    )
    .await;
    observe_gateway_stage_ms(
        "stream_openai_chat_attempt_source_init",
        attempt_source_started_at.elapsed().as_millis() as u64,
    );
    let Some((attempt_source, candidate_count)) = attempt_source? else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let attempt_source_execute_started_at = std::time::Instant::now();
    let outcome = execute_stream_attempt_source::<AiStreamAttempt, _>(
        state,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await;
    observe_gateway_stage_ms(
        "stream_openai_chat_attempt_source_execute",
        attempt_source_execute_started_at.elapsed().as_millis() as u64,
    );
    let outcome = outcome?;

    if let LocalExecutionRequestOutcome::Exhausted(_) = &outcome {
        set_local_openai_chat_execution_exhausted_diagnostic(
            state,
            trace_id,
            decision,
            plan_kind,
            body_json,
            candidate_count,
        );
    }

    Ok(outcome)
}

pub(crate) async fn maybe_execute_sync_via_local_openai_responses_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some((attempt_source, _candidate_count)) =
        build_local_openai_responses_sync_attempt_source_for_kind(
            state, parts, trace_id, decision, body_json, plan_kind,
        )
        .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    if standard_text_sync_heartbeat_should_wrap(state, plan_kind).await {
        let parts_for_task = parts.clone();
        let body_json_for_task = body_json.clone();
        return Ok(LocalExecutionRequestOutcome::responded(
            build_standard_text_sync_heartbeat_shell_response(
                state.clone(),
                parts_for_task,
                trace_id.to_string(),
                decision.clone(),
                plan_kind.to_string(),
                move |state, parts, trace_id, decision, plan_kind, started_at| async move {
                    let Some((attempt_source, _candidate_count)) =
                        build_local_openai_responses_sync_attempt_source_for_kind(
                            &state,
                            &parts,
                            trace_id.as_str(),
                            &decision,
                            &body_json_for_task,
                            plan_kind.as_str(),
                        )
                        .await?
                    else {
                        return Ok(LocalExecutionRequestOutcome::NoPath);
                    };

                    let outcome = execute_sync_attempt_source::<AiSyncAttempt, _>(
                        &state,
                        &parts,
                        trace_id.as_str(),
                        &decision,
                        plan_kind.as_str(),
                        attempt_source,
                    )
                    .await?;
                    match outcome {
                        LocalExecutionRequestOutcome::Exhausted(exhaustion) => {
                            record_standard_text_sync_heartbeat_exhaustion(
                                &state,
                                exhaustion,
                                &started_at,
                            )
                            .await;
                            Ok(LocalExecutionRequestOutcome::NoPath)
                        }
                        outcome => Ok(outcome),
                    }
                },
            )?,
        ));
    }

    execute_sync_attempt_source::<AiSyncAttempt, _>(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await
}

pub(crate) async fn maybe_execute_stream_via_local_openai_responses_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    body_json: &serde_json::Value,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some((attempt_source, _candidate_count)) =
        build_local_openai_responses_stream_attempt_source_for_kind(
            state, parts, trace_id, decision, body_json, plan_kind,
        )
        .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_stream_attempt_source::<AiStreamAttempt, _>(
        state,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await
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

    let Some((attempt_source, _candidate_count)) = build_standard_family_sync_attempt_source(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    if standard_text_sync_heartbeat_should_wrap(state, plan_kind).await {
        let parts_for_task = parts.clone();
        let body_json_for_task = body_json.clone();
        return Ok(LocalExecutionRequestOutcome::responded(
            build_standard_text_sync_heartbeat_shell_response(
                state.clone(),
                parts_for_task,
                trace_id.to_string(),
                decision.clone(),
                plan_kind.to_string(),
                move |state, parts, trace_id, decision, plan_kind, started_at| async move {
                    let Some((attempt_source, _candidate_count)) =
                        build_standard_family_sync_attempt_source(
                            &state,
                            &parts,
                            trace_id.as_str(),
                            &decision,
                            &body_json_for_task,
                            spec,
                        )
                        .await?
                    else {
                        return Ok(LocalExecutionRequestOutcome::NoPath);
                    };

                    let outcome = execute_sync_attempt_source::<AiSyncAttempt, _>(
                        &state,
                        &parts,
                        trace_id.as_str(),
                        &decision,
                        plan_kind.as_str(),
                        attempt_source,
                    )
                    .await?;
                    match outcome {
                        LocalExecutionRequestOutcome::Exhausted(exhaustion) => {
                            record_standard_text_sync_heartbeat_exhaustion(
                                &state,
                                exhaustion,
                                &started_at,
                            )
                            .await;
                            Ok(LocalExecutionRequestOutcome::NoPath)
                        }
                        outcome => Ok(outcome),
                    }
                },
            )?,
        ));
    }

    execute_sync_attempt_source::<AiSyncAttempt, _>(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
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

    let Some((attempt_source, _candidate_count)) = build_standard_family_stream_attempt_source(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_stream_attempt_source::<AiStreamAttempt, _>(
        state,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await
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

    let Some((attempt_source, _candidate_count)) = build_local_same_format_sync_attempt_source(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    if standard_text_sync_heartbeat_should_wrap(state, plan_kind).await {
        let parts_for_task = parts.clone();
        let body_json_for_task = body_json.clone();
        return Ok(LocalExecutionRequestOutcome::responded(
            build_standard_text_sync_heartbeat_shell_response(
                state.clone(),
                parts_for_task,
                trace_id.to_string(),
                decision.clone(),
                plan_kind.to_string(),
                move |state, parts, trace_id, decision, plan_kind, started_at| async move {
                    let Some((attempt_source, _candidate_count)) =
                        build_local_same_format_sync_attempt_source(
                            &state,
                            &parts,
                            trace_id.as_str(),
                            &decision,
                            &body_json_for_task,
                            spec,
                        )
                        .await?
                    else {
                        return Ok(LocalExecutionRequestOutcome::NoPath);
                    };

                    let outcome = execute_sync_attempt_source::<AiSyncAttempt, _>(
                        &state,
                        &parts,
                        trace_id.as_str(),
                        &decision,
                        plan_kind.as_str(),
                        attempt_source,
                    )
                    .await?;
                    match outcome {
                        LocalExecutionRequestOutcome::Exhausted(exhaustion) => {
                            record_standard_text_sync_heartbeat_exhaustion(
                                &state,
                                exhaustion,
                                &started_at,
                            )
                            .await;
                            Ok(LocalExecutionRequestOutcome::NoPath)
                        }
                        outcome => Ok(outcome),
                    }
                },
            )?,
        ));
    }

    execute_sync_attempt_source::<AiSyncAttempt, _>(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
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

    let Some((attempt_source, _candidate_count)) = build_local_same_format_stream_attempt_source(
        state, parts, trace_id, decision, body_json, spec,
    )
    .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_stream_attempt_source::<AiStreamAttempt, _>(
        state,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await
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
    let Some((attempt_source, _candidate_count)) =
        build_local_gemini_files_sync_attempt_source_for_kind(
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
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_sync_attempt_source::<AiSyncAttempt, _>(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await
}

async fn openai_image_sync_heartbeat_enabled(state: &AppState) -> bool {
    match state
        .read_system_config_json_value(ENABLE_OPENAI_IMAGE_SYNC_HEARTBEAT_CONFIG_KEY)
        .await
    {
        Ok(value) => system_config_bool(value.as_ref(), false),
        Err(err) => {
            tracing::warn!(
                event_name = "openai_image_sync_heartbeat_config_read_failed",
                log_type = "ops",
                error = ?err,
                "gateway failed to read sync image heartbeat config; defaulting disabled"
            );
            false
        }
    }
}

async fn standard_text_sync_heartbeat_enabled(state: &AppState) -> bool {
    match state
        .read_system_config_json_value(ENABLE_STANDARD_TEXT_SYNC_HEARTBEAT_CONFIG_KEY)
        .await
    {
        Ok(value) => system_config_bool(value.as_ref(), false),
        Err(err) => {
            tracing::warn!(
                event_name = "standard_text_sync_heartbeat_config_read_failed",
                log_type = "ops",
                error = ?err,
                "gateway failed to read standard text sync heartbeat config; defaulting disabled"
            );
            false
        }
    }
}

fn standard_text_sync_heartbeat_applies_to_plan_kind(plan_kind: &str) -> bool {
    matches!(
        plan_kind,
        "openai_chat_sync"
            | "openai_responses_sync"
            | "openai_responses_compact_sync"
            | "claude_chat_sync"
            | "claude_cli_sync"
            | "gemini_chat_sync"
            | "gemini_cli_sync"
    )
}

async fn standard_text_sync_heartbeat_should_wrap(state: &AppState, plan_kind: &str) -> bool {
    standard_text_sync_heartbeat_applies_to_plan_kind(plan_kind)
        && standard_text_sync_heartbeat_enabled(state).await
}

fn standard_text_sync_heartbeat_client_api_format_for_plan_kind(plan_kind: &str) -> &'static str {
    match plan_kind {
        "openai_responses_sync" => "openai:responses",
        "openai_responses_compact_sync" => "openai:responses:compact",
        "claude_chat_sync" | "claude_cli_sync" => "claude:messages",
        "gemini_chat_sync" | "gemini_cli_sync" => "gemini:generate_content",
        _ => "openai:chat",
    }
}

fn build_standard_text_sync_heartbeat_shell_response<F, Fut>(
    state: AppState,
    parts: http::request::Parts,
    trace_id: String,
    decision: GatewayControlDecision,
    plan_kind: String,
    execute: F,
) -> Result<Response<Body>, GatewayError>
where
    F: FnOnce(
            AppState,
            http::request::Parts,
            String,
            GatewayControlDecision,
            String,
            Instant,
        ) -> Fut
        + Send
        + 'static,
    Fut: std::future::Future<Output = Result<LocalExecutionRequestOutcome, GatewayError>>
        + Send
        + 'static,
{
    let request_id = (!trace_id.trim().is_empty()).then(|| trace_id.clone());
    let client_api_format =
        standard_text_sync_heartbeat_client_api_format_for_plan_kind(plan_kind.as_str())
            .to_string();
    let redaction_slot = parts
        .extensions
        .get::<crate::privacy::RedactionSessionSlot>()
        .cloned();
    let trace_id_for_response = trace_id.clone();
    let decision_for_response = decision.clone();
    let started_at = Instant::now();
    let (tx, rx) = mpsc::channel::<Result<Bytes, IoError>>(1);

    tokio::spawn(async move {
        let bytes = standard_text_sync_heartbeat_final_bytes(
            client_api_format.as_str(),
            redaction_slot.as_ref(),
            execute(state, parts, trace_id, decision, plan_kind, started_at).await,
        )
        .await;
        let _ = tx.send(Ok(Bytes::from(bytes))).await;
    });

    let headers = BTreeMap::from([(
        CONTENT_TYPE.as_str().to_string(),
        "application/json".to_string(),
    )]);
    let response = build_client_response_from_parts_with_mutator(
        StatusCode::OK.as_u16(),
        &headers,
        Body::from_stream(build_sync_json_whitespace_heartbeat_stream(rx)),
        trace_id_for_response.as_str(),
        Some(&decision_for_response),
        |headers| {
            headers.remove(CONTENT_LENGTH);
            headers.remove(CONTENT_ENCODING);
            headers.insert(
                CACHE_CONTROL,
                HeaderValue::from_static("no-cache, no-transform"),
            );
            headers.insert(
                HeaderName::from_static("x-accel-buffering"),
                HeaderValue::from_static("no"),
            );
            Ok(())
        },
    )?;
    attach_control_metadata_headers(response, request_id.as_deref(), None)
}

async fn record_standard_text_sync_heartbeat_exhaustion(
    state: &AppState,
    exhaustion: LocalExecutionExhaustion,
    started_at: &Instant,
) {
    record_failed_usage_for_exhausted_request(
        state,
        exhaustion,
        started_at,
        "Standard text sync heartbeat exhausted all local candidates",
        EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS,
        None,
    )
    .await;
}

async fn standard_text_sync_heartbeat_final_bytes(
    client_api_format: &str,
    redaction_slot: Option<&crate::privacy::RedactionSessionSlot>,
    result: Result<LocalExecutionRequestOutcome, GatewayError>,
) -> Vec<u8> {
    match result {
        Ok(LocalExecutionRequestOutcome::Responded(response)) => {
            standard_text_sync_heartbeat_response_body_bytes(
                client_api_format,
                redaction_slot,
                response,
            )
            .await
        }
        Ok(LocalExecutionRequestOutcome::Exhausted(_))
        | Ok(LocalExecutionRequestOutcome::NoPath) => standard_text_sync_heartbeat_error_body(
            client_api_format,
            STANDARD_TEXT_SYNC_HEARTBEAT_EXHAUSTED_STATUS,
            "standard text sync exhausted all local candidates",
        ),
        Err(err) => standard_text_sync_heartbeat_error_body(
            client_api_format,
            STANDARD_TEXT_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS,
            &format!("{err:?}"),
        ),
    }
}

async fn standard_text_sync_heartbeat_response_body_bytes(
    client_api_format: &str,
    redaction_slot: Option<&crate::privacy::RedactionSessionSlot>,
    response: Response<Body>,
) -> Vec<u8> {
    let status_code = response.status().as_u16();
    let (parts, body) = response.into_parts();
    match to_bytes(body, usize::MAX).await {
        Ok(bytes) => {
            let body = match standard_text_sync_heartbeat_restore_response_body(
                redaction_slot,
                &parts.headers,
                bytes.as_ref(),
            ) {
                Ok(body) => body,
                Err(err) => {
                    return standard_text_sync_heartbeat_error_body(
                        client_api_format,
                        STANDARD_TEXT_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS,
                        &format!("{err:?}"),
                    );
                }
            };
            if (200..300).contains(&status_code) && !body.is_empty() {
                return body;
            }
            if !(200..300).contains(&status_code) {
                return standard_text_sync_heartbeat_error_body_from_response(
                    client_api_format,
                    status_code,
                    body.as_ref(),
                );
            }
            standard_text_sync_heartbeat_error_body(
                client_api_format,
                STANDARD_TEXT_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS,
                "empty standard text sync response",
            )
        }
        Err(err) => standard_text_sync_heartbeat_error_body(
            client_api_format,
            STANDARD_TEXT_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS,
            &err.to_string(),
        ),
    }
}

fn standard_text_sync_heartbeat_restore_response_body(
    redaction_slot: Option<&crate::privacy::RedactionSessionSlot>,
    headers: &http::HeaderMap,
    body: &[u8],
) -> Result<Vec<u8>, GatewayError> {
    let Some(redaction_slot) = redaction_slot else {
        return Ok(body.to_vec());
    };
    let candidate_id = headers
        .get(CONTROL_CANDIDATE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(session) = redaction_slot.take_for_candidate(candidate_id) else {
        return Ok(body.to_vec());
    };
    let mut header_values = headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    crate::privacy::restore_sync_response_body(&mut header_values, body, &session)
        .map(|restored| restored.body)
}

fn standard_text_sync_heartbeat_error_body_from_response(
    client_api_format: &str,
    status_code: u16,
    body: &[u8],
) -> Vec<u8> {
    if let Ok(mut value) = serde_json::from_slice::<Value>(body) {
        if standard_text_sync_heartbeat_insert_upstream_status(&mut value, status_code) {
            return serde_json::to_vec(&value).unwrap_or_else(|_| {
                standard_text_sync_heartbeat_error_body(
                    client_api_format,
                    status_code,
                    &format!("upstream returned status {status_code}"),
                )
            });
        }
    }

    let message = standard_text_sync_heartbeat_error_message_from_body(status_code, body);
    standard_text_sync_heartbeat_error_body(client_api_format, status_code, message.as_str())
}

fn standard_text_sync_heartbeat_insert_upstream_status(
    value: &mut Value,
    status_code: u16,
) -> bool {
    let Some(error) = value.get_mut("error").and_then(Value::as_object_mut) else {
        return false;
    };
    error.insert("upstream_status".to_string(), Value::from(status_code));
    error
        .entry("message".to_string())
        .or_insert_with(|| Value::String(format!("upstream returned status {status_code}")));
    true
}

fn standard_text_sync_heartbeat_error_message_from_body(status_code: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body).trim().to_string();
    if text.is_empty() {
        return format!("upstream returned status {status_code}");
    }
    text.chars()
        .take(STANDARD_TEXT_SYNC_HEARTBEAT_ERROR_MESSAGE_LIMIT)
        .collect()
}

fn standard_text_sync_heartbeat_error_body(
    client_api_format: &str,
    status_code: u16,
    message: &str,
) -> Vec<u8> {
    let mut body = build_core_error_body_for_client_format(
        client_api_format,
        message,
        Some("upstream_error"),
        standard_text_sync_heartbeat_error_kind(status_code),
    )
    .unwrap_or_else(|| {
        json!({
            "error": {
                "type": "upstream_error",
                "message": message,
                "code": status_code,
            }
        })
    });
    if !standard_text_sync_heartbeat_insert_upstream_status(&mut body, status_code) {
        body = json!({
            "error": {
                "type": "upstream_error",
                "message": message,
                "code": status_code,
                "upstream_status": status_code,
            }
        });
    }
    serde_json::to_vec(&body).unwrap_or_else(|_| {
        format!(
            "{{\"error\":{{\"type\":\"upstream_error\",\"code\":{status_code},\"upstream_status\":{status_code}}}}}"
        )
        .into_bytes()
    })
}

fn standard_text_sync_heartbeat_error_kind(status_code: u16) -> LocalCoreSyncErrorKind {
    match status_code {
        400 => LocalCoreSyncErrorKind::InvalidRequest,
        401 => LocalCoreSyncErrorKind::Authentication,
        403 => LocalCoreSyncErrorKind::PermissionDenied,
        404 => LocalCoreSyncErrorKind::NotFound,
        413 => LocalCoreSyncErrorKind::ContextLengthExceeded,
        429 => LocalCoreSyncErrorKind::RateLimit,
        503 => LocalCoreSyncErrorKind::Overloaded,
        _ => LocalCoreSyncErrorKind::ServerError,
    }
}

fn build_openai_image_sync_heartbeat_shell_response(
    state: AppState,
    request_path: String,
    trace_id: String,
    decision: GatewayControlDecision,
    plan_kind: String,
    attempts: Vec<AiSyncAttempt>,
) -> Result<Response<Body>, GatewayError> {
    let request_id = attempts
        .first()
        .map(|attempt| attempt.plan.request_id.clone())
        .filter(|value| !value.trim().is_empty());
    let trace_id_for_response = trace_id.clone();
    let decision_for_response = decision.clone();
    let started_at = Instant::now();
    let (tx, rx) = mpsc::channel::<Result<Bytes, IoError>>(1);

    tokio::spawn(async move {
        let bytes = openai_image_sync_heartbeat_final_bytes(
            execute_openai_image_sync_heartbeat_attempts(
                state,
                request_path,
                trace_id,
                decision,
                plan_kind,
                attempts,
                started_at,
            )
            .await,
        )
        .await;
        let _ = tx.send(Ok(Bytes::from(bytes))).await;
    });

    let headers = BTreeMap::from([(
        CONTENT_TYPE.as_str().to_string(),
        "application/json".to_string(),
    )]);
    let response = build_client_response_from_parts_with_mutator(
        StatusCode::OK.as_u16(),
        &headers,
        Body::from_stream(build_openai_image_sync_json_whitespace_heartbeat_stream(rx)),
        trace_id_for_response.as_str(),
        Some(&decision_for_response),
        |headers| {
            headers.remove(CONTENT_LENGTH);
            headers.remove(CONTENT_ENCODING);
            headers.insert(
                CACHE_CONTROL,
                HeaderValue::from_static("no-cache, no-transform"),
            );
            headers.insert(
                HeaderName::from_static("x-accel-buffering"),
                HeaderValue::from_static("no"),
            );
            Ok(())
        },
    )?;
    attach_control_metadata_headers(response, request_id.as_deref(), None)
}

async fn execute_openai_image_sync_heartbeat_attempts(
    state: AppState,
    request_path: String,
    trace_id: String,
    decision: GatewayControlDecision,
    plan_kind: String,
    attempts: Vec<AiSyncAttempt>,
    started_at: Instant,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let mut attempts = VecDeque::from(attempts);
    let mut last_attempted = None;

    while let Some(attempt) = attempts.pop_front() {
        let plan = attempt.plan;
        let report_kind = attempt.report_kind;
        let report_context = attempt.report_context;
        last_attempted = Some((plan.clone(), report_context.clone()));
        match execute_execution_runtime_sync(
            &state,
            request_path.as_str(),
            plan,
            trace_id.as_str(),
            &decision,
            plan_kind.as_str(),
            report_kind,
            report_context,
        )
        .await?
        {
            Some(response) => {
                mark_unused_local_candidates(&state, attempts.into_iter().collect()).await;
                return Ok(LocalExecutionRequestOutcome::responded(response));
            }
            None => continue,
        }
    }

    let Some((last_plan, last_report_context)) = last_attempted else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };
    let exhaustion =
        build_local_execution_exhaustion(&state, &last_plan, last_report_context.as_ref()).await;
    record_failed_usage_for_exhausted_request(
        &state,
        exhaustion,
        &started_at,
        "OpenAI image sync heartbeat exhausted all local candidates",
        EXECUTION_PATH_LOCAL_EXECUTION_RUNTIME_MISS,
        None,
    )
    .await;
    Ok(LocalExecutionRequestOutcome::NoPath)
}

async fn openai_image_sync_heartbeat_final_bytes(
    result: Result<LocalExecutionRequestOutcome, GatewayError>,
) -> Vec<u8> {
    match result {
        Ok(LocalExecutionRequestOutcome::Responded(response)) => {
            openai_image_sync_heartbeat_response_body_bytes(response).await
        }
        Ok(LocalExecutionRequestOutcome::Exhausted(_))
        | Ok(LocalExecutionRequestOutcome::NoPath) => openai_image_sync_heartbeat_error_body(
            OPENAI_IMAGE_SYNC_HEARTBEAT_EXHAUSTED_STATUS,
            "OpenAI image sync exhausted all local candidates",
        ),
        Err(err) => openai_image_sync_heartbeat_error_body(
            OPENAI_IMAGE_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS,
            &format!("{err:?}"),
        ),
    }
}

async fn openai_image_sync_heartbeat_response_body_bytes(response: Response<Body>) -> Vec<u8> {
    let status_code = response.status().as_u16();
    match to_bytes(response.into_body(), usize::MAX).await {
        Ok(bytes) if status_code < 400 && !bytes.is_empty() => bytes.to_vec(),
        Ok(bytes) if status_code >= 400 => {
            openai_image_sync_heartbeat_error_body_from_response(status_code, bytes.as_ref())
        }
        Ok(_) => openai_image_sync_heartbeat_error_body(
            OPENAI_IMAGE_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS,
            "empty sync image response",
        ),
        Err(err) => openai_image_sync_heartbeat_error_body(
            OPENAI_IMAGE_SYNC_HEARTBEAT_INTERNAL_ERROR_STATUS,
            &err.to_string(),
        ),
    }
}

fn openai_image_sync_heartbeat_error_body_from_response(status_code: u16, body: &[u8]) -> Vec<u8> {
    if let Ok(mut value) = serde_json::from_slice::<Value>(body) {
        if let Some(error) = value.get_mut("error").and_then(Value::as_object_mut) {
            error.insert("upstream_status".to_string(), Value::from(status_code));
            error
                .entry("type".to_string())
                .or_insert_with(|| Value::String("upstream_error".to_string()));
            error.entry("message".to_string()).or_insert_with(|| {
                Value::String(format!("upstream returned status {status_code}"))
            });
            return serde_json::to_vec(&value).unwrap_or_else(|_| {
                openai_image_sync_heartbeat_error_body(
                    status_code,
                    &format!("upstream returned status {status_code}"),
                )
            });
        }
    }

    let message = openai_image_sync_heartbeat_error_message_from_body(status_code, body);
    openai_image_sync_heartbeat_error_body(status_code, message.as_str())
}

fn openai_image_sync_heartbeat_error_message_from_body(status_code: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body).trim().to_string();
    if text.is_empty() {
        return format!("upstream returned status {status_code}");
    }
    text.chars()
        .take(OPENAI_IMAGE_SYNC_HEARTBEAT_ERROR_MESSAGE_LIMIT)
        .collect()
}

fn openai_image_sync_heartbeat_error_body(status_code: u16, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "error": {
            "type": "upstream_error",
            "message": message,
            "code": status_code,
            "upstream_status": status_code,
        }
    }))
    .unwrap_or_else(|_| {
        format!(
            "{{\"error\":{{\"type\":\"upstream_error\",\"code\":{status_code},\"upstream_status\":{status_code}}}}}"
        )
        .into_bytes()
    })
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
    let Some((mut attempt_source, candidate_count)) =
        build_local_image_sync_attempt_source_for_kind(
            state,
            parts,
            body_json,
            body_base64,
            trace_id,
            decision,
            plan_kind,
        )
        .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    if openai_image_sync_heartbeat_enabled(state).await {
        let mut attempts = Vec::new();
        while let Some(attempt) = attempt_source.next_execution_attempt().await? {
            attempts.push(attempt);
        }
        return Ok(LocalExecutionRequestOutcome::responded(
            build_openai_image_sync_heartbeat_shell_response(
                state.clone(),
                parts.uri.path().to_string(),
                trace_id.to_string(),
                decision.clone(),
                plan_kind.to_string(),
                attempts,
            )?,
        ));
    }

    let outcome = execute_sync_attempt_source::<AiSyncAttempt, _>(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await?;

    if let LocalExecutionRequestOutcome::Exhausted(_) = &outcome {
        set_local_openai_image_execution_exhausted_diagnostic(
            state,
            trace_id,
            decision,
            plan_kind,
            body_json,
            candidate_count,
        );
    }

    Ok(outcome)
}

pub(crate) async fn maybe_execute_stream_via_local_gemini_files_decision(
    state: &AppState,
    parts: &http::request::Parts,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some((attempt_source, _candidate_count)) =
        build_local_gemini_files_stream_attempt_source_for_kind(
            state, parts, trace_id, decision, plan_kind,
        )
        .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_stream_attempt_source::<AiStreamAttempt, _>(
        state,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await
}

pub(crate) async fn maybe_execute_stream_via_local_image_decision(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    body_base64: Option<&str>,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some((attempt_source, candidate_count)) = build_local_image_stream_attempt_source_for_kind(
        state,
        parts,
        body_json,
        body_base64,
        trace_id,
        decision,
        plan_kind,
    )
    .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    let outcome = execute_stream_attempt_source::<AiStreamAttempt, _>(
        state,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await?;

    if let LocalExecutionRequestOutcome::Exhausted(_) = &outcome {
        set_local_openai_image_execution_exhausted_diagnostic(
            state,
            trace_id,
            decision,
            plan_kind,
            body_json,
            candidate_count,
        );
    }

    Ok(outcome)
}

pub(crate) async fn maybe_execute_sync_via_local_video_decision(
    state: &AppState,
    parts: &http::request::Parts,
    body_json: &serde_json::Value,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
) -> Result<LocalExecutionRequestOutcome, GatewayError> {
    let Some((attempt_source, _candidate_count)) = build_local_video_sync_attempt_source_for_kind(
        state, parts, body_json, trace_id, decision, plan_kind,
    )
    .await?
    else {
        return Ok(LocalExecutionRequestOutcome::NoPath);
    };

    execute_sync_attempt_source::<AiSyncAttempt, _>(
        state,
        parts,
        trace_id,
        decision,
        plan_kind,
        attempt_source,
    )
    .await
}

pub(crate) fn maybe_execute_sync_request<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_bytes: &'a axum::body::Bytes,
    trace_id: &'a str,
    decision: Option<&'a GatewayControlDecision>,
) -> Pin<Box<dyn Future<Output = Result<LocalExecutionRequestOutcome, GatewayError>> + Send + 'a>> {
    Box::pin(async move {
        let Some(decision) = decision else {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        };
        #[cfg(not(test))]
        {
            if parts.method != http::Method::POST {
                return Ok(LocalExecutionRequestOutcome::NoPath);
            }
            return maybe_execute_sync_local_path(state, parts, body_bytes, trace_id, decision)
                .await;
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
    })
}

pub(crate) fn maybe_execute_stream_request<'a>(
    state: &'a AppState,
    parts: &'a http::request::Parts,
    body_bytes: &'a axum::body::Bytes,
    trace_id: &'a str,
    decision: Option<&'a GatewayControlDecision>,
) -> Pin<Box<dyn Future<Output = Result<LocalExecutionRequestOutcome, GatewayError>> + Send + 'a>> {
    Box::pin(async move {
        let Some(decision) = decision else {
            return Ok(LocalExecutionRequestOutcome::NoPath);
        };
        #[cfg(not(test))]
        {
            if parts.method != http::Method::POST {
                return Ok(LocalExecutionRequestOutcome::NoPath);
            }
            return maybe_execute_stream_local_path(state, parts, body_bytes, trace_id, decision)
                .await;
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
    })
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

pub(crate) fn decision_payload_is_direct_execution(payload: &AiExecutionDecision) -> bool {
    planner_decision_action(payload.action.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const TEST_OPENAI_IMAGE_SYNC_PLAN_KIND: &str = "openai_image_sync";
    const TEST_STANDARD_TEXT_SYNC_PLAN_KIND: &str = "openai_responses_compact_sync";

    struct TestSyncAttemptSource {
        attempts: VecDeque<AiSyncAttempt>,
    }

    impl TestSyncAttemptSource {
        fn new(attempts: Vec<AiSyncAttempt>) -> Self {
            Self {
                attempts: VecDeque::from(attempts),
            }
        }
    }

    #[async_trait::async_trait]
    impl LocalExecutionAttemptSource<AiSyncAttempt> for TestSyncAttemptSource {
        async fn next_execution_attempt(&mut self) -> Result<Option<AiSyncAttempt>, GatewayError> {
            Ok(self.attempts.pop_front())
        }

        async fn drain_execution_attempts(&mut self) -> Result<Vec<AiSyncAttempt>, GatewayError> {
            Ok(self.attempts.drain(..).collect())
        }
    }

    fn test_openai_image_heartbeat_decision() -> GatewayControlDecision {
        GatewayControlDecision::synthetic(
            "/v1/images/generations",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("image".to_string()),
            Some("openai:image".to_string()),
        )
        .with_execution_runtime_candidate(true)
    }

    fn test_openai_image_heartbeat_plan(
        endpoint_id: &str,
        candidate_id: &str,
    ) -> aether_contracts::ExecutionPlan {
        aether_contracts::ExecutionPlan {
            request_id: "trace-image-heartbeat-retry".to_string(),
            candidate_id: Some(candidate_id.to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-openai".to_string(),
            endpoint_id: endpoint_id.to_string(),
            key_id: "key-openai".to_string(),
            method: "POST".to_string(),
            url: "https://api.openai.com/v1/images/generations".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: aether_contracts::RequestBody::from_json(json!({"prompt": "test"})),
            stream: false,
            client_api_format: "openai:image".to_string(),
            provider_api_format: "openai:image".to_string(),
            model_name: Some("gpt-image-1".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn test_openai_image_heartbeat_attempt(
        candidate_index: u32,
        endpoint_id: &str,
        candidate_id: &str,
    ) -> AiSyncAttempt {
        AiSyncAttempt {
            plan: test_openai_image_heartbeat_plan(endpoint_id, candidate_id),
            report_kind: None,
            report_context: Some(json!({
                "candidate_index": candidate_index,
                "retry_index": 0,
            })),
        }
    }

    fn test_openai_image_execution_result(
        plan: &aether_contracts::ExecutionPlan,
        status_code: u16,
        body_json: Value,
    ) -> aether_contracts::ExecutionResult {
        aether_contracts::ExecutionResult {
            request_id: plan.request_id.clone(),
            candidate_id: plan.candidate_id.clone(),
            status_code,
            headers: BTreeMap::from([(
                CONTENT_TYPE.as_str().to_string(),
                "application/json".to_string(),
            )]),
            body: Some(aether_contracts::ResponseBody {
                json_body: Some(body_json),
                body_bytes_b64: None,
            }),
            telemetry: Some(aether_contracts::ExecutionTelemetry {
                ttfb_ms: None,
                elapsed_ms: Some(10),
                upstream_bytes: None,
            }),
            error: None,
        }
    }

    fn test_standard_text_heartbeat_decision() -> GatewayControlDecision {
        GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("responses".to_string()),
            Some("openai:responses:compact".to_string()),
        )
        .with_execution_runtime_candidate(true)
    }

    fn test_standard_text_heartbeat_plan(
        endpoint_id: &str,
        candidate_id: &str,
        client_api_format: &str,
    ) -> aether_contracts::ExecutionPlan {
        aether_contracts::ExecutionPlan {
            request_id: "trace-standard-text-heartbeat-retry".to_string(),
            candidate_id: Some(candidate_id.to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-openai".to_string(),
            endpoint_id: endpoint_id.to_string(),
            key_id: "key-openai".to_string(),
            method: "POST".to_string(),
            url: "https://api.openai.com/v1/responses".to_string(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".to_string()),
            content_encoding: None,
            body: aether_contracts::RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: client_api_format.to_string(),
            provider_api_format: client_api_format.to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn test_standard_text_heartbeat_attempt(
        candidate_index: u32,
        endpoint_id: &str,
        candidate_id: &str,
        client_api_format: &str,
    ) -> AiSyncAttempt {
        AiSyncAttempt {
            plan: test_standard_text_heartbeat_plan(endpoint_id, candidate_id, client_api_format),
            report_kind: None,
            report_context: Some(json!({
                "candidate_index": candidate_index,
                "retry_index": 0,
                "client_api_format": client_api_format,
                "provider_api_format": client_api_format,
            })),
        }
    }

    #[tokio::test]
    async fn openai_image_sync_heartbeat_success_body_is_unchanged() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(r#"{"data":[{"b64_json":"x"}]}"#))
            .expect("response should build");

        let bytes = openai_image_sync_heartbeat_response_body_bytes(response).await;
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(body, json!({"data": [{"b64_json": "x"}]}));
    }

    #[tokio::test]
    async fn openai_image_sync_heartbeat_missing_config_defaults_disabled() {
        let state = AppState::new().expect("state should build");

        assert!(!openai_image_sync_heartbeat_enabled(&state).await);
    }

    #[tokio::test]
    async fn openai_image_sync_heartbeat_error_body_includes_upstream_status() {
        let response = Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .body(Body::from(
                r#"{"error":{"type":"rate_limit","message":"slow down"}}"#,
            ))
            .expect("response should build");

        let bytes = openai_image_sync_heartbeat_response_body_bytes(response).await;
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(body["error"]["type"], json!("rate_limit"));
        assert_eq!(body["error"]["message"], json!("slow down"));
        assert_eq!(body["error"]["upstream_status"], json!(429));
    }

    #[test]
    fn openai_image_sync_heartbeat_non_json_error_body_is_wrapped() {
        let bytes =
            openai_image_sync_heartbeat_error_body_from_response(502, b"bad gateway from upstream");
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(body["error"]["type"], json!("upstream_error"));
        assert_eq!(body["error"]["message"], json!("bad gateway from upstream"));
        assert_eq!(body["error"]["upstream_status"], json!(502));
    }

    #[tokio::test]
    async fn openai_image_sync_heartbeat_no_path_returns_json_error_body() {
        let bytes =
            openai_image_sync_heartbeat_final_bytes(Ok(LocalExecutionRequestOutcome::NoPath)).await;
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(body["error"]["type"], json!("upstream_error"));
        assert_eq!(body["error"]["upstream_status"], json!(503));
    }

    #[tokio::test]
    async fn openai_image_sync_heartbeat_attempts_retry_first_candidate_then_return_second() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_for_override = Arc::clone(&call_count);
        let state = AppState::new()
            .expect("state should build")
            .with_execution_runtime_sync_override_for_tests(move |plan| {
                call_count_for_override.fetch_add(1, Ordering::SeqCst);
                if plan.endpoint_id == "endpoint-retry" {
                    Ok(test_openai_image_execution_result(
                        plan,
                        StatusCode::TOO_MANY_REQUESTS.as_u16(),
                        json!({"error": {"message": "retry this candidate"}}),
                    ))
                } else {
                    Ok(test_openai_image_execution_result(
                        plan,
                        StatusCode::OK.as_u16(),
                        json!({"data": [{"b64_json": "second-candidate"}]}),
                    ))
                }
            });
        let attempts = vec![
            test_openai_image_heartbeat_attempt(0, "endpoint-retry", "candidate-retry"),
            test_openai_image_heartbeat_attempt(1, "endpoint-success", "candidate-success"),
        ];

        let outcome = execute_openai_image_sync_heartbeat_attempts(
            state,
            "/v1/images/generations".to_string(),
            "trace-image-heartbeat-retry".to_string(),
            test_openai_image_heartbeat_decision(),
            TEST_OPENAI_IMAGE_SYNC_PLAN_KIND.to_string(),
            attempts,
            Instant::now(),
        )
        .await
        .expect("heartbeat attempts should execute");
        let LocalExecutionRequestOutcome::Responded(response) = outcome else {
            panic!("second candidate should return a response");
        };
        let bytes = openai_image_sync_heartbeat_response_body_bytes(response).await;
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        assert_eq!(body, json!({"data": [{"b64_json": "second-candidate"}]}));
    }

    #[tokio::test]
    async fn standard_text_sync_heartbeat_missing_config_defaults_disabled() {
        let state = AppState::new().expect("state should build");

        assert!(!standard_text_sync_heartbeat_enabled(&state).await);
    }

    #[tokio::test]
    async fn standard_text_sync_heartbeat_no_local_candidates_preserves_no_path() {
        let state = AppState::new()
            .expect("state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::disabled().with_system_config_values_for_tests([(
                    ENABLE_STANDARD_TEXT_SYNC_HEARTBEAT_CONFIG_KEY.to_string(),
                    json!(true),
                )]),
            );
        let (parts, _) = http::Request::builder()
            .method(http::Method::POST)
            .uri("/v1/responses")
            .body(())
            .expect("request should build")
            .into_parts();

        let outcome = maybe_execute_sync_via_local_openai_responses_decision(
            &state,
            &parts,
            "trace-standard-text-heartbeat-no-path",
            &test_standard_text_heartbeat_decision(),
            &json!({"model": "missing-local-candidate"}),
            TEST_STANDARD_TEXT_SYNC_PLAN_KIND,
        )
        .await
        .expect("heartbeat no-path check should execute");

        assert!(matches!(outcome, LocalExecutionRequestOutcome::NoPath));
    }

    #[tokio::test]
    async fn standard_text_sync_heartbeat_success_body_is_unchanged() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(r#"{"id":"resp_123","output":[]}"#))
            .expect("response should build");

        let bytes = standard_text_sync_heartbeat_response_body_bytes(
            "openai:responses:compact",
            None,
            response,
        )
        .await;
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(body, json!({"id": "resp_123", "output": []}));
    }

    #[tokio::test]
    async fn standard_text_sync_heartbeat_claude_error_body_includes_upstream_status() {
        let response = Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .body(Body::from(
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
            ))
            .expect("response should build");

        let bytes =
            standard_text_sync_heartbeat_response_body_bytes("claude:messages", None, response)
                .await;
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(body["type"], json!("error"));
        assert_eq!(body["error"]["type"], json!("rate_limit_error"));
        assert_eq!(body["error"]["message"], json!("slow down"));
        assert_eq!(body["error"]["upstream_status"], json!(429));
    }

    #[test]
    fn standard_text_sync_heartbeat_applies_to_chat_and_cli_plan_kinds() {
        assert!(standard_text_sync_heartbeat_applies_to_plan_kind(
            "claude_chat_sync"
        ));
        assert!(standard_text_sync_heartbeat_applies_to_plan_kind(
            "claude_cli_sync"
        ));
        assert!(standard_text_sync_heartbeat_applies_to_plan_kind(
            "gemini_chat_sync"
        ));
        assert!(standard_text_sync_heartbeat_applies_to_plan_kind(
            "gemini_cli_sync"
        ));
        assert!(!standard_text_sync_heartbeat_applies_to_plan_kind(
            "openai_embedding_sync"
        ));
    }

    #[tokio::test]
    async fn standard_text_sync_heartbeat_redirect_status_is_wrapped_as_error() {
        let response = Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .body(Body::from(r#"{"location":"https://upstream.example"}"#))
            .expect("response should build");

        let bytes = standard_text_sync_heartbeat_response_body_bytes(
            "openai:responses:compact",
            None,
            response,
        )
        .await;
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(body["error"]["type"], json!("server_error"));
        assert_eq!(body["error"]["upstream_status"], json!(307));
    }

    #[tokio::test]
    async fn standard_text_sync_heartbeat_shell_sends_whitespace_before_background_finishes() {
        let state = AppState::new().expect("state should build");
        let (parts, _) = http::Request::builder()
            .method(http::Method::POST)
            .uri("/v1/responses")
            .body(())
            .expect("request should build")
            .into_parts();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

        let response = build_standard_text_sync_heartbeat_shell_response(
            state,
            parts,
            "trace-standard-text-heartbeat-shell".to_string(),
            test_standard_text_heartbeat_decision(),
            TEST_STANDARD_TEXT_SYNC_PLAN_KIND.to_string(),
            move |_state, _parts, _trace_id, _decision, _plan_kind, _started_at| async move {
                let _ = release_rx.await;
                Ok(LocalExecutionRequestOutcome::responded(
                    Response::builder()
                        .status(StatusCode::OK)
                        .body(Body::from(r#"{"id":"resp_done","output":[]}"#))
                        .expect("response should build"),
                ))
            },
        )
        .expect("heartbeat shell should build");
        let mut body_stream = response.into_body().into_data_stream();

        let first = body_stream
            .next()
            .await
            .expect("heartbeat stream should yield")
            .expect("heartbeat chunk should be ok");
        assert_eq!(first.as_ref(), b"\n");
        let _ = release_tx.send(());
    }

    #[test]
    fn standard_text_sync_heartbeat_compact_non_json_error_body_is_wrapped_in_client_format() {
        let bytes = standard_text_sync_heartbeat_error_body_from_response(
            "openai:responses:compact",
            502,
            b"bad gateway from upstream",
        );
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(body["error"]["type"], json!("server_error"));
        assert_eq!(body["error"]["message"], json!("bad gateway from upstream"));
        assert_eq!(body["error"]["upstream_status"], json!(502));
    }

    #[tokio::test]
    async fn standard_text_sync_heartbeat_attempts_retry_first_candidate_then_return_second() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_for_override = Arc::clone(&call_count);
        let state = AppState::new()
            .expect("state should build")
            .with_execution_runtime_sync_override_for_tests(move |plan| {
                call_count_for_override.fetch_add(1, Ordering::SeqCst);
                if plan.endpoint_id == "endpoint-retry" {
                    Ok(test_openai_image_execution_result(
                        plan,
                        StatusCode::TOO_MANY_REQUESTS.as_u16(),
                        json!({"error": {"message": "retry this candidate"}}),
                    ))
                } else {
                    Ok(test_openai_image_execution_result(
                        plan,
                        StatusCode::OK.as_u16(),
                        json!({"id": "resp_second_candidate", "output": []}),
                    ))
                }
            });
        let attempts = vec![
            test_standard_text_heartbeat_attempt(
                0,
                "endpoint-retry",
                "candidate-retry",
                "openai:responses:compact",
            ),
            test_standard_text_heartbeat_attempt(
                1,
                "endpoint-success",
                "candidate-success",
                "openai:responses:compact",
            ),
        ];

        let (parts, _) = http::Request::builder()
            .method(http::Method::POST)
            .uri("/v1/responses")
            .body(())
            .expect("request should build")
            .into_parts();
        let outcome = execute_sync_attempt_source::<AiSyncAttempt, _>(
            &state,
            &parts,
            "trace-standard-text-heartbeat-retry",
            &test_standard_text_heartbeat_decision(),
            TEST_STANDARD_TEXT_SYNC_PLAN_KIND,
            TestSyncAttemptSource::new(attempts),
        )
        .await
        .expect("heartbeat attempts should execute");
        let LocalExecutionRequestOutcome::Responded(response) = outcome else {
            panic!("second candidate should return a response");
        };
        let bytes = standard_text_sync_heartbeat_response_body_bytes(
            "openai:responses:compact",
            None,
            response,
        )
        .await;
        let body: Value = serde_json::from_slice(&bytes).expect("body should decode");

        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        assert_eq!(body, json!({"id": "resp_second_candidate", "output": []}));
    }
}
