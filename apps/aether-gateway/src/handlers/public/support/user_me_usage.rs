use std::collections::{BTreeMap, BTreeSet};

use aether_ai_serving::UPSTREAM_IS_STREAM_KEY;
use aether_billing::{
    normalize_input_tokens_for_billing, normalize_total_input_context_for_cache_hit_rate,
};
use aether_data_contracts::repository::{
    candidates::{RequestCandidateStatus, StoredRequestCandidate},
    usage::{
        StoredRequestUsageAudit, StoredUsageBreakdownSummaryRow, StoredUsageDailySummary,
        UsageAuditKeywordSearchQuery, UsageAuditListQuery, UsageBreakdownGroupBy,
        UsageBreakdownSummaryQuery, UsageCacheAffinityIntervalGroupBy,
        UsageCacheAffinityIntervalQuery, UsageDashboardSummaryQuery,
    },
};
use axum::{
    body::Body,
    http,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use serde_json::{json, Value};

use crate::GatewayError;

use super::{
    admin_stats_bad_request_response, build_auth_error_response, build_auth_wallet_summary_payload,
    parse_bounded_u32, query_param_value, resolve_authenticated_local_user, round_to,
    unix_secs_to_rfc3339, AdminStatsTimeRange, AppState, GatewayPublicRequestContext,
};

const USERS_ME_USAGE_DATA_UNAVAILABLE_DETAIL: &str = "用户用量数据暂不可用";

fn build_users_me_usage_reader_unavailable_response() -> Response<Body> {
    build_auth_error_response(
        http::StatusCode::SERVICE_UNAVAILABLE,
        USERS_ME_USAGE_DATA_UNAVAILABLE_DETAIL,
        false,
    )
}

fn parse_users_me_usage_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer between 1 and 200".to_string())?;
            if (1..=200).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("limit must be an integer between 1 and 200".to_string())
            }
        }
        None => Ok(100),
    }
}

fn parse_users_me_usage_offset(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "offset") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "offset must be a non-negative integer".to_string()),
        None => Ok(0),
    }
}

fn parse_users_me_usage_hours(query: Option<&str>) -> Result<u32, String> {
    match query_param_value(query, "hours") {
        Some(value) => parse_bounded_u32("hours", &value, 1, 720),
        None => Ok(24),
    }
}

fn parse_users_me_usage_timeline_limit(query: Option<&str>) -> Result<usize, String> {
    match query_param_value(query, "limit") {
        Some(value) => {
            let parsed = value
                .parse::<usize>()
                .map_err(|_| "limit must be an integer between 100 and 20000".to_string())?;
            if (100..=20_000).contains(&parsed) {
                Ok(parsed)
            } else {
                Err("limit must be an integer between 100 and 20000".to_string())
            }
        }
        None => Ok(2_000),
    }
}

fn parse_users_me_usage_ids(query: Option<&str>) -> Option<BTreeSet<String>> {
    let ids = query_param_value(query, "ids")?;
    let values = ids
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    (!values.is_empty()).then_some(values)
}

fn users_me_usage_cache_creation_tokens(item: &StoredRequestUsageAudit) -> u64 {
    let classified = item
        .cache_creation_ephemeral_5m_input_tokens
        .saturating_add(item.cache_creation_ephemeral_1h_input_tokens);
    if item.cache_creation_input_tokens == 0 && classified > 0 {
        classified
    } else {
        item.cache_creation_input_tokens
    }
}

fn users_me_usage_total_input_context(item: &StoredRequestUsageAudit) -> u64 {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref());
    let input_tokens = i64::try_from(item.input_tokens).unwrap_or(i64::MAX);
    let cache_creation_tokens =
        i64::try_from(users_me_usage_cache_creation_tokens(item)).unwrap_or(i64::MAX);
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_total_input_context_for_cache_hit_rate(
        api_format,
        input_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    ) as u64
}

fn users_me_usage_effective_input_tokens(item: &StoredRequestUsageAudit) -> u64 {
    let api_format = item
        .endpoint_api_format
        .as_deref()
        .or(item.api_format.as_deref());
    let input_tokens = i64::try_from(item.input_tokens).unwrap_or(i64::MAX);
    let cache_read_tokens = i64::try_from(item.cache_read_input_tokens).unwrap_or(i64::MAX);
    normalize_input_tokens_for_billing(api_format, input_tokens, cache_read_tokens) as u64
}

fn users_me_usage_effective_unix_secs(item: &StoredRequestUsageAudit) -> u64 {
    item.finalized_at_unix_secs
        .unwrap_or(item.created_at_unix_ms)
}

fn users_me_usage_cache_hit_rate(total_input_context: u64, cache_read_tokens: u64) -> f64 {
    if total_input_context == 0 {
        0.0
    } else {
        round_to(
            cache_read_tokens as f64 / total_input_context as f64 * 100.0,
            2,
        )
    }
}

fn users_me_usage_api_key_name(
    item: &StoredRequestUsageAudit,
    api_key_names: &BTreeMap<String, String>,
    auth_api_key_reader_available: bool,
) -> Option<String> {
    item.api_key_id
        .as_ref()
        .and_then(|api_key_id| api_key_names.get(api_key_id))
        .cloned()
        .or_else(|| {
            (!auth_api_key_reader_available)
                .then(|| item.api_key_name.clone())
                .flatten()
        })
}

fn parse_users_me_usage_search_keywords(search: &str) -> Vec<String> {
    search
        .split_whitespace()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

fn build_users_me_usage_api_key_payload(
    item: &StoredRequestUsageAudit,
    api_key_names: &BTreeMap<String, String>,
    auth_api_key_reader_available: bool,
) -> serde_json::Value {
    let api_key_name =
        users_me_usage_api_key_name(item, api_key_names, auth_api_key_reader_available);
    match item.api_key_id.as_deref() {
        Some(api_key_id) => json!({
            "id": api_key_id,
            "name": api_key_name.clone(),
            "display": api_key_name.unwrap_or_else(|| api_key_id.to_string()),
        }),
        None => serde_json::Value::Null,
    }
}

fn users_me_usage_request_body_stream_flag(item: &StoredRequestUsageAudit) -> Option<bool> {
    item.request_body
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|body| body.get("stream"))
        .and_then(serde_json::Value::as_bool)
}

