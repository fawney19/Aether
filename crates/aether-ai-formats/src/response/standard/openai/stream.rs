use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::response::common::build_generated_tool_call_id;
use crate::response::sse::{encode_done_sse, encode_json_sse};
use crate::response::standard::stream_core::common::*;
use crate::response::AiSurfaceFinalizeError;

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
    pending_finish_reason: Option<String>,
    tool_calls: BTreeMap<usize, OpenAIChatProviderToolState>,
}

#[derive(Default)]
struct OpenAIResponsesProviderToolState {
    call_id: String,
    name: String,
    arguments: String,
    started_emitted: bool,
}

#[derive(Default)]
struct OpenAIResponsesProviderToolResultState {
    content: String,
    emitted: bool,
}

#[derive(Default)]
pub struct OpenAIResponsesProviderState {
    response_id: Option<String>,
    model: Option<String>,
    started: bool,
    finished: bool,
    text: String,
    reasoning: String,
    tool_calls: BTreeMap<usize, OpenAIResponsesProviderToolState>,
    tool_results: BTreeMap<usize, OpenAIResponsesProviderToolResultState>,
    tool_index_by_key: BTreeMap<String, usize>,
    last_tool_index: Option<usize>,
}

impl OpenAIChatProviderState {
    fn finish_usage(value: Option<&Value>) -> Option<CanonicalUsage> {
        let usage_object = value?.as_object()?;
        let has_token_fields = [
            "input_tokens",
            "prompt_tokens",
            "output_tokens",
            "completion_tokens",
            "total_tokens",
        ]
        .iter()
        .any(|key| usage_object.contains_key(*key));
        if !has_token_fields {
            return None;
        }
        canonical_usage_from_openai_usage(value)
    }

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
            if let Some(usage) = Self::finish_usage(chunk_object.get("usage")) {
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);
                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: self.pending_finish_reason.take(),
                        usage: Some(usage),
                    },
                });
                self.finished = true;
            } else if chunk_object.contains_key("choices")
                || chunk_object
                    .get("object")
                    .and_then(Value::as_str)
                    .is_some_and(|object| object.contains("chat.completion"))
            {
                out.push(self.unknown_frame(report_context, value.clone()));
            }
            return Ok(out);
        };
        if chunk_choices.is_empty() {
            if let (Some(finish_reason), Some(usage)) = (
                self.pending_finish_reason.take(),
                Self::finish_usage(chunk_object.get("usage")),
            ) {
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);
                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some(finish_reason),
                        usage: Some(usage),
                    },
                });
                self.finished = true;
            }
            return Ok(out);
        }
        for chunk_choice in chunk_choices {
            let Some(choice_object) = chunk_choice.as_object() else {
                out.push(self.unknown_frame(report_context, chunk_choice.clone()));
                continue;
            };
            let finish_reason_key_present = choice_object.contains_key("finish_reason");
            let Some(delta) = choice_object.get("delta").and_then(Value::as_object) else {
                if let Some(finish_reason) = normalize_openai_finish_reason(
                    choice_object.get("finish_reason").and_then(Value::as_str),
                ) {
                    if let Some(usage) = Self::finish_usage(chunk_object.get("usage")) {
                        self.ensure_started(report_context, &mut out);
                        let (id, model) = self.identity(report_context);
                        out.push(CanonicalStreamFrame {
                            id,
                            model,
                            event: CanonicalStreamEvent::Finish {
                                finish_reason: Some(finish_reason),
                                usage: Some(usage),
                            },
                        });
                        self.finished = true;
                    } else {
                        self.pending_finish_reason = Some(finish_reason);
                    }
                } else if !finish_reason_key_present {
                    out.push(
                        self.unknown_frame(report_context, Value::Object(choice_object.clone())),
                    );
                }
                continue;
            };

            let mut recognized_delta = false;
            if delta.get("role").and_then(Value::as_str) == Some("assistant") {
                recognized_delta = true;
                self.ensure_started(report_context, &mut out);
            } else if delta.contains_key("role") {
                recognized_delta = true;
            }

            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                recognized_delta = true;
                if !content.is_empty() {
                    self.ensure_started(report_context, &mut out);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::TextDelta(content.to_string()),
                    });
                }
            } else if delta.contains_key("content") {
                recognized_delta = true;
            }
            if let Some(reasoning_content) = delta.get("reasoning_content").and_then(Value::as_str)
            {
                recognized_delta = true;
                if !reasoning_content.is_empty() {
                    self.ensure_started(report_context, &mut out);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::ReasoningDelta(reasoning_content.to_string()),
                    });
                }
            } else if delta.contains_key("reasoning_content") {
                recognized_delta = true;
            }

            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                recognized_delta = true;
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
            } else if delta.contains_key("tool_calls") {
                recognized_delta = true;
            }

            if let Some(finish_reason) = normalize_openai_finish_reason(
                choice_object.get("finish_reason").and_then(Value::as_str),
            ) {
                recognized_delta = true;
                if let Some(usage) = Self::finish_usage(chunk_object.get("usage")) {
                    self.ensure_started(report_context, &mut out);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::Finish {
                            finish_reason: Some(finish_reason),
                            usage: Some(usage),
                        },
                    });
                    self.finished = true;
                } else {
                    self.pending_finish_reason = Some(finish_reason);
                }
            }
            if !recognized_delta && !finish_reason_key_present {
                out.push(self.unknown_frame(report_context, Value::Object(choice_object.clone())));
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
                finish_reason: self.pending_finish_reason.take(),
                usage: None,
            },
        }])
    }
}

impl OpenAIResponsesProviderState {
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

