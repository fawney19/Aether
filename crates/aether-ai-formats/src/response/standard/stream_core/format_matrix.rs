use aether_ai_formats::FormatId;
use aether_contracts::{ExecutionStreamTerminalSummary, StandardizedUsage};
use serde_json::Value;

use crate::response::error_body::{
    build_core_error_body_for_client_format, LocalCoreSyncErrorKind,
};
use crate::response::sse::encode_json_sse;
use crate::response::standard::claude::stream::{ClaudeClientEmitter, ClaudeProviderState};
use crate::response::standard::gemini::stream::{GeminiClientEmitter, GeminiProviderState};
use crate::response::standard::openai::stream::{
    OpenAIChatClientEmitter, OpenAIChatProviderState, OpenAIResponsesClientEmitter,
    OpenAIResponsesProviderState,
};
use crate::response::standard::stream_core::common::{
    decode_json_data_line, CanonicalStreamEvent, CanonicalStreamFrame, CanonicalUsage,
};
use crate::response::AiSurfaceFinalizeError;

#[derive(Default)]
pub struct StreamingStandardFormatMatrix {
    provider: Option<ProviderStreamParser>,
    client: Option<ClientStreamEmitter>,
    terminated: bool,
}

impl StreamingStandardFormatMatrix {
    pub fn transform_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.terminated {
            return Ok(Vec::new());
        }
        self.ensure_initialized(report_context);
        if let Some(error_body) = build_client_error_body_for_line(report_context, &line) {
            self.terminated = true;
            return self.emit_error(error_body);
        }
        let Some(provider) = self.provider.as_mut() else {
            return Ok(Vec::new());
        };
        let frames = provider.push_line(report_context, line)?;
        self.emit_frames(frames)
    }

    pub fn finish(&mut self, report_context: &Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.terminated {
            return Ok(Vec::new());
        }
        self.ensure_initialized(report_context);
        let Some(provider) = self.provider.as_mut() else {
            return Ok(Vec::new());
        };
        let frames = provider.finish(report_context)?;
        let mut out = self.emit_frames(frames)?;
        if let Some(client) = self.client.as_mut() {
            out.extend(client.finish()?);
        }
        Ok(out)
    }

    fn ensure_initialized(&mut self, report_context: &Value) {
        if self.provider.is_some() && self.client.is_some() {
            return;
        }

        let provider_api_format = provider_api_format_for_context(report_context);
        let client_api_format = client_api_format_for_context(report_context);

        self.provider = ProviderStreamParser::for_api_format(provider_api_format.as_str());
        self.client = ClientStreamEmitter::for_api_format(client_api_format.as_str());
    }

    fn emit_frames(
        &mut self,
        frames: Vec<CanonicalStreamFrame>,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let Some(client) = self.client.as_mut() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for frame in frames {
            out.extend(client.emit(frame)?);
        }
        Ok(out)
    }

    fn emit_error(&mut self, error_body: Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let Some(client) = self.client.as_mut() else {
            return Ok(Vec::new());
        };
        client.emit_error(error_body)
    }
}

#[derive(Default)]
pub struct StreamingStandardTerminalObserver {
    provider: Option<ProviderStreamParser>,
    latest_summary: Option<ExecutionStreamTerminalSummary>,
}

impl StreamingStandardTerminalObserver {
    pub fn push_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<(), AiSurfaceFinalizeError> {
        self.ensure_initialized(report_context);
        let Some(provider) = self.provider.as_mut() else {
            return Ok(());
        };
        let frames = provider.push_line(report_context, line)?;
        self.observe_frames(frames);
        Ok(())
    }

    pub fn finish(
        &mut self,
        report_context: &Value,
    ) -> Result<Option<ExecutionStreamTerminalSummary>, AiSurfaceFinalizeError> {
        self.ensure_initialized(report_context);
        let Some(provider) = self.provider.as_mut() else {
            return Ok(self.latest_summary.clone());
        };
        let frames = provider.finish(report_context)?;
        self.observe_frames(frames);
        Ok(self.latest_summary.clone())
    }

