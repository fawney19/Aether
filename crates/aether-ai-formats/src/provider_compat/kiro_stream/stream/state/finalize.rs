use crate::provider_compat::kiro_stream::{
    build_kiro_final_message_sse_events, encode_kiro_sse_events,
    find_kiro_real_thinking_end_tag_at_buffer_end,
};

use crate::response::AiSurfaceFinalizeError;

use super::super::KiroClaudeStreamState;

impl KiroClaudeStreamState {
    pub(super) fn finalize(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.thinking_enabled && !self.thinking_buffer.is_empty() {
            let flush_events = if self.in_thinking_block {
                if let Some(end_pos) =
                    find_kiro_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer)
                {
                    let thinking_text = self.thinking_buffer[..end_pos].to_string();
                    let mut events = Vec::new();
                    if !thinking_text.is_empty() {
                        events.extend(self.emit_thinking_delta(&thinking_text));
                    }
                    events.extend(self.close_thinking_block());
                    let remaining =
                        self.thinking_buffer[end_pos + "</thinking>".len()..].to_string();
                    if !remaining.is_empty() {
                        events.extend(self.emit_text_delta(&remaining));
                    }
                    events
                } else {
                    let mut events = self.emit_thinking_delta(&self.thinking_buffer.clone());
                    events.extend(self.close_thinking_block());
                    events
                }
            } else {
                self.emit_text_delta(&self.thinking_buffer.clone())
            };
            self.thinking_buffer.clear();
            self.in_thinking_block = false;
            self.thinking_extracted = true;
            let mut output =
                encode_kiro_sse_events(flush_events).map_err(AiSurfaceFinalizeError::from)?;
            for idx in self
                .open_blocks
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                output.extend(
                    encode_kiro_sse_events(self.close_block(idx))
                        .map_err(AiSurfaceFinalizeError::from)?,
                );
            }
            output.extend(self.final_message_bytes()?);
            return Ok(output);
        }

        let mut output = Vec::new();
        for idx in self
            .open_blocks
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            output.extend(
                encode_kiro_sse_events(self.close_block(idx))
                    .map_err(AiSurfaceFinalizeError::from)?,
            );
        }
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
        let input_tokens = self
            .context_input_tokens
            .unwrap_or(self.estimated_input_tokens) as u64;
        encode_kiro_sse_events(build_kiro_final_message_sse_events(
            &stop_reason,
            input_tokens as usize,
            self.output_tokens,
        ))
        .map_err(AiSurfaceFinalizeError::from)
    }
}