    fn emit_missing_text(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        text: &str,
    ) {
        let missing = if text.starts_with(&self.text) {
            text[self.text.len()..].to_string()
        } else if self.text == text {
            String::new()
        } else {
            text.to_string()
        };
        if missing.is_empty() {
            return;
        }
        self.ensure_started(report_context, out);
        self.text.push_str(&missing);
        let (id, model) = self.identity(report_context);
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::TextDelta(missing),
        });
    }

    fn emit_missing_reasoning(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        reasoning: &str,
    ) {
        let missing = if reasoning.starts_with(&self.reasoning) {
            reasoning[self.reasoning.len()..].to_string()
        } else if self.reasoning == reasoning {
            String::new()
        } else {
            reasoning.to_string()
        };
        if missing.is_empty() {
            return;
        }
        self.ensure_started(report_context, out);
        self.reasoning.push_str(&missing);
        let (id, model) = self.identity(report_context);
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::ReasoningDelta(missing),
        });
    }

    fn emit_tool_call_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return;
        }
        self.ensure_started(report_context, out);
        let key = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let index = self.tool_index_for_key(key, output_index);
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
        if missing.is_empty() {
            return;
        }
        state.arguments.push_str(&missing);
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                index,
                arguments: missing,
            },
        });
    }

    fn emit_missing_tool_result(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        index: usize,
        tool_use_id: String,
        name: Option<String>,
        content: &str,
    ) {
        self.ensure_started(report_context, out);
        let state = self.tool_results.entry(index).or_default();
        let missing = if !state.emitted {
            content.to_string()
        } else if content.starts_with(&state.content) {
            content[state.content.len()..].to_string()
        } else if state.content == content {
            String::new()
        } else {
            content.to_string()
        };
        if missing.is_empty() && state.emitted {
            return;
        }
        state.emitted = true;
        state.content.push_str(&missing);
        let (id, model) = self.identity(report_context);
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::ToolResultDelta {
                index,
                tool_use_id,
                name,
                content: missing,
            },
        });
    }

    fn emit_tool_result_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("function_call_output") {
            return;
        }
        let tool_use_id = item
            .get("call_id")
            .or_else(|| item.get("tool_call_id"))
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("call_auto_0")
            .to_string();
        let index = self.tool_index_for_key(
            Some(format!("function_call_output:{tool_use_id}")),
            output_index,
        );
        let content = openai_tool_result_content_from_value(
            item.get("output")
                .or_else(|| item.get("content"))
                .or_else(|| item.get("delta")),
        );
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned);
        self.emit_missing_tool_result(report_context, out, index, tool_use_id, name, &content);
    }

    fn emit_message_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            return;
        }
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
            if content.get("type").and_then(Value::as_str) == Some("output_text") {
                if let Some(text) = content.get("text").and_then(Value::as_str) {
                    completed_text.push_str(text);
                }
            }
        }
        if !completed_text.is_empty() {
            self.emit_missing_text(report_context, out, &completed_text);
        }
    }

    fn emit_reasoning_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            return;
        }
        let mut completed_reasoning = String::new();
        for raw_summary in item
            .get("summary")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(summary) = raw_summary.as_object() else {
                continue;
            };
            if summary.get("type").and_then(Value::as_str) == Some("summary_text") {
                if let Some(text) = summary.get("text").and_then(Value::as_str) {
                    completed_reasoning.push_str(text);
                }
            }
        }
        if !completed_reasoning.is_empty() {
            self.emit_missing_reasoning(report_context, out, &completed_reasoning);
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
            "response.created" | "response.in_progress" => {
                self.ensure_started(report_context, &mut out);
            }
            "response.output_text.delta" | "response.outtext.delta" => {
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
            "response.content_part.added" | "response.content_part.done" => {
                if let Some(part) = value.get("part").and_then(Value::as_object) {
                    if part.get("type").and_then(Value::as_str) == Some("output_text") {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                self.emit_missing_text(report_context, &mut out, text);
                            }
                        }
                    }
                }
            }
            "response.reasoning_summary_part.added" | "response.reasoning_summary_part.done" => {
                if let Some(part) = value.get("part").and_then(Value::as_object) {
                    if part.get("type").and_then(Value::as_str) == Some("summary_text") {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                self.emit_missing_reasoning(report_context, &mut out, text);
                            }
                        }
                    }
                }
            }
            "response.output_text.done" => {
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("part")
                            .and_then(Value::as_object)
                            .and_then(|part| part.get("text"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    self.emit_missing_text(report_context, &mut out, text);
                }
            }
            "response.reasoning_summary_text.delta" => {
                let piece = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !piece.is_empty() {
                    self.ensure_started(report_context, &mut out);
                    self.reasoning.push_str(piece);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::ReasoningDelta(piece.to_string()),
                    });
                }
            }
            "response.reasoning_summary_text.done" => {
                let text = value
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("part")
                            .and_then(Value::as_object)
                            .and_then(|part| part.get("text"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    self.emit_missing_reasoning(report_context, &mut out, text);
                }
            }
            "response.output_item.added" => {
                let Some(item) = value.get("item").and_then(Value::as_object) else {
                    return Ok(out);
                };
                let output_index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "function_call" => {
                        self.emit_tool_call_item(report_context, &mut out, item, output_index);
                    }
                    "function_call_output" => {
                        self.emit_tool_result_item(report_context, &mut out, item, output_index);
                    }
                    "message" => {
                        self.emit_message_item(report_context, &mut out, item);
                    }
                    "reasoning" => {
                        self.emit_reasoning_item(report_context, &mut out, item);
                    }
                    _ => {
                        out.push(self.unknown_frame(report_context, Value::Object(item.clone())));
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
                state.call_id = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or(state.call_id.as_str())
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
            "response.function_call_arguments.done" => {
                let arguments = value
                    .get("arguments")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("item")
                            .and_then(Value::as_object)
                            .and_then(|item| item.get("arguments"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                if arguments.is_empty() {
                    return Ok(out);
                }
                self.ensure_started(report_context, &mut out);
                let key = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        value
                            .get("item")
                            .and_then(Value::as_object)
                            .and_then(|item| item.get("call_id").or_else(|| item.get("id")))
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    });
                let output_index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                let index = self.tool_index_for_key(key, output_index);
                let (id, model) = self.identity(report_context);
                let state = self.tool_calls.entry(index).or_default();
                state.call_id = value
                    .get("item_id")
                    .or_else(|| value.get("call_id"))
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("item")
                            .and_then(Value::as_object)
                            .and_then(|item| item.get("call_id").or_else(|| item.get("id")))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or(state.call_id.as_str())
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
                let missing = if arguments.starts_with(&state.arguments) {
                    arguments[state.arguments.len()..].to_string()
                } else if state.arguments == arguments {
                    String::new()
                } else {
                    arguments.to_string()
                };
                if !missing.is_empty() {
                    state.arguments.push_str(&missing);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                            index,
                            arguments: missing,
                        },
                    });
                }
            }
            "response.function_call_output.delta" | "response.function_call_output.done" => {
                let tool_use_id = value
                    .get("call_id")
                    .or_else(|| value.get("tool_call_id"))
                    .or_else(|| value.get("item_id"))
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("call_auto_0")
                    .to_string();
                let output_index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                let index = self.tool_index_for_key(
                    Some(format!("function_call_output:{tool_use_id}")),
                    output_index,
                );
                let content = openai_tool_result_content_from_value(
                    value
                        .get("delta")
                        .or_else(|| value.get("output"))
                        .or_else(|| value.get("content")),
                );
                let name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(ToOwned::to_owned);
                self.emit_missing_tool_result(
                    report_context,
                    &mut out,
                    index,
                    tool_use_id,
                    name,
                    &content,
                );
            }
            "response.output_item.done" => {
                let Some(item) = value.get("item").and_then(Value::as_object) else {
                    return Ok(out);
                };
                let output_index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                    "function_call" => {
                        self.emit_tool_call_item(report_context, &mut out, item, output_index);
                    }
                    "function_call_output" => {
                        self.emit_tool_result_item(report_context, &mut out, item, output_index);
                    }
                    "message" => {
                        self.emit_message_item(report_context, &mut out, item);
                    }
                    "reasoning" => {
                        self.emit_reasoning_item(report_context, &mut out, item);
                    }
                    _ => {
                        out.push(self.unknown_frame(report_context, Value::Object(item.clone())));
                    }
                }
            }
            "response.completed" => {
                let Some(response) = value.get("response").and_then(Value::as_object) else {
                    return Ok(out);
                };
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);

                for (output_index, raw_item) in response
                    .get("output")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let Some(item) = raw_item.as_object() else {
                        continue;
                    };
                    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
                        "message" => {
                            self.emit_message_item(report_context, &mut out, item);
                        }
                        "function_call" => {
                            self.emit_tool_call_item(
                                report_context,
                                &mut out,
                                item,
                                Some(output_index),
                            );
                        }
                        "function_call_output" => {
                            self.emit_tool_result_item(
                                report_context,
                                &mut out,
                                item,
                                Some(output_index),
                            );
                        }
                        "reasoning" => {
                            self.emit_reasoning_item(report_context, &mut out, item);
                        }
                        _ => {
                            out.push(
                                self.unknown_frame(report_context, Value::Object(item.clone())),
                            );
                        }
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