fn users_me_usage_api_format_defaults_to_non_stream(item: &StoredRequestUsageAudit) -> bool {
    let api_format = item
        .api_format
        .as_deref()
        .or(item.endpoint_api_format.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(value) = api_format else {
        return false;
    };
    matches!(
        crate::ai_serving::normalize_api_format_alias(value).as_str(),
        "openai:chat"
            | "openai:responses"
            | "openai:responses:compact"
            | "openai:image"
            | "claude:messages"
    )
}

fn users_me_usage_request_body_implies_default_non_stream(item: &StoredRequestUsageAudit) -> bool {
    let Some(body) = item
        .request_body
        .as_ref()
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    !body.contains_key("stream") && users_me_usage_api_format_defaults_to_non_stream(item)
}

fn users_me_usage_headers_stream_flag(headers: Option<&serde_json::Value>) -> Option<bool> {
    let object = headers.and_then(serde_json::Value::as_object)?;
    let raw = object
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .and_then(|(_, value)| match value {
            serde_json::Value::String(text) => Some(text.as_str()),
            serde_json::Value::Array(values) => values.iter().find_map(serde_json::Value::as_str),
            _ => None,
        })?
        .trim();
    if raw.is_empty() {
        return None;
    }

    let normalized = raw.to_ascii_lowercase();
    Some(
        normalized.contains("event-stream")
            || normalized.contains("eventstream")
            || normalized.contains("x-ndjson"),
    )
}

fn users_me_usage_body_is_sse_capture(value: Option<&serde_json::Value>) -> bool {
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        return false;
    };
    object
        .get("chunks")
        .and_then(serde_json::Value::as_array)
        .is_some()
        && object
            .get("metadata")
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get("stream"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

fn users_me_usage_infer_client_stream_from_captured_bodies(
    item: &StoredRequestUsageAudit,
) -> Option<bool> {
    let provider_stream = users_me_usage_body_is_sse_capture(item.response_body.as_ref());
    let client_stream = users_me_usage_body_is_sse_capture(item.client_response_body.as_ref());
    if client_stream {
        Some(true)
    } else if provider_stream && item.client_response_body.is_some() {
        Some(false)
    } else {
        None
    }
}

fn users_me_usage_infer_upstream_stream_from_captured_bodies(
    item: &StoredRequestUsageAudit,
) -> Option<bool> {
    let provider_stream = users_me_usage_body_is_sse_capture(item.response_body.as_ref());
    let client_stream = users_me_usage_body_is_sse_capture(item.client_response_body.as_ref());
    if provider_stream {
        Some(true)
    } else if client_stream && item.response_body.is_some() {
        Some(false)
    } else {
        None
    }
}

fn users_me_usage_client_is_stream(item: &StoredRequestUsageAudit) -> bool {
    item.request_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get("client_requested_stream"))
        .and_then(serde_json::Value::as_bool)
        .or_else(|| users_me_usage_request_body_stream_flag(item))
        .or_else(|| users_me_usage_headers_stream_flag(item.client_response_headers.as_ref()))
        .or_else(|| users_me_usage_request_body_implies_default_non_stream(item).then_some(false))
        .or_else(|| users_me_usage_infer_client_stream_from_captured_bodies(item))
        .unwrap_or(item.is_stream)
}

fn users_me_usage_upstream_is_stream(item: &StoredRequestUsageAudit) -> bool {
    item.request_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get(UPSTREAM_IS_STREAM_KEY))
        .and_then(serde_json::Value::as_bool)
        .or_else(|| users_me_usage_headers_stream_flag(item.response_headers.as_ref()))
        .or_else(|| users_me_usage_infer_upstream_stream_from_captured_bodies(item))
        .unwrap_or(item.is_stream)
}

fn users_me_usage_metadata_string<'a>(
    item: &'a StoredRequestUsageAudit,
    key: &str,
) -> Option<&'a str> {
    item.request_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|metadata| metadata.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn infer_client_family_from_user_agent(user_agent: &str) -> Option<&'static str> {
    let normalized = user_agent.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    if normalized.starts_with("codex_vscode") {
        return Some("codex_vscode");
    }
    if normalized.starts_with("codex") {
        return Some("codex");
    }
    if normalized.contains("claude-code") || normalized.contains("claude_code") {
        return Some("claude_code");
    }
    if normalized.contains("opencode") {
        return Some("opencode");
    }
    if normalized.contains("geminicli") || normalized.contains("gemini-cli") {
        return Some("gemini_cli");
    }
    if normalized.contains("qwencode") {
        return Some("qwen_code");
    }
    if normalized.contains("roo-code") || normalized.contains("roocode") {
        return Some("roo_code");
    }
    if normalized.contains("kilo-code") || normalized.contains("kilocode") {
        return Some("kilocode");
    }
    if normalized.contains("cherrystudio") || normalized.contains("cherry-studio") {
        return Some("cherrystudio");
    }
    if normalized.contains("openui-agent-manager") || normalized.contains("openui") {
        return Some("openui");
    }
    if normalized.contains("cursor") {
        return Some("cursor");
    }
    if normalized.contains("windsurf") {
        return Some("windsurf");
    }
    if normalized.contains("continue") {
        return Some("continue");
    }
    if normalized.contains("cline") {
        return Some("cline");
    }
    if normalized.contains("aider") {
        return Some("aider");
    }
    if normalized.contains("langchain") {
        return Some("langchain");
    }
    if normalized.contains("llamaindex") || normalized.contains("llama-index") {
        return Some("llamaindex");
    }
    if normalized.starts_with("openai/js") {
        return Some("openai_js_sdk");
    }
    if normalized.starts_with("openai/python") {
        return Some("openai_python_sdk");
    }
    if normalized.starts_with("anthropic/js") || normalized.contains("anthropic-sdk-typescript") {
        return Some("anthropic_js_sdk");
    }
    if normalized.starts_with("anthropic/python") || normalized.contains("anthropic-sdk-python") {
        return Some("anthropic_python_sdk");
    }
    if normalized.contains("/js ") || normalized.contains("/python ") {
        return Some("sdk");
    }
    Some("unknown")
}

fn users_me_usage_client_family(item: &StoredRequestUsageAudit) -> Option<&str> {
    item.client_family
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            item.request_metadata
                .as_ref()
                .and_then(serde_json::Value::as_object)
                .and_then(|metadata| {
                    metadata
                        .get("client_session_affinity")
                        .and_then(serde_json::Value::as_object)
                        .and_then(|affinity| affinity.get("client_family"))
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| {
                            metadata
                                .get("client_family")
                                .and_then(serde_json::Value::as_str)
                        })
                })
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .or_else(|| {
            users_me_usage_metadata_string(item, "user_agent")
                .and_then(infer_client_family_from_user_agent)
        })
}

