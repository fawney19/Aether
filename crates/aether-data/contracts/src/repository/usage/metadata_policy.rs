use std::net::IpAddr;

use aether_ai_formats::api::{
    sanitize_request_path, sanitize_request_path_and_query, sanitize_request_query_string,
};
use serde_json::{Map, Value};

use crate::repository::candidates::sanitize_request_candidate_skip_reason;

use super::{
    LIVE_SESSION_METADATA_KEY, PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY,
    PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY, PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY,
    PROVIDER_REASONING_EFFORT_METADATA_KEY, PROVIDER_SERVICE_TIER_METADATA_KEY,
    REALTIME_SESSION_METADATA_KEY, REQUESTED_REASONING_EFFORT_METADATA_KEY,
    ROUTING_CANDIDATE_SKIP_REASON_METADATA_KEY, ROUTING_FAILURE_DIAGNOSTIC_METADATA_KEY,
    USAGE_AVAILABLE_METADATA_KEY, USAGE_PRICING_AVAILABLE_METADATA_KEY,
    WEBSOCKET_MODE_METADATA_KEY, WEBSOCKET_TRANSPORT_METADATA_KEY,
};

const UPSTREAM_IS_STREAM_KEY: &str = "upstream_is_stream";
const PLAN_USAGE_RESERVATION_TOKEN_KEY: &str = "plan_usage_reservation_token";
const BODY_SIZE_BASIS: &str = "serialized gateway request bodies after normalization";
pub const CANCELLED_REQUEST_FEE_METADATA_KEY: &str = "cancelled_request_fee";

