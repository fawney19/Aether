use std::collections::BTreeMap;
use std::io::{self, Write};

use aether_data_contracts::repository::usage::{
    resolve_provider_cache_ttl_minutes, UsageBodyCaptureState,
    PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY, PROVIDER_REASONING_EFFORT_METADATA_KEY,
    PROVIDER_SERVICE_TIER_METADATA_KEY, REQUESTED_REASONING_EFFORT_METADATA_KEY,
};
use aether_data_contracts::DataLayerError;
use serde::ser::{Impossible, SerializeMap, SerializeStruct};
use serde::{Serialize, Serializer};
use serde_json::Value;

use super::{BorrowedUsageEventEnvelope, UsageEvent, UsageEventData, USAGE_EVENT_VERSION};
use crate::body_capture::mark_usage_event_capture_truncated;
use crate::request_metadata::{
    attach_client_request_body_metadata, attach_provider_request_body_metadata,
    attach_provider_response_body_metadata, clear_client_request_body_metadata,
    clear_provider_request_body_metadata, request_body_derived_facts_action,
    RequestBodyDerivedFactsAction,
};

const DIAGNOSTIC_FIELDS: [&str; 8] = [
    "request_body",
    "provider_request_body",
    "response_body",
    "client_response_body",
    "request_headers",
    "provider_request_headers",
    "response_headers",
    "client_response_headers",
];
const BODY_STATE_FIELDS: [&str; 4] = [
    "request_body_state",
    "provider_request_body_state",
    "response_body_state",
    "client_response_body_state",
];
const BODY_METADATA_KEYS: [&str; 4] =
    ["request", "provider_request", "response", "client_response"];

#[derive(Debug)]
pub(crate) struct EncodedUsageEvent {
    pub(crate) fields: BTreeMap<String, String>,
    pub(crate) diagnostics_omitted: bool,
}

pub(super) fn encode(
    event: &UsageEvent,
    max_bytes: usize,
) -> Result<EncodedUsageEvent, DataLayerError> {
    let mut writer = BoundedJsonWriter::new(max_bytes);
    if writer.serialize(&envelope(event, &event.data))? {
        return writer.into_event(false);
    }

    // Conservatively reject oversized original metadata before cloning it, even
    // when later fact normalization could make that metadata smaller.
    let core = ProjectedData {
        data: &event.data,
        overrides: None,
    };
    if !writer.serialize(&envelope(event, &core))? {
        return Err(wire_limit_error(max_bytes));
    }

    let overrides = WireOverrides::new(&event.data)?;
    let projected = ProjectedData {
        data: &event.data,
        overrides: Some(&overrides),
    };
    if !writer.serialize(&envelope(event, &projected))? {
        return Err(wire_limit_error(max_bytes));
    }
    writer.into_event(true)
}

fn envelope<'a, T: Serialize + ?Sized>(
    event: &'a UsageEvent,
    data: &'a T,
) -> BorrowedUsageEventEnvelope<'a, T> {
    BorrowedUsageEventEnvelope {
        v: USAGE_EVENT_VERSION,
        event_type: event.event_type,
        request_id: &event.request_id,
        timestamp_ms: event.timestamp_ms,
        data,
    }
}

fn wire_limit_error(max_bytes: usize) -> DataLayerError {
    DataLayerError::InvalidInput(format!(
        "usage event exceeds the {max_bytes}-byte wire limit after omitting diagnostic bodies and headers"
    ))
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    exceeded: bool,
}

impl BoundedJsonWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
            exceeded: false,
        }
    }

    fn serialize<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<bool, DataLayerError> {
        self.bytes.clear();
        self.exceeded = false;
        match serde_json::to_writer(&mut *self, value) {
            Ok(()) => Ok(true),
            Err(_) if self.exceeded => Ok(false),
            Err(error) => Err(DataLayerError::UnexpectedValue(format!(
                "failed to serialize usage event payload: {error}"
            ))),
        }
    }

    fn into_event(self, diagnostics_omitted: bool) -> Result<EncodedUsageEvent, DataLayerError> {
        let payload = String::from_utf8(self.bytes).map_err(|error| {
            DataLayerError::UnexpectedValue(format!("usage event JSON was not UTF-8: {error}"))
        })?;
        Ok(EncodedUsageEvent {
            fields: BTreeMap::from([("payload".to_string(), payload)]),
            diagnostics_omitted,
        })
    }
}

