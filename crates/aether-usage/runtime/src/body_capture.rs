use std::io::{self, Write};

use aether_data_contracts::repository::usage::{
    UpsertUsageRecord, UsageBodyCaptureState, UsageBodyField,
};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::event::UsageEvent;
use crate::runtime::{UsageBodyCapturePolicy, UsageRequestRecordLevel};

const TRUNCATED_BODY_STRING_SUFFIX: &str = "...[truncated]";

#[derive(Debug)]
struct LimitedUsageBodyCapture {
    value: Value,
    source_bytes: Option<u64>,
    stored_bytes: Option<u64>,
    truncated: bool,
    reason: Option<&'static str>,
}

struct UsageBodyCapturePayloadMut<'a> {
    request_body: &'a mut Option<Value>,
    request_body_ref: &'a mut Option<String>,
    request_body_state: &'a mut Option<UsageBodyCaptureState>,
    provider_request_body: &'a mut Option<Value>,
    provider_request_body_ref: &'a mut Option<String>,
    provider_request_body_state: &'a mut Option<UsageBodyCaptureState>,
    response_body: &'a mut Option<Value>,
    response_body_ref: &'a mut Option<String>,
    response_body_state: &'a mut Option<UsageBodyCaptureState>,
    client_response_body: &'a mut Option<Value>,
    client_response_body_ref: &'a mut Option<String>,
    client_response_body_state: &'a mut Option<UsageBodyCaptureState>,
    request_metadata: &'a mut Option<Value>,
}

impl<'a> UsageBodyCapturePayloadMut<'a> {
    fn from_event(event: &'a mut UsageEvent) -> Self {
        Self {
            request_body: &mut event.data.request_body,
            request_body_ref: &mut event.data.request_body_ref,
            request_body_state: &mut event.data.request_body_state,
            provider_request_body: &mut event.data.provider_request_body,
            provider_request_body_ref: &mut event.data.provider_request_body_ref,
            provider_request_body_state: &mut event.data.provider_request_body_state,
            response_body: &mut event.data.response_body,
            response_body_ref: &mut event.data.response_body_ref,
            response_body_state: &mut event.data.response_body_state,
            client_response_body: &mut event.data.client_response_body,
            client_response_body_ref: &mut event.data.client_response_body_ref,
            client_response_body_state: &mut event.data.client_response_body_state,
            request_metadata: &mut event.data.request_metadata,
        }
    }

