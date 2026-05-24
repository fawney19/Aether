use std::collections::{BTreeMap, VecDeque};
use std::io::Error as IoError;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use aether_contracts::{
    ExecutionPlan, ExecutionStreamTerminalSummary, ExecutionTelemetry, StandardizedUsage,
    StreamFrame, StreamFramePayload,
};
use aether_data_contracts::repository::candidates::RequestCandidateStatus;
use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use aether_scheduler_core::{
    parse_request_candidate_report_context, SchedulerRequestCandidateStatusUpdate,
};
use aether_usage_runtime::{
    build_lifecycle_usage_seed, build_stream_terminal_usage_payload_seed,
    build_sync_terminal_usage_payload_seed, build_terminal_usage_context_seed,
    UsageBodyCapturePolicy, UsageRequestRecordLevel, UsageRuntimeAccess,
    DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
};
use async_stream::stream;
use axum::body::{Body, Bytes};
use axum::http::Response;
use base64::Engine as _;
use futures_util::stream::BoxStream;
use futures_util::{StreamExt, TryStreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_util::codec::{FramedRead, LinesCodec};
use tokio_util::io::StreamReader;
use tracing::{debug, info, warn};

use super::error::{
    build_synthetic_non_success_stream_error_body, collect_error_body, decode_stream_error_body,
    inspect_prefetched_stream_body, read_next_frame,
    should_synthesize_non_success_stream_error_body,
    stream_client_error_status_code_for_upstream_status, synthetic_error_response_headers,
    StreamPrefetchInspection,
};
#[path = "execution_failures.rs"]
mod execution_failures;
use self::execution_failures::{
    build_stream_failure_from_execution_error, build_stream_failure_from_provider_error_body,
    build_stream_failure_report, handle_prefetch_provider_private_stream_error,
    handle_prefetch_stream_failure, submit_midstream_stream_failure, StreamFailureReport,
};
use crate::ai_serving::api::{
    extract_provider_private_stream_error_body, maybe_bridge_standard_sync_json_to_stream,
    maybe_build_provider_private_stream_normalizer, maybe_build_stream_response_rewriter,
    normalize_provider_private_report_context, StreamingStandardTerminalObserver,
};
use crate::ai_serving::is_openai_responses_family_format;
use crate::api::response::{
    attach_control_metadata_headers, build_client_response, build_client_response_from_parts,
};
use crate::clock::current_unix_ms as current_request_candidate_unix_ms;
use crate::constants::{CONTROL_CANDIDATE_ID_HEADER, CONTROL_REQUEST_ID_HEADER};
use crate::control::GatewayControlDecision;
use crate::execution_runtime::build_direct_execution_frame_stream;
use crate::execution_runtime::chatgpt_web_image::maybe_execute_chatgpt_web_image_stream;
use crate::execution_runtime::grok::maybe_execute_grok_stream;
use crate::execution_runtime::kiro_cache::{
    billed_input_tokens as kiro_billed_input_tokens, build_kiro_prompt_cache_profile,
    estimate_kiro_prompt_input_tokens, kiro_prompt_cache_tracker,
    kiro_simulated_cache_enabled_from_provider_config,
    kiro_simulated_cache_enabled_from_report_context, KiroPromptCacheUsage,
    KIRO_SIMULATED_CACHE_ENABLED_CONTEXT_FIELD,
};
use crate::execution_runtime::kiro_web_search::maybe_execute_kiro_web_search_stream;
use crate::execution_runtime::oauth_retry::refresh_oauth_plan_auth_for_retry;
#[cfg(test)]
use crate::execution_runtime::remote_compat::post_stream_plan_to_remote_execution_runtime;
use crate::execution_runtime::submission::{
    resolve_core_error_background_report_kind, resolve_local_sync_error_status_code,
    strip_utf8_bom_and_ws, submit_local_core_error_or_sync_finalize,
};
use crate::execution_runtime::transport::{
    execute_stream_plan_via_local_tunnel, record_manual_proxy_request_failure,
    record_manual_proxy_request_success, record_manual_proxy_stream_error,
    DirectSyncExecutionRuntime, DirectUpstreamStreamExecution, ExecutionRuntimeTransportError,
};
use crate::execution_runtime::windsurf::maybe_execute_windsurf_stream;
use crate::execution_runtime::{
    apply_endpoint_response_header_rules, attach_provider_response_headers_to_report_context,
    local_failover_response_text, resolve_core_stream_direct_finalize_report_kind,
    resolve_core_stream_error_finalize_report_kind,
    resolve_local_candidate_failover_analysis_stream, should_fallback_to_control_stream,
    should_retry_next_local_candidate_stream, LocalFailoverDecision,
};
use crate::execution_runtime::{MAX_STREAM_PREFETCH_BYTES, MAX_STREAM_PREFETCH_FRAMES};
use crate::log_ids::short_request_id;
use crate::orchestration::{
    apply_local_execution_effect, build_local_error_flow_metadata, trace_upstream_response_body,
    with_error_flow_report_context, with_upstream_response_report_context,
    LocalAdaptiveRateLimitEffect, LocalAdaptiveSuccessEffect, LocalAttemptFailureEffect,
    LocalExecutionEffect, LocalExecutionEffectContext, LocalHealthFailureEffect,
    LocalHealthSuccessEffect, LocalOAuthInvalidationEffect, LocalPoolErrorEffect,
};
use crate::provider_pool_demand::{
    acquire_provider_pool_in_flight_guard, ProviderPoolInFlightGuard,
};
use crate::request_candidate_runtime::{
    ensure_execution_request_candidate_slot, record_local_request_candidate_status,
    record_local_request_candidate_status_snapshot, snapshot_local_request_candidate_status,
};
use crate::usage::submit_stream_report;
use crate::usage::{GatewayStreamReportRequest, GatewaySyncReportRequest};
use crate::{
    AppState, GatewayError, GEMINI_FILES_DOWNLOAD_PLAN_KIND, OPENAI_VIDEO_CONTENT_PLAN_KIND,
};

const OPENAI_IMAGE_STREAM_PLAN_KIND: &str = "openai_image_stream";
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const SSE_KEEPALIVE_BYTES: &[u8] = b": aether-keepalive\n\n";
const SSE_CONTROL_FILTER_MAX_BUFFER_BYTES: usize = 1024 * 1024;
const STREAM_IDLE_LOG_INTERVAL: Duration = Duration::from_secs(60);
const STREAM_IDLE_LOG_INTERVAL_MS: u64 = 60_000;
const REWRITTEN_STREAM_PREFETCH_TIMEOUT: Duration = Duration::from_millis(750);
const OPENAI_IMAGE_STREAM_DEFAULT_TOTAL_TIMEOUT_MS: u64 = 900_000;

fn record_sync_terminal_usage(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewaySyncReportRequest,
) {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_sync_terminal_usage_payload_seed(payload);
    state
        .usage_runtime
        .record_sync_terminal(state.data.as_ref(), context_seed, payload_seed);
}

fn build_stream_sync_payload(
    trace_id: &str,
    report_kind: String,
    report_context: Option<Value>,
    status_code: u16,
    headers: BTreeMap<String, String>,
    body_json: Option<Value>,
    body_base64: Option<String>,
    telemetry: Option<ExecutionTelemetry>,
) -> GatewaySyncReportRequest {
    GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind,
        report_context,
        status_code,
        headers,
        body_json,
        client_body_json: None,
        body_base64,
        telemetry,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream_error_sync_payload(
    trace_id: &str,
    report_kind: String,
    report_context: Option<Value>,
    upstream_status_code: u16,
    provider_headers: BTreeMap<String, String>,
    provider_body_json: Option<Value>,
    provider_body_base64: Option<String>,
    client_headers: BTreeMap<String, String>,
    client_body_json: Option<Value>,
    telemetry: Option<ExecutionTelemetry>,
) -> GatewaySyncReportRequest {
    let client_status_code =
        stream_client_error_status_code_for_upstream_status(upstream_status_code);
    let mut report_context = report_context;
    if client_status_code != upstream_status_code || client_headers != provider_headers {
        let mut object = match report_context {
            Some(Value::Object(object)) => object,
            Some(other) => serde_json::Map::from_iter([("seed".to_string(), other)]),
            None => serde_json::Map::new(),
        };
        object.insert(
            "client_response_status_code".to_string(),
            Value::from(client_status_code),
        );
        object.insert(
            "client_response_headers".to_string(),
            serde_json::to_value(client_headers).unwrap_or(Value::Null),
        );
        report_context = Some(Value::Object(object));
    }

    GatewaySyncReportRequest {
        trace_id: trace_id.to_string(),
        report_kind,
        report_context,
        status_code: upstream_status_code,
        headers: provider_headers,
        body_json: provider_body_json,
        client_body_json,
        body_base64: provider_body_base64,
        telemetry,
    }
}

fn record_stream_terminal_usage(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: Option<&serde_json::Value>,
    payload: &GatewayStreamReportRequest,
    cancelled: bool,
) {
    let context_seed = build_terminal_usage_context_seed(plan, report_context);
    let payload_seed = build_stream_terminal_usage_payload_seed(payload);
    state.usage_runtime.record_stream_terminal(
        state.data.as_ref(),
        context_seed,
        payload_seed,
        cancelled,
    );
}

fn build_stream_body_capture(
    body: &[u8],
    truncated: bool,
) -> (Option<String>, Option<UsageBodyCaptureState>) {
    let body_base64 =
        (!body.is_empty()).then(|| base64::engine::general_purpose::STANDARD.encode(body));
    let body_state = Some(if truncated {
        UsageBodyCaptureState::Truncated
    } else if body.is_empty() {
        UsageBodyCaptureState::None
    } else {
        UsageBodyCaptureState::Inline
    });
    (body_base64, body_state)
}

fn wrap_non_json_binary_stream_error_for_client(
    plan_kind: &str,
    headers: &BTreeMap<String, String>,
    error_body: &[u8],
) -> Result<Option<Value>, GatewayError> {
    let content_type = headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if content_type.starts_with("application/json") {
        return Ok(None);
    }

    let body = match plan_kind {
        GEMINI_FILES_DOWNLOAD_PLAN_KIND => json!({
            "error": String::from_utf8_lossy(error_body).to_string(),
        }),
        OPENAI_VIDEO_CONTENT_PLAN_KIND => json!({
            "error": {
                "type": "upstream_error",
                "message": "Video not available",
            }
        }),
        _ => return Ok(None),
    };
    Ok(Some(body))
}

fn with_stream_error_trace_context(
    report_context: Option<&Value>,
    status_code: u16,
    headers: &BTreeMap<String, String>,
    body_json: Option<&Value>,
    body_bytes: &[u8],
    response_text: Option<&str>,
    local_failover_analysis: crate::orchestration::LocalFailoverAnalysis,
) -> Option<Value> {
    let body = trace_upstream_response_body(body_json, body_bytes);
    let upstream_context = with_upstream_response_report_context(
        report_context,
        status_code,
        Some(headers),
        body.as_ref(),
        None,
        None,
    );
    with_error_flow_report_context(
        upstream_context.as_ref().or(report_context),
        build_local_error_flow_metadata(status_code, response_text, local_failover_analysis),
    )
}

#[allow(clippy::too_many_arguments)] // stream report payload assembly mirrors runtime state
fn build_stream_usage_payload(
    trace_id: String,
    report_kind: String,
    report_context: Option<Value>,
    status_code: u16,
    headers: BTreeMap<String, String>,
    provider_body: &[u8],
    provider_body_truncated: bool,
    client_body: &[u8],
    client_body_truncated: bool,
    terminal_summary: Option<ExecutionStreamTerminalSummary>,
    telemetry: Option<ExecutionTelemetry>,
) -> GatewayStreamReportRequest {
    let (provider_body_base64, provider_body_state) =
        build_stream_body_capture(provider_body, provider_body_truncated);
    let (client_body_base64, client_body_state) =
        build_stream_body_capture(client_body, client_body_truncated);
    GatewayStreamReportRequest {
        trace_id,
        report_kind,
        report_context,
        status_code,
        headers,
        provider_body_base64,
        provider_body_state,
        client_body_base64,
        client_body_state,
        terminal_summary,
        telemetry,
    }
}

fn seed_kiro_report_context_input_tokens(plan: &ExecutionPlan, report_context: &mut Option<Value>) {
    if !plan
        .provider_name
        .as_deref()
        .is_some_and(|provider_name| provider_name.eq_ignore_ascii_case("Kiro"))
    {
        return;
    }

    let Some(context) = report_context.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if context
        .get("input_tokens")
        .and_then(Value::as_u64)
        .is_some_and(|input_tokens| input_tokens > 0)
    {
        return;
    }

    let Some(original_request_body) = context.get("original_request_body").cloned() else {
        return;
    };
    let estimated_input_tokens = estimate_kiro_prompt_input_tokens(&original_request_body);
    context.insert(
        "input_tokens".to_string(),
        Value::from(estimated_input_tokens),
    );
}

async fn seed_kiro_simulated_cache_enabled(
    state: &AppState,
    plan: &ExecutionPlan,
    report_context: &mut Option<Value>,
) {
    if !plan
        .provider_name
        .as_deref()
        .is_some_and(|provider_name| provider_name.eq_ignore_ascii_case("Kiro"))
    {
        return;
    }

    let enabled = match state
        .read_provider_catalog_providers_by_ids(std::slice::from_ref(&plan.provider_id))
        .await
    {
        Ok(providers) => providers
            .iter()
            .find(|provider| provider.id == plan.provider_id)
            .filter(|provider| provider.provider_type.eq_ignore_ascii_case("kiro"))
            .is_some_and(|provider| {
                kiro_simulated_cache_enabled_from_provider_config(provider.config.as_ref())
            }),
        Err(err) => {
            warn!(
                event_name = "kiro_simulated_cache_config_read_failed",
                log_type = "event",
                request_id = %plan.request_id,
                provider_id = %plan.provider_id,
                error = ?err,
                "failed to read Kiro simulated cache provider config; defaulting disabled"
            );
            false
        }
    };

    let Some(context) = report_context.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if enabled {
        context.insert(
            KIRO_SIMULATED_CACHE_ENABLED_CONTEXT_FIELD.to_string(),
            Value::Bool(true),
        );
    } else {
        context.remove(KIRO_SIMULATED_CACHE_ENABLED_CONTEXT_FIELD);
    }
}

fn seed_kiro_report_context_prompt_cache_usage(
    plan: &ExecutionPlan,
    report_context: &mut Option<Value>,
) {
    if !plan
        .provider_name
        .as_deref()
        .is_some_and(|provider_name| provider_name.eq_ignore_ascii_case("Kiro"))
    {
        return;
    }

    let simulated_cache_enabled =
        kiro_simulated_cache_enabled_from_report_context(report_context.as_ref());
    let Some(context) = report_context.as_mut().and_then(Value::as_object_mut) else {
        return;
    };
    if context
        .get("kiro_web_search_mcp")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return;
    }
    if !simulated_cache_enabled {
        return;
    }
    if kiro_cache_usage_from_context_object(context).is_some() {
        return;
    }

    let Some(original_request_body) = context.get("original_request_body").cloned() else {
        return;
    };
    let input_tokens = context
        .get("input_tokens")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            let estimated = estimate_kiro_prompt_input_tokens(&original_request_body);
            context.insert("input_tokens".to_string(), Value::from(estimated));
            estimated
        });
    let Some(profile) = build_kiro_prompt_cache_profile(&original_request_body, input_tokens)
    else {
        return;
    };

    let cache_usage = kiro_prompt_cache_tracker()
        .compute_and_update(kiro_stream_cache_credential_id(plan), &profile);
    if cache_usage.cache_creation_input_tokens == 0 && cache_usage.cache_read_input_tokens == 0 {
        return;
    }
    context.insert(
        "cache_creation_input_tokens".to_string(),
        Value::from(cache_usage.cache_creation_input_tokens),
    );
    context.insert(
        "cache_read_input_tokens".to_string(),
        Value::from(cache_usage.cache_read_input_tokens),
    );
}

fn kiro_stream_cache_credential_id(plan: &ExecutionPlan) -> String {
    format!("{}:{}:{}", plan.provider_id, plan.endpoint_id, plan.key_id)
}

fn kiro_cache_usage_from_context_object(
    context: &serde_json::Map<String, Value>,
) -> Option<KiroPromptCacheUsage> {
    let cache_creation_input_tokens = context
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read_input_tokens = context
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    (cache_creation_input_tokens > 0 || cache_read_input_tokens > 0).then_some(
        KiroPromptCacheUsage {
            cache_creation_input_tokens,
            cache_read_input_tokens,
        },
    )
}

fn kiro_cache_usage_from_report_context(report_context: &Value) -> Option<KiroPromptCacheUsage> {
    report_context
        .as_object()
        .and_then(kiro_cache_usage_from_context_object)
}

fn maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
    plan: &ExecutionPlan,
    report_context: Option<&Value>,
    summary: &mut Option<ExecutionStreamTerminalSummary>,
) {
    if !plan
        .provider_name
        .as_deref()
        .is_some_and(|provider_name| provider_name.eq_ignore_ascii_case("Kiro"))
    {
        return;
    }

    let Some(report_context) = report_context else {
        return;
    };
    let Some(original_request_body) = report_context.get("original_request_body") else {
        return;
    };
    let simulated_cache_enabled =
        kiro_simulated_cache_enabled_from_report_context(Some(report_context));

    let summary = summary.get_or_insert_with(ExecutionStreamTerminalSummary::default);
    let usage = summary
        .standardized_usage
        .get_or_insert_with(StandardizedUsage::new);
    let estimated_input_tokens = report_context
        .get("input_tokens")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            let estimated_input_tokens = estimate_kiro_prompt_input_tokens(original_request_body);
            if estimated_input_tokens > 0 {
                estimated_input_tokens
            } else {
                usage.input_tokens.max(0) as u64
            }
        });

    if !simulated_cache_enabled {
        usage.cache_creation_tokens = 0;
        usage.cache_read_tokens = 0;
        if usage.input_tokens <= 0 {
            usage.input_tokens = estimated_input_tokens as i64;
        }
        return;
    }

    if let Some(cache_usage) = kiro_cache_usage_from_report_context(report_context) {
        usage.input_tokens = kiro_billed_input_tokens(estimated_input_tokens, cache_usage) as i64;
        usage.cache_creation_tokens = cache_usage.cache_creation_input_tokens as i64;
        usage.cache_read_tokens = cache_usage.cache_read_input_tokens as i64;
        return;
    }

    if usage.cache_creation_tokens > 0 || usage.cache_read_tokens > 0 {
        if usage.input_tokens <= 0 {
            usage.input_tokens = kiro_billed_input_tokens(
                estimated_input_tokens,
                KiroPromptCacheUsage {
                    cache_creation_input_tokens: usage.cache_creation_tokens.max(0) as u64,
                    cache_read_input_tokens: usage.cache_read_tokens.max(0) as u64,
                },
            ) as i64;
        }
        return;
    }

    if usage.input_tokens <= 0 {
        usage.input_tokens = estimated_input_tokens as i64;
    }

    let Some(profile) =
        build_kiro_prompt_cache_profile(original_request_body, estimated_input_tokens)
    else {
        return;
    };

    let cache_usage = kiro_prompt_cache_tracker()
        .compute_and_update(kiro_stream_cache_credential_id(plan), &profile);
    if cache_usage.cache_creation_input_tokens == 0 && cache_usage.cache_read_input_tokens == 0 {
        return;
    }

    let billed_input_tokens = kiro_billed_input_tokens(estimated_input_tokens, cache_usage);
    usage.input_tokens = billed_input_tokens as i64;
    usage.cache_creation_tokens = cache_usage.cache_creation_input_tokens as i64;
    usage.cache_read_tokens = cache_usage.cache_read_input_tokens as i64;
}

fn append_stream_capture_bytes(
    buffer: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
    truncated: &mut bool,
) {
    if chunk.is_empty() || max_bytes == 0 {
        return;
    }
    if buffer.len() >= max_bytes {
        *truncated = true;
        return;
    }
    let remaining = max_bytes - buffer.len();
    let keep_len = remaining.min(chunk.len());
    buffer.extend_from_slice(&chunk[..keep_len]);
    if keep_len < chunk.len() {
        *truncated = true;
    }
}

fn observe_stream_usage_bytes(
    observer: &mut StreamingStandardTerminalObserver,
    report_context: &Value,
    buffered: &mut Vec<u8>,
    chunk: &[u8],
) {
    if chunk.is_empty() {
        return;
    }

    buffered.extend_from_slice(chunk);
    while let Some(line_end) = buffered.iter().position(|byte| *byte == b'\n') {
        let line = buffered.drain(..=line_end).collect::<Vec<_>>();
        if let Err(err) = observer.push_line(report_context, line) {
            observer.disable_with_error(err.to_string());
            buffered.clear();
            break;
        }
    }
}

