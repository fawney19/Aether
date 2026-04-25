use aether_contracts::ExecutionPlan;
use serde_json::{json, Map, Value};

const MAX_USAGE_REQUEST_METADATA_DEPTH: usize = 32;
const MAX_USAGE_REQUEST_METADATA_NODES: usize = 4_000;
const MAX_USAGE_REQUEST_METADATA_BYTES: usize = 16 * 1024;
const MAX_USAGE_REQUEST_METADATA_STRING_BYTES: usize = 1_024;

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

pub(crate) fn sanitize_usage_request_metadata_ref(value: Option<&Value>) -> Option<Value> {
    let object = value.and_then(Value::as_object)?;

    let mut filtered = Map::new();
    copy_allowed_metadata_fields(object, &mut filtered);

    (!filtered.is_empty()).then_some(Value::Object(filtered))
}

fn copy_allowed_metadata_fields(source: &Map<String, Value>, target: &mut Map<String, Value>) {
    copy_non_empty_string(source, target, "trace_id");
    copy_bool(source, target, "client_requested_stream");
    copy_bool(source, target, "upstream_is_stream");
    copy_number(source, target, "provider_request_body_base64_bytes");
    copy_number(source, target, "provider_response_body_base64_bytes");
    copy_number(source, target, "client_response_body_base64_bytes");
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
    copy_number(source, target, "rate_multiplier");
    copy_bool(source, target, "is_free_tier");
    copy_number(source, target, "input_price_per_1m");
    copy_number(source, target, "output_price_per_1m");
    copy_number(source, target, "cache_creation_price_per_1m");
    copy_number(source, target, "cache_read_price_per_1m");
    copy_number(source, target, "price_per_request");
}

fn move_allowed_metadata_fields(mut source: Map<String, Value>, target: &mut Map<String, Value>) {
    remove_non_empty_string(&mut source, target, "trace_id");
    remove_bool(&mut source, target, "client_requested_stream");
    remove_bool(&mut source, target, "upstream_is_stream");
    remove_number(&mut source, target, "provider_request_body_base64_bytes");
    remove_number(&mut source, target, "provider_response_body_base64_bytes");
    remove_number(&mut source, target, "client_response_body_base64_bytes");
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
    remove_number(&mut source, target, "rate_multiplier");
    remove_bool(&mut source, target, "is_free_tier");
    remove_number(&mut source, target, "input_price_per_1m");
    remove_number(&mut source, target, "output_price_per_1m");
    remove_number(&mut source, target, "cache_creation_price_per_1m");
    remove_number(&mut source, target, "cache_read_price_per_1m");
    remove_number(&mut source, target, "price_per_request");
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

    use super::{
        build_usage_request_metadata_seed, merge_usage_request_metadata,
        merge_usage_request_metadata_owned, sanitize_usage_request_metadata,
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
            tls_profile: None,
            timeouts: None,
        }
    }

    #[test]
    fn sanitizes_request_metadata_to_allowlist() {
        let metadata = sanitize_usage_request_metadata(Some(json!({
            "request_id": "req-1",
            "provider_id": "provider-1",
            "provider_name": "OpenAI",
            "model": "gpt-5",
            "candidate_index": 2,
            "trace_id": "trace-1",
            "client_requested_stream": false,
            "upstream_is_stream": true,
            "provider_request_body_base64_bytes": 512,
            "provider_response_body_base64_bytes": 1024,
            "client_response_body_base64_bytes": 2048,
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
                "client_requested_stream": false,
                "upstream_is_stream": true,
                "provider_request_body_base64_bytes": 512,
                "provider_response_body_base64_bytes": 1024,
                "client_response_body_base64_bytes": 2048,
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
                "price_per_request": 0.02
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
    fn builds_seed_from_context_and_allowlisted_metadata_only() {
        let metadata = build_usage_request_metadata_seed(
            &sample_plan(),
            Some(
                json!({
                    "request_id": "req-1",
                    "candidate_index": 0,
                    "client_requested_stream": false,
                    "upstream_is_stream": true,
                    "provider_id": "provider-1",
                    "model_id": "model-1",
                    "global_model_id": "global-model-1",
                    "global_model_name": "gpt-5",
                    "billing_snapshot": {"status": "complete"}
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
                "model_id": "model-1",
                "global_model_id": "global-model-1",
                "global_model_name": "gpt-5",
                "billing_snapshot": {"status": "complete"}
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
    fn owned_merge_matches_filtered_merge_for_trusted_objects() {
        let base = Some(json!({
            "trace_id": "trace-1",
            "provider_request_body_base64_bytes": 128
        }));
        let override_value = Some(json!({
            "billing_snapshot_status": "complete",
            "trace_id": "trace-2"
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
