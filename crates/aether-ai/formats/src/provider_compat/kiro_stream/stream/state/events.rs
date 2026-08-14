use serde_json::{json, Map, Value};

use crate::provider_compat::kiro_stream::{
    calculate_kiro_context_input_tokens, encode_kiro_sse_events, estimate_kiro_tokens,
    find_kiro_real_thinking_end_tag, find_kiro_real_thinking_end_tag_at_buffer_end,
    find_kiro_real_thinking_start_tag, KiroStreamMetering, KIRO_MAX_THINKING_BUFFER,
};

use crate::formats::shared::AiSurfaceFinalizeError;

use super::super::AwsEventFrame;
use super::super::KiroClaudeStreamState;

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut boundary = index.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn split_preserving_trailing_bytes(
    buffer: &str,
    trailing_bytes: usize,
) -> Option<(String, String)> {
    if buffer.len() <= trailing_bytes {
        return None;
    }

    let split = floor_char_boundary(buffer, buffer.len() - trailing_bytes);
    if split == 0 {
        return None;
    }

    Some((buffer[..split].to_string(), buffer[split..].to_string()))
}

impl KiroClaudeStreamState {
    pub(super) fn process_frame(
        &mut self,
        frame: AwsEventFrame,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let message_type = frame.headers.message_type().unwrap_or("event");
        match message_type {
            "event" => self.process_event_frame(frame),
            "exception" => self.process_exception_frame(frame),
            "error" => self.process_error_frame(frame),
            _ => Ok(Vec::new()),
        }
    }

