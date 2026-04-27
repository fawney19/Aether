use serde_json::Value;

use crate::ai_pipeline::adaptation::private_envelope::transform_provider_private_stream_line as transform_envelope_line;
use crate::ai_pipeline::adaptation::KiroToClaudeCliStreamState;
use crate::ai_pipeline::finalize::sse::encode_json_sse;
use crate::ai_pipeline::finalize::standard::StreamingStandardConversionState;
use crate::ai_pipeline::{resolve_finalize_stream_rewrite_mode, FinalizeStreamRewriteMode};
use crate::GatewayError;

enum RewriteMode {
    EnvelopeUnwrap,
    OpenAiImage(OpenAiImageStreamState),
    Standard(StreamingStandardConversionState),
    KiroToClaudeCli(KiroToClaudeCliStreamState),
    KiroToClaudeCliThenStandard {
        kiro: KiroToClaudeCliStreamState,
        standard: StreamingStandardConversionState,
    },
}

pub(crate) struct LocalStreamRewriter<'a> {
    report_context: &'a Value,
    buffered: Vec<u8>,
    mode: RewriteMode,
}

pub(crate) fn maybe_build_local_stream_rewriter<'a>(
    report_context: Option<&'a Value>,
) -> Option<LocalStreamRewriter<'a>> {
    let report_context = report_context?;
    let mode = match resolve_finalize_stream_rewrite_mode(report_context)? {
        FinalizeStreamRewriteMode::EnvelopeUnwrap => RewriteMode::EnvelopeUnwrap,
        FinalizeStreamRewriteMode::OpenAiImage => {
            RewriteMode::OpenAiImage(OpenAiImageStreamState::default())
        }
        FinalizeStreamRewriteMode::Standard => {
            RewriteMode::Standard(StreamingStandardConversionState::default())
        }
        FinalizeStreamRewriteMode::KiroToClaudeCli => {
            RewriteMode::KiroToClaudeCli(KiroToClaudeCliStreamState::new(report_context))
        }
        FinalizeStreamRewriteMode::KiroToClaudeCliThenStandard => {
            RewriteMode::KiroToClaudeCliThenStandard {
                kiro: KiroToClaudeCliStreamState::new(report_context),
                standard: StreamingStandardConversionState::default(),
            }
        }
    };

    Some(LocalStreamRewriter {
        report_context,
        buffered: Vec::new(),
        mode,
    })
}

impl LocalStreamRewriter<'_> {
    pub(crate) fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<u8>, GatewayError> {
        if let RewriteMode::OpenAiImage(state) = &mut self.mode {
            return state.push_chunk(self.report_context, chunk);
        }
        if let RewriteMode::KiroToClaudeCli(state) = &mut self.mode {
            return state.push_chunk(self.report_context, chunk);
        }
        if let RewriteMode::KiroToClaudeCliThenStandard { kiro, standard } = &mut self.mode {
            let claude_bytes = kiro.push_chunk(self.report_context, chunk)?;
            return transform_standard_bytes(standard, self.report_context, claude_bytes);
        }
        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(line_end) = self.buffered.iter().position(|byte| *byte == b'\n') {
            let line = self.buffered.drain(..=line_end).collect::<Vec<_>>();
            output.extend(self.transform_line(line)?);
        }
        Ok(output)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<u8>, GatewayError> {
        if let RewriteMode::OpenAiImage(state) = &mut self.mode {
            return state.finish(self.report_context);
        }
        if let RewriteMode::KiroToClaudeCli(state) = &mut self.mode {
            return state.finish(self.report_context);
        }
        if let RewriteMode::KiroToClaudeCliThenStandard { kiro, standard } = &mut self.mode {
            let mut output = transform_standard_bytes(
                standard,
                self.report_context,
                kiro.finish(self.report_context)?,
            )?;
            output.extend(standard.finish(self.report_context)?);
            return Ok(output);
        }
        if self.buffered.is_empty() {
            match &mut self.mode {
                RewriteMode::Standard(state) => return state.finish(self.report_context),
                RewriteMode::OpenAiImage(_) => {}
                RewriteMode::KiroToClaudeCli(_) => {}
                RewriteMode::KiroToClaudeCliThenStandard { .. } => {}
                RewriteMode::EnvelopeUnwrap => {}
            }
            return Ok(Vec::new());
        }
        let line = std::mem::take(&mut self.buffered);
        let mut output = self.transform_line(line)?;
        match &mut self.mode {
            RewriteMode::Standard(state) => {
                output.extend(state.finish(self.report_context)?);
            }
            RewriteMode::OpenAiImage(_) => {}
            RewriteMode::KiroToClaudeCli(_) => {}
            RewriteMode::KiroToClaudeCliThenStandard { .. } => {}
            RewriteMode::EnvelopeUnwrap => {}
        }
        Ok(output)
    }

    fn transform_line(&mut self, line: Vec<u8>) -> Result<Vec<u8>, GatewayError> {
        match &mut self.mode {
            RewriteMode::EnvelopeUnwrap => transform_envelope_line(self.report_context, line)
                .map_err(|err| GatewayError::Internal(err.to_string())),
            RewriteMode::OpenAiImage(_) => Ok(Vec::new()),
            RewriteMode::Standard(state) => state.transform_line(self.report_context, line),
            RewriteMode::KiroToClaudeCli(_) => Ok(Vec::new()),
            RewriteMode::KiroToClaudeCliThenStandard { .. } => Ok(Vec::new()),
        }
    }
}

