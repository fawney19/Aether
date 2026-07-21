use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::formats::openai::responses::response::{
    ensure_modern_openai_responses_response_fields, openai_responses_current_timestamp,
};
use crate::formats::shared::response::build_generated_tool_call_id;
use crate::formats::shared::sse::{encode_done_sse, encode_json_sse};
use crate::formats::shared::stream_core::common::*;
use crate::formats::shared::AiSurfaceFinalizeError;

fn normalize_openai_service_tier(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 64)
        .map(str::to_ascii_lowercase)
}

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
    actual_service_tier: Option<String>,
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
    emitted_arguments_len: usize,
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
    actual_service_tier: Option<String>,
    started: bool,
    finished: bool,
    text_parts: BTreeMap<String, String>,
    reasoning: String,
    reasoning_parts: BTreeMap<usize, String>,
    tool_calls: BTreeMap<usize, OpenAIResponsesProviderToolState>,
    tool_results: BTreeMap<usize, OpenAIResponsesProviderToolResultState>,
    tool_index_by_key: BTreeMap<String, usize>,
    image_item_keys: BTreeSet<String>,
    opaque_completed_item_keys: BTreeSet<String>,
    last_tool_index: Option<usize>,
}

impl OpenAIChatProviderState {
    pub(crate) fn actual_service_tier(&self) -> Option<&str> {
        self.actual_service_tier.as_deref()
    }

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
        if let Some(service_tier) =
            normalize_openai_service_tier(chunk_object.get("service_tier").and_then(Value::as_str))
        {
            self.actual_service_tier = Some(service_tier);
        }

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
    pub(crate) fn actual_service_tier(&self) -> Option<&str> {
        self.actual_service_tier.as_deref()
    }

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

    fn text_part_key_from_event(value: &Value) -> String {
        let item_key = value
            .get("output_index")
            .and_then(Value::as_u64)
            .map(|value| format!("output:{value}"))
            .or_else(|| {
                value
                    .get("item_id")
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .map(|value| format!("item:{value}"))
            })
            .unwrap_or_else(|| "output:default".to_string());
        let content_index = value
            .get("content_index")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        format!("{item_key}:content:{content_index}")
    }

    fn text_part_key_from_message_item(
        output_index: Option<usize>,
        item: &Map<String, Value>,
        content_index: usize,
    ) -> String {
        let item_key = output_index
            .map(|value| format!("output:{value}"))
            .or_else(|| {
                item.get("id")
                    .and_then(Value::as_str)
                    .map(|value| format!("item:{value}"))
            })
            .unwrap_or_else(|| "output:default".to_string());
        format!("{item_key}:content:{content_index}")
    }