    fn from_record(record: &'a mut UpsertUsageRecord) -> Self {
        Self {
            request_body: &mut record.request_body,
            request_body_ref: &mut record.request_body_ref,
            request_body_state: &mut record.request_body_state,
            provider_request_body: &mut record.provider_request_body,
            provider_request_body_ref: &mut record.provider_request_body_ref,
            provider_request_body_state: &mut record.provider_request_body_state,
            response_body: &mut record.response_body,
            response_body_ref: &mut record.response_body_ref,
            response_body_state: &mut record.response_body_state,
            client_response_body: &mut record.client_response_body,
            client_response_body_ref: &mut record.client_response_body_ref,
            client_response_body_state: &mut record.client_response_body_state,
            request_metadata: &mut record.request_metadata,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UsageBodyCaptureEngine {
    policy: UsageBodyCapturePolicy,
}

#[derive(Default)]
struct CountingWriter {
    bytes: u64,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buf.len() as u64);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeBodyCaptureStates {
    pub request: UsageBodyCaptureState,
    pub provider_request: UsageBodyCaptureState,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeBodyCaptureMetadataInput<'a> {
    pub request_has_inline_body: bool,
    pub request_body_ref: Option<&'a str>,
    pub provider_request_has_inline_body: bool,
    pub provider_request_body_ref: Option<&'a str>,
    pub provider_request_source_bytes: Option<u64>,
    pub provider_request_unavailable: bool,
    pub provider_request_unavailable_reason: Option<&'a str>,
}

impl UsageBodyCaptureEngine {
    pub fn new(policy: UsageBodyCapturePolicy) -> Self {
        Self { policy }
    }

    pub fn apply_to_event(self, event: &mut UsageEvent) {
        let force_disabled = is_sensitive_oauth_exchange(
            event.data.api_format.as_deref(),
            event.data.endpoint_api_format.as_deref(),
        );
        self.apply_to_payload(
            UsageBodyCapturePayloadMut::from_event(event),
            force_disabled,
        );
    }

    pub fn apply_to_record(self, record: &mut UpsertUsageRecord) {
        let force_disabled = is_sensitive_oauth_exchange(
            record.api_format.as_deref(),
            record.endpoint_api_format.as_deref(),
        );
        self.apply_to_payload(
            UsageBodyCapturePayloadMut::from_record(record),
            force_disabled,
        );
    }

    fn apply_to_payload(self, payload: UsageBodyCapturePayloadMut<'_>, force_disabled: bool) {
        if force_disabled || matches!(self.policy.record_level, UsageRequestRecordLevel::Basic) {
            let reason = if force_disabled {
                "sensitive_oauth_exchange"
            } else {
                "request_record_level_basic"
            };
            disable_usage_body_capture_field(
                UsageBodyField::RequestBody,
                "request",
                payload.request_body,
                payload.request_body_ref,
                payload.request_body_state,
                payload.request_metadata,
                reason,
            );
            disable_usage_body_capture_field(
                UsageBodyField::ProviderRequestBody,
                "provider_request",
                payload.provider_request_body,
                payload.provider_request_body_ref,
                payload.provider_request_body_state,
                payload.request_metadata,
                reason,
            );
            disable_usage_body_capture_field(
                UsageBodyField::ResponseBody,
                "response",
                payload.response_body,
                payload.response_body_ref,
                payload.response_body_state,
                payload.request_metadata,
                reason,
            );
            disable_usage_body_capture_field(
                UsageBodyField::ClientResponseBody,
                "client_response",
                payload.client_response_body,
                payload.client_response_body_ref,
                payload.client_response_body_state,
                payload.request_metadata,
                reason,
            );
            return;
        }

        apply_usage_body_capture_limit(
            UsageBodyField::RequestBody,
            "request",
            None,
            payload.request_body,
            payload.request_body_ref,
            payload.request_body_state,
            payload.request_metadata,
        );
        apply_usage_body_capture_limit(
            UsageBodyField::ProviderRequestBody,
            "provider_request",
            None,
            payload.provider_request_body,
            payload.provider_request_body_ref,
            payload.provider_request_body_state,
            payload.request_metadata,
        );
        apply_usage_body_capture_limit(
            UsageBodyField::ResponseBody,
            "response",
            None,
            payload.response_body,
            payload.response_body_ref,
            payload.response_body_state,
            payload.request_metadata,
        );
        apply_usage_body_capture_limit(
            UsageBodyField::ClientResponseBody,
            "client_response",
            None,
            payload.client_response_body,
            payload.client_response_body_ref,
            payload.client_response_body_state,
            payload.request_metadata,
        );
    }
}

fn is_sensitive_oauth_exchange(
    api_format: Option<&str>,
    endpoint_api_format: Option<&str>,
) -> bool {
    [api_format, endpoint_api_format]
        .into_iter()
        .flatten()
        .map(str::trim)
        .any(|format| {
            format.eq_ignore_ascii_case("oauth:exchange")
                || format.eq_ignore_ascii_case("provider_oauth:exchange")
                || format.eq_ignore_ascii_case("provider_oauth:local_refresh")
                || format.eq_ignore_ascii_case("vertex_ai:service_account_token")
        })
}

pub fn apply_usage_body_capture_policy_to_event(
    policy: UsageBodyCapturePolicy,
    event: &mut UsageEvent,
) {
    UsageBodyCaptureEngine::new(policy).apply_to_event(event);
}

pub fn apply_usage_body_capture_policy_to_record(
    policy: UsageBodyCapturePolicy,
    record: &mut UpsertUsageRecord,
) {
    UsageBodyCaptureEngine::new(policy).apply_to_record(record);
}

fn disable_usage_body_capture_field(
    field: UsageBodyField,
    metadata_key: &str,
    body: &mut Option<Value>,
    body_ref: &mut Option<String>,
    state: &mut Option<UsageBodyCaptureState>,
    request_metadata: &mut Option<Value>,
    reason: &'static str,
) {
    *body = None;
    *body_ref = None;
    *state = Some(UsageBodyCaptureState::Disabled);
    sync_usage_body_ref_metadata(request_metadata, field, None);
    upsert_body_capture_metadata_value_entry(
        request_metadata,
        metadata_key,
        Some(UsageBodyCaptureState::Disabled),
        None,
        None,
        Some(reason),
    );
}

fn apply_usage_body_capture_limit(
    field: UsageBodyField,
    metadata_key: &str,
    max_bytes: Option<usize>,
    body: &mut Option<Value>,
    body_ref: &mut Option<String>,
    state: &mut Option<UsageBodyCaptureState>,
    request_metadata: &mut Option<Value>,
) {
    *body_ref = sanitize_usage_body_ref(body_ref.take());
    if body.is_some() && body_ref.is_some() {
        *body = None;
    }

    if let Some(body_ref_value) = body_ref.as_ref() {
        *state = Some(UsageBodyCaptureState::Reference);
        sync_usage_body_ref_metadata(request_metadata, field, Some(body_ref_value));
        upsert_body_capture_metadata_value_entry(
            request_metadata,
            metadata_key,
            Some(UsageBodyCaptureState::Reference),
            None,
            None,
            None,
        );
        return;
    }

    let Some(value) = body.take() else {
        if matches!(state, Some(UsageBodyCaptureState::Unavailable)) {
            upsert_body_capture_metadata_value_entry(
                request_metadata,
                metadata_key,
                *state,
                None,
                None,
                None,
            );
        } else if state.is_none() {
            *state = Some(UsageBodyCaptureState::None);
        }
        sync_usage_body_ref_metadata(request_metadata, field, None);
        return;
    };

    let limited = limit_usage_body_capture_value(value, max_bytes);
    let next_state = if limited.truncated {
        UsageBodyCaptureState::Truncated
    } else {
        UsageBodyCaptureState::Inline
    };
    *state = Some(next_state);
    *body = Some(limited.value);
    sync_usage_body_ref_metadata(request_metadata, field, None);
    upsert_body_capture_metadata_value_entry(
        request_metadata,
        metadata_key,
        Some(next_state),
        limited.stored_bytes,
        limited.source_bytes,
        limited.reason,
    );
}

fn limit_usage_body_capture_value(
    value: Value,
    max_bytes: Option<usize>,
) -> LimitedUsageBodyCapture {
    let source_bytes = json_serialized_len(&value);
    let Some(limit) = max_bytes.filter(|value| *value > 0) else {
        return LimitedUsageBodyCapture {
            stored_bytes: source_bytes,
            source_bytes,
            value,
            truncated: false,
            reason: None,
        };
    };
    let Some(source_len) = source_bytes else {
        return LimitedUsageBodyCapture {
            stored_bytes: None,
            source_bytes: None,
            value,
            truncated: false,
            reason: None,
        };
    };
    if source_len <= limit as u64 {
        return LimitedUsageBodyCapture {
            stored_bytes: Some(source_len),
            source_bytes: Some(source_len),
            value,
            truncated: false,
            reason: None,
        };
    }

    let truncated_value = match value {
        Value::String(text) => Value::String(truncate_usage_body_string(&text, limit)),
        other => json!({
            "truncated": true,
            "reason": "body_capture_limit_exceeded",
            "max_bytes": limit,
            "source_bytes": source_len,
            "value_kind": usage_value_kind(&other),
        }),
    };
    let stored_bytes = json_serialized_len(&truncated_value);
    LimitedUsageBodyCapture {
        value: truncated_value,
        source_bytes: Some(source_len),
        stored_bytes,
        truncated: true,
        reason: Some("body_capture_limit_exceeded"),
    }
}

fn truncate_usage_body_string(value: &str, max_bytes: usize) -> String {
    let mut end = value.len();
    while end > 0 {
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        let mut candidate = value[..end].to_string();
        candidate.push_str(TRUNCATED_BODY_STRING_SUFFIX);
        if json_serialized_len(&candidate).is_some_and(|bytes| bytes <= max_bytes as u64) {
            return candidate;
        }
        end = value[..end]
            .char_indices()
            .last()
            .map(|(index, _)| index)
            .unwrap_or(0);
        if end == 0 {
            break;
        }
    }

    json!({
        "truncated": true,
        "reason": "body_capture_limit_exceeded",
        "max_bytes": max_bytes,
        "value_kind": "string",
    })
    .to_string()
}

fn json_serialized_len<T: Serialize>(value: &T) -> Option<u64> {
    let mut writer = CountingWriter::default();
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(writer.bytes)
}

pub(crate) fn sync_usage_body_ref_metadata(
    metadata: &mut Option<Value>,
    field: UsageBodyField,
    body_ref: Option<&str>,
) {
    let key = field.as_ref_key();
    let Some(body_ref) = body_ref.map(str::trim).filter(|value| !value.is_empty()) else {
        let clear_metadata = match metadata.as_mut() {
            Some(Value::Object(object)) => {
                object.remove(key);
                object.is_empty()
            }
            _ => false,
        };
        if clear_metadata {
            *metadata = None;
        }
        return;
    };
    if let Some(Value::Object(object)) = metadata.as_mut() {
        if object.get(key).and_then(Value::as_str) == Some(body_ref) {
            return;
        }
        object.insert(key.to_owned(), Value::String(body_ref.to_owned()));
        return;
    }
    let object = metadata
        .get_or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut();
    let Some(object) = object else {
        return;
    };
    object.insert(key.to_owned(), Value::String(body_ref.to_owned()));
}

pub(crate) fn build_payload_body_capture_metadata(
    provider_body_base64: Option<&str>,
    client_body_base64: Option<&str>,
    provider_body_state: Option<UsageBodyCaptureState>,
    client_body_state: Option<UsageBodyCaptureState>,
) -> Option<Value> {
    let provider_decoded_len = provider_body_base64.and_then(decoded_base64_len_hint);
    let client_decoded_len = client_body_base64.and_then(decoded_base64_len_hint);
    let body_capture_capacity =
        usize::from(provider_body_state.is_some()) + usize::from(client_body_state.is_some());
    let mut metadata = Map::with_capacity(
        usize::from(provider_decoded_len.is_some())
            + usize::from(client_decoded_len.is_some())
            + usize::from(body_capture_capacity > 0),
    );
    if let Some(decoded_len) = provider_decoded_len {
        metadata.insert(
            "provider_response_body_base64_bytes".to_string(),
            Value::Number(decoded_len.into()),
        );
    }
    if let Some(decoded_len) = client_decoded_len {
        metadata.insert(
            "client_response_body_base64_bytes".to_string(),
            Value::Number(decoded_len.into()),
        );
    }

    if body_capture_capacity > 0 {
        let mut body_capture = Map::with_capacity(body_capture_capacity);
        append_body_capture_metadata_entry(
            &mut body_capture,
            "response",
            provider_body_state,
            provider_decoded_len,
            provider_decoded_len,
        );
        append_body_capture_metadata_entry(
            &mut body_capture,
            "client_response",
            client_body_state,
            client_decoded_len,
            client_decoded_len,
        );
        metadata.insert("body_capture".to_string(), Value::Object(body_capture));
    }

    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

pub(crate) fn build_runtime_body_capture_states(
    request_has_inline_body: bool,
    request_body_ref: Option<&str>,
    provider_request_has_inline_body: bool,
    provider_request_body_ref: Option<&str>,
    provider_request_unavailable: bool,
) -> RuntimeBodyCaptureStates {
    RuntimeBodyCaptureStates {
        request: UsageBodyCaptureState::from_capture_parts(
            request_has_inline_body,
            request_body_ref.is_some(),
            false,
        ),
        provider_request: UsageBodyCaptureState::from_capture_parts(
            provider_request_has_inline_body,
            provider_request_body_ref.is_some(),
            provider_request_unavailable,
        ),
    }
}

pub(crate) fn append_runtime_body_capture_metadata(
    metadata: &mut Map<String, Value>,
    input: RuntimeBodyCaptureMetadataInput<'_>,
) {
    let states = build_runtime_body_capture_states(
        input.request_has_inline_body,
        input.request_body_ref,
        input.provider_request_has_inline_body,
        input.provider_request_body_ref,
        input.provider_request_unavailable,
    );
    let Some(body_capture_object) = body_capture_object_mut(metadata, 2) else {
        return;
    };
    body_capture_object.insert(
        "request".to_string(),
        build_body_capture_metadata_entry(states.request, None, None, None),
    );
    body_capture_object.insert(
        "provider_request".to_string(),
        build_body_capture_metadata_entry(
            states.provider_request,
            input.provider_request_source_bytes,
            input.provider_request_source_bytes,
            input.provider_request_unavailable_reason,
        ),
    );
}

pub(crate) fn build_plan_body_capture_metadata(
    provider_request_body_base64: Option<&str>,
) -> Option<Value> {
    provider_request_body_base64?;
    let mut metadata = Map::with_capacity(2);
    append_plan_body_capture_metadata(&mut metadata, provider_request_body_base64);
    (!metadata.is_empty()).then_some(Value::Object(metadata))
}

pub(crate) fn append_plan_body_capture_metadata(
    metadata: &mut Map<String, Value>,
    provider_request_body_base64: Option<&str>,
) {
    if let Some(body_bytes_b64) = provider_request_body_base64 {
        let decoded_len = decoded_base64_len_hint(body_bytes_b64);
        if let Some(decoded_len) = decoded_len {
            metadata.insert(
                "provider_request_body_base64_bytes".to_string(),
                Value::Number(decoded_len.into()),
            );
        }
        let Some(body_capture_object) = body_capture_object_mut(metadata, 1) else {
            return;
        };
        body_capture_object.insert(
            "provider_request".to_string(),
            build_body_capture_metadata_entry(
                UsageBodyCaptureState::Unavailable,
                decoded_len,
                decoded_len,
                Some("body_bytes_base64_only"),
            ),
        );
    }
}

fn append_body_capture_metadata_entry(
    target: &mut Map<String, Value>,
    key: &str,
    state: Option<UsageBodyCaptureState>,
    stored_bytes: Option<u64>,
    source_bytes: Option<u64>,
) {
    let Some(state) = state else {
        return;
    };
    target.insert(
        key.to_string(),
        build_body_capture_metadata_entry(
            state,
            stored_bytes,
            source_bytes,
            matches!(state, UsageBodyCaptureState::Truncated)
                .then_some("body_capture_limit_exceeded"),
        ),
    );
}

pub(crate) fn mark_usage_event_capture_truncated(metadata: &mut Option<Value>, key: &str) {
    aether_data_contracts::repository::usage::mark_usage_capture_memory_omitted(metadata, key);
}

fn upsert_body_capture_metadata_value_entry(
    metadata: &mut Option<Value>,
    key: &str,
    state: Option<UsageBodyCaptureState>,
    stored_bytes: Option<u64>,
    source_bytes: Option<u64>,
    reason: Option<&str>,
) {
    let Some(state) = state else {
        return;
    };
    let Some(body_capture_object) = body_capture_value_object_mut(metadata, 1) else {
        return;
    };
    body_capture_object.insert(
        key.to_string(),
        build_body_capture_metadata_entry(state, stored_bytes, source_bytes, reason),
    );
}

fn body_capture_object_mut(
    metadata: &mut Map<String, Value>,
    capacity: usize,
) -> Option<&mut Map<String, Value>> {
    let body_capture = metadata
        .entry("body_capture".to_string())
        .or_insert_with(|| Value::Object(Map::with_capacity(capacity)));
    body_capture.as_object_mut()
}

fn body_capture_value_object_mut(
    metadata: &mut Option<Value>,
    capacity: usize,
) -> Option<&mut Map<String, Value>> {
    let metadata_object = metadata
        .get_or_insert_with(|| Value::Object(Map::with_capacity(1)))
        .as_object_mut();
    let metadata_object = metadata_object?;
    body_capture_object_mut(metadata_object, capacity)
}

fn build_body_capture_metadata_entry(
    state: UsageBodyCaptureState,
    stored_bytes: Option<u64>,
    source_bytes: Option<u64>,
    reason: Option<&str>,
) -> Value {
    let mut entry = Map::with_capacity(
        1 + usize::from(stored_bytes.is_some())
            + usize::from(source_bytes.is_some())
            + usize::from(reason.is_some()),
    );
    entry.insert(
        "state".to_string(),
        Value::String(state.as_str().to_owned()),
    );
    if let Some(bytes) = stored_bytes {
        entry.insert("stored_bytes".to_string(), json!(bytes));
    }
    if let Some(bytes) = source_bytes {
        entry.insert("source_bytes".to_string(), json!(bytes));
    }
    if let Some(reason) = reason {
        entry.insert("reason".to_string(), Value::String(reason.to_owned()));
    }
    Value::Object(entry)
}

pub(crate) fn decoded_base64_len_hint(body_base64: &str) -> Option<u64> {
    let body_base64 = body_base64.trim();
    if body_base64.is_empty() {
        return None;
    }

    let usable_len = body_base64.len();
    if usable_len % 4 == 1 {
        return None;
    }

    let padding = body_base64
        .chars()
        .rev()
        .take_while(|char| *char == '=')
        .count();
    let full_quads = usable_len / 4;
    let remainder = usable_len % 4;
    let base_len = full_quads.saturating_mul(3);
    let remainder_len = match remainder {
        0 => 0,
        2 => 1,
        3 => 2,
        _ => return None,
    };
    let decoded_len = base_len
        .saturating_add(remainder_len)
        .saturating_sub(padding.min(2));

    Some(decoded_len as u64)
}

fn sanitize_usage_body_ref(value: Option<String>) -> Option<String> {
    value.and_then(trim_owned_non_empty_string)
}

fn trim_owned_non_empty_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() == value.len() {
        return Some(value);
    }
    Some(trimmed.to_string())
}

fn usage_value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_usage_body_capture_policy_to_event, apply_usage_body_capture_policy_to_record,
        build_plan_body_capture_metadata, sync_usage_body_ref_metadata,
        trim_owned_non_empty_string, truncate_usage_body_string,
        upsert_body_capture_metadata_value_entry,
    };
    use aether_data_contracts::repository::usage::UsageBodyCaptureState;
    use aether_data_contracts::repository::usage::UsageBodyField;
    use serde_json::{json, Map, Value};

    use crate::{
        UsageBodyCapturePolicy, UsageEvent, UsageEventData, UsageEventType, UsageRequestRecordLevel,
    };

    fn sensitive_oauth_event(api_format: &str) -> UsageEvent {
        UsageEvent::new(
            UsageEventType::Completed,
            "oauth-sensitive-request",
            UsageEventData {
                provider_name: "oauth".to_string(),
                model: "oauth-exchange".to_string(),
                api_format: Some(api_format.to_string()),
                endpoint_api_format: Some(api_format.to_string()),
                request_body: Some(json!({"client_secret":"request-secret"})),
                request_body_ref: Some("usage://oauth/request".to_string()),
                provider_request_body: Some(json!({"refresh_token":"refresh-secret"})),
                provider_request_body_ref: Some("usage://oauth/provider-request".to_string()),
                response_body: Some(json!({
                    "access_token":"access-secret",
                    "refresh_token":"rotated-refresh-secret"
                })),
                response_body_ref: Some("usage://oauth/response".to_string()),
                client_response_body: Some(json!({"access_token":"client-access-secret"})),
                client_response_body_ref: Some("usage://oauth/client-response".to_string()),
                ..UsageEventData::default()
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn assert_sensitive_bodies_disabled(
        request_body: &Option<Value>,
        request_body_ref: &Option<String>,
        request_body_state: Option<UsageBodyCaptureState>,
        provider_request_body: &Option<Value>,
        provider_request_body_ref: &Option<String>,
        provider_request_body_state: Option<UsageBodyCaptureState>,
        response_body: &Option<Value>,
        response_body_ref: &Option<String>,
        response_body_state: Option<UsageBodyCaptureState>,
        client_response_body: &Option<Value>,
        client_response_body_ref: &Option<String>,
        client_response_body_state: Option<UsageBodyCaptureState>,
        request_metadata: &Option<Value>,
    ) {
        assert!(request_body.is_none());
        assert!(request_body_ref.is_none());
        assert_eq!(request_body_state, Some(UsageBodyCaptureState::Disabled));
        assert!(provider_request_body.is_none());
        assert!(provider_request_body_ref.is_none());
        assert_eq!(
            provider_request_body_state,
            Some(UsageBodyCaptureState::Disabled)
        );
        assert!(response_body.is_none());
        assert!(response_body_ref.is_none());
        assert_eq!(response_body_state, Some(UsageBodyCaptureState::Disabled));
        assert!(client_response_body.is_none());
        assert!(client_response_body_ref.is_none());
        assert_eq!(
            client_response_body_state,
            Some(UsageBodyCaptureState::Disabled)
        );
        let body_capture = request_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("body_capture"))
            .and_then(Value::as_object)
            .expect("body capture metadata should exist");
        for field in ["request", "provider_request", "response", "client_response"] {
            assert_eq!(
                body_capture
                    .get(field)
                    .and_then(|entry| entry.get("reason"))
                    .and_then(Value::as_str),
                Some("sensitive_oauth_exchange")
            );
        }
    }

    #[test]
    fn build_plan_body_capture_metadata_returns_none_without_base64_body() {
        assert!(build_plan_body_capture_metadata(None).is_none());
    }

    #[test]
    fn full_policy_never_captures_sensitive_oauth_event_bodies() {
        let mut event = sensitive_oauth_event("oauth:exchange");

        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Full,
            },
            &mut event,
        );

        assert_sensitive_bodies_disabled(
            &event.data.request_body,
            &event.data.request_body_ref,
            event.data.request_body_state,
            &event.data.provider_request_body,
            &event.data.provider_request_body_ref,
            event.data.provider_request_body_state,
            &event.data.response_body,
            &event.data.response_body_ref,
            event.data.response_body_state,
            &event.data.client_response_body,
            &event.data.client_response_body_ref,
            event.data.client_response_body_state,
            &event.data.request_metadata,
        );
    }

    #[test]
    fn full_policy_never_captures_sensitive_oauth_record_bodies() {
        let event = sensitive_oauth_event("provider_oauth:exchange");
        let mut record = crate::record::build_upsert_usage_record_from_event(&event)
            .expect("usage record should build");

        apply_usage_body_capture_policy_to_record(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Full,
            },
            &mut record,
        );

        assert_sensitive_bodies_disabled(
            &record.request_body,
            &record.request_body_ref,
            record.request_body_state,
            &record.provider_request_body,
            &record.provider_request_body_ref,
            record.provider_request_body_state,
            &record.response_body,
            &record.response_body_ref,
            record.response_body_state,
            &record.client_response_body,
            &record.client_response_body_ref,
            record.client_response_body_state,
            &record.request_metadata,
        );
    }