    pub(super) fn process_event_frame(
        &mut self,
        frame: AwsEventFrame,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let event_type = frame.headers.event_type().unwrap_or_default();
        let payload: Value = if frame.payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&frame.payload).unwrap_or_else(|_| json!({}))
        };
        let payload_object = payload.as_object();
        let mut events = Vec::new();
        match event_type {
            "assistantResponseEvent" => {
                if let Some(content) = payload_object
                    .and_then(|value| value.get("content"))
                    .and_then(Value::as_str)
                {
                    events.extend(self.process_assistant_response(content));
                }
            }
            "toolUseEvent" => {
                if let Some(payload_object) = payload_object {
                    let nested_tool_use = payload_object.get("toolUse").and_then(Value::as_object);
                    let name = payload_object
                        .get("name")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            nested_tool_use
                                .and_then(|tool_use| tool_use.get("name").and_then(Value::as_str))
                        })
                        .unwrap_or_default();
                    let tool_use_id = payload_object
                        .get("toolUseId")
                        .or_else(|| payload_object.get("tool_use_id"))
                        .and_then(Value::as_str)
                        .or_else(|| {
                            nested_tool_use.and_then(|tool_use| {
                                tool_use
                                    .get("toolUseId")
                                    .or_else(|| tool_use.get("tool_use_id"))
                                    .and_then(Value::as_str)
                            })
                        })
                        .unwrap_or_default();
                    let input_json = tool_use_input_json(payload_object, name)?;
                    let stop = payload_object
                        .get("stop")
                        .and_then(Value::as_bool)
                        .or_else(|| {
                            nested_tool_use
                                .and_then(|tool_use| tool_use.get("stop"))
                                .and_then(Value::as_bool)
                        })
                        .unwrap_or(false);
                    events.extend(self.process_tool_use(name, tool_use_id, &input_json, stop));
                }
            }
            "reasoningContentEvent" => {
                if let Some(payload_object) = payload_object {
                    events.extend(self.process_reasoning_content(payload_object));
                }
            }
            "contextUsageEvent" => {
                if let Some(percentage) = payload_object
                    .and_then(|value| value.get("contextUsagePercentage"))
                    .and_then(Value::as_f64)
                {
                    self.context_input_tokens =
                        Some(calculate_kiro_context_input_tokens(percentage));
                }
            }
            "metadataEvent" => {
                if let Some(payload_object) = payload_object {
                    if let Some(stop_reason) = kiro_stop_reason(payload_object) {
                        self.stop_reason_override = Some(stop_reason);
                    }
                }
            }
            "meteringEvent" => {
                if let Some(payload_object) = payload_object {
                    if let Some(credit_usage) = payload_object.get("usage").and_then(Value::as_f64)
                    {
                        // Kiro 只下发 credit 计量；保留最近一条完整事件用于最终扩展字段。
                        self.metering = Some(KiroStreamMetering {
                            credit_usage,
                            credit_unit: payload_object
                                .get("unit")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            credit_unit_plural: payload_object
                                .get("unitPlural")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
        encode_kiro_sse_events(events).map_err(AiSurfaceFinalizeError::from)
    }

    pub(super) fn process_exception_frame(
        &mut self,
        frame: AwsEventFrame,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let exception_type = frame
            .headers
            .exception_type()
            .unwrap_or("UnknownException")
            .to_string();
        if exception_type == "ContentLengthExceededException" {
            self.stop_reason_override = Some("max_tokens".to_string());
            return Ok(Vec::new());
        }
        self.emit_stream_error("upstream_exception", &exception_type)
    }

    pub(super) fn process_error_frame(
        &mut self,
        frame: AwsEventFrame,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let error_code = frame
            .headers
            .error_code()
            .unwrap_or("UnknownError")
            .to_string();
        self.emit_stream_error("upstream_error", &error_code)
    }

    pub(super) fn process_assistant_response(&mut self, content: &str) -> Vec<Value> {
        if content.is_empty() {
            return Vec::new();
        }
        self.output_tokens += estimate_kiro_tokens(content);

        // 原生 reasoning 已独立成块，后续正文必须先关闭它，确保 signature_delta
        // 在新的 text/tool 块和最终 message_stop 之前发出。
        let mut events = self.close_native_thinking_before_next_content();

        if !self.thinking_enabled {
            events.extend(self.emit_text_delta(content));
            return events;
        }

        self.thinking_buffer.push_str(content);
        if self.thinking_buffer.len() > KIRO_MAX_THINKING_BUFFER {
            let overflow = std::mem::take(&mut self.thinking_buffer);
            if self.in_thinking_block {
                events.extend(self.emit_thinking_delta(&overflow));
                events.extend(self.close_thinking_block());
                self.in_thinking_block = false;
                self.thinking_extracted = true;
                return events;
            }
            events.extend(self.emit_text_delta(&overflow));
            return events;
        }

        loop {
            if !self.in_thinking_block && !self.thinking_extracted {
                if let Some(start_pos) = find_kiro_real_thinking_start_tag(&self.thinking_buffer) {
                    let before = self.thinking_buffer[..start_pos].to_string();
                    if !before.trim().is_empty() {
                        events.extend(self.emit_text_delta(&before));
                    }
                    self.in_thinking_block = true;
                    self.strip_thinking_leading_newline = true;
                    self.thinking_buffer =
                        self.thinking_buffer[start_pos + "<thinking>".len()..].to_string();
                    events.extend(self.ensure_thinking_block_open());
                    continue;
                }

                let keep = "<thinking>".len();
                if let Some((safe, remaining)) =
                    split_preserving_trailing_bytes(&self.thinking_buffer, keep)
                {
                    if !safe.trim().is_empty() {
                        events.extend(self.emit_text_delta(&safe));
                        self.thinking_buffer = remaining;
                    }
                }
                break;
            }

            if self.in_thinking_block {
                if self.strip_thinking_leading_newline {
                    if self.thinking_buffer.starts_with('\n') {
                        self.thinking_buffer.remove(0);
                        self.strip_thinking_leading_newline = false;
                    } else if !self.thinking_buffer.is_empty() {
                        self.strip_thinking_leading_newline = false;
                    }
                }

                if let Some(end_pos) = find_kiro_real_thinking_end_tag(&self.thinking_buffer) {
                    let thinking_text = self.thinking_buffer[..end_pos].to_string();
                    if !thinking_text.is_empty() {
                        events.extend(self.emit_thinking_delta(&thinking_text));
                    }
                    events.extend(self.close_thinking_block());
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;
                    self.thinking_buffer =
                        self.thinking_buffer[end_pos + "</thinking>".len()..].to_string();
                    continue;
                }

                let keep = "</thinking>".len();
                if let Some((safe, remaining)) =
                    split_preserving_trailing_bytes(&self.thinking_buffer, keep)
                {
                    if !safe.is_empty() {
                        events.extend(self.emit_thinking_delta(&safe));
                        self.thinking_buffer = remaining;
                    }
                }
                break;
            }

            if !self.thinking_buffer.is_empty() {
                let remaining = std::mem::take(&mut self.thinking_buffer);
                events.extend(self.emit_text_delta(&remaining));
            }
            break;
        }

        events
    }

    pub(super) fn process_reasoning_content(&mut self, payload: &Map<String, Value>) -> Vec<Value> {
        let text = payload.get("text").and_then(Value::as_str);

        if !self.thinking_enabled {
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                self.output_tokens += estimate_kiro_tokens(text);
                return self.emit_text_delta(text);
            }
            return Vec::new();
        }

        if let Some(signature) = payload
            .get("signature")
            .and_then(Value::as_str)
            .filter(|signature| !signature.is_empty())
        {
            // 签名可先于正文抵达，因此暂存并只留给下一个原生 reasoning 块。
            self.pending_thinking_signature = Some(signature.to_string());
        }

        let Some(text) = text.filter(|text| !text.is_empty()) else {
            return Vec::new();
        };

        let mut events = Vec::new();
        if !self.native_thinking_block_open {
            events.extend(self.flush_assistant_buffer_before_native_thinking());
            // Claude content block 不允许 text 与 thinking 交错开放；先结束已有正文。
            events.extend(self.close_text_block());
            self.thinking_extracted = true;
            self.native_thinking_block_open = true;
            events.extend(self.ensure_thinking_block_open());
        }
        self.output_tokens += estimate_kiro_tokens(text);
        events.extend(self.emit_thinking_delta(text));
        events
    }

    fn close_native_thinking_before_next_content(&mut self) -> Vec<Value> {
        if self.native_thinking_block_open {
            self.close_thinking_block()
        } else {
            Vec::new()
        }
    }

    fn flush_assistant_buffer_before_native_thinking(&mut self) -> Vec<Value> {
        let mut events = Vec::new();
        if self.in_thinking_block {
            let thinking = std::mem::take(&mut self.thinking_buffer);
            if !thinking.is_empty() {
                events.extend(self.emit_thinking_delta(&thinking));
            }
            // 这是标签式 thinking，close_thinking_block 会使用兼容签名，保留原生签名。
            events.extend(self.close_thinking_block());
            self.in_thinking_block = false;
            self.strip_thinking_leading_newline = false;
        } else if !self.thinking_buffer.is_empty() {
            let buffered = std::mem::take(&mut self.thinking_buffer);
            events.extend(self.emit_text_delta(&buffered));
        }
        events
    }

    pub(super) fn process_tool_use(
        &mut self,
        name: &str,
        tool_use_id: &str,
        input_json: &str,
        stop: bool,
    ) -> Vec<Value> {
        if tool_use_id.is_empty() {
            return Vec::new();
        }

        self.has_tool_use = true;
        let mut events = self.close_native_thinking_before_next_content();

        if self.thinking_enabled && self.in_thinking_block {
            if self.thinking_buffer.is_empty() {
                // 即使还没收到标签正文，工具块也不能与 thinking 块同时开放。
                events.extend(self.close_thinking_block());
                self.in_thinking_block = false;
                self.thinking_extracted = true;
            } else if let Some(end_pos) =
                find_kiro_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer)
            {
                let thinking_text = self.thinking_buffer[..end_pos].to_string();
                if !thinking_text.is_empty() {
                    events.extend(self.emit_thinking_delta(&thinking_text));
                }
                events.extend(self.close_thinking_block());
                let remaining = self.thinking_buffer[end_pos + "</thinking>".len()..].to_string();
                self.thinking_buffer.clear();
                self.in_thinking_block = false;
                self.thinking_extracted = true;
                if !remaining.is_empty() {
                    events.extend(self.emit_text_delta(&remaining));
                }
            } else {
                let thinking = std::mem::take(&mut self.thinking_buffer);
                events.extend(self.emit_thinking_delta(&thinking));
                events.extend(self.close_thinking_block());
                self.in_thinking_block = false;
                self.thinking_extracted = true;
            }
        }

        if self.thinking_enabled
            && !self.in_thinking_block
            && !self.thinking_extracted
            && !self.thinking_buffer.is_empty()
        {
            let buffered = std::mem::take(&mut self.thinking_buffer);
            events.extend(self.emit_text_delta(&buffered));
        }

        events.extend(self.close_text_block());

        let block_index = if let Some(block_index) = self.tool_block_indices.get(tool_use_id) {
            *block_index
        } else {
            let block_index = self.next_block_index;
            self.next_block_index += 1;
            self.tool_block_indices
                .insert(tool_use_id.to_string(), block_index);
            block_index
        };

        if let std::collections::btree_map::Entry::Vacant(e) = self.open_blocks.entry(block_index) {
            e.insert("tool_use".to_string());
            events.push(json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": "tool_use",
                    "id": tool_use_id,
                    "name": name,
                    "input": {},
                }
            }));
        }

        if !input_json.is_empty() {
            self.output_tokens += estimate_kiro_tokens(input_json);
            events.push(json!({
                "type": "content_block_delta",
                "index": block_index,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": input_json,
                }
            }));
        }

        if stop {
            events.extend(self.close_block(block_index));
        }

        events
    }
}

