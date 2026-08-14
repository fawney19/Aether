use crate::provider_compat::kiro_stream::{
    build_kiro_final_message_sse_events, encode_kiro_sse_events,
    find_kiro_real_thinking_end_tag_at_buffer_end,
};

use crate::formats::shared::AiSurfaceFinalizeError;

use super::super::KiroClaudeStreamState;

impl KiroClaudeStreamState {
    pub(super) fn finalize(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut events = Vec::new();

        // 原生 reasoning 没有后续正文时也必须在 message_stop 前带真实 signature 结束。
        if self.native_thinking_block_open {
            events.extend(self.close_thinking_block());
        }

        if self.thinking_enabled && self.in_thinking_block && self.thinking_buffer.is_empty() {
            // 标签式 thinking 可能恰好在开始标签后结束流；仍需先发兼容签名再关闭。
            events.extend(self.close_thinking_block());
            self.in_thinking_block = false;
            self.thinking_extracted = true;
        }

        if self.thinking_enabled && !self.thinking_buffer.is_empty() {
            let buffered = std::mem::take(&mut self.thinking_buffer);
            if self.in_thinking_block {
                if let Some(end_pos) = find_kiro_real_thinking_end_tag_at_buffer_end(&buffered) {
                    let thinking_text = &buffered[..end_pos];
                    if !thinking_text.is_empty() {
                        events.extend(self.emit_thinking_delta(&thinking_text));
                    }
                    events.extend(self.close_thinking_block());
                    let remaining = &buffered[end_pos + "</thinking>".len()..];
                    if !remaining.is_empty() {
                        events.extend(self.emit_text_delta(&remaining));
                    }
                } else {
                    events.extend(self.emit_thinking_delta(&buffered));
                    events.extend(self.close_thinking_block());
                }
            } else {
                events.extend(self.emit_text_delta(&buffered));
            }
            self.in_thinking_block = false;
            self.thinking_extracted = true;
        }

        for idx in self
            .open_blocks
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            events.extend(self.close_block(idx));
        }
        let mut output = encode_kiro_sse_events(events).map_err(AiSurfaceFinalizeError::from)?;
        output.extend(self.final_message_bytes()?);
        Ok(output)
    }

    pub(super) fn final_message_bytes(&self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let stop_reason = self.stop_reason_override.clone().unwrap_or_else(|| {
            if self.has_tool_use {
                "tool_use"
            } else {
                "end_turn"
            }
            .to_string()
        });
        let input_tokens = if self.estimated_input_tokens > 0 {
            self.estimated_input_tokens
        } else {
            self.context_input_tokens.unwrap_or_default()
        };
        encode_kiro_sse_events(build_kiro_final_message_sse_events(
            &stop_reason,
            input_tokens,
            self.output_tokens,
            self.cache_usage,
            self.metering.as_ref(),
        ))
        .map_err(AiSurfaceFinalizeError::from)
    }
}