    #[test]
    fn full_policy_never_captures_refresh_or_service_account_token_bodies() {
        for api_format in [
            "provider_oauth:local_refresh",
            "vertex_ai:service_account_token",
        ] {
            let mut event = sensitive_oauth_event(api_format);

            apply_usage_body_capture_policy_to_event(
                UsageBodyCapturePolicy {
                    record_level: UsageRequestRecordLevel::Full,
                },
                &mut event,
            );

            assert_sensitive_bodies_disabled(
                &event.data.request_body,
                &event.data.request_body_ref,
                event.data.request_body_state,
                &event.data.provider_request_body,
                &event.data.provider_request_body_ref,
                event.data.provider_request_body_state,
                &event.data.response_body,
                &event.data.response_body_ref,
                event.data.response_body_state,
                &event.data.client_response_body,
                &event.data.client_response_body_ref,
                event.data.client_response_body_state,
                &event.data.request_metadata,
            );
        }
    }

    #[test]
    fn trim_owned_non_empty_string_preserves_clean_values_and_drops_blank_ones() {
        assert_eq!(
            trim_owned_non_empty_string("blob://body-ref-1".to_string()),
            Some("blob://body-ref-1".to_string()),
        );
        assert_eq!(
            trim_owned_non_empty_string("  blob://body-ref-1  ".to_string()),
            Some("blob://body-ref-1".to_string()),
        );
        assert_eq!(trim_owned_non_empty_string("   ".to_string()), None);
    }

