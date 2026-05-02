use std::collections::BTreeMap;

use serde_json::Value;

use crate::provider_compat::kiro_stream::KiroToClaudeCliStreamState;
use crate::response::AiSurfaceFinalizeError;

use super::surfaces::{
    provider_adaptation_allows_sync_finalize_envelope, provider_adaptation_descriptor_for_envelope,
    provider_adaptation_should_unwrap_stream_envelope, ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME,
    GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME, KIRO_ENVELOPE_NAME,
};

pub fn provider_private_response_allows_sync_finalize(report_context: &Value) -> bool {
    let has_envelope = report_context
        .get("has_envelope")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !has_envelope {
        return true;
    }
    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    provider_adaptation_allows_sync_finalize_envelope(envelope_name, provider_api_format)
}

pub fn normalize_provider_private_report_context(report_context: Option<&Value>) -> Option<Value> {
    let report_context = report_context?;
    if !report_context
        .get("has_envelope")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some(report_context.clone());
    }
    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if provider_adaptation_descriptor_for_envelope(envelope_name, provider_api_format).is_none() {
        return Some(report_context.clone());
    }
    Some(clear_private_envelope_context(report_context))
}

pub fn normalize_provider_private_response_value(
    data: Value,
    report_context: &Value,
) -> Option<Value> {
    if !report_context
        .get("has_envelope")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some(data);
    }
    let mut unwrapped = match report_context.get("envelope_name").and_then(Value::as_str) {
        Some(KIRO_ENVELOPE_NAME) => data,
        Some(GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME) => {
            if let Some(response) = data
                .get("response")
                .and_then(Value::as_object)
                .filter(|response| !response.contains_key("response"))
            {
                Value::Object(response.clone())
            } else {
                data
            }
        }
        Some(ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME) => {
            if let Some(response) = data
                .get("response")
                .and_then(Value::as_object)
                .filter(|response| !response.contains_key("response"))
            {
                let mut unwrapped = response.clone();
                if let Some(response_id) = data.get("responseId").cloned() {
                    unwrapped.insert("_v1internal_response_id".to_string(), response_id);
                }
                Value::Object(unwrapped)
            } else {
                data
            }
        }
        _ => return None,
    };
    postprocess_private_response_value(&mut unwrapped, report_context);
    Some(unwrapped)
}

pub fn transform_provider_private_stream_line(
    report_context: &Value,
    line: Vec<u8>,
) -> Result<Vec<u8>, serde_json::Error> {
    let Ok(text) = std::str::from_utf8(&line) else {
        return Ok(line);
    };
    let trimmed = text.trim_matches('\r').trim();
    if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with("event:") {
        return Ok(Vec::new());
    }
    let Some(data_line) = trimmed.strip_prefix("data:") else {
        return Ok(line);
    };
    let data_line = data_line.trim();
    if data_line.is_empty() || data_line == "[DONE]" {
        return Ok(line);
    }

    let body: Value = match serde_json::from_str(data_line) {
        Ok(value) => value,
        Err(_) => return Ok(line),
    };

    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !provider_adaptation_should_unwrap_stream_envelope(envelope_name, provider_api_format) {
        return Ok(line);
    }
    let unwrapped = match envelope_name {
        GEMINI_CLI_V1INTERNAL_ENVELOPE_NAME => body.get("response").cloned().unwrap_or(body),
        ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME => {
            let mut response = body.get("response").cloned().unwrap_or(body.clone());
            if let Some(response_id) = body.get("responseId").cloned() {
                if let Some(object) = response.as_object_mut() {
                    object
                        .entry("_v1internal_response_id".to_string())
                        .or_insert(response_id);
                }
            }
            inject_antigravity_stream_tool_ids(&mut response);
            response
        }
        _ => body,
    };

    let mut out = b"data: ".to_vec();
    out.extend(serde_json::to_vec(&unwrapped)?);
    out.extend_from_slice(b"\n\n");
    Ok(out)
}

enum ProviderPrivateStreamNormalizeMode {
    EnvelopeUnwrap,
    KiroToClaudeCli(Box<KiroToClaudeCliStreamState>),
}

pub struct ProviderPrivateStreamNormalizer<'a> {
    report_context: &'a Value,
    buffered: Vec<u8>,
    mode: ProviderPrivateStreamNormalizeMode,
}

