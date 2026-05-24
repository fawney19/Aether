use std::collections::BTreeSet;

use aether_contracts::{ExecutionStreamTerminalSummary, StandardizedUsage};
use base64::Engine as _;
use serde_json::{Map, Value};

use crate::contracts::OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND;
use crate::formats::openai::responses::codex::CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT;
use crate::formats::shared::sse::{encode_done_sse, encode_json_sse};
use crate::formats::shared::stream_core::common::{
    build_openai_chat_chunk, build_openai_chat_finish_chunk,
    build_openai_chat_usage_chunk_with_cache,
};
use crate::formats::shared::AiSurfaceFinalizeError;

#[derive(Default)]
pub struct OpenAiImageStreamState {
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

#[derive(Default)]
pub struct OpenAiImageChatStreamState {
    buffered: Vec<u8>,
    response_id: Option<String>,
    model: Option<String>,
    latest_image: Option<OpenAiImageChatFrame>,
    emitted_image_count: u64,
    emitted_image_keys: BTreeSet<String>,
    started: bool,
    finished: bool,
    emitted_failure: bool,
}

#[derive(Clone)]
struct OpenAiImageChatFrame {
    b64_json: String,
    output_format: Option<String>,
}

#[derive(Default)]
pub struct OpenAiImageStreamTerminalState {
    event_name: Option<String>,
    data_lines: Vec<String>,
    response_id: Option<String>,
    model: Option<String>,
    image_count: u64,
    image_keys: BTreeSet<String>,
    usage: Option<Value>,
    observed_finish: bool,
    parser_error: Option<String>,
}

impl OpenAiImageStreamState {
    pub fn push_chunk(
        &mut self,
        report_context: &Value,
        chunk: &[u8],
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(block_end) = find_sse_block_end(&self.buffered) {
            let block = self.buffered.drain(..block_end).collect::<Vec<_>>();
            output.extend(self.transform_block(report_context, &block)?);
            drain_sse_separator(&mut self.buffered);
        }
        Ok(output)
    }