impl Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.max_bytes.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("usage event wire limit exceeded"));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.max_bytes);
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|error| {
                    io::Error::other(format!("usage event wire allocation failed: {error}"))
                })?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct WireOverrides {
    truncated: [bool; 4],
    metadata: Option<Value>,
}

impl WireOverrides {
    fn new(data: &UsageEventData) -> Result<Self, DataLayerError> {
        // The full v1 consumer decodes a JSON null Option<Value> as no body.
        let request_body = data.request_body.as_ref().filter(|body| !body.is_null());
        let provider_request_body = data
            .provider_request_body
            .as_ref()
            .filter(|body| !body.is_null());
        let body_cache_ttl = resolve_provider_cache_ttl_minutes(
            data.endpoint_api_format
                .as_deref()
                .or(data.api_format.as_deref()),
            data.target_model.as_deref().or(Some(data.model.as_str())),
            Some(data.model.as_str()),
            provider_request_body,
        );
        if body_cache_ttl.is_some()
            && data.provider_request_body_state == Some(UsageBodyCaptureState::None)
        {
            return Err(DataLayerError::InvalidInput(
                "usage event cannot omit a provider request body whose cache TTL would be cleared by its explicit none capture state"
                    .to_string(),
            ));
        }
        let mut metadata = data.request_metadata.clone();
        match request_body_derived_facts_action(request_body, data.request_body_state) {
            RequestBodyDerivedFactsAction::Refresh => {
                if request_body.is_some_and(|body| !body.is_object()) {
                    if let Some(Value::Object(object)) = metadata.as_mut() {
                        object.remove(REQUESTED_REASONING_EFFORT_METADATA_KEY);
                    }
                } else {
                    metadata = attach_client_request_body_metadata(metadata, request_body);
                }
            }
            RequestBodyDerivedFactsAction::Clear
                if request_body.is_some() || data.request_body_state.is_some() =>
            {
                metadata = clear_client_request_body_metadata(metadata);
            }
            RequestBodyDerivedFactsAction::Clear | RequestBodyDerivedFactsAction::Preserve => {}
        }
        match request_body_derived_facts_action(
            provider_request_body,
            data.provider_request_body_state,
        ) {
            RequestBodyDerivedFactsAction::Refresh => {
                if provider_request_body.is_some_and(|body| !body.is_object()) {
                    // An authoritative scalar/array has no tier or reasoning,
                    // but billing still falls back to the metadata's cache TTL.
                    if let Some(Value::Object(object)) = metadata.as_mut() {
                        object.remove(PROVIDER_REASONING_EFFORT_METADATA_KEY);
                        object.remove(PROVIDER_SERVICE_TIER_METADATA_KEY);
                    }
                } else {
                    metadata = attach_provider_request_body_metadata(
                        metadata,
                        data.endpoint_api_format
                            .as_deref()
                            .or(data.api_format.as_deref()),
                        data.target_model.as_deref().or(Some(data.model.as_str())),
                        Some(data.model.as_str()),
                        provider_request_body,
                    );
                }
            }
            RequestBodyDerivedFactsAction::Clear
                if provider_request_body.is_some()
                    || data.provider_request_body_state.is_some() =>
            {
                metadata = clear_provider_request_body_metadata(metadata);
            }
            RequestBodyDerivedFactsAction::Clear | RequestBodyDerivedFactsAction::Preserve => {}
        }
        metadata = attach_provider_response_body_metadata(metadata, data.response_body.as_ref());
        // Billing reads raw-body TTL before metadata regardless of capture state.
        // Preserve that precedence independently of reasoning and tier authority.
        if let Some(cache_ttl) = body_cache_ttl {
            let object = metadata
                .get_or_insert_with(|| Value::Object(serde_json::Map::new()))
                .as_object_mut()
                .ok_or_else(|| {
                    DataLayerError::InvalidInput(
                        "usage event cannot preserve provider cache TTL in non-object metadata after omitting diagnostic bodies"
                            .to_string(),
                    )
                })?;
            object.insert(
                PROVIDER_CACHE_TTL_MINUTES_METADATA_KEY.to_string(),
                Value::Number(cache_ttl.into()),
            );
        }
        let truncated = [
            (data.request_body.as_ref(), data.request_body_state),
            (
                data.provider_request_body.as_ref(),
                data.provider_request_body_state,
            ),
            (data.response_body.as_ref(), data.response_body_state),
            (
                data.client_response_body.as_ref(),
                data.client_response_body_state,
            ),
        ]
        .map(|(body, state)| {
            body.is_some_and(|body| !body.is_null())
                && !matches!(
                    state,
                    Some(
                        UsageBodyCaptureState::None
                            | UsageBodyCaptureState::Disabled
                            | UsageBodyCaptureState::Unavailable
                    )
                )
        });
        for (truncated, key) in truncated.into_iter().zip(BODY_METADATA_KEYS) {
            if !truncated {
                continue;
            }
            mark_usage_event_capture_truncated(&mut metadata, key);
            if let Some(entry) = metadata
                .as_mut()
                .and_then(|value| value.get_mut("body_capture"))
                .and_then(|value| value.get_mut(key))
                .and_then(Value::as_object_mut)
            {
                entry.insert(
                    "reason".to_string(),
                    Value::String("wire_limit_exceeded".to_string()),
                );
            }
        }
        Ok(Self {
            truncated,
            metadata,
        })
    }
}

