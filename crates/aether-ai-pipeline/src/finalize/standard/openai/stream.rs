use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::conversion::response::build_openai_cli_response;
use crate::finalize::common::build_generated_tool_call_id;
use crate::finalize::sse::{encode_done_sse, encode_json_sse};
use crate::finalize::standard::stream_core::common::*;
use crate::finalize::PipelineFinalizeError;

#[derive(Default)]
struct OpenAIChatProviderToolState {
    id: Option<String>,
    name: Option<String>,
    started_emitted: bool,
}

#[derive(Default)]
pub struct OpenAIChatProviderState {
    response_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
    tool_calls: BTreeMap<usize, OpenAIChatProviderToolState>,
}

#[derive(Default)]
struct OpenAICliProviderToolState {
    call_id: String,
    name: String,
    arguments: String,
    started_emitted: bool,
}

#[derive(Default)]
pub struct OpenAICliProviderState {
    response_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
    text: String,
    tool_calls: BTreeMap<usize, OpenAICliProviderToolState>,
    tool_index_by_key: BTreeMap<String, usize>,
    last_tool_index: Option<usize>,
}

impl OpenAIChatProviderState {
    fn identity(&self, report_context: &Value) -> (String, String) {
        resolve_identity(
            self.response_id.as_deref(),
            self.model.as_deref(),
            report_context,
            "chatcmpl-local-stream",
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
        let Some(chunk_object) = value.as_object() else {
            return Ok(Vec::new());
        };
        self.response_id = chunk_object
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| self.response_id.clone());
        self.model = chunk_object
            .get("model")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| self.model.clone());

        let mut out = Vec::new();
        let Some(chunk_choices) = chunk_object.get("choices").and_then(Value::as_array) else {
            return Ok(out);
        };
        for chunk_choice in chunk_choices {
            let Some(choice_object) = chunk_choice.as_object() else {
                continue;
            };
            let Some(delta) = choice_object.get("delta").and_then(Value::as_object) else {
                if let Some(finish_reason) = normalize_openai_finish_reason(
                    choice_object.get("finish_reason").and_then(Value::as_str),
                ) {
                    self.ensure_started(report_context, &mut out);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::Finish {
                            finish_reason: Some(finish_reason),
                            usage: canonical_usage_from_openai_usage(chunk_object.get("usage")),
                        },
                    });
                    self.finished = true;
                }
                continue;
            };

            if delta.get("role").and_then(Value::as_str) == Some("assistant") {
                self.ensure_started(report_context, &mut out);
            }

            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                if !content.is_empty() {
                    self.ensure_started(report_context, &mut out);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::TextDelta(content.to_string()),
                    });
                }
            }

            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);
                for tool_call in tool_calls {
                    let Some(tool_call_object) = tool_call.as_object() else {
                        continue;
                    };
                    let index = tool_call_object
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|value| value as usize)
                        .unwrap_or(0);
                    let state = self.tool_calls.entry(index).or_default();
                    if let Some(call_id) = tool_call_object.get("id").and_then(Value::as_str) {
                        state.id = Some(call_id.to_string());
                    }
                    if let Some(function) =
                        tool_call_object.get("function").and_then(Value::as_object)
                    {
                        if let Some(name) = function.get("name").and_then(Value::as_str) {
                            state.name = Some(name.to_string());
                        }
                        if !state.started_emitted && (state.id.is_some() || state.name.is_some()) {
                            out.push(CanonicalStreamFrame {
                                id: id.clone(),
                                model: model.clone(),
                                event: CanonicalStreamEvent::ToolCallStart {
                                    index,
                                    call_id: state
                                        .id
                                        .clone()
                                        .unwrap_or_else(|| build_generated_tool_call_id(index)),
                                    name: state
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| "unknown".to_string()),
                                },
                            });
                            state.started_emitted = true;
                        }
                        if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                            if !arguments.is_empty() {
                                if !state.started_emitted {
                                    out.push(CanonicalStreamFrame {
                                        id: id.clone(),
                                        model: model.clone(),
                                        event: CanonicalStreamEvent::ToolCallStart {
                                            index,
                                            call_id: state.id.clone().unwrap_or_else(|| {
                                                build_generated_tool_call_id(index)
                                            }),
                                            name: state
                                                .name
                                                .clone()
                                                .unwrap_or_else(|| "unknown".to_string()),
                                        },
                                    });
                                    state.started_emitted = true;
                                }
                                out.push(CanonicalStreamFrame {
                                    id: id.clone(),
                                    model: model.clone(),
                                    event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                                        index,
                                        arguments: arguments.to_string(),
                                    },
                                });
                            }
                        }
                    }
                }
            }

            if let Some(finish_reason) = normalize_openai_finish_reason(
                choice_object.get("finish_reason").and_then(Value::as_str),
            ) {
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);
                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some(finish_reason),
                        usage: canonical_usage_from_openai_usage(chunk_object.get("usage")),
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