    pub fn disable_with_error(&mut self, parser_error: impl Into<String>) {
        let parser_error = parser_error.into();
        if let Some(summary) = self.latest_summary.as_mut() {
            if summary.parser_error.is_none() {
                summary.parser_error = Some(parser_error);
            }
        } else {
            self.latest_summary = Some(ExecutionStreamTerminalSummary {
                parser_error: Some(parser_error),
                ..ExecutionStreamTerminalSummary::default()
            });
        }
        self.provider = None;
    }

    pub fn latest_summary(&self) -> Option<&ExecutionStreamTerminalSummary> {
        self.latest_summary.as_ref()
    }

    fn ensure_initialized(&mut self, report_context: &Value) {
        if self.provider.is_some() || self.latest_summary.is_some() {
            return;
        }
        let provider_api_format = provider_api_format_for_context(report_context);
        self.provider = ProviderStreamParser::for_api_format(provider_api_format.as_str());
    }

    fn observe_frames(&mut self, frames: Vec<CanonicalStreamFrame>) {
        for frame in frames {
            self.observe_frame(frame);
        }
    }

    fn observe_frame(&mut self, frame: CanonicalStreamFrame) {
        let CanonicalStreamFrame { id, model, event } = frame;
        let summary = self
            .latest_summary
            .get_or_insert_with(|| ExecutionStreamTerminalSummary {
                response_id: Some(id.clone()),
                model: Some(model.clone()),
                ..ExecutionStreamTerminalSummary::default()
            });
        if summary.response_id.is_none() {
            summary.response_id = Some(id);
        }
        if summary.model.is_none() {
            summary.model = Some(model);
        }
        match event {
            CanonicalStreamEvent::UnknownEvent(_) => {
                summary.unknown_event_count = summary.unknown_event_count.saturating_add(1);
            }
            CanonicalStreamEvent::Finish {
                finish_reason,
                usage,
            } => {
                summary.finish_reason = finish_reason;
                summary.standardized_usage = usage.map(standardized_usage_from_canonical);
                summary.observed_finish = true;
            }
            _ => {}
        }
    }
}

enum ProviderStreamParser {
    OpenAIChat(OpenAIChatProviderState),
    OpenAIResponses(OpenAIResponsesProviderState),
    Claude(ClaudeProviderState),
    Gemini(GeminiProviderState),
}

impl ProviderStreamParser {
    fn for_api_format(provider_api_format: &str) -> Option<Self> {
        Some(match FormatId::parse(provider_api_format)? {
            FormatId::OpenAiChat => Self::OpenAIChat(OpenAIChatProviderState::default()),
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
                Self::OpenAIResponses(OpenAIResponsesProviderState::default())
            }
            FormatId::ClaudeMessages => Self::Claude(ClaudeProviderState::default()),
            FormatId::GeminiGenerateContent => Self::Gemini(GeminiProviderState::default()),
        })
    }

    fn push_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<Vec<CanonicalStreamFrame>, AiSurfaceFinalizeError> {
        match self {
            ProviderStreamParser::OpenAIChat(state) => state.push_line(report_context, line),
            ProviderStreamParser::OpenAIResponses(state) => state.push_line(report_context, line),
            ProviderStreamParser::Claude(state) => state.push_line(report_context, line),
            ProviderStreamParser::Gemini(state) => state.push_line(report_context, line),
        }
    }

    fn finish(
        &mut self,
        report_context: &Value,
    ) -> Result<Vec<CanonicalStreamFrame>, AiSurfaceFinalizeError> {
        match self {
            ProviderStreamParser::OpenAIChat(state) => state.finish(report_context),
            ProviderStreamParser::OpenAIResponses(state) => state.finish(report_context),
            ProviderStreamParser::Claude(state) => state.finish(report_context),
            ProviderStreamParser::Gemini(state) => state.finish(report_context),
        }
    }
}

