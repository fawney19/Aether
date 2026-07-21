use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::formats::openai::image::stream::{OpenAiImageChatStreamState, OpenAiImageStreamState};
use crate::formats::openai::responses::response::ensure_modern_openai_responses_response_fields;
use crate::formats::shared::model_directives::model_directive_display_model_from_report_context;
use crate::formats::shared::response::{
    remove_empty_pages_from_tool_arguments, remove_empty_pages_from_tool_input_value,
};
use crate::formats::shared::sse::encode_json_sse;
use crate::formats::shared::stream_core::StreamingStandardFormatMatrix;
use crate::formats::shared::AiSurfaceFinalizeError;
use crate::provider_compat::kiro_stream::KiroToClaudeCliStreamState;
use crate::provider_compat::private_envelope::transform_provider_private_stream_line;
use crate::provider_compat::surfaces::{
    provider_adaptation_should_unwrap_stream_envelope, KIRO_ENVELOPE_NAME,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeStreamRewriteMode {
    EnvelopeUnwrap,
    ModelDirectiveDisplay,
    OpenAiResponsesCompat,
    OpenAiImage,
    OpenAiImageToOpenAiChat,
    ClaudeReadToolSanitize,
    Standard,
    KiroToClaudeCli,
    KiroToClaudeCliThenStandard,
}

pub fn resolve_finalize_stream_rewrite_mode(
    report_context: &Value,
) -> Option<FinalizeStreamRewriteMode> {
    let needs_conversion = report_context
        .get("needs_conversion")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let provider_stream_event_api_format = report_context
        .get("provider_stream_event_api_format")
        .or_else(|| report_context.get("provider_stream_api_format"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| provider_api_format.clone());
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let stream_needs_conversion = needs_conversion
        || !is_same_format_family(
            provider_stream_event_api_format.as_str(),
            client_api_format.as_str(),
        );

    if !needs_conversion
        && client_consumes_same_private_stream_envelope(
            report_context,
            envelope_name.as_str(),
            provider_api_format.as_str(),
            client_api_format.as_str(),
        )
    {
        return model_directive_display_model_from_report_context(report_context)
            .map(|_| FinalizeStreamRewriteMode::ModelDirectiveDisplay);
    }

    if needs_conversion
        && envelope_name.eq_ignore_ascii_case(KIRO_ENVELOPE_NAME)
        && provider_api_format == "claude:messages"
    {
        return supports_standard_stream_rewrite(
            provider_api_format.as_str(),
            client_api_format.as_str(),
        )
        .then_some(FinalizeStreamRewriteMode::KiroToClaudeCliThenStandard);
    }

    if provider_api_format == "openai:image" && client_api_format == "openai:chat" {
        return Some(FinalizeStreamRewriteMode::OpenAiImageToOpenAiChat);
    }

    if provider_api_format == "openai:image" && client_api_format == "openai:image" {
        return Some(FinalizeStreamRewriteMode::OpenAiImage);
    }

    if stream_needs_conversion {
        // CPA strategy: when provider and client share the same wire format
        // (exact match or same family), pass through the stream verbatim.
        // Parsing→rebuilding only adds overhead and may lose information
        // (encrypted_content, original item IDs, etc.).
        if is_same_format_family(
            provider_stream_event_api_format.as_str(),
            client_api_format.as_str(),
        ) {
            if is_openai_responses_family(provider_stream_event_api_format.as_str())
                && is_openai_responses_family(client_api_format.as_str())
            {
                return Some(FinalizeStreamRewriteMode::OpenAiResponsesCompat);
            }
            if provider_stream_event_api_format == "claude:messages"
                && client_api_format == "claude:messages"
            {
                return Some(FinalizeStreamRewriteMode::ClaudeReadToolSanitize);
            }
            return model_directive_display_model_from_report_context(report_context)
                .map(|_| FinalizeStreamRewriteMode::ModelDirectiveDisplay);
        }
        return supports_standard_stream_rewrite(
            provider_stream_event_api_format.as_str(),
            client_api_format.as_str(),
        )
        .then_some(FinalizeStreamRewriteMode::Standard);
    }

    if envelope_name.eq_ignore_ascii_case(KIRO_ENVELOPE_NAME) {
        return (provider_api_format == "claude:messages"
            && client_api_format == "claude:messages")
            .then_some(FinalizeStreamRewriteMode::KiroToClaudeCli);
    }

    if model_directive_display_model_from_report_context(report_context).is_some()
        && provider_stream_event_api_format == client_api_format
        && is_standard_provider_api_format(provider_stream_event_api_format.as_str())
        && !provider_adaptation_should_unwrap_stream_envelope(
            envelope_name.as_str(),
            provider_api_format.as_str(),
        )
    {
        if provider_stream_event_api_format == "claude:messages" {
            return Some(FinalizeStreamRewriteMode::ClaudeReadToolSanitize);
        }
        return Some(FinalizeStreamRewriteMode::ModelDirectiveDisplay);
    }

    if provider_stream_event_api_format == "claude:messages"
        && client_api_format == "claude:messages"
    {
        return Some(FinalizeStreamRewriteMode::ClaudeReadToolSanitize);
    }

    if is_same_format_family(
        provider_stream_event_api_format.as_str(),
        client_api_format.as_str(),
    ) && is_openai_responses_family(provider_stream_event_api_format.as_str())
        && is_openai_responses_family(client_api_format.as_str())
    {
        return Some(FinalizeStreamRewriteMode::OpenAiResponsesCompat);
    }

    (provider_api_format == client_api_format
        && provider_adaptation_should_unwrap_stream_envelope(
            envelope_name.as_str(),
            provider_api_format.as_str(),
        ))
    .then_some(FinalizeStreamRewriteMode::EnvelopeUnwrap)
}

fn client_consumes_same_private_stream_envelope(
    report_context: &Value,
    envelope_name: &str,
    provider_api_format: &str,
    client_api_format: &str,
) -> bool {
    if envelope_name.is_empty()
        || provider_api_format != client_api_format
        || !provider_adaptation_should_unwrap_stream_envelope(envelope_name, provider_api_format)
    {
        return false;
    }
    report_context
        .get("client_envelope_name")
        .and_then(Value::as_str)
        .is_some_and(|client_envelope_name| {
            client_envelope_name.eq_ignore_ascii_case(envelope_name)
        })
}

enum AiSurfaceStreamRewriteState {
    EnvelopeUnwrap,
    ModelDirectiveDisplay,
    OpenAiResponsesCompat,
    OpenAiImage(Box<OpenAiImageStreamState>),
    OpenAiImageToOpenAiChat(Box<OpenAiImageChatStreamState>),
    ClaudeReadToolSanitize(Box<ClaudeReadToolStreamSanitizer>),
    Standard(Box<StreamingStandardFormatMatrix>),
    KiroToClaudeCli(Box<KiroToClaudeCliStreamState>),
    KiroToClaudeCliThenStandard {
        kiro: Box<KiroToClaudeCliStreamState>,
        standard: Box<StreamingStandardFormatMatrix>,
    },
}

pub struct AiSurfaceStreamRewriter<'a> {
    report_context: &'a Value,
    buffered: Vec<u8>,
    state: AiSurfaceStreamRewriteState,
}

pub fn maybe_build_ai_surface_stream_rewriter<'a>(
    report_context: Option<&'a Value>,
) -> Option<AiSurfaceStreamRewriter<'a>> {
    let report_context = report_context?;
    let state = match resolve_finalize_stream_rewrite_mode(report_context)? {
        FinalizeStreamRewriteMode::EnvelopeUnwrap => AiSurfaceStreamRewriteState::EnvelopeUnwrap,
        FinalizeStreamRewriteMode::ModelDirectiveDisplay => {
            AiSurfaceStreamRewriteState::ModelDirectiveDisplay
        }
        FinalizeStreamRewriteMode::OpenAiResponsesCompat => {
            AiSurfaceStreamRewriteState::OpenAiResponsesCompat
        }
        FinalizeStreamRewriteMode::OpenAiImage => {
            AiSurfaceStreamRewriteState::OpenAiImage(Box::<OpenAiImageStreamState>::default())
        }
        FinalizeStreamRewriteMode::OpenAiImageToOpenAiChat => {
            AiSurfaceStreamRewriteState::OpenAiImageToOpenAiChat(
                Box::<OpenAiImageChatStreamState>::default(),
            )
        }
        FinalizeStreamRewriteMode::ClaudeReadToolSanitize => {
            AiSurfaceStreamRewriteState::ClaudeReadToolSanitize(
                Box::<ClaudeReadToolStreamSanitizer>::default(),
            )
        }
        FinalizeStreamRewriteMode::Standard => {
            AiSurfaceStreamRewriteState::Standard(Box::<StreamingStandardFormatMatrix>::default())
        }
        FinalizeStreamRewriteMode::KiroToClaudeCli => AiSurfaceStreamRewriteState::KiroToClaudeCli(
            Box::new(KiroToClaudeCliStreamState::new(report_context)),
        ),
        FinalizeStreamRewriteMode::KiroToClaudeCliThenStandard => {
            AiSurfaceStreamRewriteState::KiroToClaudeCliThenStandard {
                kiro: Box::new(KiroToClaudeCliStreamState::new(report_context)),
                standard: Box::<StreamingStandardFormatMatrix>::default(),
            }
        }
    };

    Some(AiSurfaceStreamRewriter {
        report_context,
        buffered: Vec::new(),
        state,
    })
}

