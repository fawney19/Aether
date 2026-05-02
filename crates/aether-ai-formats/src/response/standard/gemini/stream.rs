use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::response::common::{build_generated_tool_call_id, canonicalize_tool_arguments};
use crate::response::sse::encode_json_sse;
use crate::response::standard::stream_core::common::*;
use crate::response::AiSurfaceFinalizeError;

#[derive(Default)]
struct GeminiProviderToolState {
    call_id: String,
    name: String,
    arguments: String,
    started_emitted: bool,
}

#[derive(Default)]
struct GeminiProviderToolResultState {
    content: String,
    emitted: bool,
}

#[derive(Default)]
pub struct GeminiProviderState {
    response_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
    text_parts: BTreeMap<usize, String>,
    reasoning_parts: BTreeMap<usize, String>,
    reasoning_signatures: BTreeMap<usize, String>,
    content_parts: BTreeMap<usize, CanonicalContentPart>,
    tool_calls: BTreeMap<usize, GeminiProviderToolState>,
    tool_results: BTreeMap<usize, GeminiProviderToolResultState>,
}

impl GeminiProviderState {
    fn identity(&self, report_context: &Value) -> (String, String) {
        resolve_identity(
            self.response_id.as_deref(),
            self.model.as_deref(),
            report_context,
            "resp-local-stream",
        )
    }