fn transform_standard_bytes(
    standard: &mut StreamingStandardConversionState,
    report_context: &Value,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, GatewayError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        output.extend(standard.transform_line(report_context, line.to_vec())?);
    }
    Ok(output)
}

#[derive(Default)]
struct OpenAiImageStreamState {
    buffered: Vec<u8>,
    latest_image: Option<OpenAiImageFrame>,
    emitted_partial_count: u64,
    saw_upstream_partial: bool,
    emitted_failure: bool,
}

#[derive(Clone)]
struct OpenAiImageFrame {
    b64_json: String,
}

impl OpenAiImageStreamState {
    fn push_chunk(
        &mut self,
        report_context: &Value,
        chunk: &[u8],
    ) -> Result<Vec<u8>, GatewayError> {
        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(block_end) = find_sse_block_end(&self.buffered) {
            let block = self.buffered.drain(..block_end).collect::<Vec<_>>();
            output.extend(self.transform_block(report_context, &block)?);
            drain_sse_separator(&mut self.buffered);
        }
        Ok(output)
    }

    fn finish(&mut self, report_context: &Value) -> Result<Vec<u8>, GatewayError> {
        if self.buffered.is_empty() {
            return Ok(Vec::new());
        }
        let block = std::mem::take(&mut self.buffered);
        self.transform_block(report_context, &block)
    }

    fn transform_block(
        &mut self,
        report_context: &Value,
        block: &[u8],
    ) -> Result<Vec<u8>, GatewayError> {
        let text =
            std::str::from_utf8(block).map_err(|err| GatewayError::Internal(err.to_string()))?;
        let mut event_name = None::<String>;
        let mut data_lines = Vec::new();
        for raw_line in text.lines() {
            let line = raw_line.trim_end_matches('\r');
            if let Some(value) = line.strip_prefix("event:") {
                event_name = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                data_lines.push(value.trim().to_string());
            }
        }
        let data = data_lines.join("\n");
        if data.is_empty() || data == "[DONE]" {
            return Ok(Vec::new());
        }
        let event: Value =
            serde_json::from_str(&data).map_err(|err| GatewayError::Internal(err.to_string()))?;
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .or(event_name.as_deref())
            .unwrap_or_default();
        match event_type {
            "error" | "response.failed" => self.handle_failed(report_context, &event),
            "response.image_generation_call.partial_image" => {
                self.handle_image_generation_partial(report_context, &event)
            }
            "response.output_item.done" => self.handle_output_item_done(report_context, &event),
            "response.completed" => self.handle_completed(report_context, &event),
            _ => Ok(Vec::new()),
        }
    }

    fn handle_image_generation_partial(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, GatewayError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        if requested_partial_images(report_context) == 0 {
            return Ok(Vec::new());
        }
        let Some(result) = event
            .get("partial_image_b64")
            .or_else(|| event.get("b64_json"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(Vec::new());
        };
        let partial_image_index = event
            .get("partial_image_index")
            .or_else(|| event.get("output_index"))
            .and_then(Value::as_u64)
            .unwrap_or(self.emitted_partial_count);
        self.emitted_partial_count = self
            .emitted_partial_count
            .max(partial_image_index.saturating_add(1));
        self.saw_upstream_partial = true;
        self.latest_image = Some(OpenAiImageFrame {
            b64_json: result.to_string(),
        });

        encode_json_sse(
            Some(image_partial_event_name(report_context)),
            &serde_json::json!({
                "type": image_partial_event_name(report_context),
                "b64_json": result,
                "partial_image_index": partial_image_index,
            }),
        )
    }

    fn handle_output_item_done(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, GatewayError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        let Some(item) = event.get("item").and_then(Value::as_object) else {
            return Ok(Vec::new());
        };
        if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
            return Ok(Vec::new());
        }
        let Some(result) = item.get("result").and_then(Value::as_str).map(str::trim) else {
            return Ok(Vec::new());
        };
        if result.is_empty() {
            return Ok(Vec::new());
        }
        self.latest_image = Some(OpenAiImageFrame {
            b64_json: result.to_string(),
        });

        if requested_partial_images(report_context) == 0 || self.saw_upstream_partial {
            return Ok(Vec::new());
        }

        let partial_image_index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or(self.emitted_partial_count);
        self.emitted_partial_count = partial_image_index.saturating_add(1);

        encode_json_sse(
            Some(image_partial_event_name(report_context)),
            &serde_json::json!({
                "type": image_partial_event_name(report_context),
                "b64_json": result,
                "partial_image_index": partial_image_index,
            }),
        )
    }

