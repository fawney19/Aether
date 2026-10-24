use std::io::{self, Write};

use aether_data_contracts::repository::usage::{
    UpsertUsageRecord, UsageBodyCaptureState, UsageBodyField,
};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::event::UsageEvent;
use crate::runtime::{UsageBodyCapturePolicy, UsageRequestRecordLevel};

const TRUNCATED_BODY_STRING_SUFFIX: &str = "...[truncated]";
const BASE64_DATA_URL_PREFIX: &str = "data:";
const BASE64_DATA_URL_MARKER: &str = ";base64,";

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
            self.policy,
            payload.request_body,
            payload.request_body_ref,
            payload.request_body_state,
            payload.request_metadata,
        );
        apply_usage_body_capture_limit(
            UsageBodyField::ProviderRequestBody,
            "provider_request",
            self.policy,
            payload.provider_request_body,
            payload.provider_request_body_ref,
            payload.provider_request_body_state,
            payload.request_metadata,
        );
        apply_usage_body_capture_limit(
            UsageBodyField::ResponseBody,
            "response",
            self.policy,
            payload.response_body,
            payload.response_body_ref,
            payload.response_body_state,
            payload.request_metadata,
        );
        apply_usage_body_capture_limit(
            UsageBodyField::ClientResponseBody,
            "client_response",
            self.policy,
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
    policy: UsageBodyCapturePolicy,
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

    let Some(mut value) = body.take() else {
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

    let original_source_bytes = policy
        .compact_base64_images
        .then(|| json_serialized_len(&value))
        .flatten();
    let compacted_opaque_count = if policy.compact_base64_images {
        compact_opaque_payloads(&mut value)
    } else {
        0
    };
    let compacted_base64_count = if policy.compact_base64_images {
        compact_base64_payloads(&mut value)
    } else {
        0
    };
    let mut limited = limit_usage_body_capture_value(value, policy.max_body_bytes);
    if compacted_base64_count > 0 || compacted_opaque_count > 0 {
        limited.source_bytes = original_source_bytes.or(limited.source_bytes);
        limited.truncated = true;
        if limited.reason.is_none() {
            limited.reason = Some(
                match (compacted_base64_count > 0, compacted_opaque_count > 0) {
                    (true, true) => "base64_and_opaque_payloads_compacted",
                    (true, false) => "base64_payload_compacted",
                    (false, true) => "opaque_payload_compacted",
                    (false, false) => unreachable!(),
                },
            );
        }
    }
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

fn compact_base64_payloads(value: &mut Value) -> usize {
    compact_base64_payloads_with_context(value, None)
}

fn compact_base64_payloads_with_context(value: &mut Value, parent_key: Option<&str>) -> usize {
    match value {
        Value::String(text) => {
            if let Some((compacted, count)) = compact_base64_data_urls_in_string(text) {
                *text = compacted;
                return count;
            }
            0
        }
        Value::Array(items) => items
            .iter_mut()
            .map(|item| compact_base64_payloads_with_context(item, parent_key))
            .sum(),
        Value::Object(object) => {
            let is_image_generation_call = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("image_generation_call"));
            let is_base64_source = object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("base64"));
            let has_mime_type = ["media_type", "mime_type", "mimeType"]
                .iter()
                .any(|key| object.get(*key).and_then(Value::as_str).is_some());
            let is_base64_container = is_base64_source
                || has_mime_type
                || parent_key.is_some_and(is_base64_container_key);
            let mut compacted = 0usize;
            for (key, child) in object.iter_mut() {
                let is_base64_field = is_base64_payload_key(key)
                    || (key.eq_ignore_ascii_case("result") && is_image_generation_call)
                    || (key.eq_ignore_ascii_case("data") && is_base64_container);
                if is_base64_field {
                    if let Some(payload) = child.as_str() {
                        if let Some(original_bytes) = valid_base64_payload_decoded_len(payload) {
                            *child = Value::String(format!(
                                "<base64 payload omitted; original size: {}>",
                                format_payload_size(original_bytes)
                            ));
                            compacted = compacted.saturating_add(1);
                            continue;
                        }
                    }
                }
                compacted = compacted
                    .saturating_add(compact_base64_payloads_with_context(child, Some(key)));
            }
            compacted
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

fn is_base64_payload_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == "b64"
        || key == "b64_json"
        || key == "file_data"
        || key == "filedata"
        || key.ends_with("_b64")
        || key.ends_with("_base64")
}

fn is_base64_container_key(key: &str) -> bool {
    [
        "audio",
        "input_audio",
        "input_file",
        "inline_data",
        "inlinedata",
    ]
    .iter()
    .any(|candidate| key.eq_ignore_ascii_case(candidate))
}

fn compact_opaque_payloads(value: &mut Value) -> usize {
    match value {
        Value::Array(items) => items.iter_mut().map(compact_opaque_payloads).sum(),
        Value::Object(object) => {
            let mut compacted = 0usize;
            for (key, child) in object.iter_mut() {
                if key.eq_ignore_ascii_case("encrypted_content") {
                    if let Some(payload) = child.as_str() {
                        let original_bytes = payload.len() as u64;
                        if !payload.is_empty() {
                            *child = Value::String(format!(
                                "<encrypted content omitted; original size: {}>",
                                format_payload_size(original_bytes)
                            ));
                            compacted = compacted.saturating_add(1);
                            continue;
                        }
                    }
                } else if key.eq_ignore_ascii_case("body_bytes_b64") {
                    if let Some(payload) = child.as_str() {
                        if let Some(original_bytes) = valid_base64_payload_decoded_len(payload) {
                            *child = Value::String(format!(
                                "<raw body bytes omitted; original size: {}>",
                                format_payload_size(original_bytes)
                            ));
                            compacted = compacted.saturating_add(1);
                            continue;
                        }
                    }
                }
                compacted = compacted.saturating_add(compact_opaque_payloads(child));
            }
            compacted
        }
        Value::String(_) | Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

fn compact_base64_data_urls_in_string(value: &str) -> Option<(String, usize)> {
    let mut cursor = 0usize;
    let mut compacted = String::new();
    let mut replacement_count = 0usize;

    while let Some(start) = find_ascii_case_insensitive(value, BASE64_DATA_URL_PREFIX, cursor) {
        let metadata_start = start + BASE64_DATA_URL_PREFIX.len();
        let Some(marker) =
            find_ascii_case_insensitive(value, BASE64_DATA_URL_MARKER, metadata_start)
        else {
            cursor = metadata_start;
            continue;
        };
        let media_type = &value[metadata_start..marker];
        if !valid_base64_data_url_metadata(media_type) {
            cursor = metadata_start;
            continue;
        }

        let payload_start = marker + BASE64_DATA_URL_MARKER.len();
        let payload_len = value.as_bytes()[payload_start..]
            .iter()
            .take_while(|byte| {
                let byte = **byte;
                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_' | b'=')
            })
            .count();
        if payload_len == 0 {
            cursor = payload_start;
            continue;
        }
        let payload_end = payload_start + payload_len;
        let payload = &value[payload_start..payload_end];
        let Some(original_bytes) = valid_base64_payload_decoded_len(payload) else {
            cursor = payload_end;
            continue;
        };

        compacted.push_str(&value[cursor..start]);
        compacted.push_str(&format!(
            "<base64 payload omitted; original size: {}>",
            format_payload_size(original_bytes)
        ));
        cursor = payload_end;
        replacement_count = replacement_count.saturating_add(1);
    }

    if replacement_count == 0 {
        return None;
    }
    compacted.push_str(&value[cursor..]);
    Some((compacted, replacement_count))
}

fn find_ascii_case_insensitive(value: &str, needle: &str, from: usize) -> Option<usize> {
    let value = value.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || from > value.len() || needle.len() > value.len().saturating_sub(from) {
        return None;
    }

    (from..=value.len() - needle.len()).find(|start| {
        value[*start..*start + needle.len()]
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

fn valid_base64_data_url_metadata(value: &str) -> bool {
    let mut parts = value.split(';');
    let Some(media_type) = parts.next() else {
        return false;
    };
    if !media_type.is_empty() {
        let Some((media_type, media_subtype)) = media_type.split_once('/') else {
            return false;
        };
        if !valid_mime_token(media_type) || !valid_mime_token(media_subtype) {
            return false;
        }
    }

    parts.all(|parameter| {
        let Some((name, value)) = parameter.split_once('=') else {
            return false;
        };
        valid_mime_token(name)
            && !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b',')
    })
}

fn valid_mime_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        ..=b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
                )
        })
}

fn valid_base64_payload_decoded_len(value: &str) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    let bytes = value.as_bytes();
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 || bytes.len() % 4 == 1 || (padding > 0 && !bytes.len().is_multiple_of(4)) {
        return None;
    }
    let symbols = bytes.len().saturating_sub(padding);
    if bytes[..symbols]
        .iter()
        .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(*byte, b'+' | b'/' | b'-' | b'_'))
        || bytes[symbols..].iter().any(|byte| *byte != b'=')
    {
        return None;
    }
    if (padding == 1 && symbols % 4 != 3) || (padding == 2 && symbols % 4 != 2) {
        return None;
    }

    let decoded = (symbols / 4)
        .saturating_mul(3)
        .saturating_add(match symbols % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        });
    u64::try_from(decoded).ok()
}