pub fn cancelled_request_fee_is_billable(metadata: Option<&Value>) -> bool {
    metadata
        .and_then(|metadata| metadata.get(CANCELLED_REQUEST_FEE_METADATA_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Projects request metadata onto the persistence contract. Unknown fields and malformed values
/// are discarded instead of being recursively copied into an audit row.
pub fn sanitize_usage_request_metadata(value: Option<Value>) -> Option<Value> {
    let Value::Object(object) = value? else {
        return None;
    };
    sanitize_usage_request_metadata_object(&object)
}

pub fn sanitize_usage_request_metadata_ref(value: Option<&Value>) -> Option<Value> {
    sanitize_usage_request_metadata_object(value?.as_object()?)
}

pub fn sanitize_usage_request_metadata_object(source: &Map<String, Value>) -> Option<Value> {
    let mut target = Map::new();

    insert_token(source, &mut target, "trace_id", 128);
    insert_ip_address(source, &mut target, "client_ip");
    insert_client_family(source, &mut target);
    for key in [
        "client_requested_stream",
        UPSTREAM_IS_STREAM_KEY,
        "api_key_is_standalone",
        WEBSOCKET_MODE_METADATA_KEY,
        PLAN_USAGE_RESERVATION_DEFERRED_METADATA_KEY,
        "transport_error",
        "is_free_tier",
        CANCELLED_REQUEST_FEE_METADATA_KEY,
        USAGE_AVAILABLE_METADATA_KEY,
        USAGE_PRICING_AVAILABLE_METADATA_KEY,
    ] {
        insert_bool(source, &mut target, key);
    }
    insert_known_string(
        source,
        &mut target,
        WEBSOCKET_TRANSPORT_METADATA_KEY,
        sanitize_websocket_transport,
    );
    if let Some(session) = source
        .get(LIVE_SESSION_METADATA_KEY)
        .and_then(project_live_session)
    {
        target.insert(LIVE_SESSION_METADATA_KEY.to_string(), session);
    }
    if let Some(session) = source
        .get(REALTIME_SESSION_METADATA_KEY)
        .and_then(project_realtime_session)
    {
        target.insert(REALTIME_SESSION_METADATA_KEY.to_string(), session);
    }
    insert_uuid(source, &mut target, PLAN_USAGE_RESERVATION_TOKEN_KEY);
    insert_request_paths(source, &mut target);
    for key in [
        REQUESTED_REASONING_EFFORT_METADATA_KEY,
        PROVIDER_REASONING_EFFORT_METADATA_KEY,
    ] {
        insert_known_string(source, &mut target, key, sanitize_reasoning_effort);
    }
    for key in [
        PROVIDER_SERVICE_TIER_METADATA_KEY,
        PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY,
    ] {
        insert_known_string(source, &mut target, key, sanitize_service_tier);
    }
    insert_bounded_u64(
        source,
        &mut target,
        PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY,
        10_080,
    );
    for key in [
        "provider_request_body_base64_bytes",
        "provider_response_body_base64_bytes",
        "client_response_body_base64_bytes",
        "end_to_end_time_ms",
        "end_to_end_first_byte_time_ms",
    ] {
        insert_u64(source, &mut target, key);
    }
    insert_bounded_u64(source, &mut target, "client_response_status_code", 599);
    if let Some(body_size) = source.get("body_size").and_then(project_body_size) {
        target.insert("body_size".to_string(), body_size);
    }
    insert_transport_error_type(source, &mut target);

    for key in ["model_id", "global_model_id", "global_model_name"] {
        insert_model_token(source, &mut target, key);
    }
    if let Some(dimensions) = source.get("dimensions").and_then(project_dimensions) {
        target.insert("dimensions".to_string(), dimensions);
    }
    if let Some(dimensions) = source
        .get("billing_dimensions")
        .and_then(project_dimensions)
    {
        target.insert("billing_dimensions".to_string(), dimensions);
    }
    insert_routing_skip_reason(source, &mut target);
    if let Some(diagnostic) = source
        .get(ROUTING_FAILURE_DIAGNOSTIC_METADATA_KEY)
        .and_then(project_routing_failure_diagnostic)
    {
        target.insert(
            ROUTING_FAILURE_DIAGNOSTIC_METADATA_KEY.to_string(),
            diagnostic,
        );
    }

    for key in [
        "rate_multiplier",
        "input_price_per_1m",
        "output_price_per_1m",
        "cache_creation_price_per_1m",
        "cache_read_price_per_1m",
        "price_per_request",
    ] {
        insert_nonnegative_number(source, &mut target, key);
    }

    let billing_snapshot = source
        .get("billing_snapshot")
        .and_then(project_billing_snapshot);
    insert_schema_version_with_fallback(
        source,
        &mut target,
        "billing_snapshot_schema_version",
        billing_snapshot.as_ref(),
    );
    insert_billing_status_with_fallback(
        source,
        &mut target,
        "billing_snapshot_status",
        billing_snapshot.as_ref(),
    );
    if let Some(snapshot) = billing_snapshot {
        target.insert("billing_snapshot".to_string(), snapshot);
    }

    let settlement_snapshot = source
        .get("settlement_snapshot")
        .and_then(project_settlement_snapshot);
    insert_schema_version_with_fallback(
        source,
        &mut target,
        "settlement_snapshot_schema_version",
        settlement_snapshot.as_ref(),
    );
    if let Some(snapshot) = settlement_snapshot {
        target.insert("settlement_snapshot".to_string(), snapshot);
    }

    (!target.is_empty()).then_some(Value::Object(target))
}

fn insert_request_paths(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    let path = source
        .get("request_path")
        .and_then(Value::as_str)
        .and_then(sanitize_request_path);
    let query = source
        .get("request_query_string")
        .and_then(Value::as_str)
        .and_then(sanitize_request_query_string);
    let combined = path
        .as_deref()
        .and_then(|path| sanitize_request_path_and_query(path, query.as_deref()))
        .or_else(|| {
            source
                .get("request_path_and_query")
                .and_then(Value::as_str)
                .and_then(|value| sanitize_request_path_and_query(value, None))
        });

    insert_owned_string(target, "request_path", path);
    insert_owned_string(target, "request_query_string", query);
    insert_owned_string(target, "request_path_and_query", combined);
}

fn insert_client_family(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    let value = source
        .get("client_family")
        .and_then(Value::as_str)
        .or_else(|| {
            source
                .get("client_session_affinity")
                .and_then(Value::as_object)
                .and_then(|affinity| affinity.get("client_family"))
                .and_then(Value::as_str)
        })
        .and_then(sanitize_client_family)
        .or_else(|| {
            source
                .get("user_agent")
                .and_then(Value::as_str)
                .and_then(infer_client_family_from_user_agent)
        });
    insert_owned_string(target, "client_family", value);
}

fn sanitize_client_family(value: &str) -> Option<String> {
    known_lowercase(
        value,
        &[
            "aider",
            "anthropic_js_sdk",
            "anthropic_python_sdk",
            "cherrystudio",
            "claude_code",
            "cline",
            "codex",
            "codex_vscode",
            "continue",
            "cursor",
            "gemini_cli",
            "generic",
            "kilocode",
            "langchain",
            "llamaindex",
            "openai_js_sdk",
            "openai_python_sdk",
            "opencode",
            "openui",
            "qwen_code",
            "roo_code",
            "sdk",
            "unknown",
            "windsurf",
        ],
    )
}

fn infer_client_family_from_user_agent(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    let family = if normalized.starts_with("codex_vscode") {
        "codex_vscode"
    } else if normalized.starts_with("codex") {
        "codex"
    } else if normalized.contains("claude-code") || normalized.contains("claude_code") {
        "claude_code"
    } else if normalized.contains("opencode") {
        "opencode"
    } else if normalized.contains("geminicli") || normalized.contains("gemini-cli") {
        "gemini_cli"
    } else if normalized.contains("qwencode") {
        "qwen_code"
    } else if normalized.contains("roo-code") || normalized.contains("roocode") {
        "roo_code"
    } else if normalized.contains("kilo-code") || normalized.contains("kilocode") {
        "kilocode"
    } else if normalized.contains("cherrystudio") || normalized.contains("cherry-studio") {
        "cherrystudio"
    } else if normalized.contains("openui-agent-manager") || normalized.contains("openui") {
        "openui"
    } else if normalized.contains("cursor") {
        "cursor"
    } else if normalized.contains("windsurf") {
        "windsurf"
    } else if normalized.contains("continue") {
        "continue"
    } else if normalized.contains("cline") {
        "cline"
    } else if normalized.contains("aider") {
        "aider"
    } else if normalized.contains("langchain") {
        "langchain"
    } else if normalized.contains("llamaindex") || normalized.contains("llama-index") {
        "llamaindex"
    } else if normalized.starts_with("openai/js") {
        "openai_js_sdk"
    } else if normalized.starts_with("openai/python") {
        "openai_python_sdk"
    } else if normalized.starts_with("anthropic/js")
        || normalized.contains("anthropic-sdk-typescript")
    {
        "anthropic_js_sdk"
    } else if normalized.starts_with("anthropic/python")
        || normalized.contains("anthropic-sdk-python")
    {
        "anthropic_python_sdk"
    } else if normalized.contains("/js ") || normalized.contains("/python ") {
        "sdk"
    } else {
        return None;
    };
    Some(family.to_string())
}

fn sanitize_websocket_transport(value: &str) -> Option<String> {
    known_lowercase(
        value,
        &[
            "codex_live_direct",
            "codex_live_sideband",
            "openai_realtime",
            "openai_responses",
            "responses",
        ],
    )
}

fn sanitize_reasoning_effort(value: &str) -> Option<String> {
    known_lowercase(
        value,
        &["none", "minimal", "low", "medium", "high", "xhigh", "max"],
    )
}

fn sanitize_service_tier(value: &str) -> Option<String> {
    known_lowercase(
        value,
        &[
            "auto",
            "batch",
            "default",
            "expedited",
            "fast",
            "flex",
            "free_tier",
            "priority",
            "standard",
        ],
    )
}

fn known_lowercase(value: &str, allowed: &[&str]) -> Option<String> {
    let value = value.trim();
    if value.len() > 128 {
        return None;
    }
    let normalized = value.to_ascii_lowercase();
    allowed.contains(&normalized.as_str()).then_some(normalized)
}

fn project_live_session(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    insert_schema_version(source, &mut target, "schema_version");
    insert_known_string(
        source,
        &mut target,
        "transport",
        sanitize_live_session_transport,
    );
    insert_known_string(source, &mut target, "mode", sanitize_live_session_mode);
    insert_known_string(source, &mut target, "state", sanitize_session_state);
    insert_known_string(
        source,
        &mut target,
        "termination",
        sanitize_session_termination,
    );
    for key in [
        "elapsed_ms",
        "client_frames",
        "client_bytes",
        "upstream_frames",
        "upstream_bytes",
        "first_upstream_frame_ms",
    ] {
        insert_u64(source, &mut target, key);
    }
    insert_known_string(
        source,
        &mut target,
        "usage_state",
        sanitize_live_usage_state,
    );
    (!target.is_empty()).then_some(Value::Object(target))
}

fn project_realtime_session(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    insert_schema_version(source, &mut target, "schema_version");
    insert_known_string(
        source,
        &mut target,
        "transport",
        sanitize_realtime_session_transport,
    );
    insert_known_string(source, &mut target, "state", sanitize_session_state);
    insert_known_string(
        source,
        &mut target,
        "termination",
        sanitize_session_termination,
    );
    for key in [
        "elapsed_ms",
        "client_frames",
        "client_bytes",
        "upstream_frames",
        "upstream_bytes",
        "first_upstream_frame_ms",
        "usage_response_count",
        "cached_input_tokens",
        "input_audio_tokens",
        "output_audio_tokens",
    ] {
        insert_u64(source, &mut target, key);
    }
    insert_known_string(
        source,
        &mut target,
        "usage_state",
        sanitize_realtime_usage_state,
    );
    insert_known_string(
        source,
        &mut target,
        "pricing_state",
        sanitize_realtime_pricing_state,
    );
    insert_known_string(source, &mut target, "usage_scope", sanitize_usage_scope);
    insert_bool(source, &mut target, "input_transcription_usage_included");
    (!target.is_empty()).then_some(Value::Object(target))
}

fn sanitize_live_session_transport(value: &str) -> Option<String> {
    known_lowercase(value, &["sideband", "webrtc", "websocket"])
}

fn sanitize_realtime_session_transport(value: &str) -> Option<String> {
    known_lowercase(value, &["websocket"])
}

fn sanitize_live_session_mode(value: &str) -> Option<String> {
    known_lowercase(value, &["call_create", "direct", "sideband"])
}

fn sanitize_session_state(value: &str) -> Option<String> {
    known_lowercase(value, &["cancelled", "closed", "failed"])
}

fn sanitize_live_usage_state(value: &str) -> Option<String> {
    known_lowercase(value, &["unavailable"])
}

fn sanitize_realtime_usage_state(value: &str) -> Option<String> {
    known_lowercase(value, &["authoritative", "unavailable"])
}

fn sanitize_realtime_pricing_state(value: &str) -> Option<String> {
    known_lowercase(
        value,
        &[
            "compatible_text_usage",
            "unsupported_audio_breakdown",
            "usage_unavailable",
        ],
    )
}

fn sanitize_usage_scope(value: &str) -> Option<String> {
    known_lowercase(value, &["response_done"])
}

fn sanitize_session_termination(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() > 96 {
        return Some("other".to_string());
    }
    let normalized = value.to_ascii_lowercase();
    let allowed = [
        "admission_failed",
        "admission_plan_unavailable",
        "admission_planning_timeout",
        "admission_timeout",
        "auth_context_missing",
        "authentication_required",
        "balance_capacity_check_failed",
        "balance_capacity_rejected",
        "balance_rejected",
        "binding_failed",
        "binding_changed",
        "call_created",
        "candidate_unavailable",
        "client_close_frame",
        "client_closed",
        "client_read_failed",
        "client_write_failed",
        "connection_admission_lost",
        "connection_duration_limit",
        "control_unavailable",
        "codex_live_architecture_invalid",
        "codex_live_boundary_invalid",
        "codex_live_body_too_large",
        "codex_live_call_id_invalid",
        "codex_live_call_location_invalid",
        "codex_live_expected_session_update",
        "codex_live_initial_client_read_failed",
        "codex_live_initial_event_invalid",
        "codex_live_initial_event_must_be_text",
        "codex_live_initial_session_update_timeout",
        "codex_live_intent_invalid",
        "codex_live_media_type_unsupported",
        "codex_live_model_invalid",
        "codex_live_model_query_invalid",
        "codex_live_multipart_invalid",
        "codex_live_multipart_part_duplicate",
        "codex_live_multipart_part_unexpected",
        "codex_live_oauth_direct_unsupported",
        "codex_live_oauth_upstream_unsupported",
        "codex_live_sdp_invalid",
        "codex_live_sdp_missing",
        "codex_live_sdp_too_large",
        "codex_live_session_invalid",
        "codex_live_session_missing",
        "codex_live_session_too_large",
        "codex_live_upstream_url_invalid",
        "codex_live_upstream_url_missing",
        "downstream_response_build_failed",
        "explicit_failure",
        "finite_balance_unsupported",
        "initial_upstream_write_failed",
        "last_admin_delete_denied",
        "last_admin_update_denied",
        "location_invalid",
        "location_missing",
        "model_invalid",
        "model_missing",
        "multipart_parse_failed",
        "planning_failed",
        "pool_key_lease_lost",
        "pool_lease_lost",
        "provider_body_build_failed",
        "provider_plan_build_failed",
        "provider_plan_unavailable",
        "relay_cancelled",
        "request_rejected",
        "request_body_missing",
        "request_body_too_large",
        "request_future_cancelled",
        "response_body_unavailable",
        "route_unavailable",
        "session_close_drain_timeout",
        "sideband_attachment_conflict",
        "sideband_attachment_lease_lost",
        "sideband_attachment_lease_renewal_failed",
        "sideband_attachment_timeout",
        "sideband_attachment_unavailable",
        "sideband_binding_changed",
        "sideband_binding_disabled",
        "sideband_binding_expired",
        "sideband_binding_lookup_timeout",
        "sideband_binding_missing",
        "sideband_binding_unavailable",
        "upstream_close_frame",
        "upstream_closed",
        "upstream_connect_failed",
        "upstream_error_body_unavailable",
        "upstream_execute_failed",
        "upstream_read_failed",
        "upstream_rejected",
        "upstream_url_invalid",
        "upstream_write_failed",
        "usage_settlement_unavailable",
    ];
    if allowed.contains(&normalized.as_str()) {
        Some(normalized)
    } else {
        Some("other".to_string())
    }
}

fn insert_transport_error_type(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    let Some(value) = source
        .get("transport_error_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let normalized = value.to_ascii_lowercase();
    let value = match normalized.as_str() {
        "chatgpt_web_image_execution_unavailable"
        | "connect_timeout"
        | "execution_runtime_unavailable"
        | "first_byte_timeout"
        | "gateway_admission_timeout"
        | "grok_execution_unavailable"
        | "kiro_web_search_mcp_unavailable"
        | "local_stream_candidate_watchdog_timeout"
        | "protocol_error"
        | "proxy_error"
        | "read_timeout"
        | "tls_error"
        | "upstream_transport_error"
        | "windsurf_native_execution_unavailable" => normalized,
        _ => "other_transport_error".to_string(),
    };
    target.insert("transport_error_type".to_string(), Value::String(value));
}

fn insert_routing_skip_reason(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    let value = source
        .get(ROUTING_CANDIDATE_SKIP_REASON_METADATA_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if let Some(value) = sanitize_request_candidate_skip_reason(value) {
        target.insert(
            ROUTING_CANDIDATE_SKIP_REASON_METADATA_KEY.to_string(),
            Value::String(value),
        );
    }
}

fn project_routing_failure_diagnostic(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let kind = source
        .get("kind")
        .and_then(Value::as_str)
        .and_then(|value| {
            known_lowercase(
                value,
                &[
                    "body_rules",
                    "envelope_build",
                    "header_rules",
                    "request_body_build",
                    "request_conversion",
                    "transport_auth",
                    "url_build",
                ],
            )
        })?;
    let mut target = Map::from_iter([("kind".to_string(), Value::String(kind))]);
    if let Some(path) = source
        .get("path")
        .and_then(Value::as_str)
        .and_then(sanitize_json_path)
    {
        target.insert("path".to_string(), Value::String(path));
    }
    Some(Value::Object(target))
}

fn sanitize_json_path(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || !value.starts_with('$')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"$._[]:-".contains(&byte))
    {
        return None;
    }
    Some(value.to_string())
}

fn project_body_size(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    if source
        .get("basis")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| value == BODY_SIZE_BASIS)
    {
        target.insert(
            "basis".to_string(),
            Value::String(BODY_SIZE_BASIS.to_string()),
        );
    }
    for key in [
        "client_request_body",
        "provider_request_body",
        "provider_over_client",
    ] {
        let Some(value) = source
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 32
                    && value.bytes().all(|byte| {
                        byte.is_ascii_digit()
                            || byte == b'.'
                            || byte == b' '
                            || matches!(byte, b'B' | b'K' | b'M' | b'G' | b'x')
                    })
            })
        else {
            continue;
        };
        target.insert(key.to_string(), Value::String(value.to_string()));
    }
    (!target.is_empty()).then_some(Value::Object(target))
}

fn project_dimensions(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    for key in [
        "input_tokens",
        "effective_input_tokens",
        "output_tokens",
        "total_tokens",
        "reasoning_tokens",
        "cache_creation_tokens",
        "cache_creation_uncategorized_tokens",
        "cache_creation_ephemeral_5m_tokens",
        "cache_creation_ephemeral_1h_tokens",
        "cache_read_tokens",
        "request_count",
        "image_count",
        "image_count_unmetered",
        "total_input_context",
        "cache_ttl_minutes",
        "cache_creation_ephemeral_5m_ttl_minutes",
        "cache_creation_ephemeral_1h_ttl_minutes",
        "image_pixels",
        "windsurf_generator_entry_count",
    ] {
        insert_u64(source, &mut target, key);
    }
    for key in ["cache_storage_token_hours", "image_output_price_per_image"] {
        insert_nonnegative_number(source, &mut target, key);
    }
    for key in [
        "image_output_pricing_enabled",
        "image_output_matrix_enabled",
        "image_output_range_enabled",
    ] {
        insert_bool(source, &mut target, key);
    }
    insert_known_string(
        source,
        &mut target,
        "effective_task_type",
        sanitize_task_type,
    );
    for key in [
        "requested_processing_tier",
        "actual_processing_tier",
        "billing_processing_tier",
    ] {
        insert_nullable_known_string(source, &mut target, key, sanitize_service_tier);
    }
    insert_known_string(
        source,
        &mut target,
        "image_output_pricing_mode",
        sanitize_image_pricing_mode,
    );
    insert_known_string(source, &mut target, "image_quality", sanitize_image_quality);
    insert_known_string(
        source,
        &mut target,
        "image_output_format",
        sanitize_image_output_format,
    );
    for key in ["image_size", "image_price_key", "image_output_price_bucket"] {
        insert_dimension_token(source, &mut target, key);
    }
    (!target.is_empty()).then_some(Value::Object(target))
}

fn sanitize_task_type(value: &str) -> Option<String> {
    known_lowercase(
        value,
        &["chat", "embedding", "image", "rerank", "search", "video"],
    )
}

fn sanitize_image_pricing_mode(value: &str) -> Option<String> {
    known_lowercase(value, &["matrix", "none", "per_image", "pixel_tiers"])
}

fn sanitize_image_quality(value: &str) -> Option<String> {
    known_lowercase(value, &["auto", "low", "medium", "high", "standard", "hd"])
}

fn sanitize_image_output_format(value: &str) -> Option<String> {
    known_lowercase(value, &["jpeg", "jpg", "png", "webp"])
}

fn project_billing_snapshot(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    insert_schema_version(source, &mut target, "schema_version");
    insert_token(source, &mut target, "rule_id", 128);
    insert_billing_status(source, &mut target, "status");
    if let Some(value) = source
        .get("resolved_dimensions")
        .and_then(project_dimensions)
    {
        target.insert("resolved_dimensions".to_string(), value);
    }
    if let Some(value) = source
        .get("resolved_variables")
        .and_then(project_resolved_variables)
    {
        target.insert("resolved_variables".to_string(), value);
    }
    if let Some(value) = source
        .get("cost_breakdown")
        .and_then(project_cost_breakdown)
    {
        target.insert("cost_breakdown".to_string(), value);
    }
    insert_nonnegative_number(source, &mut target, "total_cost");
    (!target.is_empty()).then_some(Value::Object(target))
}

fn project_settlement_snapshot(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    insert_schema_version(source, &mut target, "schema_version");
    insert_billing_status(source, &mut target, "status");
    for key in ["total_cost", "actual_total_cost"] {
        insert_nonnegative_number(source, &mut target, key);
    }
    if let Some(value) = source
        .get("pricing_snapshot")
        .and_then(project_pricing_snapshot)
    {
        target.insert("pricing_snapshot".to_string(), value);
    }
    if let Some(value) = source
        .get("billing_plan_snapshot")
        .and_then(project_billing_plan_snapshot)
    {
        target.insert("billing_plan_snapshot".to_string(), value);
    }
    if let Some(value) = source
        .get("resolved_dimensions")
        .and_then(project_dimensions)
    {
        target.insert("resolved_dimensions".to_string(), value);
    }
    if let Some(value) = source
        .get("resolved_variables")
        .and_then(project_resolved_variables)
    {
        target.insert("resolved_variables".to_string(), value);
    }
    if let Some(value) = source
        .get("cost_breakdown")
        .and_then(project_cost_breakdown)
    {
        target.insert("cost_breakdown".to_string(), value);
    }
    (!target.is_empty()).then_some(Value::Object(target))
}

fn project_pricing_snapshot(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    for key in [
        "requested_processing_tier",
        "actual_processing_tier",
        "billing_processing_tier",
    ] {
        insert_nullable_known_string(source, &mut target, key, sanitize_service_tier);
    }
    for key in [
        "pricing_source",
        "tiered_pricing_source",
        "price_per_request_source",
    ] {
        insert_nullable_known_string(source, &mut target, key, sanitize_pricing_source);
    }
    for key in [
        "processing_tier_price_multiplier",
        "price_per_request",
        "rate_multiplier",
    ] {
        insert_nonnegative_number(source, &mut target, key);
    }
    insert_bool(source, &mut target, "is_free_tier");
    (!target.is_empty()).then_some(Value::Object(target))
}

fn sanitize_pricing_source(value: &str) -> Option<String> {
    known_lowercase(
        value,
        &["global_default", "mixed", "provider_override", "unpriced"],
    )
}

fn project_billing_plan_snapshot(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    insert_token(source, &mut target, "rule_id", 128);
    if let Some(value) = source.get("rule_version").and_then(safe_version_value) {
        target.insert("rule_version".to_string(), Value::String(value));
    }
    (!target.is_empty()).then_some(Value::Object(target))
}

fn project_resolved_variables(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    for key in [
        "input_price_per_1m",
        "output_price_per_1m",
        "cache_creation_price_per_1m",
        "cache_creation_ephemeral_5m_price_per_1m",
        "cache_creation_ephemeral_1h_price_per_1m",
        "cache_read_price_per_1m",
        "price_per_request",
        "image_output_price_per_image",
    ] {
        insert_nonnegative_number(source, &mut target, key);
    }
    (!target.is_empty()).then_some(Value::Object(target))
}

fn project_cost_breakdown(value: &Value) -> Option<Value> {
    let source = value.as_object()?;
    let mut target = Map::new();
    for key in [
        "input_cost",
        "output_cost",
        "cache_creation_uncategorized_cost",
        "cache_creation_ephemeral_5m_cost",
        "cache_creation_ephemeral_1h_cost",
        "cache_creation_cost",
        "cache_read_cost",
        "image_output_cost",
        "request_cost",
    ] {
        insert_nonnegative_number(source, &mut target, key);
    }
    (!target.is_empty()).then_some(Value::Object(target))
}

fn insert_schema_version_with_fallback(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    snapshot: Option<&Value>,
) {
    let value = source
        .get(key)
        .and_then(Value::as_str)
        .and_then(sanitize_schema_version)
        .or_else(|| {
            snapshot
                .and_then(Value::as_object)
                .and_then(|snapshot| snapshot.get("schema_version"))
                .and_then(Value::as_str)
                .and_then(sanitize_schema_version)
        });
    insert_owned_string(target, key, value);
}

fn insert_billing_status_with_fallback(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    snapshot: Option<&Value>,
) {
    let value = source
        .get(key)
        .and_then(Value::as_str)
        .and_then(sanitize_billing_status)
        .or_else(|| {
            snapshot
                .and_then(Value::as_object)
                .and_then(|snapshot| snapshot.get("status"))
                .and_then(Value::as_str)
                .and_then(sanitize_billing_status)
        });
    insert_owned_string(target, key, value);
}

fn insert_schema_version(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    insert_known_string(source, target, key, sanitize_schema_version);
}

fn sanitize_schema_version(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() > 32 {
        return None;
    }
    let value = value.to_ascii_lowercase();
    (!value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
    .then_some(value)
}

fn insert_billing_status(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    insert_known_string(source, target, key, sanitize_billing_status);
}

fn sanitize_billing_status(value: &str) -> Option<String> {
    known_lowercase(
        value,
        &[
            "complete",
            "incomplete",
            "legacy",
            "no_rule",
            "pending",
            "resolved",
            "void",
        ],
    )
}

fn insert_ip_address(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .and_then(|value| value.parse::<IpAddr>().ok())
    else {
        return;
    };
    target.insert(key.to_string(), Value::String(value.to_string()));
}

fn insert_uuid(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| is_canonical_uuid(value))
    else {
        return;
    };
    target.insert(key.to_string(), Value::String(value.to_ascii_lowercase()));
}

fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn insert_model_token(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._:/@+-".contains(&byte))
        })
    else {
        return;
    };
    target.insert(key.to_string(), Value::String(value.to_string()));
}