fn finalize_stream_usage_observer(
    observer: &mut Option<StreamingStandardTerminalObserver>,
    report_context: Option<&Value>,
    buffered: &mut Vec<u8>,
) -> Option<ExecutionStreamTerminalSummary> {
    let (Some(observer), Some(report_context)) = (observer.as_mut(), report_context) else {
        return None;
    };

    if !buffered.is_empty() {
        let line = std::mem::take(buffered);
        if let Err(err) = observer.push_line(report_context, line) {
            observer.disable_with_error(err.to_string());
        }
    }

    match observer.finish(report_context) {
        Ok(summary) => summary,
        Err(err) => {
            observer.disable_with_error(err.to_string());
            observer.latest_summary().cloned()
        }
    }
}

fn merge_stream_terminal_summary(
    mut current: Option<ExecutionStreamTerminalSummary>,
    observed: Option<ExecutionStreamTerminalSummary>,
) -> Option<ExecutionStreamTerminalSummary> {
    let Some(observed) = observed else {
        return current;
    };

    let Some(current_summary) = current.as_mut() else {
        return Some(observed);
    };

    if should_replace_stream_usage(
        current_summary.standardized_usage.as_ref(),
        observed.standardized_usage.as_ref(),
    ) {
        current_summary.standardized_usage = observed.standardized_usage;
    }
    if current_summary.finish_reason.is_none() {
        current_summary.finish_reason = observed.finish_reason;
    }
    if current_summary.response_id.is_none() {
        current_summary.response_id = observed.response_id;
    }
    if current_summary.model.is_none() {
        current_summary.model = observed.model;
    }
    current_summary.observed_finish |= observed.observed_finish;
    current_summary.unknown_event_count = current_summary
        .unknown_event_count
        .saturating_add(observed.unknown_event_count);
    if current_summary.parser_error.is_none() {
        current_summary.parser_error = observed.parser_error;
    }

    current
}

fn should_replace_stream_usage(
    current: Option<&aether_contracts::StandardizedUsage>,
    observed: Option<&aether_contracts::StandardizedUsage>,
) -> bool {
    let Some(observed) = observed else {
        return false;
    };
    let Some(current) = current else {
        return true;
    };

    observed.is_more_complete_than(current)
}

fn stream_terminal_summary_missing_observed_finish(
    summary: Option<&ExecutionStreamTerminalSummary>,
) -> bool {
    summary.is_some_and(|summary| {
        !summary.observed_finish
            && !summary
                .standardized_usage
                .as_ref()
                .is_some_and(StandardizedUsage::has_token_signal)
    })
}

fn stream_report_context_format_field<'a>(
    report_context: Option<&'a Value>,
    field: &str,
) -> Option<&'a str> {
    report_context
        .and_then(Value::as_object)
        .and_then(|object| object.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn stream_requires_observed_terminal_event(
    provider_api_format: &str,
    report_context: Option<&Value>,
) -> bool {
    is_openai_responses_family_format(provider_api_format)
        || [
            "provider_stream_event_api_format",
            "provider_stream_api_format",
            "provider_api_format",
        ]
        .into_iter()
        .filter_map(|field| stream_report_context_format_field(report_context, field))
        .any(is_openai_responses_family_format)
}

fn stream_terminal_summary_missing_observed_finish_with_requirement(
    summary: Option<&ExecutionStreamTerminalSummary>,
    requires_observed_terminal_event: bool,
) -> bool {
    if !requires_observed_terminal_event {
        return stream_terminal_summary_missing_observed_finish(summary);
    }

    summary.is_some_and(|summary| !summary.observed_finish)
}

fn ensure_stream_terminal_summary_for_missing_observed_finish(
    summary: &mut Option<ExecutionStreamTerminalSummary>,
    requires_observed_terminal_event: bool,
) {
    if !requires_observed_terminal_event {
        return;
    }

    let summary = summary.get_or_insert_with(ExecutionStreamTerminalSummary::default);
    if !summary.observed_finish && summary.parser_error.is_none() {
        summary.parser_error =
            Some("execution runtime stream ended before provider terminal event".to_string());
    }
}

fn stream_terminal_summary_represents_failure_with_requirement(
    summary: Option<&ExecutionStreamTerminalSummary>,
    requires_observed_terminal_event: bool,
) -> bool {
    summary.is_some_and(|summary| {
        summary.parser_error.is_some()
            || stream_terminal_summary_missing_observed_finish_with_requirement(
                Some(summary),
                requires_observed_terminal_event,
            )
    })
}

async fn execute_in_process_stream(
    state: &AppState,
    plan: &ExecutionPlan,
) -> Result<DirectUpstreamStreamExecution, ExecutionRuntimeTransportError> {
    if let Some(execution) = execute_stream_plan_via_local_tunnel(state, plan).await? {
        return Ok(execution);
    }

    match DirectSyncExecutionRuntime::new().execute_stream(plan).await {
        Ok(execution) => {
            record_manual_proxy_request_success(state, plan).await;
            Ok(execution)
        }
        Err(error) => {
            record_manual_proxy_request_failure(state, plan).await;
            Err(error)
        }
    }
}

async fn execute_in_process_stream_with_oauth_retry(
    state: &AppState,
    plan: &mut ExecutionPlan,
    trace_id: &str,
    report_context: Option<&Value>,
) -> Result<DirectUpstreamStreamExecution, ExecutionRuntimeTransportError> {
    let mut execution = execute_in_process_stream(state, plan).await?;
    apply_stream_summary_report_context(&mut execution, report_context);
    if execution.status_code >= 400
        && refresh_oauth_plan_auth_for_retry(state, plan, execution.status_code, None, trace_id)
            .await
    {
        execution = execute_in_process_stream(state, plan).await?;
        apply_stream_summary_report_context(&mut execution, report_context);
    }
    Ok(execution)
}

#[allow(clippy::too_many_arguments)] // internal function, grouping would add unnecessary indirection
pub(crate) async fn execute_execution_runtime_stream(
    state: &AppState,
    mut plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    mut report_context: Option<serde_json::Value>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let stream_started_at = Instant::now();
    ensure_execution_request_candidate_slot(state, &mut plan, &mut report_context).await;
    seed_kiro_report_context_input_tokens(&plan, &mut report_context);
    let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
    let request_candidate_status_snapshot =
        snapshot_local_request_candidate_status(&plan, report_context.as_ref());
    state
        .usage_runtime
        .record_pending(state.data.as_ref(), lifecycle_seed.clone());
    let candidate_started_unix_secs = current_request_candidate_unix_ms();
    if let Some(snapshot) = request_candidate_status_snapshot.clone() {
        let state_bg = state.clone();
        tokio::spawn(async move {
            record_local_request_candidate_status_snapshot(
                &state_bg,
                &snapshot,
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Pending,
                    status_code: None,
                    error_type: None,
                    error_message: None,
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: None,
                },
            )
            .await;
        });
    }
    let plan_request_id_for_log = short_request_id(plan.request_id.as_str());
    let provider_name = plan
        .provider_name
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let endpoint_id = plan.endpoint_id.clone();
    let key_id = plan.key_id.clone();
    let model_name = plan.model_name.clone().unwrap_or_else(|| "-".to_string());
    let candidate_index = parse_request_candidate_report_context(report_context.as_ref())
        .and_then(|context| context.candidate_index)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let mut provider_pool_in_flight_guard = acquire_provider_pool_in_flight_guard(
        state.runtime_state.clone(),
        &plan.provider_id,
        plan.request_id.as_str(),
        plan.candidate_id.as_deref(),
        key_id.as_str(),
    )
    .await;
    match maybe_execute_grok_stream(&plan, report_context.as_ref()).await {
        Ok(Some(grok_stream)) => {
            return execute_stream_from_frame_stream(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                grok_stream.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                grok_stream.frame_stream,
                provider_pool_in_flight_guard.take(),
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
            info!(
                event_name = "grok_execution_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %plan_request_id_for_log,
                candidate_id = ?plan.candidate_id,
                provider_name = provider_name.as_str(),
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                model_name = model_name.as_str(),
                candidate_index = candidate_index.as_str(),
                error = %err,
                "gateway Grok stream execution unavailable"
            );
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("grok_execution_unavailable".to_string()),
                    error_message: Some(format!("{err:?}")),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            return Ok(None);
        }
    }
    match maybe_execute_windsurf_stream(state, &plan, report_context.as_ref()).await {
        Ok(Some(windsurf_stream)) => {
            return execute_stream_from_frame_stream(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                windsurf_stream.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                windsurf_stream.frame_stream,
                provider_pool_in_flight_guard.take(),
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
            info!(
                event_name = "windsurf_native_execution_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %plan_request_id_for_log,
                candidate_id = ?plan.candidate_id,
                provider_name = provider_name.as_str(),
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                model_name = model_name.as_str(),
                candidate_index = candidate_index.as_str(),
                error = %err,
                "gateway native Windsurf stream execution unavailable"
            );
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("windsurf_native_execution_unavailable".to_string()),
                    error_message: Some(err.to_string()),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            return Ok(None);
        }
    }
    match maybe_execute_kiro_web_search_stream(state, &plan, report_context.as_ref()).await {
        Ok(Some(kiro_web_search)) => {
            return execute_stream_from_frame_stream(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                kiro_web_search.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                kiro_web_search.frame_stream,
                provider_pool_in_flight_guard.take(),
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
            info!(
                event_name = "kiro_web_search_mcp_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %plan_request_id_for_log,
                candidate_id = ?plan.candidate_id,
                provider_name = provider_name.as_str(),
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                model_name = model_name.as_str(),
                candidate_index = candidate_index.as_str(),
                error = %err,
                "gateway Kiro web_search MCP execution unavailable"
            );
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("kiro_web_search_mcp_unavailable".to_string()),
                    error_message: Some(format!("{err:?}")),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            return Ok(None);
        }
    }
    match maybe_execute_chatgpt_web_image_stream(state, &plan, report_context.as_ref()).await {
        Ok(Some(chatgpt_web_image)) => {
            return execute_stream_from_frame_stream(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                chatgpt_web_image.report_context.or(report_context),
                candidate_started_unix_secs,
                stream_started_at,
                chatgpt_web_image.frame_stream,
                provider_pool_in_flight_guard.take(),
            )
            .await;
        }
        Ok(None) => {}
        Err(err) => {
            info!(
                event_name = "chatgpt_web_image_execution_unavailable",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %plan_request_id_for_log,
                candidate_id = ?plan.candidate_id,
                provider_name = provider_name.as_str(),
                endpoint_id = %endpoint_id,
                key_id = %key_id,
                model_name = model_name.as_str(),
                candidate_index = candidate_index.as_str(),
                error = %err,
                "gateway ChatGPT-Web image stream execution unavailable"
            );
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: None,
                    error_type: Some("chatgpt_web_image_execution_unavailable".to_string()),
                    error_message: Some(format!("{err:?}")),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            return Ok(None);
        }
    }
    #[cfg(not(test))]
    {
        let execution = match execute_in_process_stream_with_oauth_retry(
            state,
            &mut plan,
            trace_id,
            report_context.as_ref(),
        )
        .await
        {
            Ok(execution) => execution,
            Err(err) => {
                info!(
                    event_name = "stream_execution_runtime_unavailable",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %plan_request_id_for_log,
                    candidate_id = ?plan.candidate_id,
                    provider_name,
                    endpoint_id,
                    key_id,
                    model_name,
                    candidate_index = candidate_index.as_str(),
                    error = %err,
                    "gateway in-process stream execution unavailable"
                );
                let terminal_unix_secs = current_request_candidate_unix_ms();
                record_local_request_candidate_status(
                    state,
                    &plan,
                    report_context.as_ref(),
                    SchedulerRequestCandidateStatusUpdate {
                        status: RequestCandidateStatus::Failed,
                        status_code: None,
                        error_type: Some("execution_runtime_unavailable".to_string()),
                        error_message: Some(format!("{err:?}")),
                        latency_ms: None,
                        started_at_unix_ms: Some(candidate_started_unix_secs),
                        finished_at_unix_ms: Some(terminal_unix_secs),
                    },
                )
                .await;
                return Ok(None);
            }
        };
        let frame_stream = build_direct_execution_frame_stream(execution).boxed();
        return execute_stream_from_frame_stream(
            state,
            plan,
            trace_id,
            decision,
            plan_kind,
            report_kind,
            report_context,
            candidate_started_unix_secs,
            stream_started_at,
            frame_stream,
            provider_pool_in_flight_guard.take(),
        )
        .await;
    }
    #[cfg(test)]
    {
        let remote_execution_runtime_base_url = state
            .execution_runtime_override_base_url()
            .unwrap_or_default();
        if remote_execution_runtime_base_url.trim().is_empty() {
            let execution = match execute_in_process_stream_with_oauth_retry(
                state,
                &mut plan,
                trace_id,
                report_context.as_ref(),
            )
            .await
            {
                Ok(execution) => execution,
                Err(err) => {
                    info!(
                        event_name = "stream_execution_runtime_unavailable",
                        log_type = "ops",
                        trace_id = %trace_id,
                        request_id = %plan_request_id_for_log,
                        candidate_id = ?plan.candidate_id,
                        provider_name,
                        endpoint_id,
                        key_id,
                        model_name,
                        candidate_index = candidate_index.as_str(),
                        error = %err,
                        "gateway in-process stream execution unavailable"
                    );
                    let terminal_unix_secs = current_request_candidate_unix_ms();
                    record_local_request_candidate_status(
                        state,
                        &plan,
                        report_context.as_ref(),
                        SchedulerRequestCandidateStatusUpdate {
                            status: RequestCandidateStatus::Failed,
                            status_code: None,
                            error_type: Some("execution_runtime_unavailable".to_string()),
                            error_message: Some(err.to_string()),
                            latency_ms: None,
                            started_at_unix_ms: Some(candidate_started_unix_secs),
                            finished_at_unix_ms: Some(terminal_unix_secs),
                        },
                    )
                    .await;
                    return Ok(None);
                }
            };
            let frame_stream = build_direct_execution_frame_stream(execution).boxed();
            return execute_stream_from_frame_stream(
                state,
                plan,
                trace_id,
                decision,
                plan_kind,
                report_kind,
                report_context,
                candidate_started_unix_secs,
                stream_started_at,
                frame_stream,
                provider_pool_in_flight_guard.take(),
            )
            .await;
        }

        let response = match post_stream_plan_to_remote_execution_runtime(
            state,
            remote_execution_runtime_base_url,
            Some(trace_id),
            &plan,
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                warn!(
                    event_name = "stream_execution_runtime_remote_unavailable",
                    log_type = "ops",
                    trace_id = %trace_id,
                    request_id = %plan_request_id_for_log,
                    candidate_id = ?plan.candidate_id,
                    error = ?err,
                    "gateway remote execution runtime stream unavailable"
                );
                let terminal_unix_secs = current_request_candidate_unix_ms();
                record_local_request_candidate_status(
                    state,
                    &plan,
                    report_context.as_ref(),
                    SchedulerRequestCandidateStatusUpdate {
                        status: RequestCandidateStatus::Failed,
                        status_code: None,
                        error_type: Some("execution_runtime_unavailable".to_string()),
                        error_message: Some(format!("{err:?}")),
                        latency_ms: None,
                        started_at_unix_ms: Some(candidate_started_unix_secs),
                        finished_at_unix_ms: Some(terminal_unix_secs),
                    },
                )
                .await;
                return Ok(None);
            }
        };

        if response.status() != http::StatusCode::OK {
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: Some(response.status().as_u16()),
                    error_type: Some("execution_runtime_http_error".to_string()),
                    error_message: Some(format!(
                        "execution runtime returned HTTP {}",
                        response.status()
                    )),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            return Ok(Some(attach_control_metadata_headers(
                build_client_response(response, trace_id, Some(decision))?,
                Some(plan.request_id.as_str()),
                plan.candidate_id.as_deref(),
            )?));
        }

        let frame_stream = response
            .bytes_stream()
            .map_err(|err| IoError::other(err.to_string()))
            .boxed();
        return execute_stream_from_frame_stream(
            state,
            plan,
            trace_id,
            decision,
            plan_kind,
            report_kind,
            report_context,
            candidate_started_unix_secs,
            stream_started_at,
            frame_stream,
            provider_pool_in_flight_guard.take(),
        )
        .await;
    }
}

fn decode_stream_data_chunk(
    chunk_b64: Option<&str>,
    text: Option<&str>,
) -> Result<Vec<u8>, GatewayError> {
    if let Some(chunk_b64) = chunk_b64 {
        return base64::engine::general_purpose::STANDARD
            .decode(chunk_b64)
            .map_err(|err| GatewayError::Internal(err.to_string()));
    }
    Ok(text.unwrap_or_default().as_bytes().to_vec())
}

fn response_headers_indicate_sse(headers: &BTreeMap<String, String>) -> bool {
    headers
        .get("content-type")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
}

fn parse_prefetched_sync_json_body(body: &[u8]) -> Option<Value> {
    let stripped = strip_utf8_bom_and_ws(body);
    serde_json::from_slice::<Value>(stripped).ok()
}

fn encode_terminal_sse_error_event(failure: &StreamFailureReport) -> Result<Bytes, std::io::Error> {
    let payload = failure
        .to_json_string()
        .map_err(|err| IoError::other(err.to_string()))?;
    let mut event = String::new();
    for line in payload.lines() {
        event.push_str("data: ");
        event.push_str(line);
        event.push('\n');
    }
    event.push_str("\ndata: [DONE]\n\n");
    Ok(Bytes::from(event))
}

fn image_stream_failed_event_name(report_context: Option<&Value>) -> &'static str {
    let operation = report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("operation"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if operation == "edit" {
        "image_edit.failed"
    } else {
        "image_generation.failed"
    }
}

fn encode_openai_image_failed_event(
    report_context: Option<&Value>,
    failure: &StreamFailureReport,
) -> Result<Bytes, std::io::Error> {
    let event_name = image_stream_failed_event_name(report_context);
    let failure_body = failure
        .to_json_string()
        .map_err(|err| IoError::other(err.to_string()))?;
    let failure_json: Value =
        serde_json::from_str(&failure_body).map_err(|err| IoError::other(err.to_string()))?;
    let error = failure_json.get("error").cloned().unwrap_or_else(|| {
        serde_json::json!({
            "type": failure.error_type.as_str(),
            "message": failure.error_message.as_str(),
            "code": failure.status_code,
        })
    });
    let payload = serde_json::json!({
        "type": event_name,
        "error": error,
    });
    let payload = serde_json::to_string(&payload).map_err(|err| IoError::other(err.to_string()))?;
    let mut event = format!("event: {event_name}\n");
    for line in payload.lines() {
        event.push_str("data: ");
        event.push_str(line);
        event.push('\n');
    }
    event.push('\n');
    Ok(Bytes::from(event))
}

fn resolve_openai_image_stream_total_timeout_ms(
    plan_kind: &str,
    plan: &ExecutionPlan,
) -> Option<u64> {
    if plan_kind != OPENAI_IMAGE_STREAM_PLAN_KIND {
        return None;
    }
    Some(
        plan.timeouts
            .as_ref()
            .and_then(|timeouts| timeouts.total_ms)
            .unwrap_or(OPENAI_IMAGE_STREAM_DEFAULT_TOTAL_TIMEOUT_MS)
            .max(1),
    )
}

fn should_limit_direct_finalize_prefetch(plan_kind: &str, has_local_stream_rewriter: bool) -> bool {
    plan_kind == OPENAI_IMAGE_STREAM_PLAN_KIND || has_local_stream_rewriter
}

fn build_sse_body_stream(
    prefetched_chunks_for_body: Vec<Bytes>,
    mut rx: mpsc::Receiver<Result<Bytes, IoError>>,
    emit_keepalive: bool,
    keepalive_interval: Duration,
) -> impl futures_util::Stream<Item = Result<Bytes, IoError>> + Send + 'static {
    stream! {
        let mut upstream_control_filter = emit_keepalive.then(SseControlBlockFilter::default);
        let mut sent_prefetched_chunk = false;
        for chunk in prefetched_chunks_for_body {
            if let Some(chunk) = filter_upstream_sse_control_chunk(&mut upstream_control_filter, chunk) {
                sent_prefetched_chunk = true;
                yield Ok(chunk);
            }
        }

        if emit_keepalive {
            if !sent_prefetched_chunk {
                yield Ok(Bytes::from_static(SSE_KEEPALIVE_BYTES));
            }
            let mut keepalive = tokio::time::interval(keepalive_interval);
            keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
            keepalive.tick().await;
            loop {
                tokio::select! {
                    biased;
                    item = rx.recv() => {
                        let Some(item) = item else {
                            break;
                        };
                        match item {
                            Ok(chunk) => {
                                if let Some(chunk) = filter_upstream_sse_control_chunk(&mut upstream_control_filter, chunk) {
                                    yield Ok(chunk);
                                }
                            }
                            Err(err) => yield Err(err),
                        }
                    }
                    _ = keepalive.tick() => {
                        yield Ok(Bytes::from_static(SSE_KEEPALIVE_BYTES));
                    }
                }
            }
            if let Some(chunk) = flush_upstream_sse_control_filter(&mut upstream_control_filter) {
                yield Ok(chunk);
            }
        } else {
            while let Some(item) = rx.recv().await {
                yield item;
            }
        }
    }
}

#[derive(Default)]
struct SseControlBlockFilter {
    buffered: Vec<u8>,
    emitted_len: usize,
    passthrough_current_block: bool,
}

impl SseControlBlockFilter {
    fn push_chunk(&mut self, chunk: &[u8]) -> Vec<u8> {
        if chunk.is_empty() {
            return Vec::new();
        }

        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some((block_end, separator_len)) = find_sse_block_boundary(&self.buffered) {
            let block_len = block_end + separator_len;
            let block = self.buffered.drain(..block_len).collect::<Vec<_>>();
            if self.passthrough_current_block {
                let emitted_len = self.emitted_len.min(block.len());
                output.extend_from_slice(&block[emitted_len..]);
            } else if sse_block_has_data_line(&block) {
                output.extend_from_slice(&block);
            }
            self.emitted_len = 0;
            self.passthrough_current_block = false;
        }

        if self.passthrough_current_block {
            if self.buffered.len() > self.emitted_len {
                output.extend_from_slice(&self.buffered[self.emitted_len..]);
                self.emitted_len = self.buffered.len();
            }
        } else if sse_buffer_has_data_line(&self.buffered) {
            self.passthrough_current_block = true;
            output.extend_from_slice(&self.buffered);
            self.emitted_len = self.buffered.len();
        }

        if self.buffered.len() > SSE_CONTROL_FILTER_MAX_BUFFER_BYTES {
            let buffered = std::mem::take(&mut self.buffered);
            if self.passthrough_current_block {
                let emitted_len = self.emitted_len.min(buffered.len());
                output.extend_from_slice(&buffered[emitted_len..]);
            } else {
                output.extend(buffered);
            }
            self.emitted_len = 0;
            self.passthrough_current_block = false;
        }

        output
    }

    fn finish(&mut self) -> Vec<u8> {
        if self.buffered.is_empty() {
            return Vec::new();
        }

        let block = std::mem::take(&mut self.buffered);
        let emitted_len = self.emitted_len.min(block.len());
        let passthrough_current_block = self.passthrough_current_block;
        self.emitted_len = 0;
        self.passthrough_current_block = false;
        if passthrough_current_block {
            block[emitted_len..].to_vec()
        } else if sse_block_has_data_line(&block) {
            block
        } else {
            Vec::new()
        }
    }
}

fn filter_upstream_sse_control_chunk(
    filter: &mut Option<SseControlBlockFilter>,
    chunk: Bytes,
) -> Option<Bytes> {
    let Some(filter) = filter.as_mut() else {
        return Some(chunk);
    };

    let filtered = filter.push_chunk(chunk.as_ref());
    (!filtered.is_empty()).then(|| Bytes::from(filtered))
}

fn flush_upstream_sse_control_filter(filter: &mut Option<SseControlBlockFilter>) -> Option<Bytes> {
    let filtered = filter.as_mut()?.finish();
    (!filtered.is_empty()).then(|| Bytes::from(filtered))
}

fn find_sse_block_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2));
    let crlf = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4));

    match (lf, crlf) {
        (Some(lf), Some(crlf)) => Some(if lf.0 <= crlf.0 { lf } else { crlf }),
        (Some(lf), None) => Some(lf),
        (None, Some(crlf)) => Some(crlf),
        (None, None) => None,
    }
}