    fn emit_text_delta(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        key: String,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        self.text_parts.entry(key).or_default().push_str(text);
        self.ensure_started(report_context, out);
        let (id, model) = self.identity(report_context);
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::TextDelta(text.to_string()),
        });
    }

    fn emit_missing_text(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        key: String,
        text: &str,
    ) {
        let missing = {
            let current = self.text_parts.entry(key).or_default();
            let missing = if text.starts_with(current.as_str()) {
                text[current.len()..].to_string()
            } else if current.as_str() == text || current.starts_with(text) {
                String::new()
            } else {
                text.to_string()
            };
            if !missing.is_empty() {
                current.push_str(&missing);
            }
            missing
        };
        if missing.is_empty() {
            return;
        }
        self.ensure_started(report_context, out);
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

    fn emit_missing_reasoning_part_text(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        summary_index: usize,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        let missing = {
            let current = self.reasoning_parts.entry(summary_index).or_default();
            let missing = if text.starts_with(current.as_str()) {
                text[current.len()..].to_string()
            } else if current.as_str() == text {
                String::new()
            } else if current.is_empty() {
                text.to_string()
            } else {
                String::new()
            };
            if !missing.is_empty() {
                current.push_str(&missing);
            }
            missing
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

    fn emit_ready_tool_call(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        index: usize,
    ) {
        let (id, model) = self.identity(report_context);
        let Some(state) = self.tool_calls.get_mut(&index) else {
            return;
        };
        if state.name.is_empty() {
            return;
        }
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
                    name: state.name.clone(),
                },
            });
            state.started_emitted = true;
        }
        if state.emitted_arguments_len > state.arguments.len() {
            state.emitted_arguments_len = 0;
        }
        let pending = state
            .arguments
            .get(state.emitted_arguments_len..)
            .unwrap_or_default()
            .to_string();
        if pending.is_empty() {
            return;
        }
        state.emitted_arguments_len = state.arguments.len();
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                index,
                arguments: pending,
            },
        });
    }

    fn merge_tool_call_arguments(state: &mut OpenAIResponsesProviderToolState, arguments: &str) {
        if arguments.is_empty() {
            return;
        }
        if arguments.starts_with(&state.arguments) {
            state
                .arguments
                .push_str(&arguments[state.arguments.len()..]);
        } else if state.arguments != arguments {
            if state.emitted_arguments_len == 0 {
                state.arguments = arguments.to_string();
            } else {
                state.arguments.push_str(arguments);
            }
        }
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
        // Chat-backed Responses implementations may attach transport-only metadata to
        // function-call items. It does not change the executable tool call, and the
        // sync Responses aggregator retains the original item verbatim. Treat these
        // sidecars as recognized while continuing to fail closed for semantic fields
        // (for example `caller`) that the canonical tool-call events cannot represent.
        const EXECUTION_FIELDS: &[&str] = &["type", "id", "call_id", "status", "name", "arguments"];
        let has_chat_metadata_passthrough =
            item.contains_key("internal_chat_message_metadata_passthrough");
        let chat_metadata_target_supported = report_context
            .get("client_api_format")
            .and_then(Value::as_str)
            .map(aether_ai_formats::normalize_api_format_alias)
            .is_none_or(|client_api_format| {
                matches!(
                    client_api_format.as_str(),
                    "openai:chat" | "openai:responses" | "openai:responses:compact"
                )
            });
        if item.keys().any(|field| {
            !EXECUTION_FIELDS.contains(&field.as_str())
                && !(chat_metadata_target_supported
                    && (field == "internal_chat_message_metadata_passthrough"
                        || (field == "metadata" && has_chat_metadata_passthrough)))
        }) {
            out.push(self.unknown_frame(report_context, Value::Object(item.clone())));
            return;
        }
        self.ensure_started(report_context, out);
        let key = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let index = self.tool_index_for_key(key, output_index);
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
        let completed_arguments = item
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Self::merge_tool_call_arguments(state, &completed_arguments);
        self.emit_ready_tool_call(report_context, out, index);
    }

    fn emit_custom_tool_call_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("custom_tool_call") {
            return;
        }
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("custom_tool")
            .to_string();
        let arguments = tool_arguments_from_maybe_json_string(
            item.get("input").or_else(|| item.get("arguments")),
            "input",
        );
        self.emit_generic_tool_call_item(report_context, out, item, output_index, name, arguments);
    }

    fn emit_shell_tool_call_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
    ) {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let name = match item_type {
            "local_shell_call" => "local_shell",
            "shell_call" => "shell",
            _ => return,
        };
        let arguments = tool_arguments_from_named_fields(
            item,
            &[
                "action",
                "environment",
                "status",
                "created_by",
                "max_output_length",
            ],
        );
        self.emit_generic_tool_call_item(
            report_context,
            out,
            item,
            output_index,
            name.to_string(),
            arguments,
        );
    }

    fn emit_apply_patch_tool_call_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("apply_patch_call") {
            return;
        }
        let arguments = tool_arguments_from_named_fields(item, &["operation", "status"]);
        self.emit_generic_tool_call_item(
            report_context,
            out,
            item,
            output_index,
            "apply_patch".to_string(),
            arguments,
        );
    }

    fn emit_computer_tool_call_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("computer_call") {
            return;
        }
        let arguments = tool_arguments_from_named_fields(
            item,
            &["action", "actions", "pending_safety_checks", "status"],
        );
        self.emit_generic_tool_call_item(
            report_context,
            out,
            item,
            output_index,
            "computer".to_string(),
            arguments,
        );
    }

    fn emit_generic_tool_call_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
        name: String,
        arguments: String,
    ) {
        self.ensure_started(report_context, out);
        let key = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let index = self.tool_index_for_key(key, output_index);
        let state = self.tool_calls.entry(index).or_default();
        state.call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or(state.call_id.as_str())
            .to_string();
        state.name = name;
        Self::merge_tool_call_arguments(state, &arguments);
        self.emit_ready_tool_call(report_context, out, index);
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

    fn emit_generic_tool_result_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
    ) {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let is_supported_result = matches!(
            item_type,
            "custom_tool_call_output"
                | "local_shell_call_output"
                | "shell_call_output"
                | "apply_patch_call_output"
                | "computer_call_output"
        );
        if !is_supported_result {
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
        let index =
            self.tool_index_for_key(Some(format!("{item_type}:{tool_use_id}")), output_index);
        let content = openai_tool_result_content_from_value(
            item.get("output")
                .or_else(|| item.get("content"))
                .or_else(|| item.get("delta")),
        );
        let name = match item_type {
            "local_shell_call_output" => Some("local_shell".to_string()),
            "shell_call_output" => Some("shell".to_string()),
            "apply_patch_call_output" => Some("apply_patch".to_string()),
            "computer_call_output" => Some("computer".to_string()),
            _ => item
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned),
        };
        self.emit_missing_tool_result(report_context, out, index, tool_use_id, name, &content);
    }

    fn emit_message_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            return;
        }
        for (content_index, raw_content) in item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let Some(content) = raw_content.as_object() else {
                continue;
            };
            if content.get("type").and_then(Value::as_str) == Some("output_text") {
                if let Some(text) = content.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        let key = Self::text_part_key_from_message_item(
                            output_index,
                            item,
                            content_index,
                        );
                        self.emit_missing_text(report_context, out, key, text);
                    }
                }
            }
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

    fn emit_image_generation_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
        final_item: bool,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
            return;
        }
        if !final_item
            && !item
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("completed"))
        {
            return;
        }
        let has_image_payload = item
            .get("result")
            .or_else(|| item.get("url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        if !has_image_payload {
            return;
        }
        let index = output_index.unwrap_or(self.image_item_keys.len());
        let key = item
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("image_generation_call:{index}"));
        if !self.image_item_keys.insert(key) {
            return;
        }
        self.ensure_started(report_context, out);
        let (id, model) = self.identity(report_context);
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::ImageGenerationCall {
                index,
                item: Value::Object(item.clone()),
            },
        });
    }

    fn output_item_key(item: &Map<String, Value>) -> String {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if let Some(item_id) = item.get("id").and_then(Value::as_str) {
            return format!("{item_type}:id:{item_id}");
        }
        if let Some(encrypted_content) = item.get("encrypted_content").and_then(Value::as_str) {
            return format!("{item_type}:encrypted_content:{encrypted_content}");
        }
        format!(
            "{item_type}:{}",
            serde_json::to_string(item).unwrap_or_default()
        )
    }

    fn emit_output_item(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        item: &Map<String, Value>,
        output_index: Option<usize>,
        final_item: bool,
    ) -> bool {
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "function_call" => {
                self.emit_tool_call_item(report_context, out, item, output_index);
                true
            }
            "function_call_output" => {
                self.emit_tool_result_item(report_context, out, item, output_index);
                true
            }
            "custom_tool_call" => {
                self.emit_custom_tool_call_item(report_context, out, item, output_index);
                true
            }
            "local_shell_call" | "shell_call" => {
                self.emit_shell_tool_call_item(report_context, out, item, output_index);
                true
            }
            "apply_patch_call" => {
                self.emit_apply_patch_tool_call_item(report_context, out, item, output_index);
                true
            }
            "computer_call" => {
                self.emit_computer_tool_call_item(report_context, out, item, output_index);
                true
            }
            "custom_tool_call_output"
            | "local_shell_call_output"
            | "shell_call_output"
            | "apply_patch_call_output"
            | "computer_call_output" => {
                self.emit_generic_tool_result_item(report_context, out, item, output_index);
                true
            }
            "message" => {
                self.emit_message_item(report_context, out, item, output_index);
                true
            }
            "reasoning" if final_item => {
                self.emit_reasoning_item(report_context, out, item);
                true
            }
            "reasoning" => {
                self.ensure_started(report_context, out);
                true
            }
            "image_generation_call" => {
                self.emit_image_generation_item(
                    report_context,
                    out,
                    item,
                    output_index,
                    final_item,
                );
                true
            }
            "web_search_call" | "file_search_call" | "code_interpreter_call" | "mcp_call" => {
                if !final_item {
                    self.ensure_started(report_context, out);
                }
                true
            }
            _ => false,
        }
    }

    fn emit_output_item_event(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        raw_event: &Value,
        item: &Map<String, Value>,
        output_index: Option<usize>,
        final_item: bool,
    ) {
        if self.emit_output_item(report_context, out, item, output_index, final_item) {
            return;
        }

        if item
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            out.push(self.unknown_frame(report_context, raw_event.clone()));
            return;
        }

        if final_item {
            self.opaque_completed_item_keys
                .insert(Self::output_item_key(item));
        }
        self.ensure_started(report_context, out);
        let (id, model) = self.identity(report_context);
        out.push(CanonicalStreamFrame {
            id,
            model,
            event: CanonicalStreamEvent::OpenAiResponsesOutputItem {
                output_index,
                item: Value::Object(item.clone()),
                raw_event: raw_event.clone(),
            },
        });
    }

    fn emit_response_output_items(
        &mut self,
        report_context: &Value,
        out: &mut Vec<CanonicalStreamFrame>,
        response: &Map<String, Value>,
    ) {
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
            if !self.emit_output_item(report_context, out, item, Some(output_index), true)
                && !self
                    .opaque_completed_item_keys
                    .contains(&Self::output_item_key(item))
            {
                out.push(self.unknown_frame(report_context, Value::Object(item.clone())));
            }
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
            if let Some(service_tier) =
                normalize_openai_service_tier(response.get("service_tier").and_then(Value::as_str))
            {
                self.actual_service_tier = Some(service_tier);
            }
        }

        match value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.created" | "response.in_progress" => {
                self.ensure_started(report_context, &mut out);
            }
            "response.output_text.delta" | "response.outtext.delta" => match value.get("delta") {
                Some(Value::String(piece)) if !piece.is_empty() => {
                    let key = Self::text_part_key_from_event(&value);
                    self.emit_text_delta(report_context, &mut out, key, piece);
                }
                Some(Value::Object(delta)) => {
                    if let Some(text) = delta.get("text").and_then(Value::as_str) {
                        let key = Self::text_part_key_from_event(&value);
                        self.emit_missing_text(report_context, &mut out, key, text);
                    }
                }
                _ => {}
            },
            "response.content_part.added" | "response.content_part.done" => {
                if let Some(part) = value.get("part").and_then(Value::as_object) {
                    if part.get("type").and_then(Value::as_str) == Some("output_text") {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                let key = Self::text_part_key_from_event(&value);
                                self.emit_missing_text(report_context, &mut out, key, text);
                            }
                        }
                    }
                }
            }
            "response.reasoning_summary_part.added" | "response.reasoning_summary_part.done" => {
                if let Some(part) = value.get("part").and_then(Value::as_object) {
                    if part.get("type").and_then(Value::as_str) == Some("summary_text") {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            let summary_index = value
                                .get("summary_index")
                                .and_then(Value::as_u64)
                                .map(|value| value as usize)
                                .unwrap_or(0);
                            self.emit_missing_reasoning_part_text(
                                report_context,
                                &mut out,
                                summary_index,
                                text,
                            );
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
                    let key = Self::text_part_key_from_event(&value);
                    self.emit_missing_text(report_context, &mut out, key, text);
                }
            }
            "response.refusal.delta" => {
                let piece = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !piece.is_empty() {
                    let key = Self::text_part_key_from_event(&value);
                    self.emit_text_delta(report_context, &mut out, key, piece);
                }
            }
            "response.refusal.done" => {
                let refusal = value
                    .get("refusal")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("part")
                            .and_then(Value::as_object)
                            .and_then(|part| part.get("refusal"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                if !refusal.is_empty() {
                    let key = Self::text_part_key_from_event(&value);
                    self.emit_missing_text(report_context, &mut out, key, refusal);
                }
            }
            "response.audio.transcript.delta" => {
                let piece = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !piece.is_empty() {
                    let key = Self::text_part_key_from_event(&value);
                    self.emit_text_delta(report_context, &mut out, key, piece);
                }
            }
            "response.audio.transcript.done" => {
                let transcript = value
                    .get("transcript")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("text").and_then(Value::as_str))
                    .unwrap_or_default();
                if !transcript.is_empty() {
                    let key = Self::text_part_key_from_event(&value);
                    self.emit_missing_text(report_context, &mut out, key, transcript);
                }
            }
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                let piece = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !piece.is_empty() {
                    let summary_index = value
                        .get("summary_index")
                        .and_then(Value::as_u64)
                        .map(|value| value as usize)
                        .unwrap_or(0);
                    self.ensure_started(report_context, &mut out);
                    self.reasoning.push_str(piece);
                    self.reasoning_parts
                        .entry(summary_index)
                        .or_default()
                        .push_str(piece);
                    let (id, model) = self.identity(report_context);
                    out.push(CanonicalStreamFrame {
                        id,
                        model,
                        event: CanonicalStreamEvent::ReasoningDelta(piece.to_string()),
                    });
                }
            }
            "response.reasoning_text.done" | "response.reasoning_summary_text.done" => {
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
                    let summary_index = value
                        .get("summary_index")
                        .and_then(Value::as_u64)
                        .map(|value| value as usize)
                        .unwrap_or(0);
                    self.emit_missing_reasoning_part_text(
                        report_context,
                        &mut out,
                        summary_index,
                        text,
                    );
                }
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);
                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::ReasoningSummaryDone,
                });
            }
            "response.output_item.added" => {
                let Some(item) = value.get("item").and_then(Value::as_object) else {
                    return Ok(out);
                };
                let output_index = value
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                self.emit_output_item_event(
                    report_context,
                    &mut out,
                    &value,
                    item,
                    output_index,
                    false,
                );
            }
            "response.custom_tool_call_input.delta" => {
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
                let state = self.tool_calls.entry(index).or_default();
                if state.name.is_empty() {
                    state.name = value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("custom_tool")
                        .to_string();
                }
                state.arguments.push_str(delta);
                self.emit_ready_tool_call(report_context, &mut out, index);
            }
            "response.custom_tool_call_input.done" => {
                let input = value
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
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
                let state = self.tool_calls.entry(index).or_default();
                if state.name.is_empty() {
                    state.name = value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("custom_tool")
                        .to_string();
                }
                let arguments = tool_arguments_from_maybe_json_string(
                    Some(&Value::String(input.to_string())),
                    "input",
                );
                Self::merge_tool_call_arguments(state, &arguments);
                self.emit_ready_tool_call(report_context, &mut out, index);
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
                let state = self.tool_calls.entry(index).or_default();
                if let Some(call_id) = value
                    .get("call_id")
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                {
                    state.call_id = call_id.to_string();
                } else if state.call_id.is_empty() {
                    state.call_id = value
                        .get("item_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                }
                state.arguments.push_str(delta);
                self.emit_ready_tool_call(report_context, &mut out, index);
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
                let state = self.tool_calls.entry(index).or_default();
                state.call_id = value
                    .get("call_id")
                    .or_else(|| value.get("id"))
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("item")
                            .and_then(Value::as_object)
                            .and_then(|item| item.get("call_id").or_else(|| item.get("id")))
                            .and_then(Value::as_str)
                    })
                    .or_else(|| value.get("item_id").and_then(Value::as_str))
                    .unwrap_or(state.call_id.as_str())
                    .to_string();
                state.name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        value
                            .get("item")
                            .and_then(Value::as_object)
                            .and_then(|item| item.get("name"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or(state.name.as_str())
                    .to_string();
                Self::merge_tool_call_arguments(state, arguments);
                self.emit_ready_tool_call(report_context, &mut out, index);
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
            event_type if openai_responses_hosted_tool_output_item_type(event_type).is_some() => {
                let item_type = openai_responses_hosted_tool_output_item_type(event_type)
                    .expect("guarded by is_some");
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
                let index = self
                    .tool_index_for_key(Some(format!("{item_type}:{tool_use_id}")), output_index);
                let content = openai_tool_result_content_from_value(
                    value
                        .get("delta")
                        .or_else(|| value.get("output"))
                        .or_else(|| value.get("content")),
                );
                let name = openai_responses_hosted_tool_output_name(item_type)
                    .map(ToOwned::to_owned)
                    .or_else(|| {
                        value
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                            .map(ToOwned::to_owned)
                    });
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
                self.emit_output_item_event(
                    report_context,
                    &mut out,
                    &value,
                    item,
                    output_index,
                    true,
                );
            }
            "response.incomplete" => {
                let Some(response) = value.get("response").and_then(Value::as_object) else {
                    return Ok(out);
                };
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);
                self.emit_response_output_items(report_context, &mut out, response);

                out.push(CanonicalStreamFrame {
                    id,
                    model,
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some(openai_responses_incomplete_finish_reason(&value)),
                        usage: canonical_usage_from_openai_usage(response.get("usage")),
                    },
                });
                self.finished = true;
            }
            "keepalive" => {}
            event_type if openai_responses_stream_event_is_known_noop(event_type) => {
                self.ensure_started(report_context, &mut out);
            }
            event_type if openai_stream_payload_is_terminal_error(&value) => {
                self.finished = true;
                let mut payload = value.clone();
                if event_type != "response.failed"
                    && event_type != "response.incomplete"
                    && event_type != "error"
                {
                    payload = openai_stream_terminal_error_body(&value).unwrap_or(payload);
                    if let Some(object) = payload.as_object_mut() {
                        object.insert(
                            "type".to_string(),
                            Value::String("response.failed".to_string()),
                        );
                    }
                }
                out.push(self.unknown_frame(report_context, payload));
            }
            "response.completed" | "response.done" => {
                let Some(response) = value.get("response").and_then(Value::as_object) else {
                    return Ok(out);
                };
                self.ensure_started(report_context, &mut out);
                let (id, model) = self.identity(report_context);
                self.emit_response_output_items(report_context, &mut out, response);

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
    actual_service_tier: Option<String>,
    started: bool,
    finished: bool,
    next_tool_call_index: usize,
    tool_call_index_by_canonical: BTreeMap<usize, usize>,
}

#[derive(Clone, Default)]
struct OpenAIResponsesClientToolState {
    call_id: String,
    name: String,
    arguments: String,
    output_index: Option<usize>,
    web_search: bool,
}

#[derive(Clone, Default)]
struct OpenAIResponsesClientToolResultState {
    tool_use_id: String,
    name: Option<String>,
    content: String,
    output_index: Option<usize>,
    item_started: bool,
}

fn is_responses_web_search_tool(name: &str) -> bool {
    matches!(name, "web_search" | "web_search_preview")
}

fn web_search_query_from_arguments(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("query")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| value.as_str().map(ToOwned::to_owned))
        })
        .unwrap_or_default()
}