impl AiSurfaceStreamRewriter<'_> {
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        match &mut self.state {
            AiSurfaceStreamRewriteState::OpenAiImage(state) => {
                state.push_chunk(self.report_context, chunk)
            }
            AiSurfaceStreamRewriteState::OpenAiImageToOpenAiChat(state) => {
                state.push_chunk(self.report_context, chunk)
            }
            AiSurfaceStreamRewriteState::ClaudeReadToolSanitize(state) => {
                state.push_chunk(self.report_context, chunk)
            }
            AiSurfaceStreamRewriteState::KiroToClaudeCli(state) => {
                state.push_chunk(self.report_context, chunk)
            }
            AiSurfaceStreamRewriteState::KiroToClaudeCliThenStandard { kiro, standard } => {
                let claude_bytes = kiro.push_chunk(self.report_context, chunk)?;
                transform_standard_bytes(standard, self.report_context, claude_bytes)
            }
            AiSurfaceStreamRewriteState::EnvelopeUnwrap
            | AiSurfaceStreamRewriteState::ModelDirectiveDisplay
            | AiSurfaceStreamRewriteState::OpenAiResponsesCompat
            | AiSurfaceStreamRewriteState::Standard(_) => {
                self.buffered.extend_from_slice(chunk);
                let mut output = Vec::new();
                while let Some(line_end) = self.buffered.iter().position(|byte| *byte == b'\n') {
                    let line = self.buffered.drain(..=line_end).collect::<Vec<_>>();
                    output.extend(self.transform_line(line)?);
                }
                Ok(output)
            }
        }
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        match &mut self.state {
            AiSurfaceStreamRewriteState::OpenAiImage(state) => state.finish(self.report_context),
            AiSurfaceStreamRewriteState::OpenAiImageToOpenAiChat(state) => {
                state.finish(self.report_context)
            }
            AiSurfaceStreamRewriteState::ClaudeReadToolSanitize(state) => {
                state.finish(self.report_context)
            }
            AiSurfaceStreamRewriteState::KiroToClaudeCli(state) => {
                state.finish(self.report_context)
            }
            AiSurfaceStreamRewriteState::KiroToClaudeCliThenStandard { kiro, standard } => {
                let mut output = transform_standard_bytes(
                    standard,
                    self.report_context,
                    kiro.finish(self.report_context)?,
                )?;
                output.extend(standard.finish(self.report_context)?);
                Ok(output)
            }
            AiSurfaceStreamRewriteState::EnvelopeUnwrap
            | AiSurfaceStreamRewriteState::ModelDirectiveDisplay
            | AiSurfaceStreamRewriteState::OpenAiResponsesCompat
            | AiSurfaceStreamRewriteState::Standard(_) => {
                if self.buffered.is_empty() {
                    if let AiSurfaceStreamRewriteState::Standard(state) = &mut self.state {
                        return state.finish(self.report_context);
                    }
                    return Ok(Vec::new());
                }
                let line = std::mem::take(&mut self.buffered);
                let mut output = self.transform_line(line)?;
                if let AiSurfaceStreamRewriteState::Standard(state) = &mut self.state {
                    output.extend(state.finish(self.report_context)?);
                }
                Ok(output)
            }
        }
    }

    fn transform_line(&mut self, line: Vec<u8>) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        match &mut self.state {
            AiSurfaceStreamRewriteState::EnvelopeUnwrap => {
                let output = transform_provider_private_stream_line(self.report_context, line)
                    .map_err(AiSurfaceFinalizeError::from)?;
                rewrite_model_directive_stream_line(self.report_context, output)
            }
            AiSurfaceStreamRewriteState::ModelDirectiveDisplay => {
                rewrite_model_directive_stream_line(self.report_context, line)
            }
            AiSurfaceStreamRewriteState::OpenAiResponsesCompat => {
                rewrite_openai_responses_compat_stream_line(self.report_context, line)
            }
            AiSurfaceStreamRewriteState::Standard(state) => {
                transform_standard_line(state, self.report_context, line)
            }
            AiSurfaceStreamRewriteState::OpenAiImage(_)
            | AiSurfaceStreamRewriteState::OpenAiImageToOpenAiChat(_)
            | AiSurfaceStreamRewriteState::ClaudeReadToolSanitize(_)
            | AiSurfaceStreamRewriteState::KiroToClaudeCli(_)
            | AiSurfaceStreamRewriteState::KiroToClaudeCliThenStandard { .. } => Ok(Vec::new()),
        }
    }
}

