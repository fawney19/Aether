use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::response::common::{
    build_generated_tool_call_id, canonicalize_tool_arguments,
    remove_empty_pages_from_tool_arguments,
};
use crate::response::sse::{encode_json_sse, map_claude_stop_reason};
use crate::response::standard::stream_core::common::*;
use crate::response::AiSurfaceFinalizeError;

#[derive(Default)]
struct ClaudeProviderToolState {
    call_id: String,
    name: String,
    started_emitted: bool,
}

#[derive(Default)]
pub struct ClaudeProviderState {
    message_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
    usage: Option<CanonicalUsage>,
    tool_calls: BTreeMap<usize, ClaudeProviderToolState>,
}

impl ClaudeProviderState {
    fn identity(&self, report_context: &Value) -> (String, String) {
        resolve_identity(
            self.message_id.as_deref(),
            self.model.as_deref(),
            report_context,
            "msg-local-stream",
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
        let Some(event_object) = value.as_object() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        match event_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "message_start" => {
                if let Some(message) = event_object.get("message").and_then(Value::as_object) {
                    self.message_id = message
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    self.model = message
                        .get("model")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    self.merge_usage(canonical_usage_from_claude_usage(message.get("usage")));
                }
                self.ensure_started(report_context, &mut out);
            }
            "content_block_delta" => {
                let index = event_object
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
                    .unwrap_or(0);
                let Some(delta) = event_object.get("delta").and_then(Value::as_object) else {
                    return Ok(out);
                };
                match delta
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "text_delta" => {
                        let Some(piece) = delta.get("text").and_then(Value::as_str) else {
                            return Ok(out);
                        };
                        if piece.is_empty() {
                            return Ok(out);
                        }
                        self.ensure_started(report_context, &mut out);
                        let (id, model) = self.identity(report_context);
                        out.push(CanonicalStreamFrame {
                            id,
                            model,
                            event: CanonicalStreamEvent::TextDelta(piece.to_string()),
                        });
                    }
                    "input_json_delta" => {
                        let Some(partial_json) = delta.get("partial_json").and_then(Value::as_str)
                        else {
                            return Ok(out);
                        };
                        if partial_json.is_empty() {
                            return Ok(out);
                        }
                        self.ensure_started(report_context, &mut out);
                        let (id, model) = self.identity(report_context);
                        let tool_state = self.tool_calls.entry(index).or_default();
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
                        out.push(CanonicalStreamFrame {
                            id,
                            model,
                            event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                                index,
                                arguments: partial_json.to_string(),
                            },
                        });
                    }
                    "thinking_delta" => {
                        let Some(piece) = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .or_else(|| delta.get("text").and_then(Value::as_str))
                        else {
                            return Ok(out);
                        };
                        if piece.is_empty() {
                            return Ok(out);
                        }
                        self.ensure_started(report_context, &mut out);
                        let (id, model) = self.identity(report_context);
                        out.push(CanonicalStreamFrame {
                            id,
                            model,
                            event: CanonicalStreamEvent::ReasoningDelta(piece.to_string()),
                        });
                    }
                    "signature_delta" => {
                        let Some(signature) = delta.get("signature").and_then(Value::as_str) else {
                            return Ok(out);
                        };
                        if signature.is_empty() {
                            return Ok(out);
                        }
                        self.ensure_started(report_context, &mut out);
                        let (id, model) = self.identity(report_context);
                        out.push(CanonicalStreamFrame {
                            id,
                            model,
                            event: CanonicalStreamEvent::ReasoningSignature(signature.to_string()),
                        });
                    }
                    _ => {
                        out.push(
                            self.unknown_frame(report_context, Value::Object(event_object.clone())),
                        );
                    }
                }
            }
            "content_block_start" => {
                let index = event_object
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
                    .unwrap_or(0);
                let Some(block) = event_object.get("content_block").and_then(Value::as_object)
                else {
                    return Ok(out);
                };
                let block_type = block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if block_type == "thinking" {
                    let Some(piece) = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .or_else(|| block.get("text").and_then(Value::as_str))
                    else {
                        return Ok(out);
                    };
                    if piece.is_empty() {
                        return Ok(out);
                    }
                    self.ensure_started(report_context, &mut out);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::ReasoningDelta(piece.to_string()),
                    });
                    return Ok(out);
                }
                if block_type == "text" {
                    let Some(text) = block.get("text").and_then(Value::as_str) else {
                        return Ok(out);
                    };
                    if text.is_empty() {
                        return Ok(out);
                    }
                    self.ensure_started(report_context, &mut out);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::TextDelta(text.to_string()),
                    });
                    return Ok(out);
                }
                if let Some(part) = canonical_content_part_from_claude_block(block) {
                    self.ensure_started(report_context, &mut out);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::ContentPart(part),
                    });
                    return Ok(out);
                }
                if block_type != "tool_use" {
                    out.push(
                        self.unknown_frame(report_context, Value::Object(event_object.clone())),
                    );
                    return Ok(out);
                }
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);
                let tool_state = self.tool_calls.entry(index).or_default();
                tool_state.call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or(tool_state.call_id.as_str())
                    .to_string();
                tool_state.name = block
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
                let arguments = canonicalize_tool_arguments(block.get("input").cloned());
                if !arguments.is_empty() && arguments != "{}" {
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments },
                    });
                }
            }
            "message_delta" => {
                self.ensure_started(report_context, &mut out);
                let Some(delta) = event_object.get("delta").and_then(Value::as_object) else {
                    return Ok(out);
                };
                let finish_reason = map_claude_stop_reason(
                    delta.get("stop_reason").and_then(Value::as_str),
                    delta.get("stop_reason").and_then(Value::as_str) == Some("tool_use"),
                )
                .map(ToOwned::to_owned);
                let (id, model) = self.identity(report_context);
                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::Finish {
                        finish_reason,
                        usage: self.finish_usage(canonical_usage_from_claude_usage(
                            event_object.get("usage"),
                        )),
                    },
                });
                self.finished = true;
            }
            "content_block_stop" | "message_stop" | "ping" => {}
            _ => {
                out.push(self.unknown_frame(report_context, value.clone()));
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

    fn merge_usage(&mut self, usage: Option<CanonicalUsage>) {
        let Some(usage) = usage else {
            return;
        };
        let current = self.usage.take().unwrap_or_default();
        self.usage = Some(merge_claude_usage(current, usage));
    }

    fn finish_usage(&mut self, usage: Option<CanonicalUsage>) -> Option<CanonicalUsage> {
        self.merge_usage(usage);
        self.usage.take()
    }
}