    pub fn finish(&mut self, report_context: &Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let text = std::str::from_utf8(block)
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?;
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
        let event: Value = serde_json::from_str(&data)?;
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
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
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

impl OpenAiImageChatStreamState {
    pub fn push_chunk(
        &mut self,
        report_context: &Value,
        chunk: &[u8],
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(block_end) = find_sse_block_end(&self.buffered) {
            let block = self.buffered.drain(..block_end).collect::<Vec<_>>();
            output.extend(self.transform_block(report_context, &block)?);
            drain_sse_separator(&mut self.buffered);
        }
        Ok(output)
    }

    pub fn finish(&mut self, report_context: &Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let mut output = if self.buffered.is_empty() {
            Vec::new()
        } else {
            let block = std::mem::take(&mut self.buffered);
            self.transform_block(report_context, &block)?
        };
        if !self.finished && !self.emitted_failure && self.latest_image.is_some() {
            output.extend(self.emit_final(report_context, None)?);
        }
        Ok(output)
    }

    fn transform_block(
        &mut self,
        report_context: &Value,
        block: &[u8],
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let text = std::str::from_utf8(block)
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?;
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
        let event: Value = serde_json::from_str(&data)?;
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .or(event_name.as_deref())
            .unwrap_or_default();
        match event_type {
            "error" | "response.failed" | "image_generation.failed" | "image_edit.failed" => {
                self.handle_failed(report_context, &event)
            }
            "response.image_generation_call.partial_image" => {
                self.emit_empty_progress_chunk(report_context)
            }
            "response.output_item.done" => self.handle_output_item_done(report_context, &event),
            "response.completed" | "response.done" => self.handle_completed(report_context, &event),
            "image_generation.completed" | "image_edit.completed" => {
                self.handle_image_completed(report_context, &event)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn handle_output_item_done(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.finished || self.emitted_failure {
            return Ok(Vec::new());
        }
        let Some(item) = event.get("item").and_then(Value::as_object) else {
            return Ok(Vec::new());
        };
        if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
            return Ok(Vec::new());
        }
        if let Some(result) = item.get("result").and_then(Value::as_str).map(str::trim) {
            if !result.is_empty() {
                let key = image_chat_output_key(item, result);
                if self.emitted_image_keys.insert(key) {
                    self.latest_image = Some(OpenAiImageChatFrame {
                        b64_json: result.to_string(),
                        output_format: item
                            .get("output_format")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(ToOwned::to_owned),
                    });
                    self.emitted_image_count = self.emitted_image_count.saturating_add(1);
                }
            }
        }
        self.ensure_started(report_context)
    }

    fn handle_completed(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.finished || self.emitted_failure {
            return Ok(Vec::new());
        }
        if let Some(response) = event.get("response") {
            self.update_identity_from_response(response);
            if self.latest_image.is_none() {
                if let Some(frame) = completed_response_image_chat_frame(response) {
                    self.latest_image = Some(frame);
                    self.emitted_image_count = self.emitted_image_count.saturating_add(1);
                }
            }
        }
        let usage = event
            .get("response")
            .and_then(Value::as_object)
            .and_then(|response| {
                response
                    .get("tool_usage")
                    .and_then(|value| value.get("image_gen"))
                    .cloned()
                    .or_else(|| response.get("usage").cloned())
            });
        self.emit_final(report_context, usage.as_ref())
    }

    fn handle_image_completed(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.finished || self.emitted_failure {
            return Ok(Vec::new());
        }
        if let Some(result) = event
            .get("b64_json")
            .or_else(|| event.get("result"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.latest_image = Some(OpenAiImageChatFrame {
                b64_json: result.to_string(),
                output_format: event
                    .get("output_format")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
            });
            self.emitted_image_count = self.emitted_image_count.max(1);
        }
        self.emit_final(report_context, event.get("usage"))
    }

    fn handle_failed(
        &mut self,
        _report_context: &Value,
        event: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.emitted_failure {
            return Ok(Vec::new());
        }
        self.emitted_failure = true;
        self.finished = true;
        let mut output = encode_json_sse(
            None,
            &serde_json::json!({
                "error": image_failure_error(event),
            }),
        )?;
        output.extend(encode_done_sse());
        Ok(output)
    }

    fn ensure_started(
        &mut self,
        report_context: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.started {
            return Ok(Vec::new());
        }
        self.emit_empty_progress_chunk(report_context)
    }

    fn emit_empty_progress_chunk(
        &mut self,
        report_context: &Value,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.started = true;
        let (response_id, model) = self.identity(report_context);
        encode_json_sse(
            None,
            &build_openai_chat_chunk(&response_id, &model, String::new(), None, None),
        )
    }

    fn emit_final(
        &mut self,
        report_context: &Value,
        usage: Option<&Value>,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.finished || self.emitted_failure {
            return Ok(Vec::new());
        }
        let Some(latest_image) = self.latest_image.clone() else {
            return self.ensure_started(report_context);
        };
        let mut output = self.ensure_started(report_context)?;
        let (response_id, model) = self.identity(report_context);
        output.extend(encode_json_sse(
            None,
            &build_openai_chat_chunk(
                &response_id,
                &model,
                image_chat_markdown(&latest_image),
                None,
                None,
            ),
        )?);
        output.extend(encode_json_sse(
            None,
            &build_openai_chat_finish_chunk(&response_id, &model, Some("stop")),
        )?);
        if let Some((
            input_tokens,
            output_tokens,
            total_tokens,
            reasoning_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        )) = openai_image_chat_usage_counts(usage)
        {
            output.extend(encode_json_sse(
                None,
                &build_openai_chat_usage_chunk_with_cache(
                    &response_id,
                    &model,
                    input_tokens,
                    output_tokens,
                    total_tokens,
                    reasoning_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                ),
            )?);
        }
        output.extend(encode_done_sse());
        self.finished = true;
        Ok(output)
    }

    fn update_identity_from_response(&mut self, response: &Value) {
        if let Some(id) = response
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.response_id = Some(id.replace("resp", "chatcmpl"));
        }
        if let Some(model) = response
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.model = Some(model.to_string());
        }
    }

    fn identity(&self, report_context: &Value) -> (String, String) {
        let response_id = self.response_id.clone().unwrap_or_else(|| {
            report_context
                .get("request_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("chatcmpl-image-{value}"))
                .unwrap_or_else(|| "chatcmpl-image".to_string())
        });
        let model = self
            .model
            .clone()
            .or_else(|| {
                report_context
                    .get("mapped_model")
                    .or_else(|| report_context.get("model"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| "gpt-image".to_string());
        (response_id, model)
    }
}

impl OpenAiImageStreamTerminalState {
    pub fn push_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<Option<ExecutionStreamTerminalSummary>, AiSurfaceFinalizeError> {
        let text = std::str::from_utf8(&line)
            .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?;
        let trimmed = text.trim_matches('\r').trim_matches('\n');
        if trimmed.is_empty() {
            self.flush_event(report_context)?;
            return Ok(self.latest_summary(report_context));
        }
        if let Some(value) = trimmed.strip_prefix("event:") {
            self.event_name = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("data:") {
            self.data_lines.push(value.trim().to_string());
        }
        Ok(self.latest_summary(report_context))
    }

    pub fn finish(
        &mut self,
        report_context: &Value,
    ) -> Result<Option<ExecutionStreamTerminalSummary>, AiSurfaceFinalizeError> {
        self.flush_event(report_context)?;
        if self.image_count > 0 && !self.observed_finish {
            self.observed_finish = true;
        }
        Ok(self.latest_summary(report_context))
    }

    fn flush_event(&mut self, report_context: &Value) -> Result<(), AiSurfaceFinalizeError> {
        if self.data_lines.is_empty() {
            self.event_name = None;
            return Ok(());
        }
        let data = std::mem::take(&mut self.data_lines).join("\n");
        let event_name = self.event_name.take();
        if data.is_empty() || data == "[DONE]" {
            return Ok(());
        }
        let event = match serde_json::from_str::<Value>(&data) {
            Ok(event) => event,
            Err(err) => {
                self.parser_error.get_or_insert_with(|| err.to_string());
                return Ok(());
            }
        };
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .or(event_name.as_deref())
            .unwrap_or_default();
        match event_type {
            "response.output_item.done" => self.observe_output_item_done(&event),
            "response.completed" | "response.done" => self.observe_completed(&event),
            "image_generation.completed" | "image_edit.completed" => {
                self.observe_image_completed(&event)
            }
            "error" | "response.failed" | "image_generation.failed" | "image_edit.failed" => {
                self.parser_error
                    .get_or_insert_with(|| image_failure_error(&event).to_string());
                self.observed_finish = true;
            }
            _ => {}
        }
        if self.model.is_none() {
            self.model = image_bridge_model(Some(report_context));
        }
        Ok(())
    }

    fn observe_output_item_done(&mut self, event: &Value) {
        let Some(item) = event.get("item").and_then(Value::as_object) else {
            return;
        };
        if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
            return;
        }
        let Some(result) = item
            .get("result")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        let key = image_chat_output_key(item, result);
        if self.image_keys.insert(key) {
            self.image_count = self.image_count.saturating_add(1);
        }
    }

    fn observe_completed(&mut self, event: &Value) {
        self.observed_finish = true;
        let Some(response) = event.get("response") else {
            return;
        };
        self.update_identity_from_response(response);
        if self.image_count == 0 {
            self.image_count = completed_response_image_count(response);
        }
        self.usage = response
            .get("tool_usage")
            .and_then(|value| value.get("image_gen"))
            .cloned()
            .or_else(|| response.get("usage").cloned())
            .or_else(|| self.usage.clone());
    }

    fn observe_image_completed(&mut self, event: &Value) {
        self.observed_finish = true;
        if self.image_count == 0
            && event
                .get("b64_json")
                .or_else(|| event.get("result"))
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
        {
            self.image_count = 1;
        }
        self.usage = event.get("usage").cloned().or_else(|| self.usage.clone());
    }

    fn update_identity_from_response(&mut self, response: &Value) {
        if let Some(id) = response
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.response_id = Some(id.to_string());
        }
        if let Some(model) = response
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            self.model = Some(model.to_string());
        }
    }

    fn latest_summary(&self, report_context: &Value) -> Option<ExecutionStreamTerminalSummary> {
        if self.image_count == 0
            && self.usage.is_none()
            && self.response_id.is_none()
            && self.model.is_none()
            && self.parser_error.is_none()
        {
            return None;
        }
        Some(ExecutionStreamTerminalSummary {
            standardized_usage: openai_image_stream_standardized_usage(
                self.usage.as_ref(),
                Some(report_context),
                self.image_count,
            ),
            finish_reason: self.observed_finish.then(|| "stop".to_string()),
            response_id: self.response_id.clone(),
            model: self
                .model
                .clone()
                .or_else(|| image_bridge_model(Some(report_context))),
            observed_finish: self.observed_finish,
            unknown_event_count: 0,
            parser_error: self.parser_error.clone(),
        })
    }
}

fn completed_response_image_chat_frame(response: &Value) -> Option<OpenAiImageChatFrame> {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("image_generation_call"))
        .find_map(|item| {
            let result = item.get("result").and_then(Value::as_str)?.trim();
            if result.is_empty() {
                return None;
            }
            Some(OpenAiImageChatFrame {
                b64_json: result.to_string(),
                output_format: item
                    .get("output_format")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
            })
        })
}

fn completed_response_image_count(response: &Value) -> u64 {
    response
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("image_generation_call"))
        .filter(|item| {
            item.get("result")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
        })
        .count() as u64
}

fn image_chat_output_key(item: &Map<String, Value>, result: &str) -> String {
    item.get("id")
        .or_else(|| item.get("call_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| result.to_string())
}

fn openai_image_stream_standardized_usage(
    usage: Option<&Value>,
    report_context: Option<&Value>,
    image_count: u64,
) -> Option<StandardizedUsage> {
    let mut standardized_usage = usage
        .and_then(openai_image_usage_to_standardized_usage)
        .unwrap_or_default();
    if image_count > 0 {
        standardized_usage.request_count = i64::try_from(image_count).unwrap_or(i64::MAX);
        standardized_usage
            .dimensions
            .insert("image_count".to_string(), serde_json::json!(image_count));
    }
    if let Some(output_format) = image_request_output_format(report_context) {
        standardized_usage.dimensions.insert(
            "image_output_format".to_string(),
            serde_json::json!(output_format),
        );
    }
    if let Some(size) = image_request_size(report_context) {
        standardized_usage
            .dimensions
            .insert("image_size".to_string(), serde_json::json!(size));
    }
    if let Some(quality) = image_request_quality(report_context) {
        standardized_usage
            .dimensions
            .insert("image_quality".to_string(), serde_json::json!(quality));
    }
    (standardized_usage.signal_score() > 0).then_some(standardized_usage)
}

fn openai_image_usage_to_standardized_usage(value: &Value) -> Option<StandardizedUsage> {
    let usage = value.as_object()?;
    let mut input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_i64)
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .or_else(|| usage.get("prompt_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_creation_tokens"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_i64)
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .or_else(|| usage.get("prompt_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(0);
    let total_tokens = usage.get("total_tokens").and_then(Value::as_i64).unwrap_or(
        input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_creation_tokens)
            .saturating_add(cache_read_tokens),
    );
    if input_tokens == 0 && total_tokens > output_tokens {
        input_tokens = total_tokens.saturating_sub(output_tokens);
    }
    let mut standardized_usage = StandardizedUsage::new();
    standardized_usage.input_tokens = input_tokens;
    standardized_usage.output_tokens = output_tokens;
    standardized_usage.cache_creation_tokens = cache_creation_tokens;
    standardized_usage.cache_read_tokens = cache_read_tokens;
    standardized_usage
        .dimensions
        .insert("total_tokens".to_string(), serde_json::json!(total_tokens));
    Some(standardized_usage.normalize_cache_creation_breakdown())
}

fn image_chat_markdown(frame: &OpenAiImageChatFrame) -> String {
    let mime_type = match frame
        .output_format
        .as_deref()
        .unwrap_or("png")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "webp" => "image/webp".to_string(),
        "png" => "image/png".to_string(),
        value if !value.is_empty() => format!("image/{value}"),
        _ => "image/png".to_string(),
    };
    format!(
        "![generated image](data:{mime_type};base64,{})",
        frame.b64_json
    )
}

fn openai_image_chat_usage_counts(usage: Option<&Value>) -> Option<(u64, u64, u64, u64, u64, u64)> {
    let usage = usage.and_then(Value::as_object)?;
    let mut input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .or_else(|| usage.get("prompt_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_creation_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .or_else(|| {
            usage
                .get("input_tokens_details")
                .or_else(|| usage.get("prompt_tokens_details"))
                .and_then(Value::as_object)
                .and_then(|details| details.get("cached_tokens"))
                .and_then(Value::as_u64)
        })
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(input_tokens.saturating_add(output_tokens));
    if input_tokens == 0 && total_tokens > output_tokens {
        input_tokens = total_tokens.saturating_sub(output_tokens);
    }
    (total_tokens > 0).then_some((
        input_tokens,
        output_tokens,
        total_tokens,
        0,
        cache_creation_tokens,
        cache_read_tokens,
    ))
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

fn image_request_output_format(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("output_format"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn image_request_size(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("size"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn image_request_quality(report_context: Option<&Value>) -> Option<String> {
    report_context
        .and_then(|value| value.get("image_request"))
        .and_then(|value| value.get("quality"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn image_bridge_model(report_context: Option<&Value>) -> Option<String> {
    report_context.and_then(|context| {
        context
            .get("mapped_model")
            .or_else(|| context.get("model"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
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

pub struct OpenAiImageSyncFinalizeProduct {
    pub client_body_json: Value,
    pub provider_body_json: Value,
}

pub fn maybe_build_openai_image_sync_finalize_product(
    report_kind: &str,
    status_code: u16,
    report_context: Option<&Value>,
    body_json: Option<&Value>,
    body_base64: Option<&str>,
) -> Result<Option<OpenAiImageSyncFinalizeProduct>, AiSurfaceFinalizeError> {
    if report_kind != OPENAI_IMAGE_SYNC_FINALIZE_REPORT_KIND || status_code >= 400 {
        return Ok(None);
    }
    let Some(report_context) = report_context else {
        return Ok(None);
    };
    if report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .map(str::trim)
        != Some("openai:image")
    {
        return Ok(None);
    }
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if provider_api_format == "gemini:generate_content" {
        let Some(provider_body_json) = body_json else {
            return Ok(None);
        };
        let Some(client_body_json) =
            crate::formats::shared::image_bridge::build_openai_image_response_from_gemini_response(
                provider_body_json,
                Some(report_context),
            )
        else {
            return Ok(None);
        };
        return Ok(Some(OpenAiImageSyncFinalizeProduct {
            client_body_json,
            provider_body_json: provider_body_json.clone(),
        }));
    }
    if provider_api_format != "openai:image" {
        return Ok(None);
    }
    if let Some(provider_body_json) = body_json {
        if openai_image_response_has_standard_data(provider_body_json) {
            return Ok(Some(OpenAiImageSyncFinalizeProduct {
                client_body_json: provider_body_json.clone(),
                provider_body_json: provider_body_json.clone(),
            }));
        }
        if provider_body_json.get("output").is_some() && provider_body_json.get("data").is_none() {
            let Some(client_body_json) = crate::formats::shared::image_bridge::build_openai_image_response_from_response_stream_sync_body(
                provider_body_json,
                Some(report_context),
            ) else {
                return Ok(None);
            };
            return Ok(Some(OpenAiImageSyncFinalizeProduct {
                client_body_json,
                provider_body_json: provider_body_json.clone(),
            }));
        }
    }
    let Some(body_base64) = body_base64 else {
        return Ok(None);
    };
    let default_output_format = report_context
        .get("image_request")
        .and_then(|value| value.get("output_format"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(CODEX_OPENAI_IMAGE_DEFAULT_OUTPUT_FORMAT);
    let body_bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)?;
    let text = std::str::from_utf8(&body_bytes)
        .map_err(|err| AiSurfaceFinalizeError::new(err.to_string()))?;

    let mut created = None;
    let mut completed_response = None;
    let mut images = Vec::new();

    for raw_block in text.split("\n\n") {
        let block = raw_block.trim();
        if block.is_empty() {
            continue;
        }
        let data_line = block
            .lines()
            .find_map(|line| line.trim().strip_prefix("data:").map(str::trim));
        let Some(data_line) = data_line else {
            continue;
        };
        if data_line.is_empty() || data_line == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(data_line)?;
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "response.created" => {
                created = event
                    .get("response")
                    .and_then(|value| value.get("created_at"))
                    .and_then(Value::as_i64)
                    .or(created);
            }
            "response.output_item.done" => {
                let Some(item) = event.get("item").and_then(Value::as_object) else {
                    continue;
                };
                if item.get("type").and_then(Value::as_str) != Some("image_generation_call") {
                    continue;
                }
                let Some(result) = item.get("result").and_then(Value::as_str) else {
                    continue;
                };
                images.push(serde_json::json!({
                    "b64_json": result,
                    "output_format": item.get("output_format").cloned().unwrap_or(Value::String(default_output_format.to_string())),
                    "revised_prompt": item.get("revised_prompt").cloned().unwrap_or(Value::Null),
                }));
            }
            "response.completed" => {
                completed_response = event.get("response").and_then(Value::as_object).cloned();
            }
            _ => {}
        }
    }

    if images.is_empty() {
        return Ok(None);
    }

    let completed_response = completed_response.unwrap_or_default();
    let provider_usage = completed_response
        .get("tool_usage")
        .and_then(|value| value.get("image_gen"))
        .cloned()
        .or_else(|| completed_response.get("usage").cloned());
    let provider_body_json = serde_json::json!({
        "id": completed_response.get("id").cloned().unwrap_or(Value::Null),
        "object": "response",
        "model": completed_response.get("model").cloned().unwrap_or(Value::Null),
        "status": completed_response.get("status").cloned().unwrap_or(Value::String("completed".to_string())),
        "usage": provider_usage,
        "tool_usage": completed_response.get("tool_usage").cloned().unwrap_or(Value::Null),
        "output": images
            .iter()
            .map(|image| serde_json::json!({
                "type": "image_generation_call",
                "output_format": image.get("output_format").cloned().unwrap_or(Value::Null),
                "revised_prompt": image.get("revised_prompt").cloned().unwrap_or(Value::Null),
            }))
            .collect::<Vec<_>>(),
    });
    let client_images = images
        .iter()
        .map(|image| {
            let revised_prompt = image.get("revised_prompt").cloned().unwrap_or(Value::Null);
            let b64_json = image
                .get("b64_json")
                .and_then(Value::as_str)
                .unwrap_or_default();
            serde_json::json!({
                "b64_json": b64_json,
                "revised_prompt": revised_prompt,
            })
        })
        .collect::<Vec<_>>();
    let client_body_json = serde_json::json!({
        "created": created.unwrap_or_default(),
        "data": client_images,
        "usage": provider_body_json.get("usage").cloned().unwrap_or(Value::Null),
    });

    Ok(Some(OpenAiImageSyncFinalizeProduct {
        client_body_json,
        provider_body_json,
    }))
}

fn openai_image_response_has_standard_data(body_json: &Value) -> bool {
    body_json
        .get("data")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.as_object().is_some_and(|object| {
                    ["b64_json", "url"].iter().any(|field| {
                        object
                            .get(*field)
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .is_some_and(|value| !value.is_empty())
                    })
                })
            })
        })
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use serde_json::json;

    use super::{maybe_build_openai_image_sync_finalize_product, OpenAiImageStreamState};

    fn utf8(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("utf8 should decode")
    }

    #[test]
    fn emits_completed_event_for_generate() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "needs_conversion": false,
            "image_request": {
                "operation": "generate"
            }
        });
        let mut rewriter = OpenAiImageStreamState::default();

        let first = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.output_item.done\n",
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"result\":\"aGVsbG8=\"}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        assert!(first.is_empty());

        let second = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"tool_usage\":{\"image_gen\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        let output_text = utf8(second);
        assert!(output_text.contains("event: image_generation.completed"));
        assert!(output_text.contains("\"type\":\"image_generation.completed\""));
        assert!(output_text.contains("\"b64_json\":\"aGVsbG8=\""));
        assert!(output_text.contains("\"input_tokens\":1"));
        assert!(!output_text.contains("data: [DONE]"));
        assert!(rewriter
            .finish(&report_context)
            .expect("finish should succeed")
            .is_empty());
    }

    #[test]
    fn maps_responses_partial_image_events() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "needs_conversion": false,
            "image_request": {
                "operation": "generate",
                "partial_images": 1
            }
        });
        let mut rewriter = OpenAiImageStreamState::default();

        let partial = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.image_generation_call.partial_image\n",
                    "data: {\"type\":\"response.image_generation_call.partial_image\",\"partial_image_index\":0,\"partial_image_b64\":\"cGFydGlhbA==\"}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        let partial_text = utf8(partial);
        assert!(partial_text.contains("event: image_generation.partial_image"));
        assert!(partial_text.contains("\"type\":\"image_generation.partial_image\""));
        assert!(partial_text.contains("\"b64_json\":\"cGFydGlhbA==\""));
        assert!(partial_text.contains("\"partial_image_index\":0"));
        assert!(!partial_text.contains("response.image_generation_call.partial_image"));

        let done = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.output_item.done\n",
                    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"result\":\"ZmluYWw=\"}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        assert!(done.is_empty());

        let completed = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":5,\"total_tokens\":9}}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        let completed_text = utf8(completed);
        assert!(completed_text.contains("event: image_generation.completed"));
        assert!(completed_text.contains("\"type\":\"image_generation.completed\""));
        assert!(completed_text.contains("\"b64_json\":\"ZmluYWw=\""));
        assert!(completed_text.contains("\"total_tokens\":9"));
    }

    #[test]
    fn maps_upstream_error_to_generation_failed_once() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "needs_conversion": false,
            "image_request": {
                "operation": "generate"
            }
        });
        let mut rewriter = OpenAiImageStreamState::default();

        let output = rewriter
            .push_chunk(
                &report_context,
                concat!(
                    "event: error\n",
                    "data: {\"type\":\"error\",\"error\":{\"type\":\"input-images\",\"code\":\"rate_limit_exceeded\",\"message\":\"Rate limit reached for gpt-image-2\",\"param\":null}}\n\n",
                    "event: response.failed\n",
                    "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"Rate limit reached for gpt-image-2\"}}}\n\n"
                )
                .as_bytes(),
            )
            .expect("rewrite should succeed");
        let output_text = utf8(output);
        assert!(output_text.contains("event: image_generation.failed"));
        assert_eq!(
            output_text
                .matches("event: image_generation.failed")
                .count(),
            1
        );
        assert!(output_text.contains("\"type\":\"image_generation.failed\""));
        assert!(output_text.contains("\"type\":\"input-images\""));
        assert!(output_text.contains("\"code\":\"rate_limit_exceeded\""));
        assert!(output_text.contains("\"message\":\"Rate limit reached for gpt-image-2\""));
        assert!(!output_text.contains("response.failed"));
        assert!(rewriter
            .finish(&report_context)
            .expect("finish should succeed")
            .is_empty());
    }

    #[test]
    fn sync_finalize_product_maps_stream_response_to_client_and_provider_bodies() {
        let report_context = json!({
            "client_api_format": "openai:image",
            "provider_api_format": "openai:image",
            "image_request": {
                "operation": "generate",
                "output_format": "png"
            }
        });
        let body_base64 = base64::engine::general_purpose::STANDARD.encode(
            concat!(
                "event: response.created\n",
                "data: {\"type\":\"response.created\",\"response\":{\"created_at\":1776839946}}\n\n",
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"image_generation_call\",\"output_format\":\"png\",\"revised_prompt\":\"revised history prompt\",\"result\":\"aGVsbG8=\"}}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_123\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"tool_usage\":{\"image_gen\":{\"input_tokens\":171,\"output_tokens\":1372,\"total_tokens\":1543}}}}\n\n"
            )
            .as_bytes(),
        );

        let product = maybe_build_openai_image_sync_finalize_product(
            "openai_image_sync_finalize",
            200,
            Some(&report_context),
            None,
            Some(&body_base64),
        )
        .expect("finalize should succeed")
        .expect("finalize should match");

        assert_eq!(product.client_body_json["created"], 1776839946);
        assert_eq!(product.client_body_json["data"][0]["b64_json"], "aGVsbG8=");
        assert_eq!(
            product.client_body_json["data"][0]["revised_prompt"],
            "revised history prompt"
        );
        assert_eq!(product.client_body_json["usage"]["input_tokens"], 171);
        assert_eq!(product.provider_body_json["id"], "resp_img_123");
        assert_eq!(
            product.provider_body_json["output"][0]["output_format"],
            "png"
        );
        assert_eq!(
            product.provider_body_json["output"][0]["revised_prompt"],
            "revised history prompt"
        );
    }

    #[test]
    fn sync_finalize_accepts_standard_openai_image_response() {
        let provider_body = json!({
            "created": 1779273523,
            "data": [{
                "b64_json": "aGVsbG8=",
                "revised_prompt": "draw a small cat"
            }]
        });
        let product = maybe_build_openai_image_sync_finalize_product(
            "openai_image_sync_finalize",
            200,
            Some(&json!({
                "client_api_format": "openai:image",
                "provider_api_format": "openai:image",
                "image_request": {
                    "operation": "generate",
                    "response_format": "b64_json"
                }
            })),
            Some(&provider_body),
            None,
        )
        .expect("standard image response should finalize")
        .expect("standard image response should match");

        assert_eq!(product.client_body_json["created"], 1779273523);
        assert_eq!(product.client_body_json["data"][0]["b64_json"], "aGVsbG8=");
        assert_eq!(product.provider_body_json, provider_body);
    }
}