fn tool_use_input_json(
    payload: &Map<String, Value>,
    tool_name: &str,
) -> Result<String, AiSurfaceFinalizeError> {
    // Runtime 事件规范使用 input；兼容旧事件和中间代理使用的 arguments/嵌套 toolUse。
    // Kiro 有时会先写入占位 input: {}，再将实际参数放在 arguments；空对象不能
    // 抢占后者，否则 Read 等工具会收到缺少必填字段的空参数。
    let input = payload
        .get("input")
        .filter(|value| tool_use_input_has_value(value))
        .or_else(|| {
            payload
                .get("arguments")
                .filter(|value| tool_use_input_has_value(value))
        })
        .or_else(|| {
            payload
                .get("toolUse")
                .and_then(Value::as_object)
                .and_then(|tool_use| {
                    tool_use
                        .get("input")
                        .filter(|value| tool_use_input_has_value(value))
                        .or_else(|| {
                            tool_use
                                .get("arguments")
                                .filter(|value| tool_use_input_has_value(value))
                        })
                })
        });

    match input {
        None => Ok(String::new()),
        Some(Value::String(text)) => restore_kiro_read_tool_input_json(tool_name, text),
        Some(value) => {
            serde_json::to_string(&restore_kiro_read_tool_input(tool_name, value.clone()))
                .map_err(AiSurfaceFinalizeError::from)
        }
    }
}