    fn ensure_started(&mut self, report_context: &Value, out: &mut Vec<CanonicalStreamFrame>) {
        if self.started {
            return;
        }
        let (id, model) = self.identity(report_context);
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::Start,
        });
        self.started = true;
    }

    fn unknown_frame(&self, report_context: &Value, payload: Value) -> CanonicalStreamFrame {
        let (id, model) = self.identity(report_context);
        CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::UnknownEvent(payload),
        }
    }

    pub fn push_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<Vec<CanonicalStreamFrame>, AiSurfaceFinalizeError> {
        let Some(value) = decode_json_data_line(&line) else {
            return Ok(Vec::new());
        };
        let Some(raw_event_object) = value.as_object() else {
            return Ok(Vec::new());
        };
        if let Some(id) = raw_event_object.get("responseId").and_then(Value::as_str) {
            self.response_id = Some(id.to_string());
        }
        let event_object = raw_event_object
            .get("response")
            .and_then(Value::as_object)
            .filter(|response| response.contains_key("candidates"))
            .unwrap_or(raw_event_object);
        if let Some(id) = event_object.get("responseId").and_then(Value::as_str) {
            self.response_id = Some(id.to_string());
        }
        if let Some(version) = event_object.get("modelVersion").and_then(Value::as_str) {
            self.model = Some(version.to_string());
        }

        let mut out = Vec::new();
        let Some(candidates) = event_object.get("candidates").and_then(Value::as_array) else {
            out.push(self.unknown_frame(report_context, value.clone()));
            return Ok(out);
        };

        for candidate in candidates {
            let Some(candidate_object) = candidate.as_object() else {
                continue;
            };
            let Some(content) = candidate_object.get("content").and_then(Value::as_object) else {
                continue;
            };
            let Some(parts) = content.get("parts").and_then(Value::as_array) else {
                continue;
            };
            if !parts.is_empty() {
                self.ensure_started(report_context, &mut out);
            }
            let (id, model) = self.identity(report_context);
            for (index, part) in parts.iter().enumerate() {
                let Some(part_object) = part.as_object() else {
                    continue;
                };
                let reasoning_signature = part_object
                    .get("thoughtSignature")
                    .or_else(|| part_object.get("thought_signature"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                if let Some(text) = render_gemini_part_as_text(part_object) {
                    let is_reasoning = part_object
                        .get("thought")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let previous = if is_reasoning {
                        self.reasoning_parts.entry(index).or_default()
                    } else {
                        self.text_parts.entry(index).or_default()
                    };
                    let delta = if text.starts_with(previous.as_str()) {
                        text[previous.len()..].to_string()
                    } else if previous.as_str() == text {
                        String::new()
                    } else {
                        text.to_string()
                    };
                    *previous = text;
                    if !delta.is_empty() {
                        out.push(CanonicalStreamFrame {
                            id: id.clone(),
                            model: model.clone(),
                            event: if is_reasoning {
                                CanonicalStreamEvent::ReasoningDelta(delta)
                            } else {
                                CanonicalStreamEvent::TextDelta(delta)
                            },
                        });
                    }
                    if is_reasoning {
                        if let Some(signature) = reasoning_signature.as_ref() {
                            let previous_signature =
                                self.reasoning_signatures.entry(index).or_default();
                            if previous_signature.as_str() != signature.as_str() {
                                *previous_signature = signature.clone();
                                out.push(CanonicalStreamFrame {
                                    id: id.clone(),
                                    model: model.clone(),
                                    event: CanonicalStreamEvent::ReasoningSignature(
                                        signature.clone(),
                                    ),
                                });
                            }
                        }
                    }
                    continue;
                }
                if let Some(function_response) = part_object
                    .get("functionResponse")
                    .or_else(|| part_object.get("function_response"))
                    .and_then(Value::as_object)
                {
                    let tool_use_id = function_response
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| build_generated_tool_call_id(index));
                    let name = function_response
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned);
                    let content = gemini_function_response_content(
                        function_response.get("response").unwrap_or(&Value::Null),
                    );
                    let state = self.tool_results.entry(index).or_default();
                    let delta = if !state.emitted {
                        content.clone()
                    } else if content.starts_with(&state.content) {
                        content[state.content.len()..].to_string()
                    } else if state.content == content {
                        String::new()
                    } else {
                        content.clone()
                    };
                    if !delta.is_empty() || !state.emitted {
                        state.emitted = true;
                        state.content.push_str(&delta);
                        out.push(CanonicalStreamFrame {
                            id: id.clone(),
                            model: model.clone(),
                            event: CanonicalStreamEvent::ToolResultDelta {
                                index,
                                tool_use_id,
                                name,
                                content: delta,
                            },
                        });
                    }
                    continue;
                }
                let Some(function_call) =
                    part_object.get("functionCall").and_then(Value::as_object)
                else {
                    if let Some(content_part) = canonical_content_part_from_gemini_part(part_object)
                    {
                        let should_emit = self
                            .content_parts
                            .get(&index)
                            .map(|existing| existing != &content_part)
                            .unwrap_or(true);
                        if should_emit {
                            self.content_parts.insert(index, content_part.clone());
                            out.push(CanonicalStreamFrame {
                                id: id.clone(),
                                model: model.clone(),
                                event: CanonicalStreamEvent::ContentPart(content_part),
                            });
                        }
                    } else {
                        out.push(
                            self.unknown_frame(report_context, Value::Object(part_object.clone())),
                        );
                    }
                    continue;
                };
                let tool_state = self.tool_calls.entry(index).or_default();
                tool_state.call_id = function_call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(tool_state.call_id.as_str())
                    .to_string();
                tool_state.name = function_call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(tool_state.name.as_str())
                    .to_string();
                if !tool_state.started_emitted {
                    out.push(CanonicalStreamFrame {
                        id: id.clone(),
                        model: model.clone(),
                        event: CanonicalStreamEvent::ToolCallStart {
                            index,
                            call_id: if tool_state.call_id.is_empty() {
                                build_generated_tool_call_id(index)
                            } else {
                                tool_state.call_id.clone()
                            },
                            name: if tool_state.name.is_empty() {
                                "unknown".to_string()
                            } else {
                                tool_state.name.clone()
                            },
                        },
                    });
                    tool_state.started_emitted = true;
                }
                let arguments = canonicalize_tool_arguments(function_call.get("args").cloned());
                let delta = if arguments.starts_with(&tool_state.arguments) {
                    arguments[tool_state.arguments.len()..].to_string()
                } else if tool_state.arguments == arguments {
                    String::new()
                } else {
                    arguments.clone()
                };
                tool_state.arguments = arguments;
                if !delta.is_empty() {
                    out.push(CanonicalStreamFrame {
                        id: id.clone(),
                        model: model.clone(),
                        event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                            index,
                            arguments: delta,
                        },
                    });
                }
            }
            if let Some(finish_reason) =
                candidate_object.get("finishReason").and_then(Value::as_str)
            {
                let has_tool_calls = !self.tool_calls.is_empty();
                let mut finish_reason = normalize_openai_finish_reason(match finish_reason {
                    "STOP" => Some("stop"),
                    "MAX_TOKENS" => Some("length"),
                    "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII"
                    | "OTHER" => Some("content_filter"),
                    other => Some(other),
                });
                if has_tool_calls && finish_reason.as_deref().is_none_or(|value| value == "stop") {
                    finish_reason = Some("tool_calls".to_string());
                }
                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::Finish {
                        finish_reason,
                        usage: canonical_usage_from_gemini_usage(event_object.get("usageMetadata")),
                    },
                });
                self.finished = true;
            }
        }

        Ok(out)
    }

    pub fn finish(
        &mut self,
        report_context: &Value,
    ) -> Result<Vec<CanonicalStreamFrame>, AiSurfaceFinalizeError> {
        if !self.started || self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;
        let (id, model) = self.identity(report_context);
        Ok(vec![CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::Finish {
                finish_reason: None,
                usage: None,
            },
        }])
    }
}

