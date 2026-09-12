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
use crate::formats::openai::responses::history::{
    record_converted_response_history, ResponseHistoryRecord,
};
use crate::formats::shared::error_body::{
    build_core_error_body_for_client_format, LocalCoreSyncErrorKind,
};
use crate::formats::shared::sse::encode_json_sse;
use crate::formats::shared::stream_core::common::{
    canonical_usage_from_openai_usage, decode_json_data_line, openai_stream_terminal_error_body,
    openai_stream_terminal_error_message, unsupported_stream_event_message, CanonicalStreamEvent,
    CanonicalStreamFrame, CanonicalUsage,
};
use crate::formats::shared::AiSurfaceFinalizeError;

const PROVIDER_STREAM_FINISH_ERROR_MESSAGE: &str =
    "Upstream stream ended with finish reason: error";

#[derive(Default)]
pub struct StreamingStandardFormatMatrix {
    provider: Option<ProviderStreamParser>,
    client: Option<ClientStreamEmitter>,
    propagated_actual_service_tier: Option<String>,
    pending_sse_event: Option<String>,
    terminated: bool,
    history_recorded: bool,
    pending_history_record: Option<ResponseHistoryRecord>,
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
        let line = self.apply_sse_event_type(line);
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
        self.emit_frames(report_context, frames)
    }

    fn apply_sse_event_type(&mut self, line: Vec<u8>) -> Vec<u8> {
        apply_sse_event_type(&mut self.pending_sse_event, line)
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
        let mut out = self.emit_frames(report_context, frames)?;
        if let Some(client) = self.client.as_mut() {
            out.extend(client.finish()?);
        }
        self.record_response_history(report_context);
        Ok(out)
    }

    fn ensure_initialized(&mut self, report_context: &Value) {
        if self.provider.is_some() && self.client.is_some() {
            return;
        }

        let provider_api_format = provider_api_format_for_context(report_context);
        let client_api_format = client_api_format_for_context(report_context);

        self.provider = ProviderStreamParser::for_api_format(provider_api_format.as_str());
        self.client =
            ClientStreamEmitter::for_api_format(client_api_format.as_str(), report_context);
    }

    fn emit_frames(
        &mut self,
        report_context: &Value,
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
                    out.extend(client.emit_finish_reason_error(finish_reason)?);
                    break;
                }
            }
            if let CanonicalStreamEvent::UnknownEvent(payload) = &frame.event {
                self.terminated = true;
                if openai_stream_terminal_error_body(payload).is_some() {
                    out.extend(client.emit_terminal_error_frame(frame)?);
                } else {
                    out.extend(client.emit_unknown_event(payload)?);
                }
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
        self.record_response_history(report_context);
        Ok(out)
    }

    fn record_response_history(&mut self, report_context: &Value) {
        if self.history_recorded {
            return;
        }
        let Some(response) = self.client.as_ref().and_then(|client| match client {
            ClientStreamEmitter::OpenAIResponses(emitter) => {
                emitter.completed_response_for_history()
            }
            _ => None,
        }) else {
            return;
        };
        self.pending_history_record = record_converted_response_history(report_context, response);
        self.history_recorded = true;
    }

    pub fn take_response_history_record(&mut self) -> Option<ResponseHistoryRecord> {
        self.pending_history_record.take()
    }

    fn emit_error(&mut self, error_body: Value) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let Some(client) = self.client.as_mut() else {
            return Ok(Vec::new());
        };
        client.emit_error(error_body)
    }
}

fn apply_sse_event_type(pending_sse_event: &mut Option<String>, line: Vec<u8>) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(&line) else {
        return line;
    };
    let trimmed = text.trim_matches('\r').trim();
    if let Some(event) = trimmed.strip_prefix("event:").map(str::trim) {
        *pending_sse_event = (!event.is_empty()).then(|| event.to_string());
        return line;
    }
    if trimmed.is_empty() {
        *pending_sse_event = None;
        return line;
    }
    if !trimmed.starts_with("data:") {
        return line;
    }
    let Some(event) = pending_sse_event.take() else {
        return line;
    };
    let Some(mut payload) = decode_json_data_line(&line) else {
        return line;
    };
    let Some(payload) = payload.as_object_mut() else {
        return line;
    };
    if payload.contains_key("type") {
        return line;
    }
    payload.insert("type".to_string(), Value::String(event));
    let mut normalized = b"data: ".to_vec();
    normalized.extend(serde_json::to_vec(&payload).expect("JSON value serialization cannot fail"));
    normalized.push(b'\n');
    normalized
}

#[derive(Default)]
pub struct StreamingStandardTerminalObserver {
    provider: Option<TerminalStreamParser>,
    latest_summary: Option<ExecutionStreamTerminalSummary>,
    pending_sse_event: Option<String>,
}