#[derive(Default)]
struct ClaudeReadToolBlockState {
    name: String,
    buffered_input_json: String,
}

#[derive(Default)]
struct ClaudeReadToolStreamSanitizer {
    buffered: Vec<u8>,
    blocks: BTreeMap<usize, ClaudeReadToolBlockState>,
}

impl ClaudeReadToolStreamSanitizer {
    fn push_chunk(
        &mut self,
        report_context: &Value,
        chunk: &[u8],
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        self.buffered.extend_from_slice(chunk);
        let mut output = Vec::new();
        while let Some(record) = drain_next_sse_record(&mut self.buffered) {
            output.extend(self.transform_record(report_context, record)?);
        }
        Ok(output)
    }

    fn finish(&mut self, report_context: &Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.buffered.is_empty() {
            return Ok(Vec::new());
        }
        let record = std::mem::take(&mut self.buffered);
        self.transform_record(report_context, record)
    }

    fn transform_record(
        &mut self,
        report_context: &Value,
        record: Vec<u8>,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let Some((event, mut payload)) = parse_sse_record_json(&record) else {
            return rewrite_model_directive_stream_record(report_context, record);
        };
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(event.as_deref().unwrap_or_default())
            .to_string();
        let mut output = match event_type.as_str() {
            "content_block_start" => {
                self.transform_content_block_start(event.as_deref(), payload, record)?
            }
            "content_block_delta" => self.transform_content_block_delta(payload, record)?,
            "content_block_stop" => self.transform_content_block_stop(payload, record)?,
            _ => {
                if !rewrite_stream_payload_model_from_context(report_context, &mut payload) {
                    return Ok(record);
                }
                encode_json_sse(event.as_deref(), &payload)?
            }
        };
        if model_directive_display_model_from_report_context(report_context).is_some()
            && !matches!(event_type.as_str(), "message_start" | "message_delta")
        {
            output = rewrite_model_directive_stream_record(report_context, output)?;
        }
        Ok(output)
    }