fn insert_token(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    max_len: usize,
) {
    let Some(value) = source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= max_len
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        })
    else {
        return;
    };
    target.insert(key.to_string(), Value::String(value.to_string()));
}

fn insert_dimension_token(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._:<>=".contains(&byte))
        })
    else {
        return;
    };
    target.insert(key.to_string(), Value::String(value.to_ascii_lowercase()));
}

fn insert_known_string(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    sanitize: fn(&str) -> Option<String>,
) {
    let value = source.get(key).and_then(Value::as_str).and_then(sanitize);
    insert_owned_string(target, key, value);
}

fn insert_nullable_known_string(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    sanitize: fn(&str) -> Option<String>,
) {
    match source.get(key) {
        Some(Value::Null) => {
            target.insert(key.to_string(), Value::Null);
        }
        Some(Value::String(value)) => {
            if let Some(value) = sanitize(value) {
                target.insert(key.to_string(), Value::String(value));
            }
        }
        _ => {}
    }
}

fn insert_owned_string(target: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        target.insert(key.to_string(), Value::String(value));
    }
}

fn insert_bool(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_bool) {
        target.insert(key.to_string(), Value::Bool(value));
    }
}

fn insert_u64(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_u64) {
        target.insert(key.to_string(), Value::Number(value.into()));
    }
}