pub fn maybe_build_provider_private_stream_normalizer<'a>(
    report_context: Option<&'a Value>,
) -> Option<ProviderPrivateStreamNormalizer<'a>> {
    let report_context = report_context?;
    if !report_context
        .get("has_envelope")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }
    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let descriptor =
        provider_adaptation_descriptor_for_envelope(envelope_name, provider_api_format)?;
    let mode = if descriptor
        .envelope_name
        .eq_ignore_ascii_case(KIRO_ENVELOPE_NAME)
    {
        ProviderPrivateStreamNormalizeMode::KiroToClaudeCli(Box::new(
            KiroToClaudeCliStreamState::new(report_context),
        ))
    } else if descriptor.unwraps_response_envelope {
        ProviderPrivateStreamNormalizeMode::EnvelopeUnwrap
    } else {
        return None;
    };
    Some(ProviderPrivateStreamNormalizer {
        report_context,
        buffered: Vec::new(),
        mode,
    })
}

impl ProviderPrivateStreamNormalizer<'_> {
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        match &mut self.mode {
            ProviderPrivateStreamNormalizeMode::KiroToClaudeCli(state) => {
                state.push_chunk(self.report_context, chunk)
            }
            ProviderPrivateStreamNormalizeMode::EnvelopeUnwrap => {
                self.buffered.extend_from_slice(chunk);
                let mut output = Vec::new();
                while let Some(line_end) = self.buffered.iter().position(|byte| *byte == b'\n') {
                    let line = self.buffered.drain(..=line_end).collect::<Vec<_>>();
                    output.extend(
                        transform_provider_private_stream_line(self.report_context, line)
                            .map_err(AiSurfaceFinalizeError::from)?,
                    );
                }
                Ok(output)
            }
        }
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        match &mut self.mode {
            ProviderPrivateStreamNormalizeMode::KiroToClaudeCli(state) => {
                state.finish(self.report_context)
            }
            ProviderPrivateStreamNormalizeMode::EnvelopeUnwrap => {
                if self.buffered.is_empty() {
                    return Ok(Vec::new());
                }
                let line = std::mem::take(&mut self.buffered);
                transform_provider_private_stream_line(self.report_context, line)
                    .map_err(AiSurfaceFinalizeError::from)
            }
        }
    }
}

pub fn stream_body_contains_error_event(body: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };
    let mut current_event_type: Option<String> = None;
    for raw_line in text.lines() {
        let line = raw_line.trim_matches('\r').trim();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(event_name) = line.strip_prefix("event:") {
            current_event_type = Some(event_name.trim().to_string());
            continue;
        }
        let data_line = if let Some(rest) = line.strip_prefix("data:") {
            rest.trim()
        } else {
            line
        };
        if data_line.is_empty() || data_line == "[DONE]" {
            continue;
        }
        let Ok(mut event) = serde_json::from_str::<Value>(data_line) else {
            continue;
        };
        if let Some(event_object) = event.as_object_mut() {
            if !event_object.contains_key("type") {
                if let Some(event_name) = current_event_type.take() {
                    event_object.insert("type".to_string(), Value::String(event_name));
                }
            }
        }
        if event
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("error"))
        {
            return true;
        }
        current_event_type = None;
    }
    false
}

fn clear_private_envelope_context(report_context: &Value) -> Value {
    let mut normalized = report_context.clone();
    if let Some(object) = normalized.as_object_mut() {
        object.insert("has_envelope".to_string(), Value::Bool(false));
        object.remove("envelope_name");
    }
    normalized
}

