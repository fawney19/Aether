use serde_json::Value;

use crate::{
    protocol::canonical::{CanonicalRequest, CanonicalResponse},
    protocol::formats::{
        claude_messages, gemini_generate_content, openai_chat, openai_responses, FormatId,
    },
};

pub use crate::protocol::context::{FormatContext, FormatError};

pub fn parse_request(
    source_format: &str,
    body: &Value,
    ctx: &FormatContext,
) -> Result<CanonicalRequest, FormatError> {
    let source = parse_format(source_format)?;
    match source {
        FormatId::OpenAiChat => openai_chat::request::from(body, ctx),
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
            openai_responses::request::from(body, ctx)
        }
        FormatId::ClaudeMessages => claude_messages::request::from(body, ctx),
        FormatId::GeminiGenerateContent => gemini_generate_content::request::from(body, ctx),
    }
    .ok_or_else(|| FormatError::RequestParseFailed {
        format: source.as_str().to_string(),
    })
}

pub fn emit_request(
    target_format: &str,
    request: &CanonicalRequest,
    ctx: &FormatContext,
) -> Result<Value, FormatError> {
    let target = parse_format(target_format)?;
    let mut request = request.clone();
    if let Some(mapped_model) = ctx
        .mapped_model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        request.model = mapped_model.to_string();
    }
    match target {
        FormatId::OpenAiChat => openai_chat::request::to(&request, ctx),
        FormatId::OpenAiResponses => openai_responses::request::to(&request, ctx),
        FormatId::OpenAiResponsesCompact => openai_responses::request::to_compact(&request, ctx),
        FormatId::ClaudeMessages => claude_messages::request::to(&request, ctx),
        FormatId::GeminiGenerateContent => gemini_generate_content::request::to(&request, ctx),
    }
    .ok_or_else(|| FormatError::RequestEmitFailed {
        format: target.as_str().to_string(),
    })
}

pub fn convert_request(
    source_format: &str,
    target_format: &str,
    body: &Value,
    ctx: &FormatContext,
) -> Result<Value, FormatError> {
    let request = parse_request(source_format, body, ctx)?;
    emit_request(target_format, &request, ctx)
}

pub fn parse_response(
    source_format: &str,
    body: &Value,
    ctx: &FormatContext,
) -> Result<CanonicalResponse, FormatError> {
    let source = parse_format(source_format)?;
    match source {
        FormatId::OpenAiChat => openai_chat::response::from(body, ctx),
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
            openai_responses::response::from(body, ctx)
        }
        FormatId::ClaudeMessages => claude_messages::response::from(body, ctx),
        FormatId::GeminiGenerateContent => gemini_generate_content::response::from(body, ctx),
    }
    .ok_or_else(|| FormatError::ResponseParseFailed {
        format: source.as_str().to_string(),
    })
}

pub fn emit_response(
    target_format: &str,
    response: &CanonicalResponse,
    ctx: &FormatContext,
) -> Result<Value, FormatError> {
    let target = parse_format(target_format)?;
    match target {
        FormatId::OpenAiChat => openai_chat::response::to(response, ctx),
        FormatId::OpenAiResponses => openai_responses::response::to(response, ctx),
        FormatId::OpenAiResponsesCompact => openai_responses::response::to_compact(response, ctx),
        FormatId::ClaudeMessages => claude_messages::response::to(response, ctx),
        FormatId::GeminiGenerateContent => gemini_generate_content::response::to(response, ctx),
    }
    .ok_or_else(|| FormatError::ResponseEmitFailed {
        format: target.as_str().to_string(),
    })
}

pub fn convert_response(
    source_format: &str,
    target_format: &str,
    body: &Value,
    ctx: &FormatContext,
) -> Result<Value, FormatError> {
    let mut response = parse_response(source_format, body, ctx)?;
    if response.model.trim().is_empty() || response.model == "unknown" {
        if let Some(mapped_model) = ctx
            .mapped_model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            response.model = mapped_model.to_string();
        }
    }
    emit_response(target_format, &response, ctx)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamTranscoderSpec {
    pub source: FormatId,
    pub target: FormatId,
}

pub fn build_stream_transcoder(
    source_format: &str,
    target_format: &str,
    _ctx: &FormatContext,
) -> Result<StreamTranscoderSpec, FormatError> {
    Ok(StreamTranscoderSpec {
        source: parse_format(source_format)?,
        target: parse_format(target_format)?,
    })
}

fn parse_format(format: &str) -> Result<FormatId, FormatError> {
    FormatId::parse(format).ok_or_else(|| FormatError::UnsupportedFormat(format.to_string()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{convert_request, FormatContext};
    use crate::protocol::formats::FormatId;

    #[test]
    fn openai_cli_alias_is_not_a_primary_format() {
        assert_eq!(FormatId::parse("openai:cli"), None);
    }

    #[test]
    fn converts_openai_chat_to_responses_via_registry() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let ctx = FormatContext::default().with_mapped_model("gpt-target");

        let converted = convert_request("openai:chat", "openai:responses", &body, &ctx)
            .expect("request conversion should succeed");

        assert_eq!(converted["model"], "gpt-target");
        assert_eq!(converted["input"][0]["type"], "message");
        assert_eq!(converted["input"][0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn registry_does_not_call_wire_specific_canonical_functions_directly() {
        let implementation = include_str!("registry.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("registry implementation should be readable");
        for forbidden in [
            "canonical_to_openai",
            "canonical_to_claude",
            "canonical_to_gemini",
            "from_openai_chat_to_canonical",
            "from_openai_responses_to_canonical",
            "from_claude_to_canonical",
            "from_gemini_to_canonical",
        ] {
            assert!(
                !implementation.contains(forbidden),
                "registry should dispatch through protocol::formats::<format> adapters, found {forbidden}"
            );
        }
    }
}