enum ClientStreamEmitter {
    OpenAIChat(OpenAIChatClientEmitter),
    OpenAIResponses(OpenAIResponsesClientEmitter),
    Claude(ClaudeClientEmitter),
    Gemini(GeminiClientEmitter),
}

fn provider_api_format_for_context(report_context: &Value) -> String {
    string_context_field(report_context, "provider_stream_event_api_format")
        .or_else(|| string_context_field(report_context, "provider_stream_api_format"))
        .or_else(|| string_context_field(report_context, "provider_api_format"))
        .unwrap_or_default()
}

fn string_context_field(report_context: &Value, key: &str) -> Option<String> {
    let value = report_context.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_ascii_lowercase())
}

fn client_api_format_for_context(report_context: &Value) -> String {
    report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn standardized_usage_from_canonical(usage: CanonicalUsage) -> StandardizedUsage {
    let mut standardized = StandardizedUsage::new();
    standardized.input_tokens = usage.input_tokens as i64;
    standardized.output_tokens = usage.output_tokens as i64;
    standardized.cache_creation_tokens = usage.cache_creation_tokens as i64;
    standardized.cache_creation_ephemeral_5m_tokens =
        usage.cache_creation_ephemeral_5m_tokens as i64;
    standardized.cache_creation_ephemeral_1h_tokens =
        usage.cache_creation_ephemeral_1h_tokens as i64;
    standardized.cache_read_tokens = usage.cache_read_tokens as i64;
    standardized.reasoning_tokens = usage.reasoning_tokens as i64;
    standardized.dimensions.insert(
        "total_tokens".to_string(),
        serde_json::json!(usage.total_tokens),
    );
    standardized.normalize_cache_creation_breakdown()
}

impl ClientStreamEmitter {
    fn for_api_format(client_api_format: &str) -> Option<Self> {
        Some(match FormatId::parse(client_api_format)? {
            FormatId::OpenAiChat => Self::OpenAIChat(OpenAIChatClientEmitter::default()),
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
                Self::OpenAIResponses(OpenAIResponsesClientEmitter::default())
            }
            FormatId::ClaudeMessages => Self::Claude(ClaudeClientEmitter::default()),
            FormatId::GeminiGenerateContent => Self::Gemini(GeminiClientEmitter::default()),
        })
    }

    fn emit(&mut self, frame: CanonicalStreamFrame) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        match self {
            ClientStreamEmitter::OpenAIChat(state) => state.emit(frame),
            ClientStreamEmitter::OpenAIResponses(state) => state.emit(frame),
            ClientStreamEmitter::Claude(state) => state.emit(frame),
            ClientStreamEmitter::Gemini(state) => state.emit(frame),
        }
    }

    fn finish(&mut self) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        match self {
            ClientStreamEmitter::OpenAIChat(state) => state.finish(),
            ClientStreamEmitter::OpenAIResponses(state) => state.finish(),
            ClientStreamEmitter::Claude(state) => state.finish(),
            ClientStreamEmitter::Gemini(state) => state.finish(),
        }
    }

    fn emit_error(&mut self, error_body: Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        match self {
            ClientStreamEmitter::OpenAIResponses(state) => state.emit_error(error_body),
            ClientStreamEmitter::Claude(_) => {
                let event = error_body.get("type").and_then(Value::as_str);
                encode_json_sse(event, &error_body)
            }
            ClientStreamEmitter::OpenAIChat(_) | ClientStreamEmitter::Gemini(_) => {
                encode_json_sse(None, &error_body)
            }
        }
    }
}