#[derive(Clone, Default)]
struct OpenAIResponsesClientToolState {
    call_id: String,
    name: String,
    arguments: String,
    output_index: Option<usize>,
}

#[derive(Clone, Default)]
struct OpenAIResponsesClientToolResultState {
    tool_use_id: String,
    name: Option<String>,
    content: String,
    output_index: Option<usize>,
    item_started: bool,
}

#[derive(Default)]
pub struct OpenAIResponsesClientEmitter {
    response_id: Option<String>,
    model: Option<String>,
    message_item_id: Option<String>,
    reasoning_item_id: Option<String>,
    started: bool,
    finished: bool,
    sequence_number: u64,
    next_output_index: usize,
    reasoning_item_started: bool,
    reasoning_part_started: bool,
    reasoning_output_index: Option<usize>,
    text_item_started: bool,
    text_part_started: bool,
    message_output_index: Option<usize>,
    text: String,
    reasoning: String,
    tool_calls: BTreeMap<usize, OpenAIResponsesClientToolState>,
    tool_results: BTreeMap<usize, OpenAIResponsesClientToolResultState>,
}

impl OpenAIChatClientEmitter {
    fn update_identity(&mut self, frame: &CanonicalStreamFrame) {
        self.response_id = Some(frame.id.clone());
        self.model = Some(frame.model.clone());
    }

    fn ensure_started(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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
            CanonicalStreamEvent::ReasoningDelta(text) => {
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
                                "reasoning_content": text,
                            },
                            "finish_reason": Value::Null
                        }]
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningSignature(_) => Ok(Vec::new()),
            CanonicalStreamEvent::ContentPart(part) => {
                let placeholder = openai_stream_placeholder_for_content_part(&part);
                let mut out = self.ensure_started()?;
                out.extend(encode_json_sse(
                    None,
                    &build_openai_chat_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        placeholder,
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
            CanonicalStreamEvent::ToolResultDelta {
                tool_use_id,
                name,
                content,
                ..
            } => {
                let mut out = self.ensure_started()?;
                let mut delta = Map::new();
                delta.insert("role".to_string(), Value::String("tool".to_string()));
                delta.insert("tool_call_id".to_string(), Value::String(tool_use_id));
                if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
                    delta.insert("name".to_string(), Value::String(name));
                }
                delta.insert("content".to_string(), Value::String(content));
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
                            "delta": Value::Object(delta),
                            "finish_reason": Value::Null
                        }]
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::UnknownEvent(_) => Ok(Vec::new()),
            CanonicalStreamEvent::Finish {
                finish_reason,
                usage,
            } => {
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
                if let Some(usage) = usage {
                    out.extend(encode_json_sse(
                        None,
                        &build_openai_chat_usage_chunk(
                            self.response_id
                                .as_deref()
                                .unwrap_or("chatcmpl-local-stream"),
                            self.model.as_deref().unwrap_or("unknown"),
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.total_tokens,
                            usage.reasoning_tokens,
                        ),
                    )?);
                }
                out.extend(encode_done_sse());
                self.finished = true;
                Ok(out)
            }
        }
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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

impl OpenAIResponsesClientEmitter {
    fn response_id(&self) -> &str {
        self.response_id.as_deref().unwrap_or("resp-local-stream")
    }

    fn model(&self) -> &str {
        self.model.as_deref().unwrap_or("unknown")
    }

    fn message_item_id(&self) -> String {
        self.message_item_id
            .clone()
            .unwrap_or_else(|| format!("{}_msg", self.response_id()))
    }

    fn reasoning_item_id(&self) -> String {
        self.reasoning_item_id
            .clone()
            .unwrap_or_else(|| format!("{}_rs_0", self.response_id()))
    }

    fn ensure_message_item_id(&mut self) -> String {
        if self.message_item_id.is_none() {
            self.message_item_id = Some(format!("{}_msg", self.response_id()));
        }
        self.message_item_id()
    }

    fn ensure_reasoning_item_id(&mut self) -> String {
        if self.reasoning_item_id.is_none() {
            self.reasoning_item_id = Some(format!("{}_rs_0", self.response_id()));
        }
        self.reasoning_item_id()
    }

    fn in_progress_response(&self) -> Value {
        json!({
            "id": self.response_id(),
            "object": "response",
            "model": self.model(),
            "status": "in_progress",
            "output": [],
        })
    }

    fn allocate_output_index(&mut self) -> usize {
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        output_index
    }

    fn next_sequence_number(&mut self) -> u64 {
        self.sequence_number += 1;
        self.sequence_number
    }

    fn encode_response_event(
        &mut self,
        event: &str,
        mut payload: Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if let Some(object) = payload.as_object_mut() {
            object.insert(
                "sequence_number".to_string(),
                Value::from(self.next_sequence_number()),
            );
        }
        encode_json_sse(Some(event), &payload)
    }

    fn update_identity(&mut self, frame: &CanonicalStreamFrame) {
        self.response_id = Some(frame.id.clone().replace("chatcmpl", "resp"));
        self.model = Some(frame.model.clone());
    }

    fn ensure_started(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.started {
            return Ok(Vec::new());
        }
        self.started = true;
        let mut out = self.encode_response_event(
            "response.created",
            json!({
                "type": "response.created",
                "response": self.in_progress_response(),
            }),
        )?;
        out.extend(self.encode_response_event(
            "response.in_progress",
            json!({
                "type": "response.in_progress",
                "response": self.in_progress_response(),
            }),
        )?);
        Ok(out)
    }

    fn ensure_reasoning_output_index(&mut self) -> usize {
        if let Some(output_index) = self.reasoning_output_index {
            return output_index;
        }
        let output_index = self.allocate_output_index();
        self.reasoning_output_index = Some(output_index);
        output_index
    }

    fn ensure_message_output_index(&mut self) -> usize {
        if let Some(output_index) = self.message_output_index {
            return output_index;
        }
        let output_index = self.allocate_output_index();
        self.message_output_index = Some(output_index);
        output_index
    }

