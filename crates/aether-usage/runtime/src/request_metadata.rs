use aether_ai_formats::api::{
    sanitize_request_path, sanitize_request_path_and_query, sanitize_request_query_string,
};
use aether_ai_formats::UPSTREAM_IS_STREAM_KEY;
use aether_contracts::ExecutionPlan;
use aether_data_contracts::repository::usage::{
    extract_provider_actual_service_tier_from_response,
    extract_provider_reasoning_effort_from_body, extract_provider_service_tier_from_body,
    normalize_provider_service_tier, resolve_provider_cache_ttl_minutes, UsageBodyCaptureState,
    PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY, PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY,
    PROVIDER_REASONING_EFFORT_METADATA_KEY, PROVIDER_SERVICE_TIER_METADATA_KEY,
    REQUESTED_REASONING_EFFORT_METADATA_KEY,
};
use serde_json::{json, Map, Value};

const MAX_USAGE_REQUEST_METADATA_DEPTH: usize = 32;
const MAX_USAGE_REQUEST_METADATA_NODES: usize = 4_000;
const MAX_USAGE_REQUEST_METADATA_BYTES: usize = 16 * 1024;
const MAX_USAGE_REQUEST_METADATA_STRING_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestBodyDerivedFactsAction {
    Refresh,
    Preserve,
    Clear,
}

pub(crate) fn request_body_derived_facts_action(
    request_body: Option<&Value>,
    state: Option<UsageBodyCaptureState>,
) -> RequestBodyDerivedFactsAction {
    // A typed `none` marker is produced by the final capture attempt. It must take precedence over
    // a body value that may have survived from an earlier candidate. A missing marker (`None`)
    // remains compatible with legacy events, where a present body is still authoritative.
    if state == Some(UsageBodyCaptureState::None) {
        return RequestBodyDerivedFactsAction::Clear;
    }

    if let Some(request_body) = request_body {
        if matches!(
            state,
            Some(
                UsageBodyCaptureState::Truncated
                    | UsageBodyCaptureState::Disabled
                    | UsageBodyCaptureState::Unavailable
            )
        ) || request_body.as_object().is_some_and(|body| {
            body.get("truncated").and_then(Value::as_bool) == Some(true)
                && body.get("reason").and_then(Value::as_str) == Some("body_capture_limit_exceeded")
        }) {
            return RequestBodyDerivedFactsAction::Preserve;
        }
        return RequestBodyDerivedFactsAction::Refresh;
    }

    match state {
        Some(
            UsageBodyCaptureState::Inline
            | UsageBodyCaptureState::Reference
            | UsageBodyCaptureState::Truncated
            | UsageBodyCaptureState::Disabled
            | UsageBodyCaptureState::Unavailable,
        ) => RequestBodyDerivedFactsAction::Preserve,
        Some(UsageBodyCaptureState::None) | None => RequestBodyDerivedFactsAction::Clear,
    }
}

