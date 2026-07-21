use aether_ai_formats::FormatId;
use aether_contracts::{ExecutionStreamTerminalSummary, StandardizedUsage};
use serde_json::Value;

use crate::formats::claude::messages::stream::{ClaudeClientEmitter, ClaudeProviderState};
use crate::formats::gemini::generate_content::stream::{GeminiClientEmitter, GeminiProviderState};
use crate::formats::openai::chat::stream::{
    OpenAIChatClientEmitter, OpenAIChatProviderState, OpenAIResponsesClientEmitter,
    OpenAIResponsesProviderState,
};
use crate::formats::openai::image::stream::OpenAiImageStreamTerminalState;
use crate::formats::shared::error_body::{
    build_core_error_body_for_client_format, LocalCoreSyncErrorKind,
};
use crate::formats::shared::sse::encode_json_sse;
use crate::formats::shared::stream_core::common::{
    decode_json_data_line, openai_stream_terminal_error_body, openai_stream_terminal_error_message,
    unsupported_stream_event_message, CanonicalStreamEvent, CanonicalStreamFrame, CanonicalUsage,
};
use crate::formats::shared::AiSurfaceFinalizeError;

#[derive(Default)]
pub struct StreamingStandardFormatMatrix {
    provider: Option<ProviderStreamParser>,
    client: Option<ClientStreamEmitter>,
    propagated_actual_service_tier: Option<String>,
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
        let (provider, client, propagated_actual_service_tier) = (
            &mut self.provider,
            &mut self.client,
            &mut self.propagated_actual_service_tier,
        );
        let Some(provider) = provider.as_mut() else {
            return Ok(Vec::new());
        };
        let frames = provider.push_line(report_context, line)?;
        if provider.actual_service_tier() != propagated_actual_service_tier.as_deref() {
            *propagated_actual_service_tier = provider.actual_service_tier().map(ToOwned::to_owned);
            if let Some(client) = client.as_mut() {
                client.set_actual_service_tier(propagated_actual_service_tier.as_deref());
            }
        }
        self.emit_frames(frames)
    }

    pub fn finish(&mut self, report_context: &Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if self.terminated {
            return Ok(Vec::new());
        }
        self.ensure_initialized(report_context);
        let (provider, client, propagated_actual_service_tier) = (
            &mut self.provider,
            &mut self.client,
            &mut self.propagated_actual_service_tier,
        );
        let Some(provider) = provider.as_mut() else {
            return Ok(Vec::new());
        };
        let frames = provider.finish(report_context)?;
        if provider.actual_service_tier() != propagated_actual_service_tier.as_deref() {
            *propagated_actual_service_tier = provider.actual_service_tier().map(ToOwned::to_owned);
            if let Some(client) = client.as_mut() {
                client.set_actual_service_tier(propagated_actual_service_tier.as_deref());
            }
        }
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
            if let CanonicalStreamEvent::Finish {
                finish_reason: Some(ref finish_reason),
                ..
            } = frame.event
            {
                if !canonical_stream_finish_reason_is_supported(finish_reason) {
                    self.terminated = true;
                    out.extend(client.emit_unsupported_finish_reason(finish_reason)?);
                    break;
                }
            }
            if let CanonicalStreamEvent::UnknownEvent(payload) = &frame.event {
                self.terminated = true;
                out.extend(client.emit_unknown_event(payload)?);
                break;
            }
            if let CanonicalStreamEvent::OpenAiResponsesOutputItem { raw_event, .. } = &frame.event
            {
                if !matches!(client, ClientStreamEmitter::OpenAIResponses(_)) {
                    self.terminated = true;
                    out.extend(client.emit_unknown_event(raw_event)?);
                    break;
                }
            }
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
    provider: Option<TerminalStreamParser>,
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
        match provider {
            TerminalStreamParser::Standard(provider) => {
                let frames = provider.push_line(report_context, line)?;
                let actual_service_tier = provider.actual_service_tier().map(ToOwned::to_owned);
                self.observe_frames(frames);
                if let Some(actual_service_tier) = actual_service_tier {
                    self.latest_summary
                        .get_or_insert_with(ExecutionStreamTerminalSummary::default)
                        .provider_actual_service_tier = Some(actual_service_tier);
                }
            }
            TerminalStreamParser::OpenAIImage(provider) => {
                if let Some(summary) = provider.push_line(report_context, line)? {
                    self.latest_summary = Some(summary);
                }
            }
        }
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
        match provider {
            TerminalStreamParser::Standard(provider) => {
                let frames = provider.finish(report_context)?;
                self.observe_frames(frames);
            }
            TerminalStreamParser::OpenAIImage(provider) => {
                if let Some(summary) = provider.finish(report_context)? {
                    self.latest_summary = Some(summary);
                }
            }
        }
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
        self.provider = TerminalStreamParser::for_api_format(provider_api_format.as_str());
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
            CanonicalStreamEvent::UnknownEvent(payload)
                if openai_stream_terminal_error_body(&payload).is_some() =>
            {
                summary.unknown_event_count = summary.unknown_event_count.saturating_add(1);
                summary.observed_finish = true;
                summary.finish_reason = Some("error".to_string());
                summary.parser_error = openai_stream_terminal_error_message(&payload);
            }
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

enum TerminalStreamParser {
    Standard(ProviderStreamParser),
    OpenAIImage(OpenAiImageStreamTerminalState),
}

impl TerminalStreamParser {
    fn for_api_format(provider_api_format: &str) -> Option<Self> {
        if provider_api_format
            .trim()
            .eq_ignore_ascii_case("openai:image")
        {
            return Some(Self::OpenAIImage(OpenAiImageStreamTerminalState::default()));
        }
        ProviderStreamParser::for_api_format(provider_api_format).map(Self::Standard)
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
            FormatId::OpenAiEmbedding
            | FormatId::OpenAiSearch
            | FormatId::OpenAiRerank
            | FormatId::GeminiEmbedding
            | FormatId::GeminiInteractions
            | FormatId::JinaEmbedding
            | FormatId::JinaRerank
            | FormatId::DoubaoEmbedding
            | FormatId::AliyunMultimodalEmbedding => return None,
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

    fn actual_service_tier(&self) -> Option<&str> {
        match self {
            ProviderStreamParser::OpenAIChat(state) => state.actual_service_tier(),
            ProviderStreamParser::OpenAIResponses(state) => state.actual_service_tier(),
            ProviderStreamParser::Claude(_) | ProviderStreamParser::Gemini(_) => None,
        }
    }
}

enum ClientStreamEmitter {
    OpenAIChat(OpenAIChatClientEmitter),
    OpenAIResponses(Box<OpenAIResponsesClientEmitter>),
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
                Self::OpenAIResponses(Box::default())
            }
            FormatId::ClaudeMessages => Self::Claude(ClaudeClientEmitter::default()),
            FormatId::GeminiGenerateContent => Self::Gemini(GeminiClientEmitter::default()),
            FormatId::OpenAiEmbedding
            | FormatId::OpenAiSearch
            | FormatId::OpenAiRerank
            | FormatId::GeminiEmbedding
            | FormatId::GeminiInteractions
            | FormatId::JinaEmbedding
            | FormatId::JinaRerank
            | FormatId::DoubaoEmbedding
            | FormatId::AliyunMultimodalEmbedding => return None,
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

    fn set_actual_service_tier(&mut self, value: Option<&str>) {
        match self {
            ClientStreamEmitter::OpenAIChat(state) => state.set_actual_service_tier(value),
            ClientStreamEmitter::OpenAIResponses(state) => state.set_actual_service_tier(value),
            ClientStreamEmitter::Claude(_) | ClientStreamEmitter::Gemini(_) => {}
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

    fn emit_unknown_event(&mut self, payload: &Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let Some(error_body) = build_core_error_body_for_client_format(
            self.api_format(),
            &unsupported_stream_event_message(payload),
            Some("unsupported_stream_event"),
            LocalCoreSyncErrorKind::ServerError,
        ) else {
            return Ok(Vec::new());
        };
        self.emit_error(error_body)
    }

    fn emit_unsupported_finish_reason(
        &mut self,
        finish_reason: &str,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let Some(error_body) = build_core_error_body_for_client_format(
            self.api_format(),
            &format!(
                "Unsupported provider stream finish reason cannot be converted losslessly: field $.finish_reason = {}",
                serde_json::json!(finish_reason)
            ),
            Some("unsupported_finish_reason"),
            LocalCoreSyncErrorKind::ServerError,
        ) else {
            return Ok(Vec::new());
        };
        self.emit_error(error_body)
    }

    fn api_format(&self) -> &'static str {
        match self {
            ClientStreamEmitter::OpenAIChat(_) => "openai:chat",
            ClientStreamEmitter::OpenAIResponses(_) => "openai:responses",
            ClientStreamEmitter::Claude(_) => "claude:messages",
            ClientStreamEmitter::Gemini(_) => "gemini:generate_content",
        }
    }
}

fn canonical_stream_finish_reason_is_supported(finish_reason: &str) -> bool {
    matches!(
        finish_reason.trim(),
        "stop" | "length" | "tool_calls" | "function_call" | "content_filter"
    )
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
        FormatId::GeminiGenerateContent | FormatId::GeminiInteractions => {
            parse_gemini_error(payload)
        }
        FormatId::OpenAiEmbedding
        | FormatId::OpenAiSearch
        | FormatId::OpenAiRerank
        | FormatId::GeminiEmbedding
        | FormatId::JinaEmbedding
        | FormatId::JinaRerank
        | FormatId::DoubaoEmbedding
        | FormatId::AliyunMultimodalEmbedding => None,
    }
}

fn parse_openai_error(payload: &Value) -> Option<(String, Option<String>, LocalCoreSyncErrorKind)> {
    let error_body = openai_stream_terminal_error_body(payload)?;
    let error = error_body.get("error")?.as_object()?;
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

    fn json_data_events(bytes: &[u8]) -> Vec<Value> {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|payload| *payload != "[DONE]")
            .filter_map(|payload| serde_json::from_str(payload).ok())
            .collect()
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
    fn transforms_openai_responses_text_snapshot_deltas_to_openai_chat_without_duplicates() {
        let report_context = report_context("openai:responses", "openai:chat");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let mut output = Vec::new();

        for line in [
            data_line(json!({
                "type": "response.output_text.delta",
                "response_id": "resp_snapshot_delta",
                "output_index": 0,
                "content_index": 0,
                "delta": {
                    "text": "Hello",
                }
            })),
            data_line(json!({
                "type": "response.output_text.delta",
                "response_id": "resp_snapshot_delta",
                "output_index": 0,
                "content_index": 0,
                "delta": {
                    "text": "Hello world",
                }
            })),
            data_line(json!({
                "type": "response.output_text.done",
                "response_id": "resp_snapshot_delta",
                "output_index": 0,
                "content_index": 0,
                "text": "Hello world",
            })),
            data_line(json!({
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
            })),
        ] {
            output.extend(
                matrix
                    .transform_line(&report_context, line)
                    .expect("responses stream line should convert"),
            );
        }

        let sse = String::from_utf8(output).expect("sse should be utf8");
        let content = sse
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|payload| serde_json::from_str::<Value>(payload).ok())
            .filter_map(|value| {
                value
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<String>();

        assert_eq!(content, "Hello world");
        assert!(!sse.contains("HelloHello"));
    }

    #[test]
    fn transforms_chat_backed_function_call_metadata_to_chat_tool_calls() {
        let report_context = report_context("openai:responses", "openai:chat");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let mut output = Vec::new();

        for line in [
            data_line(json!({
                "type": "response.output_item.added",
                "response_id": "resp_chat_metadata_123",
                "output_index": 0,
                "item": {
                    "type": "function_call",
                    "id": "fc_chat_metadata_123",
                    "call_id": "call_chat_metadata_123",
                    "status": "completed",
                    "arguments": "{\"query\":\"aether\"}",
                    "name": "lookup",
                    "metadata": {"source": "chat"},
                    "internal_chat_message_metadata_passthrough": {
                        "turn_id": "turn_123"
                    }
                }
            })),
            data_line(json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_chat_metadata_123",
                    "object": "response",
                    "model": "gpt-5",
                    "status": "completed",
                    "output": [],
                },
            })),
        ] {
            output.extend(
                matrix
                    .transform_line(&report_context, line)
                    .expect("chat-backed function call should convert"),
            );
        }

        let sse = String::from_utf8(output).expect("sse should be utf8");
        assert!(!sse.contains("unsupported_stream_event"), "{sse}");
        assert!(!sse.contains("Unsupported provider stream event"), "{sse}");
        assert!(sse.contains("\"id\":\"call_chat_metadata_123\""), "{sse}");
        assert!(sse.contains("\"name\":\"lookup\""), "{sse}");
        assert!(sse.contains("\\\"query\\\":\\\"aether\\\""), "{sse}");
        assert!(!sse.contains("internal_chat_message_metadata_passthrough"));
        assert!(!sse.contains("\"metadata\""));
    }

    #[test]
    fn ignores_openai_responses_keepalive_events_for_chat_clients() {
        let report_context = report_context("openai:responses", "openai:chat");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let mut output = Vec::new();

        let keepalive = matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "type": "keepalive",
                    "sequence_number": 1,
                })),
            )
            .expect("keepalive should be ignored");
        assert!(keepalive.is_empty());

        for line in [
            data_line(json!({
                "type": "response.output_text.delta",
                "response_id": "resp_keepalive_123",
                "output_index": 0,
                "content_index": 0,
                "delta": "pong",
            })),
            data_line(json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_keepalive_123",
                    "object": "response",
                    "model": "gpt-5.4",
                    "status": "completed",
                    "output": [],
                },
            })),
        ] {
            output.extend(
                matrix
                    .transform_line(&report_context, line)
                    .expect("keepalive and text should convert"),
            );
        }

        let sse = String::from_utf8(output).expect("sse should be utf8");
        assert!(!sse.contains("Unsupported provider stream event"), "{sse}");
        assert!(!sse.contains("unsupported_stream_event"), "{sse}");
        assert!(sse.contains("pong"), "{sse}");
        assert!(sse.contains("chat.completion.chunk"), "{sse}");
    }

    #[test]
    fn ignores_openai_responses_keepalive_events_for_responses_clients() {
        let mut report_context = report_context("openai:chat", "openai:responses");
        report_context["provider_stream_event_api_format"] = json!("openai:responses");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let mut output = Vec::new();

        let keepalive = matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "type": "keepalive",
                    "sequence_number": 1,
                })),
            )
            .expect("keepalive should be ignored");
        assert!(keepalive.is_empty());

        for line in [
            data_line(json!({
                "type": "response.output_text.delta",
                "response_id": "resp_keepalive_456",
                "output_index": 0,
                "content_index": 0,
                "delta": "pong",
            })),
            data_line(json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_keepalive_456",
                    "object": "response",
                    "model": "gpt-5.4",
                    "status": "completed",
                    "output": [],
                },
            })),
        ] {
            output.extend(
                matrix
                    .transform_line(&report_context, line)
                    .expect("keepalive and text should convert"),
            );
        }

        let sse = String::from_utf8(output).expect("sse should be utf8");
        assert!(!sse.contains("Unsupported provider stream event"), "{sse}");
        assert!(!sse.contains("unsupported_stream_event"), "{sse}");
        assert!(sse.contains("pong"), "{sse}");
        assert!(sse.contains("event: response.output_text.delta"), "{sse}");
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
    fn transforms_unknown_provider_stream_events_to_visible_client_errors() {
        let cases = [
            (
                "openai:chat",
                "data: {\"error\":",
                "\"code\":\"unsupported_stream_event\"",
            ),
            (
                "openai:responses",
                "event: response.failed\n",
                "\"code\":\"unsupported_stream_event\"",
            ),
            (
                "claude:messages",
                "event: error\n",
                "\"code\":\"unsupported_stream_event\"",
            ),
            (
                "gemini:generate_content",
                "data: {\"error\":",
                "\"status\":\"INTERNAL\"",
            ),
        ];

        for (client_api_format, prefix, marker) in cases {
            let mut report_context = report_context("openai:responses", client_api_format);
            report_context["provider_stream_event_api_format"] = json!("openai:responses");
            let mut matrix = StreamingStandardFormatMatrix::default();
            let output = matrix
                .transform_line(
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
                .expect("unknown provider event should fail closed visibly");
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.contains(prefix), "{client_api_format}: {sse}");
            assert!(
                sse.contains("Unsupported provider stream event cannot be converted losslessly"),
                "{client_api_format}: {sse}"
            );
            assert!(
                sse.contains("field $.type = \\\"response.future.delta\\\""),
                "{sse}"
            );
            assert!(sse.contains(marker), "{client_api_format}: {sse}");
            assert!(matrix
                .finish(&report_context)
                .expect("finish should succeed")
                .is_empty());
            assert!(matrix
                .transform_line(
                    &report_context,
                    data_line(json!({
                        "type": "response.output_text.delta",
                        "response_id": "resp_unknown_123",
                        "output_index": 0,
                        "content_index": 0,
                        "delta": "after",
                    })),
                )
                .expect("terminated matrix should ignore later lines")
                .is_empty());
        }
    }

    #[test]
    fn responses_compaction_output_is_lossless_within_family_and_rejected_cross_format() {
        let compaction_event = json!({
            "type": "response.output_item.done",
            "item": {
                "type": "compaction",
                "encrypted_content": "ENCRYPTED_CONTEXT_COMPACTION_SUMMARY"
            }
        });

        let mut responses_matrix = StreamingStandardFormatMatrix::default();
        let responses_context = report_context("openai:responses", "openai:responses");
        let mut responses_output = responses_matrix
            .transform_line(&responses_context, data_line(compaction_event.clone()))
            .expect("same-family compaction output should convert");
        responses_output.extend(
            responses_matrix
                .transform_line(
                    &responses_context,
                    data_line(json!({
                        "type": "response.completed",
                        "response": {
                            "id": "resp-compact",
                            "model": "gpt-5.6-sol",
                            "usage": {
                                "input_tokens": 0,
                                "output_tokens": 0,
                                "total_tokens": 0
                            }
                        }
                    })),
                )
                .expect("terminal response should convert"),
        );
        let responses_sse = String::from_utf8(responses_output).expect("valid Responses SSE");
        assert!(responses_sse.contains("event: response.output_item.done\n"));
        assert!(responses_sse.contains("\"type\":\"compaction\""));
        assert!(!responses_sse.contains("\"output_index\""));
        assert!(responses_sse.contains("event: response.completed\n"));

        for client_api_format in ["openai:chat", "claude:messages", "gemini:generate_content"] {
            let mut matrix = StreamingStandardFormatMatrix::default();
            let context = report_context("openai:responses", client_api_format);
            let output = matrix
                .transform_line(&context, data_line(compaction_event.clone()))
                .expect("cross-format rejection should be encoded for the client");
            let sse = String::from_utf8(output).expect("valid error SSE");
            assert!(
                sse.contains("Unsupported provider stream event cannot be converted losslessly")
                    && sse.contains("compaction"),
                "{client_api_format}: {sse}"
            );
            if client_api_format == "gemini:generate_content" {
                assert!(sse.contains("\"status\":\"INTERNAL\""), "{sse}");
            } else {
                assert!(sse.contains("unsupported_stream_event"), "{sse}");
            }
        }
    }

    #[test]
    fn transforms_openai_responses_known_sidecar_events_without_unsupported_errors() {
        let report_context = report_context("openai:responses", "claude:messages");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let mut output = Vec::new();

        for line in [
            data_line(json!({
                "type": "response.created",
                "response": {
                    "id": "resp_sidecar_123",
                    "model": "gpt-5.4",
                    "status": "in_progress",
                    "output": [],
                },
            })),
            data_line(json!({
                "type": "response.output_item.added",
                "response_id": "resp_sidecar_123",
                "output_index": 0,
                "item": {
                    "type": "web_search_call",
                    "id": "ws_123",
                    "status": "in_progress",
                    "action": {"type": "search", "query": "aether format conversion"},
                },
            })),
            data_line(json!({
                "type": "response.web_search_call.searching",
                "item_id": "ws_123",
                "output_index": 0,
            })),
            data_line(json!({
                "type": "response.metadata",
                "response_id": "resp_sidecar_123",
                "sequence_number": 4,
                "metadata": {
                    "candidate_id": "provider-a",
                },
            })),
            data_line(json!({
                "type": "response.output_text.annotation.added",
                "response_id": "resp_sidecar_123",
                "output_index": 1,
                "content_index": 0,
                "annotation_index": 0,
                "annotation": {"type": "url_citation", "url": "https://example.invalid"},
            })),
            data_line(json!({
                "type": "response.output_text.delta",
                "response_id": "resp_sidecar_123",
                "output_index": 1,
                "content_index": 0,
                "delta": "sidecar ok",
            })),
            data_line(json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_sidecar_123",
                    "object": "response",
                    "model": "gpt-5.4",
                    "status": "completed",
                    "output": [],
                    "usage": {
                        "input_tokens": 1,
                        "output_tokens": 2,
                        "total_tokens": 3,
                    },
                },
            })),
        ] {
            output.extend(
                matrix
                    .transform_line(&report_context, line)
                    .expect("known responses sidecar event should convert or be ignored"),
            );
        }

        let sse = String::from_utf8(output).expect("sse should be utf8");
        assert!(!sse.contains("unsupported_stream_event"), "{sse}");
        assert!(!sse.contains("Unsupported provider stream event"), "{sse}");
        assert!(sse.contains("sidecar ok"), "{sse}");
        assert!(sse.contains("event: message_stop"), "{sse}");
    }

    #[test]
    fn transforms_openai_responses_incomplete_max_tokens_as_normal_finish() {
        let report_context = report_context("openai:responses", "claude:messages");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let output = matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "type": "response.incomplete",
                    "response": {
                        "id": "resp_incomplete_123",
                        "object": "response",
                        "model": "gpt-5.4",
                        "status": "incomplete",
                        "incomplete_details": {
                            "reason": "max_output_tokens",
                        },
                        "output": [{
                            "type": "message",
                            "id": "msg_incomplete_123",
                            "role": "assistant",
                            "status": "incomplete",
                            "content": [{
                                "type": "output_text",
                                "text": "partial answer",
                            }],
                        }],
                        "usage": {
                            "input_tokens": 10,
                            "output_tokens": 20,
                            "total_tokens": 30,
                        },
                    },
                })),
            )
            .expect("incomplete max token response should convert as length finish");

        let sse = String::from_utf8(output).expect("sse should be utf8");
        assert!(!sse.contains("Response incomplete"), "{sse}");
        assert!(!sse.contains("unsupported_stream_event"), "{sse}");
        assert!(sse.contains("partial answer"), "{sse}");
        assert!(sse.contains("\"stop_reason\":\"max_tokens\""), "{sse}");
        assert!(matrix
            .finish(&report_context)
            .expect("finish should be terminated")
            .is_empty());
    }

    #[test]
    fn transforms_openai_responses_local_shell_call_to_claude_tool_use() {
        let report_context = report_context("openai:responses", "claude:messages");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let mut output = Vec::new();

        for line in [
            data_line(json!({
                "type": "response.output_item.done",
                "response_id": "resp_shell_123",
                "output_index": 0,
                "item": {
                    "type": "local_shell_call",
                    "id": "lsc_123",
                    "call_id": "call_shell_123",
                    "status": "completed",
                    "action": {
                        "type": "exec",
                        "command": ["pwd"],
                        "env": {},
                    },
                },
            })),
            data_line(json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_shell_123",
                    "object": "response",
                    "model": "gpt-5.4",
                    "status": "completed",
                    "output": [],
                },
            })),
        ] {
            output.extend(
                matrix
                    .transform_line(&report_context, line)
                    .expect("local shell call should convert to a generic tool use"),
            );
        }

        let sse = String::from_utf8(output).expect("sse should be utf8");
        assert!(!sse.contains("unsupported_stream_event"), "{sse}");
        assert!(sse.contains("\"type\":\"tool_use\""), "{sse}");
        assert!(sse.contains("\"name\":\"local_shell\""), "{sse}");
        assert!(sse.contains("\\\"command\\\":[\\\"pwd\\\"]"), "{sse}");
        assert!(sse.contains("\"stop_reason\":\"tool_use\""), "{sse}");
    }

    #[test]
    fn transforms_unknown_stream_finish_reasons_to_visible_client_errors() {
        let cases = [
            (
                "openai:chat",
                "data: {\"error\":",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "openai:responses",
                "event: response.failed\n",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "claude:messages",
                "event: error\n",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "gemini:generate_content",
                "data: {\"error\":",
                "\"status\":\"INTERNAL\"",
            ),
        ];

        for (client_api_format, prefix, marker) in cases {
            let report_context = report_context("openai:chat", client_api_format);
            let mut matrix = StreamingStandardFormatMatrix::default();
            let output = matrix
                .transform_line(
                    &report_context,
                    data_line(json!({
                        "id": "chatcmpl_unknown_finish",
                        "object": "chat.completion.chunk",
                        "model": "gpt-5.4",
                        "choices": [{
                            "index": 0,
                            "delta": {},
                            "finish_reason": "future_reason"
                        }],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 2,
                            "total_tokens": 3
                        }
                    })),
                )
                .expect("unknown finish reason should fail closed visibly");
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.contains(prefix), "{client_api_format}: {sse}");
            assert!(
                sse.contains("Unsupported provider stream finish reason"),
                "{client_api_format}: {sse}"
            );
            assert!(
                sse.contains("field $.finish_reason = \\\"future_reason\\\""),
                "{client_api_format}: {sse}"
            );
            assert!(sse.contains("future_reason"), "{client_api_format}: {sse}");
            assert!(sse.contains(marker), "{client_api_format}: {sse}");
            assert!(matrix
                .finish(&report_context)
                .expect("finish should succeed")
                .is_empty());
        }
    }

    #[test]
    fn transforms_unmappable_gemini_stream_finish_reasons_to_visible_client_errors() {
        let cases = [
            (
                "openai:chat",
                "data: {\"error\":",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "openai:responses",
                "event: response.failed\n",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "claude:messages",
                "event: error\n",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "gemini:generate_content",
                "data: {\"error\":",
                "\"status\":\"INTERNAL\"",
            ),
        ];

        for (client_api_format, prefix, marker) in cases {
            let report_context = report_context("gemini:generate_content", client_api_format);
            let mut matrix = StreamingStandardFormatMatrix::default();
            let output = matrix
                .transform_line(
                    &report_context,
                    data_line(json!({
                        "responseId": "gemini_unmappable_finish",
                        "modelVersion": "gemini-2.5-pro",
                        "candidates": [{
                            "index": 0,
                            "content": {
                                "role": "model",
                                "parts": [{"text": "partial"}]
                            },
                            "finishReason": "OTHER"
                        }],
                        "usageMetadata": {
                            "promptTokenCount": 1,
                            "candidatesTokenCount": 2,
                            "totalTokenCount": 3
                        }
                    })),
                )
                .expect("unmappable Gemini finish reason should fail closed visibly");
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.contains(prefix), "{client_api_format}: {sse}");
            assert!(
                sse.contains("Unsupported provider stream finish reason"),
                "{client_api_format}: {sse}"
            );
            assert!(
                sse.contains("field $.finish_reason = \\\"OTHER\\\""),
                "{client_api_format}: {sse}"
            );
            assert!(sse.contains("OTHER"), "{client_api_format}: {sse}");
            assert!(sse.contains(marker), "{client_api_format}: {sse}");
            assert!(matrix
                .finish(&report_context)
                .expect("finish should succeed")
                .is_empty());
        }
    }

    #[test]
    fn transforms_unknown_claude_stream_stop_reasons_to_visible_client_errors() {
        let cases = [
            (
                "openai:chat",
                "data: {\"error\":",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "openai:responses",
                "event: response.failed\n",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "claude:messages",
                "event: error\n",
                "\"code\":\"unsupported_finish_reason\"",
            ),
            (
                "gemini:generate_content",
                "data: {\"error\":",
                "\"status\":\"INTERNAL\"",
            ),
        ];

        for (client_api_format, prefix, marker) in cases {
            let report_context = report_context("claude:messages", client_api_format);
            let mut matrix = StreamingStandardFormatMatrix::default();
            let output = matrix
                .transform_line(
                    &report_context,
                    data_line(json!({
                        "type": "message_delta",
                        "delta": {
                            "stop_reason": "future_reason"
                        },
                        "usage": {
                            "input_tokens": 1,
                            "output_tokens": 2
                        }
                    })),
                )
                .expect("unknown Claude stop reason should fail closed visibly");
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.contains(prefix), "{client_api_format}: {sse}");
            assert!(
                sse.contains("Unsupported provider stream finish reason"),
                "{client_api_format}: {sse}"
            );
            assert!(sse.contains("future_reason"), "{client_api_format}: {sse}");
            assert!(sse.contains(marker), "{client_api_format}: {sse}");
            assert!(matrix
                .finish(&report_context)
                .expect("finish should succeed")
                .is_empty());
        }
    }

    #[test]
    fn openai_responses_client_emits_incomplete_for_length_finish_reason() {
        let report_context = report_context("openai:chat", "openai:responses");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let output = matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "id": "chatcmpl_length_finish",
                    "object": "chat.completion.chunk",
                    "model": "gpt-5.4",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "length"
                    }],
                    "usage": {
                        "prompt_tokens": 1,
                        "completion_tokens": 2,
                        "total_tokens": 3
                    }
                })),
            )
            .expect("length finish reason should map to response.incomplete");
        let sse = String::from_utf8(output).expect("sse should be utf8");

        assert!(sse.contains("event: response.incomplete\n"));
        assert!(sse.contains("\"status\":\"incomplete\""));
        assert!(sse.contains("\"incomplete_details\":{\"reason\":\"max_output_tokens\"}"));
        assert!(!sse.contains("event: response.completed\n"));
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
                        "error": null,
                        "incomplete_details": null,
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
        assert!(summary.observed_finish);
        assert_eq!(summary.parser_error, None);
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

    #[test]
    fn terminal_observer_marks_openai_responses_failed_event_as_terminal_error() {
        let mut report_context = report_context("openai:chat", "openai:responses");
        report_context["provider_stream_event_api_format"] = json!("openai:responses");
        let mut observer = StreamingStandardTerminalObserver::default();

        observer
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
            .expect("failed event should be observed");

        let summary = observer
            .latest_summary()
            .cloned()
            .expect("summary should exist");
        assert!(summary.observed_finish);
        assert_eq!(summary.finish_reason.as_deref(), Some("error"));
        assert_eq!(summary.parser_error.as_deref(), Some("policy failure"));
        assert_eq!(summary.unknown_event_count, 1);
    }

    #[test]
    fn terminal_observer_marks_openai_responses_incomplete_as_length_finish() {
        let mut report_context = report_context("openai:chat", "openai:responses");
        report_context["provider_stream_event_api_format"] = json!("openai:responses");
        let mut observer = StreamingStandardTerminalObserver::default();

        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.incomplete",
                    "response": {
                        "id": "resp_incomplete_123",
                        "model": "gpt-5.4",
                        "status": "incomplete",
                        "incomplete_details": {
                            "reason": "max_output_tokens",
                        },
                        "output": [],
                        "usage": {
                            "input_tokens": 10,
                            "output_tokens": 20,
                            "total_tokens": 30,
                        },
                    },
                })),
            )
            .expect("incomplete event should be observed as terminal finish");

        let summary = observer
            .latest_summary()
            .cloned()
            .expect("summary should exist");
        assert!(summary.observed_finish);
        assert_eq!(summary.finish_reason.as_deref(), Some("length"));
        assert_eq!(summary.parser_error, None);
        assert_eq!(summary.unknown_event_count, 0);
    }

    #[test]
    fn terminal_observer_preserves_actual_service_tier_without_response_capture() {
        let chat_context = report_context("openai:chat", "openai:chat");
        let mut chat_observer = StreamingStandardTerminalObserver::default();
        chat_observer
            .push_line(
                &chat_context,
                data_line(json!({
                    "id": "chatcmpl_tier_1",
                    "object": "chat.completion.chunk",
                    "model": "gpt-5.6",
                    "service_tier": "Default",
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12},
                })),
            )
            .expect("Chat terminal tier should be observed");
        assert_eq!(
            chat_observer
                .latest_summary()
                .and_then(|summary| summary.provider_actual_service_tier.as_deref()),
            Some("default")
        );
        let chat_summary = chat_observer
            .latest_summary()
            .expect("Chat summary should exist");
        assert!(chat_summary.observed_finish);
        assert_eq!(
            chat_summary
                .standardized_usage
                .as_ref()
                .map(|usage| (usage.input_tokens, usage.output_tokens)),
            Some((10, 2))
        );

        let responses_context = report_context("openai:responses", "openai:responses");
        let mut responses_observer = StreamingStandardTerminalObserver::default();
        responses_observer
            .push_line(
                &responses_context,
                data_line(json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_tier_1",
                        "model": "gpt-5.6",
                        "status": "completed",
                        "service_tier": "Flex",
                        "output": [],
                        "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12},
                    },
                    "sequence_number": 1,
                })),
            )
            .expect("Responses terminal tier should be observed");
        assert_eq!(
            responses_observer
                .latest_summary()
                .and_then(|summary| summary.provider_actual_service_tier.as_deref()),
            Some("flex")
        );
        let responses_summary = responses_observer
            .latest_summary()
            .expect("Responses summary should exist");
        assert!(responses_summary.observed_finish);
        assert_eq!(
            responses_summary
                .standardized_usage
                .as_ref()
                .map(|usage| (usage.input_tokens, usage.output_tokens)),
            Some((10, 2))
        );
    }

    #[test]
    fn openai_chat_client_chunks_carry_provider_actual_service_tier() {
        let context = report_context("openai:chat", "openai:chat");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let output = matrix
            .transform_line(
                &context,
                data_line(json!({
                    "id": "chatcmpl_tier_stream",
                    "object": "chat.completion.chunk",
                    "model": "gpt-5.6",
                    "service_tier": "Default",
                    "choices": [{
                        "index": 0,
                        "delta": {"role": "assistant", "content": "done"},
                        "finish_reason": "stop"
                    }],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12}
                })),
            )
            .expect("Chat stream should transform");
        let events = json_data_events(&output);

        assert!(!events.is_empty());
        assert!(events
            .iter()
            .all(|event| event.get("service_tier") == Some(&json!("default"))));
    }

    #[test]
    fn responses_actual_service_tier_reaches_transformed_chat_chunks() {
        let context = report_context("openai:responses", "openai:chat");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let output = matrix
            .transform_line(
                &context,
                data_line(json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_tier_stream",
                        "object": "response",
                        "model": "gpt-5.6",
                        "status": "completed",
                        "service_tier": "Flex",
                        "output": [{
                            "type": "message",
                            "id": "msg_tier_stream",
                            "role": "assistant",
                            "status": "completed",
                            "content": [{
                                "type": "output_text",
                                "text": "done",
                                "annotations": []
                            }]
                        }],
                        "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}
                    }
                })),
            )
            .expect("Responses stream should transform");
        let events = json_data_events(&output);

        assert!(!events.is_empty());
        assert!(events
            .iter()
            .all(|event| event.get("service_tier") == Some(&json!("flex"))));
    }

    #[test]
    fn terminal_observer_tracks_openai_image_stream_usage() {
        let mut report_context = report_context("openai:image", "openai:chat");
        report_context["image_request"] = json!({
            "size": "1024x1024",
            "quality": "medium",
            "output_format": "png",
        });
        let mut observer = StreamingStandardTerminalObserver::default();

        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.output_item.done",
                    "output_index": 0,
                    "item": {
                        "id": "ig_123",
                        "type": "image_generation_call",
                        "result": "aGVsbG8=",
                    },
                })),
            )
            .expect("image output item should parse");
        observer
            .push_line(&report_context, b"\n".to_vec())
            .expect("image output event should flush");
        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_image_123",
                        "model": "gpt-image-2",
                        "output": [],
                        "tool_usage": {
                            "image_gen": {
                                "input_tokens": 40,
                                "output_tokens": 60,
                                "total_tokens": 100,
                            },
                        },
                    },
                })),
            )
            .expect("image completed should parse");
        observer
            .push_line(&report_context, b"\n".to_vec())
            .expect("image completed event should flush");

        let summary = observer
            .finish(&report_context)
            .expect("image summary should finish")
            .expect("summary should exist");
        let usage = summary
            .standardized_usage
            .expect("standardized usage should exist");

        assert_eq!(summary.response_id.as_deref(), Some("resp_image_123"));
        assert_eq!(summary.model.as_deref(), Some("gpt-image-2"));
        assert_eq!(summary.finish_reason.as_deref(), Some("stop"));
        assert!(summary.observed_finish);
        assert_eq!(usage.input_tokens, 40);
        assert_eq!(usage.output_tokens, 60);
        assert_eq!(usage.request_count, 1);
        assert_eq!(usage.dimensions.get("image_count"), Some(&json!(1)));
        assert_eq!(usage.dimensions.get("total_tokens"), Some(&json!(100)));
        assert_eq!(
            usage.dimensions.get("image_size"),
            Some(&json!("1024x1024"))
        );
        assert_eq!(
            usage.dimensions.get("image_output_format"),
            Some(&json!("png"))
        );
        assert_eq!(
            usage.dimensions.get("image_quality"),
            Some(&json!("medium"))
        );
    }
}