fn tool_use_input_has_value(value: &&Value) -> bool {
    !value.is_null()
        && !value.as_object().is_some_and(Map::is_empty)
        && !value.as_str().is_some_and(|text| text.trim().is_empty())
}

fn restore_kiro_read_tool_input_json(
    tool_name: &str,
    input_json: &str,
) -> Result<String, AiSurfaceFinalizeError> {
    if !is_kiro_read_tool(tool_name) {
        return Ok(input_json.to_string());
    }

    // 流式参数片段通常不是完整 JSON；解析失败时必须原样转发，不能提前改写。
    let Ok(value) = serde_json::from_str::<Value>(input_json) else {
        return Ok(input_json.to_string());
    };
    if !value.is_object() {
        return Ok(input_json.to_string());
    }
    serde_json::to_string(&restore_kiro_read_tool_input(tool_name, value))
        .map_err(AiSurfaceFinalizeError::from)
}

fn restore_kiro_read_tool_input(tool_name: &str, mut input: Value) -> Value {
    let Some(kiro_path_key) = kiro_read_tool_path_key(tool_name) else {
        return input;
    };
    let Some(object) = input.as_object_mut() else {
        return input;
    };
    if !object.contains_key("file_path") {
        if let Some(file_path) = object.remove(kiro_path_key) {
            object.insert("file_path".to_string(), file_path);
        }
    }
    input
}

fn is_kiro_read_tool(tool_name: &str) -> bool {
    kiro_read_tool_path_key(tool_name).is_some()
}

fn kiro_read_tool_path_key(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "Read" | "read" => Some("filePath"),
        "read_file" | "readFile" => Some("path"),
        _ => None,
    }
}

fn kiro_stop_reason(payload: &Map<String, Value>) -> Option<String> {
    let stop_reason = payload
        .get("stopReason")
        .or_else(|| payload.get("stop_reason"))
        .and_then(Value::as_str)?;
    match stop_reason.trim().to_ascii_uppercase().as_str() {
        "END_TURN" => Some("end_turn".to_string()),
        "MAX_TOKENS" => Some("max_tokens".to_string()),
        "TOOL_USE" => Some("tool_use".to_string()),
        "STOP_SEQUENCE" => Some("stop_sequence".to_string()),
        _ => None,
    }
}