fn sse_block_has_data_line(block: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(block) else {
        return true;
    };

    text.lines()
        .any(|line| line.trim_start().starts_with("data:"))
}

fn sse_buffer_has_data_line(buffer: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(buffer) else {
        return true;
    };

    text.lines()
        .any(|line| line.trim_start().starts_with("data:"))
}

fn stream_chunk_contains_sse_done(chunk: &[u8]) -> bool {
    std::str::from_utf8(chunk).ok().is_some_and(|text| {
        text.lines().any(|line| {
            let line = line.trim();
            if matches!(
                line,
                "data: [DONE]"
                    | "event: message_stop"
                    | "event: response.completed"
                    | "event: response.failed"
                    | "event: response.incomplete"
                    | "event: error"
            ) {
                return true;
            }
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                return false;
            };
            data == "[DONE]"
                || serde_json::from_str::<serde_json::Value>(data).is_ok_and(|value| {
                    value
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|event_type| {
                            matches!(
                                event_type,
                                "message_stop"
                                    | "response.completed"
                                    | "response.failed"
                                    | "response.incomplete"
                                    | "error"
                            )
                        })
                })
        })
    })
}

async fn next_stream_frame<R>(
    buffered_frames: &mut VecDeque<StreamFrame>,
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Option<StreamFrame>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    if let Some(frame) = buffered_frames.pop_front() {
        return Ok(Some(frame));
    }
    read_next_frame(lines).await
}

fn should_refresh_stream_usage_telemetry(
    previous: Option<&ExecutionTelemetry>,
    next: &ExecutionTelemetry,
) -> bool {
    let previous_ttfb = previous.and_then(|telemetry| telemetry.ttfb_ms);
    let previous_elapsed = previous.and_then(|telemetry| telemetry.elapsed_ms);
    let next_ttfb = next.ttfb_ms;
    let next_elapsed = next.elapsed_ms;

    (next_ttfb.is_some() && next_ttfb != previous_ttfb)
        || (next_elapsed.is_some() && next_elapsed != previous_elapsed)
}

fn build_terminal_stream_telemetry(
    stream_started_at: Instant,
    telemetry: Option<&ExecutionTelemetry>,
    usage_stream_telemetry: Option<&ExecutionTelemetry>,
    upstream_bytes: u64,
) -> ExecutionTelemetry {
    let current_elapsed_ms = stream_started_at
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let ttfb_ms = telemetry
        .and_then(|telemetry| telemetry.ttfb_ms)
        .or_else(|| usage_stream_telemetry.and_then(|telemetry| telemetry.ttfb_ms));
    let prior_elapsed_ms = telemetry
        .and_then(|telemetry| telemetry.elapsed_ms)
        .or_else(|| usage_stream_telemetry.and_then(|telemetry| telemetry.elapsed_ms))
        .unwrap_or(0);
    let elapsed_ms = current_elapsed_ms
        .max(prior_elapsed_ms)
        .max(ttfb_ms.unwrap_or(0));
    ExecutionTelemetry {
        ttfb_ms,
        elapsed_ms: Some(elapsed_ms),
        upstream_bytes: Some(upstream_bytes),
    }
}

fn should_skip_direct_finalize_prefetch(
    direct_stream_finalize_kind: Option<&str>,
    content_type: Option<&str>,
    provider_api_format: &str,
    client_api_format: &str,
    has_private_stream_normalizer: bool,
    has_local_stream_rewriter: bool,
) -> bool {
    if direct_stream_finalize_kind.is_none() {
        return false;
    }

    let content_type = content_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.contains("text/event-stream") {
        return true;
    }

    if has_private_stream_normalizer || has_local_stream_rewriter {
        return false;
    }

    if !provider_api_format.eq_ignore_ascii_case(client_api_format) {
        return false;
    }

    if content_type.is_empty() {
        return true;
    }

    !(content_type.contains("json") || content_type.ends_with("+json"))
}

fn should_probe_success_failover_before_stream(headers: &BTreeMap<String, String>) -> bool {
    let content_type = headers
        .get("content-type")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();

    content_type.contains("json") || content_type.ends_with("+json")
}

async fn probe_local_stream_success_failover_text<R>(
    buffered_frames: &mut VecDeque<StreamFrame>,
    lines: &mut FramedRead<R, LinesCodec>,
) -> Result<Option<String>, GatewayError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    while let Some(frame) = read_next_frame(lines).await? {
        let probe_text = match &frame.payload {
            StreamFramePayload::Data { chunk_b64, text } => {
                match decode_stream_data_chunk(chunk_b64.as_deref(), text.as_deref()) {
                    Ok(chunk) if !chunk.is_empty() => {
                        Some(String::from_utf8_lossy(&chunk).into_owned())
                    }
                    Ok(_) | Err(_) => None,
                }
            }
            StreamFramePayload::Error { .. } | StreamFramePayload::Eof { .. } => None,
            StreamFramePayload::Headers { .. } | StreamFramePayload::Telemetry { .. } => None,
        };
        buffered_frames.push_back(frame);
        if probe_text.is_some() {
            return Ok(probe_text);
        }
    }

    Ok(None)
}