fn build_users_me_usage_record_payload(
    item: &StoredRequestUsageAudit,
    include_actual_cost: bool,
    api_key_names: &BTreeMap<String, String>,
    auth_api_key_reader_available: bool,
) -> serde_json::Value {
    let input_price_per_1m = item.settlement_input_price_per_1m();
    let output_price_per_1m = item.settlement_output_price_per_1m();
    let cache_creation_price_per_1m = item.settlement_cache_creation_price_per_1m();
    let cache_read_price_per_1m = item.settlement_cache_read_price_per_1m();
    let cache_creation_input_tokens = users_me_usage_cache_creation_tokens(item);
    let rate_multiplier = item.settlement_rate_multiplier();
    let client_is_stream = users_me_usage_client_is_stream(item);
    let upstream_is_stream = users_me_usage_upstream_is_stream(item);
    let mut payload = json!({
        "id": item.id,
        "model": item.model,
        "target_model": serde_json::Value::Null,
        "api_format": item.api_format,
        "endpoint_api_format": item.endpoint_api_format,
        "has_format_conversion": item.has_format_conversion,
        "input_tokens": item.input_tokens,
        "effective_input_tokens": users_me_usage_effective_input_tokens(item),
        "output_tokens": item.output_tokens,
        "total_tokens": item.total_tokens,
        "cost": round_to(item.total_cost_usd, 6),
        "response_time_ms": item.response_time_ms,
        "first_byte_time_ms": item.first_byte_time_ms,
        "is_stream": item.is_stream,
        "upstream_is_stream": upstream_is_stream,
        "client_requested_stream": client_is_stream,
        "client_is_stream": client_is_stream,
        "client_family": users_me_usage_client_family(item),
        "client_ip": users_me_usage_metadata_string(item, "client_ip"),
        "user_agent": users_me_usage_metadata_string(item, "user_agent"),
        "request_path": users_me_usage_metadata_string(item, "request_path"),
        "request_path_and_query": users_me_usage_metadata_string(item, "request_path_and_query"),
        "status": item.status,
        "has_fallback": item.has_fallback(),
        "created_at": unix_secs_to_rfc3339(item.created_at_unix_ms),
        "cache_creation_input_tokens": cache_creation_input_tokens,
        "cache_creation_ephemeral_5m_input_tokens": item.cache_creation_ephemeral_5m_input_tokens,
        "cache_creation_ephemeral_1h_input_tokens": item.cache_creation_ephemeral_1h_input_tokens,
        "cache_read_input_tokens": item.cache_read_input_tokens,
        "status_code": item.status_code,
        "error_message": item.error_message,
        "input_price_per_1m": input_price_per_1m,
        "output_price_per_1m": output_price_per_1m,
        "cache_creation_price_per_1m": cache_creation_price_per_1m,
        "cache_read_price_per_1m": cache_read_price_per_1m,
        "api_key": build_users_me_usage_api_key_payload(
            item,
            api_key_names,
            auth_api_key_reader_available,
        ),
    });

    if item.target_model.is_some() {
        payload["target_model"] = json!(item.target_model.clone());
    }
    if let Some(reasoning_effort) = item.provider_reasoning_effort() {
        payload["reasoning_effort"] = json!(reasoning_effort);
    }
    if let Some(service_tier) = item.provider_service_tier() {
        payload["service_tier"] = json!(service_tier);
    }
    if include_actual_cost {
        payload["actual_cost"] = json!(round_to(item.actual_total_cost_usd, 6));
        payload["rate_multiplier"] = json!(rate_multiplier);
    }
    payload
}

fn build_users_me_usage_active_payload(item: &StoredRequestUsageAudit) -> serde_json::Value {
    let cache_creation_input_tokens = users_me_usage_cache_creation_tokens(item);
    let client_is_stream = users_me_usage_client_is_stream(item);
    let upstream_is_stream = users_me_usage_upstream_is_stream(item);
    let mut payload = json!({
        "id": item.id,
        "status": item.status,
        "input_tokens": item.input_tokens,
        "effective_input_tokens": users_me_usage_effective_input_tokens(item),
        "output_tokens": item.output_tokens,
        "cache_creation_input_tokens": cache_creation_input_tokens,
        "cache_creation_ephemeral_5m_input_tokens": item.cache_creation_ephemeral_5m_input_tokens,
        "cache_creation_ephemeral_1h_input_tokens": item.cache_creation_ephemeral_1h_input_tokens,
        "cache_read_input_tokens": item.cache_read_input_tokens,
        "cost": round_to(item.total_cost_usd, 6),
        "actual_cost": round_to(item.actual_total_cost_usd, 6),
        "rate_multiplier": item.settlement_rate_multiplier(),
        "response_time_ms": item.response_time_ms,
        "first_byte_time_ms": item.first_byte_time_ms,
        "updated_at": unix_secs_to_rfc3339(item.updated_at_unix_secs),
        "response_time_updated_at": users_me_usage_response_time_updated_at(item),
        "status_code": item.status_code,
        "error_message": item.error_message,
        "api_format": item.api_format,
        "endpoint_api_format": item.endpoint_api_format,
        "is_stream": item.is_stream,
        "upstream_is_stream": upstream_is_stream,
        "client_requested_stream": client_is_stream,
        "client_is_stream": client_is_stream,
        "has_format_conversion": item.has_format_conversion,
        "client_family": users_me_usage_client_family(item),
        "client_ip": users_me_usage_metadata_string(item, "client_ip"),
        "user_agent": users_me_usage_metadata_string(item, "user_agent"),
        "target_model": item.target_model,
        "has_fallback": item.has_fallback(),
    });
    if item.api_format.is_none() {
        payload
            .as_object_mut()
            .expect("object")
            .remove("api_format");
    }
    if item.endpoint_api_format.is_none() {
        payload
            .as_object_mut()
            .expect("object")
            .remove("endpoint_api_format");
    }
    if item.target_model.is_none() {
        payload
            .as_object_mut()
            .expect("object")
            .remove("target_model");
    }
    if let Some(reasoning_effort) = item.provider_reasoning_effort() {
        payload["reasoning_effort"] = json!(reasoning_effort);
    }
    if let Some(service_tier) = item.provider_service_tier() {
        payload["service_tier"] = json!(service_tier);
    }
    payload
}

fn users_me_usage_response_time_updated_at(item: &StoredRequestUsageAudit) -> Option<String> {
    item.response_time_ms?;
    if matches!(item.status.as_str(), "pending" | "streaming")
        && item.updated_at_unix_secs <= item.created_at_unix_ms
    {
        return None;
    }
    unix_secs_to_rfc3339(item.updated_at_unix_secs)
}

fn unix_millis_to_rfc3339(unix_ms: u64) -> Option<String> {
    let secs = i64::try_from(unix_ms / 1_000).ok()?;
    let nanos = u32::try_from(unix_ms % 1_000)
        .ok()?
        .saturating_mul(1_000_000);
    chrono::DateTime::<Utc>::from_timestamp(secs, nanos).map(|timestamp| timestamp.to_rfc3339())
}

fn users_me_usage_current_candidate(
    candidates: &[StoredRequestCandidate],
) -> Option<&StoredRequestCandidate> {
    candidates
        .iter()
        .filter(|candidate| {
            !matches!(
                candidate.status,
                RequestCandidateStatus::Available
                    | RequestCandidateStatus::Unused
                    | RequestCandidateStatus::Skipped
            )
        })
        .max_by_key(|candidate| {
            (
                candidate.candidate_index,
                candidate.retry_index,
                candidate
                    .started_at_unix_ms
                    .or(candidate.finished_at_unix_ms)
                    .unwrap_or(candidate.created_at_unix_ms),
            )
        })
}

fn users_me_usage_terminal_candidate_state_override(
    candidates: &[StoredRequestCandidate],
) -> Option<Value> {
    let candidate = users_me_usage_current_candidate(candidates)?;

    let status = match candidate.status {
        RequestCandidateStatus::Success => "completed",
        RequestCandidateStatus::Failed => "failed",
        RequestCandidateStatus::Cancelled => "cancelled",
        _ => return None,
    };
    let latency_ms = candidate.latency_ms.or_else(|| {
        Some(
            candidate
                .finished_at_unix_ms?
                .saturating_sub(candidate.started_at_unix_ms?),
        )
    });
    let mut payload = json!({ "status": status });
    if let Some(latency_ms) = latency_ms {
        payload["response_time_ms"] = json!(latency_ms);
        if let Some(response_time_updated_at) = candidate
            .finished_at_unix_ms
            .or_else(|| {
                candidate
                    .started_at_unix_ms
                    .map(|started_at| started_at.saturating_add(latency_ms))
            })
            .and_then(unix_millis_to_rfc3339)
        {
            payload["response_time_updated_at"] = json!(response_time_updated_at);
        }
    }
    if let Some(status_code) = candidate.status_code {
        payload["status_code"] = json!(status_code);
    }
    if let Some(error_message) = candidate.error_message.as_ref() {
        payload["error_message"] = json!(error_message);
    }
    Some(payload)
}