#[derive(Default)]
pub struct OpenAIResponsesClientEmitter {
    response_id: Option<String>,
    model: Option<String>,
    actual_service_tier: Option<String>,
    created_at: Option<i64>,
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
    reasoning_part: String,
    reasoning_summary_parts: Vec<String>,
    tool_calls: BTreeMap<usize, OpenAIResponsesClientToolState>,
    tool_results: BTreeMap<usize, OpenAIResponsesClientToolResultState>,
    image_generation_items: BTreeMap<usize, Value>,
    opaque_output_items: BTreeMap<usize, Value>,
    opaque_output_indexes: BTreeMap<String, usize>,
}

impl OpenAIChatClientEmitter {
    pub(crate) fn set_actual_service_tier(&mut self, value: Option<&str>) {
        if value.is_some_and(|value| {
            self.actual_service_tier
                .as_deref()
                .is_some_and(|current| current.eq_ignore_ascii_case(value.trim()))
        }) {
            return;
        }
        if let Some(value) = normalize_openai_service_tier(value) {
            self.actual_service_tier = Some(value);
        }
    }

    fn encode_chunk(&self, mut chunk: Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if let (Some(service_tier), Some(object)) =
            (self.actual_service_tier.as_ref(), chunk.as_object_mut())
        {
            object.insert(
                "service_tier".to_string(),
                Value::String(service_tier.clone()),
            );
        }
        encode_json_sse(None, &chunk)
    }

    fn update_identity(&mut self, frame: &CanonicalStreamFrame) {
        self.response_id = Some(frame.id.clone());
        self.model = Some(frame.model.clone());
    }

    fn ensure_started(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.started {
            return Ok(Vec::new());
        }
        self.started = true;
        self.encode_chunk(build_openai_chat_role_chunk(
            self.response_id
                .as_deref()
                .unwrap_or("chatcmpl-local-stream"),
            self.model.as_deref().unwrap_or("unknown"),
        ))
    }

    fn chat_tool_call_index(&mut self, canonical_index: usize) -> usize {
        if let Some(index) = self.tool_call_index_by_canonical.get(&canonical_index) {
            return *index;
        }
        let index = self.next_tool_call_index;
        self.next_tool_call_index += 1;
        self.tool_call_index_by_canonical
            .insert(canonical_index, index);
        index
    }

    pub fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.update_identity(&frame);
        match frame.event {
            CanonicalStreamEvent::Start => self.ensure_started(),
            CanonicalStreamEvent::TextDelta(text) => {
                let mut out = self.ensure_started()?;
                out.extend(
                    self.encode_chunk(build_openai_chat_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        text,
                        None,
                        None,
                    ))?,
                );
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningDelta(text) => {
                let mut out = self.ensure_started()?;
                out.extend(self.encode_chunk(json!({
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
                }))?);
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningSummaryDone => {
                // CPA strategy: emit "\n\n" as paragraph separator between
                // reasoning sections, matching CPA's Chat downstream behavior.
                let mut out = self.ensure_started()?;
                out.extend(self.encode_chunk(json!({
                    "id": self.response_id
                        .as_deref()
                        .unwrap_or("chatcmpl-local-stream"),
                    "object": "chat.completion.chunk",
                    "model": self.model.as_deref().unwrap_or("unknown"),
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "reasoning_content": "\n\n",
                        },
                        "finish_reason": Value::Null
                    }]
                }))?);
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningSignature(_) => Ok(Vec::new()),
            CanonicalStreamEvent::ContentPart(part) => {
                let placeholder = openai_stream_placeholder_for_content_part(&part);
                let mut out = self.ensure_started()?;
                out.extend(
                    self.encode_chunk(build_openai_chat_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        placeholder,
                        None,
                        None,
                    ))?,
                );
                Ok(out)
            }
            CanonicalStreamEvent::ImageGenerationCall { item, .. } => {
                let Some(part) = content_part_from_openai_image_generation_item(&item) else {
                    return Ok(Vec::new());
                };
                let placeholder = openai_stream_placeholder_for_content_part(&part);
                let mut out = self.ensure_started()?;
                out.extend(
                    self.encode_chunk(build_openai_chat_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        placeholder,
                        None,
                        None,
                    ))?,
                );
                Ok(out)
            }
            CanonicalStreamEvent::OpenAiResponsesOutputItem { .. } => {
                Err(AiSurfaceFinalizeError::new(
                    "OpenAI Responses output items cannot be converted losslessly to OpenAI Chat",
                ))
            }
            CanonicalStreamEvent::ToolCallStart {
                index,
                call_id,
                name,
            } => {
                let mut out = self.ensure_started()?;
                let chat_index = self.chat_tool_call_index(index);
                out.extend(
                    self.encode_chunk(build_openai_chat_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        String::new(),
                        Some(vec![json!({
                            "index": chat_index,
                            "id": call_id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": "",
                            }
                        })]),
                        None,
                    ))?,
                );
                Ok(out)
            }
            CanonicalStreamEvent::ToolCallArgumentsDelta { index, arguments } => {
                let mut out = self.ensure_started()?;
                let chat_index = self.chat_tool_call_index(index);
                out.extend(self.encode_chunk(json!({
                    "id": self.response_id
                        .as_deref()
                        .unwrap_or("chatcmpl-local-stream"),
                    "object": "chat.completion.chunk",
                    "model": self.model.as_deref().unwrap_or("unknown"),
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": chat_index,
                                "function": {
                                    "arguments": arguments,
                                }
                            }]
                        },
                        "finish_reason": Value::Null
                    }]
                }))?);
                Ok(out)
            }
            CanonicalStreamEvent::ToolResultDelta {
                index: _,
                tool_use_id,
                name,
                content,
            } => {
                let mut out = self.ensure_started()?;
                let mut delta = Map::new();
                delta.insert("role".to_string(), Value::String("tool".to_string()));
                delta.insert("tool_call_id".to_string(), Value::String(tool_use_id));
                if let Some(name) = name.filter(|value| !value.trim().is_empty()) {
                    delta.insert("name".to_string(), Value::String(name));
                }
                delta.insert("content".to_string(), Value::String(content));
                out.extend(self.encode_chunk(json!({
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
                }))?);
                Ok(out)
            }
            CanonicalStreamEvent::UnknownEvent(payload)
                if openai_stream_terminal_error_body(&payload).is_some() =>
            {
                self.finished = true;
                let error_body = openai_stream_terminal_error_body(&payload).unwrap_or(payload);
                encode_json_sse(None, &error_body)
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
                out.extend(
                    self.encode_chunk(build_openai_chat_finish_chunk(
                        self.response_id
                            .as_deref()
                            .unwrap_or("chatcmpl-local-stream"),
                        self.model.as_deref().unwrap_or("unknown"),
                        finish_reason.as_deref(),
                    ))?,
                );
                if let Some(usage) = usage {
                    out.extend(
                        self.encode_chunk(build_openai_chat_usage_chunk_from_usage(
                            self.response_id
                                .as_deref()
                                .unwrap_or("chatcmpl-local-stream"),
                            self.model.as_deref().unwrap_or("unknown"),
                            &usage,
                        ))?,
                    );
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
        let out = self.encode_chunk(build_openai_chat_finish_chunk(
            self.response_id
                .as_deref()
                .unwrap_or("chatcmpl-local-stream"),
            self.model.as_deref().unwrap_or("unknown"),
            None,
        ))?;
        self.finished = true;
        let mut bytes = out;
        bytes.extend(encode_done_sse());
        Ok(bytes)
    }
}