impl OpenAICliProviderState {
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

    fn tool_index_for_key(&mut self, key: Option<String>, output_index: Option<usize>) -> usize {
        if let Some(output_index) = output_index {
            if let Some(key) = key.as_ref() {
                self.tool_index_by_key
                    .entry(key.clone())
                    .or_insert(output_index);
            }
            self.last_tool_index = Some(output_index);
            return output_index;
        }
        if let Some(key) = key.as_ref() {
            if let Some(index) = self.tool_index_by_key.get(key).copied() {
                self.last_tool_index = Some(index);
                return index;
            }
        }
        let index = self.last_tool_index.unwrap_or(self.tool_calls.len());
        if let Some(key) = key {
            self.tool_index_by_key.insert(key, index);
        }
        self.last_tool_index = Some(index);
        index
    }

    pub fn push_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<Vec<CanonicalStreamFrame>, PipelineFinalizeError> {
        let Some(value) = decode_json_data_line(&line) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if let Some(response) = value.get("response").and_then(Value::as_object) {
            self.response_id = response
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| self.response_id.clone());
            self.model = response
                .get("model")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| self.model.clone());
        }

        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.created" => {
                self.ensure_started(report_context, &mut out);
            }
            "response.output_text.delta" => {
                let piece = match value.get("delta") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Object(delta)) => delta
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    _ => String::new(),
                };
                if !piece.is_empty() {
                    self.ensure_started(report_context, &mut out);
                    self.text.push_str(&piece);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::TextDelta(piece),
                    });
                }
            }
            "response.output_item.added" => {
                let Some(item) = value.get("item").and_then(Value::as_object) else {
                    return Ok(out);
                };
                if item.get("type").and_then(Value::as_str) != Some("function_call") {
                    return Ok(out);
                }
                self.ensure_started(report_context, &mut out);
                let key = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let output_index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                let index = self.tool_index_for_key(key.clone(), output_index);
                let (id, model) = self.identity(report_context);
                let state = self.tool_calls.entry(index).or_default();
                state.call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(state.call_id.as_str())
                    .to_string();
                state.name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(state.name.as_str())
                    .to_string();
                if !state.started_emitted {
                    out.push(CanonicalStreamFrame {
                        id: id.clone(),
                        model: model.clone(),
                        event: CanonicalStreamEvent::ToolCallStart {
                            index,
                            call_id: if state.call_id.is_empty() {
                                build_generated_tool_call_id(index)
                            } else {
                                state.call_id.clone()
                            },
                            name: if state.name.is_empty() {
                                "unknown".to_string()
                            } else {
                                state.name.clone()
                            },
                        },
                    });
                    state.started_emitted = true;
                }
                if let Some(arguments) = item.get("arguments").and_then(Value::as_str) {
                    if !arguments.is_empty() {
                        state.arguments.push_str(arguments);
                        out.push(CanonicalStreamFrame {
                            id,
                            model,
                            event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                                index,
                                arguments: arguments.to_string(),
                            },
                        });
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if delta.is_empty() {
                    return Ok(out);
                }
                self.ensure_started(report_context, &mut out);
                let key = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let output_index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                let index = self.tool_index_for_key(key, output_index);
                let (id, model) = self.identity(report_context);
                let state = self.tool_calls.entry(index).or_default();
                if !state.started_emitted {
                    out.push(CanonicalStreamFrame {
                        id: id.clone(),
                        model: model.clone(),
                        event: CanonicalStreamEvent::ToolCallStart {
                            index,
                            call_id: if state.call_id.is_empty() {
                                build_generated_tool_call_id(index)
                            } else {
                                state.call_id.clone()
                            },
                            name: if state.name.is_empty() {
                                "unknown".to_string()
                            } else {
                                state.name.clone()
                            },
                        },
                    });
                    state.started_emitted = true;
                }
                state.arguments.push_str(delta);
                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                        index,
                        arguments: delta.to_string(),
                    },
                });
            }
            "response.completed" => {
                let Some(response) = value.get("response").and_then(Value::as_object) else {
                    return Ok(out);
                };
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);

                for raw_item in response
                    .get("output")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(item) = raw_item.as_object() else {
                        continue;
                    };
                    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                        "message" => {
                            let mut completed_text = String::new();
                            for raw_content in item
                                .get("content")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                            {
                                let Some(content) = raw_content.as_object() else {
                                    continue;
                                };
                                if content.get("type").and_then(Value::as_str)
                                    == Some("output_text")
                                {
                                    if let Some(text) = content.get("text").and_then(Value::as_str)
                                    {
                                        completed_text.push_str(text);
                                    }
                                }
                            }
                            let missing = if completed_text.starts_with(&self.text) {
                                completed_text[self.text.len()..].to_string()
                            } else if self.text == completed_text {
                                String::new()
                            } else {
                                completed_text.clone()
                            };
                            if !missing.is_empty() {
                                self.text.push_str(&missing);
                                out.push(CanonicalStreamFrame {
                                    id: id.clone(),
                                    model: model.clone(),
                                    event: CanonicalStreamEvent::TextDelta(missing),
                                });
                            }
                        }
                        "function_call" => {
                            let key = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned);
                            let index = self.tool_index_for_key(key, None);
                            let state = self.tool_calls.entry(index).or_default();
                            state.call_id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or(state.call_id.as_str())
                                .to_string();
                            state.name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(state.name.as_str())
                                .to_string();
                            if !state.started_emitted {
                                out.push(CanonicalStreamFrame {
                                    id: id.clone(),
                                    model: model.clone(),
                                    event: CanonicalStreamEvent::ToolCallStart {
                                        index,
                                        call_id: if state.call_id.is_empty() {
                                            build_generated_tool_call_id(index)
                                        } else {
                                            state.call_id.clone()
                                        },
                                        name: if state.name.is_empty() {
                                            "unknown".to_string()
                                        } else {
                                            state.name.clone()
                                        },
                                    },
                                });
                                state.started_emitted = true;
                            }
                            let completed_arguments = item
                                .get("arguments")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let missing = if completed_arguments.starts_with(&state.arguments) {
                                completed_arguments[state.arguments.len()..].to_string()
                            } else if state.arguments == completed_arguments {
                                String::new()
                            } else {
                                completed_arguments.clone()
                            };
                            if !missing.is_empty() {
                                state.arguments.push_str(&missing);
                                out.push(CanonicalStreamFrame {
                                    id: id.clone(),
                                    model: model.clone(),
                                    event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                                        index,
                                        arguments: missing,
                                    },
                                });
                            }
                        }
                        _ => {}
                    }
                }

                let finish_reason = if self.tool_calls.is_empty() {
                    Some("stop".to_string())
                } else {
                    Some("tool_calls".to_string())
                };
                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::Finish {
                        finish_reason,
                        usage: canonical_usage_from_openai_usage(response.get("usage")),
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
        let finish_reason = if self.tool_calls.is_empty() {
            Some("stop".to_string())
        } else {
            Some("tool_calls".to_string())
        };
        Ok(vec![CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::Finish {
                finish_reason,
                usage: None,
            },
        }])
    }
}