async fn resolve_users_me_usage_active_state_overrides_by_request_id(
    state: &AppState,
    items: &[StoredRequestUsageAudit],
) -> Result<BTreeMap<String, Value>, GatewayError> {
    if !state.has_request_candidate_data_reader() || items.is_empty() {
        return Ok(BTreeMap::new());
    }

    let active_request_ids = items
        .iter()
        .filter(|item| matches!(item.status.as_str(), "pending" | "streaming"))
        .map(|item| item.request_id.clone())
        .collect::<BTreeSet<_>>();
    let mut overrides = BTreeMap::new();
    for request_id in active_request_ids {
        let candidates = state
            .read_request_candidates_by_request_id(&request_id)
            .await?;
        if let Some(override_payload) =
            users_me_usage_terminal_candidate_state_override(&candidates)
        {
            overrides.insert(request_id, override_payload);
        }
    }
    Ok(overrides)
}

fn users_me_usage_is_failed(item: &StoredRequestUsageAudit) -> bool {
    let has_failure_signal = item.status_code.is_some_and(|value| value >= 400)
        || item
            .error_message
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    let status = item.status.trim().to_ascii_lowercase();
    if status.is_empty() {
        return has_failure_signal;
    }
    match status.as_str() {
        "completed" | "cancelled" => false,
        "pending" | "streaming" => has_failure_signal,
        "failed" => true,
        _ => false,
    }
}

fn build_users_me_usage_summary_by_model(
    rows: &[StoredUsageBreakdownSummaryRow],
    include_actual_cost: bool,
) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            let mut value = json!({
                "model": row.group_key,
                "requests": row.request_count,
                "input_tokens": row.input_tokens,
                "effective_input_tokens": row.effective_input_tokens,
                "output_tokens": row.output_tokens,
                "total_tokens": row.total_tokens,
                "cache_read_tokens": row.cache_read_tokens,
                "cache_creation_tokens": row.cache_creation_tokens,
                "cache_creation_ephemeral_5m_tokens": row.cache_creation_ephemeral_5m_tokens,
                "cache_creation_ephemeral_1h_tokens": row.cache_creation_ephemeral_1h_tokens,
                "total_input_context": row.total_input_context,
                "cache_hit_rate": users_me_usage_cache_hit_rate(
                    row.total_input_context,
                    row.cache_read_tokens,
                ),
                "total_cost_usd": round_to(row.total_cost_usd, 6),
            });
            if include_actual_cost {
                value["actual_total_cost_usd"] = json!(round_to(row.actual_total_cost_usd, 6));
            }
            value
        })
        .collect()
}

fn build_users_me_usage_summary_by_provider(
    rows: &[StoredUsageBreakdownSummaryRow],
) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            json!({
                "provider": row.group_key,
                "requests": row.request_count,
                "effective_input_tokens": row.effective_input_tokens,
                "total_tokens": row.total_tokens,
                "total_input_context": row.total_input_context,
                "output_tokens": row.output_tokens,
                "cache_read_tokens": row.cache_read_tokens,
                "cache_creation_tokens": row.cache_creation_tokens,
                "cache_creation_ephemeral_5m_tokens": row.cache_creation_ephemeral_5m_tokens,
                "cache_creation_ephemeral_1h_tokens": row.cache_creation_ephemeral_1h_tokens,
                "cache_hit_rate": users_me_usage_cache_hit_rate(
                    row.total_input_context,
                    row.cache_read_tokens,
                ),
                "total_cost_usd": round_to(row.total_cost_usd, 6),
                "success_rate": if row.request_count == 0 {
                    100.0
                } else {
                    round_to(row.success_count as f64 / row.request_count as f64 * 100.0, 2)
                },
                "avg_response_time_ms": if row.response_time_samples == 0 {
                    0.0
                } else {
                    round_to(row.response_time_sum_ms / row.response_time_samples as f64, 2)
                },
            })
        })
        .collect()
}

fn build_users_me_usage_summary_by_api_format(
    rows: &[StoredUsageBreakdownSummaryRow],
) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            json!({
                "api_format": row.group_key,
                "request_count": row.request_count,
                "total_tokens": row.total_tokens,
                "effective_input_tokens": row.effective_input_tokens,
                "total_input_context": row.total_input_context,
                "output_tokens": row.output_tokens,
                "cache_read_tokens": row.cache_read_tokens,
                "cache_creation_tokens": row.cache_creation_tokens,
                "cache_creation_ephemeral_5m_tokens": row.cache_creation_ephemeral_5m_tokens,
                "cache_creation_ephemeral_1h_tokens": row.cache_creation_ephemeral_1h_tokens,
                "cache_hit_rate": users_me_usage_cache_hit_rate(
                    row.total_input_context,
                    row.cache_read_tokens,
                ),
                "total_cost_usd": round_to(row.total_cost_usd, 6),
                "avg_response_time_ms": if row.overall_response_time_samples == 0 {
                    0.0
                } else {
                    round_to(
                        row.overall_response_time_sum_ms
                            / row.overall_response_time_samples as f64,
                        2,
                    )
                },
            })
        })
        .collect()
}

async fn load_users_me_usage_by_ids(
    state: &AppState,
    requested_ids: &BTreeSet<String>,
    user_id: &str,
) -> Result<Vec<StoredRequestUsageAudit>, GatewayError> {
    let usage_ids = requested_ids.iter().cloned().collect::<Vec<_>>();
    let mut items = state.list_request_usage_by_ids(&usage_ids).await?;
    items.retain(|item| item.user_id.as_deref() == Some(user_id));
    Ok(items)
}