impl StreamingStandardTerminalObserver {
    pub fn push_line(
        &mut self,
        report_context: &Value,
        line: Vec<u8>,
    ) -> Result<(), AiSurfaceFinalizeError> {
        self.ensure_initialized(report_context);
        let line = apply_sse_event_type(&mut self.pending_sse_event, line);
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

    /// 结构化入口：给已经持有解析好的协议事件的传输用（Responses WebSocket），
    /// 避免为了复用 [`Self::push_line`] 把事件重新拼成 `data: {json}` 再解析回来。
    ///
    /// 只要 provider 的协议状态机本身接受结构化事件，这条路径与 `push_line`
    /// 完全等价——`push_line` 现在就是「解码 + `push_event`」。
    ///
    /// `openai:image` 的终态状态机没有结构化入口（它按 SSE 行做增量解析），
    /// 这里返回 `Err`，由调用方 `disable_with_error` 把摘要标成 parser_error，
    /// 而不是静默丢事件。
    pub fn push_event(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<(), AiSurfaceFinalizeError> {
        self.ensure_initialized(report_context);
        let Some(provider) = self.provider.as_mut() else {
            return Ok(());
        };
        match provider {
            TerminalStreamParser::Standard(provider) => {
                let frames = provider.push_event(report_context, event)?;
                let actual_service_tier = provider.actual_service_tier().map(ToOwned::to_owned);
                self.observe_frames(frames);
                if let Some(actual_service_tier) = actual_service_tier {
                    self.latest_summary
                        .get_or_insert_with(ExecutionStreamTerminalSummary::default)
                        .provider_actual_service_tier = Some(actual_service_tier);
                }
            }
            TerminalStreamParser::OpenAIImage(_) => {
                return Err(AiSurfaceFinalizeError::new(
                    "openai:image terminal observation has no structured event entry",
                ));
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
                summary.standardized_usage = payload
                    .pointer("/response/usage")
                    .and_then(|usage| canonical_usage_from_openai_usage(Some(usage)))
                    .map(standardized_usage_from_canonical);
            }
            CanonicalStreamEvent::UnknownEvent(_) => {
                summary.unknown_event_count = summary.unknown_event_count.saturating_add(1);
            }
            CanonicalStreamEvent::Finish {
                finish_reason,
                usage,
            } => {
                if let Some(parser_error) = finish_reason
                    .as_deref()
                    .filter(|reason| !canonical_stream_finish_reason_is_supported(reason))
                    .map(|reason| {
                        if reason.trim() == "error" {
                            PROVIDER_STREAM_FINISH_ERROR_MESSAGE.to_string()
                        } else {
                            format!("unsupported provider stream finish reason: {reason}")
                        }
                    })
                {
                    summary.parser_error.get_or_insert(parser_error);
                }
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
        let provider = match ProviderStreamParser::for_api_format(provider_api_format)? {
            ProviderStreamParser::OpenAIChat(_) => {
                ProviderStreamParser::OpenAIChat(OpenAIChatProviderState::terminal_observation())
            }
            ProviderStreamParser::OpenAIResponses(_) => ProviderStreamParser::OpenAIResponses(
                OpenAIResponsesProviderState::terminal_observation(),
            ),
            ProviderStreamParser::Gemini(_) => {
                ProviderStreamParser::Gemini(GeminiProviderState::terminal_observation())
            }
            provider @ ProviderStreamParser::Claude(_) => provider,
        };
        Some(Self::Standard(provider))
    }
}

#[cfg(test)]
#[path = "terminal_observation_tests.rs"]
mod terminal_observation_tests;

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
            | FormatId::OpenAiRealtime
            | FormatId::OpenAiSearch
            | FormatId::OpenAiRerank
            | FormatId::GeminiEmbedding
            | FormatId::GeminiInteractions
            | FormatId::JinaEmbedding
            | FormatId::JinaRerank
            | FormatId::DoubaoEmbedding
            | FormatId::AliyunMultimodalEmbedding
            | FormatId::CodexLive => return None,
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

    /// 结构化入口。目前只有 `openai:responses` 有传输会走它（Responses
    /// WebSocket）；其余格式的协议状态机同样可以按「解码 + push_event」机械拆分，
    /// 等到真有非 SSE 传输需要时再拆，不做无调用方的接口。
    fn push_event(
        &mut self,
        report_context: &Value,
        event: &Value,
    ) -> Result<Vec<CanonicalStreamFrame>, AiSurfaceFinalizeError> {
        match self {
            ProviderStreamParser::OpenAIResponses(state) => state.push_event(report_context, event),
            ProviderStreamParser::OpenAIChat(_)
            | ProviderStreamParser::Claude(_)
            | ProviderStreamParser::Gemini(_) => Err(AiSurfaceFinalizeError::new(
                "this provider stream parser has no structured event entry",
            )),
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
    fn for_api_format(client_api_format: &str, report_context: &Value) -> Option<Self> {
        Some(match FormatId::parse(client_api_format)? {
            FormatId::OpenAiChat => Self::OpenAIChat(OpenAIChatClientEmitter::default()),
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
                Self::OpenAIResponses(Box::new(OpenAIResponsesClientEmitter::with_report_context(
                    report_context,
                )))
            }
            FormatId::ClaudeMessages => Self::Claude(ClaudeClientEmitter::default()),
            FormatId::GeminiGenerateContent => Self::Gemini(GeminiClientEmitter::default()),
            FormatId::OpenAiEmbedding
            | FormatId::OpenAiRealtime
            | FormatId::OpenAiSearch
            | FormatId::OpenAiRerank
            | FormatId::GeminiEmbedding
            | FormatId::GeminiInteractions
            | FormatId::JinaEmbedding
            | FormatId::JinaRerank
            | FormatId::DoubaoEmbedding
            | FormatId::AliyunMultimodalEmbedding
            | FormatId::CodexLive => return None,
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

    fn emit_terminal_error_frame(
        &mut self,
        frame: CanonicalStreamFrame,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        if matches!(
            self,
            ClientStreamEmitter::OpenAIChat(_) | ClientStreamEmitter::OpenAIResponses(_)
        ) {
            return self.emit(frame);
        }
        let CanonicalStreamEvent::UnknownEvent(payload) = frame.event else {
            return self.emit(frame);
        };
        let Some(source_error_body) = openai_stream_terminal_error_body(&payload) else {
            return self.emit_unknown_event(&payload);
        };
        let Some(error) = source_error_body.get("error") else {
            return self.emit_unknown_event(&payload);
        };
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Upstream stream ended with an error");
        let code = error.get("code").and_then(|value| match value {
            Value::String(value) => Some(value.as_str()),
            _ => None,
        });
        let Some(error_body) = build_core_error_body_for_client_format(
            self.api_format(),
            message,
            code,
            LocalCoreSyncErrorKind::ServerError,
        ) else {
            return Ok(Vec::new());
        };
        self.emit_error(error_body)
    }

    fn emit_finish_reason_error(
        &mut self,
        finish_reason: &str,
    ) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
        let (message, code) = if finish_reason.trim() == "error" {
            (
                PROVIDER_STREAM_FINISH_ERROR_MESSAGE.to_string(),
                "stream_terminal_error",
            )
        } else {
            (
                format!(
                    "Unsupported provider stream finish reason cannot be converted losslessly: field $.finish_reason = {}",
                    serde_json::json!(finish_reason)
                ),
                "unsupported_finish_reason",
            )
        };
        let Some(error_body) = build_core_error_body_for_client_format(
            self.api_format(),
            &message,
            Some(code),
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
        | FormatId::OpenAiRealtime
        | FormatId::OpenAiSearch
        | FormatId::OpenAiRerank
        | FormatId::GeminiEmbedding
        | FormatId::JinaEmbedding
        | FormatId::JinaRerank
        | FormatId::DoubaoEmbedding
        | FormatId::AliyunMultimodalEmbedding
        | FormatId::CodexLive => None,
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
    use crate::formats::{
        context::FormatContext, openai::namespace::NamespaceToolAliases, registry::convert_request,
    };
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

    fn event_only_line(event: &str) -> Vec<u8> {
        format!("event: {event}\n").into_bytes()
    }

    #[test]
    fn terminal_observer_marks_malformed_gemini_function_call_as_failure() {
        let context = report_context("gemini:generate_content", "openai:responses");
        let mut observer = StreamingStandardTerminalObserver::default();
        observer
            .push_line(
                &context,
                data_line(json!({
                    "response": {
                        "responseId": "resp_malformed_tool_call",
                        "modelVersion": "gemini-3.7-flash-tiered",
                        "candidates": [{
                            "index": 0,
                            "content": {
                                "role": "model",
                                "parts": [{"thoughtSignature": "signature", "text": ""}]
                            },
                            "finishReason": "MALFORMED_FUNCTION_CALL",
                            "finishMessage": "Malformed function call: Function call is empty - no input to parse."
                        }],
                        "usageMetadata": {
                            "promptTokenCount": 206744,
                            "cachedContentTokenCount": 203947,
                            "thoughtsTokenCount": 1130,
                            "totalTokenCount": 207874
                        }
                    },
                    "responseId": "resp_malformed_tool_call"
                })),
            )
            .expect("Gemini terminal frame should parse");

        let summary = observer
            .finish(&context)
            .expect("terminal observation should finish")
            .expect("Gemini terminal frame should produce a summary");

        assert!(summary.observed_finish);
        assert_eq!(summary.finish_reason.as_deref(), Some("error"));
        assert_eq!(
            summary.parser_error.as_deref(),
            Some("Malformed function call: Function call is empty - no input to parse.")
        );
        let usage = summary
            .standardized_usage
            .expect("failed Gemini terminal should preserve usage");
        assert_eq!(usage.input_tokens, 206744);
        assert_eq!(usage.output_tokens, 1130);
        assert_eq!(usage.cache_read_tokens, 203947);
    }

    #[test]
    fn streams_gemini_thought_text_to_openai_responses_immediately() {
        let context = report_context("gemini:generate_content", "openai:responses");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let output = matrix
            .transform_line(
                &context,
                data_line(json!({
                    "response": {
                        "responseId": "resp_reasoning_123",
                        "modelVersion": "gemini-3.7-flash-tiered",
                        "candidates": [{
                            "index": 0,
                            "content": {
                                "role": "model",
                                "parts": [{"thought": true, "text": "checking"}]
                            }
                        }]
                    }
                })),
            )
            .expect("first Gemini thought chunk should transform");
        let sse = String::from_utf8(output).expect("reasoning SSE should be utf8");

        assert!(
            sse.contains("event: response.reasoning_summary_text.delta\n"),
            "{sse}"
        );
        assert!(sse.contains("\"delta\":\"checking\""), "{sse}");
    }

    #[test]
    fn transforms_malformed_gemini_function_call_to_responses_failed() {
        let context = report_context("gemini:generate_content", "openai:responses");
        let mut matrix = StreamingStandardFormatMatrix::default();
        let output = matrix
            .transform_line(
                &context,
                data_line(json!({
                    "response": {
                        "responseId": "resp_malformed_tool_call",
                        "modelVersion": "gemini-3.7-flash-tiered",
                        "candidates": [{
                            "index": 0,
                            "content": {
                                "role": "model",
                                "parts": [{
                                    "text": "",
                                    "thoughtSignature": "opaque-thought-signature"
                                }]
                            },
                            "finishReason": "MALFORMED_FUNCTION_CALL",
                            "finishMessage": "Malformed function call: Function call is empty - no input to parse."
                        }],
                        "usageMetadata": {
                            "promptTokenCount": 206744,
                            "cachedContentTokenCount": 203947,
                            "thoughtsTokenCount": 1130,
                            "totalTokenCount": 207874
                        }
                    }
                })),
            )
            .expect("malformed Gemini terminal should transform to a stream error");
        let sse = String::from_utf8(output).expect("failed response SSE should be utf8");

        assert!(sse.contains("event: response.failed\n"), "{sse}");
        assert!(sse.contains("\"type\":\"response.failed\""), "{sse}");
        assert!(
            sse.contains("\"code\":\"MALFORMED_FUNCTION_CALL\""),
            "{sse}"
        );
        assert!(
            sse.contains(
                "\"message\":\"Malformed function call: Function call is empty - no input to parse.\""
            ),
            "{sse}"
        );
        assert!(sse.contains("\"input_tokens\":206744"), "{sse}");
        assert!(sse.contains("\"output_tokens\":1130"), "{sse}");
        assert!(sse.contains("\"cached_tokens\":203947"), "{sse}");
        assert!(!sse.contains("unsupported_finish_reason"), "{sse}");
        assert!(matrix
            .finish(&context)
            .expect("failed matrix should stay terminated")
            .is_empty());
    }

    #[test]
    fn event_only_stream_types_convert_across_standard_formats() {
        let responses_payload = json!({
            "response": {
                "id": "resp_event_only_123",
                "model": "gpt-5.4",
                "status": "in_progress",
                "output": [],
            },
        });

        let mut claude_matrix = StreamingStandardFormatMatrix::default();
        let claude_context = report_context("openai:responses", "claude:messages");
        assert!(claude_matrix
            .transform_line(&claude_context, event_only_line("response.created"))
            .expect("event should parse")
            .is_empty());
        let claude_output = claude_matrix
            .transform_line(&claude_context, data_line(responses_payload))
            .expect("response.created should convert to Claude");
        assert!(String::from_utf8_lossy(&claude_output).contains("event: message_start"));

        let mut gemini_matrix = StreamingStandardFormatMatrix::default();
        let gemini_context = report_context("openai:responses", "gemini:generate_content");
        gemini_matrix
            .transform_line(
                &gemini_context,
                event_only_line("response.output_text.delta"),
            )
            .expect("event should parse");
        let gemini_output = gemini_matrix
            .transform_line(
                &gemini_context,
                data_line(json!({
                    "response_id": "resp_event_only_123",
                    "delta": "hello",
                })),
            )
            .expect("text delta should convert to Gemini");
        assert!(String::from_utf8_lossy(&gemini_output).contains("\"text\":\"hello\""));

        let mut chat_matrix = StreamingStandardFormatMatrix::default();
        let chat_context = report_context("claude:messages", "openai:chat");
        chat_matrix
            .transform_line(&chat_context, event_only_line("message_start"))
            .expect("event should parse");
        let chat_output = chat_matrix
            .transform_line(
                &chat_context,
                data_line(json!({
                    "message": {
                        "id": "msg_event_only_123",
                        "model": "claude-sonnet-4-5",
                    },
                })),
            )
            .expect("message_start should convert to Chat");
        assert!(
            String::from_utf8_lossy(&chat_output).contains("\"delta\":{\"role\":\"assistant\"}")
        );
    }

    #[test]
    fn streamed_chat_tool_call_records_responses_continuation_history() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:responses",
            "mapped_model": "deepseek-v4-flash",
            "needs_conversion": true,
            "original_request_body": {
                "model": "deepseek-v4-flash",
                "input": [{"role": "user", "content": "perform a deep scan"}]
            }
        });
        let mut matrix = StreamingStandardFormatMatrix::default();
        matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "id": "chatcmpl_history_stream_test_1",
                    "model": "deepseek-v4-flash",
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "id": "call_history_stream_test_1",
                                "type": "function",
                                "function": {
                                    "name": "create_discovery_manifest",
                                    "arguments": "{\"root\":"
                                }
                            }]
                        },
                        "finish_reason": Value::Null
                    }]
                })),
            )
            .expect("tool start should convert");
        matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "id": "chatcmpl_history_stream_test_1",
                    "model": "deepseek-v4-flash",
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "tool_calls": [{
                                "index": 0,
                                "function": {"arguments": "\"src\"}"}
                            }]
                        },
                        "finish_reason": Value::Null
                    }]
                })),
            )
            .expect("tool arguments should convert");
        let terminal = matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "id": "chatcmpl_history_stream_test_1",
                    "model": "deepseek-v4-flash",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 4,
                        "total_tokens": 14
                    }
                })),
            )
            .expect("tool finish should convert");
        let terminal_sse = String::from_utf8(terminal).expect("SSE should be utf8");
        assert!(terminal_sse.contains("event: response.function_call_arguments.done"));
        assert!(terminal_sse.contains("event: response.output_item.done"));
        assert!(terminal_sse.contains("event: response.completed"));
        let persisted = matrix
            .take_response_history_record()
            .expect("completed stream should expose one persistence record");
        assert!(persisted
            .storage_key
            .starts_with("ai:responses:history:v1:"));
        assert!(persisted.payload.contains("resp_history_stream_test_1"));
        assert!(matrix.take_response_history_record().is_none());

        let continuation = convert_request(
            "openai:responses",
            "openai:chat",
            &json!({
                "model": "deepseek-v4-flash",
                "previous_response_id": "resp_history_stream_test_1",
                "input": [{
                    "type": "function_call_output",
                    "call_id": "call_history_stream_test_1",
                    "output": "manifest-created"
                }]
            }),
            &FormatContext::default(),
        )
        .expect("streamed response history should restore the next Chat request");
        assert_eq!(continuation["messages"][1]["role"], "assistant");
        assert_eq!(
            continuation["messages"][1]["tool_calls"][0]["id"],
            "call_history_stream_test_1"
        );
        assert_eq!(continuation["messages"][2]["role"], "tool");
        assert_eq!(
            continuation["messages"][2]["tool_call_id"],
            "call_history_stream_test_1"
        );
    }

    #[test]
    fn streamed_chat_namespace_tool_call_restores_responses_identity() {
        let report_context = json!({
            "provider_api_format": "openai:chat",
            "client_api_format": "openai:responses",
            "mapped_model": "qwen",
            "needs_conversion": true,
            "original_request_body": {
                "model": "qwen",
                "input": [{"role": "user", "content": "write the report"}],
                "tools": [
                    {
                        "type": "function",
                        "name": "vulnerability_report",
                        "parameters": {"type": "object", "properties": {}}
                    },
                    {
                        "type": "namespace",
                        "name": "mcp__vulnerability_report",
                        "description": "reporting tools",
                        "tools": [{
                            "type": "function",
                            "name": "vulnerability_report",
                            "description": "write the confirmed report",
                            "parameters": {
                                "type": "object",
                                "properties": {"report_path": {"type": "string"}},
                                "required": ["report_path"]
                            },
                            "strict": true
                        }]
                    }
                ]
            }
        });
        let aliases = NamespaceToolAliases::from_report_context(&report_context);
        let chat_name = aliases
            .chat_name("mcp__vulnerability_report", "vulnerability_report")
            .expect("namespace child should have a Chat alias")
            .to_string();
        assert_ne!(chat_name, "vulnerability_report");

        let mut matrix = StreamingStandardFormatMatrix::default();
        let mut output = Vec::new();
        output.extend(
            matrix
                .transform_line(
                    &report_context,
                    data_line(json!({
                        "id": "chatcmpl_namespace_stream_1",
                        "model": "qwen",
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": 0,
                                    "id": "call_namespace_stream_1",
                                    "type": "function",
                                    "function": {
                                        "name": chat_name,
                                        "arguments": "{\"report_path\":"
                                    }
                                }]
                            },
                            "finish_reason": Value::Null
                        }]
                    })),
                )
                .expect("tool start should convert"),
        );
        output.extend(
            matrix
                .transform_line(
                    &report_context,
                    data_line(json!({
                        "id": "chatcmpl_namespace_stream_1",
                        "model": "qwen",
                        "choices": [{
                            "index": 0,
                            "delta": {
                                "tool_calls": [{
                                    "index": 0,
                                    "function": {"arguments": "\"reports/sql-001.md\"}"}
                                }]
                            },
                            "finish_reason": Value::Null
                        }]
                    })),
                )
                .expect("tool arguments should convert"),
        );
        output.extend(
            matrix
                .transform_line(
                    &report_context,
                    data_line(json!({
                        "id": "chatcmpl_namespace_stream_1",
                        "model": "qwen",
                        "choices": [{
                            "index": 0,
                            "delta": {},
                            "finish_reason": "tool_calls"
                        }],
                        "usage": {
                            "prompt_tokens": 10,
                            "completion_tokens": 4,
                            "total_tokens": 14
                        }
                    })),
                )
                .expect("tool finish should convert"),
        );

        let events = json_data_events(&output);
        let added = events
            .iter()
            .find(|event| event["type"] == "response.output_item.added")
            .expect("function-call item should start");
        assert_eq!(added["item"]["name"], "vulnerability_report");
        assert_eq!(added["item"]["namespace"], "mcp__vulnerability_report");
        let done = events
            .iter()
            .find(|event| event["type"] == "response.output_item.done")
            .expect("function-call item should complete");
        assert_eq!(done["item"]["name"], "vulnerability_report");
        assert_eq!(done["item"]["namespace"], "mcp__vulnerability_report");
        let completed = events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .expect("response should complete");
        let function_call = completed["response"]["output"]
            .as_array()
            .expect("response output")
            .iter()
            .find(|item| item["type"] == "function_call")
            .expect("completed function call");
        assert_eq!(function_call["name"], "vulnerability_report");
        assert_eq!(function_call["namespace"], "mcp__vulnerability_report");

        let persisted = matrix
            .take_response_history_record()
            .expect("completed stream should expose response history");
        assert!(persisted.payload.contains("mcp__vulnerability_report"));
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

        let ping = matrix
            .transform_line(
                &report_context,
                data_line(json!({
                    "type": "ping",
                    "cost": "0",
                })),
            )
            .expect("provider ping should be ignored");
        assert!(ping.is_empty());

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
    fn terminal_observer_treats_claude_error_finish_reason_as_upstream_failure() {
        for upstream_message in [None, Some("Provider temporarily overloaded")] {
            let context = report_context("claude:messages", "claude:messages");
            let mut observer = StreamingStandardTerminalObserver::default();
            observer
                .push_line(
                    &context,
                    data_line(json!({
                        "type": "message_start",
                        "message": {
                            "id": "msg_error_finish",
                            "model": "claude-sonnet-4-5",
                            "usage": {
                                "input_tokens": 22,
                                "cache_read_input_tokens": 7,
                                "cache_creation_input_tokens": 3,
                                "cache_creation": { "ephemeral_5m_input_tokens": 3 }
                            }
                        }
                    })),
                )
                .expect("message start should be observed");
            if let Some(message) = upstream_message {
                observer
                    .push_line(
                        &context,
                        data_line(json!({
                            "type": "error",
                            "error": { "type": "overloaded_error", "message": message }
                        })),
                    )
                    .expect("upstream error should be observed");
            }
            observer
                .push_line(
                    &context,
                    data_line(json!({
                        "type": "message_delta",
                        "delta": { "stop_reason": "error" },
                        "usage": { "output_tokens": 5 }
                    })),
                )
                .expect("error finish reason should be observed");
            let summary = observer
                .finish(&context)
                .expect("terminal observation should finish")
                .expect("failed stream should have a summary");

            assert!(summary.observed_finish);
            assert_eq!(summary.finish_reason.as_deref(), Some("error"));
            assert_eq!(
                summary.parser_error.as_deref(),
                Some(upstream_message.unwrap_or("Upstream stream ended with finish reason: error"))
            );
            let usage = summary
                .standardized_usage
                .expect("usage should be retained");
            assert_eq!(usage.input_tokens, 22);
            assert_eq!(usage.output_tokens, 5);
            assert_eq!(usage.cache_read_tokens, 7);
            assert_eq!(usage.cache_creation_tokens, 3);
            assert_eq!(usage.cache_creation_ephemeral_5m_tokens, 3);
        }
    }

    #[test]
    fn transforms_claude_error_finish_reason_to_terminal_errors() {
        let cases = [
            (
                "openai:chat",
                "data: {\"error\":",
                "\"code\":\"stream_terminal_error\"",
            ),
            (
                "openai:responses",
                "event: response.failed\n",
                "\"code\":\"stream_terminal_error\"",
            ),
            (
                "claude:messages",
                "event: error\n",
                "\"code\":\"stream_terminal_error\"",
            ),
            (
                "gemini:generate_content",
                "data: {\"error\":",
                "\"status\":\"INTERNAL\"",
            ),
        ];
        for (client_api_format, prefix, marker) in cases {
            let context = report_context("claude:messages", client_api_format);
            let mut matrix = StreamingStandardFormatMatrix::default();
            let mut output = matrix
                .transform_line(
                    &context,
                    data_line(json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": { "type": "text_delta", "text": "Partial answer" }
                    })),
                )
                .expect("partial response should be emitted");
            output.extend(
                matrix
                    .transform_line(
                        &context,
                        data_line(json!({
                            "type": "message_delta",
                            "delta": { "stop_reason": "error" },
                            "usage": { "output_tokens": 5 }
                        })),
                    )
                    .expect("error finish reason should emit a terminal error"),
            );
            let sse = String::from_utf8(output).expect("sse should be utf8");

            assert!(sse.contains("Partial answer"), "{client_api_format}: {sse}");
            assert!(sse.contains(prefix), "{client_api_format}: {sse}");
            assert!(sse.contains(marker), "{client_api_format}: {sse}");
            assert!(
                sse.contains("Upstream stream ended with finish reason: error"),
                "{client_api_format}: {sse}"
            );
            assert!(
                !sse.contains("unsupported_finish_reason"),
                "{client_api_format}: {sse}"
            );
            assert!(
                !sse.contains("response.completed"),
                "{client_api_format}: {sse}"
            );
            assert!(
                !sse.contains("\"stop_reason\":\"end_turn\""),
                "{client_api_format}: {sse}"
            );
            assert!(
                !sse.contains("\"finish_reason\":\"stop\""),
                "{client_api_format}: {sse}"
            );
            assert!(matrix
                .transform_line(
                    &context,
                    data_line(json!({
                        "type": "message_delta",
                        "delta": { "stop_reason": "end_turn" }
                    }))
                )
                .expect("events after the error should be ignored")
                .is_empty());
            assert!(matrix
                .finish(&context)
                .expect("failed matrix should stay terminated")
                .is_empty());
        }
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
    fn terminal_observer_uses_event_type_when_data_omits_type() {
        let report_context = report_context("openai:responses", "openai:chat");
        let mut observer = StreamingStandardTerminalObserver::default();

        observer
            .push_line(&report_context, event_only_line("response.completed"))
            .expect("event should parse");
        observer
            .push_line(
                &report_context,
                data_line(json!({
                    "response": {
                        "id": "resp_terminal_event_only_123",
                        "model": "gpt-5.4",
                        "status": "completed",
                        "output": [],
                        "usage": {
                            "input_tokens": 2,
                            "output_tokens": 3,
                            "total_tokens": 5,
                        },
                    },
                })),
            )
            .expect("response.completed should parse");

        let summary = observer.latest_summary().expect("summary should exist");
        assert!(summary.observed_finish);
        assert_eq!(
            summary.response_id.as_deref(),
            Some("resp_terminal_event_only_123")
        );
        assert_eq!(
            summary
                .standardized_usage
                .as_ref()
                .map(|usage| usage.input_tokens),
            Some(2)
        );
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

#[cfg(test)]
mod structured_entry_tests {
    use super::StreamingStandardTerminalObserver;
    use aether_contracts::ExecutionStreamTerminalSummary;
    use serde_json::{json, Value};

    fn report_context() -> Value {
        json!({
            "provider_api_format": "openai:responses",
            "client_api_format": "openai:responses",
            "mapped_model": "gpt-5-codex",
        })
    }

    /// 用 SSE 入口观测一组事件。这是 C5 之前 WebSocket 走的路径：把结构化事件
    /// 拼成 `data: {json}` 再交给解析器。
    fn summary_via_push_line(events: &[Value]) -> ExecutionStreamTerminalSummary {
        let context = report_context();
        let mut observer = StreamingStandardTerminalObserver::default();
        for event in events {
            observer
                .push_line(&context, format!("data: {event}\n\n").into_bytes())
                .expect("the SSE entry must accept these events");
        }
        observer
            .finish(&context)
            .expect("the observer must finish")
            .unwrap_or_default()
    }

    /// 用结构化入口观测同一组事件。这是 C5 之后的路径。
    fn summary_via_push_event(events: &[Value]) -> ExecutionStreamTerminalSummary {
        let context = report_context();
        let mut observer = StreamingStandardTerminalObserver::default();
        for event in events {
            observer
                .push_event(&context, event)
                .expect("the structured entry must accept these events");
        }
        observer
            .finish(&context)
            .expect("the observer must finish")
            .unwrap_or_default()
    }

    fn assert_entries_agree(label: &str, events: &[Value]) {
        let via_line = summary_via_push_line(events);
        let via_event = summary_via_push_event(events);
        assert_eq!(
            via_line, via_event,
            "the SSE entry and the structured entry must produce identical summaries for {label}"
        );
    }

    fn created() -> Value {
        json!({"type": "response.created", "response": {"id": "resp_diff", "model": "gpt-5-codex"}})
    }

    fn text_delta(piece: &str) -> Value {
        json!({
            "type": "response.output_text.delta",
            "item_id": "msg_diff",
            "output_index": 0,
            "content_index": 0,
            "delta": piece,
        })
    }

    /// 批量事件：WS 一帧可以带多个协议事件，逐个喂入的结果必须和逐行喂入一致。
    #[test]
    fn a_batched_delta_sequence_agrees_across_both_entries() {
        let events = vec![
            created(),
            text_delta("he"),
            text_delta("ll"),
            text_delta("o"),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_diff",
                    "model": "gpt-5-codex",
                    "status": "completed",
                    "usage": {"input_tokens": 11, "output_tokens": 3, "total_tokens": 14},
                },
            }),
        ];
        assert_entries_agree("a batched delta sequence", &events);
        let summary = summary_via_push_event(&events);
        assert!(summary.observed_finish);
        assert_eq!(summary.response_id.as_deref(), Some("resp_diff"));
        let usage = summary
            .standardized_usage
            .as_ref()
            .expect("completed carries usage");
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 3);
    }

    /// 合法 `response.incomplete`：C1 定过的语义（终态、可计费），两条入口必须
    /// 得到同一个摘要，尤其是 finish_reason 与 parser_error 的取值。
    #[test]
    fn a_legitimate_incomplete_agrees_across_both_entries() {
        let events = vec![
            created(),
            text_delta("partial"),
            json!({
                "type": "response.incomplete",
                "response": {
                    "id": "resp_diff",
                    "model": "gpt-5-codex",
                    "status": "incomplete",
                    "incomplete_details": {"reason": "max_output_tokens"},
                    "usage": {"input_tokens": 7, "output_tokens": 5, "total_tokens": 12},
                },
            }),
        ];
        assert_entries_agree("a legitimate incomplete", &events);
        let summary = summary_via_push_event(&events);
        assert!(summary.observed_finish);
        assert!(
            summary.parser_error.is_none(),
            "a legitimate incomplete is not a parser error: {:?}",
            summary.parser_error
        );
    }

    #[test]
    fn a_terminal_error_agrees_across_both_entries() {
        let events = vec![
            created(),
            json!({
                "type": "error",
                "error": {"type": "server_error", "message": "upstream exploded"},
            }),
        ];
        assert_entries_agree("a terminal error", &events);
        let summary = summary_via_push_event(&events);
        assert!(summary.observed_finish);
        assert_eq!(summary.finish_reason.as_deref(), Some("error"));
        assert!(summary.parser_error.is_some());
    }

    #[test]
    fn a_response_failed_event_agrees_across_both_entries() {
        assert_entries_agree(
            "a response.failed event",
            &[
                created(),
                json!({
                    "type": "response.failed",
                    "response": {
                        "id": "resp_diff",
                        "model": "gpt-5-codex",
                        "status": "failed",
                        "error": {"type": "server_error", "message": "generation failed"},
                    },
                }),
            ],
        );
    }

    /// 未知事件只增计数、不改终态判定，两条入口的计数必须一致。
    #[test]
    fn unknown_events_agree_across_both_entries() {
        let events = vec![
            created(),
            json!({"type": "response.some_future_event", "payload": {"anything": true}}),
            json!({"type": "response.another_future_event"}),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_diff",
                    "model": "gpt-5-codex",
                    "status": "completed",
                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
                },
            }),
        ];
        assert_entries_agree("unknown events", &events);
        let summary = summary_via_push_event(&events);
        assert!(summary.observed_finish);
        assert!(
            summary.unknown_event_count > 0,
            "unknown events are counted"
        );
    }

    /// 供应商声明的 service tier 通过两条入口都要落到摘要上。
    #[test]
    fn a_service_tier_agrees_across_both_entries() {
        assert_entries_agree(
            "a declared service tier",
            &[
                json!({
                    "type": "response.created",
                    "response": {
                        "id": "resp_diff",
                        "model": "gpt-5-codex",
                        "service_tier": "priority",
                    },
                }),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_diff",
                        "model": "gpt-5-codex",
                        "status": "completed",
                        "service_tier": "priority",
                        "usage": {"input_tokens": 2, "output_tokens": 2, "total_tokens": 4},
                    },
                }),
            ],
        );
    }

    /// 没有任何供应商终态事件。注意 `finish()` 会补一个 `stop`（这是 HTTP 与
    /// WebSocket 共享的既有行为，C5 不改），所以 `observed_finish` 为真而 usage
    /// 缺失——真正的「缺终态」判定看的是 usage 与被捕获的 body。这里要钉住的是
    /// 两条入口在这种不完整序列上仍然给出同一个摘要。
    #[test]
    fn a_missing_terminal_agrees_across_both_entries() {
        let events = vec![created(), text_delta("truncated")];
        assert_entries_agree("a missing terminal", &events);
        let summary = summary_via_push_event(&events);
        assert_eq!(summary.finish_reason.as_deref(), Some("stop"));
        assert!(
            summary.standardized_usage.is_none(),
            "a synthesized finish carries no usage"
        );
    }

    /// `openai:image` 没有结构化入口：必须显式报错，让调用方标记 parser_error，
    /// 而不是静默丢掉事件、把摘要留成「未观察到终态」。
    #[test]
    fn the_image_format_rejects_the_structured_entry() {
        let context = json!({
            "provider_api_format": "openai:image",
            "client_api_format": "openai:image",
            "mapped_model": "gpt-image-1",
        });
        let mut observer = StreamingStandardTerminalObserver::default();
        let error = observer
            .push_event(&context, &json!({"type": "image_generation.completed"}))
            .expect_err("openai:image has no structured entry");
        assert!(
            error.to_string().contains("structured event entry"),
            "the error must name the missing entry: {error}"
        );

        observer.disable_with_error(error.to_string());
        let summary = observer
            .latest_summary()
            .expect("disable_with_error records a summary");
        assert!(summary.parser_error.is_some());
    }
}