struct ProjectedData<'a> {
    data: &'a UsageEventData,
    overrides: Option<&'a WireOverrides>,
}

impl Serialize for ProjectedData<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.data.serialize(FieldProjectionSerializer {
            map: serializer.serialize_map(None)?,
            overrides: self.overrides,
        })
    }
}

// Reuse UsageEventData's derived field traversal, including future fields and
// skip_serializing_if rules. Only diagnostic fields and explicit overrides differ.
struct FieldProjectionSerializer<'a, M> {
    map: M,
    overrides: Option<&'a WireOverrides>,
}

impl<M: SerializeMap> SerializeStruct for FieldProjectionSerializer<'_, M> {
    type Ok = M::Ok;
    type Error = M::Error;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if DIAGNOSTIC_FIELDS.contains(&key) {
            return Ok(());
        }
        if let Some(overrides) = self.overrides {
            if key == "request_metadata"
                || BODY_STATE_FIELDS
                    .iter()
                    .zip(overrides.truncated)
                    .any(|(state, truncated)| *state == key && truncated)
            {
                return Ok(());
            }
        }
        self.map.serialize_entry(key, value)
    }

    fn end(mut self) -> Result<Self::Ok, Self::Error> {
        if let Some(overrides) = self.overrides {
            for (key, truncated) in BODY_STATE_FIELDS.into_iter().zip(overrides.truncated) {
                if truncated {
                    self.map
                        .serialize_entry(key, &UsageBodyCaptureState::Truncated)?;
                }
            }
            if let Some(metadata) = overrides.metadata.as_ref() {
                self.map.serialize_entry("request_metadata", metadata)?;
            }
        }
        self.map.end()
    }
}

fn expected_struct<E: serde::ser::Error, T>() -> Result<T, E> {
    Err(E::custom("usage event data must serialize as a struct"))
}

macro_rules! reject_scalar_serialization {
    ($($name:ident($value:ident: $ty:ty)),* $(,)?) => {
        $(fn $name(self, $value: $ty) -> Result<Self::Ok, Self::Error> {
            let _ = $value;
            expected_struct()
        })*
    };
}