async fn resolve_users_me_api_key_names(
    state: &AppState,
    items: &[StoredRequestUsageAudit],
) -> Result<BTreeMap<String, String>, GatewayError> {
    if !state.has_auth_api_key_data_reader() {
        return Ok(BTreeMap::new());
    }

    let api_key_ids = items
        .iter()
        .filter_map(|item| item.api_key_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if api_key_ids.is_empty() {
        return Ok(BTreeMap::new());
    }

    state.resolve_auth_api_key_names_by_ids(&api_key_ids).await
}

async fn resolve_users_me_search_api_key_context(
    state: &AppState,
    user_id: &str,
    keywords: &[String],
) -> Result<(BTreeMap<String, String>, Vec<Vec<String>>), GatewayError> {
    if !state.has_auth_api_key_data_reader() {
        return Ok((BTreeMap::new(), Vec::new()));
    }

    let user_ids = [user_id.to_string()];
    let export_records = state
        .list_auth_api_key_export_records_by_user_ids(&user_ids)
        .await?;
    let api_key_names = export_records
        .iter()
        .filter_map(|record| {
            record
                .name
                .clone()
                .map(|name| (record.api_key_id.clone(), name))
        })
        .collect::<BTreeMap<_, _>>();
    let matched_api_key_ids_by_keyword = keywords
        .iter()
        .map(|keyword| {
            export_records
                .iter()
                .filter(|record| {
                    record
                        .name
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(keyword)
                })
                .map(|record| record.api_key_id.clone())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    Ok((api_key_names, matched_api_key_ids_by_keyword))
}

pub(super) async fn handle_users_me_usage_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_users_me_usage_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = request_context.request_query_string.as_deref();
    let time_range = match AdminStatsTimeRange::resolve_optional(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };
    let search = query_param_value(query, "search");
    let limit = match parse_users_me_usage_limit(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };
    let offset = match parse_users_me_usage_offset(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };

    // When no time range is specified, default to 7 days to avoid full-table scans.
    let effective_time_range = time_range.or_else(|| {
        let today = Utc::now().date_naive();
        let start_date = today
            .checked_sub_signed(chrono::Duration::days(6))
            .unwrap_or(today);
        Some(AdminStatsTimeRange {
            start_date,
            end_date: today,
            tz_offset_minutes: 0,
        })
    });

    let include_actual_cost = auth.user.role.eq_ignore_ascii_case("admin");
    let auth_api_key_reader_available = state.has_auth_api_key_data_reader();
    let mut usage_summary =
        aether_data_contracts::repository::usage::StoredUsageDashboardSummary::default();
    let mut summary_by_model = Vec::<StoredUsageBreakdownSummaryRow>::new();
    let mut summary_by_provider = Vec::<StoredUsageBreakdownSummaryRow>::new();
    let mut summary_by_api_format = Vec::<StoredUsageBreakdownSummaryRow>::new();
    let mut api_key_names = BTreeMap::new();
    let mut total_record_count = 0usize;
    let mut record_items = Vec::<StoredRequestUsageAudit>::new();

    if let Some((created_from_unix_secs, created_until_unix_secs)) = effective_time_range
        .as_ref()
        .and_then(AdminStatsTimeRange::to_unix_bounds)
    {
        usage_summary = match state
            .summarize_dashboard_usage(&UsageDashboardSummaryQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                user_id: Some(auth.user.id.clone()),
            })
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user usage summary lookup failed: {err:?}"),
                    false,
                );
            }
        };
        summary_by_model = match state
            .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                user_id: Some(auth.user.id.clone()),
                provider_name: None,
                group_by: UsageBreakdownGroupBy::Model,
            })
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user usage model breakdown lookup failed: {err:?}"),
                    false,
                );
            }
        };
        if include_actual_cost {
            summary_by_provider = match state
                .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
                    created_from_unix_secs,
                    created_until_unix_secs,
                    user_id: Some(auth.user.id.clone()),
                    provider_name: None,
                    group_by: UsageBreakdownGroupBy::Provider,
                })
                .await
            {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user usage provider breakdown lookup failed: {err:?}"),
                        false,
                    );
                }
            };
        }
        summary_by_api_format = match state
            .summarize_usage_breakdown(&UsageBreakdownSummaryQuery {
                created_from_unix_secs,
                created_until_unix_secs,
                user_id: Some(auth.user.id.clone()),
                provider_name: None,
                group_by: UsageBreakdownGroupBy::ApiFormat,
            })
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user usage api_format breakdown lookup failed: {err:?}"),
                    false,
                );
            }
        };

        let active_search = search.as_deref().filter(|value| !value.trim().is_empty());
        if let Some(search) = active_search {
            let keywords = parse_users_me_usage_search_keywords(search);
            let matched_api_key_ids_by_keyword;
            (api_key_names, matched_api_key_ids_by_keyword) =
                match resolve_users_me_search_api_key_context(state, &auth.user.id, &keywords).await
                {
                    Ok(value) => value,
                    Err(err) => {
                        return build_auth_error_response(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!("user api key search context lookup failed: {err:?}"),
                            false,
                        );
                    }
                };
            let keyword_query = UsageAuditKeywordSearchQuery {
                created_from_unix_secs: Some(created_from_unix_secs),
                created_until_unix_secs: Some(created_until_unix_secs),
                user_id: Some(auth.user.id.clone()),
                provider_name: None,
                model: None,
                api_format: None,
                client_family: None,
                exclude_unknown_model_or_provider: false,
                statuses: None,
                is_stream: None,
                error_only: false,
                keywords,
                matched_user_ids_by_keyword: Vec::new(),
                auth_user_reader_available: false,
                matched_api_key_ids_by_keyword,
                auth_api_key_reader_available,
                username_keyword: None,
                matched_user_ids_for_username: Vec::new(),
                limit: None,
                offset: None,
                newest_first: true,
            };
            total_record_count = match state
                .count_usage_audits_by_keyword_search(&keyword_query)
                .await
            {
                Ok(value) => usize::try_from(value).unwrap_or(usize::MAX),
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user usage search count lookup failed: {err:?}"),
                        false,
                    );
                }
            };
            record_items = match state
                .list_usage_audits_by_keyword_search(&UsageAuditKeywordSearchQuery {
                    limit: Some(limit),
                    offset: Some(offset),
                    ..keyword_query
                })
                .await
            {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user usage search lookup failed: {err:?}"),
                        false,
                    );
                }
            };
        } else {
            total_record_count = match state
                .count_usage_audits(&UsageAuditListQuery {
                    created_from_unix_secs: Some(created_from_unix_secs),
                    created_until_unix_secs: Some(created_until_unix_secs),
                    user_id: Some(auth.user.id.clone()),
                    provider_name: None,
                    model: None,
                    api_format: None,
                    client_family: None,
                    exclude_unknown_model_or_provider: false,
                    statuses: None,
                    is_stream: None,
                    error_only: false,
                    limit: None,
                    offset: None,
                    newest_first: true,
                })
                .await
            {
                Ok(value) => usize::try_from(value).unwrap_or(usize::MAX),
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user usage count lookup failed: {err:?}"),
                        false,
                    );
                }
            };
            record_items = match state
                .list_usage_audits(&UsageAuditListQuery {
                    created_from_unix_secs: Some(created_from_unix_secs),
                    created_until_unix_secs: Some(created_until_unix_secs),
                    user_id: Some(auth.user.id.clone()),
                    provider_name: None,
                    model: None,
                    api_format: None,
                    client_family: None,
                    exclude_unknown_model_or_provider: false,
                    statuses: None,
                    is_stream: None,
                    error_only: false,
                    limit: Some(limit),
                    offset: Some(offset),
                    newest_first: true,
                })
                .await
            {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user usage records lookup failed: {err:?}"),
                        false,
                    );
                }
            };
            api_key_names = match resolve_users_me_api_key_names(state, &record_items).await {
                Ok(value) => value,
                Err(err) => {
                    return build_auth_error_response(
                        http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("user api key name lookup failed: {err:?}"),
                        false,
                    );
                }
            };
        }
    }

    let total_requests = usage_summary.total_requests;
    let total_input_tokens = usage_summary.input_tokens;
    let total_output_tokens = usage_summary.output_tokens;
    let total_tokens = usage_summary.total_tokens;
    let total_cost = round_to(usage_summary.total_cost_usd, 6);
    let total_actual_cost = round_to(usage_summary.actual_total_cost_usd, 6);
    let avg_response_time = if usage_summary.response_time_samples == 0 {
        0.0
    } else {
        round_to(
            usage_summary.response_time_sum_ms
                / usage_summary.response_time_samples as f64
                / 1000.0,
            2,
        )
    };

    let records = record_items
        .into_iter()
        .map(|item| {
            build_users_me_usage_record_payload(
                &item,
                include_actual_cost,
                &api_key_names,
                auth_api_key_reader_available,
            )
        })
        .collect::<Vec<_>>();

    let wallet = state
        .read_wallet_snapshot_for_auth(&auth.user.id, "", false)
        .await
        .ok()
        .flatten();

    let mut payload = json!({
        "total_requests": total_requests,
        "total_input_tokens": total_input_tokens,
        "total_output_tokens": total_output_tokens,
        "total_tokens": total_tokens,
        "total_cost": total_cost,
        "avg_response_time": avg_response_time,
        "billing": build_auth_wallet_summary_payload(wallet.as_ref()),
        "summary_by_model": build_users_me_usage_summary_by_model(&summary_by_model, include_actual_cost),
        "summary_by_api_format": build_users_me_usage_summary_by_api_format(&summary_by_api_format),
        "pagination": {
            "total": total_record_count,
            "limit": limit,
            "offset": offset,
            "has_more": offset.saturating_add(limit) < total_record_count,
        },
        "records": records,
    });
    if include_actual_cost {
        payload["total_actual_cost"] = json!(total_actual_cost);
        payload["summary_by_provider"] = json!(build_users_me_usage_summary_by_provider(
            &summary_by_provider
        ));
    }
    Json(payload).into_response()
}

