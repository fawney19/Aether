use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::finalize::common::{build_generated_tool_call_id, canonicalize_tool_arguments};
use crate::finalize::sse::encode_json_sse;
use crate::finalize::standard::stream_core::common::*;
use crate::finalize::PipelineFinalizeError;

#[derive(Default)]
struct GeminiProviderToolState {
    call_id: String,
    name: String,
    arguments: String,
    started_emitted: bool,
}

#[derive(Default)]
pub struct GeminiProviderState {
    response_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
    text_parts: BTreeMap<usize, String>,
    tool_calls: BTreeMap<usize, GeminiProviderToolState>,
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

    pub fn push_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<Vec<CanonicalStreamFrame>, PipelineFinalizeError> {
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
                if let Some(text) = part_object.get("text").and_then(Value::as_str) {
                    let previous = self.text_parts.entry(index).or_default();
                    let delta = if text.starts_with(previous.as_str()) {
                        text[previous.len()..].to_string()
                    } else if previous.as_str() == text {
                        String::new()
                    } else {
                        text.to_string()
                    };
                    *previous = text.to_string();
                    if !delta.is_empty() {
                        out.push(CanonicalStreamFrame {
                            id: id.clone(),
                            model: model.clone(),
                            event: CanonicalStreamEvent::TextDelta(delta),
                        });
                    }
                    continue;
                }
                let Some(function_call) =
                    part_object.get("functionCall").and_then(Value::as_object)
                else {
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
                    "SAFETY" => Some("content_filter"),
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
    ) -> Result<Vec<CanonicalStreamFrame>, PipelineFinalizeError> {
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
    ) -> Result<Vec<u8>, PipelineFinalizeError> {
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
            response.insert(
                "usageMetadata".to_string(),
                json!({
                    "promptTokenCount": usage.input_tokens,
                    "candidatesTokenCount": usage.output_tokens,
                    "totalTokenCount": usage.total_tokens,
                }),
            );
        }
        encode_json_sse(None, &Value::Object(response))
    }

    fn flush_pending_tool_calls(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
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

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, PipelineFinalizeError> {
        self.update_identity(&frame);
        match frame.event {
            CanonicalStreamEvent::Start => Ok(Vec::new()),
            CanonicalStreamEvent::TextDelta(text) => {
                self.emit_candidate(vec![json!({ "text": text })], None, None)
            }
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

    pub fn finish(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
        if self.finished {
            return Ok(Vec::new());
        }
        let out = self.flush_pending_tool_calls()?;
        self.finished = true;
        Ok(out)
    }
}