impl<M: SerializeMap> Serializer for FieldProjectionSerializer<'_, M> {
    type Ok = M::Ok;
    type Error = M::Error;
    type SerializeSeq = Impossible<Self::Ok, Self::Error>;
    type SerializeTuple = Impossible<Self::Ok, Self::Error>;
    type SerializeTupleStruct = Impossible<Self::Ok, Self::Error>;
    type SerializeTupleVariant = Impossible<Self::Ok, Self::Error>;
    type SerializeMap = Impossible<Self::Ok, Self::Error>;
    type SerializeStruct = Self;
    type SerializeStructVariant = Impossible<Self::Ok, Self::Error>;

    reject_scalar_serialization! {
        serialize_bool(value: bool), serialize_i8(value: i8), serialize_i16(value: i16),
        serialize_i32(value: i32), serialize_i64(value: i64), serialize_i128(value: i128),
        serialize_u8(value: u8), serialize_u16(value: u16), serialize_u32(value: u32),
        serialize_u64(value: u64), serialize_u128(value: u128), serialize_f32(value: f32),
        serialize_f64(value: f64), serialize_char(value: char), serialize_str(value: &str),
        serialize_bytes(value: &[u8]),
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        expected_struct()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<Self::Ok, Self::Error> {
        expected_struct()
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        expected_struct()
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Self::Ok, Self::Error> {
        expected_struct()
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        expected_struct()
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<Self::Ok, Self::Error> {
        expected_struct()
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<Self::Ok, Self::Error> {
        expected_struct()
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        expected_struct()
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Self::Error> {
        expected_struct()
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        expected_struct()
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        expected_struct()
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        expected_struct()
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(self)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        expected_struct()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::event::UsageEventType;
    use crate::event_capture_budget::EventCaptureMemoryBudget;

    fn event() -> UsageEvent {
        UsageEvent {
            event_type: UsageEventType::Completed,
            request_id: "wire-request".to_string(),
            timestamp_ms: 1_234_567,
            data: UsageEventData {
                provider_name: "provider".to_string(),
                model: "gpt-5.6-sol".to_string(),
                endpoint_api_format: Some("openai:responses".to_string()),
                user_id: Some("user-id".to_string()),
                api_key_id: Some("key-id".to_string()),
                provider_id: Some("provider-id".to_string()),
                provider_endpoint_id: Some("endpoint-id".to_string()),
                provider_api_key_id: Some("provider-key-id".to_string()),
                input_tokens: Some(100),
                output_tokens: Some(500),
                total_tokens: Some(600),
                cache_read_input_tokens: Some(0),
                cache_creation_input_tokens: Some(25),
                cache_creation_ephemeral_5m_input_tokens: Some(0),
                cache_creation_ephemeral_1h_input_tokens: Some(25),
                cache_read_cost_usd: Some(0.0),
                total_cost_usd: Some(1.25),
                actual_total_cost_usd: Some(1.25),
                status_code: Some(200),
                is_stream: Some(false),
                candidate_index: Some(0),
                first_byte_time_ms: Some(0),
                error_message: Some(String::new()),
                request_metadata: Some(json!({
                    "plan_usage_reservation_token": "550e8400-e29b-41d4-a716-446655440000",
                    "dimensions": {"image_count": 2, "size": "1024x1024", "quality": "high"},
                    "usage_available": true,
                    "usage_pricing_available": true
                })),
                ..UsageEventData::default()
            },
        }
    }

    fn wire_value(encoded: &EncodedUsageEvent) -> Value {
        serde_json::from_str(&encoded.fields["payload"]).expect("complete JSON wire payload")
    }

    #[test]
    fn event_wire_exact_limit_accounts_for_json_escaping_and_utf8() {
        let mut event = event();
        event.request_id = "escaped\0\n\r\t\"\\\u{03bb}\u{1f600}".to_string();
        let original = event.to_stream_fields().expect("original wire payload");
        let length = original["payload"].len();
        for limit in [length, length + 1] {
            let encoded = event.to_bounded_stream_fields(limit).expect("exact fit");
            assert_eq!(encoded.fields, original);
            assert!(!encoded.diagnostics_omitted);
        }
        for limit in [0, 1, length - 1] {
            assert!(matches!(
                event.to_bounded_stream_fields(limit),
                Err(DataLayerError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn event_wire_writer_does_not_append_past_limit_and_reuses_failed_buffer() {
        let mut writer = BoundedJsonWriter::new(5);
        writer.write_all(b"12345").expect("exact fit");
        assert!(writer.write_all(b"6").is_err());
        assert_eq!(writer.bytes, b"12345");
        assert!(writer.exceeded);
        assert!(!writer.serialize(&"\u{0000}").expect("size rejection"));
        assert!(writer.bytes.len() <= 5);
        assert!(writer.serialize(&"\u{03bb}").expect("valid UTF-8 retry"));
        assert_eq!(writer.bytes, "\"\u{03bb}\"".as_bytes());
        assert!(!writer.exceeded);
    }

    #[test]
    fn event_wire_projection_transparently_forwards_derived_fields() {
        #[derive(Serialize)]
        struct FutureFields<'a> {
            request_body: &'a Value,
            new_billing_field: &'a Value,
            zero_count: u64,
            enabled: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            missing_field: Option<&'a str>,
        }
        let diagnostic = json!({"large": "x".repeat(8_192)});
        let billing = json!({"nested": [0, false, "unchanged"]});
        let data = FutureFields {
            request_body: &diagnostic,
            new_billing_field: &billing,
            zero_count: 0,
            enabled: false,
            missing_field: None,
        };
        let mut bytes = Vec::new();
        let mut serializer = serde_json::Serializer::new(&mut bytes);
        data.serialize(FieldProjectionSerializer {
            map: (&mut serializer)
                .serialize_map(None)
                .expect("object serializer"),
            overrides: None,
        })
        .expect("project derived fields");
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).expect("projected JSON"),
            json!({"new_billing_field": billing, "zero_count": 0, "enabled": false})
        );
    }

    #[test]
    fn event_wire_omission_preserves_billing_facts_refs_and_source_ownership() {
        let mut event = event();
        let padding = "x".repeat(16_384);
        event.data.request_body = Some(json!({"reasoning": {"effort": "high"}, "input": padding}));
        event.data.provider_request_body = Some(
            json!({"model": "gpt-5.6-sol", "reasoning": {"effort": "medium"}, "service_tier": "priority", "input": padding}),
        );
        event.data.response_body = Some(json!({"service_tier": "Default", "output": padding}));
        event.data.client_response_body = Some(json!({"output": padding}));
        event.data.request_headers = Some(json!({"x-request": padding}));
        event.data.provider_request_headers = Some(json!({"x-provider": padding}));
        event.data.response_headers = Some(json!({"x-response": padding}));
        event.data.client_response_headers = Some(json!({"x-client": padding}));
        event.data.request_body_ref = Some("usage://wire-request/request_body".to_string());
        event.data.request_body_state = Some(UsageBodyCaptureState::Inline);
        event.data.provider_request_body_state = Some(UsageBodyCaptureState::Inline);
        event.data.response_body_state = Some(UsageBodyCaptureState::Inline);
        event.data.request_metadata.as_mut().unwrap()["body_capture"] = json!({
            "response": {"state": "inline", "source_bytes": 123_456}
        });
        let weight = event.data.capture_heap_estimate();
        let budget = Arc::new(EventCaptureMemoryBudget::new(weight));
        event.data.apply_capture_memory_budget(Arc::clone(&budget));
        let before = serde_json::to_value(&event).expect("source snapshot");
        let original_wire: Value =
            serde_json::from_str(&event.to_stream_fields().unwrap()["payload"]).unwrap();
        let encoded = event
            .to_bounded_stream_fields(8_192)
            .expect("diagnostic omission");
        assert!(encoded.diagnostics_omitted);
        assert!(encoded.fields["payload"].len() <= 8_192);
        let value = wire_value(&encoded);
        for field in DIAGNOSTIC_FIELDS {
            assert!(
                value["data"].get(field).is_none(),
                "{field} must be omitted"
            );
        }
        for (field, original) in original_wire["data"].as_object().unwrap() {
            if !DIAGNOSTIC_FIELDS.contains(&field.as_str())
                && !BODY_STATE_FIELDS.contains(&field.as_str())
                && field != "request_metadata"
            {
                assert_eq!(&value["data"][field], original, "core field {field}");
            }
        }
        for (field, key) in BODY_STATE_FIELDS.into_iter().zip(BODY_METADATA_KEYS) {
            assert_eq!(value["data"][field], "truncated");
            assert_eq!(
                value["data"]["request_metadata"]["body_capture"][key]["reason"],
                "wire_limit_exceeded"
            );
            assert_eq!(
                value["data"]["request_metadata"]["body_capture"][key]["stored_bytes"],
                0
            );
        }
        let metadata = &value["data"]["request_metadata"];
        assert_eq!(metadata["requested_reasoning_effort"], "high");
        assert_eq!(metadata["provider_reasoning_effort"], "medium");
        assert_eq!(metadata["provider_service_tier"], "priority");
        assert_eq!(metadata["provider_actual_service_tier"], "default");
        assert_eq!(metadata["provider_cache_ttl_minutes"], 30);
        assert_eq!(
            metadata["body_capture"]["response"]["source_bytes"],
            123_456
        );
        assert_eq!(
            metadata["dimensions"],
            before["data"]["request_metadata"]["dimensions"]
        );
        assert_eq!(serde_json::to_value(&event).unwrap(), before);
        assert_eq!(budget.retained_bytes(), weight);
        assert_eq!(budget.downgraded_total(), 0);

        let decoded = UsageEvent::from_stream_fields(&encoded.fields).expect("wire decode");
        let record = crate::build_upsert_usage_record_from_event(&decoded).expect("record mapping");
        assert_eq!(record.input_tokens, Some(100));
        assert_eq!(record.output_tokens, Some(500));
        assert_eq!(record.cache_read_input_tokens, Some(0));
        assert_eq!(record.cache_creation_ephemeral_5m_input_tokens, Some(0));
        assert_eq!(record.cache_creation_ephemeral_1h_input_tokens, Some(25));
        assert_eq!(record.error_message.as_deref(), Some(""));
        assert_eq!(record.request_body_ref, event.data.request_body_ref);
        assert_eq!(
            record.request_body_state,
            Some(UsageBodyCaptureState::Truncated)
        );
        let metadata = record.request_metadata.expect("record billing metadata");
        assert_eq!(metadata["provider_cache_ttl_minutes"], 30);
        assert_eq!(metadata["dimensions"]["image_count"], 2);
        drop(event);
        assert_eq!(budget.retained_bytes(), 0);
    }

    #[test]
    fn event_wire_preserves_explicit_capture_states_and_legacy_metadata() {
        for state in [
            None,
            Some(UsageBodyCaptureState::None),
            Some(UsageBodyCaptureState::Disabled),
            Some(UsageBodyCaptureState::Unavailable),
            Some(UsageBodyCaptureState::Reference),
        ] {
            let mut event = event();
            event.data.request_body_state = state;
            event.data.provider_request_body_state = state;
            event.data.response_body_state = state;
            event.data.client_response_body_state = state;
            event.data.request_body_ref = Some("usage://wire-request/request_body".to_string());
            event.data.response_headers = Some(json!({"large-header": "x".repeat(16_384)}));
            event.data.request_metadata = Some(json!({
                "requested_reasoning_effort": "high",
                "provider_reasoning_effort": "medium",
                "provider_service_tier": "priority",
                "provider_cache_ttl_minutes": 60,
                "provider_actual_service_tier": "flex"
            }));
            let before = serde_json::to_value(&event).unwrap();
            let encoded = event
                .to_bounded_stream_fields(4_096)
                .expect("headers omitted");
            let value = wire_value(&encoded);
            for field in BODY_STATE_FIELDS {
                assert_eq!(value["data"].get(field), before["data"].get(field));
            }
            let metadata = &value["data"]["request_metadata"];
            if state == Some(UsageBodyCaptureState::None) {
                assert!(metadata.get("requested_reasoning_effort").is_none());
                assert!(metadata.get("provider_cache_ttl_minutes").is_none());
            } else {
                assert_eq!(metadata["requested_reasoning_effort"], "high");
                assert_eq!(metadata["provider_cache_ttl_minutes"], 60);
            }
            assert_eq!(metadata["provider_actual_service_tier"], "flex");
            assert_eq!(
                value["data"]["request_body_ref"],
                before["data"]["request_body_ref"]
            );
            assert_eq!(serde_json::to_value(&event).unwrap(), before);
            if matches!(
                state,
                Some(
                    UsageBodyCaptureState::None
                        | UsageBodyCaptureState::Disabled
                        | UsageBodyCaptureState::Unavailable
                )
            ) {
                event.data.endpoint_api_format = Some("claude:messages".to_string());
                event.data.request_body = Some(json!({"reasoning": {"effort": "low"}}));
                event.data.provider_request_body =
                    Some(json!({"reasoning": {"effort": "low"}, "service_tier": "default"}));
                event.data.response_body = Some(json!({"service_tier": "priority"}));
                event.data.client_response_body = Some(json!({"stale": true}));
                let baseline = UsageEvent::from_stream_fields_with_capture_budget(
                    &event.to_stream_fields().expect("full v1 fields"),
                    Arc::new(EventCaptureMemoryBudget::new(usize::MAX)),
                )
                .expect("full v1 consumer baseline");
                let with_stale_bodies = wire_value(
                    &event
                        .to_bounded_stream_fields(4_096)
                        .expect("explicit capture states override stale bodies"),
                );
                for field in BODY_STATE_FIELDS {
                    assert_eq!(
                        with_stale_bodies["data"].get(field),
                        value["data"].get(field)
                    );
                }
                assert_eq!(
                    with_stale_bodies["data"]["request_metadata"],
                    serde_json::to_value(&baseline.data.request_metadata).unwrap(),
                    "wire omission must preserve the full v1 consumer's billing facts"
                );
            }
        }
    }

    #[test]
    fn event_wire_preserves_raw_body_cache_ttl_across_capture_states() {
        use aether_data_contracts::repository::usage::extract_provider_cache_ttl_minutes_from_metadata;

        for state in [
            None,
            Some(UsageBodyCaptureState::Inline),
            Some(UsageBodyCaptureState::Reference),
            Some(UsageBodyCaptureState::Truncated),
            Some(UsageBodyCaptureState::Disabled),
            Some(UsageBodyCaptureState::Unavailable),
        ] {
            let mut event = event();
            event.data.provider_request_body_state = state;
            event.data.provider_request_body = Some(json!({
                "prompt_cache_options": {"ttl": "30m"},
                "service_tier": "default",
                "reasoning": {"effort": "low"}
            }));
            event.data.response_headers = Some(json!({"large": "x".repeat(16_384)}));
            event.data.request_metadata = Some(json!({
                "provider_cache_ttl_minutes": 60,
                "provider_service_tier": "priority",
                "provider_reasoning_effort": "high"
            }));
            let before = serde_json::to_value(&event).unwrap();
            let baseline = UsageEvent::from_stream_fields_with_capture_budget(
                &event.to_stream_fields().expect("full v1 fields"),
                Arc::new(EventCaptureMemoryBudget::new(usize::MAX)),
            )
            .expect("full v1 consumer baseline");
            let encoded = event.to_bounded_stream_fields(4_096).expect("body omitted");
            assert!(encoded.diagnostics_omitted);
            let decoded = UsageEvent::from_stream_fields_with_capture_budget(
                &encoded.fields,
                Arc::new(EventCaptureMemoryBudget::new(usize::MAX)),
            )
            .expect("projected consumer event");
            assert!(decoded.data.provider_request_body.is_none());
            assert_eq!(
                extract_provider_cache_ttl_minutes_from_metadata(
                    decoded.data.request_metadata.as_ref()
                ),
                Some(30),
                "raw body TTL must win over stale metadata for {state:?}"
            );
            for field in ["provider_service_tier", "provider_reasoning_effort"] {
                assert_eq!(
                    decoded.data.request_metadata.as_ref().unwrap().get(field),
                    baseline.data.request_metadata.as_ref().unwrap().get(field),
                    "TTL preservation must not change {field} authority for {state:?}"
                );
            }
            assert_eq!(serde_json::to_value(&event).unwrap(), before);
        }
    }

    #[test]
    fn event_wire_non_object_bodies_match_full_consumer_facts() {
        use aether_data_contracts::repository::usage::{
            extract_provider_cache_ttl_minutes_from_metadata,
            resolve_provider_service_tier_from_request_capture,
        };

        for body in [
            json!(null),
            json!("opaque"),
            json!([]),
            json!(42),
            json!(true),
        ] {
            for state in [
                None,
                Some(UsageBodyCaptureState::None),
                Some(UsageBodyCaptureState::Inline),
                Some(UsageBodyCaptureState::Reference),
                Some(UsageBodyCaptureState::Truncated),
                Some(UsageBodyCaptureState::Disabled),
                Some(UsageBodyCaptureState::Unavailable),
            ] {
                let mut event = event();
                event.data.request_body = Some(body.clone());
                event.data.provider_request_body = Some(body.clone());
                event.data.request_body_state = state;
                event.data.provider_request_body_state = state;
                event.data.response_headers = Some(json!({"large": "x".repeat(16_384)}));
                event.data.request_metadata = Some(json!({
                    "requested_reasoning_effort": "medium",
                    "provider_reasoning_effort": "high",
                    "provider_service_tier": "priority",
                    "provider_cache_ttl_minutes": 60,
                    "unrelated": {"preserved": true}
                }));
                let original = event.to_stream_fields().expect("full v1 fields");
                let baseline = UsageEvent::from_stream_fields_with_capture_budget(
                    &original,
                    Arc::new(EventCaptureMemoryBudget::new(usize::MAX)),
                )
                .expect("full v1 consumer baseline");
                let encoded = event
                    .to_bounded_stream_fields(4_096)
                    .expect("omit diagnostics");
                assert!(encoded.diagnostics_omitted);
                let decoded = UsageEvent::from_stream_fields_with_capture_budget(
                    &encoded.fields,
                    Arc::new(EventCaptureMemoryBudget::new(usize::MAX)),
                )
                .expect("projected consumer event");
                let tier = |data: &UsageEventData| {
                    resolve_provider_service_tier_from_request_capture(
                        data.provider_request_body.as_ref(),
                        data.provider_request_body_state,
                        data.request_metadata.as_ref(),
                    )
                };
                assert_eq!(
                    tier(&decoded.data),
                    tier(&baseline.data),
                    "provider tier for {body:?}, {state:?}"
                );
                let metadata = decoded.data.request_metadata.as_ref().unwrap();
                let baseline_metadata = baseline.data.request_metadata.as_ref().unwrap();
                assert_eq!(
                    extract_provider_cache_ttl_minutes_from_metadata(Some(metadata)),
                    extract_provider_cache_ttl_minutes_from_metadata(Some(baseline_metadata)),
                    "metadata TTL fallback for {body:?}, {state:?}"
                );
                let authoritative = !body.is_null()
                    && matches!(
                        state,
                        None | Some(
                            UsageBodyCaptureState::Inline | UsageBodyCaptureState::Reference
                        )
                    );
                for field in [
                    REQUESTED_REASONING_EFFORT_METADATA_KEY,
                    PROVIDER_REASONING_EFFORT_METADATA_KEY,
                ] {
                    assert_eq!(
                        metadata.get(field),
                        if authoritative {
                            None
                        } else {
                            baseline_metadata.get(field)
                        },
                        "reasoning authority for {body:?}, {state:?}, {field}"
                    );
                }
                assert_eq!(metadata["unrelated"], baseline_metadata["unrelated"]);
                if body.is_null() {
                    assert_eq!(
                        decoded.data.request_body_state,
                        baseline.data.request_body_state
                    );
                    assert_eq!(
                        decoded.data.provider_request_body_state,
                        baseline.data.provider_request_body_state
                    );
                }
                assert_eq!(event.to_stream_fields().unwrap(), original);
            }
        }
    }

    #[test]
    fn event_wire_rejects_cache_ttl_loss_from_explicit_none_capture_state() {
        let mut event = event();
        event.data.provider_request_body_state = Some(UsageBodyCaptureState::None);
        event.data.provider_request_body = Some(json!({
            "prompt_cache_options": {"ttl": "30m"},
            "large": "x".repeat(16_384)
        }));
        let original = event.to_stream_fields().expect("full v1 fields");
        let full = event
            .to_bounded_stream_fields(original["payload"].len())
            .expect("complete diagnostics remain representable");
        assert_eq!(full.fields, original);
        assert!(!full.diagnostics_omitted);
        assert!(matches!(
            event.to_bounded_stream_fields(4_096),
            Err(DataLayerError::InvalidInput(_))
        ));
        assert_eq!(event.to_stream_fields().unwrap(), original);
    }

    #[test]
    fn event_wire_rejects_oversized_core_and_post_projection_metadata() {
        for field in ["request_metadata", "error_message"] {
            let mut event = event();
            if field == "request_metadata" {
                event.data.request_metadata = Some(json!({"large": "x".repeat(32_768)}));
            } else {
                event.data.error_message = Some("x".repeat(32_768));
            }
            let error = event
                .to_bounded_stream_fields(1_024)
                .expect_err("oversized core must fail");
            assert!(matches!(error, DataLayerError::InvalidInput(_)));
            assert!(
                error.to_string().len() < 200,
                "errors must not include payload contents"
            );
        }
        let mut event = event();
        event.data.response_body = Some(json!({"large": "x".repeat(8_192)}));
        let core = ProjectedData {
            data: &event.data,
            overrides: None,
        };
        let core_size = serde_json::to_vec(&envelope(&event, &core)).unwrap().len();
        assert!(
            matches!(
                event.to_bounded_stream_fields(core_size),
                Err(DataLayerError::InvalidInput(_))
            ),
            "added capture metadata must also fit the exact wire limit"
        );
    }
}
