use aether_contracts::StandardizedUsage;
use aether_data_contracts::repository::usage::extract_provider_actual_service_tier_from_response;
use aether_usage_runtime::{map_usage, map_usage_from_response};
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::commit_policy::find_sse_record_boundary;

/// Retain only the current protocol record and the latest usage signal. This
/// preserves the old captured-body billing fallback when audit capture stops.
pub(super) struct StreamUsageFallback {
    record: Vec<u8>,
    limit: usize,
    dropping_record: bool,
    json_body: Option<bool>,
    latest_usage: Option<StandardizedUsage>,
    latest_service_tier: Option<String>,
    claude_usage: Value,
    claude_usage_observed: bool,
    #[cfg(test)]
    copied_record_bytes: usize,
}

impl StreamUsageFallback {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            record: Vec::new(),
            limit,
            dropping_record: false,
            json_body: None,
            latest_usage: None,
            latest_service_tier: None,
            claude_usage: Value::Object(serde_json::Map::new()),
            claude_usage_observed: false,
            #[cfg(test)]
            copied_record_bytes: 0,
        }
    }

    pub(super) fn observe(&mut self, context: &Value, chunk: &[u8]) {
        self.observe_inner::<true>(context, chunk);
    }

    fn observe_inner<const BORROW_COMPLETE_RECORDS: bool>(
        &mut self,
        context: &Value,
        chunk: &[u8],
    ) {
        if self.json_body.is_none() {
            self.json_body = chunk
                .iter()
                .find(|byte| !byte.is_ascii_whitespace())
                .map(|byte| matches!(byte, b'{' | b'['));
        }
        if self.json_body == Some(true) {
            if self.record.len().saturating_add(chunk.len()) > self.limit {
                self.record = Vec::new();
                self.dropping_record = true;
            } else if !self.dropping_record {
                self.append_record(chunk);
            }
            return;
        }
        let mut remaining = chunk;
        while !remaining.is_empty() {
            if BORROW_COMPLETE_RECORDS && self.record.is_empty() && !self.dropping_record {
                if let Some((end, separator)) = find_sse_record_boundary(remaining) {
                    let mut consumed = end + separator;
                    // The buffered path processes CR and LF separately, so a
                    // final CR already ends the record and leaves its LF behind.
                    // Preserve that boundary and its contribution to the next limit.
                    if remaining[..consumed].ends_with(b"\r\n") {
                        consumed -= 1;
                    }
                    if consumed <= self.limit {
                        self.observe_record(context, &remaining[..consumed]);
                        remaining = &remaining[consumed..];
                        continue;
                    }
                }
            }
            // Only incomplete or oversized records need the original carry path.
            let part_len = remaining
                .iter()
                .position(|byte| matches!(byte, b'\r' | b'\n'))
                .map_or(remaining.len(), |index| index + 1);
            let (part, rest) = remaining.split_at(part_len);
            remaining = rest;
            let scan_start = self.record.len().saturating_sub(3);
            if self.record.len().saturating_add(part.len()) > self.limit {
                self.dropping_record = true;
                let suffix = self.record.len().saturating_sub(3);
                self.record = self.record[suffix..].to_vec();
            }
            if self.dropping_record {
                let boundary = find_sse_record_boundary(&self.record).is_some()
                    || self
                        .record
                        .last()
                        .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
                        && matches!(part, b"\n" | b"\r" | b"\r\n")
                        && !(self.record.last() == Some(&b'\r') && part == b"\n");
                self.record = part[part.len().saturating_sub(3)..].to_vec();
                if boundary {
                    self.record = Vec::new();
                    self.dropping_record = false;
                }
                continue;
            }
            self.append_record(part);
            if find_sse_record_boundary(&self.record[scan_start..]).is_some() {
                let record = std::mem::take(&mut self.record);
                self.observe_record(context, &record);
            }
        }
    }

    pub(super) fn finish(&mut self, context: &Value) -> Option<StandardizedUsage> {
        let record = std::mem::take(&mut self.record);
        if !self.dropping_record && !record.is_empty() {
            self.observe_record(context, &record);
        }
        self.latest_usage.take()
    }

    pub(super) fn take_service_tier(&mut self) -> Option<String> {
        self.latest_service_tier.take()
    }

    fn append_record(&mut self, bytes: &[u8]) {
        #[cfg(test)]
        {
            self.copied_record_bytes += bytes.len();
        }
        let required = self.record.len().saturating_add(bytes.len());
        if required > self.record.capacity() {
            let capacity = required
                .max(self.record.capacity().saturating_mul(2))
                .min(self.limit);
            self.record.reserve_exact(capacity - self.record.len());
        }
        self.record.extend_from_slice(bytes);
    }

    fn observe_record(&mut self, context: &Value, record: &[u8]) {
        // Content-only events do not need another JSON parse.
        let image_response = is_openai_image_api(provider_format(context));
        if !record.contains(&b'\\')
            && !record.windows(5).any(|part| {
                matches!(part, b"usage" | b"_tier" | b"speed")
                    || image_response && matches!(part, b"\"data" | b"resul")
            })
        {
            return;
        }
        let Ok(record) = std::str::from_utf8(record) else {
            return;
        };
        let mut data_lines = record
            .split(['\r', '\n'])
            .filter_map(|line| line.trim().strip_prefix("data:").map(str::trim));
        let Some(first) = data_lines.next() else {
            self.observe_json(context, record);
            return;
        };
        let Some(second) = data_lines.next() else {
            self.observe_json(context, first);
            return;
        };
        let mut payload = String::with_capacity(record.len());
        payload.push_str(first);
        for line in std::iter::once(second).chain(data_lines) {
            payload.push('\n');
            payload.push_str(line);
        }
        self.observe_json(context, &payload);
    }

    fn observe_json(&mut self, context: &Value, json: &str) {
        // serde ignores text/image/output fields instead of allocating another
        // copy of a large completed response just to recover its usage object.
        let Ok(envelope) = serde_json::from_str::<UsageEnvelope>(json) else {
            return;
        };
        let provider_format = provider_format(context);
        let image_count = is_openai_image_api(provider_format)
            .then(|| envelope.image_count())
            .flatten();
        let envelope = envelope.into_value();
        if let Some(tier) = extract_provider_actual_service_tier_from_response(Some(&envelope)) {
            self.latest_service_tier = Some(tier);
        }
        // Anthropic alone sends partial usage. Keep only fields the mapper reads;
        // unknown fields must not accumulate across the lifetime of the stream.
        let provider_family = provider_format.split(':').next().unwrap_or_default().trim();
        if provider_family.eq_ignore_ascii_case("claude")
            || provider_family.eq_ignore_ascii_case("anthropic")
        {
            let event_type = envelope.get("type").and_then(Value::as_str);
            let raw_usage = match event_type {
                Some("message_start") => {
                    self.claude_usage = Value::Object(serde_json::Map::new());
                    self.claude_usage_observed = false;
                    envelope.pointer("/message/usage")
                }
                Some("message_delta") => envelope.get("usage"),
                _ => None,
            };
            if let Some(raw_usage) = raw_usage.and_then(Value::as_object) {
                self.claude_usage_observed |= !raw_usage.is_empty();
                merge_claude_usage_projection(&mut self.claude_usage, raw_usage);
                let usage = map_usage(&self.claude_usage, provider_format);
                if usage.has_token_signal() || self.claude_usage_observed {
                    self.latest_usage = Some(usage);
                }
                return;
            }
        }
        let mut usage = map_usage_from_response(&envelope, provider_format);
        if let Some(image_count) = image_count {
            usage.request_count = image_count;
            usage
                .dimensions
                .insert("image_count".to_owned(), Value::from(image_count));
        }
        if usage.has_token_signal() || contains_explicit_usage(&envelope) {
            self.latest_usage = Some(usage);
        }
    }
}