pub(super) async fn handle_users_me_usage_active_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_users_me_usage_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let ids = parse_users_me_usage_ids(request_context.request_query_string.as_deref());
    // When polling for active (pending/streaming) requests without specific ids,
    // limit to the last 1 hour to avoid scanning all historical records.
    let items = match ids.as_ref() {
        Some(ids) => match load_users_me_usage_by_ids(state, ids, &auth.user.id).await {
            Ok(mut value) => {
                value.sort_by(|left, right| {
                    right
                        .created_at_unix_ms
                        .cmp(&left.created_at_unix_ms)
                        .then_with(|| left.id.cmp(&right.id))
                });
                value
            }
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user active usage lookup failed: {err:?}"),
                    false,
                );
            }
        },
        None => match state
            .list_usage_audits(&UsageAuditListQuery {
                created_from_unix_secs: Some(Utc::now().timestamp().saturating_sub(3600) as u64),
                created_until_unix_secs: None,
                user_id: Some(auth.user.id.clone()),
                provider_name: None,
                model: None,
                api_format: None,
                client_family: None,
                exclude_unknown_model_or_provider: false,
                statuses: Some(vec!["pending".to_string(), "streaming".to_string()]),
                is_stream: None,
                error_only: false,
                limit: Some(50),
                offset: None,
                newest_first: true,
            })
            .await
        {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user active usage lookup failed: {err:?}"),
                    false,
                );
            }
        },
    };

    let items = if ids.is_some() {
        items
    } else {
        items
            .into_iter()
            .filter(|item| !users_me_usage_is_failed(item))
            .collect::<Vec<_>>()
    };
    let active_state_overrides =
        match resolve_users_me_usage_active_state_overrides_by_request_id(state, &items).await {
            Ok(value) => value,
            Err(err) => {
                return build_auth_error_response(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("user active usage candidate lookup failed: {err:?}"),
                    false,
                );
            }
        };

    Json(json!({
        "requests": items
            .iter()
            .map(|item| {
                let mut payload = build_users_me_usage_active_payload(item);
                if let (Some(payload), Some(overrides)) = (
                    payload.as_object_mut(),
                    active_state_overrides
                        .get(&item.request_id)
                        .and_then(Value::as_object),
                ) {
                    for (key, value) in overrides {
                        payload.insert(key.clone(), value.clone());
                    }
                }
                payload
            })
            .collect::<Vec<_>>(),
    }))
    .into_response()
}

pub(super) async fn handle_users_me_usage_interval_timeline_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_users_me_usage_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let query = request_context.request_query_string.as_deref();
    let hours = match parse_users_me_usage_hours(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };
    let limit = match parse_users_me_usage_timeline_limit(query) {
        Ok(value) => value,
        Err(detail) => return admin_stats_bad_request_response(detail),
    };
    let now_unix_secs = u64::try_from(Utc::now().timestamp()).unwrap_or_default();
    let created_from_unix_secs = now_unix_secs.saturating_sub(u64::from(hours) * 3600);

    let intervals = match state
        .list_usage_cache_affinity_intervals(&UsageCacheAffinityIntervalQuery {
            created_from_unix_secs,
            created_until_unix_secs: now_unix_secs.saturating_add(1),
            group_by: UsageCacheAffinityIntervalGroupBy::User,
            user_id: Some(auth.user.id.clone()),
            api_key_id: None,
        })
        .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user interval timeline lookup failed: {err:?}"),
                false,
            );
        }
    };

    let mut points = Vec::new();
    for row in intervals {
        points.push(json!({
            "x": unix_secs_to_rfc3339(row.created_at_unix_secs),
            "y": round_to(row.interval_minutes, 2),
            "model": row.model,
        }));
        if points.len() >= limit {
            break;
        }
    }

    Json(json!({
        "analysis_period_hours": hours,
        "total_points": points.len(),
        "points": points,
    }))
    .into_response()
}

