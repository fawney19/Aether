use std::collections::BTreeMap;

use aether_contracts::{ExecutionStreamTerminalSummary, ExecutionTelemetry};
use aether_data_contracts::repository::usage::UsageBodyCaptureState;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const GEMINI_FILE_MAPPING_TTL_SECONDS: u64 = 60 * 60 * 48;
const GEMINI_FILE_MAPPING_CACHE_PREFIX: &str = "gemini_files:key";
pub const STREAM_MISSING_TERMINAL_EVENT_CATEGORY: &str = "stream_missing_terminal_event";
pub const STREAM_TERMINAL_ERROR_CATEGORY: &str = "stream_terminal_error";
pub const STREAM_MISSING_TERMINAL_EVENT_MESSAGE: &str =
    "execution runtime stream ended before provider terminal event";
pub const STREAM_TERMINAL_ERROR_MESSAGE: &str =
    "execution runtime stream ended with a terminal error";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamCapturedTerminalState {
    Completed,
    Failed,
    Missing,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewaySyncReportRequest {
    pub trace_id: String,
    pub report_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_context: Option<serde_json::Value>,
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_body_json: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<ExecutionTelemetry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewayStreamReportRequest {
    pub trace_id: String,
    pub report_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_context: Option<serde_json::Value>,
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_body_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_body_state: Option<UsageBodyCaptureState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_body_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_body_state: Option<UsageBodyCaptureState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_summary: Option<ExecutionStreamTerminalSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<ExecutionTelemetry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InternalFinalizeRoute {
    pub public_path: &'static str,
    pub route_family: &'static str,
    pub route_kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiFileMappingEntry {
    pub file_name: String,
    pub display_name: Option<String>,
    pub mime_type: Option<String>,
}

pub fn infer_internal_finalize_signature(payload: &GatewaySyncReportRequest) -> Option<String> {
    let report_context = payload.report_context.as_ref()?;
    let from_context = report_context
        .get("client_api_format")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            report_context
                .get("provider_api_format")
                .and_then(serde_json::Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if from_context.is_some() {
        return from_context;
    }

    let report_kind = payload.report_kind.trim().to_ascii_lowercase();
    if report_kind.starts_with("openai_chat_") {
        return Some("openai:chat".to_string());
    }
    if report_kind.starts_with("openai_compact_") {
        return Some("openai:responses:compact".to_string());
    }
    if report_kind.starts_with("openai_responses_compact_") {
        return Some("openai:responses:compact".to_string());
    }
    if report_kind.starts_with("openai_responses_") {
        return Some("openai:responses".to_string());
    }
    if report_kind.starts_with("openai_image_") {
        return Some("openai:image".to_string());
    }
    if report_kind.starts_with("openai_search_") {
        return Some("openai:search".to_string());
    }
    if report_kind.starts_with("openai_cli_") {
        return Some("openai:responses".to_string());
    }
    if report_kind.starts_with("openai_video_") {
        return Some("openai:video".to_string());
    }
    if report_kind.starts_with("claude_chat_") {
        return Some("claude:messages".to_string());
    }
    if report_kind.starts_with("claude_cli_") {
        return Some("claude:messages".to_string());
    }
    if report_kind.starts_with("gemini_chat_") {
        return Some("gemini:generate_content".to_string());
    }
    if report_kind.starts_with("gemini_cli_") {
        return Some("gemini:generate_content".to_string());
    }
    if report_kind.starts_with("gemini_video_") {
        return Some("gemini:video".to_string());
    }
    None
}

pub fn resolve_internal_finalize_route(signature: &str) -> Option<InternalFinalizeRoute> {
    match aether_ai_formats::normalize_api_format_alias(signature).as_str() {
        "openai:chat" => Some(InternalFinalizeRoute {
            public_path: "/v1/chat/completions",
            route_family: "openai",
            route_kind: "chat",
        }),
        "openai:responses" => Some(InternalFinalizeRoute {
            public_path: "/v1/responses",
            route_family: "openai",
            route_kind: "responses",
        }),
        "openai:responses:compact" => Some(InternalFinalizeRoute {
            public_path: "/v1/responses/compact",
            route_family: "openai",
            route_kind: "responses:compact",
        }),
        "openai:search" => Some(InternalFinalizeRoute {
            public_path: "/v1/alpha/search",
            route_family: "openai",
            route_kind: "search",
        }),
        "openai:image" => Some(InternalFinalizeRoute {
            public_path: "/v1/images/generations",
            route_family: "openai",
            route_kind: "image",
        }),
        "openai:video" => Some(InternalFinalizeRoute {
            public_path: "/v1/videos",
            route_family: "openai",
            route_kind: "video",
        }),
        "claude:messages" => Some(InternalFinalizeRoute {
            public_path: "/v1/messages",
            route_family: "claude",
            route_kind: "messages",
        }),
        "gemini:generate_content" => Some(InternalFinalizeRoute {
            public_path: "/v1beta/models",
            route_family: "gemini",
            route_kind: "generate_content",
        }),
        "gemini:video" => Some(InternalFinalizeRoute {
            public_path: "/v1beta/models",
            route_family: "gemini",
            route_kind: "video",
        }),
        _ => None,
    }
}

pub fn normalize_gemini_file_name(file_name: &str) -> Option<String> {
    let file_name = file_name.trim();
    if file_name.is_empty() {
        return None;
    }
    if file_name.starts_with("files/") {
        Some(file_name.to_string())
    } else {
        Some(format!("files/{file_name}"))
    }
}

pub fn gemini_file_mapping_cache_key(file_name: &str) -> String {
    format!("{GEMINI_FILE_MAPPING_CACHE_PREFIX}:{file_name}")
}

pub fn extract_gemini_file_mapping_entries(
    payload: &GatewaySyncReportRequest,
) -> Vec<GeminiFileMappingEntry> {
    let Some(body) = extract_sync_report_body_json(payload) else {
        return Vec::new();
    };
    let Some(object) = body.as_object() else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    maybe_push_gemini_file_mapping_entry(&mut entries, object);

    if let Some(file_object) = object.get("file").and_then(Value::as_object) {
        maybe_push_gemini_file_mapping_entry(&mut entries, file_object);
    }

    if let Some(files) = object.get("files").and_then(Value::as_array) {
        for item in files {
            if let Some(file_object) = item.as_object() {
                maybe_push_gemini_file_mapping_entry(&mut entries, file_object);
            }
        }
    }

    entries
}

pub fn report_request_id(report_context: Option<&serde_json::Value>) -> &str {
    report_context
        .and_then(|context| context.get("request_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("-")
}

pub fn is_local_ai_sync_report_kind(report_kind: &str) -> bool {
    matches!(
        report_kind,
        "openai_chat_sync_success"
            | "claude_chat_sync_success"
            | "gemini_chat_sync_success"
            | "openai_chat_sync_error"
            | "claude_chat_sync_error"
            | "gemini_chat_sync_error"
            | "openai_responses_sync_success"
            | "openai_responses_compact_sync_success"
            | "openai_responses_sync_error"
            | "openai_responses_compact_sync_error"
            | "openai_cli_sync_success"
            | "openai_image_sync_success"
            | "openai_search_sync_success"
            | "openai_image_sync_error"
            | "openai_embedding_sync_success"
            | "openai_embedding_sync_error"
            | "gemini_embedding_sync_success"
            | "claude_cli_sync_success"
            | "gemini_cli_sync_success"
            | "openai_cli_sync_error"
            | "openai_compact_sync_error"
            | "claude_cli_sync_error"
            | "gemini_cli_sync_error"
            | "openai_video_create_sync_success"
            | "openai_video_remix_sync_success"
            | "gemini_video_create_sync_success"
            | "openai_video_delete_sync_success"
            | "openai_video_cancel_sync_success"
            | "gemini_video_cancel_sync_success"
            | "openai_video_create_sync_error"
            | "openai_video_remix_sync_error"
            | "gemini_video_create_sync_error"
            | "openai_video_delete_sync_error"
            | "openai_video_cancel_sync_error"
            | "gemini_video_cancel_sync_error"
            | "gemini_files_store_mapping"
            | "gemini_files_delete_mapping"
    )
}

pub fn is_local_ai_stream_report_kind(report_kind: &str) -> bool {
    matches!(
        report_kind,
        "openai_chat_stream_success"
            | "claude_chat_stream_success"
            | "gemini_chat_stream_success"
            | "openai_responses_stream_success"
            | "openai_responses_compact_stream_success"
            | "openai_cli_stream_success"
            | "claude_cli_stream_success"
            | "gemini_cli_stream_success"
    )
}

pub fn sync_report_represents_failure(
    payload: &GatewaySyncReportRequest,
    error_type: Option<&str>,
) -> bool {
    if payload.report_kind == "openai_video_delete_sync_success" && payload.status_code == 404 {
        return false;
    }

    payload.status_code >= 400
        || payload.report_kind.contains("error")
        || error_type.is_some()
        || payload
            .body_json
            .as_ref()
            .and_then(|body| body.get("error"))
            .is_some_and(|value| !value.is_null())
}

fn stream_terminal_summary_missing_observed_finish(
    summary: &ExecutionStreamTerminalSummary,
    requires_observed_terminal_event: bool,
) -> bool {
    !summary.observed_finish
        && (requires_observed_terminal_event
            || !summary
                .standardized_usage
                .as_ref()
                .is_some_and(aether_contracts::StandardizedUsage::has_token_signal))
}

fn stream_terminal_summary_represents_failure(
    summary: &ExecutionStreamTerminalSummary,
    requires_observed_terminal_event: bool,
) -> bool {
    summary.parser_error.is_some()
        || stream_terminal_summary_missing_observed_finish(
            summary,
            requires_observed_terminal_event,
        )
}

pub fn stream_report_represents_failure(payload: &GatewayStreamReportRequest) -> bool {
    let requires_observed_terminal_event = stream_report_requires_observed_terminal_event(
        payload.report_kind.as_str(),
        payload.report_context.as_ref(),
    );
    payload.status_code >= 400
        || payload.report_kind.contains("error")
        || payload.terminal_summary.as_ref().is_some_and(|summary| {
            stream_terminal_summary_represents_failure(summary, requires_observed_terminal_event)
        })
        || stream_report_captured_terminal_failure(payload)
        || stream_report_missing_terminal_event(payload)
}

pub fn stream_report_missing_terminal_event(payload: &GatewayStreamReportRequest) -> bool {
    let requires_observed_terminal_event = stream_report_requires_observed_terminal_event(
        payload.report_kind.as_str(),
        payload.report_context.as_ref(),
    );
    if !requires_observed_terminal_event {
        return false;
    }

    if let Some(summary) = payload.terminal_summary.as_ref() {
        if stream_terminal_summary_missing_observed_finish(summary, true) {
            return true;
        }
        if summary.observed_finish {
            return false;
        }
    }

    matches!(
        stream_report_captured_terminal_state(payload),
        Some(StreamCapturedTerminalState::Missing)
    )
}

pub fn stream_report_captured_terminal_failure(payload: &GatewayStreamReportRequest) -> bool {
    matches!(
        stream_report_captured_terminal_state(payload),
        Some(StreamCapturedTerminalState::Failed)
    )
}

pub fn stream_report_requires_observed_terminal_event(
    report_kind: &str,
    report_context: Option<&Value>,
) -> bool {
    let report_kind = report_kind.trim().to_ascii_lowercase();
    if report_kind.starts_with("openai_responses_")
        || report_kind.starts_with("openai_compact_")
        || report_kind.starts_with("openai_cli_")
    {
        return true;
    }

    let Some(context) = report_context.and_then(Value::as_object) else {
        return false;
    };
    [
        "provider_stream_event_api_format",
        "provider_stream_api_format",
        "provider_api_format",
        "client_api_format",
    ]
    .into_iter()
    .filter_map(|field| context.get(field).and_then(Value::as_str))
    .any(is_openai_responses_family_format_alias)
}

fn is_openai_responses_family_format_alias(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace('_', ":");
    aether_ai_formats::is_openai_responses_family_format(normalized.as_str())
}

fn stream_report_captured_terminal_state(
    payload: &GatewayStreamReportRequest,
) -> Option<StreamCapturedTerminalState> {
    let provider_requires_terminal =
        stream_report_provider_capture_requires_terminal_event(payload);
    let client_requires_terminal = stream_report_client_capture_requires_terminal_event(payload);
    let provider_state = provider_requires_terminal
        .then(|| {
            stream_capture_terminal_state_from_base64(
                payload.provider_body_base64.as_deref(),
                payload.provider_body_state,
            )
        })
        .flatten();
    let client_state = client_requires_terminal
        .then(|| {
            stream_capture_terminal_state_from_base64(
                payload.client_body_base64.as_deref(),
                payload.client_body_state,
            )
        })
        .flatten();
    combine_stream_terminal_states(provider_state, client_state).or_else(|| {
        stream_report_required_captures_are_empty(
            payload,
            provider_requires_terminal,
            client_requires_terminal,
        )
        .then_some(StreamCapturedTerminalState::Missing)
    })
}

fn stream_report_required_captures_are_empty(
    payload: &GatewayStreamReportRequest,
    provider_requires_terminal: bool,
    client_requires_terminal: bool,
) -> bool {
    let mut has_required_capture = false;
    let mut all_required_captures_empty = true;

    if provider_requires_terminal {
        has_required_capture = true;
        all_required_captures_empty &= payload.provider_body_base64.is_none()
            && payload.provider_body_state == Some(UsageBodyCaptureState::None);
    }

    if client_requires_terminal {
        has_required_capture = true;
        all_required_captures_empty &= payload.client_body_base64.is_none()
            && payload.client_body_state == Some(UsageBodyCaptureState::None);
    }

    has_required_capture && all_required_captures_empty
}

fn stream_report_provider_capture_requires_terminal_event(
    payload: &GatewayStreamReportRequest,
) -> bool {
    let context = payload.report_context.as_ref();
    stream_report_context_has_openai_responses_format(
        context,
        &[
            "provider_stream_event_api_format",
            "provider_stream_api_format",
            "provider_api_format",
        ],
    ) || (stream_report_kind_requires_observed_terminal_event(payload.report_kind.as_str())
        && !stream_report_context_has_any_format(context))
}

fn stream_report_client_capture_requires_terminal_event(
    payload: &GatewayStreamReportRequest,
) -> bool {
    let context = payload.report_context.as_ref();
    stream_report_context_has_openai_responses_format(context, &["client_api_format"])
        || (stream_report_kind_requires_observed_terminal_event(payload.report_kind.as_str())
            && !stream_report_context_has_any_format(context))
}

fn stream_report_context_has_openai_responses_format(
    report_context: Option<&Value>,
    fields: &[&str],
) -> bool {
    fields
        .iter()
        .filter_map(|field| {
            report_context
                .and_then(Value::as_object)
                .and_then(|context| context.get(*field))
                .and_then(Value::as_str)
        })
        .any(is_openai_responses_family_format_alias)
}

fn stream_report_context_has_any_format(report_context: Option<&Value>) -> bool {
    [
        "provider_stream_event_api_format",
        "provider_stream_api_format",
        "provider_api_format",
        "client_api_format",
    ]
    .into_iter()
    .any(|field| {
        report_context
            .and_then(Value::as_object)
            .and_then(|context| context.get(field))
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    })
}

fn stream_report_kind_requires_observed_terminal_event(report_kind: &str) -> bool {
    let report_kind = report_kind.trim().to_ascii_lowercase();
    report_kind.starts_with("openai_responses_")
        || report_kind.starts_with("openai_compact_")
        || report_kind.starts_with("openai_cli_")
}

fn stream_capture_terminal_state_from_base64(
    body_base64: Option<&str>,
    body_state: Option<UsageBodyCaptureState>,
) -> Option<StreamCapturedTerminalState> {
    let body_base64 = body_base64?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    let state = if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
        stream_capture_terminal_state(&value)
    } else {
        let text = String::from_utf8(bytes).ok()?;
        stream_capture_terminal_state_from_sse_text(text.as_str())
    };
    filter_incomplete_capture_terminal_state(state, body_state)
}

fn filter_incomplete_capture_terminal_state(
    state: Option<StreamCapturedTerminalState>,
    body_state: Option<UsageBodyCaptureState>,
) -> Option<StreamCapturedTerminalState> {
    match state {
        Some(StreamCapturedTerminalState::Missing)
            if !stream_body_capture_can_prove_missing_terminal(body_state) =>
        {
            None
        }
        other => other,
    }
}

fn stream_body_capture_can_prove_missing_terminal(
    body_state: Option<UsageBodyCaptureState>,
) -> bool {
    matches!(
        body_state,
        None | Some(UsageBodyCaptureState::Inline) | Some(UsageBodyCaptureState::None)
    )
}

pub fn stream_capture_terminal_state(value: &Value) -> Option<StreamCapturedTerminalState> {
    if let Some(chunks) = value.get("chunks").and_then(Value::as_array) {
        let mut state = None;
        for chunk in chunks {
            state = combine_stream_terminal_states(state, openai_response_terminal_state(chunk));
        }
        return state.or_else(|| {
            stream_capture_looks_like_stream(value).then_some(StreamCapturedTerminalState::Missing)
        });
    }

    openai_response_terminal_state(value)
}

fn stream_capture_looks_like_stream(value: &Value) -> bool {
    value
        .get("metadata")
        .and_then(|metadata| metadata.get("stream"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || value.get("chunks").and_then(Value::as_array).is_some()
}

fn openai_response_terminal_state(value: &Value) -> Option<StreamCapturedTerminalState> {
    if value.get("error").is_some_and(|error| !error.is_null()) {
        return Some(StreamCapturedTerminalState::Failed);
    }

    let event_type = value
        .get("type")
        .or_else(|| value.get("event"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    match event_type {
        "response.completed" => return Some(StreamCapturedTerminalState::Completed),
        "response.failed" | "response.incomplete" | "error" => {
            return Some(StreamCapturedTerminalState::Failed);
        }
        _ => {}
    }

    let response = value.get("response");
    if response
        .and_then(|response| response.get("error"))
        .is_some_and(|error| !error.is_null())
        || response
            .and_then(|response| response.get("incomplete_details"))
            .is_some_and(|details| !details.is_null())
    {
        return Some(StreamCapturedTerminalState::Failed);
    }

    match response
        .and_then(|response| response.get("status"))
        .and_then(Value::as_str)
        .map(str::trim)
    {
        Some("completed") => Some(StreamCapturedTerminalState::Completed),
        Some("failed" | "incomplete" | "cancelled") => Some(StreamCapturedTerminalState::Failed),
        _ => None,
    }
}

fn combine_stream_terminal_states(
    current: Option<StreamCapturedTerminalState>,
    next: Option<StreamCapturedTerminalState>,
) -> Option<StreamCapturedTerminalState> {
    match (current, next) {
        (Some(StreamCapturedTerminalState::Failed), _)
        | (_, Some(StreamCapturedTerminalState::Failed)) => {
            Some(StreamCapturedTerminalState::Failed)
        }
        (Some(StreamCapturedTerminalState::Missing), _)
        | (_, Some(StreamCapturedTerminalState::Missing)) => {
            Some(StreamCapturedTerminalState::Missing)
        }
        (Some(StreamCapturedTerminalState::Completed), _)
        | (_, Some(StreamCapturedTerminalState::Completed)) => {
            Some(StreamCapturedTerminalState::Completed)
        }
        (None, None) => None,
    }
}

fn stream_capture_terminal_state_from_sse_text(text: &str) -> Option<StreamCapturedTerminalState> {
    let mut state = None;
    let mut saw_stream_payload = false;
    for_each_sse_payload(text, |payload| {
        saw_stream_payload = true;
        if payload == "[DONE]" {
            return;
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload) {
            state = combine_stream_terminal_states(state, openai_response_terminal_state(&value));
        }
    });
    state.or_else(|| saw_stream_payload.then_some(StreamCapturedTerminalState::Missing))
}

fn flush_sse_payload<F>(payload: &mut String, has_payload: &mut bool, on_payload: &mut F)
where
    F: FnMut(&str),
{
    if !*has_payload {
        return;
    }
    on_payload(payload);
    payload.clear();
    *has_payload = false;
}

fn for_each_sse_payload<F>(text: &str, mut on_payload: F)
where
    F: FnMut(&str),
{
    let mut payload = String::new();
    let mut has_payload = false;
    let bytes = text.as_bytes();
    let mut line_start = 0usize;
    let mut cursor = 0usize;

    while cursor <= bytes.len() {
        if cursor < bytes.len() && bytes[cursor] != b'\n' && bytes[cursor] != b'\r' {
            cursor += 1;
            continue;
        }

        let line = text[line_start..cursor].trim();
        if line.is_empty() {
            flush_sse_payload(&mut payload, &mut has_payload, &mut on_payload);
        } else if let Some(data) = line.strip_prefix("data:").map(str::trim) {
            if !data.is_empty() {
                if has_payload {
                    payload.push('\n');
                }
                payload.push_str(data);
                has_payload = true;
            }
        }

        if cursor == bytes.len() {
            break;
        }
        if bytes[cursor] == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
            cursor += 2;
        } else {
            cursor += 1;
        }
        line_start = cursor;
    }
    flush_sse_payload(&mut payload, &mut has_payload, &mut on_payload);
}

pub fn should_handle_local_sync_report(
    report_context: Option<&serde_json::Value>,
    report_kind: &str,
) -> bool {
    crate::report_context::report_context_is_locally_actionable(report_context)
        && is_local_ai_sync_report_kind(report_kind)
}

pub fn should_handle_local_stream_report(
    report_context: Option<&serde_json::Value>,
    report_kind: &str,
) -> bool {
    crate::report_context::report_context_is_locally_actionable(report_context)
        && is_local_ai_stream_report_kind(report_kind)
}

fn maybe_push_gemini_file_mapping_entry(
    entries: &mut Vec<GeminiFileMappingEntry>,
    object: &serde_json::Map<String, Value>,
) {
    let file_name = object
        .get("name")
        .and_then(Value::as_str)
        .and_then(normalize_gemini_file_name);
    let Some(file_name) = file_name else {
        return;
    };

    if entries.iter().any(|entry| entry.file_name == file_name) {
        return;
    }

    entries.push(GeminiFileMappingEntry {
        file_name,
        display_name: object
            .get("displayName")
            .or_else(|| object.get("display_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        mime_type: object
            .get("mimeType")
            .or_else(|| object.get("mime_type"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
    });
}

fn extract_sync_report_body_json(payload: &GatewaySyncReportRequest) -> Option<Value> {
    if let Some(body_json) = payload.body_json.as_ref() {
        return Some(body_json.clone());
    }
    if let Some(client_body_json) = payload.client_body_json.as_ref() {
        return Some(client_body_json.clone());
    }
    if !content_type_starts_with(&payload.headers, "application/json") {
        return None;
    }

    let body_base64 = payload.body_base64.as_deref()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body_base64)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn content_type_starts_with(headers: &BTreeMap<String, String>, expected_prefix: &str) -> bool {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim().to_ascii_lowercase())
        .is_some_and(|value| value.starts_with(expected_prefix))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_contracts::ExecutionStreamTerminalSummary;
    use aether_data_contracts::repository::usage::UsageBodyCaptureState;
    use base64::Engine as _;
    use serde_json::json;

    use super::{
        extract_gemini_file_mapping_entries, gemini_file_mapping_cache_key,
        infer_internal_finalize_signature, is_local_ai_stream_report_kind,
        is_local_ai_sync_report_kind, normalize_gemini_file_name, report_request_id,
        resolve_internal_finalize_route, should_handle_local_stream_report,
        should_handle_local_sync_report, stream_report_represents_failure,
        sync_report_represents_failure, GatewayStreamReportRequest, GatewaySyncReportRequest,
        GeminiFileMappingEntry, InternalFinalizeRoute,
    };

    fn sample_sync_report(report_kind: &str, status_code: u16) -> GatewaySyncReportRequest {
        GatewaySyncReportRequest {
            trace_id: "trace-123".to_string(),
            report_kind: report_kind.to_string(),
            report_context: None,
            status_code,
            headers: BTreeMap::new(),
            body_json: None,
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        }
    }

    fn sample_sync_report_with_context(
        report_kind: &str,
        report_context: serde_json::Value,
    ) -> GatewaySyncReportRequest {
        GatewaySyncReportRequest {
            trace_id: "trace-123".to_string(),
            report_kind: report_kind.to_string(),
            report_context: Some(report_context),
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: None,
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        }
    }

    fn sample_stream_report(report_kind: &str, status_code: u16) -> GatewayStreamReportRequest {
        GatewayStreamReportRequest {
            trace_id: "trace-stream-123".to_string(),
            report_kind: report_kind.to_string(),
            report_context: None,
            status_code,
            headers: BTreeMap::new(),
            provider_body_base64: None,
            provider_body_state: None,
            client_body_base64: None,
            client_body_state: None,
            terminal_summary: None,
            telemetry: None,
        }
    }

    #[test]
    fn classifies_local_ai_sync_report_kinds() {
        assert!(is_local_ai_sync_report_kind(
            "openai_video_create_sync_success"
        ));
        assert!(is_local_ai_sync_report_kind(
            "openai_responses_compact_sync_success"
        ));
        assert!(is_local_ai_sync_report_kind(
            "openai_responses_compact_sync_error"
        ));
        assert!(is_local_ai_sync_report_kind("openai_image_sync_success"));
        assert!(is_local_ai_sync_report_kind("openai_search_sync_success"));
        assert!(is_local_ai_sync_report_kind("openai_image_sync_error"));
        assert!(is_local_ai_sync_report_kind(
            "openai_embedding_sync_success"
        ));
        assert!(is_local_ai_sync_report_kind("openai_embedding_sync_error"));
        assert!(is_local_ai_sync_report_kind(
            "gemini_embedding_sync_success"
        ));
        assert!(is_local_ai_sync_report_kind("gemini_files_delete_mapping"));
        assert!(!is_local_ai_sync_report_kind("unknown_sync_kind"));
    }

    #[test]
    fn classifies_local_ai_stream_report_kinds() {
        assert!(is_local_ai_stream_report_kind("openai_chat_stream_success"));
        assert!(is_local_ai_stream_report_kind(
            "openai_responses_compact_stream_success"
        ));
        assert!(!is_local_ai_stream_report_kind("openai_chat_stream_error"));
    }

    #[test]
    fn treats_openai_video_delete_404_success_as_non_failure() {
        let payload = sample_sync_report("openai_video_delete_sync_success", 404);
        assert!(!sync_report_represents_failure(&payload, None));
    }

    #[test]
    fn detects_sync_report_failure_from_status_kind_error_type_or_body() {
        let status_payload = sample_sync_report("openai_chat_sync_success", 500);
        assert!(sync_report_represents_failure(&status_payload, None));

        let kind_payload = sample_sync_report("openai_chat_sync_error", 200);
        assert!(sync_report_represents_failure(&kind_payload, None));

        let error_type_payload = sample_sync_report("openai_chat_sync_success", 200);
        assert!(sync_report_represents_failure(
            &error_type_payload,
            Some("authentication_error")
        ));

        let mut error_body_payload = sample_sync_report("openai_chat_sync_success", 200);
        error_body_payload.body_json = Some(json!({"error": {"message": "bad request"}}));
        assert!(sync_report_represents_failure(&error_body_payload, None));

        let mut null_error_payload = sample_sync_report("openai_chat_sync_success", 200);
        null_error_payload.body_json = Some(json!({"error": null}));
        assert!(!sync_report_represents_failure(&null_error_payload, None));

        let success_payload = sample_sync_report("openai_chat_sync_success", 200);
        assert!(!sync_report_represents_failure(&success_payload, None));
    }

    #[test]
    fn detects_stream_report_failure_from_terminal_summary_error() {
        let mut payload = sample_stream_report("openai_responses_stream_success", 200);
        payload.terminal_summary = Some(ExecutionStreamTerminalSummary {
            observed_finish: true,
            parser_error: Some("policy failure".to_string()),
            ..ExecutionStreamTerminalSummary::default()
        });

        assert!(stream_report_represents_failure(&payload));
    }

    #[test]
    fn detects_openai_responses_stream_missing_terminal_from_captured_sse() {
        let provider_sse = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
        );
        let mut payload = sample_stream_report("openai_responses_stream_success", 200);
        payload.report_context = Some(json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:responses"
        }));
        payload.provider_body_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(provider_sse.as_bytes()));
        payload.provider_body_state = Some(UsageBodyCaptureState::Inline);

        assert!(stream_report_represents_failure(&payload));
        assert!(super::stream_report_missing_terminal_event(&payload));
    }

    #[test]
    fn accepts_openai_responses_stream_completed_from_captured_sse() {
        let provider_sse = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n"
        );
        let mut payload = sample_stream_report("openai_responses_stream_success", 200);
        payload.report_context = Some(json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:responses"
        }));
        payload.provider_body_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(provider_sse.as_bytes()));
        payload.provider_body_state = Some(UsageBodyCaptureState::Inline);

        assert!(!stream_report_represents_failure(&payload));
        assert!(!super::stream_report_missing_terminal_event(&payload));
    }

    #[test]
    fn accepts_terminal_summary_completion_when_captured_sse_is_truncated_before_terminal() {
        let provider_sse = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
        );
        let mut payload = sample_stream_report("openai_responses_stream_success", 200);
        payload.report_context = Some(json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:responses"
        }));
        payload.provider_body_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(provider_sse.as_bytes()));
        payload.provider_body_state = Some(UsageBodyCaptureState::Truncated);
        payload.terminal_summary = Some(ExecutionStreamTerminalSummary {
            observed_finish: true,
            ..ExecutionStreamTerminalSummary::default()
        });

        assert!(!stream_report_represents_failure(&payload));
        assert!(!super::stream_report_missing_terminal_event(&payload));
    }

    #[test]
    fn ignores_missing_terminal_from_truncated_captured_sse_without_summary() {
        let provider_sse = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"
        );
        let mut payload = sample_stream_report("openai_responses_stream_success", 200);
        payload.report_context = Some(json!({
            "client_api_format": "openai:responses",
            "provider_api_format": "openai:responses"
        }));
        payload.provider_body_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(provider_sse.as_bytes()));
        payload.provider_body_state = Some(UsageBodyCaptureState::Truncated);

        assert!(!stream_report_represents_failure(&payload));
        assert!(!super::stream_report_missing_terminal_event(&payload));
    }

    #[test]
    fn accepts_completed_openai_responses_provider_with_rewritten_openai_chat_client_stream() {
        let provider_sse = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n"
        );
        let client_sse = concat!(
            "data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let mut payload = sample_stream_report("openai_chat_stream_success", 200);
        payload.report_context = Some(json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "openai:responses",
            "provider_stream_event_api_format": "openai:responses"
        }));
        payload.provider_body_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(provider_sse.as_bytes()));
        payload.provider_body_state = Some(UsageBodyCaptureState::Inline);
        payload.client_body_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(client_sse.as_bytes()));
        payload.client_body_state = Some(UsageBodyCaptureState::Inline);

        assert!(!stream_report_represents_failure(&payload));
        assert!(!super::stream_report_missing_terminal_event(&payload));
    }

    #[test]
    fn infers_internal_finalize_signature_from_context_or_report_kind() {
        let from_context = sample_sync_report_with_context(
            "unknown_sync_finalize",
            json!({"client_api_format": "gemini:video"}),
        );
        assert_eq!(
            infer_internal_finalize_signature(&from_context),
            Some("gemini:video".to_string())
        );

        let from_report_kind =
            sample_sync_report_with_context("openai_video_create_sync_finalize", json!({}));
        assert_eq!(
            infer_internal_finalize_signature(&from_report_kind),
            Some("openai:video".to_string())
        );

        let from_image_report_kind =
            sample_sync_report_with_context("openai_image_sync_finalize", json!({}));
        assert_eq!(
            infer_internal_finalize_signature(&from_image_report_kind),
            Some("openai:image".to_string())
        );

        let from_compact_report_kind =
            sample_sync_report_with_context("openai_responses_compact_sync_finalize", json!({}));
        assert_eq!(
            infer_internal_finalize_signature(&from_compact_report_kind),
            Some("openai:responses:compact".to_string())
        );

        let from_search_report_kind =
            sample_sync_report_with_context("openai_search_sync_finalize", json!({}));
        assert_eq!(
            infer_internal_finalize_signature(&from_search_report_kind),
            Some("openai:search".to_string())
        );

        let unknown = sample_sync_report("unknown_sync_finalize", 200);
        assert_eq!(infer_internal_finalize_signature(&unknown), None);
    }

    #[test]
    fn resolves_internal_finalize_route_for_supported_signatures() {
        assert_eq!(
            resolve_internal_finalize_route("openai:search"),
            Some(InternalFinalizeRoute {
                public_path: "/v1/alpha/search",
                route_family: "openai",
                route_kind: "search",
            })
        );
        assert_eq!(
            resolve_internal_finalize_route("openai:responses:compact"),
            Some(InternalFinalizeRoute {
                public_path: "/v1/responses/compact",
                route_family: "openai",
                route_kind: "responses:compact",
            })
        );
        assert_eq!(resolve_internal_finalize_route("openai:compact"), None);
        assert_eq!(
            resolve_internal_finalize_route("gemini:video"),
            Some(InternalFinalizeRoute {
                public_path: "/v1beta/models",
                route_family: "gemini",
                route_kind: "video",
            })
        );
        assert_eq!(
            resolve_internal_finalize_route("openai:image"),
            Some(InternalFinalizeRoute {
                public_path: "/v1/images/generations",
                route_family: "openai",
                route_kind: "image",
            })
        );
        assert_eq!(resolve_internal_finalize_route("unknown:kind"), None);
    }

    #[test]
    fn normalizes_gemini_file_names() {
        assert_eq!(
            normalize_gemini_file_name("abc123"),
            Some("files/abc123".to_string())
        );
        assert_eq!(
            normalize_gemini_file_name("files/abc123"),
            Some("files/abc123".to_string())
        );
        assert_eq!(normalize_gemini_file_name("   "), None);
    }

    #[test]
    fn builds_gemini_file_mapping_cache_keys() {
        assert_eq!(
            gemini_file_mapping_cache_key("files/abc123"),
            "gemini_files:key:files/abc123"
        );
    }

    #[test]
    fn extracts_gemini_file_mapping_entries_from_supported_shapes() {
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-123".to_string(),
            report_kind: "gemini_files_store_mapping".to_string(),
            report_context: None,
            status_code: 200,
            headers: BTreeMap::new(),
            body_json: Some(json!({
                "name": "abc123",
                "displayName": "root-name",
                "file": {
                    "name": "files/def456",
                    "mimeType": "image/png"
                },
                "files": [
                    {
                        "name": "abc123",
                        "display_name": "deduped"
                    },
                    {
                        "name": "ghi789",
                        "display_name": "third"
                    }
                ]
            })),
            client_body_json: None,
            body_base64: None,
            telemetry: None,
        };

        let entries = extract_gemini_file_mapping_entries(&payload);
        assert_eq!(
            entries,
            vec![
                GeminiFileMappingEntry {
                    file_name: "files/abc123".to_string(),
                    display_name: Some("root-name".to_string()),
                    mime_type: None,
                },
                GeminiFileMappingEntry {
                    file_name: "files/def456".to_string(),
                    display_name: None,
                    mime_type: Some("image/png".to_string()),
                },
                GeminiFileMappingEntry {
                    file_name: "files/ghi789".to_string(),
                    display_name: Some("third".to_string()),
                    mime_type: None,
                }
            ]
        );
    }

    #[test]
    fn extracts_gemini_file_mapping_entries_from_base64_json_body() {
        let encoded_body = base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&json!({
                "name": "base64-file"
            }))
            .expect("json should encode"),
        );
        let payload = GatewaySyncReportRequest {
            trace_id: "trace-123".to_string(),
            report_kind: "gemini_files_store_mapping".to_string(),
            report_context: None,
            status_code: 200,
            headers: BTreeMap::from([(
                "content-type".to_string(),
                "application/json; charset=utf-8".to_string(),
            )]),
            body_json: None,
            client_body_json: None,
            body_base64: Some(encoded_body),
            telemetry: None,
        };

        let entries = extract_gemini_file_mapping_entries(&payload);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file_name, "files/base64-file");
    }

    #[test]
    fn reads_report_request_id_from_context() {
        assert_eq!(
            report_request_id(Some(&json!({"request_id": "req-123"}))),
            "req-123"
        );
        assert_eq!(report_request_id(Some(&json!({"request_id": "   "}))), "-");
        assert_eq!(report_request_id(None), "-");
    }

    #[test]
    fn decides_when_local_sync_report_should_be_handled() {
        assert!(should_handle_local_sync_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "openai_chat_sync_success"
        ));
        assert!(should_handle_local_sync_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "openai_image_sync_success"
        ));
        assert!(!should_handle_local_sync_report(
            Some(&json!({"request_id": "req-123"})),
            "openai_chat_sync_success"
        ));
        assert!(!should_handle_local_sync_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "unknown_sync_kind"
        ));
    }

    #[test]
    fn decides_when_local_stream_report_should_be_handled() {
        assert!(should_handle_local_stream_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "openai_chat_stream_success"
        ));
        assert!(!should_handle_local_stream_report(
            Some(&json!({
                "request_id": "req-123",
                "provider_id": "provider-123",
                "endpoint_id": "endpoint-123",
                "key_id": "key-123"
            })),
            "openai_chat_stream_error"
        ));
    }
}