    fn transform_content_block_start(
        &mut self,
        event: Option<&str>,
        mut payload: Value,
        original_record: Vec<u8>,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let index = payload
            .get("index")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(0);
        let Some(block) = payload
            .get_mut("content_block")
            .and_then(Value::as_object_mut)
        else {
            return Ok(original_record);
        };
        let block_type = block
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if block_type != "tool_use" {
            return Ok(original_record);
        }
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.blocks.insert(
            index,
            ClaudeReadToolBlockState {
                name: name.clone(),
                buffered_input_json: String::new(),
            },
        );
        if sanitize_claude_tool_input_object(block, &name) {
            encode_json_sse(event, &payload)
        } else {
            Ok(original_record)
        }
    }

    fn transform_content_block_delta(
        &mut self,
        payload: Value,
        original_record: Vec<u8>,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let index = payload
            .get("index")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(0);
        let delta_type = payload
            .get("delta")
            .and_then(Value::as_object)
            .and_then(|delta| delta.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let partial_json = payload
            .get("delta")
            .and_then(Value::as_object)
            .and_then(|delta| delta.get("partial_json"))
            .and_then(Value::as_str);
        if delta_type != "input_json_delta" {
            return Ok(original_record);
        }
        let Some(state) = self.blocks.get_mut(&index) else {
            return Ok(original_record);
        };
        if state.name != "Read" {
            return Ok(original_record);
        }
        if let Some(partial_json) = partial_json {
            state.buffered_input_json.push_str(partial_json);
        }
        Ok(Vec::new())
    }

    fn transform_content_block_stop(
        &mut self,
        payload: Value,
        original_record: Vec<u8>,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let index = payload
            .get("index")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .unwrap_or(0);
        let Some(state) = self.blocks.remove(&index) else {
            return Ok(original_record);
        };
        let mut output = Vec::new();
        if state.name == "Read" && !state.buffered_input_json.is_empty() {
            let partial_json =
                remove_empty_pages_from_tool_arguments("Read", &state.buffered_input_json);
            if !partial_json.is_empty() {
                output.extend(encode_json_sse(
                    Some("content_block_delta"),
                    &json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": partial_json,
                        }
                    }),
                )?);
            }
        }
        if output.is_empty() {
            output = original_record;
        } else {
            output.extend(original_record);
        }
        Ok(output)
    }
}

fn sanitize_claude_tool_input_object(block: &mut Map<String, Value>, name: &str) -> bool {
    let Some(input) = block.get("input") else {
        return false;
    };
    let sanitized = remove_empty_pages_from_tool_input_value(name, input);
    if sanitized == *input {
        return false;
    }
    block.insert("input".to_string(), sanitized);
    true
}

fn drain_next_sse_record(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let mut line_start = 0usize;
    let mut index = 0usize;
    while index < buffer.len() {
        if buffer[index] != b'\n' {
            index += 1;
            continue;
        }
        let line_end = index + 1;
        let line = &buffer[line_start..line_end];
        let line_without_newline = line
            .strip_suffix(b"\n")
            .unwrap_or(line)
            .strip_suffix(b"\r")
            .unwrap_or_else(|| line.strip_suffix(b"\n").unwrap_or(line));
        if line_without_newline.is_empty() {
            return Some(buffer.drain(..line_end).collect());
        }
        line_start = line_end;
        index = line_end;
    }
    None
}

fn parse_sse_record_json(record: &[u8]) -> Option<(Option<String>, Value)> {
    let text = std::str::from_utf8(record).ok()?;
    let mut event = None;
    let mut data = String::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    if data.trim().is_empty() || data.trim() == "[DONE]" {
        return None;
    }
    let value = serde_json::from_str::<Value>(data.trim()).ok()?;
    Some((event, value))
}

fn rewrite_model_directive_stream_record(
    report_context: &Value,
    record: Vec<u8>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut output = Vec::new();
    for line in record.split_inclusive(|byte| *byte == b'\n') {
        output.extend(rewrite_model_directive_stream_line(
            report_context,
            line.to_vec(),
        )?);
    }
    Ok(output)
}