#[derive(Default)]
struct GeminiClientToolState {
    call_id: String,
    name: String,
    arguments: String,
    emitted: bool,
}

#[derive(Default)]
pub struct GeminiClientEmitter {
    response_id: Option<String>,
    model: Option<String>,
    finished: bool,
    tool_calls: BTreeMap<usize, GeminiClientToolState>,
}

impl GeminiClientEmitter {
    fn update_identity(&mut self, frame: &CanonicalStreamFrame) {
        self.response_id = Some(frame.id.clone());
        self.model = Some(frame.model.clone());
    }

    fn emit_candidate(
        &self,
        parts: Vec<Value>,
        finish_reason: Option<&str>,
        usage: Option<CanonicalUsage>,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut candidate = Map::new();
        candidate.insert(
            "content".to_string(),
            json!({
                "role": "model",
                "parts": parts,
            }),
        );
        candidate.insert("index".to_string(), Value::from(0_u64));
        if let Some(finish_reason) = finish_reason {
            candidate.insert(
                "finishReason".to_string(),
                Value::String(map_openai_finish_reason_to_gemini(Some(finish_reason)).to_string()),
            );
        }
        let mut response = Map::new();
        response.insert(
            "responseId".to_string(),
            Value::String(
                self.response_id
                    .clone()
                    .unwrap_or_else(|| "resp-local-stream".to_string()),
            ),
        );
        response.insert(
            "modelVersion".to_string(),
            Value::String(self.model.clone().unwrap_or_else(|| "unknown".to_string())),
        );
        response.insert(
            "candidates".to_string(),
            Value::Array(vec![Value::Object(candidate)]),
        );
        if let Some(usage) = usage {
            let visible_output_tokens = usage.output_tokens.saturating_sub(usage.reasoning_tokens);
            let mut usage_metadata = Map::new();
            usage_metadata.insert(
                "promptTokenCount".to_string(),
                Value::from(usage.input_tokens),
            );
            usage_metadata.insert(
                "candidatesTokenCount".to_string(),
                Value::from(visible_output_tokens),
            );
            usage_metadata.insert(
                "totalTokenCount".to_string(),
                Value::from(usage.total_tokens),
            );
            if usage.reasoning_tokens > 0 {
                usage_metadata.insert(
                    "thoughtsTokenCount".to_string(),
                    Value::from(usage.reasoning_tokens),
                );
            }
            response.insert("usageMetadata".to_string(), Value::Object(usage_metadata));
        }
        encode_json_sse(None, &Value::Object(response))
    }