enum ClaudeOpenBlock {
    Text {
        block_index: usize,
    },
    Thinking {
        block_index: usize,
    },
    Tool {
        tool_index: usize,
        block_index: usize,
    },
}

#[derive(Default)]
struct ClaudeClientToolState {
    call_id: String,
    name: String,
}

#[derive(Default)]
pub struct ClaudeClientEmitter {
    message_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
    next_block_index: usize,
    open_block: Option<ClaudeOpenBlock>,
    tool_block_indices: BTreeMap<usize, usize>,
    tool_states: BTreeMap<usize, ClaudeClientToolState>,
}

impl ClaudeClientEmitter {
    fn update_identity(&mut self, frame: &CanonicalStreamFrame) {
        self.message_id = Some(frame.id.clone());
        self.model = Some(frame.model.clone());
    }

    fn ensure_started(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.started {
            return Ok(Vec::new());
        }
        self.started = true;
        encode_json_sse(
            Some("message_start"),
            &json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id.as_deref().unwrap_or("msg-local-stream"),
                    "type": "message",
                    "role": "assistant",
                    "model": self.model.as_deref().unwrap_or("unknown"),
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 0,
                    },
                }
            }),
        )
    }

    fn close_open_block(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let Some(open_block) = self.open_block.take() else {
            return Ok(Vec::new());
        };
        let block_index = match open_block {
            ClaudeOpenBlock::Text { block_index } => block_index,
            ClaudeOpenBlock::Thinking { block_index } => block_index,
            ClaudeOpenBlock::Tool { block_index, .. } => block_index,
        };
        encode_json_sse(
            Some("content_block_stop"),
            &json!({
                "type": "content_block_stop",
                "index": block_index,
            }),
        )
    }

    fn ensure_text_block(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = Vec::new();
        if let Some(ClaudeOpenBlock::Text { .. }) = self.open_block {
            return Ok(out);
        }
        out.extend(self.close_open_block()?);
        let block_index = self.next_block_index;
        self.next_block_index += 1;
        self.open_block = Some(ClaudeOpenBlock::Text { block_index });
        out.extend(encode_json_sse(
            Some("content_block_start"),
            &json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": "text",
                    "text": "",
                }
            }),
        )?);
        Ok(out)
    }

    fn ensure_thinking_block(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = Vec::new();
        if let Some(ClaudeOpenBlock::Thinking { .. }) = self.open_block {
            return Ok(out);
        }
        out.extend(self.close_open_block()?);
        let block_index = self.next_block_index;
        self.next_block_index += 1;
        self.open_block = Some(ClaudeOpenBlock::Thinking { block_index });
        out.extend(encode_json_sse(
            Some("content_block_start"),
            &json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": "thinking",
                    "thinking": "",
                }
            }),
        )?);
        Ok(out)
    }

    fn ensure_tool_block(
        &mut self,
        tool_index: usize,
        call_id: &str,
        name: &str,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = Vec::new();
        if let Some(ClaudeOpenBlock::Tool {
            tool_index: current_tool_index,
            ..
        }) = self.open_block
        {
            if current_tool_index == tool_index {
                return Ok(out);
            }
        }
        out.extend(self.close_open_block()?);
        let block_index = self
            .tool_block_indices
            .get(&tool_index)
            .copied()
            .unwrap_or_else(|| {
                let block_index = self.next_block_index;
                self.next_block_index += 1;
                self.tool_block_indices.insert(tool_index, block_index);
                block_index
            });
        self.open_block = Some(ClaudeOpenBlock::Tool {
            tool_index,
            block_index,
        });
        out.extend(encode_json_sse(
            Some("content_block_start"),
            &json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": "tool_use",
                    "id": call_id,
                    "name": name,
                    "input": {},
                }
            }),
        )?);
        Ok(out)
    }

    fn emit_tool_result_block(
        &mut self,
        index: usize,
        tool_use_id: String,
        name: Option<String>,
        content: String,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = self.ensure_started()?;
        out.extend(self.close_open_block()?);
        let block_index = self.next_block_index;
        self.next_block_index += 1;
        let mut content_block = Map::new();
        content_block.insert("type".to_string(), Value::String("tool_result".to_string()));
        content_block.insert("tool_use_id".to_string(), Value::String(tool_use_id));
        if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
            content_block.insert("name".to_string(), Value::String(name));
        }
        content_block.insert("content".to_string(), Value::String(content));
        out.extend(encode_json_sse(
            Some("content_block_start"),
            &json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": Value::Object(content_block),
            }),
        )?);
        out.extend(encode_json_sse(
            Some("content_block_stop"),
            &json!({
                "type": "content_block_stop",
                "index": block_index,
                "canonical_index": index,
            }),
        )?);
        Ok(out)
    }

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.update_identity(&frame);
        match frame.event {
            CanonicalStreamEvent::Start => self.ensure_started(),
            CanonicalStreamEvent::TextDelta(text) => {
                let mut out = self.ensure_started()?;
                out.extend(self.ensure_text_block()?);
                let block_index = match self.open_block {
                    Some(ClaudeOpenBlock::Text { block_index }) => block_index,
                    _ => return Ok(out),
                };
                out.extend(encode_json_sse(
                    Some("content_block_delta"),
                    &json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": {
                            "type": "text_delta",
                            "text": text,
                        }
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningDelta(text) => {
                let mut out = self.ensure_started()?;
                out.extend(self.ensure_thinking_block()?);
                let block_index = match self.open_block {
                    Some(ClaudeOpenBlock::Thinking { block_index }) => block_index,
                    _ => return Ok(out),
                };
                out.extend(encode_json_sse(
                    Some("content_block_delta"),
                    &json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": {
                            "type": "thinking_delta",
                            "thinking": text,
                        }
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningSignature(signature) => {
                let mut out = self.ensure_started()?;
                out.extend(self.ensure_thinking_block()?);
                let block_index = match self.open_block {
                    Some(ClaudeOpenBlock::Thinking { block_index }) => block_index,
                    _ => return Ok(out),
                };
                out.extend(encode_json_sse(
                    Some("content_block_delta"),
                    &json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": {
                            "type": "signature_delta",
                            "signature": signature,
                        }
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ContentPart(part) => self.emit_content_part(part),
            CanonicalStreamEvent::ToolCallStart {
                index,
                call_id,
                name,
            } => {
                let mut out = self.ensure_started()?;
                let state = self.tool_states.entry(index).or_default();
                state.call_id = call_id.clone();
                state.name = name.clone();
                out.extend(self.ensure_tool_block(index, &call_id, &name)?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments } => {
                let arguments = remove_empty_pages_from_tool_arguments(&arguments);
                if arguments.is_empty() {
                    return Ok(Vec::new());
                }
                let mut out = self.ensure_started()?;
                let state = self.tool_states.entry(index).or_default();
                let call_id = if state.call_id.is_empty() {
                    format!("tool_{index}")
                } else {
                    state.call_id.clone()
                };
                let name = if state.name.is_empty() {
                    "unknown".to_string()
                } else {
                    state.name.clone()
                };
                out.extend(self.ensure_tool_block(index, &call_id, &name)?);
                let block_index = match self.open_block {
                    Some(ClaudeOpenBlock::Tool { block_index, .. }) => block_index,
                    _ => return Ok(out),
                };
                out.extend(encode_json_sse(
                    Some("content_block_delta"),
                    &json!({
                        "type": "content_block_delta",
                        "index": block_index,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": arguments,
                        }
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolResultDelta {
                index,
                tool_use_id,
                name,
                content,
            } => self.emit_tool_result_block(index, tool_use_id, name, content),
            CanonicalStreamEvent::UnknownEvent(_) => Ok(Vec::new()),
            CanonicalStreamEvent::Finish {
                finish_reason,
                usage,
            } => {
                if self.finished {
                    return Ok(Vec::new());
                }
                let mut out = self.ensure_started()?;
                out.extend(self.close_open_block()?);
                let mut payload = Map::new();
                payload.insert(
                    "type".to_string(),
                    Value::String("message_delta".to_string()),
                );
                payload.insert(
                    "delta".to_string(),
                    json!({
                        "stop_reason": map_openai_finish_reason_to_claude(
                            finish_reason.as_deref()
                        ),
                        "stop_sequence": Value::Null,
                    }),
                );
                let usage = usage.unwrap_or_default();
                payload.insert(
                    "usage".to_string(),
                    json!({
                        "input_tokens": usage.input_tokens,
                        "output_tokens": usage.output_tokens,
                    }),
                );
                out.extend(encode_json_sse(
                    Some("message_delta"),
                    &Value::Object(payload),
                )?);
                out.extend(encode_json_sse(
                    Some("message_stop"),
                    &json!({
                        "type": "message_stop",
                    }),
                )?);
                self.finished = true;
                Ok(out)
            }
        }
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if !self.started || self.finished {
            return Ok(Vec::new());
        }
        self.emit(CanonicalStreamFrame {
            id: self
                .message_id
                .clone()
                .unwrap_or_else(|| "msg-local-stream".to_string()),
            model: self.model.clone().unwrap_or_else(|| "unknown".to_string()),
            event: CanonicalStreamEvent::Finish {
                finish_reason: None,
                usage: None,
            },
        })
    }

    fn emit_content_part(
        &mut self,
        part: CanonicalContentPart,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = self.ensure_started()?;
        out.extend(self.close_open_block()?);
        let block_index = self.next_block_index;
        self.next_block_index += 1;
        let content_block = match part {
            CanonicalContentPart::ImageUrl(url) => {
                if let Some((media_type, data)) = parse_data_url(url.as_str()) {
                    json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data,
                        }
                    })
                } else {
                    json!({
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": url,
                        }
                    })
                }
            }
            CanonicalContentPart::File {
                file_data,
                reference,
                mime_type: _,
                filename: _,
            } => {
                if let Some(file_data) = file_data {
                    if let Some((media_type, data)) = parse_data_url(file_data.as_str()) {
                        json!({
                            "type": "document",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": data,
                            }
                        })
                    } else {
                        json!({
                            "type": "text",
                            "text": "[File]",
                        })
                    }
                } else if let Some(reference) = reference {
                    json!({
                        "type": "document",
                        "source": {
                            "type": "url",
                            "url": reference,
                        }
                    })
                } else {
                    json!({
                        "type": "text",
                        "text": "[File]",
                    })
                }
            }
            CanonicalContentPart::Audio { data, format } => json!({
                "type": "document",
                "source": {
                    "type": "base64",
                    "media_type": format!("audio/{format}"),
                    "data": data,
                }
            }),
        };
        out.extend(encode_json_sse(
            Some("content_block_start"),
            &json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": content_block,
            }),
        )?);
        out.extend(encode_json_sse(
            Some("content_block_stop"),
            &json!({
                "type": "content_block_stop",
                "index": block_index,
            }),
        )?);
        Ok(out)
    }
}

fn merge_claude_usage(mut current: CanonicalUsage, next: CanonicalUsage) -> CanonicalUsage {
    if next.input_tokens > 0 {
        current.input_tokens = next.input_tokens;
    }
    if next.output_tokens > 0 {
        current.output_tokens = next.output_tokens;
    }
    if next.cache_creation_tokens > 0 {
        current.cache_creation_tokens = next.cache_creation_tokens;
    }
    if next.cache_creation_ephemeral_5m_tokens > 0 {
        current.cache_creation_ephemeral_5m_tokens = next.cache_creation_ephemeral_5m_tokens;
    }
    if next.cache_creation_ephemeral_1h_tokens > 0 {
        current.cache_creation_ephemeral_1h_tokens = next.cache_creation_ephemeral_1h_tokens;
    }
    if next.cache_read_tokens > 0 {
        current.cache_read_tokens = next.cache_read_tokens;
    }
    if next.reasoning_tokens > 0 {
        current.reasoning_tokens = next.reasoning_tokens;
    }
    current.total_tokens = current
        .input_tokens
        .saturating_add(current.output_tokens)
        .saturating_add(current.cache_creation_tokens)
        .saturating_add(current.cache_read_tokens);
    current
}

fn canonical_content_part_from_claude_block(
    block: &Map<String, Value>,
) -> Option<CanonicalContentPart> {
    match block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "image" => {
            let source = block.get("source")?.as_object()?;
            match source.get("type")?.as_str()? {
                "base64" => {
                    let media_type = source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    let data = source
                        .get("data")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    Some(CanonicalContentPart::ImageUrl(format!(
                        "data:{media_type};base64,{data}"
                    )))
                }
                "url" => source
                    .get("url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| CanonicalContentPart::ImageUrl(value.to_string())),
                _ => None,
            }
        }
        "document" => {
            let source = block.get("source")?.as_object()?;
            match source.get("type")?.as_str()? {
                "base64" => {
                    let media_type = source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    let data = source
                        .get("data")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    if let Some(format) = media_type.strip_prefix("audio/") {
                        Some(CanonicalContentPart::Audio {
                            data: data.to_string(),
                            format: format.to_string(),
                        })
                    } else {
                        Some(CanonicalContentPart::File {
                            file_data: Some(format!("data:{media_type};base64,{data}")),
                            reference: None,
                            mime_type: Some(media_type.to_string()),
                            filename: None,
                        })
                    }
                }
                "url" => source
                    .get("url")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| CanonicalContentPart::File {
                        file_data: None,
                        reference: Some(value.to_string()),
                        mime_type: None,
                        filename: None,
                    }),
                _ => None,
            }
        }
        _ => None,
    }
}

fn parse_data_url(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let media_type = meta.strip_suffix(";base64")?;
    if media_type.trim().is_empty() || data.trim().is_empty() {
        return None;
    }
    Some((media_type.to_string(), data.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn data_line(value: Value) -> Vec<u8> {
        format!("data: {}\n", value).into_bytes()
    }

    #[test]
    fn claude_provider_state_emits_unknown_events_for_unknown_stream_types() {
        let mut state = ClaudeProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "future_event",
                    "payload": {
                        "kept": true
                    }
                })),
            )
            .expect("unknown stream event should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::UnknownEvent(ref payload)
                if payload.get("type").and_then(Value::as_str) == Some("future_event")
        )));
    }

    #[test]
    fn claude_provider_state_parses_thinking_deltas() {
        let mut state = ClaudeProviderState::default();
        let report_context = json!({});
        let _ = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_123",
                        "model": "claude-sonnet-4-5"
                    }
                })),
            )
            .expect("message_start should parse");
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "thinking_delta",
                        "thinking": "step by step"
                    }
                })),
            )
            .expect("thinking delta should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ReasoningDelta(ref text) if text == "step by step"
        )));
    }

    #[test]
    fn claude_provider_state_parses_signature_deltas() {
        let mut state = ClaudeProviderState::default();
        let report_context = json!({});
        let _ = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_123",
                        "model": "claude-sonnet-4-5"
                    }
                })),
            )
            .expect("message_start should parse");
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "signature_delta",
                        "signature": "sig_123"
                    }
                })),
            )
            .expect("signature delta should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ReasoningSignature(ref signature) if signature == "sig_123"
        )));
    }

    #[test]
    fn claude_provider_state_merges_start_and_delta_usage() {
        let mut state = ClaudeProviderState::default();
        let report_context = json!({});
        let _ = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_123",
                        "model": "claude-sonnet-4-5",
                        "usage": {
                            "input_tokens": 5,
                            "cache_creation_input_tokens": 59_573,
                            "cache_read_input_tokens": 0,
                            "output_tokens": 0,
                        },
                    },
                })),
            )
            .expect("message_start should parse");
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": "end_turn",
                    },
                    "usage": {
                        "output_tokens": 162,
                    },
                })),
            )
            .expect("message_delta should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::Finish {
                usage: Some(CanonicalUsage {
                    input_tokens: 5,
                    output_tokens: 162,
                    cache_creation_tokens: 59_573,
                    cache_read_tokens: 0,
                    ..
                }),
                ..
            }
        )));
    }

    #[test]
    fn claude_client_emitter_preserves_tool_identity_and_emits_thinking_blocks() {
        let mut emitter = ClaudeClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "msg_123".to_string(),
                model: "claude-sonnet-4-5".to_string(),
                event: CanonicalStreamEvent::Start,
            })
            .expect("start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "msg_123".to_string(),
                    model: "claude-sonnet-4-5".to_string(),
                    event: CanonicalStreamEvent::ReasoningDelta("step by step".to_string()),
                })
                .expect("reasoning should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "msg_123".to_string(),
                    model: "claude-sonnet-4-5".to_string(),
                    event: CanonicalStreamEvent::ReasoningSignature("sig_123".to_string()),
                })
                .expect("signature should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "msg_123".to_string(),
                    model: "claude-sonnet-4-5".to_string(),
                    event: CanonicalStreamEvent::ToolCallStart {
                        index: 0,
                        call_id: "toolu_1".to_string(),
                        name: "lookup".to_string(),
                    },
                })
                .expect("tool start should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "msg_123".to_string(),
                    model: "claude-sonnet-4-5".to_string(),
                    event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                        index: 0,
                        arguments: "{\"city\":\"Shanghai\"}".to_string(),
                    },
                })
                .expect("tool delta should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"type\":\"thinking\""));
        assert!(sse.contains("\"type\":\"thinking_delta\""));
        assert!(sse.contains("\"type\":\"signature_delta\""));
        assert!(sse.contains("\"signature\":\"sig_123\""));
        assert!(sse.contains("\"id\":\"toolu_1\""));
        assert!(sse.contains("\"name\":\"lookup\""));
        assert!(sse.contains("\"partial_json\":\"{\\\"city\\\":\\\"Shanghai\\\"}\""));
        assert!(sse.contains("\"usage\":{\"input_tokens\":0,\"output_tokens\":0}"));
    }

    #[test]
    fn claude_client_emitter_removes_empty_pages_from_tool_arguments() {
        let mut emitter = ClaudeClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "msg_123".to_string(),
                model: "claude-sonnet-4-5".to_string(),
                event: CanonicalStreamEvent::ToolCallStart {
                    index: 0,
                    call_id: "toolu_read".to_string(),
                    name: "Read".to_string(),
                },
            })
            .expect("tool start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "msg_123".to_string(),
                    model: "claude-sonnet-4-5".to_string(),
                    event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                        index: 0,
                        arguments: r#"{"file_path":"/tmp/a.txt","offset":1,"limit":20,"pages":""}"#
                            .to_string(),
                    },
                })
                .expect("tool delta should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/a.txt\\\",\\\"offset\\\":1,\\\"limit\\\":20}\""));
        assert!(!sse.contains("\\\"pages\\\":\\\"\\\""));
    }

    #[test]
    fn claude_client_emitter_injects_default_usage_into_finish_events() {
        let mut emitter = ClaudeClientEmitter::default();
        let bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "msg_456".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::Finish {
                    finish_reason: Some("stop".to_string()),
                    usage: None,
                },
            })
            .expect("finish should encode");

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("event: message_start"));
        assert!(sse.contains("event: message_delta"));
        assert!(sse.contains("\"stop_reason\":\"end_turn\""));
        assert!(sse.contains("\"usage\":{\"input_tokens\":0,\"output_tokens\":0}"));
    }

    #[test]
    fn claude_client_emitter_emits_image_blocks_for_media_parts() {
        let mut emitter = ClaudeClientEmitter::default();
        let bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "msg_img_123".to_string(),
                model: "claude-sonnet-4-5".to_string(),
                event: CanonicalStreamEvent::ContentPart(CanonicalContentPart::ImageUrl(
                    "data:image/png;base64,iVBORw0KGgo=".to_string(),
                )),
            })
            .expect("image should encode");

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"type\":\"image\""));
        assert!(sse.contains("\"media_type\":\"image/png\""));
        assert!(sse.contains("\"data\":\"iVBORw0KGgo=\""));
        assert!(sse.contains("event: content_block_stop"));
    }

    #[test]
    fn claude_client_emitter_emits_tool_result_blocks() {
        let mut emitter = ClaudeClientEmitter::default();
        let bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "msg_tool_result_123".to_string(),
                model: "claude-sonnet-4-5".to_string(),
                event: CanonicalStreamEvent::ToolResultDelta {
                    index: 2,
                    tool_use_id: "toolu_1".to_string(),
                    name: Some("lookup".to_string()),
                    content: "{\"ok\":true}".to_string(),
                },
            })
            .expect("tool result should encode");

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"type\":\"tool_result\""));
        assert!(sse.contains("\"tool_use_id\":\"toolu_1\""));
        assert!(sse.contains("\"name\":\"lookup\""));
        assert!(sse.contains("\"content\":\"{\\\"ok\\\":true}\""));
        assert!(sse.contains("\"canonical_index\":2"));
    }
}