fn build_client_error_body_for_line(report_context: &Value, line: &[u8]) -> Option<Value> {
    let value = decode_json_data_line(line)?;
    let provider_api_format = provider_api_format_for_context(report_context);
    let client_api_format = report_context
        .get("client_api_format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let (message, code, kind) = parse_provider_error(&provider_api_format, &value)?;
    build_core_error_body_for_client_format(&client_api_format, &message, code.as_deref(), kind)
}

fn parse_provider_error(
    provider_api_format: &str,
    payload: &Value,
) -> Option<(String, Option<String>, LocalCoreSyncErrorKind)> {
    match FormatId::parse(provider_api_format)? {
        FormatId::OpenAiChat | FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
            parse_openai_error(payload)
        }
        FormatId::ClaudeMessages => parse_claude_error(payload),
        FormatId::GeminiGenerateContent => parse_gemini_error(payload),
    }
}

fn parse_openai_error(payload: &Value) -> Option<(String, Option<String>, LocalCoreSyncErrorKind)> {
    let error = payload.get("error")?.as_object()?;
    let message = error.get("message").and_then(Value::as_str)?.to_string();
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let kind = match error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "invalid_request_error" => LocalCoreSyncErrorKind::InvalidRequest,
        "authentication_error" => LocalCoreSyncErrorKind::Authentication,
        "permission_error" => LocalCoreSyncErrorKind::PermissionDenied,
        "not_found_error" => LocalCoreSyncErrorKind::NotFound,
        "rate_limit_error" => LocalCoreSyncErrorKind::RateLimit,
        "context_length_exceeded" => LocalCoreSyncErrorKind::ContextLengthExceeded,
        "overloaded_error" => LocalCoreSyncErrorKind::Overloaded,
        _ => LocalCoreSyncErrorKind::ServerError,
    };
    Some((message, code, kind))
}

fn parse_claude_error(payload: &Value) -> Option<(String, Option<String>, LocalCoreSyncErrorKind)> {
    let error = payload.get("error")?.as_object()?;
    let message = error.get("message").and_then(Value::as_str)?.to_string();
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let kind = match error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "invalid_request_error" => LocalCoreSyncErrorKind::InvalidRequest,
        "authentication_error" => LocalCoreSyncErrorKind::Authentication,
        "permission_error" => LocalCoreSyncErrorKind::PermissionDenied,
        "not_found_error" => LocalCoreSyncErrorKind::NotFound,
        "rate_limit_error" => LocalCoreSyncErrorKind::RateLimit,
        "overloaded_error" => LocalCoreSyncErrorKind::Overloaded,
        _ => LocalCoreSyncErrorKind::ServerError,
    };
    Some((message, code, kind))
}

fn parse_gemini_error(payload: &Value) -> Option<(String, Option<String>, LocalCoreSyncErrorKind)> {
    let error = payload.get("error")?.as_object()?;
    let message = error.get("message").and_then(Value::as_str)?.to_string();
    let code = error.get("code").map(|value| match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    });
    let kind = match error
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "INVALID_ARGUMENT" => LocalCoreSyncErrorKind::InvalidRequest,
        "UNAUTHENTICATED" => LocalCoreSyncErrorKind::Authentication,
        "PERMISSION_DENIED" => LocalCoreSyncErrorKind::PermissionDenied,
        "NOT_FOUND" => LocalCoreSyncErrorKind::NotFound,
        "RESOURCE_EXHAUSTED" => LocalCoreSyncErrorKind::RateLimit,
        "UNAVAILABLE" => LocalCoreSyncErrorKind::Overloaded,
        _ => LocalCoreSyncErrorKind::ServerError,
    };
    let code = code.filter(|value| !value.is_empty());
    Some((message, code, kind))
}

#[cfg(test)]
mod tests {
    use super::{StreamingStandardFormatMatrix, StreamingStandardTerminalObserver};
    use serde_json::{json, Value};

    fn report_context(provider_api_format: &str, client_api_format: &str) -> Value {
        json!({
            "provider_api_format": provider_api_format,
            "client_api_format": client_api_format,
            "mapped_model": "test-model",
        })
    }

