use serde_json::Value;
use uuid::Uuid;

use crate::provider_compat::kiro_stream::{
    build_kiro_initial_sse_events, build_kiro_stream_error_sse_events, encode_kiro_sse_events,
};
use crate::response::AiSurfaceFinalizeError;

use super::super::{EventStreamDecoder, KiroClaudeStreamState, KiroToClaudeCliStreamState};

impl KiroToClaudeCliStreamState {
    pub fn new(report_context: &Value) -> Self {
        Self {
            decoder: EventStreamDecoder::default(),
            state: KiroClaudeStreamState::new(report_context),
            started: false,
        }
    }

    pub fn push_chunk(
        &mut self,
        _report_context: &Value,
        chunk: &[u8],
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut output = Vec::new();
        if !self.started {
            self.started = true;
            output.extend(self.state.generate_initial_bytes()?);
        }

        if let Err(err) = self.decoder.feed(chunk) {
            output.extend(
                self.state
                    .emit_stream_error("upstream_stream_error", &err)?,
            );
            return Ok(output);
        }

        match self.decoder.decode_available() {
            Ok(frames) => {
                for frame in frames {
                    output.extend(self.state.process_frame(frame)?);
                }
            }
            Err(err) => {
                output.extend(
                    self.state
                        .emit_stream_error("upstream_stream_error", &err)?,
                );
            }
        }

        Ok(output)
    }

    pub fn finish(&mut self, _report_context: &Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if !self.started || self.state.had_error {
            return Ok(Vec::new());
        }
        self.state.finalize()
    }
}

impl KiroClaudeStreamState {
    pub(super) fn new(report_context: &Value) -> Self {
        let model = report_context
            .get("mapped_model")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                report_context
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or("unknown")
            .to_string();
        let thinking_enabled = report_context
            .get("original_request_body")
            .and_then(Value::as_object)
            .and_then(|body| body.get("thinking"))
            .and_then(Value::as_object)
            .and_then(|thinking| thinking.get("type"))
            .and_then(Value::as_str)
            .map(|value| {
                value.trim().eq_ignore_ascii_case("enabled")
                    || value.trim().eq_ignore_ascii_case("adaptive")
            })
            .unwrap_or(false);
        let estimated_input_tokens = report_context
            .get("input_tokens")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(0);
        Self {
            model,
            thinking_enabled,
            estimated_input_tokens,
            message_id: format!("msg_{}", Uuid::new_v4().simple()),
            ..Self::default()
        }
    }

    pub(super) fn generate_initial_bytes(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let events = build_kiro_initial_sse_events(
            &self.message_id,
            &self.model,
            self.estimated_input_tokens,
        );
        let mut events = events;
        if !self.thinking_enabled {
            events.extend(self.ensure_text_block_open());
        }
        encode_kiro_sse_events(events).map_err(AiSurfaceFinalizeError::from)
    }

    pub(super) fn emit_stream_error(
        &mut self,
        error_type: &str,
        message: &str,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.had_error {
            return Ok(Vec::new());
        }
        self.had_error = true;
        encode_kiro_sse_events(build_kiro_stream_error_sse_events(error_type, message))
            .map_err(AiSurfaceFinalizeError::from)
    }
}