#[derive(Default)]
pub struct OpenAIChatClientEmitter {
    response_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
}

#[derive(Default)]
struct OpenAICliClientToolState {
    call_id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
pub struct OpenAICliClientEmitter {
    response_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
    text: String,
    tool_calls: BTreeMap<usize, OpenAICliClientToolState>,
}

impl OpenAIChatClientEmitter {
    fn update_identity(&mut self, frame: &CanonicalStreamFrame) {
        self.response_id = Some(frame.id.clone());
        self.model = Some(frame.model.clone());
    }

    fn ensure_started(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
        if self.started {
            return Ok(Vec::new());
        }
        self.started = true;
        encode_json_sse(
            None,
            &build_openai_chat_role_chunk(
                self.response_id
                    .as_deref()
                    .unwrap_or("chatcmpl-local-stream"),
                self.model.as_deref().unwrap_or("unknown"),
            ),
        )
    }

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, PipelineFinalizeError> {
        self.update_identity(&frame);
        match frame.event {
            CanonicalStreamEvent::Start => self.ensure_started(),
            CanonicalStreamEvent::TextDelta(text) => {
                let mut out = self.ensure_started()?;
                out.extend(encode_json_sse(
                    None,
                    &build_openai_chat_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        text,
                        None,
                        None,
                    ),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolCallStart {
                index,
                call_id,
                name,
            } => {
                let mut out = self.ensure_started()?;
                out.extend(encode_json_sse(
                    None,
                    &build_openai_chat_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        String::new(),
                        Some(vec![json!({
                            "index": index,
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": "",
                            }
                        })]),
                        None,
                    ),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments } => {
                let mut out = self.ensure_started()?;
                out.extend(encode_json_sse(
                    None,
                    &json!({
                        "id": self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        "object": "chat.completion.chunk",
                        "model": self.model.as_deref().unwrap_or("unknown"),
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": index,
                                    "function": {
                                        "arguments": arguments,
                                    }
                                }]
                            },
                            "finish_reason": Value::Null
                        }]
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::Finish { finish_reason, .. } => {
                if self.finished {
                    return Ok(Vec::new());
                }
                let mut out = self.ensure_started()?;
                out.extend(encode_json_sse(
                    None,
                    &build_openai_chat_finish_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        finish_reason.as_deref(),
                    ),
                )?);
                out.extend(encode_done_sse());
                self.finished = true;
                Ok(out)
            }
        }
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
        if !self.started || self.finished {
            return Ok(Vec::new());
        }
        let out = encode_json_sse(
            None,
            &build_openai_chat_finish_chunk(
                self.response_id
                    .as_deref()
                    .unwrap_or("chatcmpl-local-stream"),
                self.model.as_deref().unwrap_or("unknown"),
                None,
            ),
        )?;
        self.finished = true;
        let mut bytes = out;
        bytes.extend(encode_done_sse());
        Ok(bytes)
    }
}

impl OpenAICliClientEmitter {
    fn update_identity(&mut self, frame: &CanonicalStreamFrame) {
        self.response_id = Some(frame.id.clone().replace("chatcmpl", "resp"));
        self.model = Some(frame.model.clone());
    }