    #[test]
    fn upsert_body_capture_metadata_value_entry_ignores_none_state() {
        let mut metadata = Some(Value::Object(Map::<String, Value>::new()));
        upsert_body_capture_metadata_value_entry(&mut metadata, "response", None, None, None, None);
        assert_eq!(metadata, Some(Value::Object(Map::new())));
    }

    #[test]
    fn upsert_body_capture_metadata_value_entry_preserves_existing_metadata_fields() {
        let mut metadata = Some(Value::Object(Map::from_iter([(
            "request_body_ref".to_string(),
            Value::String("blob://body-ref-1".to_string()),
        )])));

        upsert_body_capture_metadata_value_entry(
            &mut metadata,
            "response",
            Some(UsageBodyCaptureState::Reference),
            None,
            None,
            None,
        );

        assert_eq!(
            metadata,
            Some(Value::Object(Map::from_iter([
                (
                    "request_body_ref".to_string(),
                    Value::String("blob://body-ref-1".to_string()),
                ),
                (
                    "body_capture".to_string(),
                    Value::Object(Map::from_iter([(
                        "response".to_string(),
                        Value::Object(Map::from_iter([(
                            "state".to_string(),
                            Value::String("reference".to_string()),
                        )])),
                    )])),
                ),
            ]))),
        );
    }