fn provider_format(context: &Value) -> &str {
    [
        "provider_stream_event_api_format",
        "provider_stream_api_format",
        "provider_api_format",
    ]
    .into_iter()
    .filter_map(|field| context.get(field).and_then(Value::as_str))
    .map(str::trim)
    .find(|value| !value.is_empty())
    .unwrap_or_default()
}

fn is_openai_image_api(api_format: &str) -> bool {
    let mut parts = api_format.split(':').map(str::trim);
    parts
        .next()
        .is_some_and(|part| part.eq_ignore_ascii_case("openai"))
        && parts
            .next()
            .is_some_and(|part| part.eq_ignore_ascii_case("image"))
}

fn merge_claude_usage_projection(target: &mut Value, incoming: &serde_json::Map<String, Value>) {
    let target = target.as_object_mut().expect("Claude usage is an object");
    for key in [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "total_tokens",
    ] {
        if let Some(value) = incoming.get(key) {
            target.insert(key.to_owned(), usage_integer_or_null(value));
        }
    }
    if let Some(value) = incoming.get("cache_creation") {
        let projected = value.as_object().map(|object| {
            ["ephemeral_5m_input_tokens", "ephemeral_1h_input_tokens"]
                .into_iter()
                .filter_map(|key| {
                    object
                        .get(key)
                        .map(|value| (key.to_owned(), usage_integer_or_null(value)))
                })
                .collect::<serde_json::Map<_, _>>()
        });
        // The original merge replaces this whole subobject, including an empty
        // or malformed replacement. Do not merge its leaves with older values.
        target.insert(
            "cache_creation".to_owned(),
            projected.map(Value::Object).unwrap_or(Value::Null),
        );
    }
}

fn usage_integer_or_null(value: &Value) -> Value {
    value.as_i64().map(Value::from).unwrap_or(Value::Null)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct UsageEnvelope {
    #[serde(rename = "type")]
    event_type: Option<String>,
    #[serde(deserialize_with = "deserialize_present_usage")]
    usage: Option<Value>,
    #[serde(
        rename = "usageMetadata",
        deserialize_with = "deserialize_present_usage"
    )]
    usage_metadata: Option<Value>,
    service_tier: Option<Value>,
    speed: Option<Value>,
    #[serde(deserialize_with = "deserialize_nested_usage")]
    response: Option<Box<UsageEnvelope>>,
    #[serde(deserialize_with = "deserialize_nested_usage")]
    message: Option<Box<UsageEnvelope>>,
    #[serde(deserialize_with = "deserialize_nested_usage")]
    item: Option<Box<UsageEnvelope>>,
    #[serde(deserialize_with = "deserialize_usage_array")]
    candidates: Option<Vec<UsageEnvelope>>,
    #[serde(deserialize_with = "deserialize_usage_array")]
    chunks: Option<Vec<UsageEnvelope>>,
    data: ImageResponseCount,
    result: ImageResponseCount,
}