fn format_payload_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
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
        build_plan_body_capture_metadata, compact_base64_payloads, sync_usage_body_ref_metadata,
        trim_owned_non_empty_string, truncate_usage_body_string,
        upsert_body_capture_metadata_value_entry,
    };
    use aether_data_contracts::repository::usage::UsageBodyCaptureState;
    use aether_data_contracts::repository::usage::UsageBodyField;
    use base64::Engine as _;
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
                ..UsageBodyCapturePolicy::default()
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
                ..UsageBodyCapturePolicy::default()
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
                    ..UsageBodyCapturePolicy::default()
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

    #[test]
    fn compact_base64_payloads_replaces_nested_data_urls_with_size_placeholders() {
        let mut body = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "input_image",
                        "image_url": "data:image/png;base64,aGVsbG8="
                    },
                    {
                        "type": "input_text",
                        "text": "preview: data:image/jpeg;base64,d29ybGQ= end"
                    }
                ]
            }],
            "remote_image": "https://example.com/image.png",
            "other_data_url": "data:text/plain;base64,aGVsbG8=",
            "plain_base64": "aGVsbG8=",
            "status": "ok"
        });

        let compacted_count = compact_base64_payloads(&mut body);

        assert_eq!(compacted_count, 4);
        assert_eq!(
            body["messages"][0]["content"][0]["image_url"],
            "<base64 payload omitted; original size: 5 B>"
        );
        assert_eq!(
            body["messages"][0]["content"][1]["text"],
            "preview: <base64 payload omitted; original size: 5 B> end"
        );
        assert_eq!(body["remote_image"], "https://example.com/image.png");
        assert_eq!(
            body["other_data_url"],
            "<base64 payload omitted; original size: 5 B>"
        );
        assert_eq!(
            body["plain_base64"],
            "<base64 payload omitted; original size: 5 B>"
        );
        assert_eq!(body["status"], "ok");
    }

    #[test]
    fn compact_base64_payloads_cover_multiple_api_formats_and_mime_types() {
        let encoded_image = base64::engine::general_purpose::STANDARD.encode(vec![7u8; 4096]);
        let mut body = json!({
            "input": format!("prefix DATA:IMAGE/PNG;charset=utf-8;BASE64,{encoded_image} suffix"),
            "data": [{"b64_json": encoded_image}],
            "stream_event": {
                "type": "response.image_generation_call.partial_image",
                "partial_image_b64": encoded_image
            },
            "gemini_camel_case": {
                "inlineData": {"mimeType": "image/png", "data": encoded_image}
            },
            "gemini_snake_case": {
                "inline_data": {"mime_type": "image/webp", "data": encoded_image}
            },
            "claude": {
                "source": {"type": "base64", "media_type": "image/jpeg", "data": encoded_image}
            },
            "document": {
                "source": {"type": "base64", "media_type": "application/pdf", "data": encoded_image}
            },
            "document_data_url": format!("data:application/pdf;name=scan.pdf;base64,{encoded_image}"),
            "output": [{
                "type": "image_generation_call",
                "result": encoded_image
            }],
            "file_data": encoded_image,
            "audio": {"input_audio": {"data": encoded_image}},
            "other": "data:image/png;broken;base64,aGVsbG8="
        });

        let compacted_count = compact_base64_payloads(&mut body);

        assert_eq!(compacted_count, 11);
        assert_eq!(
            body["input"],
            "prefix <base64 payload omitted; original size: 4.00 KiB> suffix"
        );
        assert_eq!(
            body["data"][0]["b64_json"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["stream_event"]["partial_image_b64"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["gemini_camel_case"]["inlineData"]["data"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["gemini_snake_case"]["inline_data"]["data"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["claude"]["source"]["data"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["document"]["source"]["data"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["document_data_url"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["output"][0]["result"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["file_data"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(
            body["audio"]["input_audio"]["data"],
            "<base64 payload omitted; original size: 4.00 KiB>"
        );
        assert_eq!(body["other"], "data:image/png;broken;base64,aGVsbG8=");
    }

    #[test]
    fn base64_compaction_also_replaces_opaque_audit_fields_at_any_size() {
        let encoded_body = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        let encrypted_content = "state".to_string();
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-compact-opaque",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                request_body: Some(json!({
                    "prompt": "keep this human-readable text",
                    "body_bytes_b64": encoded_body,
                    "input": [{"type": "reasoning", "encrypted_content": encrypted_content.clone()}]
                })),
                provider_request_body: Some(json!({
                    "output": [{"type": "reasoning", "encrypted_content": encrypted_content.clone()}]
                })),
                response_body: Some(json!({
                    "output": [{"type": "reasoning", "encrypted_content": encrypted_content.clone()}]
                })),
                client_response_body: Some(json!({
                    "output": [{"type": "reasoning", "encrypted_content": encrypted_content}]
                })),
                ..UsageEventData::default()
            },
        );

        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Full,
                compact_base64_images: true,
                ..UsageBodyCapturePolicy::default()
            },
            &mut event,
        );

        let body = event
            .data
            .request_body
            .as_ref()
            .expect("request body should remain");
        assert_eq!(body["prompt"], "keep this human-readable text");
        assert_eq!(
            body["body_bytes_b64"],
            "<raw body bytes omitted; original size: 3 B>"
        );
        assert_eq!(
            body["input"][0]["encrypted_content"],
            "<encrypted content omitted; original size: 5 B>"
        );
        for (field, body) in [
            (
                "provider_request",
                event.data.provider_request_body.as_ref(),
            ),
            ("response", event.data.response_body.as_ref()),
            ("client_response", event.data.client_response_body.as_ref()),
        ] {
            assert_eq!(
                body.expect("audit body should remain")["output"][0]["encrypted_content"],
                "<encrypted content omitted; original size: 5 B>",
                "{field} body should have its opaque audit payload compacted"
            );
        }
        assert_eq!(
            event.data.request_body_state,
            Some(UsageBodyCaptureState::Truncated)
        );
        let capture = &event.data.request_metadata.as_ref().unwrap()["body_capture"];
        for field in ["request", "provider_request", "response", "client_response"] {
            assert_eq!(capture[field]["reason"], "opaque_payload_compacted");
            assert_eq!(capture[field]["state"], "truncated");
            assert!(capture[field]["source_bytes"].as_u64().is_some());
            assert!(capture[field]["stored_bytes"].as_u64().is_some());
        }
    }

    #[test]
    fn opaque_payload_fields_are_preserved_when_compaction_is_disabled() {
        let encoded_body = base64::engine::general_purpose::STANDARD.encode(vec![1u8; 4096]);
        let event = UsageEvent::new(
            UsageEventType::Completed,
            "req-keep-opaque",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                request_body: Some(json!({
                    "body_bytes_b64": encoded_body,
                    "encrypted_content": "x".repeat(8192)
                })),
                ..UsageEventData::default()
            },
        );
        let mut event = event;

        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Full,
                ..UsageBodyCapturePolicy::default()
            },
            &mut event,
        );

        let body = event
            .data
            .request_body
            .as_ref()
            .expect("request body should remain");
        assert_eq!(body["body_bytes_b64"], encoded_body);
        assert_eq!(body["encrypted_content"], "x".repeat(8192));
        assert_eq!(
            event.data.request_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
    }

    #[test]
    fn body_capture_policy_compacts_base64_before_enforcing_the_size_limit() {
        let encoded_image = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 1536]);
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-compact-images",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-image".to_string(),
                request_body: Some(json!({
                    "input": [{
                        "type": "input_image",
                        "image_url": format!("data:image/png;base64,{encoded_image}")
                    }]
                })),
                response_body: Some(json!({"status": "ok"})),
                ..UsageEventData::default()
            },
        );

        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Full,
                compact_base64_images: true,
                max_body_bytes: Some(1024),
            },
            &mut event,
        );

        assert_eq!(
            event.data.request_body_state,
            Some(UsageBodyCaptureState::Truncated)
        );
        assert_eq!(
            event.data.request_body.as_ref().unwrap()["input"][0]["image_url"],
            "<base64 payload omitted; original size: 1.50 KiB>"
        );
        assert_eq!(
            event.data.response_body_state,
            Some(UsageBodyCaptureState::Inline)
        );
        let request_capture =
            &event.data.request_metadata.as_ref().unwrap()["body_capture"]["request"];
        assert_eq!(request_capture["reason"], "base64_payload_compacted");
        assert!(
            request_capture["source_bytes"].as_u64().unwrap()
                > request_capture["stored_bytes"].as_u64().unwrap()
        );
    }

    #[test]
    fn body_capture_policy_replaces_oversized_json_with_truncation_metadata() {
        let mut event = UsageEvent::new(
            UsageEventType::Completed,
            "req-body-limit",
            UsageEventData {
                provider_name: "OpenAI".to_string(),
                model: "gpt-5".to_string(),
                request_body: Some(json!({"prompt": "x".repeat(2048)})),
                ..UsageEventData::default()
            },
        );

        apply_usage_body_capture_policy_to_event(
            UsageBodyCapturePolicy {
                record_level: UsageRequestRecordLevel::Full,
                compact_base64_images: false,
                max_body_bytes: Some(1024),
            },
            &mut event,
        );

        assert_eq!(
            event.data.request_body_state,
            Some(UsageBodyCaptureState::Truncated)
        );
        assert_eq!(
            event.data.request_body.as_ref().unwrap()["reason"],
            "body_capture_limit_exceeded"
        );
        assert_eq!(
            event.data.request_metadata.as_ref().unwrap()["body_capture"]["request"]
                ["source_bytes"],
            2061
        );
    }
}