    fn flush_pending_tool_calls(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = Vec::new();
        let mut pending = Vec::new();
        for (index, tool_call) in &mut self.tool_calls {
            if tool_call.emitted {
                continue;
            }
            let args_value = parse_json_arguments_value(&tool_call.arguments)
                .unwrap_or_else(|| Value::Object(Map::new()));
            tool_call.emitted = true;
            pending.push(json!({
                "functionCall": {
                    "id": if tool_call.call_id.is_empty() {
                        build_generated_tool_call_id(*index)
                    } else {
                        tool_call.call_id.clone()
                    },
                    "name": if tool_call.name.is_empty() {
                        "unknown".to_string()
                    } else {
                        tool_call.name.clone()
                    },
                    "args": args_value,
                }
            }));
        }
        for part in pending {
            out.extend(self.emit_candidate(vec![part], None, None)?);
        }
        Ok(out)
    }

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.update_identity(&frame);
        match frame.event {
            CanonicalStreamEvent::Start => Ok(Vec::new()),
            CanonicalStreamEvent::TextDelta(text) => {
                self.emit_candidate(vec![json!({ "text": text })], None, None)
            }
            CanonicalStreamEvent::ReasoningDelta(text) => {
                self.emit_candidate(vec![json!({ "text": text, "thought": true })], None, None)
            }
            CanonicalStreamEvent::ReasoningSignature(signature) => self.emit_candidate(
                vec![json!({
                    "text": "",
                    "thought": true,
                    "thoughtSignature": signature,
                })],
                None,
                None,
            ),
            CanonicalStreamEvent::ContentPart(part) => self.emit_candidate(
                vec![gemini_part_from_canonical_content_part(part)],
                None,
                None,
            ),
            CanonicalStreamEvent::ToolCallStart {
                index,
                call_id,
                name,
            } => {
                let state = self.tool_calls.entry(index).or_default();
                state.call_id = call_id;
                state.name = name;
                Ok(Vec::new())
            }
            CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments } => {
                let emitted_part = {
                    let state = self.tool_calls.entry(index).or_default();
                    state.arguments.push_str(&arguments);
                    if state.emitted {
                        None
                    } else {
                        let args_value = parse_json_arguments_value(&state.arguments);
                        args_value.map(|args_value| {
                            state.emitted = true;
                            json!({
                                "functionCall": {
                                    "id": if state.call_id.is_empty() {
                                        build_generated_tool_call_id(index)
                                    } else {
                                        state.call_id.clone()
                                    },
                                    "name": if state.name.is_empty() {
                                        "unknown".to_string()
                                    } else {
                                        state.name.clone()
                                    },
                                    "args": args_value,
                                }
                            })
                        })
                    }
                };
                let Some(part) = emitted_part else {
                    return Ok(Vec::new());
                };
                self.emit_candidate(vec![part], None, None)
            }
            CanonicalStreamEvent::ToolResultDelta {
                tool_use_id,
                name,
                content,
                ..
            } => self.emit_candidate(
                vec![gemini_function_response_part(tool_use_id, name, content)],
                None,
                None,
            ),
            CanonicalStreamEvent::UnknownEvent(_) => Ok(Vec::new()),
            CanonicalStreamEvent::Finish {
                finish_reason,
                usage,
            } => {
                if self.finished {
                    return Ok(Vec::new());
                }
                let mut out = self.flush_pending_tool_calls()?;
                out.extend(self.emit_candidate(vec![], finish_reason.as_deref(), usage)?);
                self.finished = true;
                Ok(out)
            }
        }
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.finished {
            return Ok(Vec::new());
        }
        let out = self.flush_pending_tool_calls()?;
        self.finished = true;
        Ok(out)
    }
}

fn gemini_function_response_part(
    tool_use_id: String,
    name: Option<String>,
    content: String,
) -> Value {
    json!({
        "functionResponse": {
            "id": tool_use_id,
            "name": name
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "unknown".to_string()),
            "response": gemini_function_response_value(&content),
        }
    })
}