    fn ensure_tool_output_index(&mut self, index: usize) -> usize {
        if let Some(output_index) = self
            .tool_calls
            .get(&index)
            .and_then(|state| state.output_index)
        {
            return output_index;
        }
        let output_index = self.allocate_output_index();
        self.tool_calls.entry(index).or_default().output_index = Some(output_index);
        output_index
    }

    fn ensure_tool_result_output_index(&mut self, index: usize) -> usize {
        if let Some(output_index) = self
            .tool_results
            .get(&index)
            .and_then(|state| state.output_index)
        {
            return output_index;
        }
        let output_index = self.allocate_output_index();
        self.tool_results.entry(index).or_default().output_index = Some(output_index);
        output_index
    }

    fn ensure_reasoning_item_started(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = self.ensure_started()?;
        let output_index = self.ensure_reasoning_output_index();
        let item_id = self.ensure_reasoning_item_id();
        if !self.reasoning_item_started {
            out.extend(self.encode_response_event(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item": {
                        "type": "reasoning",
                        "id": item_id.clone(),
                        "summary": [],
                    }
                }),
            )?);
            self.reasoning_item_started = true;
        }
        if !self.reasoning_part_started {
            out.extend(self.encode_response_event(
                "response.reasoning_summary_part.added",
                json!({
                    "type": "response.reasoning_summary_part.added",
                    "response_id": self.response_id(),
                    "item_id": item_id,
                    "output_index": output_index,
                    "summary_index": 0,
                    "part": {
                        "type": "summary_text",
                        "text": "",
                    }
                }),
            )?);
            self.reasoning_part_started = true;
        }
        Ok(out)
    }

    fn ensure_text_item_started(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = self.ensure_started()?;
        let output_index = self.ensure_message_output_index();
        let item_id = self.ensure_message_item_id();
        if !self.text_item_started {
            out.extend(self.encode_response_event(
                "response.output_item.added",
                json!({
                    "type": "response.output_item.added",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item": {
                        "type": "message",
                        "id": item_id.clone(),
                        "status": "in_progress",
                        "role": "assistant",
                        "content": [],
                    }
                }),
            )?);
            self.text_item_started = true;
        }
        if !self.text_part_started {
            out.extend(self.encode_response_event(
                "response.content_part.added",
                json!({
                    "type": "response.content_part.added",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item_id": item_id,
                    "content_index": 0,
                    "part": {
                        "type": "output_text",
                        "text": "",
                        "annotations": [],
                    }
                }),
            )?);
            self.text_part_started = true;
        }
        Ok(out)
    }

    fn finish_text_item(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if !self.text_item_started {
            return Ok(Vec::new());
        }
        let item_id = self.message_item_id();
        let output_index = self.message_output_index.unwrap_or(0);
        let mut out = Vec::new();
        if self.text_part_started {
            out.extend(self.encode_response_event(
                "response.output_text.done",
                json!({
                    "type": "response.output_text.done",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item_id": item_id.clone(),
                    "content_index": 0,
                    "text": self.text.as_str(),
                }),
            )?);
            out.extend(self.encode_response_event(
                "response.content_part.done",
                json!({
                    "type": "response.content_part.done",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item_id": item_id.clone(),
                    "content_index": 0,
                    "part": {
                        "type": "output_text",
                        "text": self.text.as_str(),
                        "annotations": [],
                    }
                }),
            )?);
        }
        out.extend(self.encode_response_event(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "response_id": self.response_id(),
                "output_index": output_index,
                "item": {
                    "type": "message",
                    "id": item_id,
                    "status": "completed",
                    "role": "assistant",
                    "content": [{
                        "type": "output_text",
                        "text": self.text.as_str(),
                        "annotations": [],
                    }],
                }
            }),
        )?);
        Ok(out)
    }

    fn finish_reasoning_item(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if !self.reasoning_item_started {
            return Ok(Vec::new());
        }
        let output_index = self.reasoning_output_index.unwrap_or(0);
        let item_id = self.reasoning_item_id();
        let mut out = Vec::new();
        if self.reasoning_part_started {
            out.extend(self.encode_response_event(
                "response.reasoning_summary_text.done",
                json!({
                    "type": "response.reasoning_summary_text.done",
                    "response_id": self.response_id(),
                    "item_id": item_id.clone(),
                    "output_index": output_index,
                    "summary_index": 0,
                    "text": self.reasoning.as_str(),
                }),
            )?);
            out.extend(self.encode_response_event(
                "response.reasoning_summary_part.done",
                json!({
                    "type": "response.reasoning_summary_part.done",
                    "response_id": self.response_id(),
                    "item_id": item_id.clone(),
                    "output_index": output_index,
                    "summary_index": 0,
                    "part": {
                        "type": "summary_text",
                        "text": self.reasoning.as_str(),
                    }
                }),
            )?);
        }
        out.extend(self.encode_response_event(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "response_id": self.response_id(),
                "output_index": output_index,
                "item": {
                    "type": "reasoning",
                    "id": item_id,
                    "summary": [{
                        "type": "summary_text",
                        "text": self.reasoning.as_str(),
                    }],
                }
            }),
        )?);
        Ok(out)
    }

    fn finish_tool_items(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = Vec::new();
        let indices = self.tool_calls.keys().copied().collect::<Vec<_>>();
        for index in indices {
            let output_index = self.ensure_tool_output_index(index);
            let state = self.tool_calls.get(&index).cloned().unwrap_or_default();
            let item_id = if state.call_id.is_empty() {
                build_generated_tool_call_id(index)
            } else {
                state.call_id.clone()
            };
            let name = if state.name.is_empty() {
                "unknown".to_string()
            } else {
                state.name.clone()
            };
            out.extend(self.encode_response_event(
                "response.function_call_arguments.done",
                json!({
                    "type": "response.function_call_arguments.done",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item_id": item_id.clone(),
                    "call_id": item_id.clone(),
                    "arguments": state.arguments.as_str(),
                }),
            )?);
            out.extend(self.encode_response_event(
                "response.output_item.done",
                json!({
                    "type": "response.output_item.done",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item": {
                        "type": "function_call",
                        "id": item_id.clone(),
                        "call_id": item_id,
                        "name": name,
                        "arguments": state.arguments.as_str(),
                        "status": "completed",
                    }
                }),
            )?);
        }
        Ok(out)
    }

    fn finish_tool_result_items(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = Vec::new();
        let indices = self.tool_results.keys().copied().collect::<Vec<_>>();
        for index in indices {
            let output_index = self.ensure_tool_result_output_index(index);
            let state = self.tool_results.get(&index).cloned().unwrap_or_default();
            let item_id = if state.tool_use_id.is_empty() {
                build_generated_tool_call_id(index)
            } else {
                state.tool_use_id.clone()
            };
            let mut item = Map::new();
            item.insert(
                "type".to_string(),
                Value::String("function_call_output".to_string()),
            );
            item.insert("id".to_string(), Value::String(format!("{item_id}_output")));
            item.insert("call_id".to_string(), Value::String(item_id));
            if let Some(name) = state.name.filter(|value| !value.trim().is_empty()) {
                item.insert("name".to_string(), Value::String(name));
            }
            item.insert("output".to_string(), Value::String(state.content));
            out.extend(self.encode_response_event(
                "response.output_item.done",
                json!({
                    "type": "response.output_item.done",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item": Value::Object(item),
                }),
            )?);
        }
        Ok(out)
    }

    fn completed_response(&self, usage: CanonicalUsage) -> Value {
        let mut ordered_output = Vec::new();
        if !self.reasoning.trim().is_empty() {
            ordered_output.push((
                self.reasoning_output_index.unwrap_or(0),
                json!({
                    "type": "reasoning",
                    "id": self.reasoning_item_id(),
                    "status": "completed",
                    "summary": [{
                        "type": "summary_text",
                        "text": self.reasoning.as_str(),
                    }]
                }),
            ));
        }
        if self.text_item_started || !self.text.is_empty() {
            ordered_output.push((
                self.message_output_index.unwrap_or(0),
                json!({
                    "type": "message",
                    "id": self.message_item_id(),
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": self.text.as_str(),
                        "annotations": [],
                    }],
                }),
            ));
        }
        for (index, state) in &self.tool_calls {
            if let Some(output_index) = state.output_index {
                ordered_output.push((
                    output_index,
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
                        "status": "completed",
                    }),
                ));
            }
        }
        for (index, state) in &self.tool_results {
            if let Some(output_index) = state.output_index {
                let item_id = if state.tool_use_id.is_empty() {
                    build_generated_tool_call_id(*index)
                } else {
                    state.tool_use_id.clone()
                };
                let mut item = Map::new();
                item.insert(
                    "type".to_string(),
                    Value::String("function_call_output".to_string()),
                );
                item.insert("id".to_string(), Value::String(format!("{item_id}_output")));
                item.insert("call_id".to_string(), Value::String(item_id));
                if let Some(name) = state
                    .name
                    .as_ref()
                    .filter(|value| !value.trim().is_empty())
                    .cloned()
                {
                    item.insert("name".to_string(), Value::String(name));
                }
                item.insert("output".to_string(), Value::String(state.content.clone()));
                ordered_output.push((output_index, Value::Object(item)));
            }
        }
        ordered_output.sort_by_key(|(output_index, _)| *output_index);

        let mut usage_payload = Map::new();
        usage_payload.insert("input_tokens".to_string(), Value::from(usage.input_tokens));
        usage_payload.insert(
            "output_tokens".to_string(),
            Value::from(usage.output_tokens),
        );
        usage_payload.insert("total_tokens".to_string(), Value::from(usage.total_tokens));
        if usage.reasoning_tokens > 0 {
            usage_payload.insert(
                "output_tokens_details".to_string(),
                json!({ "reasoning_tokens": usage.reasoning_tokens }),
            );
        }

        json!({
            "id": self.response_id(),
            "object": "response",
            "status": "completed",
            "model": self.model(),
            "output": ordered_output
                .into_iter()
                .map(|(_, item)| item)
                .collect::<Vec<_>>(),
            "usage": usage_payload,
        })
    }

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.update_identity(&frame);
        match frame.event {
            CanonicalStreamEvent::Start => self.ensure_started(),
            CanonicalStreamEvent::TextDelta(text) => {
                let mut out = self.ensure_text_item_started()?;
                self.text.push_str(&text);
                out.extend(self.encode_response_event(
                    "response.output_text.delta",
                    json!({
                        "type": "response.output_text.delta",
                        "response_id": self.response_id(),
                        "output_index": self.message_output_index.unwrap_or(0),
                        "item_id": self.message_item_id(),
                        "content_index": 0,
                        "delta": text,
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningDelta(text) => {
                let mut out = self.ensure_reasoning_item_started()?;
                self.reasoning.push_str(&text);
                out.extend(self.encode_response_event(
                    "response.reasoning_summary_text.delta",
                    json!({
                        "type": "response.reasoning_summary_text.delta",
                        "response_id": self.response_id(),
                        "item_id": self.reasoning_item_id(),
                        "output_index": self.reasoning_output_index.unwrap_or(0),
                        "summary_index": 0,
                        "delta": text,
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningSignature(_) => Ok(Vec::new()),
            CanonicalStreamEvent::ContentPart(part) => {
                let placeholder = openai_stream_placeholder_for_content_part(&part);
                let mut out = self.ensure_text_item_started()?;
                self.text.push_str(&placeholder);
                out.extend(self.encode_response_event(
                    "response.output_text.delta",
                    json!({
                        "type": "response.output_text.delta",
                        "response_id": self.response_id(),
                        "output_index": self.message_output_index.unwrap_or(0),
                        "item_id": self.message_item_id(),
                        "content_index": 0,
                        "delta": placeholder,
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
                let output_index = self.ensure_tool_output_index(index);
                let response_id = self.response_id().to_string();
                let state = self.tool_calls.entry(index).or_default();
                state.call_id = call_id.clone();
                state.name = name.clone();
                let emitted_call_id = state.call_id.clone();
                let emitted_name = state.name.clone();
                out.extend(self.encode_response_event(
                    "response.output_item.added",
                    json!({
                        "type": "response.output_item.added",
                        "response_id": response_id,
                        "output_index": output_index,
                        "item": {
                            "type": "function_call",
                            "id": call_id,
                            "call_id": emitted_call_id,
                            "name": emitted_name,
                            "arguments": "",
                            "status": "in_progress",
                        }
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments } => {
                let mut out = self.ensure_started()?;
                let output_index = self.ensure_tool_output_index(index);
                let response_id = self.response_id().to_string();
                let state = self.tool_calls.entry(index).or_default();
                state.arguments.push_str(&arguments);
                let item_id = if state.call_id.is_empty() {
                    build_generated_tool_call_id(index)
                } else {
                    state.call_id.clone()
                };
                out.extend(self.encode_response_event(
                    "response.function_call_arguments.delta",
                    json!({
                        "type": "response.function_call_arguments.delta",
                        "response_id": response_id,
                        "output_index": output_index,
                        "item_id": item_id.clone(),
                        "call_id": item_id,
                        "delta": arguments,
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolResultDelta {
                index,
                tool_use_id,
                name,
                content,
            } => {
                let mut out = self.ensure_started()?;
                let output_index = self.ensure_tool_result_output_index(index);
                let response_id = self.response_id().to_string();
                let state = self.tool_results.entry(index).or_default();
                if state.tool_use_id.is_empty() {
                    state.tool_use_id = tool_use_id.clone();
                }
                if name.is_some() {
                    state.name = name.clone();
                }
                let emitted_tool_use_id = if state.tool_use_id.is_empty() {
                    tool_use_id
                } else {
                    state.tool_use_id.clone()
                };
                if !state.item_started {
                    let mut item = Map::new();
                    item.insert(
                        "type".to_string(),
                        Value::String("function_call_output".to_string()),
                    );
                    item.insert(
                        "id".to_string(),
                        Value::String(format!("{emitted_tool_use_id}_output")),
                    );
                    item.insert(
                        "call_id".to_string(),
                        Value::String(emitted_tool_use_id.clone()),
                    );
                    if let Some(name) = state
                        .name
                        .as_ref()
                        .filter(|value| !value.trim().is_empty())
                        .cloned()
                    {
                        item.insert("name".to_string(), Value::String(name));
                    }
                    item.insert("output".to_string(), Value::String(String::new()));
                    out.extend(self.encode_response_event(
                        "response.output_item.added",
                        json!({
                            "type": "response.output_item.added",
                            "response_id": response_id,
                            "output_index": output_index,
                            "item": Value::Object(item),
                        }),
                    )?);
                    self.tool_results.entry(index).or_default().item_started = true;
                }
                self.tool_results
                    .entry(index)
                    .or_default()
                    .content
                    .push_str(&content);
                if !content.is_empty() {
                    out.extend(self.encode_response_event(
                        "response.function_call_output.delta",
                        json!({
                            "type": "response.function_call_output.delta",
                            "response_id": self.response_id(),
                            "output_index": output_index,
                            "item_id": format!("{emitted_tool_use_id}_output"),
                            "call_id": emitted_tool_use_id,
                            "delta": content,
                        }),
                    )?);
                }
                Ok(out)
            }
            CanonicalStreamEvent::UnknownEvent(_) => Ok(Vec::new()),
            CanonicalStreamEvent::Finish { usage, .. } => {
                if self.finished {
                    return Ok(Vec::new());
                }
                let mut out = self.ensure_started()?;
                out.extend(self.finish_reasoning_item()?);
                out.extend(self.finish_text_item()?);
                out.extend(self.finish_tool_items()?);
                out.extend(self.finish_tool_result_items()?);
                let usage = usage.unwrap_or_default();
                out.extend(self.encode_response_event(
                    "response.completed",
                    json!({
                        "type": "response.completed",
                        "response": self.completed_response(usage),
                    }),
                )?);
                self.finished = true;
                Ok(out)
            }
        }
    }

    pub fn emit_error(&mut self, error_body: Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let Some(error) = error_body.get("error").cloned() else {
            return Ok(Vec::new());
        };
        self.finished = true;
        self.encode_response_event(
            "response.failed",
            json!({
                "type": "response.failed",
                "error": error,
            }),
        )
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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

fn openai_stream_placeholder_for_content_part(part: &CanonicalContentPart) -> String {
    match part {
        CanonicalContentPart::ImageUrl(url) => {
            if url.starts_with("data:") {
                "[Image]".to_string()
            } else {
                format!("[Image: {url}]")
            }
        }
        CanonicalContentPart::File {
            reference,
            mime_type,
            filename,
            ..
        } => reference
            .as_ref()
            .map(|value| format!("[File: {value}]"))
            .or_else(|| filename.as_ref().map(|value| format!("[File: {value}]")))
            .or_else(|| mime_type.as_ref().map(|value| format!("[File: {value}]")))
            .unwrap_or_else(|| "[File]".to_string()),
        CanonicalContentPart::Audio { format, .. } => format!("[Audio: {format}]"),
    }
}

fn openai_tool_result_content_from_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_line(value: Value) -> Vec<u8> {
        format!("data: {}\n", value).into_bytes()
    }

    fn response_sequence_numbers(sse: &str) -> Vec<u64> {
        let mut sequence_numbers = Vec::new();
        for payload in sse.lines().filter_map(|line| line.strip_prefix("data: ")) {
            let Ok(value) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            let Some(ty) = value.get("type").and_then(Value::as_str) else {
                continue;
            };
            if !ty.starts_with("response.") {
                continue;
            }
            if let Some(sequence_number) = value.get("sequence_number").and_then(Value::as_u64) {
                sequence_numbers.push(sequence_number);
            }
        }
        sequence_numbers
    }

    #[test]
    fn openai_chat_provider_state_emits_unknown_events_for_unrecognized_deltas() {
        let mut state = OpenAIChatProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "id": "chatcmpl_unknown_123",
                    "model": "gpt-5.4",
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "future_delta_type": {
                                "payload": true
                            }
                        }
                    }]
                })),
            )
            .expect("unknown delta should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::UnknownEvent(ref payload)
                if payload.get("delta")
                    .and_then(|delta| delta.get("future_delta_type"))
                    .is_some()
        )));
    }

    #[test]
    fn openai_responses_provider_state_emits_unknown_events_for_unknown_response_types() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.future.delta",
                    "response": {
                        "id": "resp_unknown_123",
                        "model": "gpt-5.4"
                    },
                    "delta": {
                        "payload": true
                    }
                })),
            )
            .expect("unknown response event should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::UnknownEvent(ref payload)
                if payload.get("type").and_then(Value::as_str) == Some("response.future.delta")
        )));
    }

    #[test]
    fn openai_usage_derives_missing_input_tokens_from_total() {
        let usage = canonical_usage_from_openai_usage(Some(&json!({
            "output_tokens": 177,
            "output_tokens_details": {
                "reasoning_tokens": 7,
            },
            "total_tokens": 20_612,
            "input_tokens_details": {
                "cached_tokens": 19_840,
            },
        })))
        .expect("usage should parse");

        assert_eq!(usage.input_tokens, 20_435);
        assert_eq!(usage.output_tokens, 177);
        assert_eq!(usage.cache_read_tokens, 19_840);
        assert_eq!(usage.reasoning_tokens, 7);
    }

    #[test]
    fn openai_chat_provider_state_accepts_usage_only_terminal_chunk() {
        let mut state = OpenAIChatProviderState::default();
        let report_context = json!({});
        let _ = state
            .push_line(
                &report_context,
                data_line(json!({
                    "id": "chatcmpl_123",
                    "object": "chat.completion.chunk",
                    "model": "gpt-5.4",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop",
                    }],
                })),
            )
            .expect("finish chunk should parse");
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "usage": {
                        "input_tokens": 26,
                        "input_tokens_details": {
                            "cached_tokens": 0,
                        },
                        "output_tokens": 144,
                        "output_tokens_details": {
                            "reasoning_tokens": 10,
                        },
                        "total_tokens": 170,
                    },
                })),
            )
            .expect("usage-only chunk should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::Finish {
                finish_reason: Some(ref reason),
                usage: Some(CanonicalUsage {
                    input_tokens: 26,
                    output_tokens: 144,
                    cache_read_tokens: 0,
                    reasoning_tokens: 10,
                    ..
                }),
            } if reason == "stop"
        )));
    }

    #[test]
    fn openai_responses_provider_state_extracts_response_completed_usage() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_063494bbd780be940169eb8191c4ec8191916347b2080805ee",
                        "object": "response",
                        "model": "gpt-5.5",
                        "status": "completed",
                        "output": [],
                        "usage": {
                            "input_tokens": 26,
                            "input_tokens_details": {
                                "cached_tokens": 0,
                            },
                            "output_tokens": 137,
                            "output_tokens_details": {
                                "reasoning_tokens": 0,
                            },
                            "total_tokens": 163,
                        },
                    },
                    "sequence_number": 139,
                })),
            )
            .expect("completed event should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::Finish {
                usage: Some(CanonicalUsage {
                    input_tokens: 26,
                    output_tokens: 137,
                    cache_read_tokens: 0,
                    ..
                }),
                ..
            }
        )));
    }

    #[test]
    fn openai_responses_client_emitter_emits_doc_like_text_events() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let start = CanonicalStreamFrame {
            id: "chatcmpl_stream_123".to_string(),
            model: "gpt-5.4".to_string(),
            event: CanonicalStreamEvent::Start,
        };
        let text = CanonicalStreamFrame {
            id: "chatcmpl_stream_123".to_string(),
            model: "gpt-5.4".to_string(),
            event: CanonicalStreamEvent::TextDelta("Hello".to_string()),
        };
        let finish = CanonicalStreamFrame {
            id: "chatcmpl_stream_123".to_string(),
            model: "gpt-5.4".to_string(),
            event: CanonicalStreamEvent::Finish {
                finish_reason: Some("stop".to_string()),
                usage: Some(CanonicalUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: 3,
                    ..CanonicalUsage::default()
                }),
            },
        };

        let mut bytes = emitter.emit(start).expect("start should encode");
        bytes.extend(emitter.emit(text).expect("text should encode"));
        bytes.extend(emitter.emit(finish).expect("finish should encode"));

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("event: response.created\n"));
        assert!(sse.contains("event: response.in_progress\n"));
        assert!(sse.contains("event: response.output_item.added\n"));
        assert!(sse.contains("event: response.content_part.added\n"));
        assert!(sse.contains("event: response.output_text.delta\n"));
        assert!(sse.contains("event: response.output_text.done\n"));
        assert!(sse.contains("event: response.content_part.done\n"));
        assert!(sse.contains("event: response.output_item.done\n"));
        assert!(sse.contains("event: response.completed\n"));
        assert!(sse.contains("\"response_id\":\"resp_stream_123\""));
        assert!(sse.contains("\"item_id\":\"resp_stream_123_msg\""));
        assert!(sse.contains("\"text\":\"Hello\""));
        assert_eq!(response_sequence_numbers(&sse), (1..=9).collect::<Vec<_>>());
    }

    #[test]
    fn openai_responses_client_emitter_keeps_text_item_id_stable_after_text_started() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "msg_first".to_string(),
                model: "claude-haiku-4-5-20251001".to_string(),
                event: CanonicalStreamEvent::TextDelta("Hel".to_string()),
            })
            .expect("first text should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "msg_second".to_string(),
                    model: "claude-haiku-4-5-20251001".to_string(),
                    event: CanonicalStreamEvent::TextDelta("lo".to_string()),
                })
                .expect("second text should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"item_id\":\"msg_first_msg\""));
        assert!(!sse.contains("\"item_id\":\"msg_second_msg\""));
    }

    #[test]
    fn openai_responses_provider_state_accepts_done_events_without_deltas() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.created",
                        "response": {
                            "id": "resp_123",
                            "model": "gpt-5.4",
                        }
                    })),
                )
                .expect("created should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.output_text.done",
                        "response_id": "resp_123",
                        "output_index": 0,
                        "item_id": "resp_123_msg",
                        "content_index": 0,
                        "text": "Hello",
                    })),
                )
                .expect("text done should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.function_call_arguments.done",
                        "response_id": "resp_123",
                        "output_index": 1,
                        "item_id": "call_123",
                        "call_id": "call_123",
                        "arguments": "{\"city\":\"SF\"}",
                    })),
                )
                .expect("arguments done should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.completed",
                        "response": {
                            "id": "resp_123",
                            "object": "response",
                            "model": "gpt-5.4",
                            "status": "completed",
                            "output": [{
                                "type": "message",
                                "id": "resp_123_msg",
                                "role": "assistant",
                                "status": "completed",
                                "content": [{
                                    "type": "output_text",
                                    "text": "Hello",
                                    "annotations": [],
                                }]
                            }, {
                                "type": "function_call",
                                "id": "call_123",
                                "call_id": "call_123",
                                "name": "get_weather",
                                "arguments": "{\"city\":\"SF\"}",
                            }],
                            "usage": {
                                "input_tokens": 1,
                                "output_tokens": 2,
                                "total_tokens": 3,
                            }
                        }
                    })),
                )
                .expect("completed should parse"),
        );

        assert!(matches!(
            frames.first().map(|frame| &frame.event),
            Some(CanonicalStreamEvent::Start)
        ));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::TextDelta(ref text) if text == "Hello"
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallStart { ref call_id, .. } if call_id == "call_123"
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallArgumentsDelta { ref arguments, .. }
                if arguments == "{\"city\":\"SF\"}"
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::Finish {
                finish_reason: Some(ref reason),
                usage: Some(CanonicalUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: 3,
                    ..
                }),
            } if reason == "tool_calls"
        )));
    }

    #[test]
    fn openai_responses_provider_state_parses_function_call_output_as_tool_result() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.created",
                        "response": {
                            "id": "resp_123",
                            "model": "gpt-5.4",
                        }
                    })),
                )
                .expect("created should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.function_call_output.done",
                        "response_id": "resp_123",
                        "output_index": 2,
                        "call_id": "call_123",
                        "name": "lookup",
                        "output": {"ok": true},
                    })),
                )
                .expect("tool result should parse"),
        );

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolResultDelta {
                index: 2,
                ref tool_use_id,
                name: Some(ref name),
                ref content,
            } if tool_use_id == "call_123" && name == "lookup" && content == "{\"ok\":true}"
        )));
    }

    #[test]
    fn openai_responses_client_emitter_emits_function_call_output_events() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_123".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::ToolResultDelta {
                    index: 1,
                    tool_use_id: "call_123".to_string(),
                    name: Some("lookup".to_string()),
                    content: "{\"ok\":true}".to_string(),
                },
            })
            .expect("tool result should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_123".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some("stop".to_string()),
                        usage: None,
                    },
                })
                .expect("finish should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("event: response.function_call_output.delta\n"));
        assert!(sse.contains("\"type\":\"function_call_output\""));
        assert!(sse.contains("\"call_id\":\"call_123\""));
        assert!(sse.contains("\"output\":\"{\\\"ok\\\":true}\""));
    }

    #[test]
    fn openai_responses_provider_state_accepts_legacy_outtext_delta_alias() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.created",
                        "response": {
                            "id": "resp_legacy_123",
                            "model": "gpt-5.4",
                        }
                    })),
                )
                .expect("created should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.outtext.delta",
                        "response_id": "resp_legacy_123",
                        "output_index": 0,
                        "item_id": "resp_legacy_123_msg",
                        "content_index": 0,
                        "delta": "Hello from legacy alias",
                    })),
                )
                .expect("legacy text delta should parse"),
        );

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::TextDelta(ref text) if text == "Hello from legacy alias"
        )));
    }

    #[test]
    fn openai_chat_client_emitter_emits_reasoning_content_chunks() {
        let mut emitter = OpenAIChatClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "chatcmpl_123".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::Start,
            })
            .expect("start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "chatcmpl_123".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::ReasoningDelta("because".to_string()),
                })
                .expect("reasoning should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"reasoning_content\":\"because\""));
    }

    #[test]
    fn openai_chat_client_emitter_renders_image_parts_as_placeholder() {
        let mut emitter = OpenAIChatClientEmitter::default();
        let bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "chatcmpl_img_123".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::ContentPart(CanonicalContentPart::ImageUrl(
                    "data:image/png;base64,iVBORw0KGgo=".to_string(),
                )),
            })
            .expect("image should encode");

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("[Image]"));
    }

    #[test]
    fn openai_chat_client_emitter_emits_usage_only_final_chunk() {
        let mut emitter = OpenAIChatClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "chatcmpl_789".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::Start,
            })
            .expect("start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "chatcmpl_789".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::TextDelta("Hello".to_string()),
                })
                .expect("text should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "chatcmpl_789".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some("stop".to_string()),
                        usage: Some(CanonicalUsage {
                            input_tokens: 1,
                            output_tokens: 2,
                            total_tokens: 3,
                            reasoning_tokens: 1,
                            ..CanonicalUsage::default()
                        }),
                    },
                })
                .expect("finish should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"finish_reason\":\"stop\""));
        assert!(sse.contains("\"choices\":[]"));
        assert!(sse.contains("\"prompt_tokens\":1"));
        assert!(sse.contains("\"completion_tokens\":2"));
        assert!(sse.contains("\"completion_tokens_details\":{\"reasoning_tokens\":1}"));
        assert!(sse.contains("\"total_tokens\":3"));
        assert!(sse.contains("data: [DONE]\n\n"));
    }

    #[test]
    fn openai_chat_provider_state_accepts_usage_only_final_chunk() {
        let mut state = OpenAIChatProviderState::default();
        let report_context = json!({});
        let mut frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "id": "chatcmpl_456",
                    "object": "chat.completion.chunk",
                    "model": "gpt-5.4",
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "role": "assistant",
                            "content": "Hello",
                        },
                        "finish_reason": Value::Null,
                    }]
                })),
            )
            .expect("first chunk should parse");
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "id": "chatcmpl_456",
                        "object": "chat.completion.chunk",
                        "model": "gpt-5.4",
                        "choices": [{
                            "index": 0,
                            "delta": {},
                            "finish_reason": "stop",
                        }],
                        "usage": {},
                    })),
                )
                .expect("stop chunk should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "id": "chatcmpl_456",
                        "object": "chat.completion.chunk",
                        "model": "gpt-5.4",
                        "choices": [],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 2,
                            "total_tokens": 3,
                        }
                    })),
                )
                .expect("usage chunk should parse"),
        );

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::Finish {
                finish_reason: Some(ref reason),
                usage: Some(CanonicalUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    total_tokens: 3,
                    ..
                }),
            } if reason == "stop"
        )));
    }

    #[test]
    fn openai_responses_client_emitter_includes_reasoning_in_completed_response() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_123".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::Start,
            })
            .expect("start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_123".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::ReasoningDelta("because".to_string()),
                })
                .expect("reasoning should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_123".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some("stop".to_string()),
                        usage: Some(CanonicalUsage {
                            input_tokens: 1,
                            output_tokens: 2,
                            total_tokens: 3,
                            reasoning_tokens: 1,
                            ..CanonicalUsage::default()
                        }),
                    },
                })
                .expect("finish should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("\"type\":\"reasoning\""));
        assert!(sse.contains("\"text\":\"because\""));
        assert!(sse.contains("\"output_tokens_details\":{\"reasoning_tokens\":1}"));
    }

    #[test]
    fn openai_responses_client_emitter_emits_doc_like_reasoning_events() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_456".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::Start,
            })
            .expect("start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_456".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::ReasoningDelta("step".to_string()),
                })
                .expect("reasoning should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_456".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some("stop".to_string()),
                        usage: None,
                    },
                })
                .expect("finish should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("event: response.reasoning_summary_part.added\n"));
        assert!(sse.contains("event: response.reasoning_summary_text.delta\n"));
        assert!(sse.contains("event: response.reasoning_summary_text.done\n"));
        assert!(sse.contains("event: response.reasoning_summary_part.done\n"));
        assert!(sse.contains("\"item_id\":\"resp_456_rs_0\""));
        assert!(sse.contains("\"type\":\"reasoning\""));
        assert_eq!(response_sequence_numbers(&sse), (1..=9).collect::<Vec<_>>());
    }

    #[test]
    fn openai_responses_client_emitter_emits_failed_event_with_sequence_number() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_err_123".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::Start,
            })
            .expect("start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_err_123".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::TextDelta("Hi".to_string()),
                })
                .expect("text should encode"),
        );
        bytes.extend(
            emitter
                .emit_error(json!({
                    "error": {
                        "message": "boom",
                        "type": "server_error",
                        "code": "internal",
                    }
                }))
                .expect("error should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("event: response.failed\n"));
        assert!(sse.contains("\"message\":\"boom\""));
        assert!(!sse.contains("event: response.completed\n"));
        assert_eq!(response_sequence_numbers(&sse), (1..=6).collect::<Vec<_>>());
    }

    #[test]
    fn openai_responses_provider_state_accepts_reasoning_summary_events() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.created",
                        "response": {
                            "id": "resp_456",
                            "model": "gpt-5.4",
                        }
                    })),
                )
                .expect("created should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.reasoning_summary_part.added",
                        "response_id": "resp_456",
                        "item_id": "resp_456_rs_0",
                        "output_index": 0,
                        "summary_index": 0,
                        "part": {
                            "type": "summary_text",
                            "text": "",
                        }
                    })),
                )
                .expect("part added should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.reasoning_summary_text.delta",
                        "response_id": "resp_456",
                        "item_id": "resp_456_rs_0",
                        "output_index": 0,
                        "summary_index": 0,
                        "delta": "step",
                    })),
                )
                .expect("delta should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.reasoning_summary_text.done",
                        "response_id": "resp_456",
                        "item_id": "resp_456_rs_0",
                        "output_index": 0,
                        "summary_index": 0,
                        "text": "step",
                    })),
                )
                .expect("done should parse"),
        );

        let reasoning = frames
            .iter()
            .filter(|frame| matches!(frame.event, CanonicalStreamEvent::ReasoningDelta(_)))
            .collect::<Vec<_>>();
        assert_eq!(reasoning.len(), 1);
        assert!(matches!(
            reasoning[0].event,
            CanonicalStreamEvent::ReasoningDelta(ref text) if text == "step"
        ));
    }
}