pub(super) async fn handle_users_me_usage_heatmap_get(
    state: &AppState,
    request_context: &GatewayPublicRequestContext,
    headers: &http::HeaderMap,
) -> Response<Body> {
    if !state.has_usage_data_reader() {
        return build_users_me_usage_reader_unavailable_response();
    }

    let auth = match resolve_authenticated_local_user(state, request_context, headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let today = Utc::now().date_naive();
    let start_date = today
        .checked_sub_signed(chrono::Duration::days(364))
        .unwrap_or(today);
    let Some(start_of_day) = start_date.and_hms_opt(0, 0, 0) else {
        return build_auth_error_response(
            http::StatusCode::INTERNAL_SERVER_ERROR,
            "heatmap start date is invalid",
            false,
        );
    };
    let created_from_unix_secs = u64::try_from(
        chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(start_of_day, chrono::Utc)
            .timestamp(),
    )
    .unwrap_or_default();

    let summaries = match build_usage_heatmap_summaries(
        state,
        created_from_unix_secs,
        start_date,
        today,
        Some(auth.user.id.as_str()),
    )
    .await
    {
        Ok(value) => value,
        Err(err) => {
            return build_auth_error_response(
                http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("user heatmap lookup failed: {err:?}"),
                false,
            );
        }
    };

    let include_actual_cost = auth.user.role.eq_ignore_ascii_case("admin");
    let grouped: std::collections::HashMap<String, _> =
        summaries.into_iter().map(|s| (s.date.clone(), s)).collect();

    let mut max_requests = 0_u64;
    let mut active_days = 0_u64;
    let mut cursor = start_date;
    let mut days = Vec::new();
    while cursor <= today {
        let date_str = cursor.to_string();
        let (requests, total_tokens, total_cost, actual_total_cost) =
            if let Some(s) = grouped.get(&date_str) {
                (
                    s.requests,
                    s.total_tokens,
                    s.total_cost_usd,
                    s.actual_total_cost_usd,
                )
            } else {
                (0, 0, 0.0, 0.0)
            };
        max_requests = max_requests.max(requests);
        if requests > 0 {
            active_days = active_days.saturating_add(1);
        }
        let mut day = json!({
            "date": date_str,
            "requests": requests,
            "total_tokens": total_tokens,
            "total_cost": round_to(total_cost, 6),
        });
        if include_actual_cost {
            day["actual_total_cost"] = json!(round_to(actual_total_cost, 6));
        }
        days.push(day);
        cursor = cursor
            .checked_add_signed(chrono::Duration::days(1))
            .unwrap_or(today + chrono::Duration::days(1));
    }

    Json(json!({
        "start_date": start_date.to_string(),
        "end_date": today.to_string(),
        "total_days": days.len(),
        "active_days": active_days,
        "max_requests": max_requests,
        "days": days,
    }))
    .into_response()
}

async fn build_usage_heatmap_summaries(
    state: &AppState,
    created_from_unix_secs: u64,
    _start_date: chrono::NaiveDate,
    _today: chrono::NaiveDate,
    user_id: Option<&str>,
) -> Result<Vec<StoredUsageDailySummary>, GatewayError> {
    let query = aether_data_contracts::repository::usage::UsageDailyHeatmapQuery {
        created_from_unix_secs,
        user_id: user_id.map(ToOwned::to_owned),
        admin_mode: user_id.is_none(),
    };
    let mut summaries = state.summarize_usage_daily_heatmap(&query).await?;
    summaries.sort_by(|left, right| left.date.cmp(&right.date));
    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aether_data_contracts::repository::{
        candidates::{RequestCandidateStatus, StoredRequestCandidate},
        usage::StoredRequestUsageAudit,
    };
    use serde_json::json;

    use super::{
        build_users_me_usage_active_payload, build_users_me_usage_record_payload,
        users_me_usage_client_is_stream, users_me_usage_is_failed,
        users_me_usage_terminal_candidate_state_override, users_me_usage_upstream_is_stream,
    };

    fn sample_usage(status: &str) -> StoredRequestUsageAudit {
        StoredRequestUsageAudit::new(
            "usage-1".to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            "OpenAI".to_string(),
            "gpt-5".to_string(),
            None,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            Some("openai:chat".to_string()),
            Some("openai".to_string()),
            Some("chat".to_string()),
            false,
            false,
            10,
            20,
            30,
            0.0,
            0.0,
            Some(200),
            None,
            None,
            Some(120),
            None,
            status.to_string(),
            "settled".to_string(),
            100,
            101,
            Some(102),
        )
        .expect("usage should build")
    }

    fn sample_candidate(
        status: RequestCandidateStatus,
        status_code: Option<i32>,
        latency_ms: Option<i32>,
        error_message: Option<&str>,
    ) -> StoredRequestCandidate {
        StoredRequestCandidate::new(
            "candidate-1".to_string(),
            "req-1".to_string(),
            Some("user-1".to_string()),
            Some("api-key-1".to_string()),
            Some("alice".to_string()),
            Some("default".to_string()),
            0,
            0,
            Some("provider-1".to_string()),
            Some("endpoint-1".to_string()),
            Some("provider-key-1".to_string()),
            status,
            None,
            false,
            status_code,
            None,
            error_message.map(str::to_string),
            latency_ms,
            None,
            None,
            None,
            1_000,
            Some(1_000),
            Some(10_210),
        )
        .expect("candidate should build")
    }

    #[test]
    fn user_usage_record_payload_rehydrates_cache_creation_total_from_classified_fields() {
        let item = StoredRequestUsageAudit {
            cache_creation_input_tokens: 0,
            cache_creation_ephemeral_5m_input_tokens: 9,
            cache_creation_ephemeral_1h_input_tokens: 11,
            ..sample_usage("completed")
        };

        let payload = build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);

        assert_eq!(payload["cache_creation_input_tokens"], 20);
        assert_eq!(payload["cache_creation_ephemeral_5m_input_tokens"], 9);
        assert_eq!(payload["cache_creation_ephemeral_1h_input_tokens"], 11);
    }

    #[test]
    fn user_usage_active_payload_rehydrates_cache_creation_total_from_classified_fields() {
        let item = StoredRequestUsageAudit {
            cache_creation_input_tokens: 0,
            cache_creation_ephemeral_5m_input_tokens: 4,
            cache_creation_ephemeral_1h_input_tokens: 6,
            ..sample_usage("streaming")
        };

        let payload = build_users_me_usage_active_payload(&item);

        assert_eq!(payload["cache_creation_input_tokens"], 10);
        assert_eq!(payload["cache_creation_ephemeral_5m_input_tokens"], 4);
        assert_eq!(payload["cache_creation_ephemeral_1h_input_tokens"], 6);
    }

    #[test]
    fn user_usage_active_override_uses_terminal_candidate_latency() {
        let candidate = sample_candidate(
            RequestCandidateStatus::Success,
            Some(200),
            Some(9_210),
            None,
        );

        let payload =
            users_me_usage_terminal_candidate_state_override(&[candidate]).expect("override");

        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["response_time_ms"], 9_210);
        assert_eq!(payload["status_code"], 200);
        assert_eq!(
            payload["response_time_updated_at"],
            "1970-01-01T00:00:10.210+00:00"
        );
    }

    #[test]
    fn user_usage_active_override_ignores_terminal_candidate_when_newer_attempt_is_live() {
        let failed = sample_candidate(
            RequestCandidateStatus::Failed,
            Some(503),
            Some(1_000),
            Some("first attempt failed"),
        );
        let mut streaming = sample_candidate(RequestCandidateStatus::Streaming, None, None, None);
        streaming.candidate_index = 1;
        streaming.started_at_unix_ms = Some(10_500);
        streaming.finished_at_unix_ms = None;

        let payload = users_me_usage_terminal_candidate_state_override(&[failed, streaming]);

        assert!(payload.is_none());
    }

    #[test]
    fn user_usage_payload_keeps_claude_effective_input_when_cache_read_is_large() {
        let item = StoredRequestUsageAudit {
            provider_name: "Claude".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            api_format: Some("claude:messages".to_string()),
            api_family: Some("claude".to_string()),
            endpoint_api_format: Some("claude:messages".to_string()),
            provider_api_family: Some("claude".to_string()),
            input_tokens: 4941,
            output_tokens: 973,
            total_tokens: 59474,
            cache_creation_input_tokens: 687,
            cache_read_input_tokens: 52873,
            ..sample_usage("completed")
        };

        let payload = build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);

        assert_eq!(payload["input_tokens"], 4941);
        assert_eq!(payload["effective_input_tokens"], 4941);
        assert_eq!(payload["cache_creation_input_tokens"], 687);
        assert_eq!(payload["cache_read_input_tokens"], 52873);
    }

    #[test]
    fn user_usage_active_pending_with_failure_signal_is_not_active() {
        let item = StoredRequestUsageAudit {
            status_code: Some(503),
            error_message: Some("upstream failed".to_string()),
            ..sample_usage("pending")
        };

        assert!(users_me_usage_is_failed(&item));
    }

    #[test]
    fn user_usage_payloads_include_symmetric_stream_fields() {
        let item = StoredRequestUsageAudit {
            is_stream: true,
            request_metadata: Some(json!({
                "client_requested_stream": false
            })),
            ..sample_usage("completed")
        };

        assert!(!users_me_usage_client_is_stream(&item));

        let record_payload =
            build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);
        assert_eq!(record_payload["is_stream"], true);
        assert_eq!(record_payload["upstream_is_stream"], true);
        assert_eq!(record_payload["client_requested_stream"], false);
        assert_eq!(record_payload["client_is_stream"], false);

        let active_payload = build_users_me_usage_active_payload(&item);
        assert_eq!(active_payload["is_stream"], true);
        assert_eq!(active_payload["upstream_is_stream"], true);
        assert_eq!(active_payload["client_requested_stream"], false);
        assert_eq!(active_payload["client_is_stream"], false);
    }

    #[test]
    fn user_usage_payload_infers_client_family_from_user_agent() {
        let item = StoredRequestUsageAudit {
            request_metadata: Some(json!({
                "client_ip": "192.168.0.28",
                "user_agent": "codex_vscode/0.131.0-alpha.9 (Windows 10.0.26200; x86_64)"
            })),
            ..sample_usage("completed")
        };

        let record_payload =
            build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);
        let active_payload = build_users_me_usage_active_payload(&item);

        assert_eq!(record_payload["client_family"], "codex_vscode");
        assert_eq!(record_payload["client_ip"], "192.168.0.28");
        assert_eq!(active_payload["client_family"], "codex_vscode");
        assert_eq!(active_payload["client_ip"], "192.168.0.28");
    }

    #[test]
    fn user_usage_payload_labels_openai_js_user_agent_as_sdk() {
        let item = StoredRequestUsageAudit {
            request_metadata: Some(json!({
                "user_agent": "OpenAI/JS 6.34.0"
            })),
            ..sample_usage("completed")
        };

        let record_payload =
            build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);

        assert_eq!(record_payload["client_family"], "openai_js_sdk");
    }

    #[test]
    fn user_usage_stream_inference_falls_back_to_request_body_stream_flag() {
        let item = StoredRequestUsageAudit {
            is_stream: true,
            request_body: Some(json!({
                "model": "gpt-5.4",
                "stream": false
            })),
            ..sample_usage("completed")
        };

        assert!(!users_me_usage_client_is_stream(&item));

        let record_payload =
            build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);
        assert_eq!(record_payload["is_stream"], true);
        assert_eq!(record_payload["upstream_is_stream"], true);
        assert_eq!(record_payload["client_requested_stream"], false);
        assert_eq!(record_payload["client_is_stream"], false);
    }

    #[test]
    fn user_usage_stream_defaults_to_non_stream_for_openai_responses_request_body_without_flag() {
        let item = StoredRequestUsageAudit {
            is_stream: true,
            api_format: Some("openai:responses".to_string()),
            request_body: Some(json!({
                "model": "gpt-5.4",
                "input": [{"role": "user", "content": "hi"}],
                "store": false
            })),
            ..sample_usage("completed")
        };

        assert!(!users_me_usage_client_is_stream(&item));

        let record_payload =
            build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);
        assert_eq!(record_payload["is_stream"], true);
        assert_eq!(record_payload["upstream_is_stream"], true);
        assert_eq!(record_payload["client_requested_stream"], false);
        assert_eq!(record_payload["client_is_stream"], false);

        let active_payload = build_users_me_usage_active_payload(&item);
        assert_eq!(active_payload["is_stream"], true);
        assert_eq!(active_payload["upstream_is_stream"], true);
        assert_eq!(active_payload["client_requested_stream"], false);
        assert_eq!(active_payload["client_is_stream"], false);
    }

    #[test]
    fn user_usage_upstream_stream_prefers_request_metadata_flag() {
        let item = StoredRequestUsageAudit {
            is_stream: false,
            request_metadata: Some(json!({
                "client_requested_stream": false,
                "upstream_is_stream": true
            })),
            ..sample_usage("completed")
        };

        let record_payload =
            build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);
        assert_eq!(record_payload["is_stream"], false);
        assert_eq!(record_payload["upstream_is_stream"], true);
        assert_eq!(record_payload["client_requested_stream"], false);
        assert_eq!(record_payload["client_is_stream"], false);

        let active_payload = build_users_me_usage_active_payload(&item);
        assert_eq!(active_payload["is_stream"], false);
        assert_eq!(active_payload["upstream_is_stream"], true);
        assert_eq!(active_payload["client_requested_stream"], false);
        assert_eq!(active_payload["client_is_stream"], false);
    }

    #[test]
    fn user_usage_stream_modes_fall_back_to_captured_response_bodies_when_request_metadata_is_missing(
    ) {
        let item = StoredRequestUsageAudit {
            is_stream: true,
            response_body: Some(json!({
                "chunks": [
                    {"type": "response.created"},
                    {"type": "response.output_text.delta", "delta": "Hello"}
                ],
                "metadata": {
                    "stream": true,
                    "stored_chunks": 2,
                    "total_chunks": 2
                }
            })),
            client_response_body: Some(json!({
                "id": "resp-1",
                "object": "response",
                "status": "completed",
                "output": []
            })),
            ..sample_usage("completed")
        };

        assert!(!users_me_usage_client_is_stream(&item));
        assert!(users_me_usage_upstream_is_stream(&item));

        let record_payload =
            build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);
        assert_eq!(record_payload["is_stream"], true);
        assert_eq!(record_payload["upstream_is_stream"], true);
        assert_eq!(record_payload["client_requested_stream"], false);
        assert_eq!(record_payload["client_is_stream"], false);

        let active_payload = build_users_me_usage_active_payload(&item);
        assert_eq!(active_payload["is_stream"], true);
        assert_eq!(active_payload["upstream_is_stream"], true);
        assert_eq!(active_payload["client_requested_stream"], false);
        assert_eq!(active_payload["client_is_stream"], false);
    }

    #[test]
    fn user_usage_stream_modes_fall_back_to_captured_response_headers_when_bodies_are_detached() {
        let item = StoredRequestUsageAudit {
            is_stream: true,
            response_headers: Some(json!({
                "content-type": "text/event-stream; charset=utf-8"
            })),
            client_response_headers: Some(json!({
                "content-type": "application/json"
            })),
            response_body_ref: Some("usage://request/req-1/response_body".to_string()),
            client_response_body_ref: Some(
                "usage://request/req-1/client_response_body".to_string(),
            ),
            ..sample_usage("completed")
        };

        assert!(!users_me_usage_client_is_stream(&item));
        assert!(users_me_usage_upstream_is_stream(&item));

        let record_payload =
            build_users_me_usage_record_payload(&item, false, &BTreeMap::new(), false);
        assert_eq!(record_payload["is_stream"], true);
        assert_eq!(record_payload["upstream_is_stream"], true);
        assert_eq!(record_payload["client_requested_stream"], false);
        assert_eq!(record_payload["client_is_stream"], false);

        let active_payload = build_users_me_usage_active_payload(&item);
        assert_eq!(active_payload["is_stream"], true);
        assert_eq!(active_payload["upstream_is_stream"], true);
        assert_eq!(active_payload["client_requested_stream"], false);
        assert_eq!(active_payload["client_is_stream"], false);
    }
}