    fn handle_completed(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, GatewayError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        if self.latest_image.is_none() {
            if let Some(result) = completed_response_image_result(event) {
                self.latest_image = Some(OpenAiImageFrame {
                    b64_json: result.to_string(),
                });
            }
        }
        let Some(latest_image) = self.latest_image.clone() else {
            return Ok(Vec::new());
        };
        let usage = event
            .get("response")
            .and_then(Value::as_object)
            .and_then(|response| {
                response
                    .get("tool_usage")
                    .and_then(|value| value.get("image_gen"))
                    .cloned()
                    .or_else(|| response.get("usage").cloned())
            })
            .unwrap_or(Value::Null);

        encode_json_sse(
            Some(image_completed_event_name(report_context)),
            &serde_json::json!({
                "type": image_completed_event_name(report_context),
                "b64_json": latest_image.b64_json,
                "usage": usage,
            }),
        )
    }

    fn handle_failed(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, GatewayError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        self.emitted_failure = true;
        let error = image_failure_error(event);
        encode_json_sse(
            Some(image_failed_event_name(report_context)),
            &serde_json::json!({
                "type": image_failed_event_name(report_context),
                "error": error,
            }),
        )
    }
}

fn image_failure_error(event: &Value) -> Value {
    let mut error = event
        .get("error")
        .or_else(|| event.get("response").and_then(|value| value.get("error")))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    if !error.contains_key("message") {
        if let Some(message) = event
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| {
                event
                    .get("response")
                    .and_then(|value| value.get("error"))
                    .and_then(|value| value.get("message"))
                    .and_then(Value::as_str)
            })
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            error.insert("message".to_string(), Value::String(message.to_string()));
        }
    }
    if !error.contains_key("code") {
        if let Some(code) = event
            .get("code")
            .or_else(|| {
                event
                    .get("response")
                    .and_then(|value| value.get("error"))
                    .and_then(|value| value.get("code"))
            })
            .cloned()
        {
            error.insert("code".to_string(), code);
        }
    }
    if !error.contains_key("type") {
        let inferred_type = error
            .get("code")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("upstream_error");
        error.insert("type".to_string(), Value::String(inferred_type.to_string()));
    }
    if !error.contains_key("message") {
        error.insert(
            "message".to_string(),
            Value::String("Image generation failed".to_string()),
        );
    }

    Value::Object(error)
}

fn completed_response_image_result(event: &Value) -> Option<&str> {
    event
        .get("response")
        .and_then(|value| value.get("output"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("image_generation_call"))
        .filter_map(|item| item.get("result").and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn requested_partial_images(report_context: &Value) -> u64 {
    report_context
        .get("image_request")
        .and_then(|value| value.get("partial_images"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn image_partial_event_name(report_context: &Value) -> &'static str {
    if image_request_operation(report_context) == Some("edit") {
        "image_edit.partial_image"
    } else {
        "image_generation.partial_image"
    }
}

fn image_completed_event_name(report_context: &Value) -> &'static str {
    if image_request_operation(report_context) == Some("edit") {
        "image_edit.completed"
    } else {
        "image_generation.completed"
    }
}

fn image_failed_event_name(report_context: &Value) -> &'static str {
    if image_request_operation(report_context) == Some("edit") {
        "image_edit.failed"
    } else {
        "image_generation.failed"
    }
}

fn image_request_operation(report_context: &Value) -> Option<&str> {
    report_context
        .get("image_request")
        .and_then(|value| value.get("operation"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn find_sse_block_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| index + 2)
        .or_else(|| {
            buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
        })
}

fn drain_sse_separator(buffer: &mut Vec<u8>) {
    while matches!(buffer.first(), Some(b'\n' | b'\r')) {
        buffer.remove(0);
    }
}

#[cfg(test)]
#[path = "../tests_stream.rs"]
mod tests;