fn rewrite_model_directive_stream_line(
    report_context: &Value,
    line: Vec<u8>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let Some(display_model) = model_directive_display_model_from_report_context(report_context)
    else {
        return Ok(line);
    };
    let text = match std::str::from_utf8(&line) {
        Ok(text) => text,
        Err(_) => return Ok(line),
    };
    let trimmed_line_end = text.trim_end_matches(['\r', '\n']);
    let trailing = &text[trimmed_line_end.len()..];
    let Some((prefix, payload)) = trimmed_line_end.split_once(':') else {
        return Ok(line);
    };
    if prefix.trim() != "data" {
        return Ok(line);
    }
    let payload = payload.trim_start();
    if payload.is_empty() || payload == "[DONE]" {
        return Ok(line);
    }
    let mut value = match serde_json::from_str::<Value>(payload) {
        Ok(value) => value,
        Err(_) => return Ok(line),
    };
    if !rewrite_stream_payload_model(&mut value, &display_model) {
        return Ok(line);
    }
    let mut output = Vec::new();
    output.extend_from_slice(b"data: ");
    output.extend(serde_json::to_vec(&value)?);
    output.extend_from_slice(trailing.as_bytes());
    Ok(output)
}

fn rewrite_openai_responses_compat_stream_line(
    report_context: &Value,
    line: Vec<u8>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let text = match std::str::from_utf8(&line) {
        Ok(text) => text,
        Err(_) => return Ok(line),
    };
    let trimmed_line_end = text.trim_end_matches(['\r', '\n']);
    let trailing = &text[trimmed_line_end.len()..];
    let Some((prefix, payload)) = trimmed_line_end.split_once(':') else {
        return Ok(line);
    };
    if prefix.trim() != "data" {
        return Ok(line);
    }
    let payload = payload.trim_start();
    if payload.is_empty() || payload == "[DONE]" {
        return Ok(line);
    }
    let mut value = match serde_json::from_str::<Value>(payload) {
        Ok(value) => value,
        Err(_) => return Ok(line),
    };
    let mut changed = rewrite_stream_payload_model_from_context(report_context, &mut value);
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(event_type, "response.completed" | "response.done") {
        if let Some(response) = value.get_mut("response").and_then(Value::as_object_mut) {
            changed |= ensure_modern_openai_responses_response_fields(response);
        }
    }
    if !changed {
        return Ok(line);
    }
    let mut output = Vec::new();
    output.extend_from_slice(b"data: ");
    output.extend(serde_json::to_vec(&value)?);
    output.extend_from_slice(trailing.as_bytes());
    Ok(output)
}

fn rewrite_stream_payload_model(value: &mut Value, display_model: &str) -> bool {
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for key in ["model", "modelVersion"] {
        if object.get(key).and_then(Value::as_str).is_some() {
            object.insert(key.to_string(), Value::String(display_model.to_string()));
            changed = true;
        }
    }
    for key in ["response", "message"] {
        if let Some(nested) = object.get_mut(key) {
            changed |= rewrite_stream_payload_model(nested, display_model);
        }
    }
    changed
}

fn rewrite_stream_payload_model_from_context(report_context: &Value, value: &mut Value) -> bool {
    let Some(display_model) = model_directive_display_model_from_report_context(report_context)
    else {
        return false;
    };
    rewrite_stream_payload_model(value, &display_model)
}

fn transform_standard_bytes(
    standard: &mut StreamingStandardFormatMatrix,
    report_context: &Value,
    bytes: Vec<u8>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        output.extend(transform_standard_line(
            standard,
            report_context,
            line.to_vec(),
        )?);
    }
    Ok(output)
}

fn transform_standard_line(
    standard: &mut StreamingStandardFormatMatrix,
    report_context: &Value,
    line: Vec<u8>,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let line = if should_unwrap_envelope(report_context) {
        transform_provider_private_stream_line(report_context, line)?
    } else {
        line
    };
    if line.is_empty() {
        return Ok(Vec::new());
    }
    standard.transform_line(report_context, line)
}