pub(crate) fn build_usage_request_metadata_seed(
    _plan: &ExecutionPlan,
    context: Option<&Map<String, Value>>,
) -> Option<Value> {
    let mut metadata = Map::new();
    if let Some(context) = context {
        copy_allowed_metadata_fields(context, &mut metadata);
    }
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

pub(crate) fn merge_usage_request_metadata(
    base: Option<Value>,
    override_value: Option<Value>,
) -> Option<Value> {
    let mut metadata = Map::new();
    if let Some(Value::Object(base)) = base.as_ref() {
        copy_allowed_metadata_fields(base, &mut metadata);
    }
    if let Some(Value::Object(override_object)) = override_value.as_ref() {
        copy_allowed_metadata_fields(override_object, &mut metadata);
    }
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

pub(crate) fn merge_usage_request_metadata_owned(
    base: Option<Value>,
    override_value: Option<Value>,
) -> Option<Value> {
    let mut metadata = match base {
        Some(Value::Object(base)) => base,
        _ => Map::new(),
    };
    if let Some(Value::Object(override_object)) = override_value {
        move_allowed_metadata_fields(override_object, &mut metadata);
    }
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

pub(crate) fn sanitize_usage_request_metadata(value: Option<Value>) -> Option<Value> {
    let Value::Object(object) = value? else {
        return None;
    };

    let mut filtered = Map::new();
    move_allowed_metadata_fields(object, &mut filtered);

    (!filtered.is_empty()).then_some(Value::Object(filtered))
}

pub(crate) fn retain_first_byte_request_metadata(value: Option<Value>) -> Option<Value> {
    let Value::Object(mut metadata) = sanitize_usage_request_metadata(value)? else {
        return None;
    };
    metadata.retain(|key, _| {
        matches!(
            key.as_str(),
            "trace_id"
                | "client_ip"
                | "user_agent"
                | "client_family"
                | "client_requested_stream"
                | "upstream_is_stream"
                | "client_session_affinity"
                | "api_key_is_standalone"
                | "request_path"
                | "request_query_string"
                | "request_path_and_query"
                | "requested_reasoning_effort"
                | "provider_reasoning_effort"
                | "provider_service_tier"
                | "provider_cache_ttl_minutes"
                | "model_id"
                | "global_model_id"
                | "global_model_name"
                | "proxy"
        )
    });
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

pub(crate) fn sanitize_usage_request_metadata_ref(value: Option<&Value>) -> Option<Value> {
    let object = value.and_then(Value::as_object)?;

    let mut filtered = Map::new();
    copy_allowed_metadata_fields(object, &mut filtered);

    (!filtered.is_empty()).then_some(Value::Object(filtered))
}

pub(crate) fn attach_client_request_body_metadata(
    metadata: Option<Value>,
    request_body: Option<&Value>,
) -> Option<Value> {
    let request_body_is_object = request_body.and_then(Value::as_object).is_some();
    let reasoning_effort = extract_provider_reasoning_effort_from_body(request_body);
    if !request_body_is_object && reasoning_effort.is_none() {
        return metadata;
    }

    let mut object = match metadata {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    if request_body_is_object {
        object.remove(REQUESTED_REASONING_EFFORT_METADATA_KEY);
    }
    if let Some(reasoning_effort) = reasoning_effort {
        object.insert(
            REQUESTED_REASONING_EFFORT_METADATA_KEY.to_string(),
            Value::String(reasoning_effort),
        );
    }
    (!object.is_empty()).then_some(Value::Object(object))
}

pub(crate) fn attach_provider_request_body_metadata(
    metadata: Option<Value>,
    provider_api_format: Option<&str>,
    provider_model: Option<&str>,
    source_model: Option<&str>,
    provider_request_body: Option<&Value>,
) -> Option<Value> {
    let provider_body_is_object = provider_request_body.and_then(Value::as_object).is_some();
    let reasoning_effort = extract_provider_reasoning_effort_from_body(provider_request_body);
    let service_tier = extract_provider_service_tier_from_body(provider_request_body);
    let cache_ttl_minutes = resolve_provider_cache_ttl_minutes(
        provider_api_format,
        provider_model,
        source_model,
        provider_request_body,
    );
    if !provider_body_is_object
        && reasoning_effort.is_none()
        && service_tier.is_none()
        && cache_ttl_minutes.is_none()
    {
        return metadata;
    }
    let mut object = match metadata {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    if provider_body_is_object {
        object.remove(PROVIDER_REASONING_EFFORT_METADATA_KEY);
        object.remove(PROVIDER_SERVICE_TIER_METADATA_KEY);
        object.remove(PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY);
    }
    if let Some(reasoning_effort) = reasoning_effort {
        object.insert(
            PROVIDER_REASONING_EFFORT_METADATA_KEY.to_string(),
            Value::String(reasoning_effort),
        );
    }
    if let Some(service_tier) = service_tier {
        object.insert(
            PROVIDER_SERVICE_TIER_METADATA_KEY.to_string(),
            Value::String(service_tier),
        );
    }
    if let Some(cache_ttl_minutes) = cache_ttl_minutes {
        object.insert(
            PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY.to_string(),
            Value::Number(cache_ttl_minutes.into()),
        );
    }
    (!object.is_empty()).then_some(Value::Object(object))
}

pub(crate) fn clear_client_request_body_metadata(metadata: Option<Value>) -> Option<Value> {
    clear_request_metadata_fields(metadata, &[REQUESTED_REASONING_EFFORT_METADATA_KEY])
}

pub(crate) fn clear_provider_request_body_metadata(metadata: Option<Value>) -> Option<Value> {
    clear_request_metadata_fields(
        metadata,
        &[
            PROVIDER_REASONING_EFFORT_METADATA_KEY,
            PROVIDER_SERVICE_TIER_METADATA_KEY,
            PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY,
        ],
    )
}

fn clear_request_metadata_fields(metadata: Option<Value>, keys: &[&str]) -> Option<Value> {
    let Some(Value::Object(mut object)) = metadata else {
        return None;
    };
    for key in keys {
        object.remove(*key);
    }
    (!object.is_empty()).then_some(Value::Object(object))
}

pub(crate) fn attach_provider_response_body_metadata(
    metadata: Option<Value>,
    provider_response_body: Option<&Value>,
) -> Option<Value> {
    if metadata
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get(PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY))
        .and_then(Value::as_str)
        .and_then(normalize_provider_service_tier)
        .is_some()
    {
        return metadata;
    }
    let actual_service_tier =
        extract_provider_actual_service_tier_from_response(provider_response_body);
    attach_provider_actual_service_tier_metadata(metadata, actual_service_tier.as_deref())
}

/// Refreshes the response-derived tier for a terminal snapshot. Complete response objects are
/// authoritative even when they contain no tier (which clears a stale candidate value). Capture
/// placeholders/absent bodies are not authoritative, so a terminal summary already present in
/// metadata is preserved for those cases.
pub(crate) fn refresh_provider_response_body_metadata(
    metadata: Option<Value>,
    provider_response_body: Option<&Value>,
) -> Option<Value> {
    let is_capture_placeholder = provider_response_body
        .and_then(Value::as_object)
        .is_some_and(|body| {
            body.get("truncated").and_then(Value::as_bool) == Some(true)
                && body.get("reason").and_then(Value::as_str) == Some("body_capture_limit_exceeded")
        });
    let body_is_complete_object =
        provider_response_body.and_then(Value::as_object).is_some() && !is_capture_placeholder;
    let actual_service_tier =
        extract_provider_actual_service_tier_from_response(provider_response_body)
            .and_then(|value| normalize_provider_service_tier(&value));
    let Some(actual_service_tier) = actual_service_tier else {
        if !body_is_complete_object {
            return metadata;
        }
        let Some(Value::Object(mut object)) = metadata else {
            return None;
        };
        object.remove(PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY);
        return (!object.is_empty()).then_some(Value::Object(object));
    };

    let mut object = match metadata {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    object.insert(
        PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY.to_string(),
        Value::String(actual_service_tier),
    );
    (!object.is_empty()).then_some(Value::Object(object))
}

pub(crate) fn attach_provider_actual_service_tier_metadata(
    metadata: Option<Value>,
    actual_service_tier: Option<&str>,
) -> Option<Value> {
    let Some(actual_service_tier) = actual_service_tier.and_then(normalize_provider_service_tier)
    else {
        return metadata;
    };
    let mut object = match metadata {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    };
    object.insert(
        PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY.to_string(),
        Value::String(actual_service_tier),
    );
    (!object.is_empty()).then_some(Value::Object(object))
}

fn copy_allowed_metadata_fields(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    copy_non_empty_string(source, target, "trace_id");
    copy_non_empty_string(source, target, "client_ip");
    copy_non_empty_string(source, target, "user_agent");
    copy_non_empty_string(source, target, "client_family");
    copy_bool(source, target, "client_requested_stream");
    copy_bool(source, target, UPSTREAM_IS_STREAM_KEY);
    copy_non_null_value(source, target, "client_session_affinity");
    copy_bool(source, target, "api_key_is_standalone");
    copy_non_empty_string(source, target, "request_path");
    copy_non_empty_string(source, target, "request_query_string");
    copy_non_empty_string(source, target, "request_path_and_query");
    copy_non_empty_string(source, target, REQUESTED_REASONING_EFFORT_METADATA_KEY);
    copy_non_empty_string(source, target, PROVIDER_REASONING_EFFORT_METADATA_KEY);
    copy_non_empty_string(source, target, PROVIDER_SERVICE_TIER_METADATA_KEY);
    copy_non_empty_string(source, target, PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY);
    copy_number(source, target, PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY);
    copy_number(source, target, "provider_request_body_base64_bytes");
    copy_number(source, target, "provider_response_body_base64_bytes");
    copy_number(source, target, "client_response_body_base64_bytes");
    copy_non_null_value(source, target, "body_size");
    copy_number(source, target, "client_response_status_code");
    copy_non_null_value(source, target, "billing_snapshot");
    copy_non_empty_string(source, target, "billing_snapshot_schema_version");
    copy_non_empty_string(source, target, "billing_snapshot_status");
    copy_non_null_value(source, target, "settlement_snapshot");
    copy_non_empty_string(source, target, "settlement_snapshot_schema_version");
    copy_non_null_value(source, target, "billing_dimensions");
    copy_non_empty_string(source, target, "model_id");
    copy_non_empty_string(source, target, "global_model_id");
    copy_non_empty_string(source, target, "global_model_name");
    copy_non_null_value(source, target, "dimensions");
    copy_non_null_value(source, target, "billing_rule_snapshot");
    copy_non_null_value(source, target, "scheduling_audit");
    copy_non_null_value(source, target, "tls_fingerprint");
    copy_number(source, target, "rate_multiplier");
    copy_bool(source, target, "is_free_tier");
    copy_number(source, target, "input_price_per_1m");
    copy_number(source, target, "output_price_per_1m");
    copy_number(source, target, "cache_creation_price_per_1m");
    copy_number(source, target, "cache_read_price_per_1m");
    copy_number(source, target, "price_per_request");
    copy_non_null_value(source, target, "proxy");
    copy_non_null_value(source, target, "stage_timings_ms");
    copy_non_null_value(source, target, "db_timings_ms");
    sanitize_request_path_metadata_fields(target);
}

fn move_allowed_metadata_fields(mut source: Map<String, Value>, target: &mut Map<String, Value>) {
    remove_non_empty_string(&mut source, target, "trace_id");
    remove_non_empty_string(&mut source, target, "client_ip");
    remove_non_empty_string(&mut source, target, "user_agent");
    remove_non_empty_string(&mut source, target, "client_family");
    remove_bool(&mut source, target, "client_requested_stream");
    remove_bool(&mut source, target, UPSTREAM_IS_STREAM_KEY);
    remove_non_null_value(&mut source, target, "client_session_affinity");
    remove_bool(&mut source, target, "api_key_is_standalone");
    remove_non_empty_string(&mut source, target, "request_path");
    remove_non_empty_string(&mut source, target, "request_query_string");
    remove_non_empty_string(&mut source, target, "request_path_and_query");
    remove_non_empty_string(&mut source, target, REQUESTED_REASONING_EFFORT_METADATA_KEY);
    remove_non_empty_string(&mut source, target, PROVIDER_REASONING_EFFORT_METADATA_KEY);
    remove_non_empty_string(&mut source, target, PROVIDER_SERVICE_TIER_METADATA_KEY);
    remove_non_empty_string(
        &mut source,
        target,
        PROVIDER_ACTUAL_SERVICE_TIER_METADATA_KEY,
    );
    remove_number(&mut source, target, PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY);
    remove_number(&mut source, target, "provider_request_body_base64_bytes");
    remove_number(&mut source, target, "provider_response_body_base64_bytes");
    remove_number(&mut source, target, "client_response_body_base64_bytes");
    remove_non_null_value(&mut source, target, "body_size");
    remove_number(&mut source, target, "client_response_status_code");
    remove_non_null_value(&mut source, target, "billing_snapshot");
    remove_non_empty_string(&mut source, target, "billing_snapshot_schema_version");
    remove_non_empty_string(&mut source, target, "billing_snapshot_status");
    remove_non_null_value(&mut source, target, "settlement_snapshot");
    remove_non_empty_string(&mut source, target, "settlement_snapshot_schema_version");
    remove_non_null_value(&mut source, target, "billing_dimensions");
    remove_non_empty_string(&mut source, target, "model_id");
    remove_non_empty_string(&mut source, target, "global_model_id");
    remove_non_empty_string(&mut source, target, "global_model_name");
    remove_non_null_value(&mut source, target, "dimensions");
    remove_non_null_value(&mut source, target, "billing_rule_snapshot");
    remove_non_null_value(&mut source, target, "scheduling_audit");
    remove_non_null_value(&mut source, target, "tls_fingerprint");
    remove_number(&mut source, target, "rate_multiplier");
    remove_bool(&mut source, target, "is_free_tier");
    remove_number(&mut source, target, "input_price_per_1m");
    remove_number(&mut source, target, "output_price_per_1m");
    remove_number(&mut source, target, "cache_creation_price_per_1m");
    remove_number(&mut source, target, "cache_read_price_per_1m");
    remove_number(&mut source, target, "price_per_request");
    remove_non_null_value(&mut source, target, "proxy");
    remove_non_null_value(&mut source, target, "stage_timings_ms");
    remove_non_null_value(&mut source, target, "db_timings_ms");
    sanitize_request_path_metadata_fields(target);
}

fn sanitize_request_path_metadata_fields(target: &mut Map<String, Value>) {
    let path = target
        .get("request_path")
        .and_then(Value::as_str)
        .and_then(sanitize_request_path);
    let query = target
        .get("request_query_string")
        .and_then(Value::as_str)
        .and_then(sanitize_request_query_string);
    let path_and_query = target
        .get("request_path_and_query")
        .and_then(Value::as_str)
        .and_then(|value| sanitize_request_path_and_query(value, None))
        .or_else(|| {
            path.as_deref()
                .and_then(|path| sanitize_request_path_and_query(path, query.as_deref()))
        });

    apply_optional_string_field(target, "request_path", path.as_deref());
    apply_optional_string_field(target, "request_query_string", query.as_deref());
    apply_optional_string_field(target, "request_path_and_query", path_and_query.as_deref());
}

fn apply_optional_string_field(target: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        target.insert(key.to_string(), Value::String(value.to_string()));
    } else {
        target.remove(key);
    }
}

fn copy_non_empty_string(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    target.insert(
        key.to_string(),
        Value::String(truncate_usage_request_metadata_string(value)),
    );
}

fn remove_non_empty_string(
    source: &mut Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) {
    let Some(Value::String(value)) = source.remove(key) else {
        return;
    };
    let Some(value) = trim_and_truncate_usage_request_metadata_string_owned(value) else {
        return;
    };
    target.insert(key.to_string(), Value::String(value));
}

fn copy_number(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source.get(key).filter(|value| value.is_number()) else {
        return;
    };
    target.insert(key.to_string(), value.clone());
}

fn remove_number(source: &mut Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source.remove(key).filter(|value| value.is_number()) else {
        return;
    };
    target.insert(key.to_string(), value);
}

fn copy_bool(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source.get(key).filter(|value| value.is_boolean()) else {
        return;
    };
    target.insert(key.to_string(), value.clone());
}

fn remove_bool(source: &mut Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source.remove(key).filter(|value| value.is_boolean()) else {
        return;
    };
    target.insert(key.to_string(), value);
}

fn copy_non_null_value(source: &Map<String, Value>, target: &mut Map<String, Value>, key: &str) {
    let Some(value) = source.get(key).filter(|value| !value.is_null()) else {
        return;
    };
    target.insert(
        key.to_string(),
        sanitize_usage_request_metadata_value(value),
    );
}

fn remove_non_null_value(
    source: &mut Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) {
    let Some(value) = source.remove(key).filter(|value| !value.is_null()) else {
        return;
    };
    target.insert(
        key.to_string(),
        sanitize_usage_request_metadata_value_owned(value),
    );
}

fn sanitize_usage_request_metadata_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(truncate_usage_request_metadata_string(text)),
        _ if usage_request_metadata_within_limits(value) => value.clone(),
        _ => truncated_usage_request_metadata_value(value),
    }
}

fn sanitize_usage_request_metadata_value_owned(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(truncate_usage_request_metadata_string_owned(text)),
        _ if usage_request_metadata_within_limits(&value) => value,
        _ => truncated_usage_request_metadata_value(&value),
    }
}

fn truncate_usage_request_metadata_string(value: &str) -> String {
    const TRUNCATED_SUFFIX: &str = "...[truncated]";

    if value.len() <= MAX_USAGE_REQUEST_METADATA_STRING_BYTES {
        return value.to_string();
    }

    let target_bytes =
        MAX_USAGE_REQUEST_METADATA_STRING_BYTES.saturating_sub(TRUNCATED_SUFFIX.len());
    let mut end = 0usize;
    for (idx, ch) in value.char_indices() {
        let next = idx + ch.len_utf8();
        if next > target_bytes {
            break;
        }
        end = next;
    }

    if end == 0 {
        return TRUNCATED_SUFFIX.to_string();
    }

    format!("{}{TRUNCATED_SUFFIX}", &value[..end])
}

fn trim_and_truncate_usage_request_metadata_string_owned(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == value.len() {
        return Some(truncate_usage_request_metadata_string_owned(value));
    }
    Some(truncate_usage_request_metadata_string(trimmed))
}

fn truncate_usage_request_metadata_string_owned(value: String) -> String {
    if value.len() <= MAX_USAGE_REQUEST_METADATA_STRING_BYTES {
        return value;
    }
    truncate_usage_request_metadata_string(value.as_str())
}

fn truncated_usage_request_metadata_value(value: &Value) -> Value {
    json!({
        "truncated": true,
        "reason": "usage_request_metadata_limits_exceeded",
        "max_depth": MAX_USAGE_REQUEST_METADATA_DEPTH,
        "max_nodes": MAX_USAGE_REQUEST_METADATA_NODES,
        "max_bytes": MAX_USAGE_REQUEST_METADATA_BYTES,
        "value_kind": usage_request_metadata_value_kind(value),
    })
}

fn usage_request_metadata_within_limits(value: &Value) -> bool {
    let mut nodes = 0usize;
    let mut estimated_bytes = 0usize;
    let mut stack = vec![(value, 1usize)];

    while let Some((current, depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        estimated_bytes =
            estimated_bytes.saturating_add(usage_request_metadata_value_size_hint(current));
        if depth > MAX_USAGE_REQUEST_METADATA_DEPTH
            || nodes > MAX_USAGE_REQUEST_METADATA_NODES
            || estimated_bytes > MAX_USAGE_REQUEST_METADATA_BYTES
        {
            return false;
        }
        match current {
            Value::Array(items) => {
                estimated_bytes = estimated_bytes.saturating_add(items.len().saturating_mul(2));
                for item in items.iter().rev() {
                    stack.push((item, depth + 1));
                }
            }
            Value::Object(object) => {
                estimated_bytes = estimated_bytes
                    .saturating_add(object.len().saturating_mul(3))
                    .saturating_add(
                        object
                            .keys()
                            .map(|key| key.len().saturating_add(2))
                            .sum::<usize>(),
                    );
                for item in object.values() {
                    stack.push((item, depth + 1));
                }
            }
            _ => {}
        }
    }

    true
}

fn usage_request_metadata_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn usage_request_metadata_value_size_hint(value: &Value) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(false) => 5,
        Value::Bool(true) => 4,
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => text.len().saturating_add(2),
        Value::Array(_) | Value::Object(_) => 2,
    }
}

#[cfg(test)]
mod tests {
    use aether_contracts::{ExecutionPlan, RequestBody};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    use crate::{
        apply_usage_body_capture_policy_to_event, UsageBodyCapturePolicy, UsageEvent,
        UsageEventData, UsageEventType, UsageRequestRecordLevel,
    };

    use super::{
        attach_client_request_body_metadata, attach_provider_actual_service_tier_metadata,
        attach_provider_request_body_metadata, attach_provider_response_body_metadata,
        build_usage_request_metadata_seed, merge_usage_request_metadata,
        merge_usage_request_metadata_owned, refresh_provider_response_body_metadata,
        retain_first_byte_request_metadata, sanitize_usage_request_metadata,
        sanitize_usage_request_metadata_ref, MAX_USAGE_REQUEST_METADATA_BYTES,
        MAX_USAGE_REQUEST_METADATA_DEPTH, MAX_USAGE_REQUEST_METADATA_NODES,
    };

    fn sample_plan() -> ExecutionPlan {
        ExecutionPlan {
            request_id: "req-1".to_string(),
            candidate_id: Some("cand-1".to_string()),
            provider_name: Some("OpenAI".to_string()),
            provider_id: "provider-1".to_string(),
            endpoint_id: "endpoint-1".to_string(),
            key_id: "key-1".to_string(),
            method: "POST".to_string(),
            url: "https://example.com/v1/chat/completions".to_string(),
            headers: BTreeMap::new(),
            content_type: None,
            content_encoding: None,
            body: RequestBody::from_json(json!({"model": "gpt-5"})),
            stream: false,
            client_api_format: "openai:chat".to_string(),
            provider_api_format: "openai:chat".to_string(),
            model_name: Some("gpt-5".to_string()),
            proxy: None,
            transport_profile: None,
            timeouts: None,
        }
    }

    fn sample_stage_timings_metadata() -> Value {
        json!({
            "stream_candidate_slot": 1,
            "stream_provider_in_flight": 2,
            "stream_upstream_headers": 180,
            "stream_first_data": 8210
        })
    }

    fn sample_db_timings_metadata() -> Value {
        json!({
            "query_count": 2,
            "query_total": 950,
            "query_max": 650,
            "operations": {
                "request_candidate_upsert": {"count": 1, "sum": 650, "max": 650},
                "usage_upsert": {"count": 1, "sum": 300, "max": 300}
            },
            "pool": {
                "max_checked_out": 20,
                "max_pool_size": 20,
                "min_idle": 0,
                "max_connections": 20,
                "max_usage_rate": 100.0
            }
        })
    }

    #[test]
    fn sanitizes_request_metadata_to_allowlist() {
        let stage_timings_ms = sample_stage_timings_metadata();
        let db_timings_ms = sample_db_timings_metadata();
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "request_id": "req-1",
            "provider_id": "provider-1",
            "provider_name": "OpenAI",
            "model": "gpt-5",
            "candidate_index": 2,
            "trace_id": "trace-1",
            "client_ip": "203.0.113.8",
            "user_agent": "Claude-Code/1.0",
            "client_requested_stream": false,
            "upstream_is_stream": true,
            "api_key_is_standalone": true,
            "provider_request_body_base64_bytes": 512,
            "provider_response_body_base64_bytes": 1024,
            "client_response_body_base64_bytes": 2048,
            "body_size": {
                "client_request_body": "1 KB",
                "provider_request_body": "4 KB",
                "provider_over_client": "4x"
            },
            "billing_snapshot": {"status": "complete"},
            "billing_snapshot_schema_version": "2.0",
            "billing_snapshot_status": "complete",
            "model_id": "model-1",
            "global_model_id": "global-model-1",
            "global_model_name": "gpt-5",
            "dimensions": {"total_input_context": 10},
            "rate_multiplier": 1.25,
            "is_free_tier": false,
            "input_price_per_1m": 3.0,
            "output_price_per_1m": 15.0,
            "cache_creation_price_per_1m": 3.75,
            "cache_read_price_per_1m": 0.3,
            "price_per_request": 0.02,
            "stage_timings_ms": stage_timings_ms.clone(),
            "db_timings_ms": db_timings_ms.clone(),
            "original_headers": {"authorization": "Bearer secret"},
            "original_request_body": {"messages": []},
            "provider_request_headers": {"authorization": "Bearer secret"},
            "upstream_url": "https://example.com/v1/chat/completions"
        })))
        .expect("metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "trace_id": "trace-1",
                "client_ip": "203.0.113.8",
                "user_agent": "Claude-Code/1.0",
                "client_requested_stream": false,
                "upstream_is_stream": true,
                "api_key_is_standalone": true,
                "provider_request_body_base64_bytes": 512,
                "provider_response_body_base64_bytes": 1024,
                "client_response_body_base64_bytes": 2048,
                "body_size": {
                    "client_request_body": "1 KB",
                    "provider_request_body": "4 KB",
                    "provider_over_client": "4x"
                },
                "billing_snapshot": {"status": "complete"},
                "billing_snapshot_schema_version": "2.0",
                "billing_snapshot_status": "complete",
                "model_id": "model-1",
                "global_model_id": "global-model-1",
                "global_model_name": "gpt-5",
                "dimensions": {"total_input_context": 10},
                "rate_multiplier": 1.25,
                "is_free_tier": false,
                "input_price_per_1m": 3.0,
                "output_price_per_1m": 15.0,
                "cache_creation_price_per_1m": 3.75,
                "cache_read_price_per_1m": 0.3,
                "price_per_request": 0.02,
                "stage_timings_ms": stage_timings_ms,
                "db_timings_ms": db_timings_ms
            })
        );
    }

    #[test]
    fn first_byte_metadata_keeps_trace_context_and_drops_large_snapshots() {
        let metadata = retain_first_byte_request_metadata(Some(json!({
            "trace_id": "trace-first-byte",
            "client_ip": "203.0.113.8",
            "request_path": "/v1/chat/completions",
            "upstream_is_stream": true,
            "proxy": {"mode": "manual", "node_id": "proxy-1"},
            "billing_snapshot": {"dimensions": [1, 2, 3]},
            "settlement_snapshot": {"status": "pending"},
            "stage_timings_ms": {"planning": 12}
        })))
        .expect("first-byte metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "trace_id": "trace-first-byte",
                "client_ip": "203.0.113.8",
                "request_path": "/v1/chat/completions",
                "request_path_and_query": "/v1/chat/completions",
                "upstream_is_stream": true,
                "proxy": {"mode": "manual", "node_id": "proxy-1"}
            })
        );
    }

    #[test]
    fn sanitizes_request_path_query_metadata() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "request_path": "/v1beta/models/gemini-2.5-pro:streamGenerateContent?key=secret",
            "request_query_string": "key=secret&alt=sse&pageSize=10&token=hidden",
            "request_path_and_query": "/v1beta/models/gemini-2.5-pro:streamGenerateContent?key=secret&alt=sse&pageSize=10&token=hidden",
        })))
        .expect("metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "request_path": "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
                "request_query_string": "alt=sse&pageSize=10",
                "request_path_and_query": "/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse&pageSize=10",
            })
        );
    }

    #[test]
    fn sanitizes_large_allowed_metadata_values_to_bounded_representations() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "trace_id": "t".repeat(2_048),
            "billing_snapshot": {
                "payload": "x".repeat(32 * 1024)
            }
        })))
        .expect("metadata should remain");

        assert!(metadata
            .get("trace_id")
            .and_then(Value::as_str)
            .is_some_and(|value| value.ends_with("...[truncated]")));
        assert_eq!(
            metadata.get("billing_snapshot"),
            Some(&json!({
                "truncated": true,
                "reason": "usage_request_metadata_limits_exceeded",
                "max_depth": MAX_USAGE_REQUEST_METADATA_DEPTH,
                "max_nodes": MAX_USAGE_REQUEST_METADATA_NODES,
                "max_bytes": MAX_USAGE_REQUEST_METADATA_BYTES,
                "value_kind": "object",
            }))
        );
    }

    #[test]
    fn sanitizes_request_metadata_preserves_tls_fingerprint() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "tls_fingerprint": {
                "incoming": {
                    "source": "forwarded_header",
                    "ja3": "incoming-ja3",
                    "ja4": "incoming-ja4"
                },
                "outgoing": {
                    "source": "aether_transport_config",
                    "backend": "reqwest_rustls",
                    "observed": false
                }
            },
            "untrusted_tls_fingerprint": {
                "ja3": "spoofed"
            }
        })))
        .expect("metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "tls_fingerprint": {
                    "incoming": {
                        "source": "forwarded_header",
                        "ja3": "incoming-ja3",
                        "ja4": "incoming-ja4"
                    },
                    "outgoing": {
                        "source": "aether_transport_config",
                        "backend": "reqwest_rustls",
                        "observed": false
                    }
                }
            })
        );
    }

    #[test]
    fn builds_seed_from_context_and_allowlisted_metadata_only() {
        let metadata = build_usage_request_metadata_seed(
            &sample_plan(),
            Some(
                json!({
                    "request_id": "req-1",
                    "candidate_index": 0,
                    "client_requested_stream": false,
                    "upstream_is_stream": true,
                    "api_key_is_standalone": true,
                    "provider_id": "provider-1",
                    "model_id": "model-1",
                    "global_model_id": "global-model-1",
                    "global_model_name": "gpt-5",
                    "client_ip": "203.0.113.8",
                    "user_agent": "Claude-Code/1.0",
                    "billing_snapshot": {"status": "complete"},
                    "stage_timings_ms": {
                        "stream_candidate_slot": 0,
                        "stream_upstream_headers": 180,
                        "stream_first_data": 8210
                    },
                    "db_timings_ms": {
                        "query_count": 1,
                        "query_total": 42,
                        "query_max": 42,
                        "operations": {
                            "auth_api_key_snapshot": {"count": 1, "sum": 42, "max": 42}
                        }
                    }
                })
                .as_object()
                .expect("object"),
            ),
        )
        .expect("metadata should remain");

        assert_eq!(
            metadata,
            json!({
                "client_requested_stream": false,
                "upstream_is_stream": true,
                "api_key_is_standalone": true,
                "model_id": "model-1",
                "global_model_id": "global-model-1",
                "global_model_name": "gpt-5",
                "client_ip": "203.0.113.8",
                "user_agent": "Claude-Code/1.0",
                "billing_snapshot": {"status": "complete"},
                "stage_timings_ms": {
                    "stream_candidate_slot": 0,
                    "stream_upstream_headers": 180,
                    "stream_first_data": 8210
                },
                "db_timings_ms": {
                    "query_count": 1,
                    "query_total": 42,
                    "query_max": 42,
                    "operations": {
                        "auth_api_key_snapshot": {"count": 1, "sum": 42, "max": 42}
                    }
                }
            })
        );
    }

    #[test]
    fn merges_and_filters_request_metadata() {
        let metadata = merge_usage_request_metadata(
            Some(json!({
                "request_id": "req-1"
            })),
            Some(json!({
                "candidate_index": 0,
                "provider_name": "OpenAI"
            })),
        );

        assert_eq!(metadata, None);
    }

    #[test]
    fn client_request_body_metadata_preserves_requested_reasoning_mapping_source() {
        let updated = attach_client_request_body_metadata(
            Some(json!({
                "trace_id": "trace-1",
                "requested_reasoning_effort": "low"
            })),
            Some(&json!({
                "reasoning": { "effort": "XHigh" }
            })),
        )
        .expect("metadata should remain");

        assert_eq!(updated["requested_reasoning_effort"], "xhigh");

        let cleared =
            attach_client_request_body_metadata(Some(updated), Some(&json!({ "model": "gpt-5" })))
                .expect("trace metadata should remain");
        assert!(cleared.get("requested_reasoning_effort").is_none());
    }

    #[test]
    fn provider_request_body_metadata_uses_final_provider_body_as_source_of_truth() {
        let metadata = Some(json!({
            "trace_id": "trace-1",
            "provider_reasoning_effort": "high",
            "provider_service_tier": "priority"
        }));

        let updated = attach_provider_request_body_metadata(
            metadata.clone(),
            Some("openai:responses"),
            Some("gpt-5.6-sol"),
            Some("gpt-5.6-sol"),
            Some(&json!({
                "model": "gpt-5.6-sol",
                "reasoning": { "effort": "low" },
                "service_tier": "standard"
            })),
        )
        .expect("metadata should remain");

        assert_eq!(
            updated,
            json!({
                "trace_id": "trace-1",
                "provider_reasoning_effort": "low",
                "provider_service_tier": "standard",
                "provider_cache_ttl_minutes": 30
            })
        );

        let cleared = attach_provider_request_body_metadata(
            metadata,
            Some("openai:responses"),
            Some("gpt-5"),
            Some("gpt-5"),
            Some(&json!({
                "model": "gpt-5"
            })),
        )
        .expect("metadata should retain unrelated fields");

        assert_eq!(
            cleared,
            json!({
                "trace_id": "trace-1"
            })
        );
    }

    #[test]
    fn final_provider_request_tier_survives_basic_body_capture_as_derived_metadata() {
        let request_body = json!({
            "model": "gpt-5",
            "reasoning": { "effort": "xhigh" }
        });
        let provider_request_body = json!({
            "model": "gpt-5",
            "reasoning": { "effort": "max" },
            "service_tier": "priority"
        });
        let request_metadata = attach_client_request_body_metadata(None, Some(&request_body));
        let request_metadata = attach_provider_request_body_metadata(
            request_metadata,
            Some("openai:responses"),
            Some("gpt-5"),
            Some("gpt-5"),
            Some(&provider_request_body),
        );
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-final-provider-tier",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                request_body: Some(request_body),
                provider_request_body: Some(provider_request_body),
                request_metadata,
                ..UsageEventData::default()
            },
        );

        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Basic,
                max_request_body_bytes: Some(1024),
                max_response_body_bytes: Some(1024),
            },
            &mut event,
        );

        assert_eq!(event.data.request_body, None);
        assert_eq!(event.data.provider_request_body, None);
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("requested_reasoning_effort")),
            Some(&json!("xhigh"))
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("provider_reasoning_effort")),
            Some(&json!("max"))
        );
        assert_eq!(
            event
                .data
                .request_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("provider_service_tier")),
            Some(&json!("priority"))
        );
    }

    #[test]
    fn provider_response_metadata_preserves_terminal_actual_service_tier() {
        let metadata = attach_provider_response_body_metadata(
            Some(json!({"provider_service_tier": "priority"})),
            Some(&json!({
                "chunks": [
                    {"service_tier": "priority"},
                    {"service_tier": "Default", "usage": {"total_tokens": 12}}
                ]
            })),
        )
        .expect("requested and actual provider tiers should be preserved");

        assert_eq!(
            metadata,
            json!({
                "provider_service_tier": "priority",
                "provider_actual_service_tier": "default"
            })
        );
    }

    #[test]
    fn terminal_response_refresh_replaces_stale_actual_tier() {
        let metadata = refresh_provider_response_body_metadata(
            Some(json!({"provider_actual_service_tier": "priority"})),
            Some(&json!({"service_tier": "Default"})),
        )
        .expect("actual tier should remain");
        assert_eq!(metadata["provider_actual_service_tier"], "default");

        let metadata = refresh_provider_response_body_metadata(
            Some(json!({
                "trace_id": "trace-1",
                "provider_actual_service_tier": "priority"
            })),
            Some(&json!({"id": "response-without-tier"})),
        )
        .expect("un-tiered complete response should remain auditable");
        assert_eq!(metadata, json!({"trace_id": "trace-1"}));
    }

    #[test]
    fn terminal_response_refresh_preserves_summary_over_capture_placeholder() {
        let metadata = refresh_provider_response_body_metadata(
            Some(json!({"provider_actual_service_tier": "priority"})),
            Some(&json!({
                "truncated": true,
                "reason": "body_capture_limit_exceeded"
            })),
        )
        .expect("summary should survive placeholder capture");
        assert_eq!(metadata["provider_actual_service_tier"], "priority");
    }

    #[test]
    fn terminal_summary_tier_uses_the_same_normalized_metadata_field() {
        let metadata = attach_provider_actual_service_tier_metadata(
            Some(json!({"trace_id": "trace-1"})),
            Some(" Flex "),
        )
        .expect("terminal summary tier should be retained");

        assert_eq!(
            metadata,
            json!({
                "trace_id": "trace-1",
                "provider_actual_service_tier": "flex"
            })
        );
    }

    #[test]
    fn terminal_summary_tier_precedes_truncated_response_capture() {
        let metadata = attach_provider_response_body_metadata(
            Some(json!({"provider_actual_service_tier": "default"})),
            Some(&json!({"chunks": [{"service_tier": "priority"}]})),
        )
        .expect("terminal summary tier should remain");

        assert_eq!(
            metadata.get("provider_actual_service_tier"),
            Some(&Value::String("default".to_string()))
        );
    }

    #[test]
    fn owned_merge_matches_filtered_merge_for_trusted_objects() {
        let base = Some(json!({
            "trace_id": "trace-1",
            "provider_request_body_base64_bytes": 128
        }));
        let override_value = Some(json!({
            "billing_snapshot_status": "complete",
            "trace_id": "trace-2",
            "provider_actual_service_tier": "default"
        }));

        assert_eq!(
            merge_usage_request_metadata_owned(base.clone(), override_value.clone()),
            merge_usage_request_metadata(base, override_value)
        );
    }

    #[test]
    fn borrowed_sanitize_matches_owned_sanitize() {
        let value = json!({
            "trace_id": "trace-1",
            "billing_snapshot": {"status": "complete"},
            "provider_name": "OpenAI"
        });

        assert_eq!(
            sanitize_usage_request_metadata_ref(Some(&value)),
            sanitize_usage_request_metadata(Some(value))
        );
    }
}