    #[test]
    fn sync_usage_body_ref_metadata_clears_empty_metadata_object() {
        let mut metadata = Some(Value::Object(Map::from_iter([(
            "request_body_ref".to_string(),
            Value::String("blob://body-ref-1".to_string()),
        )])));

        sync_usage_body_ref_metadata(&mut metadata, UsageBodyField::RequestBody, None);

        assert!(metadata.is_none());
    }

    #[test]
    fn sync_usage_body_ref_metadata_preserves_existing_ref_value() {
        let mut metadata = Some(Value::Object(Map::from_iter([(
            "request_body_ref".to_string(),
            Value::String("blob://body-ref-1".to_string()),
        )])));

        sync_usage_body_ref_metadata(
            &mut metadata,
            UsageBodyField::RequestBody,
            Some("blob://body-ref-1"),
        );

        assert_eq!(
            metadata,
            Some(Value::Object(Map::from_iter([(
                "request_body_ref".to_string(),
                Value::String("blob://body-ref-1".to_string()),
            )]))),
        );
    }

    #[test]
    fn truncate_usage_body_string_respects_json_byte_limit() {
        let limit = 32usize;
        let truncated = truncate_usage_body_string("x".repeat(256).as_str(), limit);

        assert!(truncated.ends_with("...[truncated]"));
        assert!(serde_json::to_vec(&truncated)
            .ok()
            .is_some_and(|bytes| bytes.len() <= limit));
    }
}