fn insert_bounded_u64(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
    max: u64,
) {
    if let Some(value) = source
        .get(key)
        .and_then(Value::as_u64)
        .filter(|value| *value <= max)
    {
        target.insert(key.to_string(), Value::Number(value.into()));
    }
}

fn insert_nonnegative_number(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) {
    let Some(value) = source.get(key).filter(|value| {
        value
            .as_f64()
            .is_some_and(|value| value.is_finite() && value >= 0.0)
    }) else {
        return;
    };
    target.insert(key.to_string(), value.clone());
}

fn safe_version_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => sanitize_schema_version(value),
        Value::Number(value) => sanitize_schema_version(&value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{sanitize_usage_request_metadata, sanitize_usage_request_metadata_ref};

    #[test]
    fn persistence_projection_drops_credentials_and_free_diagnostics() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "trace_id": "trace-1",
            "client_ip": "203.0.113.8",
            "user_agent": "Bearer browser-secret",
            "client_session_affinity": {
                "client_family": "codex",
                "session_key": "tenant/session-secret"
            },
            "proxy": {"url": "https://user:pass@proxy.example"},
            "tls_fingerprint": {"ja3": "fingerprint"},
            "stage_timings_ms": {"secret_operation": 12},
            "db_timings_ms": {"query": "SELECT credential"},
            "scheduling_audit": {"key_id": "secret-key"},
            "billing_rule_snapshot": {"expression": "tenant_secret * input_tokens"},
            "routing_candidate_skip_reason": "Authorization: Bearer secret",
            "routing_failure_diagnostic": {
                "kind": "request_body_build",
                "path": "$.reasoning.summary",
                "message": "Bearer secret",
                "source": "https://user:pass@example.com",
                "client_api_format": "secret",
                "provider_api_format": "secret"
            },
            "unknown": {"authorization": "Bearer secret"}
        })))
        .expect("safe metadata should remain");

        assert_eq!(metadata["trace_id"], "trace-1");
        assert_eq!(metadata["client_ip"], "203.0.113.8");
        assert_eq!(metadata["client_family"], "codex");
        assert_eq!(
            metadata["routing_candidate_skip_reason"],
            "unclassified_skip"
        );
        assert_eq!(
            metadata["routing_failure_diagnostic"],
            json!({"kind": "request_body_build", "path": "$.reasoning.summary"})
        );
        for key in [
            "user_agent",
            "client_session_affinity",
            "proxy",
            "tls_fingerprint",
            "stage_timings_ms",
            "db_timings_ms",
            "scheduling_audit",
            "billing_rule_snapshot",
            "unknown",
        ] {
            assert!(metadata.get(key).is_none(), "{key} must not be persisted");
        }
    }

    #[test]
    fn persistence_projection_keeps_only_bounded_settlement_facts() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "plan_usage_reservation_token": "550e8400-e29b-41d4-a716-446655440000",
            "billing_snapshot": {
                "schema_version": "2.0",
                "rule_id": "__default__",
                "rule_name": "tenant secret pricing",
                "scope": "secret scope",
                "expression": "secret_variable * input_tokens",
                "resolved_dimensions": {
                    "input_tokens": 100,
                    "image_quality": "high",
                    "tenant_id": "tenant-secret"
                },
                "resolved_variables": {
                    "input_price_per_1m": 3.0,
                    "api_key": "secret"
                },
                "cost_breakdown": {
                    "input_cost": 0.0003,
                    "tenant_secret_cost": 99
                },
                "total_cost": 0.0003,
                "status": "complete",
                "tier_info": {"catalog": "secret"}
            },
            "settlement_snapshot": {
                "schema_version": "3.0",
                "pricing_snapshot": {
                    "provider_api_key_id": "key-secret",
                    "tiered_pricing": {"tenant": "secret"},
                    "billing_processing_tier": "priority",
                    "pricing_source": "provider_override",
                    "price_per_request": 0.02,
                    "rate_multiplier": 1.25,
                    "is_free_tier": false
                },
                "billing_plan_snapshot": {
                    "rule_id": "rule-1",
                    "rule_version": "7",
                    "rule_name": "secret rule",
                    "expression": "secret"
                },
                "resolved_dimensions": {"input_tokens": 100, "secret": "value"},
                "resolved_variables": {"input_price_per_1m": 3.0, "secret": 4},
                "cost_breakdown": {"input_cost": 0.0003, "secret_cost": 4},
                "total_cost": 0.0003,
                "actual_total_cost": 0.000375,
                "status": "complete",
                "calculated_at": "secret timestamp"
            },
            "billing_dimensions": {
                "input_tokens": 100,
                "image_size": "1024x1024",
                "secret_dimension": "secret"
            }
        })))
        .expect("settlement facts should remain");

        assert_eq!(
            metadata["plan_usage_reservation_token"],
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(metadata["billing_snapshot_schema_version"], "2.0");
        assert_eq!(metadata["billing_snapshot_status"], "complete");
        assert_eq!(metadata["settlement_snapshot_schema_version"], "3.0");
        assert_eq!(
            metadata.pointer("/billing_snapshot/resolved_variables/input_price_per_1m"),
            Some(&json!(3.0))
        );
        assert_eq!(
            metadata.pointer("/settlement_snapshot/billing_plan_snapshot/rule_version"),
            Some(&json!("7"))
        );
        assert_eq!(
            metadata.pointer("/settlement_snapshot/pricing_snapshot/pricing_source"),
            Some(&json!("provider_override"))
        );
        assert!(metadata.pointer("/billing_snapshot/expression").is_none());
        assert!(metadata
            .pointer("/settlement_snapshot/pricing_snapshot/provider_api_key_id")
            .is_none());
        assert!(metadata
            .pointer("/settlement_snapshot/pricing_snapshot/tiered_pricing")
            .is_none());
        assert!(metadata
            .pointer("/billing_dimensions/secret_dimension")
            .is_none());
    }

    #[test]
    fn persistence_projection_rejects_malformed_identifiers_and_paths() {
        assert!(sanitize_usage_request_metadata(Some(json!({
            "client_ip": "127.0.0.1, 10.0.0.1",
            "trace_id": "Bearer secret",
            "plan_usage_reservation_token": "server-token",
            "request_path": "/install/sensitive-code",
            "request_query_string": "key=secret&alt=sse",
            "routing_failure_diagnostic": {
                "kind": "request_body_build",
                "path": "$['api_key=secret']"
            }
        })))
        .is_some_and(|metadata| {
            metadata.get("client_ip").is_none()
                && metadata.get("trace_id").is_none()
                && metadata.get("plan_usage_reservation_token").is_none()
                && metadata["request_path"] == "/install/[redacted]"
                && metadata["request_query_string"] == "alt=sse"
                && metadata
                    .pointer("/routing_failure_diagnostic/path")
                    .is_none()
        }));
    }

    #[test]
    fn persistence_projection_keeps_only_known_body_size_basis() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "body_size": {
                "basis": " serialized gateway request bodies after normalization ",
                "client_request_body": "1 KB",
                "provider_request_body": "4 KB",
                "untrusted": "drop-me"
            }
        })))
        .expect("body size metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "body_size": {
                    "basis": "serialized gateway request bodies after normalization",
                    "client_request_body": "1 KB",
                    "provider_request_body": "4 KB"
                }
            })
        );
        assert!(sanitize_usage_request_metadata(Some(json!({
            "body_size": {"basis": "untrusted basis"}
        })))
        .is_none());
    }

    #[test]
    fn borrowed_and_owned_projection_match() {
        let value = json!({
            "trace_id": "trace-1",
            "client_ip": "2001:db8::1",
            "billing_dimensions": {"input_tokens": 5}
        });
        assert_eq!(
            sanitize_usage_request_metadata_ref(Some(&value)),
            sanitize_usage_request_metadata(Some(value))
        );
    }

    #[test]
    fn user_agent_is_reduced_to_a_controlled_client_family() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "user_agent": "codex_vscode/0.131.0-alpha.9 (Windows; x86_64; tenant=secret)"
        })))
        .expect("recognized client family should remain");
        assert_eq!(metadata, json!({"client_family": "codex_vscode"}));

        assert!(sanitize_usage_request_metadata(Some(json!({
            "user_agent": "private-client/1.0 account-secret"
        })))
        .is_none());
    }

    #[test]
    fn persistence_projection_keeps_bounded_websocket_session_facts() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "websocket_mode": true,
            "websocket_transport": "CODEX_LIVE_DIRECT",
            "usage_available": false,
            "usage_pricing_available": false,
            "live_session": {
                "schema_version": "1",
                "transport": "websocket",
                "mode": "direct",
                "state": "cancelled",
                "termination": "client_close_frame",
                "elapsed_ms": 1200,
                "client_frames": 4,
                "client_bytes": 128,
                "upstream_frames": 3,
                "upstream_bytes": 96,
                "first_upstream_frame_ms": 42,
                "usage_state": "unavailable",
                "authorization": "Bearer secret",
                "nested": {"secret": "drop-me"}
            },
            "realtime_session": {
                "schema_version": "1",
                "transport": "websocket",
                "state": "failed",
                "termination": "Bearer secret",
                "elapsed_ms": 900,
                "client_frames": 2,
                "client_bytes": 64,
                "upstream_frames": 1,
                "upstream_bytes": 32,
                "usage_state": "authoritative",
                "pricing_state": "unsupported_audio_breakdown",
                "usage_scope": "response_done",
                "input_transcription_usage_included": false,
                "usage_response_count": 1,
                "cached_input_tokens": 5,
                "input_audio_tokens": 6,
                "output_audio_tokens": 7,
                "authorization": "Bearer secret",
                "nested": {"secret": "drop-me"}
            }
        })))
        .expect("bounded session metadata should remain");

        assert_eq!(metadata["websocket_mode"], true);
        assert_eq!(metadata["websocket_transport"], "codex_live_direct");
        assert_eq!(metadata["usage_available"], false);
        assert_eq!(metadata["usage_pricing_available"], false);
        assert_eq!(
            metadata["live_session"],
            json!({
                "schema_version": "1",
                "transport": "websocket",
                "mode": "direct",
                "state": "cancelled",
                "termination": "client_close_frame",
                "elapsed_ms": 1200,
                "client_frames": 4,
                "client_bytes": 128,
                "upstream_frames": 3,
                "upstream_bytes": 96,
                "first_upstream_frame_ms": 42,
                "usage_state": "unavailable"
            })
        );
        assert_eq!(
            metadata["realtime_session"],
            json!({
                "schema_version": "1",
                "transport": "websocket",
                "state": "failed",
                "termination": "other",
                "elapsed_ms": 900,
                "client_frames": 2,
                "client_bytes": 64,
                "upstream_frames": 1,
                "upstream_bytes": 32,
                "usage_state": "authoritative",
                "pricing_state": "unsupported_audio_breakdown",
                "usage_scope": "response_done",
                "input_transcription_usage_included": false,
                "usage_response_count": 1,
                "cached_input_tokens": 5,
                "input_audio_tokens": 6,
                "output_audio_tokens": 7
            })
        );
        assert!(metadata.pointer("/live_session/authorization").is_none());
        assert!(metadata.pointer("/live_session/nested").is_none());
        assert!(metadata
            .pointer("/realtime_session/authorization")
            .is_none());
        assert!(metadata.pointer("/realtime_session/nested").is_none());
    }
}