fn gemini_function_response_value(content: &str) -> Value {
    if content.trim().is_empty() {
        return Value::Object(Map::new());
    }
    match serde_json::from_str::<Value>(content) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(value) => json!({ "output": value }),
        Err(_) => json!({ "output": content }),
    }
}

fn gemini_function_response_content(response: &Value) -> String {
    match response {
        Value::Object(object) => object
            .get("result")
            .cloned()
            .unwrap_or_else(|| Value::Object(object.clone()))
            .to_string(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn render_gemini_part_as_text(part: &Map<String, Value>) -> Option<String> {
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    if let Some(code) = part.get("executableCode").and_then(Value::as_object) {
        let language = code
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let source = code.get("code").and_then(Value::as_str).unwrap_or_default();
        return Some(format!("```{language}\n{source}\n```"));
    }
    if let Some(result) = part.get("codeExecutionResult").and_then(Value::as_object) {
        let output = result
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Some(format!("```output\n{output}\n```"));
    }
    None
}

fn canonical_content_part_from_gemini_part(
    part: &Map<String, Value>,
) -> Option<CanonicalContentPart> {
    if let Some(image_url) = extract_gemini_image_url(part) {
        return Some(CanonicalContentPart::ImageUrl(image_url));
    }
    if let Some(inline_data) = part
        .get("inlineData")
        .or_else(|| part.get("inline_data"))
        .and_then(Value::as_object)
    {
        let mime_type = inline_data
            .get("mimeType")
            .or_else(|| inline_data.get("mime_type"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let data = inline_data
            .get("data")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        if let Some(format) = mime_type.strip_prefix("audio/") {
            return Some(CanonicalContentPart::Audio {
                data: data.to_string(),
                format: format.to_string(),
            });
        }
        return Some(CanonicalContentPart::File {
            file_data: Some(format!("data:{mime_type};base64,{data}")),
            reference: None,
            mime_type: Some(mime_type.to_string()),
            filename: None,
        });
    }
    let file_data = part
        .get("fileData")
        .or_else(|| part.get("file_data"))
        .and_then(Value::as_object)?;
    let reference = file_data
        .get("fileUri")
        .or_else(|| file_data.get("file_uri"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(CanonicalContentPart::File {
        file_data: None,
        reference: Some(reference.to_string()),
        mime_type: file_data
            .get("mimeType")
            .or_else(|| file_data.get("mime_type"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        filename: None,
    })
}

fn gemini_part_from_canonical_content_part(part: CanonicalContentPart) -> Value {
    match part {
        CanonicalContentPart::ImageUrl(url) => {
            if let Some((mime_type, data)) = parse_data_url(url.as_str()) {
                json!({
                    "inlineData": {
                        "mimeType": mime_type,
                        "data": data,
                    }
                })
            } else {
                json!({
                    "fileData": {
                        "fileUri": url.clone(),
                        "mimeType": guess_media_type_from_reference(url.as_str(), "image/jpeg"),
                    }
                })
            }
        }
        CanonicalContentPart::File {
            file_data,
            reference,
            mime_type,
            ..
        } => {
            if let Some(file_data) = file_data {
                if let Some((mime_type, data)) = parse_data_url(file_data.as_str()) {
                    json!({
                        "inlineData": {
                            "mimeType": mime_type,
                            "data": data,
                        }
                    })
                } else {
                    json!({ "text": "[File]" })
                }
            } else if let Some(reference) = reference {
                json!({
                    "fileData": {
                        "fileUri": reference.clone(),
                        "mimeType": mime_type.unwrap_or_else(|| {
                            guess_media_type_from_reference(reference.as_str(), "application/octet-stream")
                        }),
                    }
                })
            } else {
                json!({ "text": "[File]" })
            }
        }
        CanonicalContentPart::Audio { data, format } => json!({
            "inlineData": {
                "mimeType": format!("audio/{format}"),
                "data": data,
            }
        }),
    }
}

fn extract_gemini_image_url(part: &Map<String, Value>) -> Option<String> {
    let inline_data = part
        .get("inlineData")
        .or_else(|| part.get("inline_data"))
        .and_then(Value::as_object)?;
    if !inline_data
        .get("mimeType")
        .or_else(|| inline_data.get("mime_type"))
        .and_then(Value::as_str)
        .is_some_and(|value| value.starts_with("image/"))
    {
        return None;
    }
    if let Some(data) = inline_data
        .get("data")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let mime_type = inline_data
            .get("mimeType")
            .or_else(|| inline_data.get("mime_type"))
            .and_then(Value::as_str)
            .unwrap_or("image/jpeg");
        return Some(format!("data:{mime_type};base64,{data}"));
    }
    let file_data = part
        .get("fileData")
        .or_else(|| part.get("file_data"))
        .and_then(Value::as_object)?;
    if !file_data
        .get("mimeType")
        .or_else(|| file_data.get("mime_type"))
        .and_then(Value::as_str)
        .is_some_and(|value| value.starts_with("image/"))
    {
        return None;
    }
    file_data
        .get("fileUri")
        .or_else(|| file_data.get("file_uri"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn parse_data_url(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime_type = meta.strip_suffix(";base64")?;
    if mime_type.trim().is_empty() || data.trim().is_empty() {
        return None;
    }
    Some((mime_type.to_string(), data.to_string()))
}

fn guess_media_type_from_reference(reference: &str, default_mime: &str) -> String {
    let normalized = reference
        .split('?')
        .next()
        .unwrap_or(reference)
        .to_ascii_lowercase();
    if normalized.ends_with(".png") {
        "image/png".to_string()
    } else if normalized.ends_with(".gif") {
        "image/gif".to_string()
    } else if normalized.ends_with(".webp") {
        "image/webp".to_string()
    } else if normalized.ends_with(".jpg") || normalized.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if normalized.ends_with(".pdf") {
        "application/pdf".to_string()
    } else {
        default_mime.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data_line(value: Value) -> Vec<u8> {
        format!("data: {}\n", value).into_bytes()
    }

    #[test]
    fn gemini_provider_state_emits_unknown_events_for_unknown_parts() {
        let mut state = GeminiProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "responseId": "resp_unknown_123",
                    "modelVersion": "gemini-2.5-pro",
                    "candidates": [{
                        "index": 0,
                        "content": {
                            "parts": [
                                {
                                    "futurePart": {
                                        "kept": true
                                    }
                                }
                            ]
                        }
                    }]
                })),
            )
            .expect("unknown part should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::UnknownEvent(ref payload)
                if payload.get("futurePart").is_some()
        )));
    }

    #[test]
    fn gemini_provider_state_parses_thoughts_code_and_content_filter_finish() {
        let mut state = GeminiProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "responseId": "resp_123",
                    "modelVersion": "gemini-2.5-pro",
                    "candidates": [{
                        "index": 0,
                        "finishReason": "RECITATION",
                        "content": {
                            "parts": [
                                { "text": "reason", "thought": true, "thoughtSignature": "sig_123" },
                                { "executableCode": { "language": "python", "code": "print(1)" } }
                            ]
                        }
                    }],
                    "usageMetadata": {
                        "promptTokenCount": 1,
                        "candidatesTokenCount": 2,
                        "thoughtsTokenCount": 4,
                        "totalTokenCount": 7
                    }
                })),
            )
            .expect("chunk should parse");

        assert!(matches!(frames[0].event, CanonicalStreamEvent::Start));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ReasoningDelta(ref text) if text == "reason"
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ReasoningSignature(ref signature) if signature == "sig_123"
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::TextDelta(ref text) if text == "```python\nprint(1)\n```"
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::Finish { ref finish_reason, .. }
                if finish_reason.as_deref() == Some("content_filter")
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::Finish {
                usage: Some(CanonicalUsage {
                    input_tokens: 1,
                    output_tokens: 6,
                    reasoning_tokens: 4,
                    total_tokens: 7,
                    ..
                }),
                ..
            }
        )));
    }

    #[test]
    fn gemini_provider_state_parses_function_response_as_tool_result() {
        let mut state = GeminiProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "responseId": "resp_tool_result_123",
                    "modelVersion": "gemini-2.5-pro",
                    "candidates": [{
                        "index": 0,
                        "content": {
                            "parts": [{
                                "functionResponse": {
                                    "id": "call_123",
                                    "name": "lookup",
                                    "response": {"ok": true}
                                }
                            }]
                        }
                    }]
                })),
            )
            .expect("function response should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolResultDelta {
                index: 0,
                ref tool_use_id,
                name: Some(ref name),
                ref content,
            } if tool_use_id == "call_123" && name == "lookup" && content == "{\"ok\":true}"
        )));
    }

    #[test]
    fn gemini_client_emitter_marks_reasoning_parts_as_thoughts() {
        let mut emitter = GeminiClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_123".to_string(),
                model: "gemini-2.5-pro".to_string(),
                event: CanonicalStreamEvent::ReasoningDelta("reason".to_string()),
            })
            .expect("reasoning should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_123".to_string(),
                    model: "gemini-2.5-pro".to_string(),
                    event: CanonicalStreamEvent::ReasoningSignature("sig_123".to_string()),
                })
                .expect("signature should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_123".to_string(),
                    model: "gemini-2.5-pro".to_string(),
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some("stop".to_string()),
                        usage: Some(CanonicalUsage {
                            input_tokens: 1,
                            output_tokens: 3,
                            reasoning_tokens: 1,
                            total_tokens: 4,
                            ..CanonicalUsage::default()
                        }),
                    },
                })
                .expect("finish should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"thought\":true"));
        assert!(sse.contains("\"thoughtSignature\":\"sig_123\""));
        assert!(sse.contains("\"thoughtsTokenCount\":1"));
        assert!(sse.contains("\"candidatesTokenCount\":2"));
        assert!(sse.contains("\"finishReason\":\"STOP\""));
    }

    #[test]
    fn gemini_provider_state_parses_inline_image_parts() {
        let mut state = GeminiProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "responseId": "resp_media_123",
                    "modelVersion": "gemini-2.5-pro",
                    "candidates": [{
                        "index": 0,
                        "content": {
                            "parts": [
                                { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo=" } }
                            ]
                        }
                    }]
                })),
            )
            .expect("chunk should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ContentPart(CanonicalContentPart::ImageUrl(ref url))
                if url == "data:image/png;base64,iVBORw0KGgo="
        )));
    }

    #[test]
    fn gemini_client_emitter_emits_inline_image_parts() {
        let mut emitter = GeminiClientEmitter::default();
        let bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_img_123".to_string(),
                model: "gemini-2.5-pro".to_string(),
                event: CanonicalStreamEvent::ContentPart(CanonicalContentPart::ImageUrl(
                    "data:image/png;base64,iVBORw0KGgo=".to_string(),
                )),
            })
            .expect("image should encode");

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(
            sse.contains("\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"iVBORw0KGgo=\"}")
        );
    }

    #[test]
    fn gemini_client_emitter_emits_function_response_for_tool_results() {
        let mut emitter = GeminiClientEmitter::default();
        let bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_tool_result_123".to_string(),
                model: "gemini-2.5-pro".to_string(),
                event: CanonicalStreamEvent::ToolResultDelta {
                    index: 2,
                    tool_use_id: "call_123".to_string(),
                    name: Some("lookup".to_string()),
                    content: "{\"ok\":true}".to_string(),
                },
            })
            .expect("tool result should encode");

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"functionResponse\""));
        assert!(sse.contains("\"id\":\"call_123\""));
        assert!(sse.contains("\"name\":\"lookup\""));
        assert!(sse.contains("\"response\":{\"ok\":true}"));
    }
}