    fn ensure_started(&mut self) -> Result<Vec<u8>, PipelineFinalizeError> {
        if self.started {
            return Ok(Vec::new());
        }
        self.started = true;
        encode_json_sse(
            Some("response.created"),
            &json!({
                "type": "response.created",
                "response": {
                    "id": self.response_id.as_deref().unwrap_or("resp-local-stream"),
                    "object": "response",
                    "model": self.model.as_deref().unwrap_or("unknown"),
                    "status": "in_progress",
                    "output": [],
                }
            }),
        )
    }

    fn function_output_index(&self, index: usize) -> usize {
        if self.text.is_empty() {
            index
        } else {
            index + 1
        }
    }

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, PipelineFinalizeError> {
        self.update_identity(&frame);
        match frame.event {
            CanonicalStreamEvent::Start => self.ensure_started(),
            CanonicalStreamEvent::TextDelta(text) => {
                let mut out = self.ensure_started()?;
                self.text.push_str(&text);
                out.extend(encode_json_sse(
                    Some("response.output_text.delta"),
                    &json!({
                        "type": "response.output_text.delta",
                        "output_index": 0,
                        "content_index": 0,
                        "delta": text,
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
                let output_index = self.function_output_index(index);
                let state = self.tool_calls.entry(index).or_default();
                state.call_id = call_id.clone();
                state.name = name.clone();
                out.extend(encode_json_sse(
                    Some("response.output_item.added"),
                    &json!({
                        "type": "response.output_item.added",
                        "output_index": output_index,
                        "item": {
                            "type": "function_call",
                            "id": call_id,
                            "call_id": state.call_id,
                            "name": state.name,
                            "arguments": "",
                        }
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments } => {
                let mut out = self.ensure_started()?;
                let output_index = self.function_output_index(index);
                let state = self.tool_calls.entry(index).or_default();
                state.arguments.push_str(&arguments);
                out.extend(encode_json_sse(
                    Some("response.function_call_arguments.delta"),
                    &json!({
                        "type": "response.function_call_arguments.delta",
                        "output_index": output_index,
                        "item_id": if state.call_id.is_empty() {
                            build_generated_tool_call_id(index)
                        } else {
                            state.call_id.clone()
                        },
                        "delta": arguments,
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::Finish { usage, .. } => {
                if self.finished {
                    return Ok(Vec::new());
                }
                let mut out = self.ensure_started()?;
                let usage = usage.unwrap_or_default();
                let function_calls = self
                    .tool_calls
                    .iter()
                    .map(|(index, state)| {
                        json!({
                            "type": "function_call",
                            "id": if state.call_id.is_empty() {
                                build_generated_tool_call_id(*index)
                            } else {
                                state.call_id.clone()
                            },
                            "call_id": if state.call_id.is_empty() {
                                build_generated_tool_call_id(*index)
                            } else {
                                state.call_id.clone()
                            },
                            "name": if state.name.is_empty() {
                                "unknown".to_string()
                            } else {
                                state.name.clone()
                            },
                            "arguments": state.arguments.clone(),
                        })
                    })
                    .collect::<Vec<_>>();
                out.extend(encode_json_sse(
                    Some("response.completed"),
                    &json!({
                        "type": "response.completed",
                        "response": build_openai_cli_response(
                            self.response_id.as_deref().unwrap_or("resp-local-stream"),
                            self.model.as_deref().unwrap_or("unknown"),
                            &self.text,
                            function_calls,
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.total_tokens,
                        ),
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
                .response_id
                .clone()
                .unwrap_or_else(|| "resp-local-stream".to_string()),
            model: self.model.clone().unwrap_or_else(|| "unknown".to_string()),
            event: CanonicalStreamEvent::Finish {
                finish_reason: None,
                usage: None,
            },
        })
    }
}
