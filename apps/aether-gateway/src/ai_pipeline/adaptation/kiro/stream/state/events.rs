use serde_json::{json, Value};

use crate::ai_pipeline::{
    calculate_kiro_context_input_tokens, encode_kiro_sse_events, estimate_kiro_tokens,
    find_kiro_real_thinking_end_tag, find_kiro_real_thinking_end_tag_at_buffer_end,
    find_kiro_real_thinking_start_tag, KIRO_MAX_THINKING_BUFFER,
};

use crate::GatewayError;

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
    pub(super) fn process_frame(&mut self, frame: AwsEventFrame) -> Result<Vec<u8>, GatewayError> {
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
    ) -> Result<Vec<u8>, GatewayError> {
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
                    let name = payload_object
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let tool_use_id = payload_object
                        .get("toolUseId")
                        .or_else(|| payload_object.get("tool_use_id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let input_json = match payload_object.get("input") {
                        None | Some(Value::Null) => String::new(),
                        Some(Value::String(text)) => text.clone(),
                        Some(other) => serde_json::to_string(other)
                            .map_err(|err| GatewayError::Internal(err.to_string()))?,
                    };
                    let stop = payload_object
                        .get("stop")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    events.extend(self.process_tool_use(name, tool_use_id, &input_json, stop));
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
            _ => {}
        }
        encode_kiro_sse_events(events).map_err(|err| GatewayError::Internal(err.to_string()))
    }

    pub(super) fn process_exception_frame(
        &mut self,
        frame: AwsEventFrame,
    ) -> Result<Vec<u8>, GatewayError> {
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
    ) -> Result<Vec<u8>, GatewayError> {
        let error_code = frame
            .headers
            .error_code()
            .unwrap_or("UnknownError")
            .to_string();
        self.emit_stream_error("upstream_error", &error_code)
    }

    pub(super) fn process_assistant_response(&mut self, content: &str) -> Vec<Value> {
        if content.is_empty() || content == self.last_content {
            return Vec::new();
        }
        self.last_content = content.to_string();
        self.output_tokens += estimate_kiro_tokens(content);

        if !self.thinking_enabled {
            return self.emit_text_delta(content);
        }

        self.thinking_buffer.push_str(content);
        if self.thinking_buffer.len() > KIRO_MAX_THINKING_BUFFER {
            let overflow = std::mem::take(&mut self.thinking_buffer);
            if self.in_thinking_block {
                let mut events = self.emit_thinking_delta(&overflow);
                events.extend(self.close_thinking_block());
                self.in_thinking_block = false;
                self.thinking_extracted = true;
                return events;
            }
            return self.emit_text_delta(&overflow);
        }

        let mut events = Vec::new();
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
        let mut events = Vec::new();

        if self.thinking_enabled && self.in_thinking_block && !self.thinking_buffer.is_empty() {
            if let Some(end_pos) =
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

        if let Some(idx) = self.text_block_index.take() {
            events.extend(self.close_block(idx));
        }

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