fn deserialize_present_usage<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    // A present null stops the mapper's search through nested or older usage.
    // Missing fields still use the envelope's default None.
    Value::deserialize(deserializer).map(Some)
}

#[derive(Default)]
enum ImageResponseCount {
    #[default]
    Empty,
    Array(usize),
    Single,
}

impl<'de> Deserialize<'de> for ImageResponseCount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ImageCountVisitor;

        impl<'de> Visitor<'de> for ImageCountVisitor {
            type Value = ImageResponseCount;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an image response result")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut count = 0;
                while sequence.next_element::<IgnoredAny>()?.is_some() {
                    count += 1;
                }
                Ok(ImageResponseCount::Array(count))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut nonempty = false;
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {
                    nonempty = true;
                }
                Ok(if nonempty {
                    ImageResponseCount::Single
                } else {
                    ImageResponseCount::Empty
                })
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Self::Value, E> {
                Ok(if text.trim().is_empty() {
                    ImageResponseCount::Empty
                } else {
                    ImageResponseCount::Single
                })
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(ImageResponseCount::Empty)
            }

            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(ImageResponseCount::Empty)
            }

            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(ImageResponseCount::Empty)
            }

            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(ImageResponseCount::Empty)
            }

            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(ImageResponseCount::Empty)
            }
        }

        deserializer.deserialize_any(ImageCountVisitor)
    }
}

enum UsageValue {
    Object(UsageEnvelope),
    Array(Vec<UsageEnvelope>),
    Ignored,
}

impl<'de> Deserialize<'de> for UsageValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UsageValueVisitor;

        impl<'de> Visitor<'de> for UsageValueVisitor {
            type Value = UsageValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an optional usage envelope")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                UsageEnvelope::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(UsageValue::Object)
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut first = None;
                let mut envelopes = Vec::new();
                while let Some(value) = sequence.next_element::<UsageValue>()? {
                    let envelope = match value {
                        UsageValue::Object(envelope) => envelope,
                        UsageValue::Array(_) | UsageValue::Ignored => UsageEnvelope::default(),
                    };
                    if first.is_none() && envelopes.is_empty() {
                        first = Some(envelope);
                    } else if !envelope.is_empty() {
                        // Only candidates[0] is positional in the usage mapper.
                        // Keep that slot even if empty; subsequent empty slots
                        // cannot contribute usage, explicit-zero signals or tier.
                        if let Some(first) = first.take() {
                            envelopes.push(first);
                        }
                        envelopes.push(envelope);
                    }
                }
                if let Some(first) = first.filter(|first| !first.is_empty()) {
                    envelopes.push(first);
                }
                Ok(UsageValue::Array(envelopes))
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UsageValue::Ignored)
            }

            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(UsageValue::Ignored)
            }

            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(UsageValue::Ignored)
            }

            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(UsageValue::Ignored)
            }

            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(UsageValue::Ignored)
            }

            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(UsageValue::Ignored)
            }
        }

        deserializer.deserialize_any(UsageValueVisitor)
    }
}

fn deserialize_nested_usage<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Box<UsageEnvelope>>, D::Error> {
    match UsageValue::deserialize(deserializer)? {
        UsageValue::Object(envelope) => Ok(Some(Box::new(envelope))),
        UsageValue::Array(_) | UsageValue::Ignored => Ok(None),
    }
}

fn deserialize_usage_array<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<UsageEnvelope>>, D::Error> {
    match UsageValue::deserialize(deserializer)? {
        UsageValue::Array(envelopes) => Ok(Some(envelopes)),
        UsageValue::Object(_) | UsageValue::Ignored => Ok(None),
    }
}

impl UsageEnvelope {
    fn image_count(&self) -> Option<i64> {
        match self.data {
            ImageResponseCount::Array(count) if count > 0 => Some(count as i64),
            _ => match self.result {
                ImageResponseCount::Array(count) if count > 0 => Some(count as i64),
                ImageResponseCount::Single => Some(1),
                _ => None,
            },
        }
    }

    fn is_empty(&self) -> bool {
        self.event_type.is_none()
            && self.usage.is_none()
            && self.usage_metadata.is_none()
            && self.service_tier.is_none()
            && self.speed.is_none()
            && [&self.response, &self.message, &self.item]
                .into_iter()
                .all(|value| value.as_ref().is_none_or(|value| value.is_empty()))
            && [&self.candidates, &self.chunks].into_iter().all(|values| {
                values
                    .as_ref()
                    .is_none_or(|values| values.iter().all(Self::is_empty))
            })
    }

