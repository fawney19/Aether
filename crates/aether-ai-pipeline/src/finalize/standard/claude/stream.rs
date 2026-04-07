use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::finalize::common::{build_generated_tool_call_id, canonicalize_tool_arguments};
use crate::finalize::sse::{encode_json_sse, map_claude_stop_reason};
use crate::finalize::standard::stream_core::common::*;
use crate::finalize::PipelineFinalizeError;

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

    pub fn push_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<Vec<CanonicalStreamFrame>, PipelineFinalizeError> {
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
                    _ => {}
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
                if block_type != "tool_use" {
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
                        usage: canonical_usage_from_claude_usage(event_object.get("usage")),
                    },
                });
                self.finished = true;
            }
            _ => {}
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

enum ClaudeOpenBlock {
    Text {
        block_index: usize,
    },
    Tool {
        tool_index: usize,
        block_index: usize,
    },
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
}

impl ClaudeClientEmitter {
    fn update_identity(&mut self, frame: &CanonicalStreamFrame) {
        self.message_id = Some(frame.id.clone());
        self.model = Some(frame.model.clone());
    }

    fn ensure_started(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
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
                }
            }),
        )
    }

    fn close_open_block(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
        let Some(open_block) = self.open_block.take() else {
            return Ok(Vec::new());
        };
        let block_index = match open_block {
            ClaudeOpenBlock::Text { block_index } => block_index,
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

    fn ensure_text_block(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
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

    fn ensure_tool_block(
        &mut self,
        tool_index: usize,
        call_id: &str,
        name: &str,
    ) -> Result<Vec<u8>, PipelineFinalizeError> {
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

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, PipelineFinalizeError> {
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
            CanonicalStreamEvent::ToolCallStart {
                index,
                call_id,
                name,
            } => {
                let mut out = self.ensure_started()?;
                out.extend(self.ensure_tool_block(index, &call_id, &name)?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments } => {
                let mut out = self.ensure_started()?;
                let call_id = format!("tool_{index}");
                out.extend(self.ensure_tool_block(index, &call_id, "unknown")?);
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
                if let Some(usage) = usage {
                    payload.insert(
                        "usage".to_string(),
                        json!({
                            "input_tokens": usage.input_tokens,
                            "output_tokens": usage.output_tokens,
                        }),
                    );
                }
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

    pub fn finish(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
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
}