    fn data_line(value: Value) -> Vec<u8> {
        format!("data: {}\n", value).into_bytes()
    }

    #[test]
    fn transforms_provider_errors_to_openai_chat_error_bodies() {
        let cases = [
            (
                "openai:chat",
                data_line(json!({
                    "error": {
                        "message": "bad request",
                        "type": "invalid_request_error",
                        "code": "invalid_request",
                    }
                })),
                "\"message\":\"bad request\"",
                "\"type\":\"invalid_request_error\"",
                "\"code\":\"invalid_request\"",
            ),
            (
                "claude:messages",
                data_line(json!({
                    "type": "error",
                    "error": {
                        "message": "slow down",
                        "type": "rate_limit_error",
                        "code": "rate_limit",
                    }
                })),
                "\"message\":\"slow down\"",
                "\"type\":\"rate_limit_error\"",
                "\"code\":\"rate_limit\"",
            ),
            (
                "gemini:generate_content",
                data_line(json!({
                    "error": {
                        "code": 429,
                        "message": "quota exceeded",
                        "status": "RESOURCE_EXHAUSTED",
                    }
                })),
                "\"message\":\"quota exceeded\"",
                "\"type\":\"rate_limit_error\"",
                "\"code\":\"429\"",
            ),
        ];

        for (provider_api_format, line, message, err_type, code) in cases {
            let report_context = report_context(provider_api_format, "openai:chat");
            let mut matrix = StreamingStandardFormatMatrix::default();
            let output = matrix
                .transform_line(&report_context, line)
                .expect("error should convert");
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.starts_with("data: {\"error\":"));
            assert!(!sse.contains("event: "));
            assert!(sse.contains(message));
            assert!(sse.contains(err_type));
            assert!(sse.contains(code));
            assert!(matrix
                .finish(&report_context)
                .expect("finish should succeed")
                .is_empty());
        }
    }

    #[test]
    fn transforms_provider_errors_to_claude_error_events() {
        let cases = [
            (
                "openai:chat",
                data_line(json!({
                    "error": {
                        "message": "bad request",
                        "type": "invalid_request_error",
                        "code": "invalid_request",
                    }
                })),
                "\"message\":\"bad request\"",
                "\"type\":\"invalid_request_error\"",
                "\"code\":\"invalid_request\"",
            ),
            (
                "claude:messages",
                data_line(json!({
                    "type": "error",
                    "error": {
                        "message": "slow down",
                        "type": "rate_limit_error",
                        "code": "rate_limit",
                    }
                })),
                "\"message\":\"slow down\"",
                "\"type\":\"rate_limit_error\"",
                "\"code\":\"rate_limit\"",
            ),
            (
                "gemini:generate_content",
                data_line(json!({
                    "error": {
                        "code": 429,
                        "message": "quota exceeded",
                        "status": "RESOURCE_EXHAUSTED",
                    }
                })),
                "\"message\":\"quota exceeded\"",
                "\"type\":\"rate_limit_error\"",
                "\"code\":\"429\"",
            ),
        ];

        for (provider_api_format, line, message, err_type, code) in cases {
            let report_context = report_context(provider_api_format, "claude:messages");
            let mut matrix = StreamingStandardFormatMatrix::default();
            let output = matrix
                .transform_line(&report_context, line)
                .expect("error should convert");
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.starts_with("event: error\n"));
            assert!(sse.contains("data: {"));
            assert!(sse.contains("\"type\":\"error\""));
            assert!(sse.contains("\"error\":{"));
            assert!(sse.contains(message));
            assert!(sse.contains(err_type));
            assert!(sse.contains(code));
            assert!(matrix
                .finish(&report_context)
                .expect("finish should succeed")
                .is_empty());
        }
    }

    #[test]
    fn transforms_provider_errors_to_gemini_error_bodies() {
        let cases = [
            (
                "openai:chat",
                data_line(json!({
                    "error": {
                        "message": "bad request",
                        "type": "invalid_request_error",
                        "code": "invalid_request",
                    }
                })),
                "\"message\":\"bad request\"",
                "\"code\":400",
                "\"status\":\"INVALID_ARGUMENT\"",
            ),
            (
                "claude:messages",
                data_line(json!({
                    "type": "error",
                    "error": {
                        "message": "slow down",
                        "type": "rate_limit_error",
                        "code": "rate_limit",
                    }
                })),
                "\"message\":\"slow down\"",
                "\"code\":429",
                "\"status\":\"RESOURCE_EXHAUSTED\"",
            ),
            (
                "gemini:generate_content",
                data_line(json!({
                    "error": {
                        "code": 429,
                        "message": "quota exceeded",
                        "status": "RESOURCE_EXHAUSTED",
                    }
                })),
                "\"message\":\"quota exceeded\"",
                "\"code\":429",
                "\"status\":\"RESOURCE_EXHAUSTED\"",
            ),
        ];

        for (provider_api_format, line, message, code, status) in cases {
            let report_context = report_context(provider_api_format, "gemini:generate_content");
            let mut matrix = StreamingStandardFormatMatrix::default();
            let output = matrix
                .transform_line(&report_context, line)
                .expect("error should convert");
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.starts_with("data: {\"error\":"));
            assert!(!sse.contains("event: "));
            assert!(sse.contains(message));
            assert!(sse.contains(code));
            assert!(sse.contains(status));
            assert!(matrix
                .finish(&report_context)
                .expect("finish should succeed")
                .is_empty());
        }
    }

    #[test]
    fn transforms_provider_errors_to_openai_responses_failed_events() {
        let cases = [
            (
                "openai:chat",
                data_line(json!({
                    "error": {
                        "message": "bad request",
                        "type": "invalid_request_error",
                        "code": "invalid_request",
                    }
                })),
                "\"message\":\"bad request\"",
                "\"type\":\"invalid_request_error\"",
                "\"code\":\"invalid_request\"",
            ),
            (
                "claude:messages",
                data_line(json!({
                    "type": "error",
                    "error": {
                        "message": "slow down",
                        "type": "rate_limit_error",
                        "code": "rate_limit",
                    }
                })),
                "\"message\":\"slow down\"",
                "\"type\":\"rate_limit_error\"",
                "\"code\":\"rate_limit\"",
            ),
            (
                "gemini:generate_content",
                data_line(json!({
                    "error": {
                        "code": 429,
                        "message": "quota exceeded",
                        "status": "RESOURCE_EXHAUSTED",
                    }
                })),
                "\"message\":\"quota exceeded\"",
                "\"type\":\"rate_limit_error\"",
                "\"code\":\"429\"",
            ),
        ];

        for (provider_api_format, line, message, err_type, code) in cases {
            let report_context = report_context(provider_api_format, "openai:responses");
            let mut matrix = StreamingStandardFormatMatrix::default();
            let output = matrix
                .transform_line(&report_context, line)
                .expect("error should convert");
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.starts_with("event: response.failed\n"));
            assert!(sse.contains("\"sequence_number\":1"));
            assert!(sse.contains(message));
            assert!(sse.contains(err_type));
            assert!(sse.contains(code));
            assert!(matrix
                .finish(&report_context)
                .expect("finish should succeed")
                .is_empty());
        }
    }

    #[test]
    fn rewrites_gemini_inline_image_streams_to_claude_image_blocks() {
        let report_context = report_context("gemini:generate_content", "claude:messages");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let output = matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "responseId": "resp_media_123",
                    "modelVersion": "gemini-2.5-pro",
                    "candidates": [{
                        "index": 0,
                        "content": {
                            "parts": [
                                { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo=" } }
                            ]
                        }
                    }]
                })),
            )
            .expect("image chunk should rewrite");
        let sse = String::from_utf8(output).expect("sse should be utf8");

        assert!(sse.contains("event: message_start"));
        assert!(sse.contains("\"type\":\"image\""));
        assert!(sse.contains("\"media_type\":\"image/png\""));
        assert!(sse.contains("\"data\":\"iVBORw0KGgo=\""));
    }

    #[test]
    fn rewrites_claude_image_blocks_to_gemini_inline_image_streams() {
        let report_context = report_context("claude:messages", "gemini:generate_content");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let output = matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "iVBORw0KGgo="
                        }
                    }
                })),
            )
            .expect("image chunk should rewrite");
        let sse = String::from_utf8(output).expect("sse should be utf8");

        assert!(
            sse.contains("\"inlineData\":{\"mimeType\":\"image/png\",\"data\":\"iVBORw0KGgo=\"}")
        );
    }

    #[test]
    fn terminal_observer_preserves_claude_cache_usage() {
        let report_context = report_context("claude:messages", "openai:chat");
        let mut observer = StreamingStandardTerminalObserver::default();

        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "message_start",
                    "message": {
                        "id": "msg_cache_123",
                        "model": "claude-sonnet-4-5"
                    }
                })),
            )
            .expect("message_start should parse");
        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": "end_turn"
                    },
                    "usage": {
                        "input_tokens": 6,
                        "output_tokens": 20,
                        "cache_creation_input_tokens": 42262,
                        "cache_read_input_tokens": 0
                    }
                })),
            )
            .expect("message_delta should parse");

        let summary = observer
            .latest_summary()
            .cloned()
            .expect("summary should exist");
        let usage = summary
            .standardized_usage
            .expect("standardized usage should exist");

        assert_eq!(usage.input_tokens, 6);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_creation_tokens, 42_262);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn terminal_observer_uses_explicit_provider_stream_event_api_format() {
        let mut report_context = report_context("openai:chat", "openai:responses");
        report_context["provider_stream_event_api_format"] = json!("openai:responses");
        let mut observer = StreamingStandardTerminalObserver::default();

        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_codex_123",
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
                                "reasoning_tokens": 10,
                            },
                            "total_tokens": 163,
                        },
                    },
                    "sequence_number": 139,
                })),
            )
            .expect("response.completed should parse");

        let summary = observer
            .latest_summary()
            .cloned()
            .expect("summary should exist");
        let usage = summary
            .standardized_usage
            .expect("standardized usage should exist");

        assert_eq!(usage.input_tokens, 26);
        assert_eq!(usage.output_tokens, 137);
        assert_eq!(usage.reasoning_tokens, 10);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn terminal_observer_does_not_infer_provider_stream_event_api_format() {
        let report_context = report_context("openai:chat", "openai:responses");
        let mut observer = StreamingStandardTerminalObserver::default();

        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.completed",
                    "response": {
                        "usage": {
                            "input_tokens": 26,
                            "output_tokens": 137,
                            "total_tokens": 163,
                        },
                    },
                })),
            )
            .expect("line should be ignored by explicitly selected chat parser");

        assert!(
            observer.latest_summary().is_none(),
            "provider stream parser selection must come from report context, not event sniffing"
        );
    }

    #[test]
    fn terminal_observer_counts_unknown_provider_stream_events() {
        let mut report_context = report_context("openai:chat", "openai:responses");
        report_context["provider_stream_event_api_format"] = json!("openai:responses");
        let mut observer = StreamingStandardTerminalObserver::default();

        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.future.delta",
                    "response": {
                        "id": "resp_unknown_123",
                        "model": "gpt-5.4",
                    },
                    "payload": {
                        "kept": true,
                    },
                })),
            )
            .expect("unknown stream event should be observed");

        let summary = observer
            .latest_summary()
            .cloned()
            .expect("summary should exist");
        assert_eq!(summary.response_id.as_deref(), Some("resp_unknown_123"));
        assert_eq!(summary.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(summary.unknown_event_count, 1);
        assert!(!summary.observed_finish);
    }
}