fn local_finalize_response_model(report_context: &Value) -> &str {
    report_context
        .get("mapped_model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or_default()
}

fn inject_antigravity_stream_tool_ids(value: &mut Value) {
    let Some(candidates) = value.get_mut("candidates").and_then(Value::as_array_mut) else {
        return;
    };

    for candidate in candidates {
        let Some(parts) = candidate
            .get_mut("content")
            .and_then(Value::as_object_mut)
            .and_then(|content| content.get_mut("parts"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };

        let mut counters: BTreeMap<String, usize> = BTreeMap::new();
        for part in parts {
            let Some(function_call) = part.get_mut("functionCall").and_then(Value::as_object_mut)
            else {
                continue;
            };
            let has_id = function_call
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            if has_id {
                continue;
            }
            let name = function_call
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown")
                .to_string();
            let index = counters.entry(name.clone()).or_insert(0);
            function_call.insert(
                "id".to_string(),
                Value::String(format!("call_{name}_{index}")),
            );
            *index += 1;
        }
    }
}

fn inject_antigravity_sync_tool_ids(response: &mut Value, model: &str) {
    if !model.to_ascii_lowercase().contains("claude") {
        return;
    }

    let Some(candidates) = response.get_mut("candidates").and_then(Value::as_array_mut) else {
        return;
    };

    for candidate in candidates {
        let Some(parts) = candidate
            .get_mut("content")
            .and_then(Value::as_object_mut)
            .and_then(|content| content.get_mut("parts"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };

        let mut name_counters: BTreeMap<String, usize> = BTreeMap::new();
        for part in parts {
            let function_call = if let Some(function_call) =
                part.get_mut("functionCall").and_then(Value::as_object_mut)
            {
                function_call
            } else if let Some(function_call) =
                part.get_mut("function_call").and_then(Value::as_object_mut)
            {
                function_call
            } else {
                continue;
            };
            let has_id = function_call
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty());
            if has_id {
                continue;
            }
            let function_name = function_call
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("unknown")
                .to_string();
            let count = name_counters.entry(function_name.clone()).or_insert(0);
            function_call.insert(
                "id".to_string(),
                Value::String(format!("call_{function_name}_{count}")),
            );
            *count += 1;
        }
    }
}

fn postprocess_private_response_value(data: &mut Value, report_context: &Value) {
    if !matches!(
        report_context.get("envelope_name").and_then(Value::as_str),
        Some(ANTIGRAVITY_V1INTERNAL_ENVELOPE_NAME)
    ) {
        return;
    }
    if let Some(object) = data.as_object_mut() {
        if !object.contains_key("_v1internal_response_id") {
            if let Some(response_id) = object.remove("responseId") {
                object.insert("_v1internal_response_id".to_string(), response_id);
            }
        }
    }
    inject_antigravity_sync_tool_ids(data, local_finalize_response_model(report_context));
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        maybe_build_provider_private_stream_normalizer, normalize_provider_private_report_context,
        normalize_provider_private_response_value, stream_body_contains_error_event,
        transform_provider_private_stream_line,
    };

    #[test]
    fn normalizes_supported_private_report_context() {
        let report_context = json!({
            "has_envelope": true,
            "envelope_name": "antigravity:v1internal",
            "provider_api_format": "gemini:generate_content",
        });
        let normalized = normalize_provider_private_report_context(Some(&report_context))
            .expect("context should normalize");
        assert_eq!(normalized["has_envelope"], json!(false));
        assert!(normalized.get("envelope_name").is_none());
    }

    #[test]
    fn unwraps_antigravity_sync_response_and_injects_ids() {
        let report_context = json!({
            "has_envelope": true,
            "provider_api_format": "gemini:generate_content",
            "envelope_name": "antigravity:v1internal",
            "mapped_model": "claude-sonnet-4-5",
        });
        let body = json!({
            "response": {
                "candidates": [{
                    "content": {
                        "parts": [{
                            "functionCall": {
                                "name": "get_weather",
                                "args": {"city": "SF"}
                            }
                        }]
                    }
                }]
            },
            "responseId": "resp_123"
        });

        let normalized = normalize_provider_private_response_value(body, &report_context)
            .expect("body should normalize");
        assert_eq!(normalized["_v1internal_response_id"], json!("resp_123"));
        assert_eq!(
            normalized["candidates"][0]["content"]["parts"][0]["functionCall"]["id"],
            json!("call_get_weather_0")
        );
    }

    #[test]
    fn unwraps_antigravity_stream_line_and_injects_ids() {
        let report_context = json!({
            "has_envelope": true,
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "envelope_name": "antigravity:v1internal",
            "mapped_model": "claude-sonnet-4-5",
        });
        let output = transform_provider_private_stream_line(
            &report_context,
            b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"SF\"}}}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"claude-sonnet-4-5\"},\"responseId\":\"resp_123\"}\n\n".to_vec(),
        )
        .expect("unwrap should succeed");
        let output_text = String::from_utf8(output).expect("text should decode");
        assert!(output_text.contains("\"_v1internal_response_id\":\"resp_123\""));
        assert!(output_text.contains("\"id\":\"call_get_weather_0\""));
    }

    #[test]
    fn private_stream_normalizer_unwraps_antigravity_stream() {
        let report_context = json!({
            "has_envelope": true,
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "envelope_name": "antigravity:v1internal",
            "mapped_model": "claude-sonnet-4-5",
        });
        let mut normalizer = maybe_build_provider_private_stream_normalizer(Some(&report_context))
            .expect("normalizer should exist");
        let output = normalizer
            .push_chunk(
                b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"SF\"}}}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"claude-sonnet-4-5\"},\"responseId\":\"resp_123\"}\n\n",
            )
            .expect("unwrap should succeed");
        let output_text = String::from_utf8(output).expect("text should decode");
        assert!(output_text.contains("\"_v1internal_response_id\":\"resp_123\""));
        assert!(output_text.contains("\"id\":\"call_get_weather_0\""));
    }

    #[test]
    fn detects_sse_error_events_without_explicit_type_field() {
        let body = br#"event: error
data: {"message":"bad"}

"#;
        assert!(stream_body_contains_error_event(body));
    }
}