impl OpenAIResponsesClientEmitter {
    pub(crate) fn set_actual_service_tier(&mut self, value: Option<&str>) {
        if value.is_some_and(|value| {
            self.actual_service_tier
                .as_deref()
                .is_some_and(|current| current.eq_ignore_ascii_case(value.trim()))
        }) {
            return;
        }
        if let Some(value) = normalize_openai_service_tier(value) {
            self.actual_service_tier = Some(value);
        }
    }

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
        let mut response = json!({
            "id": self.response_id(),
            "object": "response",
            "model": self.model(),
            "status": "in_progress",
            "output": [],
        });
        if let (Some(created_at), Some(response_object)) =
            (self.created_at, response.as_object_mut())
        {
            response_object.insert("created_at".to_string(), Value::from(created_at));
        }
        if let (Some(service_tier), Some(response_object)) =
            (self.actual_service_tier.as_ref(), response.as_object_mut())
        {
            response_object.insert(
                "service_tier".to_string(),
                Value::String(service_tier.clone()),
            );
        }
        response
    }

    fn ensure_created_at(&mut self) -> i64 {
        if let Some(created_at) = self.created_at {
            return created_at;
        }
        let created_at = openai_responses_current_timestamp();
        self.created_at = Some(created_at);
        created_at
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
        self.ensure_created_at();
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

    fn current_reasoning_summary_index(&self) -> usize {
        self.reasoning_summary_parts.len()
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

    fn ensure_image_generation_output_index(&mut self, index: usize) -> usize {
        self.next_output_index = self.next_output_index.max(index.saturating_add(1));
        index
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
            let summary_index = self.current_reasoning_summary_index();
            out.extend(self.encode_response_event(
                "response.reasoning_summary_part.added",
                json!({
                    "type": "response.reasoning_summary_part.added",
                    "response_id": self.response_id(),
                    "item_id": item_id,
                    "output_index": output_index,
                    "summary_index": summary_index,
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
            let summary_index = self.current_reasoning_summary_index();
            let part_text = self.reasoning_part.clone();
            out.extend(self.encode_response_event(
                "response.reasoning_summary_text.done",
                json!({
                    "type": "response.reasoning_summary_text.done",
                    "response_id": self.response_id(),
                    "item_id": item_id.clone(),
                    "output_index": output_index,
                    "summary_index": summary_index,
                    "text": part_text.as_str(),
                }),
            )?);
            out.extend(self.encode_response_event(
                "response.reasoning_summary_part.done",
                json!({
                    "type": "response.reasoning_summary_part.done",
                    "response_id": self.response_id(),
                    "item_id": item_id.clone(),
                    "output_index": output_index,
                    "summary_index": summary_index,
                    "part": {
                        "type": "summary_text",
                        "text": part_text.as_str(),
                    }
                }),
            )?);
            self.reasoning_summary_parts.push(part_text);
            self.reasoning_part.clear();
            self.reasoning_part_started = false;
        }
        let summary = if self.reasoning_summary_parts.is_empty() {
            if self.reasoning.trim().is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "summary_text",
                    "text": self.reasoning.as_str(),
                })]
            }
        } else {
            self.reasoning_summary_parts
                .iter()
                .map(|text| {
                    json!({
                        "type": "summary_text",
                        "text": text,
                    })
                })
                .collect::<Vec<_>>()
        };
        out.extend(self.encode_response_event(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "response_id": self.response_id(),
                "output_index": output_index,
                "item": {
                    "type": "reasoning",
                    "id": item_id,
                    "summary": summary,
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
            if state.web_search {
                out.extend(self.encode_response_event(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "response_id": self.response_id(),
                        "output_index": output_index,
                        "item": {
                            "type": "web_search_call",
                            "id": item_id,
                            "status": "completed",
                            "action": {
                                "type": "search",
                                "query": web_search_query_from_arguments(&state.arguments),
                            },
                        }
                    }),
                )?);
                continue;
            }
            out.extend(self.encode_response_event(
                "response.function_call_arguments.done",
                json!({
                    "type": "response.function_call_arguments.done",
                    "response_id": self.response_id(),
                    "output_index": output_index,
                    "item_id": item_id.clone(),
                    "call_id": item_id.clone(),
                    "name": name,
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
            if let Some(name) = state
                .name
                .as_ref()
                .filter(|value| !value.trim().is_empty())
                .cloned()
            {
                item.insert("name".to_string(), Value::String(name));
            }
            item.insert("output".to_string(), Value::String(state.content.clone()));
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

    fn emit_image_generation_call_item(
        &mut self,
        index: usize,
        item: Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut out = self.ensure_started()?;
        let output_index = self.ensure_image_generation_output_index(index);
        let mut item = item.as_object().cloned().unwrap_or_default();
        item.insert(
            "type".to_string(),
            Value::String("image_generation_call".to_string()),
        );
        if !item.contains_key("id") {
            item.insert(
                "id".to_string(),
                Value::String(format!("{}_ig_{}", self.response_id(), output_index)),
            );
        }
        if !item.contains_key("status") {
            item.insert("status".to_string(), Value::String("completed".to_string()));
        }
        let item = Value::Object(item);
        self.image_generation_items
            .insert(output_index, item.clone());
        out.extend(self.encode_response_event(
            "response.output_item.done",
            json!({
                "type": "response.output_item.done",
                "response_id": self.response_id(),
                "output_index": output_index,
                "item": item,
            }),
        )?);
        Ok(out)
    }

    fn terminal_response(
        &self,
        usage: CanonicalUsage,
        status: &str,
        incomplete_reason: Option<&str>,
    ) -> Value {
        let mut ordered_output = Vec::new();
        let summary = if self.reasoning_summary_parts.is_empty() {
            if self.reasoning.trim().is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "summary_text",
                    "text": self.reasoning.as_str(),
                })]
            }
        } else {
            self.reasoning_summary_parts
                .iter()
                .map(|text| {
                    json!({
                        "type": "summary_text",
                        "text": text,
                    })
                })
                .collect::<Vec<_>>()
        };
        if !summary.is_empty() {
            ordered_output.push((
                self.reasoning_output_index.unwrap_or(0),
                json!({
                    "type": "reasoning",
                    "id": self.reasoning_item_id(),
                    "status": "completed",
                    "summary": summary,
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
                let item_id = if state.call_id.is_empty() {
                    build_generated_tool_call_id(*index)
                } else {
                    state.call_id.clone()
                };
                if state.web_search {
                    ordered_output.push((
                        output_index,
                        json!({
                            "type": "web_search_call",
                            "id": item_id,
                            "status": "completed",
                            "action": {
                                "type": "search",
                                "query": web_search_query_from_arguments(&state.arguments),
                            },
                        }),
                    ));
                    continue;
                }
                ordered_output.push((
                    output_index,
                    json!({
                        "type": "function_call",
                        "id": item_id.clone(),
                        "call_id": item_id,
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
        for (output_index, item) in &self.image_generation_items {
            ordered_output.push((*output_index, item.clone()));
        }
        for (output_index, item) in &self.opaque_output_items {
            ordered_output.push((*output_index, item.clone()));
        }
        ordered_output.sort_by_key(|(output_index, _)| *output_index);

        let mut response = json!({
            "id": self.response_id(),
            "object": "response",
            "status": status,
            "model": self.model(),
            "output": ordered_output
                .into_iter()
                .map(|(_, item)| item)
                .collect::<Vec<_>>(),
            "usage": openai_responses_usage_from_usage(&usage),
        });
        if let Some(reason) = incomplete_reason {
            response["incomplete_details"] = json!({ "reason": reason });
        }
        if let Some(response_object) = response.as_object_mut() {
            if let Some(created_at) = self.created_at {
                response_object.insert("created_at".to_string(), Value::from(created_at));
            }
            ensure_modern_openai_responses_response_fields(response_object);
            if let Some(service_tier) = self.actual_service_tier.as_ref() {
                response_object.insert(
                    "service_tier".to_string(),
                    Value::String(service_tier.clone()),
                );
            }
        }
        response
    }

    fn completed_response(&self, usage: CanonicalUsage) -> Value {
        self.terminal_response(usage, "completed", None)
    }

    fn incomplete_response(&self, usage: CanonicalUsage, reason: &str) -> Value {
        self.terminal_response(usage, "incomplete", Some(reason))
    }

    pub(crate) fn finish_with_authoritative_response_event(
        &mut self,
        response: Value,
        event_type: &'static str,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.finished {
            return Ok(Vec::new());
        }
        if !matches!(
            event_type,
            "response.completed" | "response.incomplete" | "response.failed"
        ) {
            return Err(AiSurfaceFinalizeError::new(format!(
                "unsupported authoritative OpenAI Responses terminal event: {event_type}"
            )));
        }
        let response_object = response.as_object().ok_or_else(|| {
            AiSurfaceFinalizeError::new(
                "authoritative OpenAI Responses terminal payload must be an object",
            )
        })?;
        self.set_actual_service_tier(response_object.get("service_tier").and_then(Value::as_str));
        if let Some(response_id) = response_object
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            self.response_id = Some(response_id.to_string());
        }
        if let Some(model) = response_object
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            self.model = Some(model.to_string());
        }
        if let Some(created_at) = response_object.get("created_at").and_then(Value::as_i64) {
            self.created_at = Some(created_at);
        }

        let mut out = self.ensure_started()?;
        out.extend(self.finish_reasoning_item()?);
        out.extend(self.finish_text_item()?);
        out.extend(self.finish_tool_items()?);
        out.extend(self.finish_tool_result_items()?);
        out.extend(self.encode_response_event(
            event_type,
            json!({
                "type": event_type,
                "response": response,
            }),
        )?);
        self.finished = true;
        Ok(out)
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
                self.reasoning_part.push_str(&text);
                out.extend(self.encode_response_event(
                    "response.reasoning_summary_text.delta",
                    json!({
                        "type": "response.reasoning_summary_text.delta",
                        "response_id": self.response_id(),
                        "item_id": self.reasoning_item_id(),
                        "output_index": self.reasoning_output_index.unwrap_or(0),
                        "summary_index": self.current_reasoning_summary_index(),
                        "delta": text,
                    }),
                )?);
                Ok(out)
            }
            CanonicalStreamEvent::ReasoningSummaryDone => {
                // Close the current reasoning part and reset state so the next
                // ReasoningDelta starts a fresh part within the same item.
                if !self.reasoning_item_started || !self.reasoning_part_started {
                    return Ok(Vec::new());
                }
                let output_index = self.reasoning_output_index.unwrap_or(0);
                let item_id = self.reasoning_item_id();
                let summary_index = self.current_reasoning_summary_index();
                let part_text = self.reasoning_part.clone();
                let mut out = Vec::new();
                out.extend(self.encode_response_event(
                    "response.reasoning_summary_text.done",
                    json!({
                        "type": "response.reasoning_summary_text.done",
                        "response_id": self.response_id(),
                        "item_id": item_id.clone(),
                        "output_index": output_index,
                        "summary_index": summary_index,
                        "text": part_text.as_str(),
                    }),
                )?);
                out.extend(self.encode_response_event(
                    "response.reasoning_summary_part.done",
                    json!({
                        "type": "response.reasoning_summary_part.done",
                        "response_id": self.response_id(),
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": summary_index,
                        "part": {
                            "type": "summary_text",
                            "text": part_text.as_str(),
                        }
                    }),
                )?);
                self.reasoning_summary_parts.push(part_text);
                self.reasoning_part.clear();
                self.reasoning_part_started = false;
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
            CanonicalStreamEvent::ImageGenerationCall { index, item } => {
                self.emit_image_generation_call_item(index, item)
            }
            CanonicalStreamEvent::OpenAiResponsesOutputItem {
                output_index,
                item,
                raw_event,
            } => {
                let event_type = raw_event
                    .get("type")
                    .and_then(Value::as_str)
                    .filter(|value| {
                        matches!(
                            *value,
                            "response.output_item.added" | "response.output_item.done"
                        )
                    })
                    .ok_or_else(|| {
                        AiSurfaceFinalizeError::new(
                            "OpenAI Responses output item event must be added or done",
                        )
                    })?
                    .to_string();
                let item_key = item
                    .as_object()
                    .map(OpenAIResponsesProviderState::output_item_key)
                    .unwrap_or_else(|| item.to_string());
                let materialized_output_index = if let Some(output_index) = output_index {
                    self.next_output_index =
                        self.next_output_index.max(output_index.saturating_add(1));
                    self.opaque_output_indexes.insert(item_key, output_index);
                    output_index
                } else if let Some(output_index) = self.opaque_output_indexes.get(&item_key) {
                    *output_index
                } else {
                    let output_index = self.allocate_output_index();
                    self.opaque_output_indexes.insert(item_key, output_index);
                    output_index
                };
                let mut out = self.ensure_started()?;
                if event_type == "response.output_item.done" {
                    self.opaque_output_items
                        .insert(materialized_output_index, item);
                }
                out.extend(encode_json_sse(Some(event_type.as_str()), &raw_event)?);
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
                state.web_search = is_responses_web_search_tool(&name);
                let emitted_call_id = state.call_id.clone();
                let emitted_name = state.name.clone();
                let item = if state.web_search {
                    json!({
                        "type": "web_search_call",
                        "id": emitted_call_id,
                        "status": "in_progress",
                        "action": {
                            "type": "search",
                            "query": "",
                        },
                    })
                } else {
                    json!({
                        "type": "function_call",
                        "id": call_id,
                        "call_id": emitted_call_id,
                        "name": emitted_name,
                        "arguments": "",
                        "status": "in_progress",
                    })
                };
                out.extend(self.encode_response_event(
                    "response.output_item.added",
                    json!({
                        "type": "response.output_item.added",
                        "response_id": response_id,
                        "output_index": output_index,
                        "item": item
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
                if state.web_search {
                    return Ok(out);
                }
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
            CanonicalStreamEvent::UnknownEvent(payload)
                if openai_stream_terminal_error_body(&payload).is_some() =>
            {
                self.finished = true;
                let raw_event = payload.get("type").and_then(Value::as_str);
                let event = raw_event
                    .filter(|event| {
                        matches!(*event, "response.failed" | "response.incomplete" | "error")
                    })
                    .unwrap_or("response.failed")
                    .to_string();
                let mut payload = if raw_event == Some(event.as_str()) {
                    payload
                } else {
                    openai_stream_terminal_error_body(&payload).unwrap_or(payload)
                };
                if payload.get("type").is_none() {
                    if let Some(object) = payload.as_object_mut() {
                        object.insert("type".to_string(), Value::String(event.clone()));
                    }
                }
                self.encode_response_event(event.as_str(), payload)
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
                out.extend(self.finish_reasoning_item()?);
                out.extend(self.finish_text_item()?);
                out.extend(self.finish_tool_items()?);
                out.extend(self.finish_tool_result_items()?);
                let usage = usage.unwrap_or_default();
                let (event_type, response) = match finish_reason.as_deref() {
                    Some("length") => (
                        "response.incomplete",
                        self.incomplete_response(usage, "max_output_tokens"),
                    ),
                    Some("content_filter") => (
                        "response.incomplete",
                        self.incomplete_response(usage, "content_filter"),
                    ),
                    _ => ("response.completed", self.completed_response(usage)),
                };
                out.extend(self.encode_response_event(
                    event_type,
                    json!({
                        "type": event_type,
                        "response": response,
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

fn tool_arguments_from_maybe_json_string(value: Option<&Value>, fallback_key: &str) -> String {
    match value {
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return String::new();
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(Value::Object(_)) => trimmed.to_string(),
                Ok(parsed) => single_field_tool_arguments(fallback_key, parsed),
                Err(_) => single_field_tool_arguments(fallback_key, Value::String(text.clone())),
            }
        }
        Some(value @ Value::Object(_)) => value.to_string(),
        Some(Value::Null) | None => String::new(),
        Some(value) => single_field_tool_arguments(fallback_key, value.clone()),
    }
}

fn single_field_tool_arguments(key: &str, value: Value) -> String {
    let mut arguments = Map::new();
    arguments.insert(key.to_string(), value);
    Value::Object(arguments).to_string()
}

fn tool_arguments_from_named_fields(item: &Map<String, Value>, field_names: &[&str]) -> String {
    let mut arguments = Map::new();
    for field_name in field_names {
        if let Some(value) = item.get(*field_name) {
            arguments.insert((*field_name).to_string(), value.clone());
        }
    }
    if arguments.is_empty() {
        String::new()
    } else {
        Value::Object(arguments).to_string()
    }
}

fn openai_responses_stream_event_is_known_noop(event_type: &str) -> bool {
    matches!(
        event_type,
        "response.queued"
            | "response.metadata"
            | "response.output_text.annotation.added"
            | "response.audio.delta"
            | "response.audio.done"
            | "response.code_interpreter_call.in_progress"
            | "response.code_interpreter_call.interpreting"
            | "response.code_interpreter_call.completed"
            | "response.code_interpreter_call_code.delta"
            | "response.code_interpreter_call_code.done"
            | "response.file_search_call.in_progress"
            | "response.file_search_call.searching"
            | "response.file_search_call.completed"
            | "response.image_generation_call.in_progress"
            | "response.image_generation_call.generating"
            | "response.image_generation_call.partial_image"
            | "response.image_generation_call.completed"
            | "response.mcp_call.in_progress"
            | "response.mcp_call.completed"
            | "response.mcp_call.failed"
            | "response.mcp_call_arguments.delta"
            | "response.mcp_call_arguments.done"
            | "response.mcp_list_tools.in_progress"
            | "response.mcp_list_tools.completed"
            | "response.mcp_list_tools.failed"
            | "response.web_search_call.in_progress"
            | "response.web_search_call.searching"
            | "response.web_search_call.completed"
            | "response.local_shell_call.in_progress"
            | "response.local_shell_call.running"
            | "response.local_shell_call.completed"
            | "response.local_shell_call.failed"
            | "response.shell_call.in_progress"
            | "response.shell_call.running"
            | "response.shell_call.completed"
            | "response.shell_call.failed"
            | "response.apply_patch_call.in_progress"
            | "response.apply_patch_call.running"
            | "response.apply_patch_call.completed"
            | "response.apply_patch_call.failed"
            | "response.computer_call.in_progress"
            | "response.computer_call.running"
            | "response.computer_call.completed"
            | "response.computer_call.failed"
    )
}

fn openai_responses_hosted_tool_output_item_type(event_type: &str) -> Option<&'static str> {
    match event_type {
        "response.custom_tool_call_output.delta" | "response.custom_tool_call_output.done" => {
            Some("custom_tool_call_output")
        }
        "response.local_shell_call_output.delta" | "response.local_shell_call_output.done" => {
            Some("local_shell_call_output")
        }
        "response.shell_call_output.delta" | "response.shell_call_output.done" => {
            Some("shell_call_output")
        }
        "response.apply_patch_call_output.delta" | "response.apply_patch_call_output.done" => {
            Some("apply_patch_call_output")
        }
        "response.computer_call_output.delta" | "response.computer_call_output.done" => {
            Some("computer_call_output")
        }
        _ => None,
    }
}

fn openai_responses_hosted_tool_output_name(item_type: &str) -> Option<&'static str> {
    match item_type {
        "local_shell_call_output" => Some("local_shell"),
        "shell_call_output" => Some("shell"),
        "apply_patch_call_output" => Some("apply_patch"),
        "computer_call_output" => Some("computer"),
        _ => None,
    }
}

fn openai_responses_incomplete_finish_reason(payload: &Value) -> String {
    let reason = payload
        .get("response")
        .and_then(Value::as_object)
        .and_then(|response| response.get("incomplete_details"))
        .and_then(Value::as_object)
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match reason {
        "content_filter" => "content_filter",
        "tool_calls" | "function_call" => "tool_calls",
        _ => "length",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::claude::messages::stream::ClaudeClientEmitter;

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

    fn response_reasoning_text_done_parts(sse: &str) -> Vec<(u64, String)> {
        let mut parts = Vec::new();
        for block in sse.split("\n\n") {
            let mut event_name = None;
            let mut data = None;
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("event: ") {
                    event_name = Some(value);
                } else if let Some(value) = line.strip_prefix("data: ") {
                    data = Some(value);
                }
            }
            if event_name != Some("response.reasoning_summary_text.done") {
                continue;
            }
            let Some(data) = data else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let Some(summary_index) = value.get("summary_index").and_then(Value::as_u64) else {
                continue;
            };
            let Some(text) = value.get("text").and_then(Value::as_str) else {
                continue;
            };
            parts.push((summary_index, text.to_string()));
        }
        parts
    }

    fn openai_chat_tool_call_indices(sse: &str) -> Vec<u64> {
        let mut indices = Vec::new();
        for payload in sse.lines().filter_map(|line| line.strip_prefix("data: ")) {
            let Ok(value) = serde_json::from_str::<Value>(payload) else {
                continue;
            };
            let Some(tool_calls) = value
                .pointer("/choices/0/delta/tool_calls")
                .and_then(Value::as_array)
            else {
                continue;
            };
            for tool_call in tool_calls {
                if let Some(index) = tool_call.get("index").and_then(Value::as_u64) {
                    indices.push(index);
                }
            }
        }
        indices
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
    fn openai_responses_provider_state_recognizes_compaction_output_without_index() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let raw_event = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "compaction",
                "encrypted_content": "ENCRYPTED_CONTEXT_COMPACTION_SUMMARY"
            }
        });
        let frames = state
            .push_line(&report_context, data_line(raw_event.clone()))
            .expect("compaction output item should parse");

        assert!(frames.iter().any(|frame| matches!(
            &frame.event,
            CanonicalStreamEvent::OpenAiResponsesOutputItem {
                output_index: None,
                item,
                raw_event: emitted_event,
            } if item["type"] == "compaction" && emitted_event == &raw_event
        )));
        assert!(!frames
            .iter()
            .any(|frame| matches!(frame.event, CanonicalStreamEvent::UnknownEvent(_))));

        let terminal = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp-compact",
                        "usage": {
                            "input_tokens": 0,
                            "output_tokens": 0,
                            "total_tokens": 0
                        }
                    }
                })),
            )
            .expect("completed response should parse");
        assert!(terminal
            .iter()
            .any(|frame| matches!(frame.event, CanonicalStreamEvent::Finish { .. })));
        assert!(!terminal
            .iter()
            .any(|frame| matches!(frame.event, CanonicalStreamEvent::UnknownEvent(_))));
    }

    #[test]
    fn openai_responses_provider_state_rejects_function_call_provenance() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.output_item.added",
                    "response_id": "resp_ptc_123",
                    "output_index": 0,
                    "item": {
                        "type": "function_call",
                        "id": "fc_123",
                        "call_id": "call_123",
                        "status": "completed",
                        "name": "lookup",
                        "arguments": "{}",
                        "caller": {"type": "program", "id": "program_123"}
                    }
                })),
            )
            .expect("function call event should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::UnknownEvent(ref payload)
                if payload.get("caller").is_some()
        )));
        assert!(!frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallStart { .. }
                | CanonicalStreamEvent::ToolCallArgumentsDelta { .. }
        )));
    }

    #[test]
    fn openai_responses_provider_state_accepts_chat_backed_function_call_metadata() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.output_item.added",
                    "response_id": "resp_chat_metadata_123",
                    "output_index": 0,
                    "item": {
                        "type": "function_call",
                        "id": "fc_chat_metadata_123",
                        "call_id": "call_chat_metadata_123",
                        "status": "completed",
                        "name": "lookup",
                        "arguments": "{\"query\":\"aether\"}",
                        "internal_chat_message_metadata_passthrough": {
                            "turn_id": "turn_123"
                        },
                        "metadata": {
                            "source": "chat"
                        }
                    }
                })),
            )
            .expect("chat-backed function call event should parse");

        assert!(!frames
            .iter()
            .any(|frame| matches!(frame.event, CanonicalStreamEvent::UnknownEvent(_))));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallStart {
                index: 0,
                ref call_id,
                ref name,
            } if call_id == "call_chat_metadata_123" && name == "lookup"
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallArgumentsDelta {
                index: 0,
                ref arguments,
            } if arguments == "{\"query\":\"aether\"}"
        )));
    }

    #[test]
    fn openai_responses_provider_state_rejects_unpaired_function_call_metadata() {
        let mut state = OpenAIResponsesProviderState::default();
        let frames = state
            .push_line(
                &json!({}),
                data_line(json!({
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": {
                        "type": "function_call",
                        "id": "fc_metadata_only_123",
                        "call_id": "call_metadata_only_123",
                        "status": "completed",
                        "name": "lookup",
                        "arguments": "{}",
                        "metadata": {"semantic": true}
                    }
                })),
            )
            .expect("function call event should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::UnknownEvent(ref payload)
                if payload.get("metadata").is_some()
        )));
        assert!(!frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallStart { .. }
                | CanonicalStreamEvent::ToolCallArgumentsDelta { .. }
        )));
    }

    #[test]
    fn openai_responses_provider_state_rejects_chat_metadata_for_non_openai_clients() {
        let mut state = OpenAIResponsesProviderState::default();
        let frames = state
            .push_line(
                &json!({"client_api_format": "claude:messages"}),
                data_line(json!({
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": {
                        "type": "function_call",
                        "id": "fc_chat_metadata_123",
                        "call_id": "call_chat_metadata_123",
                        "status": "completed",
                        "name": "lookup",
                        "arguments": "{}",
                        "metadata": {"source": "chat"},
                        "internal_chat_message_metadata_passthrough": {
                            "turn_id": "turn_123"
                        }
                    }
                })),
            )
            .expect("function call event should parse");

        assert!(frames
            .iter()
            .any(|frame| matches!(frame.event, CanonicalStreamEvent::UnknownEvent(_))));
    }

    #[test]
    fn openai_responses_provider_state_treats_failed_event_as_terminal() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.failed",
                    "response": {
                        "id": "resp_failed_123",
                        "model": "gpt-5.4",
                        "status": "failed",
                        "error": {
                            "message": "policy failure",
                            "type": "invalid_request_error",
                            "code": "cyber_policy"
                        }
                    }
                })),
            )
            .expect("failed response event should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::UnknownEvent(ref payload)
                if payload.get("type").and_then(Value::as_str) == Some("response.failed")
        )));
        assert!(state
            .finish(&report_context)
            .expect("terminal failure should not synthesize completion")
            .is_empty());
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
        assert!(usage.input_tokens_include_cache);
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
                                "cache_write_tokens": 6,
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
                    cache_creation_tokens: 6,
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
        assert!(sse.contains("\"output_text\":\"Hello\""));
        assert!(sse.contains("\"created_at\":"));
        assert!(sse.contains("\"completed_at\":"));
        assert_eq!(response_sequence_numbers(&sse), (1..=9).collect::<Vec<_>>());
    }

    #[test]
    fn openai_responses_client_emitter_forwards_failed_unknown_event() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_failed_123".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::UnknownEvent(json!({
                    "type": "response.failed",
                    "response": {
                        "id": "resp_failed_123",
                        "model": "gpt-5.4",
                        "status": "failed",
                        "error": {
                            "message": "policy failure",
                            "type": "invalid_request_error",
                            "code": "cyber_policy"
                        }
                    }
                })),
            })
            .expect("failed response event should encode");
        let mut all = bytes;
        all.extend(
            emitter
                .finish()
                .expect("failed stream should not synthesize completion"),
        );

        let sse = String::from_utf8(all).expect("sse should be utf8");
        assert!(sse.contains("event: response.failed\n"));
        assert!(sse.contains("\"message\":\"policy failure\""));
        assert!(!sse.contains("event: response.completed\n"));
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
    fn openai_responses_provider_state_accepts_refusal_events() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.refusal.delta",
                        "response_id": "resp_refusal_123",
                        "output_index": 0,
                        "item_id": "msg_refusal_123",
                        "content_index": 0,
                        "delta": "I can't",
                    })),
                )
                .expect("refusal delta should parse"),
        );
        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.refusal.done",
                        "response_id": "resp_refusal_123",
                        "output_index": 0,
                        "item_id": "msg_refusal_123",
                        "content_index": 0,
                        "refusal": "I can't help with that.",
                    })),
                )
                .expect("refusal done should parse"),
        );

        assert!(frames.iter().any(|frame| matches!(
            &frame.event,
            CanonicalStreamEvent::TextDelta(text) if text == "I can't"
        )));
        assert!(frames.iter().any(|frame| matches!(
            &frame.event,
            CanonicalStreamEvent::TextDelta(text) if text == " help with that."
        )));
    }

    #[test]
    fn openai_responses_provider_state_does_not_duplicate_text_snapshot_deltas() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        for event in [
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_snapshot_delta",
                "output_index": 0,
                "content_index": 0,
                "delta": {
                    "text": "Hello",
                }
            }),
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_snapshot_delta",
                "output_index": 0,
                "content_index": 0,
                "delta": {
                    "text": "Hello world",
                }
            }),
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_snapshot_delta",
                "output_index": 0,
                "content_index": 0,
                "delta": {
                    "text": "Hello",
                }
            }),
            json!({
                "type": "response.output_text.done",
                "response_id": "resp_snapshot_delta",
                "output_index": 0,
                "content_index": 0,
                "text": "Hello world",
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_snapshot_delta",
                    "object": "response",
                    "model": "gpt-5.4",
                    "status": "completed",
                    "output": [{
                        "type": "message",
                        "id": "msg_snapshot_delta",
                        "role": "assistant",
                        "status": "completed",
                        "content": [{
                            "type": "output_text",
                            "text": "Hello world",
                            "annotations": [],
                        }]
                    }],
                }
            }),
        ] {
            frames.extend(
                state
                    .push_line(&report_context, data_line(event))
                    .expect("responses text event should parse"),
            );
        }

        let text = frames
            .iter()
            .filter_map(|frame| match &frame.event {
                CanonicalStreamEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn openai_responses_provider_state_dedupes_text_snapshots_per_output_item() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        for event in [
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_multi_message",
                "output_index": 0,
                "item_id": "msg_1",
                "content_index": 0,
                "delta": "First message.",
            }),
            json!({
                "type": "response.output_text.done",
                "response_id": "resp_multi_message",
                "output_index": 0,
                "item_id": "msg_1",
                "content_index": 0,
                "text": "First message.",
            }),
            json!({
                "type": "response.output_item.done",
                "response_id": "resp_multi_message",
                "output_index": 0,
                "item": {
                    "type": "message",
                    "id": "msg_1",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "First message.",
                    }],
                },
            }),
            json!({
                "type": "response.output_text.delta",
                "response_id": "resp_multi_message",
                "output_index": 1,
                "item_id": "msg_2",
                "content_index": 0,
                "delta": "Second message.",
            }),
            json!({
                "type": "response.output_text.done",
                "response_id": "resp_multi_message",
                "output_index": 1,
                "item_id": "msg_2",
                "content_index": 0,
                "text": "Second message.",
            }),
            json!({
                "type": "response.content_part.done",
                "response_id": "resp_multi_message",
                "output_index": 1,
                "item_id": "msg_2",
                "content_index": 0,
                "part": {
                    "type": "output_text",
                    "text": "Second message.",
                },
            }),
            json!({
                "type": "response.output_item.done",
                "response_id": "resp_multi_message",
                "output_index": 1,
                "item": {
                    "type": "message",
                    "id": "msg_2",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Second message.",
                    }],
                },
            }),
        ] {
            frames.extend(
                state
                    .push_line(&report_context, data_line(event))
                    .expect("responses text event should parse"),
            );
        }

        let text = frames
            .iter()
            .filter_map(|frame| match &frame.event {
                CanonicalStreamEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "First message.Second message.");
    }

    #[test]
    fn openai_responses_provider_state_delays_arguments_until_tool_name_is_known() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let arguments = r#"{"file_path":"D:/projects/UIAutoTest/docs/prd/msr.md","offset":0,"limit":2000,"pages":""}"#;
        let mut frames = Vec::new();

        let delta_frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.function_call_arguments.delta",
                    "response_id": "resp_read_123",
                    "output_index": 0,
                    "item_id": "fc_read_123",
                    "delta": arguments,
                })),
            )
            .expect("arguments delta should parse");

        assert!(matches!(
            delta_frames.first().map(|frame| &frame.event),
            Some(CanonicalStreamEvent::Start)
        ));
        assert!(!delta_frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallStart { .. }
                | CanonicalStreamEvent::ToolCallArgumentsDelta { .. }
        )));
        frames.extend(delta_frames);

        frames.extend(
            state
                .push_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.function_call_arguments.done",
                        "response_id": "resp_read_123",
                        "output_index": 0,
                        "item_id": "fc_read_123",
                        "item": {
                            "type": "function_call",
                            "id": "fc_read_123",
                            "call_id": "call_read_123",
                            "name": "Read",
                            "arguments": arguments,
                        }
                    })),
                )
                .expect("arguments done should parse"),
        );

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallStart {
                ref call_id,
                ref name,
                ..
            } if call_id == "call_read_123" && name == "Read"
        )));
        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolCallArgumentsDelta {
                ref arguments,
                ..
            } if arguments.contains(r#""pages":"""#)
        )));

        let mut emitter = ClaudeClientEmitter::default();
        let mut bytes = Vec::new();
        for frame in frames {
            bytes.extend(emitter.emit(frame).expect("claude frame should encode"));
        }
        bytes.extend(
            emitter
                .finish()
                .expect("claude stream finish should encode"),
        );
        let sse = String::from_utf8(bytes).expect("claude sse should be utf8");

        assert!(sse.contains("\"name\":\"Read\""));
        assert!(sse.contains("\\\"limit\\\":2000"));
        assert!(!sse.contains("\\\"pages\\\":\\\"\\\""));
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
    fn openai_responses_provider_state_ignores_hosted_tool_progress_events() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        for event_type in [
            "response.local_shell_call.in_progress",
            "response.local_shell_call.running",
            "response.local_shell_call.completed",
            "response.apply_patch_call.in_progress",
            "response.apply_patch_call.completed",
            "response.computer_call.in_progress",
            "response.computer_call.completed",
        ] {
            frames.extend(
                state
                    .push_line(
                        &report_context,
                        data_line(json!({
                            "type": event_type,
                            "response_id": "resp_123",
                            "output_index": 0,
                            "item_id": "call_123",
                        })),
                    )
                    .expect("hosted tool progress event should parse"),
            );
        }

        assert!(frames
            .iter()
            .any(|frame| matches!(frame.event, CanonicalStreamEvent::Start)));
        assert!(!frames
            .iter()
            .any(|frame| matches!(frame.event, CanonicalStreamEvent::UnknownEvent { .. })));
    }

    #[test]
    fn openai_responses_provider_state_parses_hosted_tool_output_as_tool_result() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.local_shell_call_output.done",
                    "response_id": "resp_123",
                    "output_index": 3,
                    "call_id": "call_shell_123",
                    "output": {
                        "stdout": "ok\n",
                        "stderr": "",
                        "outcome": "success"
                    },
                })),
            )
            .expect("hosted tool result should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ToolResultDelta {
                index: 3,
                ref tool_use_id,
                name: Some(ref name),
                ref content,
            } if tool_use_id == "call_shell_123"
                && name == "local_shell"
                && content.contains("\"stdout\":\"ok\\n\"")
        )));
    }

    #[test]
    fn openai_responses_provider_state_preserves_image_generation_calls() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_img_123",
                        "model": "gpt-image-2",
                        "output": [{
                            "id": "ig_123",
                            "type": "image_generation_call",
                            "status": "completed",
                            "output_format": "png",
                            "result": "aGVsbG8="
                        }],
                        "usage": {"input_tokens": 1, "output_tokens": 2, "total_tokens": 3}
                    }
                })),
            )
            .expect("completed event should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ImageGenerationCall {
                index: 0,
                ref item,
            } if item["type"] == json!("image_generation_call")
                && item["result"] == json!("aGVsbG8=")
        )));
    }

    #[test]
    fn openai_responses_provider_state_waits_for_final_image_generation_item() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});

        let added_frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": {
                        "id": "ig_123",
                        "type": "image_generation_call",
                        "status": "generating",
                        "output_format": "png",
                        "result": "early"
                    }
                })),
            )
            .expect("added event should parse");

        assert!(!added_frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ImageGenerationCall { .. }
        )));

        let done_frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": {
                        "id": "ig_123",
                        "type": "image_generation_call",
                        "status": "completed",
                        "output_format": "png",
                        "result": "final"
                    }
                })),
            )
            .expect("done event should parse");

        assert!(done_frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ImageGenerationCall {
                index: 0,
                ref item,
            } if item["status"] == json!("completed") && item["result"] == json!("final")
        )));
    }

    #[test]
    fn openai_responses_client_emitter_emits_image_generation_call_events() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_img_123".to_string(),
                model: "gpt-image-2".to_string(),
                event: CanonicalStreamEvent::ImageGenerationCall {
                    index: 0,
                    item: json!({
                        "id": "ig_123",
                        "type": "image_generation_call",
                        "status": "completed",
                        "output_format": "png",
                        "result": "aGVsbG8="
                    }),
                },
            })
            .expect("image event should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_img_123".to_string(),
                    model: "gpt-image-2".to_string(),
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some("stop".to_string()),
                        usage: None,
                    },
                })
                .expect("finish should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("event: response.output_item.done\n"));
        assert!(sse.contains("\"type\":\"image_generation_call\""));
        assert!(sse.contains("\"result\":\"aGVsbG8=\""));
        assert!(sse.contains("\"output\":["));
        assert!(sse.contains("\"id\":\"ig_123\""));
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
    fn openai_responses_client_emitter_emits_web_search_call_item() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_123".to_string(),
                model: "gpt-5-5-low".to_string(),
                event: CanonicalStreamEvent::ToolCallStart {
                    index: 0,
                    call_id: "call_ws_1".to_string(),
                    name: "web_search".to_string(),
                },
            })
            .expect("tool start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_123".to_string(),
                    model: "gpt-5-5-low".to_string(),
                    event: CanonicalStreamEvent::ToolCallArgumentsDelta {
                        index: 0,
                        arguments: r#"{"query":"today tech"}"#.to_string(),
                    },
                })
                .expect("arguments should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_123".to_string(),
                    model: "gpt-5-5-low".to_string(),
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some("tool_calls".to_string()),
                        usage: None,
                    },
                })
                .expect("finish should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert!(sse.contains("event: response.output_item.added\n"));
        assert!(sse.contains(r#""type":"web_search_call""#));
        assert!(sse.contains(r#""status":"in_progress""#));
        assert!(sse.contains(r#""query":"""#));
        assert!(sse.contains(r#""type":"search""#));
        assert!(sse.contains("event: response.output_item.done\n"));
        assert!(sse.contains(r#""query":"today tech""#));
        assert!(!sse.contains("response.function_call_arguments.delta"));
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
    fn openai_chat_client_emitter_normalizes_sparse_tool_call_indices() {
        let mut emitter = OpenAIChatClientEmitter::default();
        let mut bytes = Vec::new();

        for event in [
            CanonicalStreamEvent::ToolCallStart {
                index: 1,
                call_id: "call_first".to_string(),
                name: "first_tool".to_string(),
            },
            CanonicalStreamEvent::ToolCallArgumentsDelta {
                index: 1,
                arguments: "{\"first\":".to_string(),
            },
            CanonicalStreamEvent::ToolCallStart {
                index: 3,
                call_id: "call_second".to_string(),
                name: "second_tool".to_string(),
            },
            CanonicalStreamEvent::ToolCallArgumentsDelta {
                index: 3,
                arguments: "{\"second\":true}".to_string(),
            },
            CanonicalStreamEvent::ToolCallArgumentsDelta {
                index: 1,
                arguments: "true}".to_string(),
            },
        ] {
            bytes.extend(
                emitter
                    .emit(CanonicalStreamFrame {
                        id: "chatcmpl_sparse".to_string(),
                        model: "claude-opus-4-6".to_string(),
                        event,
                    })
                    .expect("tool event should encode"),
            );
        }

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert_eq!(openai_chat_tool_call_indices(&sse), vec![0, 0, 1, 1, 0]);
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
                            input_tokens_include_cache: true,
                            output_tokens: 2,
                            total_tokens: 3,
                            cache_creation_tokens: 5,
                            cache_read_tokens: 4,
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
        assert!(sse.contains("\"cache_write_tokens\":5"));
        assert!(!sse.contains("\"cached_creation_tokens\""));
        assert!(sse.contains("\"cached_tokens\":4"));
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
                            cache_creation_tokens: 5,
                            cache_read_tokens: 4,
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
        assert!(sse.contains("\"input_tokens_details\""));
        assert!(sse.contains("\"cache_write_tokens\":5"));
        assert!(!sse.contains("\"cached_creation_tokens\""));
        assert!(sse.contains("\"cached_tokens\":4"));
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
    fn openai_responses_client_emitter_closes_distinct_reasoning_parts() {
        let mut emitter = OpenAIResponsesClientEmitter::default();
        let mut bytes = emitter
            .emit(CanonicalStreamFrame {
                id: "resp_789".to_string(),
                model: "gpt-5.4".to_string(),
                event: CanonicalStreamEvent::Start,
            })
            .expect("start should encode");
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_789".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::ReasoningDelta("alpha".to_string()),
                })
                .expect("first reasoning should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_789".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::ReasoningSummaryDone,
                })
                .expect("first boundary should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_789".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::ReasoningDelta("beta".to_string()),
                })
                .expect("second reasoning should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_789".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::ReasoningSummaryDone,
                })
                .expect("second boundary should encode"),
        );
        bytes.extend(
            emitter
                .emit(CanonicalStreamFrame {
                    id: "resp_789".to_string(),
                    model: "gpt-5.4".to_string(),
                    event: CanonicalStreamEvent::Finish {
                        finish_reason: Some("stop".to_string()),
                        usage: None,
                    },
                })
                .expect("finish should encode"),
        );

        let sse = String::from_utf8(bytes).expect("sse should be utf8");
        assert_eq!(
            response_reasoning_text_done_parts(&sse),
            vec![(0, "alpha".to_string()), (1, "beta".to_string())]
        );
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

    #[test]
    fn openai_responses_provider_state_accepts_reasoning_done_without_delta() {
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
                            "id": "resp_done_only",
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
                        "type": "response.reasoning_summary_text.done",
                        "response_id": "resp_done_only",
                        "item_id": "resp_done_only_rs_0",
                        "output_index": 0,
                        "summary_index": 0,
                        "text": "fallback reasoning",
                    })),
                )
                .expect("reasoning done should parse"),
        );

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ReasoningDelta(ref text) if text == "fallback reasoning"
        )));
        assert!(frames
            .iter()
            .any(|frame| matches!(frame.event, CanonicalStreamEvent::ReasoningSummaryDone)));
    }

    #[test]
    fn openai_responses_provider_state_does_not_duplicate_part_scoped_reasoning_done() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        for event in [
            json!({
                "type": "response.reasoning_summary_text.delta",
                "response_id": "resp_parts",
                "item_id": "resp_parts_rs_0",
                "output_index": 0,
                "summary_index": 0,
                "delta": "alpha",
            }),
            json!({
                "type": "response.reasoning_summary_text.done",
                "response_id": "resp_parts",
                "item_id": "resp_parts_rs_0",
                "output_index": 0,
                "summary_index": 0,
                "text": "alpha",
            }),
            json!({
                "type": "response.reasoning_summary_text.delta",
                "response_id": "resp_parts",
                "item_id": "resp_parts_rs_0",
                "output_index": 0,
                "summary_index": 1,
                "delta": "beta",
            }),
            json!({
                "type": "response.reasoning_summary_text.done",
                "response_id": "resp_parts",
                "item_id": "resp_parts_rs_0",
                "output_index": 0,
                "summary_index": 1,
                "text": "beta",
            }),
        ] {
            frames.extend(
                state
                    .push_line(&report_context, data_line(event))
                    .expect("reasoning event should parse"),
            );
        }

        let reasoning = frames
            .iter()
            .filter_map(|frame| match &frame.event {
                CanonicalStreamEvent::ReasoningDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(reasoning, vec!["alpha", "beta"]);
    }

    #[test]
    fn openai_responses_provider_state_does_not_duplicate_added_reasoning_item_summary() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let mut frames = Vec::new();

        for event in [
            json!({
                "type": "response.output_item.added",
                "response_id": "resp_added_summary",
                "output_index": 0,
                "item": {
                    "type": "reasoning",
                    "id": "resp_added_summary_rs_0",
                    "summary": [{
                        "type": "summary_text",
                        "text": "alpha",
                    }]
                }
            }),
            json!({
                "type": "response.reasoning_summary_text.delta",
                "response_id": "resp_added_summary",
                "item_id": "resp_added_summary_rs_0",
                "output_index": 0,
                "summary_index": 0,
                "delta": "alpha",
            }),
        ] {
            frames.extend(
                state
                    .push_line(&report_context, data_line(event))
                    .expect("reasoning event should parse"),
            );
        }

        let reasoning = frames
            .iter()
            .filter_map(|frame| match &frame.event {
                CanonicalStreamEvent::ReasoningDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(reasoning, vec!["alpha"]);
    }

    #[test]
    fn openai_responses_provider_state_uses_reasoning_item_as_fallback() {
        let mut state = OpenAIResponsesProviderState::default();
        let report_context = json!({});
        let frames = state
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.output_item.done",
                    "response_id": "resp_item_fallback",
                    "output_index": 0,
                    "item": {
                        "type": "reasoning",
                        "id": "resp_item_fallback_rs_0",
                        "summary": [{
                            "type": "summary_text",
                            "text": "item fallback reasoning",
                        }]
                    }
                })),
            )
            .expect("reasoning item should parse");

        assert!(frames.iter().any(|frame| matches!(
            frame.event,
            CanonicalStreamEvent::ReasoningDelta(ref text) if text == "item fallback reasoning"
        )));
    }
}