fn should_unwrap_envelope(report_context: &Value) -> bool {
    let envelope_name = report_context
        .get("envelope_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let provider_api_format = report_context
        .get("provider_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default();
    provider_adaptation_should_unwrap_stream_envelope(envelope_name, provider_api_format)
}

fn supports_standard_stream_rewrite(provider_api_format: &str, client_api_format: &str) -> bool {
    is_standard_provider_api_format(provider_api_format)
        && (is_standard_chat_client_api_format(client_api_format)
            || is_standard_cli_client_api_format(client_api_format))
}

/// Returns true for OpenAI Responses family formats that share the same SSE
/// wire format and can be passed through without parsing→rebuilding.
fn is_openai_responses_family(api_format: &str) -> bool {
    matches!(
        aether_ai_formats::normalize_api_format_alias(api_format).as_str(),
        "openai:responses" | "openai:responses:compact"
    )
}

/// Returns true when two API formats share the same SSE wire format and
/// can be passed through without parsing→rebuilding.  This covers:
///
/// - Exact matches after normalisation (e.g. `claude:messages` ↔ `claude:messages`)
/// - OpenAI Responses family (`openai:responses` ↔ `openai:responses:compact`)
fn is_same_format_family(provider_format: &str, client_format: &str) -> bool {
    let provider = aether_ai_formats::normalize_api_format_alias(provider_format);
    let client = aether_ai_formats::normalize_api_format_alias(client_format);
    if provider == client {
        return true;
    }
    // OpenAI Responses family shares the same wire format despite having
    // distinct format IDs.
    is_openai_responses_family(provider_format) && is_openai_responses_family(client_format)
}

fn is_standard_provider_api_format(api_format: &str) -> bool {
    matches!(
        aether_ai_formats::normalize_api_format_alias(api_format).as_str(),
        "openai:chat"
            | "openai:responses"
            | "openai:responses:compact"
            | "claude:messages"
            | "gemini:generate_content"
    )
}

fn is_standard_chat_client_api_format(api_format: &str) -> bool {
    matches!(
        api_format,
        "openai:chat" | "claude:messages" | "gemini:generate_content"
    )
}

fn is_standard_cli_client_api_format(api_format: &str) -> bool {
    matches!(
        aether_ai_formats::normalize_api_format_alias(api_format).as_str(),
        "openai:responses"
            | "openai:responses:compact"
            | "claude:messages"
            | "gemini:generate_content"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        maybe_build_ai_surface_stream_rewriter, resolve_finalize_stream_rewrite_mode,
        FinalizeStreamRewriteMode,
    };

    #[test]
    fn resolves_standard_mode_for_cross_format_standard_streams() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "openai:chat",
            "needs_conversion": true,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::Standard)
        );
    }

    #[test]
    fn resolves_envelope_unwrap_for_same_format_private_envelopes() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "envelope_name": "antigravity:v1internal",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::EnvelopeUnwrap)
        );
    }

    #[test]
    fn resolves_no_rewriter_when_client_consumes_same_private_envelope() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "envelope_name": "antigravity:v1internal",
            "client_envelope_name": "antigravity:v1internal",
            "needs_conversion": false,
        });
        assert_eq!(resolve_finalize_stream_rewrite_mode(&report_context), None);
        assert!(maybe_build_ai_surface_stream_rewriter(Some(&report_context)).is_none());
    }

    #[test]
    fn native_private_envelope_client_keeps_response_wrapper_for_model_display_rewrite() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "envelope_name": "antigravity:v1internal",
            "client_envelope_name": "antigravity:v1internal",
            "model": "gemini-2.5-pro-high",
            "mapped_model": "gemini-2.5-pro",
            "needs_conversion": false,
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("display-model rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"data: {\"response\":{\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[]},\"responseId\":\"resp_native_123\"}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("\"response\":"));
        assert!(output.contains("\"responseId\":\"resp_native_123\""));
        assert!(output.contains("\"modelVersion\":\"gemini-2.5-pro-high\""));
        assert!(!output.contains("_v1internal_response_id"));
    }

    #[test]
    fn resolves_kiro_same_format_streams_to_kiro_mode() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "envelope_name": "kiro:generateAssistantResponse",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::KiroToClaudeCli)
        );
    }

    #[test]
    fn rejects_unsupported_non_conversion_streams() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:chat",
            "needs_conversion": false,
        });
        assert_eq!(resolve_finalize_stream_rewrite_mode(&report_context), None);
    }

    #[test]
    fn resolves_model_directive_display_mode_for_same_format_standard_streams() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "model": "gpt-5.5-xhigh",
            "mapped_model": "gpt-5.5",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::ModelDirectiveDisplay)
        );
    }

    #[test]
    fn model_directive_display_mode_does_not_displace_kiro_stream_bridge() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "envelope_name": "kiro:generateAssistantResponse",
            "model": "claude-sonnet-4.5-high",
            "mapped_model": "claude-sonnet-4.5",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::KiroToClaudeCli)
        );
    }

    #[test]
    fn envelope_unwrap_rewriter_restores_model_directive_display_model() {
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "envelope_name": "gemini_cli:v1internal",
            "model": "gemini-2.5-pro-high",
            "mapped_model": "gemini-2.5-pro",
            "needs_conversion": false,
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"data: {\"response\":{\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[]}}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("\"modelVersion\":\"gemini-2.5-pro-high\""));
        assert!(!output.contains("\"modelVersion\":\"gemini-2.5-pro\""));
    }

    #[test]
    fn model_directive_display_rewriter_restores_response_model() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "model": "gpt-5.5-xhigh",
            "mapped_model": "gpt-5.5",
            "needs_conversion": false,
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5.5\",\"status\":\"in_progress\"}}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("event: response.created"));
        assert!(output.contains("\"model\":\"gpt-5.5-xhigh\""));
        assert!(!output.contains("\"model\":\"gpt-5.5\""));
    }

    #[test]
    fn standard_rewriter_converts_openai_responses_reasoning_delta_to_chat() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "needs_conversion": true,
            "mapped_model": "gpt-5.4",
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"event: response.reasoning_summary_text.delta\n\
data: {\"type\":\"response.reasoning_summary_text.delta\",\"response_id\":\"resp_reasoning_stream_123\",\"item_id\":\"rs_123\",\"output_index\":0,\"summary_index\":0,\"delta\":\"Need to inspect first.\"}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("\"object\":\"chat.completion.chunk\""));
        assert!(output.contains("\"reasoning_content\":\"Need to inspect first.\""));
        assert!(!output.contains("\"content\""));
        assert!(!output.contains("data: [DONE]"));
    }

    #[test]
    fn explicit_responses_stream_format_preserves_function_call_metadata() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "needs_conversion": true,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::OpenAiResponsesCompat)
        );

        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("responses compat rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_chat_metadata_123\",\"call_id\":\"call_chat_metadata_123\",\"status\":\"completed\",\"arguments\":\"{}\",\"name\":\"lookup\",\"metadata\":{\"source\":\"chat\"},\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"turn_123\"}}}\n\n",
            )
            .expect("responses event should pass through");
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("event: response.output_item.added"));
        assert!(output.contains("\"metadata\":{\"source\":\"chat\"}"));
        assert!(output
            .contains("\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"turn_123\"}"));
    }

    #[test]
    fn explicit_responses_stream_format_converts_to_chat_even_without_request_conversion() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "provider_stream_event_api_format": "openai:responses",
            "client_api_format": "openai:chat",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::Standard)
        );

        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("standard rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_chat_metadata_123\",\"call_id\":\"call_chat_metadata_123\",\"status\":\"completed\",\"arguments\":\"{}\",\"name\":\"lookup\",\"metadata\":{\"source\":\"chat\"},\"internal_chat_message_metadata_passthrough\":{\"turn_id\":\"turn_123\"}}}\n\n",
            )
            .expect("responses event should convert to chat");
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("\"object\":\"chat.completion.chunk\""));
        assert!(output.contains("\"id\":\"call_chat_metadata_123\""));
        assert!(!output.contains("unsupported_stream_event"));
    }

    #[test]
    fn same_family_responses_passthrough_preserves_encrypted_content() {
        // When provider and client are both OpenAI Responses family,
        // the stream should pass through verbatim (only model name rewrite).
        // This preserves encrypted_content, original item IDs, etc.
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses:compact",
            "needs_conversion": true,
            "model": "gpt-5.5-xhigh",
            "mapped_model": "gpt-5.5",
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"event: response.output_item.added\n\
data: {\"type\":\"response.output_item.added\",\"response_id\":\"resp_123\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_abc\",\"summary\":[],\"encrypted_content\":\"EWxvY2tlZENvbnRlbnQ=\"}}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        // Passthrough preserves the full payload structure
        assert!(output.contains("event: response.output_item.added"));
        assert!(output.contains("\"encrypted_content\":\"EWxvY2tlZENvbnRlbnQ=\""));
        assert!(output.contains("\"id\":\"rs_abc\""));
        assert!(output.contains("\"type\":\"reasoning\""));
    }

    #[test]
    fn same_family_responses_without_display_model_runs_terminal_compat_only() {
        let report_context = json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses:compact",
            "needs_conversion": true,
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("responses compat rewriter should exist");
        let mut output = rewriter
            .push_chunk(
                b"event: response.output_item.added\n\
data: {\"type\":\"response.output_item.added\",\"response_id\":\"resp_123\",\"output_index\":0,\"item\":{\"type\":\"reasoning\",\"id\":\"rs_abc\",\"encrypted_content\":\"EWxvY2tlZA==\"}}\n\n",
            )
            .expect("non-terminal event should pass through");
        output.extend(
            rewriter
                .push_chunk(
                    b"event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\"}}\n\n",
                )
                .expect("terminal event should be normalized"),
        );
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("\"encrypted_content\":\"EWxvY2tlZA==\""));
        assert!(output.contains("event: response.completed"));
        assert!(output.contains("\"output\":[]"));
        assert!(output.contains("\"output_text\":\"\""));
        assert!(output.contains("\"completed_at\":"));
    }

    #[test]
    fn same_format_claude_passthrough_with_display_model() {
        // Claude→Claude with needs_conversion=true should pass through
        // (only model name rewrite), not parse→rebuild.
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "needs_conversion": true,
            "model": "claude-sonnet-4.5-high",
            "mapped_model": "claude-sonnet-4.5",
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me reason...\"}}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        // Passthrough preserves the exact wire format
        assert!(output.contains("event: content_block_delta"));
        assert!(output.contains("\"thinking\":\"Let me reason...\""));
        assert!(output.contains("\"type\":\"thinking_delta\""));
    }

    #[test]
    fn same_format_claude_uses_read_tool_sanitizer_without_display_model() {
        // Claude→Claude needs a narrow sanitizer for Claude Code Read input.
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "needs_conversion": true,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::ClaudeReadToolSanitize)
        );
    }

    #[test]
    fn same_format_claude_stream_sanitizes_read_start_input() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "needs_conversion": false,
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("same-format claude sanitizer should exist");
        let output = rewriter
            .push_chunk(
                b"event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_read_1\",\"name\":\"Read\",\"input\":{\"file_path\":\"/tmp/a.txt\",\"limit\":20,\"pages\":\"\"}}}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("\"name\":\"Read\""));
        assert!(output.contains("\"file_path\":\"/tmp/a.txt\""));
        assert!(!output.contains("\"pages\":\"\""));
    }

    #[test]
    fn same_format_claude_stream_sanitizes_read_input_json_delta() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "needs_conversion": false,
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("same-format claude sanitizer should exist");
        let mut output = rewriter
            .push_chunk(
                b"event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_read_1\",\"name\":\"Read\",\"input\":{}}}\n\n",
            )
            .expect("start should rewrite");
        output.extend(
            rewriter
                .push_chunk(
                    b"event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"file_path\\\":\\\"/tmp/a.txt\\\",\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"limit\\\":20,\\\"pages\\\":\\\"\\\"}\"}}\n\n",
                )
                .expect("deltas should buffer"),
        );
        let buffered_output = String::from_utf8(output.clone()).expect("output should be utf8");
        assert!(!buffered_output.contains("input_json_delta"));

        output.extend(
            rewriter
                .push_chunk(
                    b"event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                )
                .expect("stop should flush sanitized delta"),
        );
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("event: content_block_delta"));
        assert!(output.contains("\\\"limit\\\":20"));
        assert!(!output.contains("\\\"pages\\\":\\\"\\\""));
        assert!(output.contains("event: content_block_stop"));
    }

    #[test]
    fn same_format_claude_stream_preserves_other_tool_empty_pages() {
        let report_context = json!({
            "provider_api_format": "claude:messages",
            "client_api_format": "claude:messages",
            "needs_conversion": false,
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("same-format claude sanitizer should exist");
        let output = rewriter
            .push_chunk(
                b"event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_search_1\",\"name\":\"Search\",\"input\":{}}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"\\\",\\\"pages\\\":\\\"\\\"}\"}}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        assert!(output.contains("\"name\":\"Search\""));
        assert!(output.contains("\\\"pages\\\":\\\"\\\""));
    }

    #[test]
    fn same_format_gemini_passthrough_with_display_model() {
        // Gemini→Gemini with needs_conversion=true should pass through.
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "needs_conversion": true,
            "model": "gemini-2.5-pro-high",
            "mapped_model": "gemini-2.5-pro",
        });
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("rewriter should exist");
        let output = rewriter
            .push_chunk(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}],\"role\":\"model\"}}],\"modelVersion\":\"gemini-2.5-pro\"}\n\n",
            )
            .expect("rewrite should succeed");
        let output = String::from_utf8(output).expect("output should be utf8");

        // Model version should be rewritten
        assert!(output.contains("\"modelVersion\":\"gemini-2.5-pro-high\""));
        assert!(!output.contains("\"modelVersion\":\"gemini-2.5-pro\""));
    }

    #[test]
    fn same_format_gemini_without_display_model_passes_through_verbatim() {
        // Gemini→Gemini without display model: no rewriter needed.
        let report_context = json!({
            "provider_api_format": "gemini:generate_content",
            "client_api_format": "gemini:generate_content",
            "needs_conversion": true,
        });
        assert!(maybe_build_ai_surface_stream_rewriter(Some(&report_context)).is_none());
    }

    #[test]
    fn resolves_openai_image_mode_for_same_format_image_streams() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::OpenAiImage)
        );
    }

    #[test]
    fn rewrites_openai_image_stream_to_openai_chat_final_chunk() {
        let report_context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:chat",
            "mapped_model": "gpt-image-2",
            "request_id": "trace-image-chat-stream",
            "needs_conversion": false,
        });
        assert_eq!(
            resolve_finalize_stream_rewrite_mode(&report_context),
            Some(FinalizeStreamRewriteMode::OpenAiImageToOpenAiChat)
        );
        let mut rewriter = maybe_build_ai_surface_stream_rewriter(Some(&report_context))
            .expect("image to chat stream rewriter should exist");

        let progress = rewriter
            .push_chunk(
                br#"event: response.image_generation_call.partial_image
data: {"type":"response.image_generation_call.partial_image","partial_image_b64":"cGFydGlhbA=="}

"#,
            )
            .expect("partial image should rewrite as progress");
        let progress_text = String::from_utf8(progress).expect("progress output should be utf8");
        assert!(progress_text.contains("\"object\":\"chat.completion.chunk\""));
        assert!(!progress_text.contains("cGFydGlhbA=="));

        let output_item = rewriter
            .push_chunk(
                br#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"image_generation_call","id":"ig_1","result":"aGVsbG8=","output_format":"png"}}

"#,
            )
            .expect("output item should rewrite");
        let output_item_text = String::from_utf8(output_item).expect("output item should be utf8");
        assert!(output_item_text.is_empty());

        let final_output = rewriter
            .push_chunk(
                br#"event: response.completed
data: {"type":"response.completed","response":{"id":"resp_123","model":"gpt-image-2","tool_usage":{"image_gen":{"total_tokens":0}},"output":[]}}

"#,
            )
            .expect("completed event should rewrite");
        let final_text = String::from_utf8(final_output).expect("final output should be utf8");
        assert!(final_text.contains("\"object\":\"chat.completion.chunk\""));
        assert!(final_text.contains("![generated image](data:image/png;base64,aGVsbG8=)"));
        assert!(final_text.contains("data: [DONE]"));
        assert!(!final_text.contains("image_generation.completed"));
    }
}