async fn execute_stream_from_frame_stream(
    state: &AppState,
    plan: ExecutionPlan,
    trace_id: &str,
    decision: &GatewayControlDecision,
    plan_kind: &str,
    report_kind: Option<String>,
    report_context: Option<serde_json::Value>,
    candidate_started_unix_secs: u64,
    stream_started_at: Instant,
    frame_stream: BoxStream<'static, Result<Bytes, IoError>>,
    in_flight_guard: Option<ProviderPoolInFlightGuard>,
) -> Result<Option<Response<Body>>, GatewayError> {
    let request_id = plan.request_id.as_str();
    let request_id_for_log = short_request_id(request_id);
    let candidate_id = plan.candidate_id.as_deref();
    let provider_name = plan.provider_name.as_deref().unwrap_or("-");
    let model_name = plan.model_name.as_deref().unwrap_or("-");
    let lifecycle_seed = build_lifecycle_usage_seed(&plan, report_context.as_ref());
    let request_candidate_status_snapshot =
        snapshot_local_request_candidate_status(&plan, report_context.as_ref());
    let candidate_index = parse_request_candidate_report_context(report_context.as_ref())
        .and_then(|context| context.candidate_index)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let reader = StreamReader::new(frame_stream);
    let mut lines = FramedRead::new(reader, LinesCodec::new());

    let first_frame = read_next_frame(&mut lines).await?.ok_or_else(|| {
        GatewayError::Internal("execution runtime stream ended before headers frame".to_string())
    })?;
    let StreamFramePayload::Headers {
        status_code,
        mut headers,
    } = first_frame.payload
    else {
        return Err(GatewayError::Internal(
            "execution runtime stream must start with headers frame".to_string(),
        ));
    };
    let mut report_context =
        attach_provider_response_headers_to_report_context(report_context, &headers);
    seed_kiro_report_context_input_tokens(&plan, &mut report_context);
    if status_code == 200 {
        seed_kiro_simulated_cache_enabled(state, &plan, &mut report_context).await;
        seed_kiro_report_context_prompt_cache_usage(&plan, &mut report_context);
    }
    let mut buffered_frames = VecDeque::new();
    let mut stream_terminal_summary: Option<ExecutionStreamTerminalSummary> = None;
    if status_code == 200 && should_probe_success_failover_before_stream(&headers) {
        let success_probe_text =
            probe_local_stream_success_failover_text(&mut buffered_frames, &mut lines).await?;
        if should_retry_next_local_candidate_stream(
            state,
            &plan,
            plan_kind,
            report_context.as_ref(),
            status_code,
            success_probe_text.as_deref(),
        )
        .await
        {
            let terminal_unix_secs = current_request_candidate_unix_ms();
            record_local_request_candidate_status(
                state,
                &plan,
                report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: Some(status_code),
                    error_type: Some("success_failover_pattern".to_string()),
                    error_message: Some(
                        "execution runtime stream matched provider success failover rule"
                            .to_string(),
                    ),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            warn!(
                event_name = "local_stream_candidate_retry_scheduled",
                log_type = "event",
                trace_id = %trace_id,
                request_id = %request_id_for_log,
                status_code,
                provider_name = provider_name,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                model_name,
                candidate_index = candidate_index.as_str(),
                "gateway local stream decision retrying next candidate after success failover rule match"
            );
            return Ok(None);
        }
    }

    let stream_error_finalize_kind =
        resolve_core_stream_error_finalize_report_kind(plan_kind, status_code);

    if !(200..300).contains(&status_code) {
        let provider_error_body = collect_error_body(&mut lines).await?;
        let private_error_body_json = extract_provider_private_stream_error_body(
            report_context.as_ref(),
            &provider_error_body,
        );
        let provider_private_error_decoded = private_error_body_json.is_some();
        let synthetic_body_json = (!provider_private_error_decoded
            && should_synthesize_non_success_stream_error_body(status_code, &provider_error_body))
        .then(|| build_synthetic_non_success_stream_error_body(status_code, &headers));
        let (provider_body_json, provider_body_base64) =
            if let Some(error_body_json) = private_error_body_json {
                (Some(error_body_json), None)
            } else {
                decode_stream_error_body(&headers, &provider_error_body)
            };
        let client_status_code = stream_client_error_status_code_for_upstream_status(status_code);
        let wrapped_binary_body_json = if provider_private_error_decoded {
            None
        } else {
            wrap_non_json_binary_stream_error_for_client(plan_kind, &headers, &provider_error_body)?
        };
        let (client_body_json, client_error_body, payload_client_body_json) =
            if let Some(body_json) = synthetic_body_json.or(wrapped_binary_body_json) {
                let body_bytes = serde_json::to_vec(&body_json)
                    .map_err(|err| GatewayError::Internal(err.to_string()))?;
                (Some(body_json.clone()), body_bytes, Some(body_json))
            } else if provider_private_error_decoded {
                let body_json = provider_body_json.clone().ok_or_else(|| {
                    GatewayError::Internal(
                        "decoded provider private stream error body is missing".to_string(),
                    )
                })?;
                let body_bytes = serde_json::to_vec(&body_json)
                    .map_err(|err| GatewayError::Internal(err.to_string()))?;
                (Some(body_json), body_bytes, None)
            } else {
                (
                    provider_body_json.clone(),
                    provider_error_body.clone(),
                    provider_body_json.clone(),
                )
            };
        let error_response_text =
            local_failover_response_text(client_body_json.as_ref(), &client_error_body, None);
        let failover_analysis = resolve_local_candidate_failover_analysis_stream(
            state,
            &plan,
            report_context.as_ref(),
            status_code,
            error_response_text.as_deref(),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::AttemptFailure(LocalAttemptFailureEffect {
                status_code,
                classification: failover_analysis.classification,
            }),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::AdaptiveRateLimit(LocalAdaptiveRateLimitEffect {
                status_code,
                classification: failover_analysis.classification,
                headers: Some(&headers),
            }),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::HealthFailure(LocalHealthFailureEffect {
                status_code,
                classification: failover_analysis.classification,
            }),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::OauthInvalidation(LocalOAuthInvalidationEffect {
                status_code,
                response_text: error_response_text.as_deref(),
            }),
        )
        .await;
        apply_local_execution_effect(
            state,
            LocalExecutionEffectContext {
                plan: &plan,
                report_context: report_context.as_ref(),
            },
            LocalExecutionEffect::PoolError(LocalPoolErrorEffect {
                status_code,
                classification: failover_analysis.classification,
                headers: &headers,
                error_body: error_response_text.as_deref(),
            }),
        )
        .await;
        let failover_decision = failover_analysis.decision;
        debug!(
            event_name = "execution_runtime_stream_failover_decided",
            log_type = "debug",
            trace_id = %trace_id,
            request_id = %request_id_for_log,
            candidate_id = ?candidate_id,
            plan_kind,
            status_code,
            provider_name,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            model_name,
            candidate_index = candidate_index.as_str(),
            failover_decision = failover_decision.as_str(),
            "gateway resolved execution runtime stream failover decision"
        );
        if matches!(failover_decision, LocalFailoverDecision::RetryNextCandidate) {
            let terminal_unix_secs = current_request_candidate_unix_ms();
            let error_trace_report_context = with_stream_error_trace_context(
                report_context.as_ref(),
                status_code,
                &headers,
                provider_body_json.as_ref(),
                &provider_error_body,
                error_response_text.as_deref(),
                failover_analysis,
            );
            record_local_request_candidate_status(
                state,
                &plan,
                error_trace_report_context
                    .as_ref()
                    .or(report_context.as_ref()),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: Some(status_code),
                    error_type: Some("retryable_upstream_status".to_string()),
                    error_message: Some(format!(
                        "execution runtime stream returned retryable status {status_code}"
                    )),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            warn!(
                event_name = "local_stream_candidate_retry_scheduled",
                log_type = "event",
                trace_id = %trace_id,
                request_id = %request_id_for_log,
                status_code,
                provider_name = provider_name,
                endpoint_id = %plan.endpoint_id,
                key_id = %plan.key_id,
                model_name,
                candidate_index = candidate_index.as_str(),
                "gateway local stream decision retrying next candidate after retryable execution runtime status"
            );
            return Ok(None);
        }

        if !matches!(failover_decision, LocalFailoverDecision::StopLocalFailover)
            && should_fallback_to_control_stream(
                plan_kind,
                status_code,
                stream_error_finalize_kind.is_some(),
            )
        {
            let terminal_unix_secs = current_request_candidate_unix_ms();
            let error_trace_report_context = with_stream_error_trace_context(
                report_context.as_ref(),
                status_code,
                &headers,
                provider_body_json.as_ref(),
                &provider_error_body,
                error_response_text.as_deref(),
                failover_analysis,
            );
            record_local_request_candidate_status(
                state,
                &plan,
                error_trace_report_context
                    .as_ref()
                    .or(report_context.as_ref()),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Failed,
                    status_code: Some(status_code),
                    error_type: Some("control_fallback".to_string()),
                    error_message: Some(format!(
                        "stream decision fell back to control after status {status_code}"
                    )),
                    latency_ms: None,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: Some(terminal_unix_secs),
                },
            )
            .await;
            return Ok(None);
        }

        let mut client_headers = if (300..400).contains(&status_code) {
            let mut headers = synthetic_error_response_headers(headers.clone());
            headers.insert(
                "x-aether-upstream-status".to_string(),
                status_code.to_string(),
            );
            headers
        } else {
            headers.clone()
        };
        if provider_private_error_decoded {
            client_headers.remove("content-encoding");
            client_headers.remove("content-length");
            client_headers.insert("content-type".to_string(), "application/json".to_string());
        }
        apply_endpoint_response_header_rules(
            state,
            &plan,
            &mut client_headers,
            client_body_json.as_ref(),
        )
        .await?;

        let client_response_headers = client_headers.clone();
        let error_trace_report_context = with_stream_error_trace_context(
            report_context.as_ref(),
            status_code,
            &headers,
            provider_body_json.as_ref(),
            &provider_error_body,
            error_response_text.as_deref(),
            failover_analysis,
        );
        let payload = build_stream_error_sync_payload(
            trace_id,
            stream_error_finalize_kind
                .as_deref()
                .or(report_kind.as_deref())
                .unwrap_or_default()
                .to_string(),
            error_trace_report_context.or(report_context),
            status_code,
            headers.clone(),
            provider_body_json,
            provider_body_base64,
            client_headers,
            payload_client_body_json,
            None,
        );
        record_sync_terminal_usage(state, &plan, payload.report_context.as_ref(), &payload);
        let terminal_unix_secs = current_request_candidate_unix_ms();
        record_local_request_candidate_status(
            state,
            &plan,
            payload.report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: RequestCandidateStatus::Failed,
                status_code: Some(status_code),
                error_type: Some("execution_runtime_stream_non_success_status".to_string()),
                error_message: Some(format!(
                    "execution runtime stream returned non-success status {status_code}"
                )),
                latency_ms: None,
                started_at_unix_ms: Some(candidate_started_unix_secs),
                finished_at_unix_ms: Some(terminal_unix_secs),
            },
        )
        .await;
        if stream_error_finalize_kind.is_some() {
            let response =
                submit_local_core_error_or_sync_finalize(state, trace_id, decision, payload)
                    .await?;
            return Ok(Some(attach_control_metadata_headers(
                response,
                Some(request_id),
                candidate_id,
            )?));
        }
        return Ok(Some(attach_control_metadata_headers(
            build_client_response_from_parts(
                client_status_code,
                &client_response_headers,
                Body::from(client_error_body),
                trace_id,
                Some(decision),
            )?,
            Some(request_id),
            candidate_id,
        )?));
    }

    let direct_stream_finalize_kind = resolve_core_stream_direct_finalize_report_kind(plan_kind);
    let normalized_stream_report_context =
        normalize_provider_private_report_context(report_context.as_ref());
    let upstream_headers = headers.clone();
    let mut private_stream_normalizer =
        maybe_build_provider_private_stream_normalizer(report_context.as_ref());
    let mut local_stream_rewriter =
        maybe_build_stream_response_rewriter(normalized_stream_report_context.as_ref());
    if private_stream_normalizer.is_some() || local_stream_rewriter.is_some() {
        headers.remove("content-encoding");
        headers.remove("content-length");
        headers.insert("content-type".to_string(), "text/event-stream".to_string());
    }
    let upstream_content_type = upstream_headers.get("content-type").map(String::as_str);
    let skip_direct_finalize_prefetch = should_skip_direct_finalize_prefetch(
        direct_stream_finalize_kind.as_deref(),
        upstream_content_type,
        plan.provider_api_format.as_str(),
        plan.client_api_format.as_str(),
        private_stream_normalizer.is_some(),
        local_stream_rewriter.is_some(),
    );
    let limit_direct_finalize_prefetch =
        should_limit_direct_finalize_prefetch(plan_kind, local_stream_rewriter.is_some());
    let mut prefetched_chunks: Vec<Bytes> = Vec::new();
    let mut provider_prefetched_body = Vec::new();
    let mut prefetched_body = Vec::new();
    let mut prefetched_inspection_body = Vec::new();
    let mut prefetched_telemetry: Option<ExecutionTelemetry> = None;
    let mut reached_eof = false;
    let mut sync_json_stream_bridge_active = false;
    if skip_direct_finalize_prefetch {
        debug!(
            event_name = "execution_runtime_stream_prefetch_skipped",
            log_type = "debug",
            trace_id = %trace_id,
            request_id = %request_id_for_log,
            candidate_id = ?candidate_id,
            plan_kind,
            provider_name,
            endpoint_id = %plan.endpoint_id,
            key_id = %plan.key_id,
            model_name,
            candidate_index = candidate_index.as_str(),
            content_type = upstream_content_type.unwrap_or("-"),
            provider_api_format = plan.provider_api_format.as_str(),
            client_api_format = plan.client_api_format.as_str(),
            "gateway skipped direct finalize prefetch for same-format passthrough stream"
        );
    }
    if let Some(report_kind) = direct_stream_finalize_kind
        .as_ref()
        .filter(|_| !skip_direct_finalize_prefetch)
    {
        while prefetched_chunks.len() < MAX_STREAM_PREFETCH_FRAMES
            && prefetched_inspection_body.len() < MAX_STREAM_PREFETCH_BYTES
        {
            let next_frame_result = if limit_direct_finalize_prefetch {
                match tokio::time::timeout(
                    REWRITTEN_STREAM_PREFETCH_TIMEOUT,
                    next_stream_frame(&mut buffered_frames, &mut lines),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => {
                        debug!(
                            event_name = "execution_runtime_stream_prefetch_limited",
                            log_type = "debug",
                            trace_id = %trace_id,
                            request_id = %request_id_for_log,
                            candidate_id = ?candidate_id,
                            plan_kind,
                            report_kind,
                            provider_name,
                            endpoint_id = %plan.endpoint_id,
                            key_id = %plan.key_id,
                            model_name,
                            candidate_index = candidate_index.as_str(),
                            timeout_ms = REWRITTEN_STREAM_PREFETCH_TIMEOUT.as_millis() as u64,
                            "gateway stopped rewritten stream prefetch before client-visible body"
                        );
                        break;
                    }
                }
            } else {
                next_stream_frame(&mut buffered_frames, &mut lines).await
            };
            let Some(frame) = (match next_frame_result {
                Ok(frame) => frame,
                Err(err) => {
                    let failure = build_stream_failure_report(
                        "execution_runtime_stream_frame_decode_error",
                        format!("failed to decode execution runtime stream frame: {err:?}"),
                        502,
                    );
                    return handle_prefetch_stream_failure(
                        state,
                        trace_id,
                        decision,
                        &plan,
                        report_context,
                        request_id,
                        candidate_id,
                        report_kind,
                        headers,
                        prefetched_telemetry,
                        &provider_prefetched_body,
                        failure,
                    )
                    .await;
                }
            }) else {
                reached_eof = true;
                break;
            };
            match frame.payload {
                StreamFramePayload::Data { chunk_b64, text } => {
                    let chunk =
                        match decode_stream_data_chunk(chunk_b64.as_deref(), text.as_deref()) {
                            Ok(chunk) => chunk,
                            Err(err) => {
                                let failure = build_stream_failure_report(
                                    "execution_runtime_stream_chunk_decode_error",
                                    format!(
                                        "failed to decode execution runtime stream chunk: {err:?}"
                                    ),
                                    502,
                                );
                                return handle_prefetch_stream_failure(
                                    state,
                                    trace_id,
                                    decision,
                                    &plan,
                                    report_context,
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    headers,
                                    prefetched_telemetry,
                                    &prefetched_body,
                                    failure,
                                )
                                .await;
                            }
                        };

                    if chunk.is_empty() {
                        continue;
                    }

                    provider_prefetched_body.extend_from_slice(&chunk);
                    prefetched_inspection_body.extend_from_slice(&chunk);

                    if let Some(error_body_json) = extract_provider_private_stream_error_body(
                        report_context.as_ref(),
                        &prefetched_inspection_body,
                    ) {
                        let error_status_code =
                            resolve_local_sync_error_status_code(status_code, &error_body_json);
                        return handle_prefetch_provider_private_stream_error(
                            state,
                            trace_id,
                            decision,
                            &plan,
                            report_context,
                            request_id,
                            candidate_id,
                            report_kind,
                            headers,
                            prefetched_telemetry,
                            &provider_prefetched_body,
                            error_status_code,
                            error_body_json,
                        )
                        .await;
                    }

                    let inspection = inspect_prefetched_stream_body(
                        &upstream_headers,
                        &prefetched_inspection_body,
                    );
                    match inspection {
                        StreamPrefetchInspection::EmbeddedError(body_json) => {
                            debug!(
                                event_name = "execution_runtime_stream_prefetch_embedded_error_detected",
                                log_type = "debug",
                                trace_id = %trace_id,
                                request_id = %request_id_for_log,
                                candidate_id = ?candidate_id,
                                plan_kind,
                                report_kind,
                                provider_name,
                                endpoint_id = %plan.endpoint_id,
                                key_id = %plan.key_id,
                                model_name,
                                candidate_index = candidate_index.as_str(),
                                provider_prefetched_body_bytes = provider_prefetched_body.len(),
                                "gateway detected embedded error while prefetching execution runtime stream"
                            );
                            let payload = build_stream_sync_payload(
                                trace_id,
                                report_kind.clone(),
                                report_context,
                                status_code,
                                headers,
                                Some(body_json),
                                None,
                                prefetched_telemetry,
                            );
                            record_sync_terminal_usage(
                                state,
                                &plan,
                                payload.report_context.as_ref(),
                                &payload,
                            );
                            let response = submit_local_core_error_or_sync_finalize(
                                state, trace_id, decision, payload,
                            )
                            .await?;
                            return Ok(Some(attach_control_metadata_headers(
                                response,
                                Some(request_id),
                                candidate_id,
                            )?));
                        }
                        StreamPrefetchInspection::NeedMore => {}
                        StreamPrefetchInspection::NonError => {}
                    }

                    if !response_headers_indicate_sse(&upstream_headers)
                        && (200..300).contains(&status_code)
                    {
                        if let Some(body_json) =
                            parse_prefetched_sync_json_body(&prefetched_inspection_body)
                        {
                            match maybe_bridge_standard_sync_json_to_stream(
                                &body_json,
                                plan.provider_api_format.as_str(),
                                plan.client_api_format.as_str(),
                                report_context.as_ref(),
                            ) {
                                Ok(Some(outcome)) => {
                                    headers.remove("content-encoding");
                                    headers.remove("content-length");
                                    headers.insert(
                                        "content-type".to_string(),
                                        "text/event-stream".to_string(),
                                    );
                                    stream_terminal_summary = outcome.terminal_summary;
                                    prefetched_body.extend_from_slice(&outcome.sse_body);
                                    prefetched_chunks.push(Bytes::from(outcome.sse_body));
                                    sync_json_stream_bridge_active = true;
                                    break;
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    let failure = build_stream_failure_report(
                                        "execution_runtime_sync_json_stream_bridge_error",
                                        format!(
                                            "failed to bridge execution runtime sync json to stream: {err:?}"
                                        ),
                                        502,
                                    );
                                    return handle_prefetch_stream_failure(
                                        state,
                                        trace_id,
                                        decision,
                                        &plan,
                                        report_context,
                                        request_id,
                                        candidate_id,
                                        report_kind,
                                        headers,
                                        prefetched_telemetry,
                                        &provider_prefetched_body,
                                        failure,
                                    )
                                    .await;
                                }
                            }
                        }
                    }

                    let normalized_chunk = if let Some(normalizer) =
                        private_stream_normalizer.as_mut()
                    {
                        match normalizer.push_chunk(&chunk) {
                            Ok(normalized_chunk) => normalized_chunk,
                            Err(err) => {
                                let failure = build_stream_failure_report(
                                    "execution_runtime_stream_rewrite_error",
                                    format!(
                                        "failed to normalize execution runtime stream chunk: {err:?}"
                                    ),
                                    502,
                                );
                                return handle_prefetch_stream_failure(
                                    state,
                                    trace_id,
                                    decision,
                                    &plan,
                                    report_context,
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    headers,
                                    prefetched_telemetry,
                                    &provider_prefetched_body,
                                    failure,
                                )
                                .await;
                            }
                        }
                    } else {
                        chunk
                    };
                    let rewritten_chunk = if let Some(rewriter) = local_stream_rewriter.as_mut() {
                        match rewriter.push_chunk(&normalized_chunk) {
                            Ok(rewritten_chunk) => rewritten_chunk,
                            Err(err) => {
                                let failure = build_stream_failure_report(
                                    "execution_runtime_stream_rewrite_error",
                                    format!(
                                        "failed to rewrite execution runtime stream chunk: {err:?}"
                                    ),
                                    502,
                                );
                                return handle_prefetch_stream_failure(
                                    state,
                                    trace_id,
                                    decision,
                                    &plan,
                                    report_context,
                                    request_id,
                                    candidate_id,
                                    report_kind,
                                    headers,
                                    prefetched_telemetry,
                                    &provider_prefetched_body,
                                    failure,
                                )
                                .await;
                            }
                        }
                    } else {
                        normalized_chunk
                    };
                    if !rewritten_chunk.is_empty() {
                        prefetched_body.extend_from_slice(&rewritten_chunk);
                        prefetched_chunks.push(Bytes::from(rewritten_chunk));
                    }

                    if matches!(inspection, StreamPrefetchInspection::NonError) {
                        break;
                    }
                }
                StreamFramePayload::Telemetry {
                    telemetry: frame_telemetry,
                } => {
                    prefetched_telemetry = Some(frame_telemetry);
                }
                StreamFramePayload::Eof { summary } => {
                    if summary.is_some() {
                        stream_terminal_summary = summary;
                    }
                    reached_eof = true;
                    break;
                }
                StreamFramePayload::Error { error } => {
                    warn!(
                        event_name = "stream_execution_prefetch_error_frame",
                        log_type = "ops",
                        trace_id = %trace_id,
                        request_id,
                        candidate_id = ?candidate_id,
                        error = %error.message,
                        "execution runtime stream emitted error frame during prefetch"
                    );
                    return handle_prefetch_stream_failure(
                        state,
                        trace_id,
                        decision,
                        &plan,
                        report_context,
                        request_id,
                        candidate_id,
                        report_kind,
                        headers,
                        prefetched_telemetry,
                        &provider_prefetched_body,
                        build_stream_failure_from_execution_error(&error),
                    )
                    .await;
                }
                StreamFramePayload::Headers { .. } => {}
            }
        }
    }
    drop(private_stream_normalizer);
    drop(local_stream_rewriter);

    state.usage_runtime.record_stream_started(
        state.data.as_ref(),
        &lifecycle_seed,
        status_code,
        prefetched_telemetry.as_ref(),
    );
    if let Some(snapshot) = request_candidate_status_snapshot {
        let state_bg = state.clone();
        let latency_ms = prefetched_telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.elapsed_ms);
        tokio::spawn(async move {
            record_local_request_candidate_status_snapshot(
                &state_bg,
                &snapshot,
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Streaming,
                    status_code: Some(status_code),
                    error_type: None,
                    error_message: None,
                    latency_ms,
                    started_at_unix_ms: Some(candidate_started_unix_secs),
                    finished_at_unix_ms: None,
                },
            )
            .await;
        });
    }

    apply_endpoint_response_header_rules(state, &plan, &mut headers, None).await?;

    let request_id = request_id.to_string();
    let candidate_id = candidate_id.map(ToOwned::to_owned);
    let (tx, mut rx) = mpsc::channel::<Result<Bytes, IoError>>(16);
    let state_for_report = state.clone();
    let trace_id_owned = trace_id.to_string();
    let headers_for_report = headers.clone();
    let report_kind_owned = report_kind;
    let report_context_owned = report_context;
    let normalized_stream_report_context_owned = normalized_stream_report_context;
    let lifecycle_seed_for_report = lifecycle_seed;
    let provider_prefetched_body_for_report = provider_prefetched_body;
    let prefetched_body_for_report = prefetched_body;
    let prefetched_chunks_for_body = prefetched_chunks;
    let sync_json_stream_bridge_active_for_report = sync_json_stream_bridge_active;
    let initial_telemetry = prefetched_telemetry;
    let initial_reached_eof = reached_eof;
    let direct_stream_finalize_kind_owned = direct_stream_finalize_kind;
    let candidate_started_unix_secs_for_report = candidate_started_unix_secs;
    let request_id_for_report = request_id.clone();
    let request_id_for_report_log = short_request_id(&request_id);
    let candidate_id_for_report = candidate_id.clone();
    let candidate_index_for_report = candidate_index.clone();
    let is_openai_image_stream_for_report = plan_kind == OPENAI_IMAGE_STREAM_PLAN_KIND;
    let openai_image_stream_total_timeout_ms =
        resolve_openai_image_stream_total_timeout_ms(plan_kind, &plan);
    let plan_for_report = plan;
    let emit_passthrough_sse_terminal_error = skip_direct_finalize_prefetch
        && response_headers_indicate_sse(&upstream_headers)
        && !is_openai_image_stream_for_report;
    let body_capture_policy = match UsageRuntimeAccess::body_capture_policy(state.data.as_ref())
        .await
    {
        Ok(policy) => policy,
        Err(err) => {
            warn!(
                event_name = "stream_execution_body_capture_policy_read_failed",
                log_type = "ops",
                trace_id = %trace_id,
                request_id = %request_id_for_report_log,
                candidate_id = ?candidate_id_for_report.as_deref(),
                error = %err,
                fallback_request_body_bytes = DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES,
                "gateway failed to read body capture policy; falling back to default stream capture limits"
            );
            UsageBodyCapturePolicy::default()
        }
    };
    let max_stream_body_buffer_bytes = if matches!(
        body_capture_policy.record_level,
        UsageRequestRecordLevel::Basic
    ) {
        0
    } else {
        body_capture_policy
            .max_response_body_bytes
            .unwrap_or(DEFAULT_USAGE_RESPONSE_BODY_CAPTURE_LIMIT_BYTES)
    };
    let plan_kind_for_report = plan_kind.to_string();
    let stream_started_at_for_report = stream_started_at;
    let provider_pool_in_flight_guard_for_report = in_flight_guard;
    tokio::spawn(async move {
        let _provider_pool_in_flight_guard = provider_pool_in_flight_guard_for_report;
        let mut provider_buffered_body = Vec::new();
        let mut buffered_body = Vec::new();
        let mut provider_body_truncated = false;
        let mut client_body_truncated = false;
        let mut private_stream_normalizer = if sync_json_stream_bridge_active_for_report {
            None
        } else {
            maybe_build_provider_private_stream_normalizer(report_context_owned.as_ref())
        };
        let mut local_stream_rewriter = if sync_json_stream_bridge_active_for_report {
            None
        } else {
            maybe_build_stream_response_rewriter(normalized_stream_report_context_owned.as_ref())
        };
        let stream_usage_report_context =
            normalized_stream_report_context_owned.clone().or_else(|| {
                Some(serde_json::json!({
                    "provider_api_format": plan_for_report.provider_api_format.as_str(),
                    "client_api_format": plan_for_report.client_api_format.as_str(),
                }))
            });
        let mut stream_usage_observer = stream_usage_report_context
            .as_ref()
            .filter(|_| !sync_json_stream_bridge_active_for_report)
            .map(|_| StreamingStandardTerminalObserver::default());
        let mut stream_usage_observer_buffered = Vec::new();
        append_stream_capture_bytes(
            &mut provider_buffered_body,
            &provider_prefetched_body_for_report,
            max_stream_body_buffer_bytes,
            &mut provider_body_truncated,
        );
        append_stream_capture_bytes(
            &mut buffered_body,
            &prefetched_body_for_report,
            max_stream_body_buffer_bytes,
            &mut client_body_truncated,
        );
        let mut client_visible_stream_completed =
            stream_chunk_contains_sse_done(&prefetched_body_for_report);
        let mut usage_stream_telemetry: Option<ExecutionTelemetry> = initial_telemetry.clone();
        let mut telemetry: Option<ExecutionTelemetry> = initial_telemetry;
        let reached_eof = initial_reached_eof;
        let mut downstream_dropped = false;
        let mut terminal_failure: Option<StreamFailureReport> = None;
        let initial_elapsed_ms = stream_started_at_for_report
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let last_upstream_frame_elapsed_ms = Arc::new(AtomicU64::new(initial_elapsed_ms));
        let last_client_chunk_elapsed_ms =
            Arc::new(AtomicU64::new(if prefetched_body_for_report.is_empty() {
                0
            } else {
                initial_elapsed_ms
            }));
        let provider_stream_bytes = Arc::new(AtomicU64::new(
            u64::try_from(provider_prefetched_body_for_report.len()).unwrap_or(u64::MAX),
        ));
        let client_stream_bytes = Arc::new(AtomicU64::new(
            u64::try_from(prefetched_body_for_report.len()).unwrap_or(u64::MAX),
        ));
        let idle_monitor_done = Arc::new(AtomicBool::new(false));
        let idle_monitor_handle = {
            let done = Arc::clone(&idle_monitor_done);
            let last_upstream = Arc::clone(&last_upstream_frame_elapsed_ms);
            let last_client = Arc::clone(&last_client_chunk_elapsed_ms);
            let provider_bytes = Arc::clone(&provider_stream_bytes);
            let client_bytes = Arc::clone(&client_stream_bytes);
            let trace_id_for_idle = trace_id_owned.clone();
            let request_id_for_idle = request_id_for_report_log.clone();
            let candidate_id_for_idle = candidate_id_for_report.clone();
            let candidate_index_for_idle = candidate_index_for_report.clone();
            let plan_kind_for_idle = plan_kind_for_report.clone();
            let provider_name_for_idle = plan_for_report
                .provider_name
                .clone()
                .unwrap_or_else(|| "-".to_string());
            let endpoint_id_for_idle = plan_for_report.endpoint_id.clone();
            let key_id_for_idle = plan_for_report.key_id.clone();
            let model_name_for_idle = plan_for_report
                .model_name
                .clone()
                .unwrap_or_else(|| "-".to_string());
            let has_local_stream_rewriter_for_idle = local_stream_rewriter.is_some();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(STREAM_IDLE_LOG_INTERVAL);
                interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
                interval.tick().await;
                loop {
                    interval.tick().await;
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    let elapsed_ms = stream_started_at_for_report
                        .elapsed()
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64;
                    let last_upstream_frame_elapsed_ms = last_upstream.load(Ordering::Relaxed);
                    let last_client_chunk_elapsed_ms = last_client.load(Ordering::Relaxed);
                    let upstream_idle_ms =
                        elapsed_ms.saturating_sub(last_upstream_frame_elapsed_ms);
                    let client_idle_ms = if last_client_chunk_elapsed_ms == 0 {
                        elapsed_ms
                    } else {
                        elapsed_ms.saturating_sub(last_client_chunk_elapsed_ms)
                    };
                    if upstream_idle_ms >= STREAM_IDLE_LOG_INTERVAL_MS {
                        warn!(
                            event_name = "stream_execution_upstream_idle",
                            log_type = "ops",
                            trace_id = %trace_id_for_idle,
                            request_id = %request_id_for_idle,
                            candidate_id = ?candidate_id_for_idle.as_deref(),
                            candidate_index = candidate_index_for_idle.as_str(),
                            plan_kind = plan_kind_for_idle.as_str(),
                            provider_name = provider_name_for_idle.as_str(),
                            endpoint_id = %endpoint_id_for_idle,
                            key_id = %key_id_for_idle,
                            model_name = model_name_for_idle.as_str(),
                            elapsed_ms,
                            provider_bytes = provider_bytes.load(Ordering::Relaxed),
                            client_bytes = client_bytes.load(Ordering::Relaxed),
                            last_upstream_frame_elapsed_ms,
                            last_client_chunk_elapsed_ms,
                            "gateway stream has not received an upstream frame within the idle window"
                        );
                    } else if client_idle_ms >= STREAM_IDLE_LOG_INTERVAL_MS
                        && last_upstream_frame_elapsed_ms >= last_client_chunk_elapsed_ms
                    {
                        warn!(
                            event_name = "stream_execution_client_visible_idle",
                            log_type = "ops",
                            trace_id = %trace_id_for_idle,
                            request_id = %request_id_for_idle,
                            candidate_id = ?candidate_id_for_idle.as_deref(),
                            candidate_index = candidate_index_for_idle.as_str(),
                            plan_kind = plan_kind_for_idle.as_str(),
                            provider_name = provider_name_for_idle.as_str(),
                            endpoint_id = %endpoint_id_for_idle,
                            key_id = %key_id_for_idle,
                            model_name = model_name_for_idle.as_str(),
                            elapsed_ms,
                            provider_bytes = provider_bytes.load(Ordering::Relaxed),
                            client_bytes = client_bytes.load(Ordering::Relaxed),
                            last_upstream_frame_elapsed_ms,
                            last_client_chunk_elapsed_ms,
                            local_stream_rewriter = has_local_stream_rewriter_for_idle,
                            "gateway stream received upstream frames but has no recent client-visible chunk"
                        );
                    }
                }
            })
        };
        if !provider_prefetched_body_for_report.is_empty() {
            let normalized_prefetched_chunk = if let Some(normalizer) =
                private_stream_normalizer.as_mut()
            {
                match normalizer.push_chunk(&provider_prefetched_body_for_report) {
                    Ok(normalized_chunk) => Some(normalized_chunk),
                    Err(err) => {
                        warn!(
                            event_name = "stream_execution_prefetch_normalize_restore_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                            "gateway failed to restore private stream normalization state after prefetch"
                        );
                        terminal_failure = Some(build_stream_failure_report(
                            "execution_runtime_stream_rewrite_error",
                            format!(
                                "failed to restore private stream normalization state after prefetch: {err:?}"
                            ),
                            502,
                        ));
                        None
                    }
                }
            } else {
                None
            };
            let replay_chunk = normalized_prefetched_chunk
                .as_deref()
                .unwrap_or(provider_prefetched_body_for_report.as_slice());
            if let (Some(observer), Some(report_context)) = (
                stream_usage_observer.as_mut(),
                stream_usage_report_context.as_ref(),
            ) {
                observe_stream_usage_bytes(
                    observer,
                    report_context,
                    &mut stream_usage_observer_buffered,
                    replay_chunk,
                );
            }
            if terminal_failure.is_none() {
                if let Some(rewriter) = local_stream_rewriter.as_mut() {
                    if let Err(err) = rewriter.push_chunk(replay_chunk) {
                        warn!(
                            event_name = "stream_execution_prefetch_rewrite_restore_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                            "gateway failed to restore local stream rewrite state after prefetch"
                        );
                        terminal_failure = Some(build_stream_failure_report(
                            "execution_runtime_stream_rewrite_error",
                            format!(
                                "failed to restore local stream rewrite state after prefetch: {err:?}"
                            ),
                            502,
                        ));
                    }
                }
            }
        }

        if terminal_failure.is_none() && !reached_eof {
            let mut image_stream_total_timeout = openai_image_stream_total_timeout_ms
                .map(|timeout_ms| Box::pin(tokio::time::sleep(Duration::from_millis(timeout_ms))));
            loop {
                let next_frame_result = if let Some(timeout_sleep) =
                    image_stream_total_timeout.as_mut()
                {
                    tokio::select! {
                        biased;
                        _ = tx.closed(), if client_visible_stream_completed => {
                            downstream_dropped = true;
                            break;
                        }
                        result = next_stream_frame(&mut buffered_frames, &mut lines) => result,
                        _ = timeout_sleep.as_mut() => {
                            let timeout_ms = openai_image_stream_total_timeout_ms
                                .unwrap_or(OPENAI_IMAGE_STREAM_DEFAULT_TOTAL_TIMEOUT_MS);
                            let elapsed_ms = stream_started_at_for_report
                                .elapsed()
                                .as_millis()
                                .min(u128::from(u64::MAX)) as u64;
                            warn!(
                                event_name = "openai_image_stream_total_timeout",
                                log_type = "ops",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                candidate_index = candidate_index_for_report.as_str(),
                                plan_kind = plan_kind_for_report.as_str(),
                                provider_name = plan_for_report.provider_name.as_deref().unwrap_or("-"),
                                endpoint_id = %plan_for_report.endpoint_id,
                                key_id = %plan_for_report.key_id,
                                model_name = plan_for_report.model_name.as_deref().unwrap_or("-"),
                                elapsed_ms,
                                timeout_ms,
                                provider_bytes = provider_stream_bytes.load(Ordering::Relaxed),
                                client_bytes = client_stream_bytes.load(Ordering::Relaxed),
                                last_upstream_frame_elapsed_ms = last_upstream_frame_elapsed_ms.load(Ordering::Relaxed),
                                last_client_chunk_elapsed_ms = last_client_chunk_elapsed_ms.load(Ordering::Relaxed),
                                "gateway OpenAI image stream exceeded total timeout"
                            );
                            telemetry = Some(ExecutionTelemetry {
                                ttfb_ms: telemetry
                                    .as_ref()
                                    .and_then(|telemetry| telemetry.ttfb_ms)
                                    .or_else(|| {
                                        usage_stream_telemetry
                                            .as_ref()
                                            .and_then(|telemetry| telemetry.ttfb_ms)
                                    }),
                                elapsed_ms: Some(elapsed_ms),
                                upstream_bytes: Some(provider_stream_bytes.load(Ordering::Relaxed)),
                            });
                            terminal_failure = Some(build_stream_failure_report(
                                "image_stream_total_timeout",
                                format!(
                                    "OpenAI image stream exceeded total timeout of {timeout_ms}ms"
                                ),
                                504,
                            ));
                            break;
                        }
                    }
                } else {
                    tokio::select! {
                        biased;
                        _ = tx.closed(), if client_visible_stream_completed => {
                            downstream_dropped = true;
                            break;
                        }
                        result = next_stream_frame(&mut buffered_frames, &mut lines) => result,
                    }
                };
                let next_frame = match next_frame_result {
                    Ok(frame) => frame,
                    Err(err) => {
                        warn!(
                            event_name = "stream_execution_frame_decode_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                            "gateway failed to decode execution runtime stream frame"
                        );
                        terminal_failure = Some(build_stream_failure_report(
                            "execution_runtime_stream_frame_decode_error",
                            format!("failed to decode execution runtime stream frame: {err:?}"),
                            502,
                        ));
                        break;
                    }
                };
                let Some(frame) = next_frame else {
                    if tx.is_closed() {
                        downstream_dropped = true;
                    }
                    break;
                };
                let frame_elapsed_ms = stream_started_at_for_report
                    .elapsed()
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64;
                last_upstream_frame_elapsed_ms.store(frame_elapsed_ms, Ordering::Relaxed);
                match frame.payload {
                    StreamFramePayload::Data { chunk_b64, text } => {
                        if sync_json_stream_bridge_active_for_report {
                            continue;
                        }
                        let chunk =
                            match decode_stream_data_chunk(chunk_b64.as_deref(), text.as_deref()) {
                                Ok(chunk) => chunk,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_chunk_decode_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to decode execution runtime chunk"
                                    );
                                    terminal_failure = Some(build_stream_failure_report(
                                        "execution_runtime_stream_chunk_decode_error",
                                        format!(
                                        "failed to decode execution runtime stream chunk: {err:?}"
                                    ),
                                        502,
                                    ));
                                    break;
                                }
                            };

                        if chunk.is_empty() {
                            continue;
                        }

                        provider_stream_bytes.fetch_add(
                            u64::try_from(chunk.len()).unwrap_or(u64::MAX),
                            Ordering::Relaxed,
                        );
                        append_stream_capture_bytes(
                            &mut provider_buffered_body,
                            &chunk,
                            max_stream_body_buffer_bytes,
                            &mut provider_body_truncated,
                        );
                        let normalized_chunk = if let Some(normalizer) =
                            private_stream_normalizer.as_mut()
                        {
                            match normalizer.push_chunk(&chunk) {
                                Ok(normalized_chunk) => normalized_chunk,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_chunk_normalize_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to normalize execution runtime stream chunk"
                                    );
                                    terminal_failure = Some(build_stream_failure_report(
                                            "execution_runtime_stream_rewrite_error",
                                            format!("failed to normalize execution runtime stream chunk: {err:?}"),
                                            502,
                                        ));
                                    break;
                                }
                            }
                        } else {
                            chunk
                        };
                        let provider_private_error_body_json =
                            extract_provider_private_stream_error_body(
                                stream_usage_report_context.as_ref(),
                                &normalized_chunk,
                            );
                        if let (Some(observer), Some(report_context)) = (
                            stream_usage_observer.as_mut(),
                            stream_usage_report_context.as_ref(),
                        ) {
                            observe_stream_usage_bytes(
                                observer,
                                report_context,
                                &mut stream_usage_observer_buffered,
                                &normalized_chunk,
                            );
                        }
                        let rewritten_chunk = if let Some(rewriter) = local_stream_rewriter.as_mut()
                        {
                            match rewriter.push_chunk(&normalized_chunk) {
                                Ok(rewritten_chunk) => rewritten_chunk,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_chunk_rewrite_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to rewrite execution runtime stream chunk"
                                    );
                                    terminal_failure = Some(build_stream_failure_report(
                                        "execution_runtime_stream_rewrite_error",
                                        format!("failed to rewrite execution runtime stream chunk: {err:?}"),
                                        502,
                                    ));
                                    break;
                                }
                            }
                        } else {
                            normalized_chunk
                        };

                        if rewritten_chunk.is_empty() {
                            if let Some(error_body_json) = provider_private_error_body_json {
                                let error_status_code = resolve_local_sync_error_status_code(
                                    status_code,
                                    &error_body_json,
                                );
                                terminal_failure =
                                    Some(build_stream_failure_from_provider_error_body(
                                        error_status_code,
                                        &error_body_json,
                                    ));
                                break;
                            }
                            continue;
                        }

                        if usage_stream_telemetry
                            .as_ref()
                            .and_then(|telemetry| telemetry.ttfb_ms)
                            .is_none()
                        {
                            let first_data_elapsed_ms = stream_started_at_for_report
                                .elapsed()
                                .as_millis()
                                .min(u128::from(u64::MAX))
                                as u64;
                            let first_data_telemetry = ExecutionTelemetry {
                                ttfb_ms: Some(first_data_elapsed_ms),
                                elapsed_ms: Some(first_data_elapsed_ms),
                                upstream_bytes: telemetry
                                    .as_ref()
                                    .and_then(|telemetry| telemetry.upstream_bytes),
                            };
                            state_for_report.usage_runtime.record_stream_started(
                                state_for_report.data.as_ref(),
                                &lifecycle_seed_for_report,
                                status_code,
                                Some(&first_data_telemetry),
                            );
                            usage_stream_telemetry = Some(first_data_telemetry);
                        }

                        append_stream_capture_bytes(
                            &mut buffered_body,
                            &rewritten_chunk,
                            max_stream_body_buffer_bytes,
                            &mut client_body_truncated,
                        );
                        let rewritten_chunk_len =
                            u64::try_from(rewritten_chunk.len()).unwrap_or(u64::MAX);
                        let chunk_completed_stream =
                            stream_chunk_contains_sse_done(&rewritten_chunk);
                        if downstream_dropped {
                            continue;
                        }
                        if tx.send(Ok(Bytes::from(rewritten_chunk))).await.is_err() {
                            warn!(
                                event_name = "stream_execution_downstream_disconnected",
                                log_type = "ops",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                "gateway stream downstream dropped; continuing to drain execution runtime stream"
                            );
                            downstream_dropped = true;
                        } else {
                            client_visible_stream_completed |= chunk_completed_stream;
                            client_stream_bytes.fetch_add(rewritten_chunk_len, Ordering::Relaxed);
                            last_client_chunk_elapsed_ms.store(
                                stream_started_at_for_report
                                    .elapsed()
                                    .as_millis()
                                    .min(u128::from(u64::MAX))
                                    as u64,
                                Ordering::Relaxed,
                            );
                        }
                        if let Some(error_body_json) = provider_private_error_body_json {
                            let error_status_code =
                                resolve_local_sync_error_status_code(status_code, &error_body_json);
                            terminal_failure = Some(build_stream_failure_from_provider_error_body(
                                error_status_code,
                                &error_body_json,
                            ));
                            break;
                        }
                    }
                    StreamFramePayload::Telemetry {
                        telemetry: frame_telemetry,
                    } => {
                        let should_refresh_stream_usage = should_refresh_stream_usage_telemetry(
                            usage_stream_telemetry.as_ref(),
                            &frame_telemetry,
                        );
                        if should_refresh_stream_usage {
                            state_for_report.usage_runtime.record_stream_started(
                                state_for_report.data.as_ref(),
                                &lifecycle_seed_for_report,
                                status_code,
                                Some(&frame_telemetry),
                            );
                            usage_stream_telemetry = Some(frame_telemetry.clone());
                        }
                        telemetry = Some(frame_telemetry);
                    }
                    StreamFramePayload::Eof { summary } => {
                        if summary.is_some() {
                            stream_terminal_summary = summary;
                        }
                        break;
                    }
                    StreamFramePayload::Error { error } => {
                        warn!(
                            event_name = "stream_execution_error_frame",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = %error.message,
                            "execution runtime stream emitted error frame"
                        );
                        terminal_failure = Some(build_stream_failure_from_execution_error(&error));
                        break;
                    }
                    StreamFramePayload::Headers { .. } => {}
                }
            }
        }

        if downstream_dropped {
            debug!(
                event_name = "execution_runtime_stream_client_flush_skipped",
                log_type = "debug",
                debug_context = "redacted",
                stream_status = "downstream_disconnected",
                trace_id = %trace_id_owned,
                "gateway skipped client stream flush after downstream disconnect"
            );
        }
        if let Some(normalizer) = private_stream_normalizer.as_mut() {
            match normalizer.finish() {
                Ok(normalized_chunk) if !normalized_chunk.is_empty() => {
                    let provider_private_error_body_json =
                        extract_provider_private_stream_error_body(
                            stream_usage_report_context.as_ref(),
                            &normalized_chunk,
                        );
                    if let (Some(observer), Some(report_context)) = (
                        stream_usage_observer.as_mut(),
                        stream_usage_report_context.as_ref(),
                    ) {
                        observe_stream_usage_bytes(
                            observer,
                            report_context,
                            &mut stream_usage_observer_buffered,
                            &normalized_chunk,
                        );
                    }
                    if !downstream_dropped {
                        let rewritten_chunk = if let Some(rewriter) = local_stream_rewriter.as_mut()
                        {
                            match rewriter.push_chunk(&normalized_chunk) {
                                Ok(rewritten_chunk) => rewritten_chunk,
                                Err(err) => {
                                    warn!(
                                        event_name = "stream_execution_normalized_flush_rewrite_failed",
                                        log_type = "ops",
                                        trace_id = %trace_id_owned,
                                        request_id = %request_id_for_report_log,
                                        candidate_id = ?candidate_id_for_report.as_deref(),
                                        error = ?err,
                                        "gateway failed to rewrite normalized private stream chunk during flush"
                                    );
                                    terminal_failure.get_or_insert_with(|| {
                                        build_stream_failure_report(
                                            "execution_runtime_stream_rewrite_flush_error",
                                            format!("failed to rewrite normalized private stream chunk during flush: {err:?}"),
                                            502,
                                        )
                                    });
                                    Vec::new()
                                }
                            }
                        } else {
                            normalized_chunk
                        };
                        if !rewritten_chunk.is_empty() {
                            append_stream_capture_bytes(
                                &mut buffered_body,
                                &rewritten_chunk,
                                max_stream_body_buffer_bytes,
                                &mut client_body_truncated,
                            );
                            let rewritten_chunk_len =
                                u64::try_from(rewritten_chunk.len()).unwrap_or(u64::MAX);
                            let chunk_completed_stream =
                                stream_chunk_contains_sse_done(&rewritten_chunk);
                            if tx.send(Ok(Bytes::from(rewritten_chunk))).await.is_err() {
                                warn!(
                                    event_name = "stream_execution_downstream_flush_disconnected",
                                    log_type = "ops",
                                    trace_id = %trace_id_owned,
                                    request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                "gateway stream downstream dropped while flushing private stream normalization"
                                );
                                downstream_dropped = true;
                            } else {
                                client_visible_stream_completed |= chunk_completed_stream;
                                client_stream_bytes
                                    .fetch_add(rewritten_chunk_len, Ordering::Relaxed);
                                last_client_chunk_elapsed_ms.store(
                                    stream_started_at_for_report
                                        .elapsed()
                                        .as_millis()
                                        .min(u128::from(u64::MAX))
                                        as u64,
                                    Ordering::Relaxed,
                                );
                            }
                        }
                        if let Some(error_body_json) = provider_private_error_body_json {
                            let error_status_code =
                                resolve_local_sync_error_status_code(status_code, &error_body_json);
                            terminal_failure.get_or_insert_with(|| {
                                build_stream_failure_from_provider_error_body(
                                    error_status_code,
                                    &error_body_json,
                                )
                            });
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(
                        event_name = "stream_execution_normalization_flush_failed",
                        log_type = "ops",
                        trace_id = %trace_id_owned,
                        request_id = %request_id_for_report_log,
                        candidate_id = ?candidate_id_for_report.as_deref(),
                        error = ?err,
                        "gateway failed to flush private stream normalization"
                    );
                    terminal_failure.get_or_insert_with(|| {
                        build_stream_failure_report(
                            "execution_runtime_stream_rewrite_flush_error",
                            format!("failed to flush private stream normalization: {err:?}"),
                            502,
                        )
                    });
                }
            }
        }
        if !downstream_dropped {
            if let Some(rewriter) = local_stream_rewriter.as_mut() {
                match rewriter.finish() {
                    Ok(flushed_chunk) if !flushed_chunk.is_empty() => {
                        append_stream_capture_bytes(
                            &mut buffered_body,
                            &flushed_chunk,
                            max_stream_body_buffer_bytes,
                            &mut client_body_truncated,
                        );
                        let flushed_chunk_len =
                            u64::try_from(flushed_chunk.len()).unwrap_or(u64::MAX);
                        let chunk_completed_stream = stream_chunk_contains_sse_done(&flushed_chunk);
                        if tx.send(Ok(Bytes::from(flushed_chunk))).await.is_err() {
                            warn!(
                                event_name = "stream_execution_downstream_rewrite_flush_disconnected",
                                log_type = "ops",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            "gateway stream downstream dropped while flushing local stream rewrite"
                            );
                            downstream_dropped = true;
                        } else {
                            client_visible_stream_completed |= chunk_completed_stream;
                            client_stream_bytes.fetch_add(flushed_chunk_len, Ordering::Relaxed);
                            last_client_chunk_elapsed_ms.store(
                                stream_started_at_for_report
                                    .elapsed()
                                    .as_millis()
                                    .min(u128::from(u64::MAX))
                                    as u64,
                                Ordering::Relaxed,
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(err) => {
                        warn!(
                            event_name = "stream_execution_rewrite_flush_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                            "gateway failed to flush local stream rewrite"
                        );
                        terminal_failure.get_or_insert_with(|| {
                            build_stream_failure_report(
                                "execution_runtime_stream_rewrite_flush_error",
                                format!("failed to flush local stream rewrite: {err:?}"),
                                502,
                            )
                        });
                    }
                }
            }
        }

        if !downstream_dropped {
            if let Some(failure) = terminal_failure.as_ref() {
                let terminal_event = if is_openai_image_stream_for_report {
                    Some(encode_openai_image_failed_event(
                        report_context_owned.as_ref(),
                        failure,
                    ))
                } else if emit_passthrough_sse_terminal_error {
                    Some(encode_terminal_sse_error_event(failure))
                } else {
                    None
                };
                if let Some(terminal_event) = terminal_event {
                    match terminal_event {
                        Ok(error_event) => {
                            let error_event_len =
                                u64::try_from(error_event.len()).unwrap_or(u64::MAX);
                            append_stream_capture_bytes(
                                &mut buffered_body,
                                error_event.as_ref(),
                                max_stream_body_buffer_bytes,
                                &mut client_body_truncated,
                            );
                            if tx.send(Ok(error_event)).await.is_err() {
                                warn!(
                                event_name = "stream_execution_downstream_terminal_error_disconnected",
                                log_type = "ops",
                                trace_id = %trace_id_owned,
                                request_id = %request_id_for_report_log,
                                candidate_id = ?candidate_id_for_report.as_deref(),
                                "gateway stream downstream dropped while sending terminal SSE error event"
                                );
                                downstream_dropped = true;
                            } else {
                                client_stream_bytes.fetch_add(error_event_len, Ordering::Relaxed);
                                last_client_chunk_elapsed_ms.store(
                                    stream_started_at_for_report
                                        .elapsed()
                                        .as_millis()
                                        .min(u128::from(u64::MAX))
                                        as u64,
                                    Ordering::Relaxed,
                                );
                            }
                        }
                        Err(err) => {
                            warn!(
                            event_name = "stream_execution_terminal_error_event_encode_failed",
                            log_type = "ops",
                            trace_id = %trace_id_owned,
                            request_id = %request_id_for_report_log,
                            candidate_id = ?candidate_id_for_report.as_deref(),
                            error = ?err,
                                "gateway failed to encode terminal SSE error event"
                            );
                        }
                    }
                }
            }
        }

        drop(tx);
        idle_monitor_done.store(true, Ordering::Relaxed);
        idle_monitor_handle.abort();

        stream_terminal_summary = merge_stream_terminal_summary(
            stream_terminal_summary,
            finalize_stream_usage_observer(
                &mut stream_usage_observer,
                stream_usage_report_context.as_ref(),
                &mut stream_usage_observer_buffered,
            ),
        );

        if downstream_dropped && client_visible_stream_completed && terminal_failure.is_none() {
            debug!(
                event_name = "execution_runtime_stream_downstream_closed_after_done",
                log_type = "debug",
                trace_id = %trace_id_owned,
                request_id = %request_id_for_report_log,
                candidate_id = ?candidate_id_for_report.as_deref(),
                "gateway treats downstream close after client-visible SSE DONE as completed"
            );
            downstream_dropped = false;
        }

        if downstream_dropped {
            debug!(
                event_name = "execution_runtime_stream_report_skipped",
                log_type = "debug",
                debug_context = "redacted",
                stream_status = "downstream_disconnected",
                status_code = 499_u16,
                trace_id = %trace_id_owned,
                "gateway skipped stream report because downstream disconnected before completion"
            );
            let terminal_telemetry = Some(build_terminal_stream_telemetry(
                stream_started_at_for_report,
                telemetry.as_ref(),
                usage_stream_telemetry.as_ref(),
                provider_stream_bytes.load(Ordering::Relaxed),
            ));
            let usage_payload = build_stream_usage_payload(
                trace_id_owned,
                report_kind_owned.unwrap_or_default(),
                report_context_owned,
                499,
                headers_for_report,
                &provider_buffered_body,
                provider_body_truncated,
                &buffered_body,
                client_body_truncated,
                stream_terminal_summary,
                terminal_telemetry,
            );
            record_stream_terminal_usage(
                &state_for_report,
                &plan_for_report,
                usage_payload.report_context.as_ref(),
                &usage_payload,
                true,
            );
            record_local_request_candidate_status(
                &state_for_report,
                &plan_for_report,
                usage_payload.report_context.as_ref(),
                SchedulerRequestCandidateStatusUpdate {
                    status: RequestCandidateStatus::Cancelled,
                    status_code: Some(499),
                    error_type: Some("downstream_disconnect".to_string()),
                    error_message: Some("client disconnected before stream completion".to_string()),
                    latency_ms: usage_payload
                        .telemetry
                        .as_ref()
                        .and_then(|value| value.elapsed_ms),
                    started_at_unix_ms: Some(candidate_started_unix_secs_for_report),
                    finished_at_unix_ms: Some(current_request_candidate_unix_ms()),
                },
            )
            .await;
            return;
        }

        if let Some(failure) = terminal_failure {
            record_manual_proxy_stream_error(&state_for_report, &plan_for_report).await;
            let terminal_telemetry = Some(build_terminal_stream_telemetry(
                stream_started_at_for_report,
                telemetry.as_ref(),
                usage_stream_telemetry.as_ref(),
                provider_stream_bytes.load(Ordering::Relaxed),
            ));
            submit_midstream_stream_failure(
                &state_for_report,
                &trace_id_owned,
                &plan_for_report,
                direct_stream_finalize_kind_owned.as_deref(),
                report_context_owned,
                headers_for_report,
                terminal_telemetry,
                &provider_buffered_body,
                candidate_started_unix_secs_for_report,
                failure,
            )
            .await;
            return;
        }

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &plan_for_report,
            report_context_owned.as_ref(),
            &mut stream_terminal_summary,
        );
        let requires_observed_terminal_event = stream_requires_observed_terminal_event(
            plan_for_report.provider_api_format.as_str(),
            stream_usage_report_context.as_ref(),
        );
        ensure_stream_terminal_summary_for_missing_observed_finish(
            &mut stream_terminal_summary,
            requires_observed_terminal_event,
        );
        let missing_observed_finish =
            stream_terminal_summary_missing_observed_finish_with_requirement(
                stream_terminal_summary.as_ref(),
                requires_observed_terminal_event,
            );

        let should_submit_report = report_kind_owned.is_some();
        let terminal_telemetry = Some(build_terminal_stream_telemetry(
            stream_started_at_for_report,
            telemetry.as_ref(),
            usage_stream_telemetry.as_ref(),
            provider_stream_bytes.load(Ordering::Relaxed),
        ));
        let stream_failed = stream_terminal_summary_represents_failure_with_requirement(
            stream_terminal_summary.as_ref(),
            requires_observed_terminal_event,
        );
        let stream_terminal_error_message = stream_terminal_summary
            .as_ref()
            .and_then(|summary| summary.parser_error.clone())
            .or_else(|| {
                missing_observed_finish.then(|| {
                    "execution runtime stream ended before provider terminal event".to_string()
                })
            });
        let usage_payload = build_stream_usage_payload(
            trace_id_owned.clone(),
            report_kind_owned.unwrap_or_default(),
            report_context_owned,
            status_code,
            headers_for_report,
            &provider_buffered_body,
            provider_body_truncated,
            &buffered_body,
            client_body_truncated,
            stream_terminal_summary,
            terminal_telemetry,
        );
        if stream_failed {
            warn!(
                event_name = "execution_runtime_stream_missing_terminal_event",
                log_type = "ops",
                trace_id = %trace_id_owned,
                request_id = %request_id_for_report_log,
                candidate_id = ?candidate_id_for_report.as_deref(),
                status_code,
                error_message = stream_terminal_error_message.as_deref().unwrap_or_default(),
                "gateway stream ended with a failed terminal state"
            );
        } else {
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::HealthSuccess(LocalHealthSuccessEffect),
            )
            .await;
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::AdaptiveSuccess(LocalAdaptiveSuccessEffect),
            )
            .await;
            apply_local_execution_effect(
                &state_for_report,
                LocalExecutionEffectContext {
                    plan: &plan_for_report,
                    report_context: usage_payload.report_context.as_ref(),
                },
                LocalExecutionEffect::PoolSuccessStream {
                    payload: &usage_payload,
                },
            )
            .await;
        }
        record_stream_terminal_usage(
            &state_for_report,
            &plan_for_report,
            usage_payload.report_context.as_ref(),
            &usage_payload,
            false,
        );
        record_local_request_candidate_status(
            &state_for_report,
            &plan_for_report,
            usage_payload.report_context.as_ref(),
            SchedulerRequestCandidateStatusUpdate {
                status: if stream_failed {
                    RequestCandidateStatus::Failed
                } else {
                    RequestCandidateStatus::Success
                },
                status_code: Some(status_code),
                error_type: if stream_failed {
                    if missing_observed_finish {
                        Some("stream_missing_terminal_event".to_string())
                    } else {
                        Some("stream_terminal_error".to_string())
                    }
                } else {
                    None
                },
                error_message: stream_failed
                    .then_some(stream_terminal_error_message)
                    .flatten(),
                latency_ms: usage_payload
                    .telemetry
                    .as_ref()
                    .and_then(|value| value.elapsed_ms),
                started_at_unix_ms: Some(candidate_started_unix_secs_for_report),
                finished_at_unix_ms: Some(current_request_candidate_unix_ms()),
            },
        )
        .await;

        if should_submit_report {
            if let Err(err) = submit_stream_report(&state_for_report, usage_payload).await {
                warn!(
                    event_name = "execution_report_submit_failed",
                    log_type = "ops",
                    trace_id = %trace_id_owned,
                    request_id = %request_id_for_report_log,
                    candidate_id = ?candidate_id_for_report.as_deref(),
                    report_scope = "stream",
                    error = ?err,
                    "gateway failed to submit stream execution report"
                );
            }
        }
    });

    headers.insert(CONTROL_REQUEST_ID_HEADER.to_string(), request_id.clone());

    if let Some(candidate_id) = candidate_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.insert(
            CONTROL_CANDIDATE_ID_HEADER.to_string(),
            candidate_id.to_string(),
        );
    }

    let emit_sse_keepalive = response_headers_indicate_sse(&headers);
    if emit_sse_keepalive {
        headers.remove("content-length");
    }
    let body_stream = build_sse_body_stream(
        prefetched_chunks_for_body,
        rx,
        emit_sse_keepalive,
        SSE_KEEPALIVE_INTERVAL,
    );

    Ok(Some(build_client_response_from_parts(
        status_code,
        &headers,
        Body::from_stream(body_stream),
        trace_id,
        Some(decision),
    )?))
}