    fn into_value(self) -> Value {
        let mut object = serde_json::Map::new();
        if let Some(event_type) = self.event_type {
            object.insert("type".to_string(), Value::String(event_type));
        }
        for (key, value) in [
            ("usage", self.usage),
            ("usageMetadata", self.usage_metadata),
            ("service_tier", self.service_tier),
            ("speed", self.speed),
        ] {
            if let Some(value) = value {
                object.insert(key.to_string(), value);
            }
        }
        for (key, value) in [
            ("response", self.response),
            ("message", self.message),
            ("item", self.item),
        ] {
            if let Some(value) = value {
                object.insert(key.to_string(), value.into_value());
            }
        }
        for (key, values) in [("candidates", self.candidates), ("chunks", self.chunks)] {
            if let Some(values) = values.filter(|values| !values.is_empty()) {
                object.insert(
                    key.to_string(),
                    Value::Array(values.into_iter().map(Self::into_value).collect()),
                );
            }
        }
        Value::Object(object)
    }
}

fn contains_explicit_usage(value: &Value) -> bool {
    ["usage", "usageMetadata"].into_iter().any(|key| {
        value
            .get(key)
            .and_then(Value::as_object)
            .is_some_and(|value| !value.is_empty())
    }) || ["response", "message", "item"]
        .into_iter()
        .any(|key| value.get(key).is_some_and(contains_explicit_usage))
        || ["candidates", "chunks"].into_iter().any(|key| {
            value
                .get(key)
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().any(contains_explicit_usage))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stream_capture_fallback_preserves_present_null_usage_lookup_barriers() {
        let previous = json!({
            "usage": {"prompt_tokens": 10, "completion_tokens": 20},
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 20}
        });
        let cases = [
            (
                "openai:chat",
                json!({"chunks": [
                    {"usage": {"prompt_tokens": 10, "completion_tokens": 20}},
                    {"usage": null}
                ]}),
            ),
            (
                "openai:responses",
                json!({"usage": null, "response": {
                    "usage": {"input_tokens": 10, "output_tokens": 20}
                }}),
            ),
            (
                "gemini:generate_content",
                json!({"chunks": [
                    {"usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 20}},
                    {"usageMetadata": null}
                ]}),
            ),
            (
                "gemini:generate_content",
                json!({"usageMetadata": null, "response": {
                    "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 20}
                }}),
            ),
            (
                "gemini:generate_content",
                json!({"candidates": [
                    {"usageMetadata": null},
                    {"usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 20}}
                ]}),
            ),
        ];
        for (format, original) in cases {
            let context = json!({"provider_api_format": format});
            let expected = map_usage_from_response(&original, format);
            assert_eq!(expected.input_tokens, 0);
            assert_eq!(expected.output_tokens, 0);
            assert!(contains_explicit_usage(&original));
            let projected = serde_json::from_value::<UsageEnvelope>(original.clone())
                .unwrap()
                .into_value();
            assert_eq!(map_usage_from_response(&projected, format), expected);
            assert!(contains_explicit_usage(&projected));

            let json = serde_json::to_vec(&original).unwrap();
            let sse = format!("data: {original}\r\n\r\n");
            for chunk_size in [1, 3, 17, sse.len()] {
                let mut fallback = StreamUsageFallback::new(1024);
                for chunk in json.chunks(chunk_size) {
                    fallback.observe(&context, chunk);
                }
                assert_eq!(fallback.finish(&context), Some(expected.clone()));

                let mut fallback = StreamUsageFallback::new(1024);
                fallback.observe(&context, format!("data: {previous}\n\n").as_bytes());
                assert_eq!(fallback.latest_usage.as_ref().unwrap().output_tokens, 20);
                for chunk in sse.as_bytes().chunks(chunk_size) {
                    fallback.observe(&context, chunk);
                }
                assert_eq!(fallback.finish(&context), Some(expected.clone()));
            }
        }
    }

    #[test]
    fn stream_capture_fallback_distinguishes_missing_and_null_usage_without_new_signal() {
        let missing = serde_json::from_value::<UsageEnvelope>(json!({})).unwrap();
        assert!(missing.usage.is_none());
        assert!(missing.usage_metadata.is_none());
        assert!(missing.is_empty());
        for (format, key, tokens) in [
            (
                "openai:chat",
                "usage",
                json!({"prompt_tokens": 10, "completion_tokens": 20}),
            ),
            (
                "gemini:generate_content",
                "usageMetadata",
                json!({"promptTokenCount": 10, "candidatesTokenCount": 20}),
            ),
        ] {
            let context = json!({"provider_api_format": format});
            let null = json!({key: null});
            let projected = serde_json::from_value::<UsageEnvelope>(null.clone()).unwrap();
            assert!(!projected.is_empty());
            assert_eq!(projected.into_value(), null);
            assert!(!contains_explicit_usage(&null));
            let valid = json!({"response": {key: tokens}});
            let expected = map_usage_from_response(&valid, format);
            assert_eq!(expected.output_tokens, 20);
            let mut fallback = StreamUsageFallback::new(1024);
            let sse = format!("data: {valid}\n\ndata: {null}\n\n");
            for chunk in sse.as_bytes().chunks(3) {
                fallback.observe(&context, chunk);
            }
            assert_eq!(fallback.finish(&context), Some(expected));
        }
    }

    #[test]
    fn stream_capture_fallback_image_counts_match_response_mapper_without_retaining_images() {
        let cases = [
            json!({}),
            json!({"unknown": [1, 2, 3]}),
            json!({"data": null, "result": null}),
            json!({"data": [], "result": []}),
            json!({"data": {}, "result": {}}),
            json!({"data": "ignored", "result": " \n\t "}),
            json!({"data": true, "result": 3.5}),
            json!({"data": false, "result": true}),
            json!({"data": [null, false, 1], "result": [1, 2]}),
            json!({"data": [], "result": [null, false]}),
            json!({"data": [null], "result": [1, 2, 3]}),
            json!({"data": {"ignored": 1}, "result": {"url": "image"}}),
            json!({"data": 1, "result": " image "}),
            json!({"result": "escaped\nimage"}),
            json!({"response": {"data": [1, 2]}, "chunks": [{"result": [1, 2]}]}),
            json!({"data": [{"b64_json": "x".repeat(64 * 1024)}, null]}),
        ];
        for mut original in cases {
            original.as_object_mut().unwrap().insert(
                "usage".to_owned(),
                json!({"prompt_tokens": 10, "completion_tokens": 20}),
            );
            let bytes = serde_json::to_vec(&original).unwrap();
            let projected = serde_json::from_slice::<UsageEnvelope>(&bytes).unwrap();
            let image_count = projected.image_count();
            let compact = projected.into_value();
            assert!(compact.get("data").is_none());
            assert!(compact.get("result").is_none());
            assert!(serde_json::to_vec(&compact).unwrap().len() < 256);
            for format in [
                " OpenAI : Image ",
                "openai:chat",
                "gemini:generate_content",
                "unknown:api",
            ] {
                let expected = map_usage_from_response(&original, format);
                if is_openai_image_api(format) {
                    assert_eq!(
                        image_count.map(Value::from).as_ref(),
                        expected.dimensions.get("image_count")
                    );
                }
                let context = json!({"provider_api_format": format});
                let sse = format!("data: {original}\r\n\r\n");
                for chunk_size in [3, 17, sse.len()] {
                    for input in [bytes.as_slice(), sse.as_bytes()] {
                        let mut fallback = StreamUsageFallback::new(128 * 1024);
                        for chunk in input.chunks(chunk_size) {
                            fallback.observe(&context, chunk);
                        }
                        assert_eq!(fallback.finish(&context), Some(expected.clone()));
                    }
                }
            }
        }
    }

    #[test]
    fn stream_capture_fallback_image_only_records_keep_count_signal() {
        for original in [
            json!({"data": [null, null]}),
            json!({"result": [null, null, null]}),
            json!({"result": {"url": "image"}}),
            json!({"result": "image"}),
        ] {
            let context = json!({"provider_api_format": "openai:image"});
            let expected = map_usage_from_response(&original, "openai:image");
            let json = serde_json::to_vec(&original).unwrap();
            let sse = format!("data: {original}\n\n");
            for input in [json.as_slice(), sse.as_bytes()] {
                for chunk_size in [1, 3, input.len()] {
                    let mut fallback = StreamUsageFallback::new(1024);
                    for chunk in input.chunks(chunk_size) {
                        fallback.observe(&context, chunk);
                    }
                    assert_eq!(fallback.finish(&context), Some(expected.clone()));
                }
            }
            let context = json!({"provider_api_format": "openai:chat"});
            let mut fallback = StreamUsageFallback::new(1024);
            fallback.observe(&context, sse.as_bytes());
            assert_eq!(fallback.finish(&context), None);
        }
    }

    #[test]
    fn stream_capture_fallback_claude_projection_matches_original_raw_merge() {
        for provider_format in ["claude:messages", " Anthropic:messages "] {
            let context = json!({"provider_api_format": provider_format});
            let mut fallback = StreamUsageFallback::new(64 * 1024);
            let mut raw_merged = serde_json::Map::new();
            let mut expected_usage = None;
            let mut expected_tier = None;
            let events = [
                json!({"type": "message_start", "message": {"usage": {"unknown": [1, 2]}}}),
                json!({"type": "message_delta", "usage": {}}),
                json!({"type": "message_delta", "usage": {
                    "input_tokens": 100, "output_tokens": 10, "total_tokens": 110,
                    "cache_creation_input_tokens": 7, "cache_read_input_tokens": 30,
                    "cache_creation": {"ephemeral_5m_input_tokens": 5, "ephemeral_1h_input_tokens": 2},
                    "speed": " FAST "
                }}),
                json!({"type": "message_delta", "usage": {
                    "input_tokens": 0, "output_tokens": 500, "total_tokens": 500,
                    "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0,
                    "cache_creation": {"ephemeral_1h_input_tokens": 3},
                    "service_tier": "priority"
                }}),
                json!({"type": "message_delta", "usage": {
                    "input_tokens": "12", "output_tokens": null, "total_tokens": "500",
                    "cache_read_input_tokens": [10], "cache_creation_input_tokens": {"nested": 20},
                    "cache_creation": {}, "speed": "standard"
                }}),
                json!({"type": "message_delta", "usage": {
                    "input_tokens": -1, "output_tokens": u64::MAX, "total_tokens": u64::MAX,
                    "cache_creation": {"ephemeral_5m_input_tokens": 1.5, "ephemeral_1h_input_tokens": false}
                }}),
                json!({"type": "message_delta", "usage": {"cache_creation": [7]}}),
                json!({"type": "message_start", "message": {"usage": {}}}),
                json!({"type": "message_delta", "usage": {"unknown_only": false}}),
                json!({"type": "message_delta", "usage": null}),
            ];
            for event in events {
                if let Some(tier) = extract_provider_actual_service_tier_from_response(Some(&event))
                {
                    expected_tier = Some(tier);
                }
                let raw_usage = match event.get("type").and_then(Value::as_str) {
                    Some("message_start") => {
                        raw_merged.clear();
                        event.pointer("/message/usage")
                    }
                    Some("message_delta") => event.get("usage"),
                    _ => None,
                };
                let original = if let Some(raw_usage) = raw_usage.and_then(Value::as_object) {
                    raw_merged.extend(raw_usage.clone());
                    json!({"usage": raw_merged})
                } else {
                    event.clone()
                };
                let mapped = map_usage_from_response(&original, provider_format);
                if mapped.has_token_signal() || contains_explicit_usage(&original) {
                    expected_usage = Some(mapped);
                }
                fallback.observe(&context, format!("data: {event}\n\n").as_bytes());
                assert_eq!(fallback.latest_usage, expected_usage, "event={event}");
                assert_eq!(fallback.latest_service_tier, expected_tier, "event={event}");
            }
        }
    }

    #[test]
    fn stream_capture_fallback_claude_unknown_fields_do_not_accumulate() {
        let context = json!({"provider_api_format": "claude:messages"});
        let mut fallback = StreamUsageFallback::new(16 * 1024);
        for index in 0..256 {
            let event = json!({"type": "message_delta", "usage": {
                format!("unknown_{index}"): "x".repeat(4096),
                "output_tokens": index,
                "cache_read_input_tokens": 0
            }});
            fallback.observe(&context, format!("data: {event}\n\n").as_bytes());
            let retained = fallback.claude_usage.as_object().unwrap();
            assert_eq!(retained.len(), 2);
            assert_eq!(retained.get("output_tokens"), Some(&json!(index)));
            assert_eq!(retained.get("cache_read_input_tokens"), Some(&json!(0)));
            assert_eq!(fallback.record.capacity(), 0);
        }
        let usage = fallback.finish(&context).unwrap();
        assert_eq!(usage.output_tokens, 255);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn stream_capture_fallback_array_projection_skips_empty_envelopes_without_shifting_first() {
        let mut empty = vec![
            Value::Null,
            json!(false),
            json!(3),
            json!({"content": "unused"}),
        ];
        empty.extend(std::iter::repeat_n(Value::Null, 10_000));
        let empty_response = json!({"candidates": empty, "chunks": empty});
        let projected = serde_json::from_value::<UsageEnvelope>(empty_response).unwrap();
        assert!(projected.candidates.as_ref().unwrap().is_empty());
        assert!(projected.chunks.as_ref().unwrap().is_empty());

        let original = json!({
            "candidates": [null, {"usageMetadata": {"promptTokenCount": 99}}],
            "chunks": [null, {"content": "unused"}, {"usage": {
                "input_tokens": 10, "output_tokens": 7, "cache_read_input_tokens": 0
            }, "service_tier": "priority"}, null, {}, {"usageMetadata": {
                "promptTokenCount": 12, "candidatesTokenCount": 0
            }, "speed": "fast"}, null]
        });
        let projected = serde_json::from_value::<UsageEnvelope>(original.clone()).unwrap();
        assert_eq!(projected.candidates.as_ref().unwrap().len(), 2);
        assert_eq!(projected.chunks.as_ref().unwrap().len(), 3);
        let compact = projected.into_value();
        for format in [
            "openai:chat",
            "openai:responses",
            "openai:image",
            "gemini:generate_content",
            "claude:messages",
            "unknown:format",
        ] {
            assert_eq!(
                map_usage_from_response(&compact, format),
                map_usage_from_response(&original, format)
            );
        }
        assert_eq!(
            contains_explicit_usage(&compact),
            contains_explicit_usage(&original)
        );
        assert_eq!(
            extract_provider_actual_service_tier_from_response(Some(&compact)),
            extract_provider_actual_service_tier_from_response(Some(&original))
        );
    }

    #[test]
    fn stream_capture_fallback_complete_records_borrow_transport_bytes() {
        let context = json!({"provider_api_format": "openai:chat"});
        let chunk = format!(
            "data: {{\"content\":\"{}\"}}\n\ndata: {{\"usage\":{{\"prompt_tokens\":10,\"completion_tokens\":20}}}}\n\n",
            "x".repeat(32 * 1024)
        );
        let mut borrowed = StreamUsageFallback::new(64 * 1024);
        let mut buffered = StreamUsageFallback::new(64 * 1024);
        borrowed.observe(&context, chunk.as_bytes());
        buffered.observe_inner::<false>(&context, chunk.as_bytes());
        assert_eq!(borrowed.copied_record_bytes, 0);
        assert_eq!(buffered.copied_record_bytes, chunk.len());
        assert_eq!(borrowed.latest_usage, buffered.latest_usage);
        assert_eq!(borrowed.finish(&context).unwrap().output_tokens, 20);
    }

    #[test]
    fn stream_capture_fallback_borrowed_records_match_buffered_limits_and_split_endings() {
        let context = json!({"provider_api_format": "openai:chat"});
        for ending in ["\n", "\r\n", "\r"] {
            let first = format!(
                "data: {{\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":10}},\"content\":\"{}\"}}{ending}{ending}",
                "x".repeat(80)
            );
            let input = format!(
                ": comment{ending}{ending}{first}data: {{\"\\u0075sage\":{ending}data: {{\"completion_tokens\":20}},\"service_tier\":\"priority\"}}{ending}{ending}data: [DONE]{ending}{ending}data: {{\"usage\":{{\"completion_tokens\":30}}}}"
            );
            for limit in [8, 64, first.len() - 1, first.len(), first.len() + 1, 512] {
                for chunk_size in [1, 2, 3, 7, 16, 64, input.len()] {
                    let mut borrowed = StreamUsageFallback::new(limit);
                    let mut buffered = StreamUsageFallback::new(limit);
                    for chunk in input.as_bytes().chunks(chunk_size) {
                        borrowed.observe(&context, chunk);
                        buffered.observe_inner::<false>(&context, chunk);
                        assert_eq!(
                            borrowed.record, buffered.record,
                            "ending={ending:?} limit={limit} chunk={chunk_size}"
                        );
                        assert_eq!(
                            borrowed.dropping_record, buffered.dropping_record,
                            "ending={ending:?} limit={limit} chunk={chunk_size}"
                        );
                        assert_eq!(
                            borrowed.latest_usage, buffered.latest_usage,
                            "ending={ending:?} limit={limit} chunk={chunk_size}"
                        );
                        assert_eq!(
                            borrowed.latest_service_tier, buffered.latest_service_tier,
                            "ending={ending:?} limit={limit} chunk={chunk_size}"
                        );
                    }
                    assert_eq!(borrowed.finish(&context), buffered.finish(&context));
                    assert_eq!(borrowed.take_service_tier(), buffered.take_service_tier());
                }
            }
        }
    }

    #[test]
    fn stream_capture_fallback_keeps_latest_split_record_usage_and_releases_capacity() {
        let context = json!({"provider_api_format": "openai:chat"});
        let mut fallback = StreamUsageFallback::new(1024);
        let events = b"data: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":10}}\r\n\r\ndata: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":30}}}\r\n\r\n";
        for part in events.chunks(7) {
            fallback.observe(&context, part);
        }
        let usage = fallback.finish(&context).expect("last usage signal");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_tokens, 30);
        assert_eq!(fallback.record.capacity(), 0);
    }

    #[test]
    fn stream_capture_fallback_recovers_after_an_oversized_record() {
        let context = json!({"provider_api_format": "openai:chat"});
        for ending in ["\n", "\r\n", "\r"] {
            for chunk_size in 1..=16 {
                let mut fallback = StreamUsageFallback::new(96);
                let events = format!(
                    "data: {}{ending}{ending}data: {{\"usage\":{{\"completion_tokens\":12}}}}{ending}{ending}",
                    "x".repeat(128),
                );
                for chunk in events.as_bytes().chunks(chunk_size) {
                    fallback.observe(&context, chunk);
                }
                assert_eq!(
                    fallback.finish(&context).unwrap().output_tokens,
                    12,
                    "ending={ending:?}, chunk_size={chunk_size}"
                );
                assert_eq!(fallback.record.capacity(), 0);
            }
        }
    }

    #[test]
    fn stream_capture_fallback_handles_nested_gemini_usage() {
        let context = json!({"provider_api_format": "gemini:generate_content"});
        let mut fallback = StreamUsageFallback::new(1024);
        fallback.observe(&context, b"data: {\"response\":{\"usageMetadata\":{\"promptTokenCount\":11,\"candidatesTokenCount\":7,\"cachedContentTokenCount\":3}}}\n\n");
        let usage = fallback.finish(&context).unwrap();
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cache_read_tokens, 3);
    }

    #[test]
    fn stream_capture_fallback_merges_anthropic_partial_fields_and_preserves_explicit_zero() {
        for format in ["claude:messages", " Anthropic:messages "] {
            let context = json!({"provider_api_format": format});
            let mut fallback = StreamUsageFallback::new(1024);
            fallback.observe(&context, b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":100,\"cache_read_input_tokens\":30,\"cache_creation_input_tokens\":7}}}\n\n");
            fallback.observe(&context, b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":20,\"cache_read_input_tokens\":0}}\n\n");
            let usage = fallback.finish(&context).unwrap();
            assert_eq!(usage.input_tokens, 100);
            assert_eq!(usage.output_tokens, 20);
            assert_eq!(usage.cache_read_tokens, 0);
            assert_eq!(usage.cache_creation_tokens, 7);
        }
    }

    #[test]
    fn stream_capture_fallback_latest_complete_snapshot_can_reset_cache_to_zero() {
        let context = json!({"provider_api_format": "openai:chat"});
        let mut fallback = StreamUsageFallback::new(1024);
        fallback.observe(&context, b"data: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":10,\"prompt_tokens_details\":{\"cached_tokens\":30}}}\n\n");
        fallback.observe(&context, b"data: {\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":0}}}\n\n");
        let usage = fallback.finish(&context).unwrap();
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn stream_capture_fallback_accepts_escaped_keys_multiline_and_eof_records() {
        let context = json!({
            "provider_stream_event_api_format": null,
            "provider_stream_api_format": " openai:chat ",
            "provider_api_format": "gemini:generate_content"
        });
        let mut fallback = StreamUsageFallback::new(1024);
        let event = b"data: {\"\\u0075sage\":\r\ndata: {\"prompt_tokens\":11,\"completion_tokens\":7},\"service_tier\":\" PRIORITY \"}";
        for chunk in event.chunks(3) {
            fallback.observe(&context, chunk);
        }
        let usage = fallback.finish(&context).unwrap();
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(fallback.take_service_tier().as_deref(), Some("priority"));
        assert_eq!(fallback.record.capacity(), 0);
    }

    #[test]
    fn stream_capture_fallback_json_whitespace_does_not_flush_an_incomplete_response() {
        let context = json!({"provider_api_format": "openai:chat"});
        let mut fallback = StreamUsageFallback::new(1024);
        let response = b"{\n\n\"choices\": [],\n\n\"usage\": {\"prompt_tokens\":11,\"completion_tokens\":7}\n}";
        for chunk in response.chunks(5) {
            fallback.observe(&context, chunk);
        }
        let usage = fallback.finish(&context).unwrap();
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(fallback.record.capacity(), 0);
    }

    #[test]
    fn stream_capture_fallback_record_boundaries_accept_all_sse_endings_and_fragment_sizes() {
        let context = json!({"provider_api_format": "openai:chat"});
        for ending in ["\n", "\r\n", "\r"] {
            for chunk_size in 1..=16 {
                let mut fallback = StreamUsageFallback::new(1024);
                let events = format!(
                    "data: {{\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":10}}}}{ending}{ending}data: {{\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20}}}}{ending}{ending}data: [DONE]{ending}{ending}",
                );
                for chunk in events.as_bytes().chunks(chunk_size) {
                    fallback.observe(&context, chunk);
                }
                let usage = fallback.finish(&context).unwrap();
                assert_eq!(
                    usage.input_tokens, 100,
                    "ending={ending:?}, chunk_size={chunk_size}"
                );
                assert_eq!(
                    usage.output_tokens, 20,
                    "ending={ending:?}, chunk_size={chunk_size}"
                );
                assert_eq!(fallback.record.capacity(), 0);
            }
        }
    }

    #[test]
    fn stream_capture_fallback_ignores_non_object_envelopes_and_large_unknown_content() {
        let context = json!({"provider_api_format": "openai:chat"});
        let mut fallback = StreamUsageFallback::new(128 * 1024);
        let body = json!({
            "usage": {"prompt_tokens": 11, "completion_tokens": 7},
            "message": "provider diagnostic text",
            "response": false,
            "item": 42,
            "content": "x".repeat(64 * 1024),
            "chunks": [null, false, 7, "diagnostic", {"content": "unused"}],
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let compact = serde_json::from_slice::<UsageEnvelope>(&bytes)
            .unwrap()
            .into_value();
        assert!(compact.get("content").is_none());
        assert!(compact.get("message").is_none());
        assert!(compact.get("response").is_none());
        assert!(serde_json::to_vec(&compact).unwrap().len() < 256);
        fallback.observe(&context, &bytes);
        let usage = fallback.finish(&context).unwrap();
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 7);

        let mut fallback = StreamUsageFallback::new(1024);
        fallback.observe(&context, b"data: {\"chunks\":[null,\"ignored\",{\"usage\":{\"prompt_tokens\":21,\"completion_tokens\":14}},false]}\n\n");
        let usage = fallback.finish(&context).unwrap();
        assert_eq!(usage.input_tokens, 21);
        assert_eq!(usage.output_tokens, 14);
    }

    #[test]
    fn stream_capture_fallback_preserves_candidate_indices_for_gemini_mapping() {
        let context = json!({"provider_api_format": "gemini:generate_content"});
        let response = json!({
            "candidates": [null, {"usageMetadata": {
                "promptTokenCount": 11, "candidatesTokenCount": 7
            }}]
        });
        let expected = map_usage_from_response(&response, "gemini:generate_content");
        let mut fallback = StreamUsageFallback::new(1024);
        fallback.observe(&context, &serde_json::to_vec(&response).unwrap());
        assert_eq!(fallback.finish(&context).unwrap(), expected);
        assert_eq!(expected.input_tokens, 0);
        assert_eq!(expected.output_tokens, 0);
    }
}