fn apply_stream_summary_report_context(
    execution: &mut DirectUpstreamStreamExecution,
    report_context: Option<&Value>,
) {
    if let Some(report_context) = report_context.cloned() {
        execution.stream_summary_report_context = report_context;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::convert::Infallible;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use aether_contracts::{
        ExecutionPlan, ExecutionStreamTerminalSummary, ExecutionTimeouts, RequestBody,
        StandardizedUsage,
    };
    use aether_data::repository::candidates::InMemoryRequestCandidateRepository;
    use aether_data::repository::usage::InMemoryUsageReadRepository;
    use aether_data_contracts::repository::candidates::{
        RequestCandidateReadRepository, RequestCandidateStatus,
    };
    use aether_data_contracts::repository::usage::UsageReadRepository;
    use aether_usage_runtime::UsageRuntimeConfig;
    use async_stream::stream;
    use axum::body::{to_bytes, Body, Bytes};
    use axum::extract::ws::Message;
    use axum::extract::Request;
    use axum::routing::any;
    use axum::{http::header, http::HeaderValue, Router};
    use base64::Engine as _;
    use futures_util::StreamExt as _;
    use serde_json::{json, Value};
    use tokio::sync::{mpsc, watch, Notify};

    use super::{
        build_sse_body_stream, ensure_stream_terminal_summary_for_missing_observed_finish,
        execute_execution_runtime_stream, execute_stream_from_frame_stream,
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary, merge_stream_terminal_summary,
        should_limit_direct_finalize_prefetch, should_probe_success_failover_before_stream,
        should_skip_direct_finalize_prefetch, stream_chunk_contains_sse_done,
        stream_requires_observed_terminal_event, stream_terminal_summary_missing_observed_finish,
        stream_terminal_summary_missing_observed_finish_with_requirement,
        stream_terminal_summary_represents_failure_with_requirement,
    };
    use crate::control::GatewayControlDecision;
    use crate::tunnel::{tunnel_protocol, TunnelProxyConn};
    use crate::AppState;

    fn test_decision() -> GatewayControlDecision {
        GatewayControlDecision::synthetic(
            "/v1/chat/completions",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
        )
        .with_execution_runtime_candidate(true)
    }

    #[test]
    fn detects_client_visible_sse_terminal_events() {
        assert!(stream_chunk_contains_sse_done(b"data: [DONE]\n\n"));
        assert!(stream_chunk_contains_sse_done(
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        ));
        assert!(stream_chunk_contains_sse_done(
            b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{}}\n\n"
        ));
        assert!(stream_chunk_contains_sse_done(
            b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n"
        ));
        assert!(!stream_chunk_contains_sse_done(
            b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\"}\n\n"
        ));
    }

    fn tunnel_proxy_snapshot(base_url: String) -> aether_contracts::ProxySnapshot {
        aether_contracts::ProxySnapshot {
            enabled: Some(true),
            mode: Some("tunnel".into()),
            node_id: Some("node-1".into()),
            label: Some("relay-node".into()),
            url: None,
            extra: Some(json!({"tunnel_base_url": base_url})),
        }
    }

    fn connect_json_frame(flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + payload.len());
        out.push(flags);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn merge_stream_terminal_summary_prefers_more_complete_observed_usage() {
        let mut runtime_usage = StandardizedUsage::new();
        runtime_usage.output_tokens = 137;
        let mut observed_usage = StandardizedUsage::new();
        observed_usage.input_tokens = 26;
        observed_usage.output_tokens = 137;

        let merged = merge_stream_terminal_summary(
            Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(runtime_usage),
                model: Some("gpt-5.5".to_string()),
                unknown_event_count: 1,
                ..ExecutionStreamTerminalSummary::default()
            }),
            Some(ExecutionStreamTerminalSummary {
                standardized_usage: Some(observed_usage),
                response_id: Some("resp_123".to_string()),
                observed_finish: true,
                unknown_event_count: 2,
                ..ExecutionStreamTerminalSummary::default()
            }),
        )
        .expect("summary should merge");
        let usage = merged
            .standardized_usage
            .expect("merged usage should exist");

        assert_eq!(usage.input_tokens, 26);
        assert_eq!(usage.output_tokens, 137);
        assert_eq!(merged.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(merged.response_id.as_deref(), Some("resp_123"));
        assert!(merged.observed_finish);
        assert_eq!(merged.unknown_event_count, 3);
    }

    #[test]
    fn detects_missing_observed_finish_only_without_usage_signal() {
        assert!(stream_terminal_summary_missing_observed_finish(Some(
            &ExecutionStreamTerminalSummary {
                response_id: Some("resp_missing_finish".to_string()),
                model: Some("gpt-5.5".to_string()),
                observed_finish: false,
                ..ExecutionStreamTerminalSummary::default()
            }
        )));

        let mut usage = StandardizedUsage::new();
        usage.output_tokens = 12;
        assert!(!stream_terminal_summary_missing_observed_finish(Some(
            &ExecutionStreamTerminalSummary {
                standardized_usage: Some(usage),
                observed_finish: false,
                ..ExecutionStreamTerminalSummary::default()
            }
        )));
        assert!(!stream_terminal_summary_missing_observed_finish(Some(
            &ExecutionStreamTerminalSummary {
                observed_finish: true,
                ..ExecutionStreamTerminalSummary::default()
            }
        )));
        assert!(!stream_terminal_summary_missing_observed_finish(None));
    }

    #[test]
    fn requires_terminal_event_for_openai_responses_streams() {
        assert!(stream_requires_observed_terminal_event(
            "openai:responses",
            None
        ));
        assert!(stream_requires_observed_terminal_event(
            "openai:responses:compact",
            None
        ));
        assert!(!stream_requires_observed_terminal_event(
            "openai:chat",
            None
        ));
        assert!(stream_requires_observed_terminal_event(
            "openai:chat",
            Some(&json!({
                "provider_stream_event_api_format": "openai:responses"
            }))
        ));
    }

    #[test]
    fn synthesizes_missing_terminal_summary_for_openai_responses_empty_stream() {
        let mut summary = None;
        ensure_stream_terminal_summary_for_missing_observed_finish(&mut summary, true);

        let summary = summary.expect("summary should be synthesized");
        assert!(!summary.observed_finish);
        assert_eq!(
            summary.parser_error.as_deref(),
            Some("execution runtime stream ended before provider terminal event")
        );
        assert!(
            stream_terminal_summary_missing_observed_finish_with_requirement(Some(&summary), true)
        );
        assert!(stream_terminal_summary_represents_failure_with_requirement(
            Some(&summary),
            true
        ));
    }

    #[test]
    fn terminal_required_stream_fails_even_with_usage_without_finish() {
        let mut usage = StandardizedUsage::new();
        usage.output_tokens = 12;
        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(usage),
            observed_finish: false,
            ..ExecutionStreamTerminalSummary::default()
        });

        ensure_stream_terminal_summary_for_missing_observed_finish(&mut summary, true);
        let summary = summary.as_ref().expect("summary should remain present");
        assert!(
            stream_terminal_summary_missing_observed_finish_with_requirement(Some(summary), true)
        );
        assert!(stream_terminal_summary_represents_failure_with_requirement(
            Some(summary),
            true
        ));
    }

    #[test]
    fn kiro_stream_summary_applies_prompt_cache_usage_from_original_request() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "cacheable system ".repeat(600),
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "cacheable prompt ".repeat(1200),
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-cache-stream".into(),
            candidate_id: Some("cand-kiro-cache-stream".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-cache-stream".into(),
            endpoint_id: "endpoint-kiro-cache-stream".into(),
            key_id: "key-kiro-cache-stream".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        let mut first_summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 6_000,
                output_tokens: 17,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &plan,
            Some(&report_context),
            &mut first_summary,
        );
        let first_usage = first_summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("first usage should exist");
        assert!(first_usage.cache_creation_tokens > 0);
        assert_eq!(first_usage.cache_read_tokens, 0);

        let mut second_summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 6_000,
                output_tokens: 19,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });
        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &plan,
            Some(&report_context),
            &mut second_summary,
        );
        let second_usage = second_summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("second usage should exist");
        assert!(second_usage.cache_read_tokens > 0);
        assert_eq!(second_usage.cache_creation_tokens, 0);
        assert!(second_usage.input_tokens < 6_000);
        assert_eq!(second_usage.output_tokens, 19);
    }

    #[test]
    fn kiro_stream_summary_seeds_input_tokens_without_cache_control() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "non cacheable system ".repeat(400)
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "non cacheable prompt ".repeat(800)
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-non-cache".into(),
            candidate_id: Some("cand-kiro-non-cache".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-non-cache".into(),
            endpoint_id: "endpoint-kiro-non-cache".into(),
            key_id: "key-kiro-non-cache".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 0,
                output_tokens: 13,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &plan,
            Some(&report_context),
            &mut summary,
        );

        let usage = summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("usage should exist");

        assert!(usage.input_tokens > 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.output_tokens, 13);
    }

    #[test]
    fn kiro_stream_summary_bills_existing_cache_usage_when_input_is_zero() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "cached system ".repeat(800)
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "cached prompt ".repeat(1400)
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-existing-cache".into(),
            candidate_id: Some("cand-kiro-existing-cache".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-existing-cache".into(),
            endpoint_id: "endpoint-kiro-existing-cache".into(),
            key_id: "key-kiro-existing-cache".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 0,
                output_tokens: 23,
                cache_read_tokens: 200,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &plan,
            Some(&report_context),
            &mut summary,
        );

        let usage = summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("usage should exist");

        assert!(usage.input_tokens > 0);
        assert_eq!(usage.cache_read_tokens, 200);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.output_tokens, 23);
    }

    #[test]
    fn kiro_stream_summary_clears_cache_usage_when_simulated_cache_disabled() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "disabled cache summary system ".repeat(800),
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "disabled cache summary prompt ".repeat(1400),
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-summary-cache-disabled".into(),
            candidate_id: Some("cand-kiro-summary-cache-disabled".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-summary-cache-disabled".into(),
            endpoint_id: "endpoint-kiro-summary-cache-disabled".into(),
            key_id: "key-kiro-summary-cache-disabled".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 0,
                output_tokens: 23,
                cache_creation_tokens: 500,
                cache_read_tokens: 700,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &plan,
            Some(&report_context),
            &mut summary,
        );

        let usage = summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("usage should exist");

        assert!(usage.input_tokens > 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.output_tokens, 23);
    }

    #[test]
    fn kiro_stream_summary_does_not_subtract_cache_from_already_billed_input() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "cached history ".repeat(400),
                            "cache_control": {"type": "ephemeral"}
                        },
                        {
                            "type": "text",
                            "text": "new user turn"
                        }
                    ]
                }
            ]
        });
        let report_context = json!({
            "original_request_body": request_body,
            "input_tokens": 24_770,
            "cache_creation_input_tokens": 175,
            "cache_read_input_tokens": 24_463,
            "kiro_simulated_cache_enabled": true
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-billed-input".into(),
            candidate_id: Some("cand-kiro-billed-input".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-billed-input".into(),
            endpoint_id: "endpoint-kiro-billed-input".into(),
            key_id: "key-kiro-billed-input".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };

        let mut summary = Some(ExecutionStreamTerminalSummary {
            standardized_usage: Some(StandardizedUsage {
                input_tokens: 132,
                output_tokens: 167,
                cache_creation_tokens: 175,
                cache_read_tokens: 24_463,
                ..StandardizedUsage::new()
            }),
            ..ExecutionStreamTerminalSummary::default()
        });

        maybe_apply_kiro_prompt_cache_usage_to_stream_summary(
            &plan,
            Some(&report_context),
            &mut summary,
        );

        let usage = summary
            .as_ref()
            .and_then(|summary| summary.standardized_usage.as_ref())
            .expect("usage should exist");

        assert_eq!(usage.input_tokens, 132);
        assert_eq!(usage.cache_creation_tokens, 175);
        assert_eq!(usage.cache_read_tokens, 24_463);
        assert_eq!(usage.output_tokens, 167);
    }

    #[tokio::test]
    async fn kiro_report_context_seeds_input_tokens_from_original_request_body() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "seeded system ".repeat(600)
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "seeded prompt ".repeat(1200)
                        }
                    ]
                }
            ]
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-seed".into(),
            candidate_id: Some("cand-kiro-seed".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-seed".into(),
            endpoint_id: "endpoint-kiro-seed".into(),
            key_id: "key-kiro-seed".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut report_context = Some(json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        }));

        super::seed_kiro_report_context_input_tokens(&plan, &mut report_context);

        let input_tokens = report_context
            .as_ref()
            .and_then(|context| context.get("input_tokens"))
            .and_then(Value::as_u64)
            .expect("kiro input tokens should be seeded");
        assert!(input_tokens > 0);
    }

    #[tokio::test]
    async fn kiro_report_context_seeds_prompt_cache_usage_before_stream_rewrite() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "cache seed system ".repeat(600),
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "cache seed prompt ".repeat(1200),
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]
                }
            ]
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-cache-seed".into(),
            candidate_id: Some("cand-kiro-cache-seed".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-cache-seed".into(),
            endpoint_id: "endpoint-kiro-cache-seed".into(),
            key_id: "key-kiro-cache-seed".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut report_context = Some(json!({
            "original_request_body": request_body,
            "kiro_simulated_cache_enabled": true,
        }));

        super::seed_kiro_report_context_input_tokens(&plan, &mut report_context);
        super::seed_kiro_report_context_prompt_cache_usage(&plan, &mut report_context);

        let context = report_context.as_ref().expect("context should exist");
        assert!(context
            .get("input_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0));
        assert!(context
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0));
        assert_eq!(
            context
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64),
            Some(0)
        );
    }

    #[tokio::test]
    async fn kiro_report_context_skips_prompt_cache_usage_when_disabled() {
        let request_body = json!({
            "model": "claude-opus-4-7",
            "system": [
                {
                    "type": "text",
                    "text": "disabled cache system ".repeat(600),
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "disabled cache prompt ".repeat(1200),
                            "cache_control": {"type": "ephemeral"}
                        }
                    ]
                }
            ]
        });
        let plan = ExecutionPlan {
            request_id: "req-kiro-cache-disabled".into(),
            candidate_id: Some("cand-kiro-cache-disabled".into()),
            provider_name: Some("Kiro".into()),
            provider_id: "provider-kiro-cache-disabled".into(),
            endpoint_id: "endpoint-kiro-cache-disabled".into(),
            key_id: "key-kiro-cache-disabled".into(),
            method: "POST".into(),
            url: "https://q.us-east-1.amazonaws.com/generateAssistantResponse?beta=true".into(),
            headers: BTreeMap::new(),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"conversationState": {}})),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "claude:messages".into(),
            model_name: Some("claude-opus-4-7".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let mut report_context = Some(json!({
            "original_request_body": request_body,
        }));

        super::seed_kiro_report_context_input_tokens(&plan, &mut report_context);
        super::seed_kiro_report_context_prompt_cache_usage(&plan, &mut report_context);

        let context = report_context.as_ref().expect("context should exist");
        assert!(context
            .get("input_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|value| value > 0));
        assert_eq!(context.get("cache_creation_input_tokens"), None);
        assert_eq!(context.get("cache_read_input_tokens"), None);
    }

    #[test]
    fn skips_prefetch_for_same_format_passthrough_event_streams() {
        assert!(should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            Some("text/event-stream"),
            "claude:messages",
            "claude:messages",
            false,
            false,
        ));
    }

    #[test]
    fn skips_prefetch_for_same_format_passthrough_streams_without_content_type() {
        assert!(should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            None,
            "claude:messages",
            "claude:messages",
            false,
            false,
        ));
    }

    #[test]
    fn keeps_prefetch_for_same_format_json_streams() {
        assert!(!should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            Some("application/json"),
            "claude:messages",
            "claude:messages",
            false,
            false,
        ));
    }

    #[test]
    fn skips_prefetch_for_event_streams_even_when_cross_format_or_rewritten() {
        assert!(should_skip_direct_finalize_prefetch(
            Some("claude_cli_sync_finalize"),
            Some("text/event-stream"),
            "openai:chat",
            "claude:messages",
            false,
            true,
        ));
    }

    #[test]
    fn skips_success_failover_probe_for_event_streams() {
        assert!(!should_probe_success_failover_before_stream(
            &BTreeMap::from([(
                "content-type".to_string(),
                "text/event-stream; charset=utf-8".to_string(),
            )])
        ));
        assert!(should_probe_success_failover_before_stream(
            &BTreeMap::from([("content-type".to_string(), "application/json".to_string(),)])
        ));
    }

    #[test]
    fn limits_prefetch_for_openai_image_and_rewritten_streams() {
        assert!(should_limit_direct_finalize_prefetch(
            "openai_image_stream",
            false
        ));
        assert!(should_limit_direct_finalize_prefetch(
            "openai_chat_stream",
            true
        ));
        assert!(!should_limit_direct_finalize_prefetch(
            "openai_chat_stream",
            false
        ));
    }

    #[tokio::test]
    async fn sse_body_stream_emits_initial_and_periodic_keepalive_without_business_chunks() {
        let (_tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            Vec::new(),
            rx,
            true,
            Duration::from_millis(10),
        ));

        let first = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("initial keepalive should be immediate")
            .expect("stream should yield initial keepalive")
            .expect("initial keepalive should be ok");
        assert_eq!(first.as_ref(), b": aether-keepalive\n\n");

        let second = tokio::time::timeout(Duration::from_millis(100), body_stream.next())
            .await
            .expect("periodic keepalive should arrive")
            .expect("stream should yield periodic keepalive")
            .expect("periodic keepalive should be ok");
        assert_eq!(second.as_ref(), b": aether-keepalive\n\n");
    }

    #[tokio::test]
    async fn sse_body_stream_drops_upstream_control_only_blocks() {
        let (_tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            vec![
                Bytes::from_static(b": upstream-keepalive\n\n"),
                Bytes::from_static(b"event: ping\nid: 1\nretry: 1000\n\n"),
                Bytes::from_static(
                    b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
                ),
            ],
            rx,
            true,
            Duration::from_secs(60),
        ));

        let chunk = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("business chunk should arrive")
            .expect("stream should yield business chunk")
            .expect("business chunk should be ok");
        let text = std::str::from_utf8(chunk.as_ref()).expect("chunk should be utf8");
        assert!(text.contains("event: response.output_text.delta"));
        assert!(text.contains("data: {\"type\":\"response.output_text.delta\""));
        assert!(!text.contains("upstream-keepalive"));
        assert!(!text.contains("event: ping"));
        assert!(!text.contains("retry: 1000"));
    }

    #[tokio::test]
    async fn sse_body_stream_filters_control_blocks_across_chunk_boundaries() {
        let (_tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            vec![
                Bytes::from_static(b": upstream-keepalive\n"),
                Bytes::from_static(b"\n"),
                Bytes::from_static(b"event: response.created\n"),
                Bytes::from_static(b"data: {\"type\":\"response.created\"}\n\n"),
            ],
            rx,
            true,
            Duration::from_secs(60),
        ));

        let chunk = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("business chunk should arrive")
            .expect("stream should yield business chunk")
            .expect("business chunk should be ok");
        let text = std::str::from_utf8(chunk.as_ref()).expect("chunk should be utf8");
        assert_eq!(
            text,
            "event: response.created\ndata: {\"type\":\"response.created\"}\n\n"
        );
    }

    #[tokio::test]
    async fn sse_body_stream_forwards_data_line_before_block_boundary() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(4);
        let mut body_stream = Box::pin(build_sse_body_stream(
            Vec::new(),
            rx,
            true,
            Duration::from_secs(60),
        ));

        let keepalive = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("initial keepalive should be immediate")
            .expect("stream should yield initial keepalive")
            .expect("initial keepalive should be ok");
        assert_eq!(keepalive.as_ref(), b": aether-keepalive\n\n");

        tx.send(Ok(Bytes::from_static(
            b"event: response.output_text.delta\n",
        )))
        .await
        .expect("event line should send");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), body_stream.next())
                .await
                .is_err(),
            "event-only partial block should remain buffered"
        );

        tx.send(Ok(Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n",
        )))
        .await
        .expect("data line should send");
        let data_chunk = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("data-bearing block should stream before terminator")
            .expect("stream should yield data-bearing block")
            .expect("data-bearing block should be ok");
        assert_eq!(
            data_chunk.as_ref(),
            b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n"
        );

        tx.send(Ok(Bytes::from_static(b"\n")))
            .await
            .expect("terminator should send");
        let terminator = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("terminator should stream")
            .expect("stream should yield terminator")
            .expect("terminator should be ok");
        assert_eq!(terminator.as_ref(), b"\n");
    }

    #[tokio::test]
    async fn sse_body_stream_uses_local_keepalive_when_prefetched_blocks_are_control_only() {
        let (_tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
        let mut body_stream = Box::pin(build_sse_body_stream(
            vec![Bytes::from_static(b": upstream-keepalive\n\n")],
            rx,
            true,
            Duration::from_secs(60),
        ));

        let first = tokio::time::timeout(Duration::from_millis(50), body_stream.next())
            .await
            .expect("local keepalive should arrive")
            .expect("stream should yield local keepalive")
            .expect("local keepalive should be ok");
        assert_eq!(first.as_ref(), b": aether-keepalive\n\n");
    }

    #[tokio::test]
    async fn openai_image_stream_total_timeout_emits_image_failed_event() {
        let state = AppState::new().expect("app state should build");
        let plan = ExecutionPlan {
            request_id: "req-image-stream-timeout".into(),
            candidate_id: Some("cand-image-stream-timeout".into()),
            provider_name: Some("codex".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-image-1",
                "prompt": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:image".into(),
            provider_api_format: "openai:image".into(),
            model_name: Some("gpt-image-1".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                total_ms: Some(25),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/images/generations",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("image".to_string()),
            Some("openai:image".to_string()),
        )
        .with_execution_runtime_candidate(true);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
            ));
            std::future::pending::<()>().await;
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-image-stream-timeout",
            &decision,
            "openai_image_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:image",
                "client_api_format": "openai:image",
                "image_request": {
                    "operation": "generate"
                }
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let body = tokio::time::timeout(
            Duration::from_secs(2),
            to_bytes(response.into_body(), usize::MAX),
        )
        .await
        .expect("timeout failure should close the response body")
        .expect("response body should read");
        let text = String::from_utf8(body.to_vec()).expect("response body should be utf8");
        assert!(text.contains(": aether-keepalive\n\n"));
        assert!(text.contains("event: image_generation.failed"));
        assert!(text.contains("\"type\":\"image_stream_total_timeout\""));
    }

    #[tokio::test]
    async fn execute_stream_from_frame_stream_treats_windsurf_connect_trailer_error_as_failure() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
        let plan = ExecutionPlan {
            request_id: "req-windsurf-connect-error".into(),
            candidate_id: Some("cand-windsurf-connect-error".into()),
            provider_name: Some("windsurf".into()),
            provider_id: "provider-windsurf".into(),
            endpoint_id: "endpoint-windsurf-chat".into(),
            key_id: "key-windsurf".into(),
            method: "POST".into(),
            url: "https://server.codeium.com/exa.api_server_pb.ApiServerService/GetChatMessage?beta=true".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/connect+json".into()),
                ("accept".into(), "application/connect+json".into()),
            ]),
            content_type: Some("application/connect+json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "claude-sonnet-4",
                "messages": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("claude-sonnet-4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let trailer_error = connect_json_frame(
            2,
            br#"{"error":{"code":"resource_exhausted","message":"an internal error occurred"}}"#,
        );
        let trailer_error_b64 = base64::engine::general_purpose::STANDARD.encode(trailer_error);
        let frame = format!(
            "{{\"type\":\"data\",\"payload\":{{\"kind\":\"data\",\"chunk_b64\":\"{trailer_error_b64}\"}}}}\n"
        );
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"application/connect+json\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from(frame));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n",
            ));
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-windsurf-connect-error",
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": "req-windsurf-connect-error",
                "candidate_id": "cand-windsurf-connect-error",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:chat",
                "client_api_format": "claude:messages",
                "needs_conversion": true,
                "has_envelope": true,
                "envelope_name": "windsurf:GetChatMessage",
                "local_failover_policy": {
                    "stop_status_codes": [429]
                }
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_json: Value =
            serde_json::from_slice(&body).expect("response body should decode as json");
        assert_eq!(status.as_u16(), 429);
        assert_eq!(body_json["type"], json!("error"));
        assert_eq!(body_json["error"]["type"], json!("rate_limit_error"));
        assert_eq!(body_json["error"]["code"], json!("resource_exhausted"));
        assert_eq!(
            body_json["error"]["message"],
            json!("an internal error occurred")
        );

        let candidates = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = request_candidate_repository
                    .list_by_request_id("req-windsurf-connect-error")
                    .await
                    .expect("request candidates should read");
                if candidates
                    .first()
                    .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Failed)
                {
                    break candidates;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("candidate should be marked failed");
        assert_eq!(candidates[0].status_code, Some(429));
        assert_eq!(
            candidates[0].error_type.as_deref(),
            Some("resource_exhausted")
        );
    }

    #[tokio::test]
    async fn execute_stream_from_frame_stream_decodes_non_success_windsurf_connect_error_body() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
        let plan = ExecutionPlan {
            request_id: "req-windsurf-connect-429".into(),
            candidate_id: Some("cand-windsurf-connect-429".into()),
            provider_name: Some("windsurf".into()),
            provider_id: "provider-windsurf".into(),
            endpoint_id: "endpoint-windsurf-chat".into(),
            key_id: "key-windsurf".into(),
            method: "POST".into(),
            url: "https://server.codeium.com/exa.api_server_pb.ApiServerService/GetChatMessage?beta=true".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/connect+json".into()),
                ("accept".into(), "application/connect+json".into()),
            ]),
            content_type: Some("application/connect+json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "claude-sonnet-4",
                "messages": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "claude:messages".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("claude-sonnet-4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let connect_error = connect_json_frame(
            2,
            br#"{"error":{"code":"resource_exhausted","message":"quota exhausted"}}"#,
        );
        let connect_error_b64 = base64::engine::general_purpose::STANDARD.encode(connect_error);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":429,\"headers\":{\"content-type\":\"application/connect+json\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from(format!(
                "{{\"type\":\"data\",\"payload\":{{\"kind\":\"data\",\"chunk_b64\":\"{connect_error_b64}\"}}}}\n"
            )));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n",
            ));
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-windsurf-connect-429",
            &test_decision(),
            "claude_chat_stream",
            Some("claude_chat_stream_success".to_string()),
            Some(json!({
                "request_id": "req-windsurf-connect-429",
                "candidate_id": "cand-windsurf-connect-429",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:chat",
                "client_api_format": "claude:messages",
                "needs_conversion": true,
                "has_envelope": true,
                "envelope_name": "windsurf:GetChatMessage",
                "local_failover_policy": {
                    "stop_status_codes": [429]
                }
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        assert_eq!(response.status().as_u16(), 429);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_json: Value =
            serde_json::from_slice(&body).expect("response body should decode as json");
        assert_eq!(body_json["type"], json!("error"));
        assert_eq!(body_json["error"]["type"], json!("rate_limit_error"));
        assert_eq!(body_json["error"]["code"], json!("resource_exhausted"));
        assert_eq!(body_json["error"]["message"], json!("quota exhausted"));

        let record = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(usage) = usage_repository
                    .find_by_request_id("req-windsurf-connect-429")
                    .await
                    .expect("usage should read")
                    .filter(|usage| usage.status == "failed")
                {
                    break usage;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be written");
        assert_eq!(record.status_code, Some(429));
        assert_eq!(
            record
                .response_body
                .as_ref()
                .and_then(|body| body.get("error"))
                .and_then(|error| error.get("code")),
            Some(&json!("resource_exhausted"))
        );
        assert!(record.response_body_ref.is_none());
    }

    #[tokio::test]
    async fn execute_stream_from_frame_stream_drains_upstream_when_client_drops_body() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
        let plan = ExecutionPlan {
            request_id: "req-client-drop-cancels-upstream".into(),
            candidate_id: Some("cand-client-drop-cancels-upstream".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/v1/chat/completions".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "messages": [],
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let release_terminal = Arc::new(Notify::new());
        let terminal_frame_drained = Arc::new(Notify::new());
        let release_terminal_for_stream = Arc::clone(&release_terminal);
        let terminal_frame_drained_for_stream = Arc::clone(&terminal_frame_drained);
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"first\\\"}\\n\\n\"}}\n",
            ));
            release_terminal_for_stream.notified().await;
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"data: {\\\"id\\\":\\\"terminal\\\",\\\"object\\\":\\\"chat.completion.chunk\\\",\\\"model\\\":\\\"gpt-5.4\\\",\\\"choices\\\":[{\\\"index\\\":0,\\\"delta\\\":{},\\\"finish_reason\\\":\\\"stop\\\"}],\\\"usage\\\":{\\\"prompt_tokens\\\":7,\\\"completion_tokens\\\":11,\\\"total_tokens\\\":18}}\\n\\ndata: [DONE]\\n\\n\"}}\n",
            ));
            terminal_frame_drained_for_stream.notify_one();
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-client-drop-cancels-upstream",
            &test_decision(),
            "openai_chat_stream",
            None,
            Some(json!({
                "request_id": "req-client-drop-cancels-upstream",
                "candidate_id": "cand-client-drop-cancels-upstream",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:chat",
                "client_api_format": "openai:chat"
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let mut body_stream = response.into_body().into_data_stream();
        let first = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let chunk = body_stream
                    .next()
                    .await
                    .expect("body should yield first chunk")
                    .expect("first chunk should be ok");
                if chunk.as_ref() != b": aether-keepalive\n\n" {
                    break chunk;
                }
            }
        })
        .await
        .expect("first business chunk should arrive");
        assert_eq!(first.as_ref(), b"data: {\"id\":\"first\"}\n\n");
        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(body_stream);
        release_terminal.notify_one();

        tokio::time::timeout(Duration::from_secs(1), terminal_frame_drained.notified())
            .await
            .expect("upstream frame stream should be drained after client disconnect");
        let candidates = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = request_candidate_repository
                    .list_by_request_id("req-client-drop-cancels-upstream")
                    .await
                    .expect("request candidates should read");
                if candidates
                    .first()
                    .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Cancelled)
                {
                    break candidates;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("candidate should be marked cancelled");
        assert_eq!(candidates[0].status_code, Some(499));
        assert_eq!(
            candidates[0].error_type.as_deref(),
            Some("downstream_disconnect")
        );

        let stored_usage = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let usage = usage_repository
                    .find_by_request_id("req-client-drop-cancels-upstream")
                    .await
                    .expect("usage should read");
                if usage
                    .as_ref()
                    .is_some_and(|usage| usage.status == "cancelled")
                {
                    break usage.expect("cancelled usage should exist");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be marked cancelled");
        assert_eq!(stored_usage.billing_status, "pending");
        assert_eq!(stored_usage.status_code, Some(499));
        assert_eq!(stored_usage.input_tokens, 7);
        assert_eq!(stored_usage.output_tokens, 11);
        assert_eq!(stored_usage.total_tokens, 18);
        let first_byte_time_ms = stored_usage
            .first_byte_time_ms
            .expect("cancelled stream should retain first byte time");
        let response_time_ms = stored_usage
            .response_time_ms
            .expect("cancelled stream should record terminal duration");
        assert!(
            response_time_ms > first_byte_time_ms,
            "terminal duration should include time after the first byte"
        );
    }

    #[tokio::test]
    async fn image_stream_downstream_close_after_done_is_recorded_success() {
        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            });
        let plan = ExecutionPlan {
            request_id: "req-image-done-close-success".into(),
            candidate_id: Some("cand-image-done-close-success".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/v1/images/generations".into(),
            headers: BTreeMap::from([("accept".into(), "text/event-stream".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-image-2",
                "prompt": "draw a small image",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:image".into(),
            model_name: Some("gpt-image-2".into()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        };
        let frame_stream = stream! {
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
            ));
            yield Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"event: response.output_item.done\\ndata: {\\\"type\\\":\\\"response.output_item.done\\\",\\\"output_index\\\":0,\\\"item\\\":{\\\"id\\\":\\\"ig_1\\\",\\\"type\\\":\\\"image_generation_call\\\",\\\"result\\\":\\\"aGVsbG8=\\\"}}\\n\\nevent: response.completed\\ndata: {\\\"type\\\":\\\"response.completed\\\",\\\"response\\\":{\\\"id\\\":\\\"resp_1\\\",\\\"model\\\":\\\"gpt-image-2\\\",\\\"status\\\":\\\"completed\\\",\\\"usage\\\":null}}\\n\\n\"}}\n",
            ));
            std::future::pending::<()>().await;
        }
        .boxed();

        let response = execute_stream_from_frame_stream(
            &state,
            plan,
            "trace-image-done-close-success",
            &test_decision(),
            "openai_chat_stream",
            Some("openai_chat_stream_success".to_string()),
            Some(json!({
                "request_id": "req-image-done-close-success",
                "candidate_id": "cand-image-done-close-success",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:image",
                "client_api_format": "openai:chat",
                "image_request": {
                    "size": "1024x1024",
                    "quality": "medium"
                }
            })),
            crate::clock::current_unix_ms(),
            Instant::now(),
            frame_stream,
            None,
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        let mut body_stream = response.into_body().into_data_stream();
        let mut body = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !String::from_utf8_lossy(&body).contains("data: [DONE]") {
                let chunk = body_stream
                    .next()
                    .await
                    .expect("body should yield until done")
                    .expect("chunk should be ok");
                body.extend_from_slice(&chunk);
            }
        })
        .await
        .expect("final DONE should arrive");
        drop(body_stream);

        let candidates = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let candidates = request_candidate_repository
                    .list_by_request_id("req-image-done-close-success")
                    .await
                    .expect("request candidates should read");
                if candidates
                    .first()
                    .is_some_and(|candidate| candidate.status == RequestCandidateStatus::Success)
                {
                    break candidates;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("candidate should be marked success");
        assert_eq!(candidates[0].status_code, Some(200));

        let stored_usage = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let usage = usage_repository
                    .find_by_request_id("req-image-done-close-success")
                    .await
                    .expect("usage should read");
                if usage
                    .as_ref()
                    .is_some_and(|usage| usage.status == "completed")
                {
                    break usage.expect("completed usage should exist");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be marked completed");
        assert_eq!(stored_usage.status_code, Some(200));
        assert!(stored_usage.total_tokens > 0);
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_records_first_data_as_streaming_before_terminal_telemetry(
    ) {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let first_data_seen = Arc::new(Notify::new());
        let release_terminal = Arc::new(Notify::new());
        let first_data_seen_for_route = Arc::clone(&first_data_seen);
        let release_terminal_for_route = Arc::clone(&release_terminal);
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(move |_request: Request| {
                    let first_data_seen = Arc::clone(&first_data_seen_for_route);
                    let release_terminal = Arc::clone(&release_terminal_for_route);
                    async move {
                        let frames = stream! {
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"text/event-stream\"}}}\n",
                            ));
                            tokio::time::sleep(Duration::from_millis(10)).await;
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"event: response.output_text.delta\\ndata: {\\\"type\\\":\\\"response.output_text.delta\\\",\\\"delta\\\":\\\"hi\\\"}\\n\\n\"}}\n",
                            ));
                            first_data_seen.notify_one();
                            release_terminal.notified().await;
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"ttfb_ms\":123,\"elapsed_ms\":456}}}\n",
                            ));
                            yield Ok::<Bytes, Infallible>(Bytes::from_static(
                                b"{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n",
                            ));
                        };
                        let mut response = axum::http::Response::new(Body::from_stream(frames));
                        response.headers_mut().insert(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("application/x-ndjson"),
                        );
                        response
                    }
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            })
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-live-stream-first-data".into(),
            candidate_id: Some("cand-live-stream-first-data".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "input": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:responses".into(),
            provider_api_format: "openai:responses".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("cli".to_string()),
            Some("openai:responses".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-live-stream-first-data",
            &decision,
            "openai_responses_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:responses",
                "client_api_format": "openai:responses",
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        first_data_seen.notified().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let live_usage = loop {
            let usage = usage_repository
                .find_by_request_id("req-live-stream-first-data")
                .await
                .expect("usage should read");
            if usage.as_ref().is_some_and(|usage| {
                usage.status == "streaming" && usage.first_byte_time_ms.is_some()
            }) {
                break usage.expect("live usage should exist");
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "usage should record streaming status with first byte before terminal telemetry"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        assert_eq!(live_usage.status, "streaming");
        assert!(live_usage.first_byte_time_ms.is_some());

        release_terminal.notify_one();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let text = String::from_utf8(body.to_vec()).expect("response body should be utf8");
        assert!(text.contains("response.output_text.delta"));

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_bridges_sync_json_body_from_remote_runtime_to_sse() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(|_request: Request| async move {
                    let frames = concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"application/json\"}}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"{\\\"id\\\":\\\"resp-remote-runtime-sync-json-123\\\",\\\"object\\\":\\\"response\\\",\\\"model\\\":\\\"gpt-5.4\\\",\\\"status\\\":\\\"completed\\\",\\\"output\\\":[{\\\"type\\\":\\\"message\\\",\\\"id\\\":\\\"msg-remote-runtime-sync-json-123\\\",\\\"role\\\":\\\"assistant\\\",\\\"content\\\":[{\\\"type\\\":\\\"output_text\\\",\\\"text\\\":\\\"Hello from remote runtime sync json\\\",\\\"annotations\\\":[]}]}],\\\"usage\\\":{\\\"input_tokens\\\":1,\\\"output_tokens\\\":2,\\\"total_tokens\\\":3}}\"}}\n",
                        "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":41}}}\n",
                        "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                    );
                    let mut response = axum::http::Response::new(Body::from(frames));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/x-ndjson"),
                    );
                    response
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let state = AppState::new()
            .expect("app state should build")
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-remote-runtime-sync-json-stream".into(),
            candidate_id: Some("cand-remote-runtime-sync-json-stream".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "input": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:responses".into(),
            provider_api_format: "openai:responses".into(),
            model_name: Some("gpt-5.4".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/responses",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("cli".to_string()),
            Some("openai:responses".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-remote-runtime-sync-json-stream",
            &decision,
            "openai_responses_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:responses",
                "client_api_format": "openai:responses",
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let text = String::from_utf8(body.to_vec()).expect("response body should be utf8");
        assert!(text.contains("event: response.output_text.delta"));
        assert!(text.contains("Hello from remote runtime sync json"));
        assert!(text.contains("event: response.completed"));

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_rewrites_redirect_to_structured_failure() {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(|_request: Request| async move {
                    let frames = concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":302,\"headers\":{\"location\":\"/\",\"content-type\":\"text/html\",\"content-length\":\"0\"}}}\n",
                        "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                    );
                    let mut response = axum::http::Response::new(Body::from(frames));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/x-ndjson"),
                    );
                    response
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let usage_repository = Arc::new(InMemoryUsageReadRepository::default());
        let request_candidate_repository = Arc::new(InMemoryRequestCandidateRepository::default());
        let state = AppState::new()
            .expect("app state should build")
            .with_data_state_for_tests(
                crate::data::GatewayDataState::with_request_candidate_and_usage_repository_for_tests(
                    Arc::clone(&request_candidate_repository),
                    Arc::clone(&usage_repository),
                ),
            )
            .with_usage_runtime_for_tests(UsageRuntimeConfig {
                enabled: true,
                ..UsageRuntimeConfig::default()
            })
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-remote-runtime-stream-redirect".into(),
            candidate_id: Some("cand-remote-runtime-stream-redirect".into()),
            provider_name: Some("ChatGPTWeb".into()),
            provider_id: "prov-redirect".into(),
            endpoint_id: "ep-redirect".into(),
            key_id: "key-redirect".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-5.4",
                "input": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "gemini:generate_content".into(),
            provider_api_format: "openai:responses".into(),
            model_name: Some("gemini-3.1-flash-image-preview".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1beta/models/gemini-3.1-flash-image-preview:streamGenerateContent",
            Some("ai_public".to_string()),
            Some("gemini".to_string()),
            Some("generate_content".to_string()),
            Some("gemini:generate_content".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-remote-runtime-stream-redirect",
            &decision,
            "gemini_chat_stream",
            None,
            Some(json!({
                "request_id": "req-remote-runtime-stream-redirect",
                "candidate_id": "cand-remote-runtime-stream-redirect",
                "candidate_index": 0,
                "retry_index": 0,
                "provider_api_format": "openai:responses",
                "client_api_format": "gemini:generate_content",
                "needs_conversion": true
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        assert_eq!(response.status(), axum::http::StatusCode::BAD_GATEWAY);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        assert_eq!(
            response
                .headers()
                .get("x-aether-upstream-status")
                .and_then(|value| value.to_str().ok()),
            Some("302")
        );
        assert!(
            response.headers().get(header::LOCATION).is_none(),
            "redirect location should not be forwarded to AI clients"
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_json: Value =
            serde_json::from_slice(&body).expect("response body should decode as json");
        assert_eq!(
            body_json["error"]["type"],
            json!("execution_runtime_non_success_status")
        );
        assert_eq!(body_json["error"]["upstream_status"], json!(302));
        assert_eq!(body_json["error"]["location"], json!("/"));
        assert!(body_json["error"]["message"]
            .as_str()
            .is_some_and(|value| value.contains("non-success status 302")));

        let usage = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(usage) = usage_repository
                    .find_by_request_id("req-remote-runtime-stream-redirect")
                    .await
                    .expect("usage should read")
                    .filter(|usage| usage.status == "failed")
                {
                    break usage;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage should be written");

        assert_eq!(usage.status_code, Some(302));
        assert_eq!(usage.error_category.as_deref(), Some("redirect"));
        assert!(usage
            .error_message
            .as_deref()
            .is_some_and(|value| value.contains("non-success status 302")));
        assert_eq!(
            usage
                .client_response_headers
                .as_ref()
                .and_then(|headers| headers.get("x-aether-upstream-status")),
            Some(&json!("302"))
        );
        assert_eq!(
            usage
                .response_headers
                .as_ref()
                .and_then(|headers| headers.get("location")),
            Some(&json!("/"))
        );
        assert!(
            usage.response_body.is_none(),
            "upstream redirect did not include a body"
        );
        assert_eq!(
            usage
                .client_response_body
                .as_ref()
                .and_then(|body| body.pointer("/error/upstream_status")),
            Some(&json!(302))
        );
        let candidates = request_candidate_repository
            .list_by_request_id("req-remote-runtime-stream-redirect")
            .await
            .expect("candidate trace should read");
        let candidate_extra = candidates
            .first()
            .and_then(|candidate| candidate.extra_data.as_ref())
            .expect("failed candidate extra_data should exist");
        assert_eq!(
            candidate_extra["upstream_response"]["status_code"],
            json!(302)
        );
        assert_eq!(
            candidate_extra["upstream_response"]["headers"]["location"],
            json!("/")
        );
        assert!(candidate_extra["upstream_response"].get("body").is_none());
        assert!(candidate_extra.get("client_response").is_none());

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_bridges_openai_image_sync_json_from_remote_runtime_to_image_sse(
    ) {
        let listener = crate::test_support::bind_loopback_listener()
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("local addr should resolve");
        let server = tokio::spawn(async move {
            let app = Router::new().route(
                "/v1/execute/stream",
                any(|_request: Request| async move {
                    let frames = concat!(
                        "{\"type\":\"headers\",\"payload\":{\"kind\":\"headers\",\"status_code\":200,\"headers\":{\"content-type\":\"application/json\"}}}\n",
                        "{\"type\":\"data\",\"payload\":{\"kind\":\"data\",\"text\":\"{\\\"created\\\":1776972364,\\\"data\\\":[{\\\"b64_json\\\":\\\"aGVsbG8=\\\"}],\\\"usage\\\":{\\\"total_tokens\\\":100,\\\"input_tokens\\\":50,\\\"output_tokens\\\":50,\\\"input_tokens_details\\\":{\\\"text_tokens\\\":10,\\\"image_tokens\\\":40}}}\"}}\n",
                        "{\"type\":\"telemetry\",\"payload\":{\"kind\":\"telemetry\",\"telemetry\":{\"elapsed_ms\":41}}}\n",
                        "{\"type\":\"eof\",\"payload\":{\"kind\":\"eof\"}}\n"
                    );
                    let mut response = axum::http::Response::new(Body::from(frames));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/x-ndjson"),
                    );
                    response
                }),
            );
            axum::serve(listener, app)
                .await
                .expect("server should start");
        });

        let state = AppState::new()
            .expect("app state should build")
            .with_execution_runtime_override_base_url(format!("http://{addr}"));
        let plan = ExecutionPlan {
            request_id: "req-remote-runtime-image-sync-json-stream".into(),
            candidate_id: Some("cand-remote-runtime-image-sync-json-stream".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://chatgpt.com/backend-api/codex/responses".into(),
            headers: BTreeMap::from([
                ("content-type".into(), "application/json".into()),
                ("accept".into(), "text/event-stream".into()),
            ]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({
                "model": "gpt-image-1",
                "prompt": "hello",
                "stream": true
            })),
            stream: true,
            client_api_format: "openai:image".into(),
            provider_api_format: "openai:image".into(),
            model_name: Some("gpt-image-1".into()),
            proxy: None,
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = GatewayControlDecision::synthetic(
            "/v1/images/generations",
            Some("ai_public".to_string()),
            Some("openai".to_string()),
            Some("image".to_string()),
            Some("openai:image".to_string()),
        )
        .with_execution_runtime_candidate(true);

        let response = execute_execution_runtime_stream(
            &state,
            plan,
            "trace-remote-runtime-image-sync-json-stream",
            &decision,
            "openai_image_stream",
            None,
            Some(json!({
                "provider_api_format": "openai:image",
                "client_api_format": "openai:image",
                "mapped_model": "gpt-image-1",
                "image_request": {
                    "operation": "generate"
                }
            })),
        )
        .await
        .expect("execution should succeed")
        .expect("execution should return a client response");

        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let text = String::from_utf8(body.to_vec()).expect("response body should be utf8");
        assert!(text.contains("event: image_generation.completed"));
        assert!(text.contains("\"type\":\"image_generation.completed\""));
        assert!(text.contains("\"b64_json\":\"aGVsbG8=\""));
        assert!(text.contains("\"total_tokens\":100"));

        server.abort();
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_returns_client_error_with_local_tunnel_message_before_first_data(
    ) {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            901,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        )));

        let plan = ExecutionPlan {
            request_id: "req-client-stream-error-1".into(),
            candidate_id: Some("cand-client-stream-error-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = test_decision();

        let state_for_task = state.clone();
        let plan_for_task = plan.clone();
        let decision_for_task = decision.clone();
        let execution_task = tokio::spawn(async move {
            execute_execution_runtime_stream(
                &state_for_task,
                plan_for_task,
                "trace-local-stream-client-error",
                &decision_for_task,
                "openai_chat_stream",
                None,
                Some(json!({
                    "client_api_format": "openai:chat",
                    "provider_api_format": "openai:chat",
                })),
            )
            .await
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header = tunnel_protocol::FrameHeader::parse(&request_body)
            .expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, tunnel_protocol::REQUEST_BODY);

        let response_meta = tunnel_protocol::ResponseMeta {
            status: 200,
            // Use a non-SSE content type so direct finalize prefetch stays enabled and the
            // pre-body tunnel error is surfaced as a client-visible structured error response.
            headers: vec![("content-type".to_string(), "application/json".to_string())],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(901, &mut response_headers_frame)
            .await;

        let original_error = "proxy disconnected before first upstream event";
        let mut response_error_frame =
            tunnel_protocol::encode_stream_error(request_header.stream_id, original_error);
        tunnel_app
            .hub
            .handle_proxy_frame(901, &mut response_error_frame)
            .await;

        let response = execution_task
            .await
            .expect("execution task should complete")
            .expect("execution should succeed")
            .expect("execution should return a client response");

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body_json: Value =
            serde_json::from_slice(&body).expect("response body should decode as json");

        let error_message = body_json
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .expect("response body should contain error.message");

        assert_eq!(error_message, original_error);
        assert!(
            !error_message.contains("unexpected EOF during chunk size line"),
            "client-facing response should preserve the original local tunnel error"
        );
    }

    #[tokio::test]
    async fn execute_execution_runtime_stream_emits_terminal_sse_error_event_after_body_started() {
        let state = AppState::new().expect("app state should build");
        let tunnel_app = state.tunnel.app_state();
        let (proxy_tx, mut proxy_rx) = aether_runtime::bounded_queue(8);
        let (proxy_close_tx, _) = watch::channel(false);
        tunnel_app.hub.register_proxy(Arc::new(TunnelProxyConn::new(
            902,
            "node-1".to_string(),
            "Node 1".to_string(),
            proxy_tx,
            proxy_close_tx,
            16,
            2,
        )));

        let plan = ExecutionPlan {
            request_id: "req-client-stream-sse-error-1".into(),
            candidate_id: Some("cand-client-stream-sse-error-1".into()),
            provider_name: Some("openai".into()),
            provider_id: "prov-1".into(),
            endpoint_id: "ep-1".into(),
            key_id: "key-1".into(),
            method: "POST".into(),
            url: "https://example.com/chat".into(),
            headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
            content_type: Some("application/json".into()),
            content_encoding: None,
            body: RequestBody::from_json(json!({"stream": true})),
            stream: true,
            client_api_format: "openai:chat".into(),
            provider_api_format: "openai:chat".into(),
            model_name: Some("gpt-5".into()),
            proxy: Some(tunnel_proxy_snapshot("http://127.0.0.1:1".to_string())),
            transport_profile: None,
            timeouts: Some(ExecutionTimeouts {
                connect_ms: Some(5_000),
                total_ms: Some(5_000),
                ..ExecutionTimeouts::default()
            }),
        };
        let decision = test_decision();

        let state_for_task = state.clone();
        let plan_for_task = plan.clone();
        let decision_for_task = decision.clone();
        let execution_task = tokio::spawn(async move {
            execute_execution_runtime_stream(
                &state_for_task,
                plan_for_task,
                "trace-local-stream-sse-error",
                &decision_for_task,
                "openai_chat_stream",
                None,
                Some(json!({
                    "client_api_format": "openai:chat",
                    "provider_api_format": "openai:chat",
                })),
            )
            .await
        });

        let request_headers = match proxy_rx.recv().await.expect("headers frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_header = tunnel_protocol::FrameHeader::parse(&request_headers)
            .expect("request header frame should parse");
        assert_eq!(request_header.msg_type, tunnel_protocol::REQUEST_HEADERS);

        let request_body = match proxy_rx.recv().await.expect("body frame should arrive") {
            Message::Binary(data) => data,
            other => panic!("unexpected message: {other:?}"),
        };
        let request_body_header = tunnel_protocol::FrameHeader::parse(&request_body)
            .expect("request body frame should parse");
        assert_eq!(request_body_header.msg_type, tunnel_protocol::REQUEST_BODY);

        let response_meta = tunnel_protocol::ResponseMeta {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
        };
        let response_payload =
            serde_json::to_vec(&response_meta).expect("response meta should serialize");
        let mut response_headers_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_HEADERS,
            0,
            &response_payload,
        );
        tunnel_app
            .hub
            .handle_proxy_frame(902, &mut response_headers_frame)
            .await;

        let mut response_body_frame = tunnel_protocol::encode_frame(
            request_header.stream_id,
            tunnel_protocol::RESPONSE_BODY,
            0,
            b"data: hello\n\n",
        );
        tunnel_app
            .hub
            .handle_proxy_frame(902, &mut response_body_frame)
            .await;

        let response = execution_task
            .await
            .expect("execution task should complete")
            .expect("execution should succeed")
            .expect("execution should return a client response");
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );

        let body_task = tokio::spawn(async move {
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("response body should read");
            String::from_utf8(body.to_vec()).expect("response body should be utf8")
        });

        let original_error = "proxy disconnected while forwarding upstream body";
        let mut response_error_frame =
            tunnel_protocol::encode_stream_error(request_header.stream_id, original_error);
        tunnel_app
            .hub
            .handle_proxy_frame(902, &mut response_error_frame)
            .await;

        let body = body_task.await.expect("body task should complete");
        assert!(body.contains("data: hello\n\n"));
        assert!(body.contains("data: {\"error\":"));
        assert!(body.contains(original_error));
        assert!(body.contains("data: [DONE]\n\n"));
        assert!(
            !body.contains("unexpected EOF during chunk size line"),
            "same-format SSE path should surface the original terminal error event"
        );
    }
}
