use serde_json::{Map, Value};

use crate::formats::{
    aliyun,
    claude::messages as claude_messages,
    doubao,
    gemini::{self, generate_content as gemini_generate_content},
    id::FormatId,
    jina,
    openai::{self, chat as openai_chat, responses as openai_responses},
};
use crate::protocol::canonical::{
    CanonicalContentBlock, CanonicalEmbeddingInput, CanonicalRequest, CanonicalResponse,
    CanonicalStopReason,
};

pub use crate::formats::context::{
    ConversionFieldStatus, ConversionReport, Converted, FormatContext, FormatError,
};

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
        FormatId::OpenAiEmbedding => openai::embedding::request::from(body, ctx),
        FormatId::JinaEmbedding => jina::embedding::request::from(body, ctx),
        FormatId::OpenAiRerank => openai::rerank::request::from(body, ctx),
        FormatId::JinaRerank => jina::rerank::request::from(body, ctx),
        FormatId::GeminiEmbedding => gemini::embedding::request::from(body, ctx),
        FormatId::DoubaoEmbedding => doubao::embedding::request::from(body, ctx),
        FormatId::AliyunMultimodalEmbedding => aliyun::embedding::request::from(body, ctx),
        FormatId::OpenAiSearch | FormatId::GeminiInteractions => None,
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
    emit_request_inner(target_format, request, &ctx.without_runtime_request_edits())
}

fn emit_request_inner(
    target_format: &str,
    request: &CanonicalRequest,
    ctx: &FormatContext,
) -> Result<Value, FormatError> {
    let target = parse_format(target_format)?;
    match target {
        FormatId::OpenAiChat => openai_chat::request::to(request, ctx),
        FormatId::OpenAiResponses => openai_responses::request::to(request, ctx),
        FormatId::OpenAiResponsesCompact => openai_responses::request::to_compact(request, ctx),
        FormatId::ClaudeMessages => claude_messages::request::to(request, ctx),
        FormatId::GeminiGenerateContent => gemini_generate_content::request::to(request, ctx),
        FormatId::OpenAiEmbedding => openai::embedding::request::to(request, ctx),
        FormatId::JinaEmbedding => jina::embedding::request::to(request, ctx),
        FormatId::OpenAiRerank => openai::rerank::request::to(request, ctx),
        FormatId::JinaRerank => jina::rerank::request::to(request, ctx),
        FormatId::GeminiEmbedding => gemini::embedding::request::to(request, ctx),
        FormatId::DoubaoEmbedding => doubao::embedding::request::to(request, ctx),
        FormatId::AliyunMultimodalEmbedding => aliyun::embedding::request::to(request, ctx),
        FormatId::OpenAiSearch | FormatId::GeminiInteractions => None,
    }
    .ok_or_else(|| FormatError::RequestEmitFailed {
        format: target.as_str().to_string(),
    })
}

pub fn parse_request_pure(
    source_format: &str,
    body: &Value,
) -> Result<CanonicalRequest, FormatError> {
    parse_request(source_format, body, &FormatContext::default())
}

pub fn emit_request_pure(
    target_format: &str,
    request: &CanonicalRequest,
) -> Result<Value, FormatError> {
    emit_request_inner(target_format, request, &FormatContext::default())
}

pub fn convert_request_pure(
    source_format: &str,
    target_format: &str,
    body: &Value,
) -> Result<Converted<Value>, FormatError> {
    convert_request_pure_with_context(
        source_format,
        target_format,
        body,
        &FormatContext::default(),
    )
}

pub fn convert_request_pure_with_context(
    source_format: &str,
    target_format: &str,
    body: &Value,
    ctx: &FormatContext,
) -> Result<Converted<Value>, FormatError> {
    let pure_ctx = ctx.without_runtime_request_edits();
    let request = parse_request(source_format, body, &pure_ctx)?;
    validate_openai_responses_target_contract(target_format, body)?;
    validate_request_conversion(
        source_format,
        target_format,
        body,
        &request,
        ctx.mapped_model.as_deref(),
    )?;
    let value = emit_request_inner(target_format, &request, &pure_ctx)?;
    let report = build_request_conversion_report(source_format, target_format, body, &value);
    Ok(Converted { value, report })
}

pub fn convert_request(
    source_format: &str,
    target_format: &str,
    body: &Value,
    ctx: &FormatContext,
) -> Result<Value, FormatError> {
    let source = parse_format(source_format)?;
    let target = parse_format(target_format)?;
    validate_openai_responses_target_contract(target_format, body)?;
    let mut request = parse_request(source_format, body, ctx)?;
    validate_runtime_request_conversion(
        source,
        target,
        body,
        &request,
        ctx.mapped_model.as_deref(),
    )?;
    if let Some(mapped_model) = ctx
        .mapped_model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        request.model = mapped_model.to_string();
    }
    emit_request_inner(target_format, &request, ctx)
}

fn validate_runtime_request_conversion(
    source: FormatId,
    target: FormatId,
    body: &Value,
    _request: &CanonicalRequest,
    mapped_model: Option<&str>,
) -> Result<(), FormatError> {
    validate_openai_cross_format_store(source, target, body)?;
    validate_openai_prompt_cache_contract(source, body, mapped_model)?;
    validate_openai_reasoning_effort(source, target, body, mapped_model)?;
    validate_openai_responses_cross_format_input(source, target, body)?;
    validate_openai_responses_runtime_reasoning(source, target, body)?;
    if matches!(source, FormatId::OpenAiChat)
        && matches!(
            target,
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
        )
        && body
            .as_object()
            .is_some_and(openai_chat_request_has_unsupported_responses_cache_breakpoint)
    {
        return Err(FormatError::LossyConversionBlocked {
            source_format: source.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "messages[].content[].prompt_cache_breakpoint".to_string(),
            reason: "OpenAI Responses supports prompt cache breakpoints only on input_text, input_image, and input_file blocks".to_string(),
        });
    }
    if source == FormatId::ClaudeMessages {
        match target {
            FormatId::OpenAiChat
                if claude_request_contains_unrepresentable_tool_result_content_for_openai_chat(
                    body,
                ) =>
            {
                return Err(FormatError::LossyConversionBlocked {
                    source_format: source.as_str().to_string(),
                    target_format: target.as_str().to_string(),
                    field: "messages[].content[].tool_result.content".to_string(),
                    reason: "OpenAI Chat tool messages cannot represent one or more Claude tool_result content blocks".to_string(),
                });
            }
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
                if claude_request_contains_unrepresentable_tool_result_content_for_openai_responses(
                    body,
                ) =>
            {
                return Err(FormatError::LossyConversionBlocked {
                    source_format: source.as_str().to_string(),
                    target_format: target.as_str().to_string(),
                    field: "messages[].content[].tool_result.content".to_string(),
                    reason: "OpenAI Responses function_call_output cannot represent one or more Claude tool_result content blocks".to_string(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_openai_responses_target_contract(
    target_format: &str,
    body: &Value,
) -> Result<(), FormatError> {
    if !matches!(
        FormatId::parse(target_format),
        Some(FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact)
    ) {
        return Ok(());
    }
    crate::formats::openai::responses::request::validate_openai_responses_request_contract(
        body,
        target_format,
    )
    .map_err(|violation| FormatError::InvalidTargetField {
        format: crate::normalize_api_format_alias(target_format),
        field: violation.field.to_string(),
        reason: violation.reason.to_string(),
    })
}

fn validate_openai_responses_runtime_reasoning(
    source: FormatId,
    target: FormatId,
    body: &Value,
) -> Result<(), FormatError> {
    if !matches!(
        source,
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
    ) || matches!(
        target,
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
    ) {
        return Ok(());
    }
    let Some(reasoning) = body.get("reasoning").and_then(Value::as_object) else {
        return Ok(());
    };
    if let Some(field) = reasoning
        .keys()
        .find(|field| !matches!(field.as_str(), "effort" | "summary"))
    {
        return openai_responses_lossy_input(
            source,
            target,
            format!("thinking.openai_responses.{field}"),
            "target format cannot preserve this provider-specific Responses reasoning field",
        );
    }
    Ok(())
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
        FormatId::OpenAiEmbedding
        | FormatId::OpenAiSearch
        | FormatId::JinaEmbedding
        | FormatId::OpenAiRerank
        | FormatId::JinaRerank
        | FormatId::GeminiEmbedding
        | FormatId::DoubaoEmbedding
        | FormatId::AliyunMultimodalEmbedding
        | FormatId::GeminiInteractions => None,
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
    emit_response_inner(
        target_format,
        response,
        &ctx.without_runtime_request_edits(),
    )
}

fn emit_response_inner(
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
        FormatId::OpenAiEmbedding
        | FormatId::OpenAiSearch
        | FormatId::JinaEmbedding
        | FormatId::OpenAiRerank
        | FormatId::JinaRerank
        | FormatId::GeminiEmbedding
        | FormatId::DoubaoEmbedding
        | FormatId::AliyunMultimodalEmbedding
        | FormatId::GeminiInteractions => None,
    }
    .ok_or_else(|| FormatError::ResponseEmitFailed {
        format: target.as_str().to_string(),
    })
}

pub fn parse_response_pure(
    source_format: &str,
    body: &Value,
) -> Result<CanonicalResponse, FormatError> {
    parse_response(source_format, body, &FormatContext::default())
}

pub fn emit_response_pure(
    target_format: &str,
    response: &CanonicalResponse,
) -> Result<Value, FormatError> {
    emit_response_inner(target_format, response, &FormatContext::default())
}

pub fn convert_response_pure(
    source_format: &str,
    target_format: &str,
    body: &Value,
) -> Result<Converted<Value>, FormatError> {
    let response = parse_response_pure(source_format, body)?;
    validate_response_conversion(source_format, target_format, body, &response)?;
    let value = emit_response_pure(target_format, &response)?;
    let report = build_response_conversion_report(source_format, target_format, body, &value);
    Ok(Converted { value, report })
}

pub fn convert_response(
    source_format: &str,
    target_format: &str,
    body: &Value,
    ctx: &FormatContext,
) -> Result<Value, FormatError> {
    let mut response = parse_response(source_format, body, ctx)?;
    validate_response_conversion(source_format, target_format, body, &response)?;
    if response.model.trim().is_empty() || response.model == "unknown" {
        if let Some(mapped_model) = ctx
            .mapped_model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            response.model = mapped_model.to_string();
        }
    }
    emit_response_inner(target_format, &response, ctx)
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

fn validate_request_conversion(
    source_format: &str,
    target_format: &str,
    body: &Value,
    request: &CanonicalRequest,
    mapped_model: Option<&str>,
) -> Result<(), FormatError> {
    let source = parse_format(source_format)?;
    let target = parse_format(target_format)?;
    validate_openai_prompt_cache_contract(source, body, mapped_model)?;
    validate_openai_reasoning_effort(source, target, body, mapped_model)?;
    if source == target {
        return Ok(());
    }
    if is_embedding_format(source) || is_embedding_format(target) {
        return validate_embedding_request_conversion(source, target, request);
    }
    if is_rerank_format(source) || is_rerank_format(target) {
        return validate_rerank_request_conversion(source, target, request);
    }
    validate_known_standard_request_root_fields(source, target, body)?;
    validate_openai_responses_cross_format_input(source, target, body)?;
    validate_cross_format_generation_target(source, target, request)?;
    validate_openai_cross_format_store(source, target, body)?;
    match (source, target) {
        (FormatId::OpenAiChat, FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact) => {
            validate_openai_chat_to_responses(body)?;
        }
        (FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact, FormatId::OpenAiChat) => {
            validate_openai_responses_to_chat(body, request)?;
        }
        (FormatId::ClaudeMessages, target) if target != FormatId::ClaudeMessages => {
            validate_claude_cross_format_request(body, target)?;
        }
        (FormatId::GeminiGenerateContent, target) if target != FormatId::GeminiGenerateContent => {
            validate_gemini_cross_format_request(body, target)?;
        }
        _ => {}
    }
    validate_cross_format_request_extensions(source, target, request)
}

fn validate_openai_responses_cross_format_input(
    source: FormatId,
    target: FormatId,
    body: &Value,
) -> Result<(), FormatError> {
    if !matches!(
        source,
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
    ) || matches!(
        target,
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
    ) {
        return Ok(());
    }
    if body
        .get("multi_agent")
        .and_then(Value::as_object)
        .and_then(|multi_agent| multi_agent.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return openai_responses_lossy_input(
            source,
            target,
            "multi_agent".to_string(),
            "target format cannot represent the OpenAI multi-agent request contract",
        );
    }

    let Some(items) = body.get("input").and_then(Value::as_array) else {
        return Ok(());
    };
    for (item_index, item) in items.iter().enumerate() {
        let Some(item_object) = item.as_object() else {
            if !item.is_string() {
                return openai_responses_lossy_input(
                    source,
                    target,
                    format!("input[{item_index}]"),
                    "target format cannot represent this raw Responses input item",
                );
            }
            continue;
        };
        let item_type = item_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("message")
            .trim()
            .to_ascii_lowercase();
        if !matches!(
            item_type.as_str(),
            "message"
                | "reasoning"
                | "function_call"
                | "custom_tool_call"
                | "function_call_output"
                | "custom_tool_call_output"
                | "local_shell_call_output"
                | "shell_call_output"
                | "apply_patch_call_output"
                | "computer_call_output"
        ) {
            return openai_responses_lossy_input(
                source,
                target,
                format!("input[{item_index}]"),
                "target format has no lossless mapping for this Responses input item type",
            );
        }
        if matches!(item_type.as_str(), "function_call" | "function_call_output")
            && item_object.contains_key("caller")
        {
            return openai_responses_lossy_input(
                source,
                target,
                format!("input[{item_index}].caller"),
                "target format cannot preserve function call caller provenance",
            );
        }
        if item_type != "message" {
            continue;
        }
        let Some(content) = item_object.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (content_index, block) in content.iter().enumerate() {
            let block_type = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if !matches!(
                block_type.as_str(),
                "input_text"
                    | "output_text"
                    | "text"
                    | "input_image"
                    | "output_image"
                    | "image_url"
                    | "input_file"
                    | "file"
                    | "input_audio"
                    | "audio"
                    | "output_audio"
                    | "reasoning"
                    | "thinking"
                    | "refusal"
            ) {
                return openai_responses_lossy_input(
                    source,
                    target,
                    format!("input[{item_index}].content[{content_index}]"),
                    "target format has no lossless mapping for this raw Responses content block",
                );
            }
        }
    }
    Ok(())
}

fn openai_responses_lossy_input(
    source: FormatId,
    target: FormatId,
    field: String,
    reason: &str,
) -> Result<(), FormatError> {
    Err(FormatError::LossyConversionBlocked {
        source_format: source.as_str().to_string(),
        target_format: target.as_str().to_string(),
        field,
        reason: reason.to_string(),
    })
}

fn validate_response_conversion(
    source_format: &str,
    target_format: &str,
    body: &Value,
    response: &CanonicalResponse,
) -> Result<(), FormatError> {
    let source = parse_format(source_format)?;
    let target = parse_format(target_format)?;
    if source == target {
        return Ok(());
    }

    validate_source_response_stop_enums(source, target, body)?;
    validate_response_content_has_no_unknown_blocks(source, target, response)?;
    validate_openai_responses_cross_format_response_extensions(source, target, response)?;
    validate_canonical_response_stop_reasons(source, target, response)
}

fn validate_openai_responses_cross_format_response_extensions(
    source: FormatId,
    target: FormatId,
    response: &CanonicalResponse,
) -> Result<(), FormatError> {
    if !matches!(
        source,
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
    ) || matches!(
        target,
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
    ) {
        return Ok(());
    }

    validate_openai_responses_response_extension_namespace(
        source,
        target,
        "response",
        &response.extensions,
        |key, value| {
            matches!(key, "raw_status" | "incomplete_details")
                || (key == "output_text" && value.is_string())
                || (matches!(key, "created_at" | "completed_at")
                    && (value.is_number() || value.is_null()))
                || (key == "error" && value.is_null())
                || (target == FormatId::OpenAiChat && key == "service_tier")
        },
    )?;

    for output in &response.outputs {
        validate_openai_responses_message_item_provenance(source, target, &output.extensions)?;
        validate_openai_responses_response_extension_namespace(
            source,
            target,
            "output[]",
            &output.extensions,
            |key, _| key == "message_items",
        )?;
    }

    if let Some(usage) = &response.usage {
        validate_openai_responses_usage_extension_fields(source, target, &usage.extensions)?;
        validate_openai_responses_response_extension_namespace(
            source,
            target,
            "usage",
            &usage.extensions,
            |key, _| matches!(key, "input_tokens_details" | "output_tokens_details"),
        )?;
    }

    for block in &response.content {
        if let CanonicalContentBlock::Thinking {
            encrypted_content: Some(encrypted_content),
            ..
        } = block
        {
            if !encrypted_content.is_empty() {
                return openai_responses_lossy_response(
                    source,
                    target,
                    "output[].encrypted_content",
                    "target format cannot preserve encrypted OpenAI reasoning content",
                );
            }
        }
        let (location, extensions): (&str, &std::collections::BTreeMap<String, Value>) = match block
        {
            CanonicalContentBlock::Text { extensions, .. } => ("output[].content[]", extensions),
            CanonicalContentBlock::Thinking { extensions, .. } => {
                ("output[].reasoning", extensions)
            }
            CanonicalContentBlock::Image { extensions, .. }
            | CanonicalContentBlock::File { extensions, .. }
            | CanonicalContentBlock::Audio { extensions, .. } => ("output[].content[]", extensions),
            CanonicalContentBlock::ToolUse { extensions, .. } => {
                ("output[].function_call", extensions)
            }
            CanonicalContentBlock::ToolResult { extensions, .. } => {
                ("output[].function_call_output", extensions)
            }
            CanonicalContentBlock::Unknown { .. } => continue,
        };
        let has_chat_metadata_passthrough = extensions.iter().any(|(namespace, value)| {
            matches!(namespace.as_str(), "openai_responses" | "openai_cli")
                && value.as_object().is_some_and(|object| {
                    object.contains_key("internal_chat_message_metadata_passthrough")
                })
        });
        validate_openai_responses_response_extension_namespace(
            source,
            target,
            location,
            extensions,
            |key, value| match block {
                CanonicalContentBlock::Text { .. } => {
                    key == "annotations"
                        && (target == FormatId::OpenAiChat
                            || value.as_array().is_some_and(Vec::is_empty))
                }
                CanonicalContentBlock::Thinking { .. } => key == "item_type",
                CanonicalContentBlock::ToolUse { .. } => {
                    // Codex may attach a private Responses-to-Chat transport sidecar.
                    // Only a Chat target may intentionally consume the tool call while
                    // discarding that paired transport metadata.
                    matches!(key, "item_id" | "item_type" | "status")
                        || (target == FormatId::OpenAiChat
                            && (key == "internal_chat_message_metadata_passthrough"
                                || (key == "metadata" && has_chat_metadata_passthrough)))
                }
                CanonicalContentBlock::ToolResult { .. } => key == "item_type",
                CanonicalContentBlock::Image { .. }
                | CanonicalContentBlock::File { .. }
                | CanonicalContentBlock::Audio { .. } => key == "item_type",
                CanonicalContentBlock::Unknown { .. } => false,
            },
        )?;
    }
    Ok(())
}

fn validate_openai_responses_message_item_provenance(
    source: FormatId,
    target: FormatId,
    extensions: &std::collections::BTreeMap<String, Value>,
) -> Result<(), FormatError> {
    for (namespace, value) in extensions {
        if !matches!(namespace.as_str(), "openai_responses" | "openai_cli") {
            continue;
        }
        let Some(message_items) = value
            .as_object()
            .and_then(|object| object.get("message_items"))
        else {
            continue;
        };
        let Some(message_items) = message_items.as_array() else {
            return openai_responses_lossy_response(
                source,
                target,
                "output[].message_items",
                "Responses message item provenance is not an array",
            );
        };
        for item in message_items {
            let Some(item) = item.as_object() else {
                return openai_responses_lossy_response(
                    source,
                    target,
                    "output[].message_items[]",
                    "Responses message item provenance entry is not an object",
                );
            };
            let output_index = item
                .get("output_index")
                .and_then(Value::as_u64)
                .map(|index| index.to_string())
                .unwrap_or_else(|| "?".to_string());
            let Some(fields) = item.get("fields").and_then(Value::as_object) else {
                return openai_responses_lossy_response(
                    source,
                    target,
                    &format!("output[{output_index}]"),
                    "Responses message item provenance fields are not an object",
                );
            };
            if let Some(field) = fields.keys().next() {
                return openai_responses_lossy_response(
                    source,
                    target,
                    &format!("output[{output_index}].{field}"),
                    "target format cannot preserve this Responses message item field",
                );
            }
        }
    }
    Ok(())
}

fn validate_openai_responses_usage_extension_fields(
    source: FormatId,
    target: FormatId,
    extensions: &std::collections::BTreeMap<String, Value>,
) -> Result<(), FormatError> {
    for (namespace, value) in extensions {
        if !matches!(namespace.as_str(), "openai_responses" | "openai_cli") {
            continue;
        }
        let Some(usage) = value.as_object() else {
            continue;
        };
        for (details_key, mapped_fields) in [
            (
                "input_tokens_details",
                &["cached_tokens", "cache_write_tokens"] as &[&str],
            ),
            ("output_tokens_details", &["reasoning_tokens"] as &[&str]),
        ] {
            let Some(details) = usage.get(details_key) else {
                continue;
            };
            let Some(details) = details.as_object() else {
                return openai_responses_lossy_response(
                    source,
                    target,
                    &format!("usage.{details_key}"),
                    "Responses usage details are not an object",
                );
            };
            for (field, value) in details {
                let mapped = mapped_fields.contains(&field.as_str()) && value.as_u64().is_some();
                let cache_write_supported =
                    field != "cache_write_tokens" || target != FormatId::GeminiGenerateContent;
                if mapped && cache_write_supported {
                    continue;
                }
                return openai_responses_lossy_response(
                    source,
                    target,
                    &format!("usage.{details_key}.{field}"),
                    "target format cannot preserve this Responses usage detail field",
                );
            }
        }
    }
    Ok(())
}

fn validate_openai_responses_response_extension_namespace(
    source: FormatId,
    target: FormatId,
    location: &str,
    extensions: &std::collections::BTreeMap<String, Value>,
    is_safe: impl Fn(&str, &Value) -> bool,
) -> Result<(), FormatError> {
    for (namespace, value) in extensions {
        if namespace == "aether" {
            continue;
        }
        let Some(object) = value.as_object() else {
            return openai_responses_lossy_response(
                source,
                target,
                location,
                "Responses extension namespace is not an object",
            );
        };
        for (key, value) in object {
            let internal_openai_field = namespace == "openai" && key == "omit_reasoning_parts";
            if internal_openai_field
                || (matches!(namespace.as_str(), "openai_responses" | "openai_cli")
                    && is_safe(key, value))
            {
                continue;
            }
            return openai_responses_lossy_response(
                source,
                target,
                &extension_field_path(location, namespace, key),
                "target format cannot preserve this Responses provider field",
            );
        }
    }
    Ok(())
}

fn openai_responses_lossy_response(
    source: FormatId,
    target: FormatId,
    field: &str,
    reason: &str,
) -> Result<(), FormatError> {
    Err(FormatError::LossyConversionBlocked {
        source_format: source.as_str().to_string(),
        target_format: target.as_str().to_string(),
        field: field.to_string(),
        reason: reason.to_string(),
    })
}

fn validate_response_content_has_no_unknown_blocks(
    source: FormatId,
    target: FormatId,
    response: &CanonicalResponse,
) -> Result<(), FormatError> {
    for block in response.content.iter().chain(
        response
            .outputs
            .iter()
            .flat_map(|output| output.content.iter()),
    ) {
        if let CanonicalContentBlock::Unknown { raw_type, .. } = block {
            if raw_type == "refusal"
                && matches!(
                    target,
                    FormatId::OpenAiChat
                        | FormatId::OpenAiResponses
                        | FormatId::OpenAiResponsesCompact
                )
            {
                continue;
            }
            return Err(FormatError::LossyConversionBlocked {
                source_format: source.as_str().to_string(),
                target_format: target.as_str().to_string(),
                field: "output[].type".to_string(),
                reason: format!(
                    "target format has no lossless mapping for unknown source output item type {raw_type:?}"
                ),
            });
        }
    }
    Ok(())
}

fn validate_known_standard_request_root_fields(
    source: FormatId,
    target: FormatId,
    body: &Value,
) -> Result<(), FormatError> {
    let Some(object) = body.as_object() else {
        return Ok(());
    };
    for key in object.keys() {
        if standard_request_root_field_is_audited(source, key) {
            continue;
        }
        return Err(FormatError::UnauditedField {
            source_format: source.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: key.clone(),
            reason: "source request root field is not in the audited provider schema for cross-format conversion".to_string(),
        });
    }
    Ok(())
}

fn standard_request_root_field_is_audited(source: FormatId, key: &str) -> bool {
    match source {
        FormatId::OpenAiChat => matches!(
            key,
            "audio"
                | "frequency_penalty"
                | "function_call"
                | "functions"
                | "logit_bias"
                | "logprobs"
                | "max_completion_tokens"
                | "max_tokens"
                | "messages"
                | "metadata"
                | "modalities"
                | "model"
                | "n"
                | "parallel_tool_calls"
                | "prediction"
                | "presence_penalty"
                | "prompt_cache_key"
                | "prompt_cache_options"
                | "prompt_cache_retention"
                | "reasoning_effort"
                | "response_format"
                | "safety_identifier"
                | "seed"
                | "service_tier"
                | "stop"
                | "store"
                | "stream"
                | "stream_options"
                | "temperature"
                | "tool_choice"
                | "tools"
                | "top_logprobs"
                | "top_p"
                | "user"
                | "verbosity"
                | "web_search_options"
        ),
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => matches!(
            key,
            "background"
                | "context_management"
                | "conversation"
                | "include"
                | "input"
                | "instructions"
                | "max_output_tokens"
                | "max_tool_calls"
                | "metadata"
                | "model"
                | "multi_agent"
                | "parallel_tool_calls"
                | "previous_response_id"
                | "prompt"
                | "prompt_cache_key"
                | "prompt_cache_options"
                | "prompt_cache_retention"
                | "reasoning"
                | "safety_identifier"
                | "service_tier"
                | "store"
                | "stream"
                | "stream_options"
                | "temperature"
                | "text"
                | "tool_choice"
                | "tools"
                | "top_logprobs"
                | "top_p"
                | "truncation"
                | "user"
        ),
        FormatId::ClaudeMessages => matches!(
            key,
            "cache_control"
                | "container"
                | "context_management"
                | "inference_geo"
                | "max_tokens"
                | "messages"
                | "metadata"
                | "model"
                | "output_config"
                | "service_tier"
                | "stop_sequences"
                | "stream"
                | "system"
                | "temperature"
                | "thinking"
                | "tool_choice"
                | "tools"
                | "top_k"
                | "top_p"
        ),
        FormatId::GeminiGenerateContent => matches!(
            key,
            "cachedContent"
                | "cached_content"
                | "contents"
                | "generationConfig"
                | "generation_config"
                | "model"
                | "safetySettings"
                | "safety_settings"
                | "serviceTier"
                | "service_tier"
                | "store"
                | "systemInstruction"
                | "system_instruction"
                | "toolConfig"
                | "tool_config"
                | "tools"
        ),
        FormatId::GeminiInteractions => matches!(
            key,
            "agent"
                | "agent_config"
                | "background"
                | "generation_config"
                | "input"
                | "metadata"
                | "model"
                | "previous_interaction_id"
                | "response_format"
                | "safety_settings"
                | "store"
                | "stream"
                | "system_instruction"
                | "tool_config"
                | "tools"
        ),
        FormatId::OpenAiEmbedding
        | FormatId::OpenAiSearch
        | FormatId::OpenAiRerank
        | FormatId::GeminiEmbedding
        | FormatId::JinaEmbedding
        | FormatId::JinaRerank
        | FormatId::DoubaoEmbedding
        | FormatId::AliyunMultimodalEmbedding => true,
    }
}

fn validate_cross_format_generation_target(
    source: FormatId,
    target: FormatId,
    request: &CanonicalRequest,
) -> Result<(), FormatError> {
    let generation = &request.generation;
    match target {
        FormatId::OpenAiChat if generation.top_k.is_some() => {
            return lossy_generation_field(
                source,
                target,
                "top_k",
                "OpenAI Chat Completions has no official top_k request field",
            );
        }
        FormatId::OpenAiChat => {}
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
            for (field, present, reason) in [
                (
                    "top_k",
                    generation.top_k.is_some(),
                    "OpenAI Responses has no top_k request field",
                ),
                (
                    "stop_sequences",
                    generation.stop_sequences.is_some(),
                    "OpenAI Responses has no stop sequence request field",
                ),
                (
                    "n",
                    generation.n.is_some(),
                    "OpenAI Responses has no multi-candidate n request field",
                ),
                (
                    "presence_penalty",
                    generation.presence_penalty.is_some(),
                    "OpenAI Responses has no presence_penalty request field",
                ),
                (
                    "frequency_penalty",
                    generation.frequency_penalty.is_some(),
                    "OpenAI Responses has no frequency_penalty request field",
                ),
                (
                    "seed",
                    generation.seed.is_some(),
                    "OpenAI Responses has no seed request field",
                ),
                (
                    "logprobs",
                    generation.logprobs.is_some(),
                    "OpenAI Responses has no logprobs boolean request field",
                ),
            ] {
                if present {
                    return lossy_generation_field(source, target, field, reason);
                }
            }
        }
        FormatId::ClaudeMessages => {
            for (field, present, reason) in [
                (
                    "n",
                    generation.n.is_some(),
                    "Claude Messages has no multi-candidate n request field",
                ),
                (
                    "presence_penalty",
                    generation.presence_penalty.is_some(),
                    "Claude Messages has no presence_penalty request field",
                ),
                (
                    "frequency_penalty",
                    generation.frequency_penalty.is_some(),
                    "Claude Messages has no frequency_penalty request field",
                ),
                (
                    "seed",
                    generation.seed.is_some(),
                    "Claude Messages has no seed request field",
                ),
                (
                    "logprobs",
                    generation.logprobs.is_some(),
                    "Claude Messages has no logprobs request field",
                ),
                (
                    "top_logprobs",
                    generation.top_logprobs.is_some(),
                    "Claude Messages has no top_logprobs request field",
                ),
            ] {
                if present {
                    return lossy_generation_field(source, target, field, reason);
                }
            }
        }
        FormatId::GeminiGenerateContent => {
            for (field, present, reason) in [
                (
                    "presence_penalty",
                    generation.presence_penalty.is_some(),
                    "Gemini GenerateContent has no presence_penalty request field",
                ),
                (
                    "frequency_penalty",
                    generation.frequency_penalty.is_some(),
                    "Gemini GenerateContent has no frequency_penalty request field",
                ),
                (
                    "logprobs",
                    generation.logprobs.is_some(),
                    "Gemini GenerateContent has no logprobs request field",
                ),
                (
                    "top_logprobs",
                    generation.top_logprobs.is_some(),
                    "Gemini GenerateContent has no top_logprobs request field",
                ),
            ] {
                if present {
                    return lossy_generation_field(source, target, field, reason);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn lossy_generation_field(
    source: FormatId,
    target: FormatId,
    canonical_field: &str,
    reason: &str,
) -> Result<(), FormatError> {
    Err(FormatError::LossyConversionBlocked {
        source_format: source.as_str().to_string(),
        target_format: target.as_str().to_string(),
        field: source_generation_field_path(source, canonical_field),
        reason: reason.to_string(),
    })
}

fn source_generation_field_path(source: FormatId, canonical_field: &str) -> String {
    let field = match (source, canonical_field) {
        (FormatId::OpenAiChat, "max_tokens") => "max_tokens/max_completion_tokens",
        (FormatId::OpenAiChat, "stop_sequences") => "stop",
        (FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact, "max_tokens") => {
            "max_output_tokens"
        }
        (FormatId::ClaudeMessages, "stop_sequences") => "stop_sequences",
        (FormatId::GeminiGenerateContent, "max_tokens") => "generationConfig.maxOutputTokens",
        (FormatId::GeminiGenerateContent, "top_p") => "generationConfig.topP",
        (FormatId::GeminiGenerateContent, "top_k") => "generationConfig.topK",
        (FormatId::GeminiGenerateContent, "stop_sequences") => "generationConfig.stopSequences",
        (FormatId::GeminiGenerateContent, "n") => "generationConfig.candidateCount",
        (FormatId::GeminiGenerateContent, "seed") => "generationConfig.seed",
        (FormatId::GeminiGenerateContent, other) => return format!("generationConfig.{other}"),
        (_, other) => other,
    };
    field.to_string()
}

fn validate_cross_format_request_extensions(
    source: FormatId,
    target: FormatId,
    request: &CanonicalRequest,
) -> Result<(), FormatError> {
    validate_request_content_has_no_unknown_blocks(source, target, request)?;
    validate_request_extension_namespace(source, target, "request", &request.extensions)?;
    for instruction in &request.instructions {
        validate_request_extension_namespace(
            source,
            target,
            "instructions[]",
            &instruction.extensions,
        )?;
    }
    for message in &request.messages {
        validate_request_extension_namespace(source, target, "messages[]", &message.extensions)?;
        for block in &message.content {
            match block {
                CanonicalContentBlock::Text { extensions, .. }
                | CanonicalContentBlock::Thinking { extensions, .. }
                | CanonicalContentBlock::Image { extensions, .. }
                | CanonicalContentBlock::File { extensions, .. }
                | CanonicalContentBlock::Audio { extensions, .. }
                | CanonicalContentBlock::ToolUse { extensions, .. }
                | CanonicalContentBlock::ToolResult { extensions, .. }
                | CanonicalContentBlock::Unknown { extensions, .. } => {
                    validate_request_extension_namespace(
                        source,
                        target,
                        "messages[].content[]",
                        extensions,
                    )?;
                }
            }
        }
    }
    for tool in &request.tools {
        validate_request_extension_namespace(source, target, "tools[]", &tool.extensions)?;
    }
    if let Some(thinking) = &request.thinking {
        validate_request_extension_namespace(source, target, "thinking", &thinking.extensions)?;
    }
    if let Some(response_format) = &request.response_format {
        validate_request_extension_namespace(
            source,
            target,
            "response_format",
            &response_format.extensions,
        )?;
    }
    Ok(())
}

fn validate_request_content_has_no_unknown_blocks(
    source: FormatId,
    target: FormatId,
    request: &CanonicalRequest,
) -> Result<(), FormatError> {
    for message in &request.messages {
        for block in &message.content {
            if let CanonicalContentBlock::Unknown { raw_type, .. } = block {
                return Err(FormatError::LossyConversionBlocked {
                    source_format: source.as_str().to_string(),
                    target_format: target.as_str().to_string(),
                    field: "messages[].content[].type".to_string(),
                    reason: format!(
                        "target format has no lossless mapping for unknown source content block type {raw_type:?}"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn validate_request_extension_namespace(
    source: FormatId,
    target: FormatId,
    location: &str,
    extensions: &std::collections::BTreeMap<String, Value>,
) -> Result<(), FormatError> {
    for (namespace, value) in extensions {
        if namespace == "aether" {
            continue;
        }
        let Some(object) = value.as_object() else {
            return Err(FormatError::UnsupportedField {
                format: source.as_str().to_string(),
                field: format!("{location}.{namespace}"),
                reason:
                    "provider extension namespace must be an object for cross-format conversion"
                        .to_string(),
            });
        };
        for key in object.keys() {
            if request_extension_key_is_cross_format_safe(source, target, location, namespace, key)
            {
                continue;
            }
            return Err(FormatError::LossyConversionBlocked {
                source_format: source.as_str().to_string(),
                target_format: target.as_str().to_string(),
                field: extension_field_path(location, namespace, key),
                reason: "provider-specific extension field has no audited lossless target mapping"
                    .to_string(),
            });
        }
    }
    Ok(())
}

fn request_extension_key_is_cross_format_safe(
    source: FormatId,
    target: FormatId,
    location: &str,
    namespace: &str,
    key: &str,
) -> bool {
    if location == "messages[].content[]"
        && key == "prompt_cache_breakpoint"
        && matches!(
            (source, target, namespace),
            (
                FormatId::OpenAiChat,
                FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
                "openai",
            ) | (
                FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
                FormatId::OpenAiChat,
                "openai_responses" | "openai_cli",
            )
        )
    {
        return true;
    }
    if location == "tools[]" {
        return tool_extension_key_is_cross_format_safe(source, target, namespace, key);
    }
    if location == "thinking" {
        return thinking_extension_key_is_cross_format_safe(source, target, namespace, key);
    }
    if location == "response_format" {
        return response_format_extension_key_is_cross_format_safe(namespace, key);
    }
    matches!(
        (source, target, namespace, key),
        (
            FormatId::OpenAiChat,
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
            "openai",
            "stream"
                | "store"
                | "service_tier"
                | "safety_identifier"
                | "prompt_cache_key"
                | "prompt_cache_options"
                | "prompt_cache_retention"
                | "user"
                | "verbosity",
        ) | (
            FormatId::OpenAiChat,
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
            "openai_responses",
            "verbosity",
        ) | (
            FormatId::OpenAiChat,
            FormatId::GeminiGenerateContent,
            "openai",
            "web_search_options",
        ) | (
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
            FormatId::OpenAiChat,
            "openai_responses" | "openai_cli",
            "stream"
                | "store"
                | "service_tier"
                | "safety_identifier"
                | "prompt_cache_key"
                | "prompt_cache_options"
                | "prompt_cache_retention"
                | "user"
                | "verbosity",
        ) | (
            FormatId::ClaudeMessages,
            _,
            "claude",
            "context_management" | "output_config",
        ) | (
            FormatId::ClaudeMessages,
            FormatId::OpenAiChat | FormatId::GeminiGenerateContent,
            "openai",
            "web_search_options",
        ) | (
            FormatId::GeminiGenerateContent,
            _,
            "gemini",
            "thinking_config" | "raw_tools" | "raw_tool_config",
        ) | (
            FormatId::GeminiGenerateContent,
            FormatId::OpenAiChat,
            "gemini",
            "builtin_tools" | "grounding",
        ) | (
            FormatId::GeminiGenerateContent,
            FormatId::GeminiGenerateContent,
            "gemini",
            _
        ) | (
            FormatId::GeminiGenerateContent,
            FormatId::OpenAiChat,
            "openai",
            "web_search_options",
        )
    )
}

fn tool_extension_key_is_cross_format_safe(
    source: FormatId,
    target: FormatId,
    namespace: &str,
    key: &str,
) -> bool {
    matches!(
        (source, target, namespace, key),
        (_, _, "claude", "raw_input_schema")
            | (_, _, "gemini", "raw_parameters")
            | (
                FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
                FormatId::GeminiGenerateContent,
                "openai_responses" | "openai_cli",
                "type",
            )
    )
}

fn thinking_extension_key_is_cross_format_safe(
    source: FormatId,
    target: FormatId,
    namespace: &str,
    key: &str,
) -> bool {
    matches!(
        (source, target, namespace, key),
        (
            FormatId::OpenAiChat,
            FormatId::OpenAiResponses
                | FormatId::OpenAiResponsesCompact
                | FormatId::ClaudeMessages
                | FormatId::GeminiGenerateContent,
            "openai",
            "reasoning_effort",
        ) | (
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
            FormatId::OpenAiChat | FormatId::ClaudeMessages | FormatId::GeminiGenerateContent,
            "openai_responses" | "openai_cli",
            "effort",
        ) | (
            FormatId::ClaudeMessages,
            _,
            "claude",
            "type" | "budget_tokens" | "output_config",
        ) | (
            FormatId::ClaudeMessages,
            FormatId::OpenAiChat | FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
            "openai",
            "reasoning_effort",
        ) | (
            FormatId::GeminiGenerateContent,
            _,
            "gemini",
            "thinking_config" | "includeThoughts" | "thinkingBudget" | "thinkingLevel",
        ) | (
            FormatId::GeminiGenerateContent,
            FormatId::OpenAiChat | FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
            "openai",
            "reasoning_effort",
        )
    )
}

fn response_format_extension_key_is_cross_format_safe(namespace: &str, key: &str) -> bool {
    matches!((namespace, key), ("openai", _) | ("gemini", "raw_schema"))
}

fn extension_field_path(location: &str, namespace: &str, key: &str) -> String {
    if location == "request" {
        format!("{namespace}.{key}")
    } else {
        format!("{location}.{namespace}.{key}")
    }
}

fn validate_source_response_stop_enums(
    source: FormatId,
    target: FormatId,
    body: &Value,
) -> Result<(), FormatError> {
    match source {
        FormatId::OpenAiChat => validate_openai_chat_response_finish_reasons(body),
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
            validate_openai_responses_response_status(body, target)
        }
        FormatId::ClaudeMessages => validate_claude_response_stop_reason(body),
        FormatId::GeminiGenerateContent => validate_gemini_response_finish_reasons(body, target),
        FormatId::GeminiInteractions => Ok(()),
        FormatId::OpenAiEmbedding
        | FormatId::OpenAiSearch
        | FormatId::OpenAiRerank
        | FormatId::GeminiEmbedding
        | FormatId::JinaEmbedding
        | FormatId::JinaRerank
        | FormatId::DoubaoEmbedding
        | FormatId::AliyunMultimodalEmbedding => Ok(()),
    }
}

fn validate_openai_chat_response_finish_reasons(body: &Value) -> Result<(), FormatError> {
    let Some(choices) = body.get("choices").and_then(Value::as_array) else {
        return Ok(());
    };
    for choice in choices {
        let Some(value) = choice.get("finish_reason") else {
            continue;
        };
        validate_nullable_string_field(
            value,
            FormatId::OpenAiChat.as_str(),
            "choices[].finish_reason",
        )?;
        let Some(raw) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if !matches!(
            raw,
            "stop" | "length" | "tool_calls" | "function_call" | "content_filter"
        ) {
            return Err(FormatError::InvalidEnumValue {
                format: FormatId::OpenAiChat.as_str().to_string(),
                field: "choices[].finish_reason".to_string(),
                value: raw.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_openai_responses_response_status(
    body: &Value,
    target: FormatId,
) -> Result<(), FormatError> {
    let Some(value) = body.get("status") else {
        return Ok(());
    };
    validate_nullable_string_field(value, FormatId::OpenAiResponses.as_str(), "status")?;
    let Some(raw) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    match raw {
        "completed" | "incomplete" => Ok(()),
        "queued" | "in_progress" | "cancelled" => Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::OpenAiResponses.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "status".to_string(),
            reason: "target sync response format cannot represent non-terminal Responses status"
                .to_string(),
        }),
        "failed" => Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::OpenAiResponses.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "status".to_string(),
            reason:
                "failed Responses objects must be handled as provider errors, not success responses"
                    .to_string(),
        }),
        _ => Err(FormatError::InvalidEnumValue {
            format: FormatId::OpenAiResponses.as_str().to_string(),
            field: "status".to_string(),
            value: raw.to_string(),
        }),
    }
}

fn validate_claude_response_stop_reason(body: &Value) -> Result<(), FormatError> {
    let Some(value) = body.get("stop_reason") else {
        return Ok(());
    };
    validate_nullable_string_field(value, FormatId::ClaudeMessages.as_str(), "stop_reason")?;
    let Some(raw) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if !matches!(
        raw,
        "end_turn"
            | "max_tokens"
            | "stop_sequence"
            | "tool_use"
            | "pause_turn"
            | "refusal"
            | "content_filtered"
    ) {
        return Err(FormatError::InvalidEnumValue {
            format: FormatId::ClaudeMessages.as_str().to_string(),
            field: "stop_reason".to_string(),
            value: raw.to_string(),
        });
    }
    Ok(())
}

fn validate_gemini_response_finish_reasons(
    body: &Value,
    target: FormatId,
) -> Result<(), FormatError> {
    let Some(candidates) = body.get("candidates").and_then(Value::as_array) else {
        return Ok(());
    };
    for candidate in candidates {
        let Some(value) = candidate
            .get("finishReason")
            .or_else(|| candidate.get("finish_reason"))
        else {
            continue;
        };
        validate_nullable_string_field(
            value,
            FormatId::GeminiGenerateContent.as_str(),
            "candidates[].finishReason",
        )?;
        let Some(raw) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let normalized = raw.to_ascii_uppercase();
        if gemini_finish_reason_is_cross_format_mappable(normalized.as_str()) {
            continue;
        }
        if gemini_finish_reason_is_known(normalized.as_str()) {
            return Err(FormatError::LossyConversionBlocked {
                source_format: FormatId::GeminiGenerateContent.as_str().to_string(),
                target_format: target.as_str().to_string(),
                field: "candidates[].finishReason".to_string(),
                reason: format!("Gemini finishReason {raw:?} has no lossless target finish reason"),
            });
        }
        return Err(FormatError::InvalidEnumValue {
            format: FormatId::GeminiGenerateContent.as_str().to_string(),
            field: "candidates[].finishReason".to_string(),
            value: raw.to_string(),
        });
    }
    Ok(())
}

fn validate_nullable_string_field(
    value: &Value,
    format: &str,
    field: &str,
) -> Result<(), FormatError> {
    if value.is_null() || value.is_string() {
        Ok(())
    } else {
        Err(FormatError::InvalidTargetField {
            format: format.to_string(),
            field: field.to_string(),
            reason: "enum field must be a string or null".to_string(),
        })
    }
}

fn gemini_finish_reason_is_cross_format_mappable(value: &str) -> bool {
    matches!(
        value,
        "STOP"
            | "MAX_TOKENS"
            | "SAFETY"
            | "RECITATION"
            | "LANGUAGE"
            | "BLOCKLIST"
            | "PROHIBITED_CONTENT"
            | "SPII"
            | "IMAGE_SAFETY"
            | "IMAGE_PROHIBITED_CONTENT"
            | "IMAGE_RECITATION"
    )
}

fn gemini_finish_reason_is_known(value: &str) -> bool {
    gemini_finish_reason_is_cross_format_mappable(value)
        || matches!(
            value,
            "FINISH_REASON_UNSPECIFIED"
                | "OTHER"
                | "MALFORMED_FUNCTION_CALL"
                | "IMAGE_OTHER"
                | "NO_IMAGE"
                | "UNEXPECTED_TOOL_CALL"
                | "TOO_MANY_TOOL_CALLS"
                | "MISSING_THOUGHT_SIGNATURE"
                | "MALFORMED_RESPONSE"
                | "ESCALATION"
        )
}

fn validate_canonical_response_stop_reasons(
    source: FormatId,
    target: FormatId,
    response: &CanonicalResponse,
) -> Result<(), FormatError> {
    if response.stop_reason == Some(CanonicalStopReason::Unknown)
        || response
            .outputs
            .iter()
            .any(|output| output.stop_reason == Some(CanonicalStopReason::Unknown))
    {
        return Err(FormatError::LossyConversionBlocked {
            source_format: source.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "stop_reason".to_string(),
            reason: "source response stop reason cannot be represented losslessly in target format"
                .to_string(),
        });
    }
    Ok(())
}

fn is_embedding_format(format: FormatId) -> bool {
    matches!(
        format,
        FormatId::OpenAiEmbedding
            | FormatId::GeminiEmbedding
            | FormatId::JinaEmbedding
            | FormatId::DoubaoEmbedding
            | FormatId::AliyunMultimodalEmbedding
    )
}

fn is_rerank_format(format: FormatId) -> bool {
    matches!(format, FormatId::OpenAiRerank | FormatId::JinaRerank)
}

fn validate_embedding_request_conversion(
    source: FormatId,
    target: FormatId,
    request: &CanonicalRequest,
) -> Result<(), FormatError> {
    if !is_embedding_format(source) || !is_embedding_format(target) {
        return Err(FormatError::UnsupportedField {
            format: target.as_str().to_string(),
            field: "embedding".to_string(),
            reason: "embedding requests can only convert to embedding target formats".to_string(),
        });
    }
    let Some(embedding) = request.embedding.as_ref() else {
        return Err(FormatError::RequestParseFailed {
            format: source.as_str().to_string(),
        });
    };
    if embedding.input.is_empty() {
        return Err(FormatError::InvalidTargetField {
            format: source.as_str().to_string(),
            field: "input".to_string(),
            reason: "embedding input must not be empty".to_string(),
        });
    }
    validate_no_cross_format_embedding_extensions(source, target, embedding)?;
    match target {
        FormatId::OpenAiEmbedding => validate_openai_embedding_target(source, target, embedding),
        FormatId::JinaEmbedding => validate_text_embedding_target(source, target, embedding, true),
        FormatId::GeminiEmbedding => validate_gemini_embedding_target(source, target, embedding),
        FormatId::DoubaoEmbedding => validate_doubao_embedding_target(source, target, embedding),
        FormatId::AliyunMultimodalEmbedding => {
            validate_aliyun_embedding_target(source, target, embedding)
        }
        _ => Ok(()),
    }
}

fn validate_no_cross_format_embedding_extensions(
    source: FormatId,
    target: FormatId,
    embedding: &crate::protocol::canonical::CanonicalEmbeddingRequest,
) -> Result<(), FormatError> {
    if embedding.extensions.is_empty() {
        return Ok(());
    }
    let target_namespace = embedding_namespace(target);
    if embedding
        .extensions
        .keys()
        .all(|key| key == target_namespace)
    {
        return Ok(());
    }
    Err(FormatError::UnsupportedField {
        format: target.as_str().to_string(),
        field: "embedding.extensions".to_string(),
        reason: format!(
            "{} provider-specific embedding fields cannot be losslessly emitted as {}",
            source.as_str(),
            target.as_str()
        ),
    })
}

fn embedding_namespace(format: FormatId) -> &'static str {
    match format {
        FormatId::OpenAiEmbedding => "openai",
        FormatId::GeminiEmbedding => "gemini",
        FormatId::JinaEmbedding => "jina",
        FormatId::DoubaoEmbedding => "doubao",
        FormatId::AliyunMultimodalEmbedding => "aliyun",
        _ => "",
    }
}

fn validate_openai_embedding_target(
    source: FormatId,
    target: FormatId,
    embedding: &crate::protocol::canonical::CanonicalEmbeddingRequest,
) -> Result<(), FormatError> {
    if matches!(embedding.input, CanonicalEmbeddingInput::Multimodal(_)) {
        return lossy_embedding_field(
            source,
            target,
            "input",
            "OpenAI embeddings do not support multimodal embedding input",
        );
    }
    if embedding.task.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "task",
            "OpenAI embeddings have no task field",
        );
    }
    if embedding.parameters.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "parameters",
            "OpenAI embeddings have no generic parameters field",
        );
    }
    Ok(())
}

fn validate_text_embedding_target(
    source: FormatId,
    target: FormatId,
    embedding: &crate::protocol::canonical::CanonicalEmbeddingRequest,
    allow_task: bool,
) -> Result<(), FormatError> {
    if !matches!(
        embedding.input,
        CanonicalEmbeddingInput::String(_) | CanonicalEmbeddingInput::StringArray(_)
    ) {
        return lossy_embedding_field(
            source,
            target,
            "input",
            "target embedding format only supports text string inputs",
        );
    }
    if embedding.encoding_format.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "encoding_format",
            "target embedding format has no encoding_format field",
        );
    }
    if embedding.user.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "user",
            "target embedding format has no user field",
        );
    }
    if !allow_task && embedding.task.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "task",
            "target embedding format has no task field",
        );
    }
    Ok(())
}

fn validate_gemini_embedding_target(
    source: FormatId,
    target: FormatId,
    embedding: &crate::protocol::canonical::CanonicalEmbeddingRequest,
) -> Result<(), FormatError> {
    validate_text_embedding_target(source, target, embedding, true)?;
    if embedding.parameters.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "parameters",
            "Gemini embeddings require named embedContent config fields, not generic parameters",
        );
    }
    if let Some(task) = embedding.task.as_deref() {
        validate_gemini_embedding_task_type(task)?;
    }
    Ok(())
}

fn validate_gemini_embedding_task_type(task: &str) -> Result<(), FormatError> {
    let normalized = task.trim().replace(['-', ' '], "_").to_ascii_uppercase();
    if matches!(
        normalized.as_str(),
        "QUERY"
            | "DOCUMENT"
            | "TASK_TYPE_UNSPECIFIED"
            | "RETRIEVAL_QUERY"
            | "RETRIEVAL_DOCUMENT"
            | "TEXT_MATCHING"
            | "SEMANTIC_SIMILARITY"
            | "CLASSIFICATION"
            | "CLUSTERING"
            | "QUESTION_ANSWERING"
            | "FACT_VERIFICATION"
            | "CODE_RETRIEVAL_QUERY"
    ) {
        Ok(())
    } else {
        Err(FormatError::InvalidEnumValue {
            format: FormatId::GeminiEmbedding.as_str().to_string(),
            field: "taskType".to_string(),
            value: task.to_string(),
        })
    }
}

fn validate_doubao_embedding_target(
    source: FormatId,
    target: FormatId,
    embedding: &crate::protocol::canonical::CanonicalEmbeddingRequest,
) -> Result<(), FormatError> {
    validate_text_embedding_target(source, target, embedding, false)?;
    if embedding.parameters.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "parameters",
            "Doubao embeddings have no generic parameters field",
        );
    }
    Ok(())
}

fn validate_aliyun_embedding_target(
    source: FormatId,
    target: FormatId,
    embedding: &crate::protocol::canonical::CanonicalEmbeddingRequest,
) -> Result<(), FormatError> {
    if matches!(
        embedding.input,
        CanonicalEmbeddingInput::TokenArray(_) | CanonicalEmbeddingInput::TokenArrayArray(_)
    ) {
        return lossy_embedding_field(
            source,
            target,
            "input",
            "Aliyun multimodal embeddings do not support token-array input",
        );
    }
    if embedding.encoding_format.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "encoding_format",
            "Aliyun multimodal embeddings have no encoding_format field",
        );
    }
    if embedding.user.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "user",
            "Aliyun multimodal embeddings have no user field",
        );
    }
    if embedding.task.is_some() {
        return lossy_embedding_field(
            source,
            target,
            "task",
            "Aliyun multimodal embeddings have no task field",
        );
    }
    Ok(())
}

fn lossy_embedding_field(
    source: FormatId,
    target: FormatId,
    field: &str,
    reason: &str,
) -> Result<(), FormatError> {
    Err(FormatError::LossyConversionBlocked {
        source_format: source.as_str().to_string(),
        target_format: target.as_str().to_string(),
        field: field.to_string(),
        reason: reason.to_string(),
    })
}

fn validate_rerank_request_conversion(
    source: FormatId,
    target: FormatId,
    request: &CanonicalRequest,
) -> Result<(), FormatError> {
    if !is_rerank_format(source) || !is_rerank_format(target) {
        return Err(FormatError::UnsupportedField {
            format: target.as_str().to_string(),
            field: "rerank".to_string(),
            reason: "rerank requests can only convert to rerank target formats".to_string(),
        });
    }
    let Some(rerank) = request.rerank.as_ref() else {
        return Err(FormatError::RequestParseFailed {
            format: source.as_str().to_string(),
        });
    };
    if rerank.is_empty() {
        return Err(FormatError::InvalidTargetField {
            format: source.as_str().to_string(),
            field: "query/documents".to_string(),
            reason: "rerank query and documents must not be empty".to_string(),
        });
    }
    if rerank.top_n == Some(0) {
        return Err(FormatError::InvalidTargetField {
            format: source.as_str().to_string(),
            field: "top_n".to_string(),
            reason: "rerank top_n must be greater than zero".to_string(),
        });
    }
    let target_namespace = rerank_namespace(target);
    if !rerank.extensions.is_empty() && !rerank.extensions.keys().all(|key| key == target_namespace)
    {
        return Err(FormatError::UnsupportedField {
            format: target.as_str().to_string(),
            field: "rerank.extensions".to_string(),
            reason: format!(
                "{} provider-specific rerank fields cannot be losslessly emitted as {}",
                source.as_str(),
                target.as_str()
            ),
        });
    }
    Ok(())
}

fn rerank_namespace(format: FormatId) -> &'static str {
    match format {
        FormatId::OpenAiRerank => "openai",
        FormatId::JinaRerank => "jina",
        _ => "",
    }
}

fn validate_openai_reasoning_effort(
    source: FormatId,
    target: FormatId,
    body: &Value,
    mapped_model: Option<&str>,
) -> Result<(), FormatError> {
    let same_openai_format = source == target
        && matches!(
            source,
            FormatId::OpenAiChat | FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
        );
    let openai_cross_format = matches!(
        (source, target),
        (
            FormatId::OpenAiChat,
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
        ) | (
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
            FormatId::OpenAiChat
        )
    );
    if !same_openai_format && !openai_cross_format {
        return Ok(());
    }
    crate::validate_openai_reasoning_request(
        source.as_str(),
        target.as_str(),
        mapped_model.unwrap_or_default(),
        body,
    )
    .map_err(|violation| match violation.kind {
        crate::OpenAiReasoningViolationKind::InvalidEnum => FormatError::InvalidEnumValue {
            format: source.as_str().to_string(),
            field: violation.field,
            value: violation.value.unwrap_or_default(),
        },
        crate::OpenAiReasoningViolationKind::InvalidType
        | crate::OpenAiReasoningViolationKind::UnsupportedForModel => {
            FormatError::InvalidTargetField {
                format: source.as_str().to_string(),
                field: violation.field,
                reason: violation.reason,
            }
        }
    })
}

fn validate_openai_prompt_cache_contract(
    source: FormatId,
    body: &Value,
    mapped_model: Option<&str>,
) -> Result<(), FormatError> {
    if !matches!(
        source,
        FormatId::OpenAiChat | FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
    ) {
        return Ok(());
    }
    let provider_model = mapped_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| body.get("model").and_then(Value::as_str))
        .unwrap_or_default();
    crate::validate_openai_prompt_cache_request(source.as_str(), provider_model, body).map_err(
        |violation| match violation.kind {
            crate::OpenAiPromptCacheViolationKind::InvalidEnum => FormatError::InvalidEnumValue {
                format: source.as_str().to_string(),
                field: violation.field,
                value: violation.value.unwrap_or_default(),
            },
            crate::OpenAiPromptCacheViolationKind::InvalidType
            | crate::OpenAiPromptCacheViolationKind::UnsupportedForModel
            | crate::OpenAiPromptCacheViolationKind::UnsupportedContentBlock => {
                FormatError::InvalidTargetField {
                    format: source.as_str().to_string(),
                    field: violation.field,
                    reason: violation.reason,
                }
            }
        },
    )
}

fn validate_openai_cross_format_store(
    source: FormatId,
    target: FormatId,
    body: &Value,
) -> Result<(), FormatError> {
    if !matches!(
        (source, target),
        (
            FormatId::OpenAiChat,
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
        ) | (
            FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact,
            FormatId::OpenAiChat
        )
    ) {
        return Ok(());
    }
    let Some(value) = body.get("store") else {
        return Ok(());
    };
    if target == FormatId::OpenAiResponsesCompact {
        return if value.is_boolean() {
            Ok(())
        } else {
            Err(FormatError::InvalidTargetField {
                format: source.as_str().to_string(),
                field: "store".to_string(),
                reason: "store must be a boolean".to_string(),
            })
        };
    }
    match value.as_bool() {
        Some(false) => Ok(()),
        Some(true) => Err(FormatError::LossyConversionBlocked {
            source_format: source.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "store".to_string(),
            reason: "Chat and Responses assign different persistence behavior to store=true"
                .to_string(),
        }),
        None => Err(FormatError::InvalidTargetField {
            format: source.as_str().to_string(),
            field: "store".to_string(),
            reason: "store must be a boolean".to_string(),
        }),
    }
}

fn validate_openai_chat_to_responses(body: &Value) -> Result<(), FormatError> {
    let Some(object) = body.as_object() else {
        return Ok(());
    };
    for field in [
        "n",
        "stop",
        "presence_penalty",
        "frequency_penalty",
        "seed",
        "logprobs",
        "stream_options",
    ] {
        if object.contains_key(field) {
            return Err(FormatError::LossyConversionBlocked {
                source_format: FormatId::OpenAiChat.as_str().to_string(),
                target_format: FormatId::OpenAiResponses.as_str().to_string(),
                field: field.to_string(),
                reason: "OpenAI Responses request has no canonical equivalent for this Chat field"
                    .to_string(),
            });
        }
    }
    if openai_chat_request_has_unsupported_responses_cache_breakpoint(object) {
        return Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::OpenAiChat.as_str().to_string(),
            target_format: FormatId::OpenAiResponses.as_str().to_string(),
            field: "messages[].content[].prompt_cache_breakpoint".to_string(),
            reason: "OpenAI Responses supports prompt cache breakpoints only on input_text, input_image, and input_file blocks".to_string(),
        });
    }
    Ok(())
}

fn openai_chat_request_has_unsupported_responses_cache_breakpoint(
    request: &Map<String, Value>,
) -> bool {
    request
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .any(|message| {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .map(str::trim)
                .map(str::to_ascii_lowercase);
            message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .any(|part| {
                    part.contains_key("prompt_cache_breakpoint")
                        && !openai_chat_part_maps_to_cacheable_responses_input(
                            role.as_deref(),
                            part,
                        )
                })
        })
}

fn openai_chat_part_maps_to_cacheable_responses_input(
    role: Option<&str>,
    part: &Map<String, Value>,
) -> bool {
    if !matches!(role, Some("user" | "system" | "developer")) {
        return false;
    }
    matches!(
        part.get("type")
            .and_then(Value::as_str)
            .unwrap_or("text")
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "text" | "input_text" | "image_url" | "input_image" | "file" | "input_file"
    )
}

fn validate_openai_responses_to_chat(
    body: &Value,
    request: &CanonicalRequest,
) -> Result<(), FormatError> {
    let Some(object) = body.as_object() else {
        return Ok(());
    };
    for field in [
        "include",
        "previous_response_id",
        "truncation",
        "prompt",
        "conversation",
        "background",
        "max_tool_calls",
        "multi_agent",
    ] {
        if object.contains_key(field) {
            return Err(FormatError::LossyConversionBlocked {
                source_format: FormatId::OpenAiResponses.as_str().to_string(),
                target_format: FormatId::OpenAiChat.as_str().to_string(),
                field: field.to_string(),
                reason: "OpenAI Chat request has no canonical equivalent for this Responses field"
                    .to_string(),
            });
        }
    }
    if let Some(reasoning) = object.get("reasoning").and_then(Value::as_object) {
        for field in ["summary", "budget_tokens"] {
            if reasoning.contains_key(field) {
                return Err(FormatError::LossyConversionBlocked {
                    source_format: FormatId::OpenAiResponses.as_str().to_string(),
                    target_format: FormatId::OpenAiChat.as_str().to_string(),
                    field: format!("reasoning.{field}"),
                    reason:
                        "OpenAI Chat reasoning_effort cannot carry this Responses reasoning field"
                            .to_string(),
                });
            }
        }
    }
    if let Some(tools) = object.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let tool_type = tool
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("function")
                .trim()
                .to_ascii_lowercase();
            if tool_type != "function" {
                return Err(FormatError::LossyConversionBlocked {
                    source_format: FormatId::OpenAiResponses.as_str().to_string(),
                    target_format: FormatId::OpenAiChat.as_str().to_string(),
                    field: "tools".to_string(),
                    reason: format!("OpenAI Chat only supports function tools, got {tool_type}"),
                });
            }
        }
    }
    if request.tools.iter().any(|tool| tool.name.trim().is_empty()) {
        return Err(FormatError::InvalidTargetField {
            format: FormatId::OpenAiChat.as_str().to_string(),
            field: "tools[].function.name".to_string(),
            reason: "OpenAI Chat function tools require a non-empty name".to_string(),
        });
    }
    Ok(())
}

fn validate_claude_cross_format_request(body: &Value, target: FormatId) -> Result<(), FormatError> {
    if claude_request_contains_provider_cache_control(body) {
        return Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::ClaudeMessages.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "cache_control".to_string(),
            reason: "target format has no lossless equivalent for Claude cache_control".to_string(),
        });
    }

    if let Some(output_effort) = body
        .as_object()
        .and_then(|object| object.get("output_config"))
        .and_then(Value::as_object)
        .and_then(|output_config| output_config.get("effort"))
    {
        validate_claude_output_effort_value(output_effort)?;
    }

    match target {
        FormatId::OpenAiChat if claude_request_contains_tool_result_content_array(body) => {
            return Err(FormatError::LossyConversionBlocked {
                source_format: FormatId::ClaudeMessages.as_str().to_string(),
                target_format: target.as_str().to_string(),
                field: "messages[].content[].tool_result.content".to_string(),
                reason: "OpenAI Chat tool messages cannot losslessly preserve Claude multi-block tool_result content".to_string(),
            });
        }
        FormatId::OpenAiChat => {}
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact => {
            if claude_request_contains_message_thinking_blocks(body) {
                return Err(FormatError::LossyConversionBlocked {
                    source_format: FormatId::ClaudeMessages.as_str().to_string(),
                    target_format: target.as_str().to_string(),
                    field: "messages[].content[].type".to_string(),
                    reason: "OpenAI Responses request input cannot losslessly preserve Claude thinking blocks".to_string(),
                });
            }
            if claude_request_contains_tool_result_content_array(body) {
                return Err(FormatError::LossyConversionBlocked {
                    source_format: FormatId::ClaudeMessages.as_str().to_string(),
                    target_format: target.as_str().to_string(),
                    field: "messages[].content[].tool_result.content".to_string(),
                    reason: "OpenAI Responses function_call_output cannot losslessly preserve Claude multi-block tool_result content".to_string(),
                });
            }
        }
        FormatId::GeminiGenerateContent
            if claude_request_contains_redacted_thinking_blocks(body) =>
        {
            return Err(FormatError::LossyConversionBlocked {
                source_format: FormatId::ClaudeMessages.as_str().to_string(),
                target_format: target.as_str().to_string(),
                field: "messages[].content[].type".to_string(),
                reason: "Gemini request parts cannot losslessly preserve Claude redacted thinking blocks".to_string(),
            });
        }
        FormatId::GeminiGenerateContent => {}
        _ => {}
    }
    Ok(())
}

fn validate_claude_output_effort_value(value: &Value) -> Result<(), FormatError> {
    let Some(raw) = value.as_str() else {
        return Err(FormatError::InvalidTargetField {
            format: FormatId::ClaudeMessages.as_str().to_string(),
            field: "output_config.effort".to_string(),
            reason: "Claude output effort must be a string".to_string(),
        });
    };
    if crate::protocol::canonical::claude_output_effort_to_openai_reasoning_effort(raw).is_some() {
        Ok(())
    } else {
        Err(FormatError::InvalidEnumValue {
            format: FormatId::ClaudeMessages.as_str().to_string(),
            field: "output_config.effort".to_string(),
            value: raw.to_string(),
        })
    }
}

fn validate_gemini_cross_format_request(body: &Value, target: FormatId) -> Result<(), FormatError> {
    let Some(object) = body.as_object() else {
        return Ok(());
    };

    if object.contains_key("safetySettings") || object.contains_key("safety_settings") {
        return Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::GeminiGenerateContent.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "safetySettings".to_string(),
            reason: "target format has no lossless equivalent for Gemini safetySettings"
                .to_string(),
        });
    }
    if object.contains_key("cachedContent") || object.contains_key("cached_content") {
        return Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::GeminiGenerateContent.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "cachedContent".to_string(),
            reason: "target format has no lossless equivalent for Gemini cachedContent".to_string(),
        });
    }
    if let Some(generation_config) = object_by_case(object, "generationConfig", "generation_config")
    {
        if generation_config.contains_key("responseModalities")
            || generation_config.contains_key("response_modalities")
        {
            return Err(FormatError::LossyConversionBlocked {
                source_format: FormatId::GeminiGenerateContent.as_str().to_string(),
                target_format: target.as_str().to_string(),
                field: "generationConfig.responseModalities".to_string(),
                reason: "target format has no lossless equivalent for Gemini responseModalities"
                    .to_string(),
            });
        }
        if let Some(thinking_config) =
            object_by_case(generation_config, "thinkingConfig", "thinking_config")
        {
            validate_gemini_cross_format_thinking_config(thinking_config)?;
        }
    }
    if let Some(tool_config) = object_by_case(object, "toolConfig", "tool_config") {
        validate_gemini_cross_format_tool_config(tool_config, target)?;
    }
    if gemini_request_contains_builtin_tool(body, "codeExecution", "code_execution") {
        return Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::GeminiGenerateContent.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "tools[].codeExecution".to_string(),
            reason: "target format has no lossless equivalent for Gemini codeExecution".to_string(),
        });
    }
    if gemini_request_contains_builtin_tool(body, "urlContext", "url_context") {
        return Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::GeminiGenerateContent.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "tools[].urlContext".to_string(),
            reason: "target format has no lossless equivalent for Gemini urlContext".to_string(),
        });
    }
    if matches!(
        target,
        FormatId::OpenAiResponses | FormatId::OpenAiResponsesCompact
    ) && gemini_request_contains_thought_parts(body)
    {
        return Err(FormatError::LossyConversionBlocked {
            source_format: FormatId::GeminiGenerateContent.as_str().to_string(),
            target_format: target.as_str().to_string(),
            field: "contents[].parts[].thoughtSignature".to_string(),
            reason:
                "OpenAI Responses request input cannot losslessly preserve Gemini thought parts"
                    .to_string(),
        });
    }
    Ok(())
}

fn validate_gemini_cross_format_thinking_config(
    thinking_config: &Map<String, Value>,
) -> Result<(), FormatError> {
    if let Some(value) = thinking_config
        .get("thinkingLevel")
        .or_else(|| thinking_config.get("thinking_level"))
    {
        let Some(raw) = value.as_str() else {
            return Err(FormatError::InvalidTargetField {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: "generationConfig.thinkingConfig.thinkingLevel".to_string(),
                reason: "Gemini thinkingLevel must be a string".to_string(),
            });
        };
        if !matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "low" | "medium" | "high"
        ) {
            return Err(FormatError::InvalidEnumValue {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: "generationConfig.thinkingConfig.thinkingLevel".to_string(),
                value: raw.to_string(),
            });
        }
    }
    for key in thinking_config.keys() {
        if !matches!(
            key.as_str(),
            "includeThoughts"
                | "include_thoughts"
                | "thinkingBudget"
                | "thinking_budget"
                | "thinkingLevel"
                | "thinking_level"
        ) {
            return Err(FormatError::UnsupportedField {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: format!("generationConfig.thinkingConfig.{key}"),
                reason: "Gemini thinkingConfig field has no canonical cross-format mapping"
                    .to_string(),
            });
        }
    }
    Ok(())
}

fn validate_gemini_cross_format_tool_config(
    tool_config: &Map<String, Value>,
    target: FormatId,
) -> Result<(), FormatError> {
    for key in tool_config.keys() {
        if !matches!(
            key.as_str(),
            "functionCallingConfig" | "function_calling_config"
        ) {
            return Err(FormatError::UnsupportedField {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: format!("toolConfig.{key}"),
                reason: "Gemini toolConfig field has no canonical cross-format mapping".to_string(),
            });
        }
    }
    let Some(function_calling_config) = object_by_case(
        tool_config,
        "functionCallingConfig",
        "function_calling_config",
    ) else {
        return Err(FormatError::UnsupportedField {
            format: FormatId::GeminiGenerateContent.as_str().to_string(),
            field: "toolConfig".to_string(),
            reason: "Gemini toolConfig must use functionCallingConfig for cross-format conversion"
                .to_string(),
        });
    };
    for key in function_calling_config.keys() {
        if !matches!(
            key.as_str(),
            "mode" | "allowedFunctionNames" | "allowed_function_names"
        ) {
            return Err(FormatError::UnsupportedField {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: format!("toolConfig.functionCallingConfig.{key}"),
                reason: "Gemini functionCallingConfig field has no canonical cross-format mapping"
                    .to_string(),
            });
        }
    }
    if let Some(value) = function_calling_config.get("mode") {
        let Some(raw) = value.as_str() else {
            return Err(FormatError::InvalidTargetField {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: "toolConfig.functionCallingConfig.mode".to_string(),
                reason: "Gemini function calling mode must be a string".to_string(),
            });
        };
        if !matches!(
            raw.trim().to_ascii_uppercase().as_str(),
            "NONE" | "AUTO" | "ANY" | "REQUIRED"
        ) {
            return Err(FormatError::InvalidEnumValue {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: "toolConfig.functionCallingConfig.mode".to_string(),
                value: raw.to_string(),
            });
        }
    }
    if let Some(value) = function_calling_config
        .get("allowedFunctionNames")
        .or_else(|| function_calling_config.get("allowed_function_names"))
    {
        let Some(names) = value.as_array() else {
            return Err(FormatError::InvalidTargetField {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: "toolConfig.functionCallingConfig.allowedFunctionNames".to_string(),
                reason: "Gemini allowedFunctionNames must be an array of strings".to_string(),
            });
        };
        if !names.iter().all(|value| value.as_str().is_some()) {
            return Err(FormatError::InvalidTargetField {
                format: FormatId::GeminiGenerateContent.as_str().to_string(),
                field: "toolConfig.functionCallingConfig.allowedFunctionNames".to_string(),
                reason: "Gemini allowedFunctionNames must be an array of strings".to_string(),
            });
        }
        if names.len() > 1 {
            return Err(FormatError::LossyConversionBlocked {
                source_format: FormatId::GeminiGenerateContent.as_str().to_string(),
                target_format: target.as_str().to_string(),
                field: "toolConfig.functionCallingConfig.allowedFunctionNames".to_string(),
                reason: "target format can represent at most one named tool choice".to_string(),
            });
        }
    }
    Ok(())
}

fn object_by_case<'a>(
    object: &'a Map<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<&'a Map<String, Value>> {
    object
        .get(camel)
        .or_else(|| object.get(snake))
        .and_then(Value::as_object)
}

fn claude_request_contains_message_thinking_blocks(body: &Value) -> bool {
    claude_request_contains_block_type(body, "thinking")
        || claude_request_contains_block_type(body, "redacted_thinking")
}

fn claude_request_contains_redacted_thinking_blocks(body: &Value) -> bool {
    claude_request_contains_block_type(body, "redacted_thinking")
}

fn claude_request_contains_block_type(body: &Value, expected_type: &str) -> bool {
    let Some(messages) = body
        .as_object()
        .and_then(|object| object.get("messages"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|block_type| block_type.eq_ignore_ascii_case(expected_type))
                })
            })
    })
}

fn claude_request_contains_tool_result_content_array(body: &Value) -> bool {
    let Some(messages) = body
        .as_object()
        .and_then(|object| object.get("messages"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|block_type| block_type.eq_ignore_ascii_case("tool_result"))
                        && block.get("content").is_some_and(Value::is_array)
                })
            })
    })
}

fn claude_request_contains_unrepresentable_tool_result_content_for_openai_chat(
    body: &Value,
) -> bool {
    claude_request_contains_unrepresentable_tool_result_content(body, |parts| {
        !openai_chat::request::claude_tool_result_parts_are_openai_chat_representable(parts)
    })
}

fn claude_request_contains_unrepresentable_tool_result_content_for_openai_responses(
    body: &Value,
) -> bool {
    claude_request_contains_unrepresentable_tool_result_content(body, |parts| {
        !openai_responses::request::claude_tool_result_parts_are_openai_responses_representable(
            parts,
        )
    })
}

fn claude_request_contains_unrepresentable_tool_result_content(
    body: &Value,
    is_unrepresentable: impl Fn(&[Value]) -> bool,
) -> bool {
    let Some(messages) = body
        .as_object()
        .and_then(|object| object.get("messages"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    messages.iter().any(|message| {
        message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks.iter().any(|block| {
                    block
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|block_type| block_type.eq_ignore_ascii_case("tool_result"))
                        && block
                            .get("content")
                            .and_then(Value::as_array)
                            .is_some_and(|parts| is_unrepresentable(parts.as_slice()))
                })
            })
    })
}

fn gemini_request_contains_builtin_tool(body: &Value, camel: &str, snake: &str) -> bool {
    let Some(tools) = body
        .as_object()
        .and_then(|object| object.get("tools"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    tools.iter().any(|tool| {
        tool.as_object()
            .is_some_and(|object| object.contains_key(camel) || object.contains_key(snake))
    })
}

fn gemini_request_contains_thought_parts(body: &Value) -> bool {
    let Some(contents) = body
        .as_object()
        .and_then(|object| object.get("contents"))
        .and_then(Value::as_array)
    else {
        return false;
    };
    contents.iter().any(|content| {
        content
            .get("parts")
            .and_then(Value::as_array)
            .is_some_and(|parts| {
                parts.iter().any(|part| {
                    part.get("thought")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
            })
    })
}

fn claude_request_contains_provider_cache_control(body: &Value) -> bool {
    let Some(object) = body.as_object() else {
        return false;
    };
    object.contains_key("cache_control")
        || claude_system_contains_cache_control(object.get("system"))
        || claude_messages_contain_cache_control(object.get("messages"))
        || claude_tools_contain_cache_control(object.get("tools"))
}

fn claude_system_contains_cache_control(system: Option<&Value>) -> bool {
    match system {
        Some(Value::Array(blocks)) => blocks.iter().any(|block| {
            block
                .as_object()
                .is_some_and(|object| object.contains_key("cache_control"))
        }),
        _ => false,
    }
}

fn claude_messages_contain_cache_control(messages: Option<&Value>) -> bool {
    let Some(messages) = messages.and_then(Value::as_array) else {
        return false;
    };
    messages.iter().any(|message| {
        message
            .get("content")
            .is_some_and(claude_content_value_contains_cache_control)
    })
}

fn claude_content_value_contains_cache_control(content: &Value) -> bool {
    match content {
        Value::Array(blocks) => blocks
            .iter()
            .any(claude_content_block_contains_cache_control),
        _ => false,
    }
}

fn claude_content_block_contains_cache_control(block: &Value) -> bool {
    let Some(object) = block.as_object() else {
        return false;
    };
    if object.contains_key("cache_control") {
        return true;
    }
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|block_type| block_type.eq_ignore_ascii_case("tool_result"))
        && object
            .get("content")
            .is_some_and(claude_content_value_contains_cache_control)
}

fn claude_tools_contain_cache_control(tools: Option<&Value>) -> bool {
    let Some(tools) = tools.and_then(Value::as_array) else {
        return false;
    };
    tools.iter().any(|tool| {
        tool.as_object()
            .is_some_and(|object| object.contains_key("cache_control"))
    })
}

fn build_request_conversion_report(
    source_format: &str,
    target_format: &str,
    body: &Value,
    output: &Value,
) -> ConversionReport {
    let mut report = ConversionReport::new(source_format, target_format);
    let source = body.as_object();
    let target = output.as_object();
    if let Some(source) = source {
        for key in source.keys() {
            let status = if target.is_some_and(|target| target.contains_key(key)) {
                ConversionFieldStatus::Native
            } else if request_field_has_known_mapping(source_format, target_format, key.as_str()) {
                ConversionFieldStatus::Mapped
            } else {
                ConversionFieldStatus::ExtensionPreserved
            };
            report.record(key.clone(), status, None);
        }
    }
    if let Some(target) = target {
        for key in target.keys() {
            if source.is_some_and(|source| source.contains_key(key)) {
                continue;
            }
            if request_field_is_target_native(source_format, target_format, key.as_str()) {
                report.record(
                    key.clone(),
                    ConversionFieldStatus::Mapped,
                    Some("emitted from canonical field".to_string()),
                );
            }
        }
    }
    report
}

fn build_response_conversion_report(
    source_format: &str,
    target_format: &str,
    body: &Value,
    output: &Value,
) -> ConversionReport {
    let mut report = ConversionReport::new(source_format, target_format);
    let source = body.as_object();
    let target = output.as_object();
    if let Some(source) = source {
        for key in source.keys() {
            let status = if target.is_some_and(|target| target.contains_key(key)) {
                ConversionFieldStatus::Native
            } else if response_field_has_known_mapping(source_format, target_format, key.as_str()) {
                ConversionFieldStatus::Mapped
            } else {
                ConversionFieldStatus::ExtensionPreserved
            };
            report.record(key.clone(), status, None);
        }
    }
    if let Some(target) = target {
        for key in target.keys() {
            if source.is_some_and(|source| source.contains_key(key)) {
                continue;
            }
            if response_field_is_target_native(source_format, target_format, key.as_str()) {
                report.record(
                    key.clone(),
                    ConversionFieldStatus::Mapped,
                    Some("emitted from canonical response field".to_string()),
                );
            }
        }
    }
    report
}

fn request_field_has_known_mapping(source_format: &str, target_format: &str, field: &str) -> bool {
    matches!(
        (
            normalize_known_format(source_format),
            normalize_known_format(target_format),
            field,
        ),
        (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "messages"
        ) | (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "max_tokens"
        ) | (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "max_completion_tokens"
        ) | (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "response_format"
        ) | (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "reasoning_effort"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "input"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "instructions"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "max_output_tokens"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "text"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "reasoning"
        )
    )
}

fn request_field_is_target_native(source_format: &str, target_format: &str, field: &str) -> bool {
    matches!(
        (
            normalize_known_format(source_format),
            normalize_known_format(target_format),
            field,
        ),
        (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "input"
        ) | (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "max_output_tokens"
        ) | (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "text"
        ) | (
            Some(FormatId::OpenAiChat),
            Some(FormatId::OpenAiResponses),
            "reasoning"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "messages"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "max_completion_tokens"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "response_format"
        ) | (
            Some(FormatId::OpenAiResponses),
            Some(FormatId::OpenAiChat),
            "reasoning_effort"
        )
    )
}

fn response_field_has_known_mapping(source_format: &str, target_format: &str, field: &str) -> bool {
    match (
        normalize_known_format(source_format),
        normalize_known_format(target_format),
    ) {
        (Some(FormatId::OpenAiChat), Some(FormatId::OpenAiResponses)) => {
            matches!(field, "choices")
        }
        (Some(FormatId::OpenAiResponses), Some(FormatId::OpenAiChat)) => {
            matches!(
                field,
                "output" | "status" | "incomplete_details" | "output_text"
            )
        }
        (
            Some(FormatId::ClaudeMessages),
            Some(FormatId::OpenAiChat | FormatId::OpenAiResponses),
        ) => {
            matches!(field, "content" | "stop_reason" | "stop_sequence")
        }
        (
            Some(FormatId::GeminiGenerateContent),
            Some(FormatId::OpenAiChat | FormatId::OpenAiResponses),
        ) => {
            matches!(field, "candidates" | "usageMetadata" | "usage_metadata")
        }
        (
            Some(FormatId::OpenAiChat | FormatId::OpenAiResponses),
            Some(FormatId::ClaudeMessages),
        ) => {
            matches!(
                field,
                "choices" | "output" | "status" | "incomplete_details"
            )
        }
        (
            Some(FormatId::OpenAiChat | FormatId::OpenAiResponses),
            Some(FormatId::GeminiGenerateContent),
        ) => {
            matches!(
                field,
                "choices" | "output" | "status" | "incomplete_details"
            )
        }
        _ => false,
    }
}

fn response_field_is_target_native(source_format: &str, target_format: &str, field: &str) -> bool {
    match (
        normalize_known_format(source_format),
        normalize_known_format(target_format),
    ) {
        (Some(FormatId::OpenAiChat), Some(FormatId::OpenAiResponses)) => {
            matches!(field, "output" | "status" | "output_text")
        }
        (Some(FormatId::OpenAiResponses), Some(FormatId::OpenAiChat)) => matches!(field, "choices"),
        (_, Some(FormatId::OpenAiChat)) => matches!(field, "choices"),
        (_, Some(FormatId::OpenAiResponses)) => {
            matches!(field, "output" | "status" | "output_text")
        }
        (_, Some(FormatId::ClaudeMessages)) => matches!(field, "content" | "stop_reason"),
        (_, Some(FormatId::GeminiGenerateContent)) => matches!(field, "candidates"),
        _ => false,
    }
}

fn normalize_known_format(format: &str) -> Option<FormatId> {
    match FormatId::parse(format)? {
        FormatId::OpenAiResponsesCompact => Some(FormatId::OpenAiResponses),
        value => Some(value),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        convert_request, convert_request_pure, convert_request_pure_with_context,
        convert_response_pure, FormatContext,
    };
    use crate::formats::id::FormatId;

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
    fn claude_request_to_responses_accepts_context_management() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 128,
            "context_management": {
                "edits": [{"type": "clear_tool_uses_20250919"}]
            }
        });

        let converted = convert_request_pure("claude:messages", "openai:responses", &body)
            .expect("context_management is audited for Claude cross-format requests")
            .value;

        assert_eq!(converted["model"], "claude-sonnet");
        assert!(converted.get("context_management").is_none());
    }

    #[test]
    fn pure_request_conversion_does_not_apply_model_or_stream_edits() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let ctx = FormatContext::default()
            .with_mapped_model("gpt-target")
            .with_upstream_stream(true);

        let converted =
            convert_request_pure_with_context("openai:chat", "openai:responses", &body, &ctx)
                .expect("pure conversion should succeed");

        assert_eq!(converted.value["model"], "gpt-source");
        assert!(converted.value.get("stream").is_none());
        assert!(converted
            .report
            .fields
            .iter()
            .any(|field| field.field == "messages"));
    }

    #[test]
    fn pure_openai_chat_to_responses_preserves_explicit_tool_strict() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "parameters": {"type": "object"},
                    "strict": true
                }
            }]
        });

        let converted = convert_request_pure("openai:chat", "openai:responses", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(converted["tools"][0]["strict"], true);
    }

    #[test]
    fn pure_openai_responses_to_chat_preserves_explicit_tool_strict() {
        let body = json!({
            "model": "gpt-source",
            "input": [{"role": "user", "content": "hello"}],
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {"type": "object"},
                "strict": false
            }]
        });

        let converted = convert_request_pure("openai:responses", "openai:chat", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(converted["tools"][0]["function"]["strict"], false);
    }

    #[test]
    fn pure_openai_chat_to_responses_maps_tool_call_ids_to_call_ids() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_lookup_1",
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "arguments": "{\"q\":\"rust\"}"
                    }
                }]
            }, {
                "role": "tool",
                "tool_call_id": "call_lookup_1",
                "content": "{\"ok\":true}"
            }]
        });

        let converted = convert_request_pure("openai:chat", "openai:responses", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(converted["input"][0]["type"], "function_call");
        assert!(converted["input"][0].get("id").is_none());
        assert_eq!(converted["input"][0]["call_id"], "call_lookup_1");
        assert_eq!(converted["input"][1]["type"], "function_call_output");
        assert_eq!(converted["input"][1]["call_id"], "call_lookup_1");
    }

    #[test]
    fn pure_openai_responses_to_chat_maps_call_ids_to_tool_call_ids() {
        let body = json!({
            "model": "gpt-source",
            "input": [{
                "type": "function_call",
                "call_id": "call_lookup_1",
                "name": "lookup",
                "arguments": "{\"q\":\"rust\"}"
            }, {
                "type": "function_call_output",
                "call_id": "call_lookup_1",
                "output": "{\"ok\":true}"
            }]
        });

        let converted = convert_request_pure("openai:responses", "openai:chat", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(
            converted["messages"][0]["tool_calls"][0]["id"],
            "call_lookup_1"
        );
        assert_eq!(converted["messages"][1]["tool_call_id"], "call_lookup_1");
    }

    #[test]
    fn pure_gemini_to_openai_chat_maps_function_response_id_to_tool_call_id() {
        let body = json!({
            "contents": [{
                "role": "model",
                "parts": [{
                    "functionCall": {
                        "id": "call_lookup_1",
                        "name": "lookup",
                        "args": {"q": "rust"}
                    }
                }]
            }, {
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "id": "call_lookup_1",
                        "name": "lookup",
                        "response": {"result": {"ok": true}}
                    }
                }]
            }]
        });

        let converted = convert_request_pure("gemini:generate_content", "openai:chat", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(
            converted["messages"][0]["tool_calls"][0]["id"],
            "call_lookup_1"
        );
        assert_eq!(converted["messages"][1]["tool_call_id"], "call_lookup_1");
    }

    #[test]
    fn pure_claude_to_openai_chat_maps_disable_parallel_tool_use() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 64,
            "tool_choice": {
                "type": "auto",
                "disable_parallel_tool_use": true
            }
        });

        let converted = convert_request_pure("claude:messages", "openai:chat", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(converted["parallel_tool_calls"], false);
    }

    #[test]
    fn pure_claude_to_openai_chat_preserves_max_output_effort() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 64,
            "output_config": {
                "effort": "max"
            }
        });

        let converted = convert_request_pure("claude:messages", "openai:chat", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(converted["reasoning_effort"], "max");
    }

    #[test]
    fn pure_openai_chat_to_claude_maps_parallel_tool_calls() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
            "parallel_tool_calls": false
        });

        let converted = convert_request_pure("openai:chat", "claude:messages", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(converted["tool_choice"]["type"], "auto");
        assert_eq!(converted["tool_choice"]["disable_parallel_tool_use"], true);
    }

    #[test]
    fn pure_claude_to_openai_responses_blocks_message_thinking_loss() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "assistant",
                "content": [{
                    "type": "thinking",
                    "thinking": "plan",
                    "signature": "sig_123"
                }]
            }],
            "max_tokens": 64
        });

        let error = convert_request_pure("claude:messages", "openai:responses", &body)
            .expect_err("Claude thinking blocks should fail closed for Responses");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "messages[].content[].type"
        ));
    }

    #[test]
    fn pure_claude_to_openai_chat_blocks_structured_tool_result_loss() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_123",
                    "content": [{
                        "type": "text",
                        "text": "first"
                    }, {
                        "type": "text",
                        "text": "second"
                    }]
                }]
            }],
            "max_tokens": 64
        });

        let error = convert_request_pure("claude:messages", "openai:chat", &body)
            .expect_err("Claude tool_result block arrays should fail closed for Chat");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "messages[].content[].tool_result.content"
        ));
    }

    #[test]
    fn pure_claude_to_gemini_blocks_redacted_thinking_loss() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "assistant",
                "content": [{
                    "type": "redacted_thinking",
                    "data": "enc_123"
                }]
            }],
            "max_tokens": 64
        });

        let error = convert_request_pure("claude:messages", "gemini:generate_content", &body)
            .expect_err("Claude redacted thinking should fail closed for Gemini");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "messages[].content[].type"
        ));
    }

    #[test]
    fn pure_gemini_to_openai_chat_maps_allowed_function_names_to_named_tool_choice() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup"]
                }
            }
        });

        let converted = convert_request_pure("gemini:generate_content", "openai:chat", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(converted["tool_choice"]["type"], "function");
        assert_eq!(converted["tool_choice"]["function"]["name"], "lookup");
    }

    #[test]
    fn pure_gemini_to_openai_chat_maps_thinking_level_to_reasoning_effort() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "generationConfig": {
                "thinkingConfig": {
                    "includeThoughts": true,
                    "thinkingLevel": "high"
                }
            }
        });

        let converted = convert_request_pure("gemini:generate_content", "openai:chat", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(converted["reasoning_effort"], "high");
    }

    #[test]
    fn pure_openai_chat_to_gemini_maps_named_tool_choice_to_allowed_function_names() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
            "tool_choice": {
                "type": "function",
                "function": {"name": "lookup"}
            }
        });

        let converted = convert_request_pure("openai:chat", "gemini:generate_content", &body)
            .expect("pure conversion should succeed")
            .value;

        assert_eq!(
            converted["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"][0],
            "lookup"
        );
    }

    #[test]
    fn pure_gemini_to_openai_responses_blocks_thought_part_loss() {
        let body = json!({
            "contents": [{
                "role": "model",
                "parts": [{
                    "text": "plan",
                    "thought": true,
                    "thoughtSignature": "sig_123"
                }]
            }]
        });

        let error = convert_request_pure("gemini:generate_content", "openai:responses", &body)
            .expect_err("Gemini thought parts should fail closed for Responses");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "contents[].parts[].thoughtSignature"
        ));
    }

    #[test]
    fn pure_gemini_to_openai_chat_blocks_safety_settings_loss() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "safetySettings": [{
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "threshold": "BLOCK_NONE"
            }]
        });

        let error = convert_request_pure("gemini:generate_content", "openai:chat", &body)
            .expect_err("Gemini safetySettings should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "safetySettings"
        ));
    }

    #[test]
    fn pure_gemini_to_openai_chat_blocks_response_modalities_loss() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "generationConfig": {
                "responseModalities": ["TEXT", "IMAGE"]
            }
        });

        let error = convert_request_pure("gemini:generate_content", "openai:chat", &body)
            .expect_err("Gemini responseModalities should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "generationConfig.responseModalities"
        ));
    }

    #[test]
    fn pure_gemini_to_openai_chat_blocks_multiple_allowed_function_names() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup", "search"]
                }
            }
        });

        let error = convert_request_pure("gemini:generate_content", "openai:chat", &body)
            .expect_err("multiple Gemini allowed function names should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "toolConfig.functionCallingConfig.allowedFunctionNames"
        ));
    }

    #[test]
    fn pure_gemini_to_openai_chat_rejects_invalid_tool_mode_enum() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "SOMETIME"
                }
            }
        });

        let error = convert_request_pure("gemini:generate_content", "openai:chat", &body)
            .expect_err("invalid Gemini tool mode should fail closed");

        assert!(matches!(
            error,
            super::FormatError::InvalidEnumValue { ref field, ref value, .. }
                if field == "toolConfig.functionCallingConfig.mode" && value == "SOMETIME"
        ));
    }

    #[test]
    fn pure_gemini_to_claude_blocks_code_execution_tool_loss() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "tools": [{
                "codeExecution": {}
            }]
        });

        let error = convert_request_pure("gemini:generate_content", "claude:messages", &body)
            .expect_err("Gemini codeExecution should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "tools[].codeExecution"
        ));
    }

    #[test]
    fn pure_claude_to_openai_chat_blocks_cache_control_loss() {
        let body = json!({
            "model": "claude-sonnet",
            "system": [{
                "type": "text",
                "text": "Cache this.",
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 64
        });

        let error = convert_request_pure("claude:messages", "openai:chat", &body)
            .expect_err("Claude cache_control should fail closed cross-format");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "cache_control"
        ));
    }

    #[test]
    fn pure_claude_to_openai_chat_allows_tool_schema_property_named_cache_control() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 64,
            "tools": [{
                "name": "configure_cache",
                "description": "Configure application cache behavior",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "cache_control": {
                            "type": "string",
                            "description": "Application-level cache policy"
                        }
                    }
                }
            }]
        });

        let converted = convert_request_pure("claude:messages", "openai:chat", &body)
            .expect("tool schema property names should not be treated as Claude cache_control")
            .value;

        assert_eq!(
            converted["tools"][0]["function"]["parameters"]["properties"]["cache_control"]["type"],
            "string"
        );
    }

    #[test]
    fn pure_openai_chat_to_responses_blocks_lossy_chat_only_fields() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
            "n": 2
        });

        let error = convert_request_pure("openai:chat", "openai:responses", &body)
            .expect_err("lossy field should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. } if field == "n"
        ));
    }

    #[test]
    fn pure_openai_responses_to_chat_blocks_responses_only_fields() {
        let body = json!({
            "model": "gpt-source",
            "input": [{"role": "user", "content": "hello"}],
            "include": ["reasoning.encrypted_content"]
        });

        let error = convert_request_pure("openai:responses", "openai:chat", &body)
            .expect_err("lossy field should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. } if field == "include"
        ));
    }

    #[test]
    fn pure_cross_format_rejects_unknown_source_root_field() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
            "future_field": true
        });

        let error = convert_request_pure("openai:chat", "gemini:generate_content", &body)
            .expect_err("unknown source root field should fail closed");

        assert!(matches!(
            error,
            super::FormatError::UnauditedField {
                ref source_format,
                ref target_format,
                ref field,
                ..
            } if source_format == "openai:chat"
                && target_format == "gemini:generate_content"
                && field == "future_field"
        ));
    }

    #[test]
    fn same_format_canonical_roundtrip_preserves_future_root_fields() {
        let cases = [
            (
                "openai:chat",
                json!({
                    "model": "gpt-source",
                    "messages": [{"role": "user", "content": "hello"}],
                    "future_field": {"enabled": true}
                }),
                "future_field",
            ),
            (
                "openai:responses",
                json!({
                    "model": "gpt-source",
                    "input": [{"role": "user", "content": "hello"}],
                    "future_field": {"enabled": true}
                }),
                "future_field",
            ),
            (
                "claude:messages",
                json!({
                    "model": "claude-source",
                    "max_tokens": 1024,
                    "messages": [{"role": "user", "content": "hello"}],
                    "future_field": {"enabled": true}
                }),
                "future_field",
            ),
            (
                "gemini:generate_content",
                json!({
                    "model": "gemini-source",
                    "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
                    "futureField": {"enabled": true}
                }),
                "futureField",
            ),
        ];

        for (format, body, field) in cases {
            let converted = convert_request_pure(format, format, &body)
                .unwrap_or_else(|err| panic!("{format} same-format roundtrip failed: {err}"));
            assert_eq!(
                converted.value.get(field),
                body.get(field),
                "{format} must preserve unknown provider root fields in canonical roundtrip"
            );
        }
    }

    #[test]
    fn pure_openai_responses_request_same_format_preserves_raw_tools_and_roles() {
        let body = json!({
            "model": "gpt-source",
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "Use policy"}]
                },
                {"role": "user", "content": "hello"}
            ],
            "tools": [
                {
                    "type": "file_search",
                    "vector_store_ids": ["vs_123"],
                    "max_num_results": 3
                },
                {
                    "type": "mcp",
                    "server_label": "docs",
                    "server_url": "https://example.com/mcp"
                }
            ]
        });

        let converted = convert_request_pure("openai:responses", "openai:responses", &body)
            .expect("same-format Responses request should preserve official raw fields")
            .value;

        assert_eq!(converted["input"][0]["role"], "developer");
        assert_eq!(converted["input"][0]["content"][0]["text"], "Use policy");
        assert_eq!(converted["tools"][0]["type"], "file_search");
        assert_eq!(converted["tools"][0]["vector_store_ids"][0], "vs_123");
        assert_eq!(converted["tools"][1]["type"], "mcp");
        assert_eq!(converted["tools"][1]["server_label"], "docs");
    }

    #[test]
    fn pure_openai_responses_same_format_preserves_ptc_contract() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {
                    "type": "program",
                    "id": "program_1",
                    "code": "result = lookup({\"id\": 7})"
                },
                {
                    "type": "program_output",
                    "program_id": "program_1",
                    "output": "ok"
                },
                {
                    "type": "agent_message",
                    "agent_id": "agent_1",
                    "content": [{"type": "input_text", "text": "delegated result"}]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": "{\"id\":7}",
                    "caller": {"type": "program", "id": "program_1"}
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "found",
                    "caller": {"type": "program", "id": "program_1"}
                }
            ],
            "tools": [
                {
                    "type": "function",
                    "name": "lookup",
                    "description": "Look up a record",
                    "parameters": {
                        "type": "object",
                        "properties": {"id": {"type": "integer"}},
                        "required": ["id"]
                    },
                    "strict": true,
                    "output_schema": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}}
                    },
                    "allowed_callers": ["programmatic"]
                },
                {"type": "programmatic_tool_calling"}
            ]
        });

        let converted = convert_request_pure("openai:responses", "openai:responses", &body)
            .expect("same-format Responses conversion should preserve the PTC wire contract")
            .value;

        assert_eq!(converted["input"], body["input"]);
        assert_eq!(converted["tools"], body["tools"]);
        assert_eq!(
            converted["tools"][0]["allowed_callers"],
            json!(["programmatic"])
        );
        assert_eq!(converted["tools"][1]["type"], "programmatic_tool_calling");
    }

    #[test]
    fn pure_openai_responses_same_format_preserves_message_items() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "input": [
                {
                    "type": "message",
                    "id": "msg_developer",
                    "status": "completed",
                    "role": "developer",
                    "content": [{
                        "type": "input_text",
                        "text": "Use policy",
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    }]
                },
                {
                    "type": "message",
                    "id": "msg_user",
                    "status": "completed",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "hello"}]
                },
                {
                    "type": "message",
                    "id": "msg_assistant",
                    "status": "completed",
                    "role": "assistant",
                    "phase": "commentary",
                    "content": [
                        {"type": "output_text", "text": ""},
                        {"type": "refusal", "refusal": "cannot comply"},
                        {"type": "future_content", "payload": {"value": 1}}
                    ]
                }
            ]
        });

        let converted = convert_request_pure("openai:responses", "openai:responses", &body)
            .expect("same-format Responses conversion should preserve message input items")
            .value;

        assert_eq!(converted["input"], body["input"]);
    }

    #[test]
    fn pure_openai_responses_refusal_history_fails_closed_across_formats() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "input": [{
                "type": "message",
                "role": "assistant",
                "content": [{"type": "refusal", "refusal": "cannot comply"}]
            }]
        });

        for target in ["openai:chat", "claude:messages", "gemini:generate_content"] {
            let error = convert_request_pure("openai:responses", target, &body)
                .expect_err("target request format cannot represent refusal history");
            assert!(matches!(
                error,
                super::FormatError::LossyConversionBlocked { ref field, .. }
                    if field == "messages[].content[].type"
            ));
        }
    }

    #[test]
    fn pure_openai_responses_cross_format_rejects_opaque_input_item() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "input": [{
                "type": "program",
                "call_id": "program-call-1",
                "fingerprint": "fp-program-1"
            }]
        });

        let error = convert_request_pure("openai:responses", "openai:chat", &body)
            .expect_err("provider-scoped opaque item must fail closed across formats");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "input[0]"
        ));
    }

    #[test]
    fn runtime_openai_responses_cross_format_rejects_unknown_content_block() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "hello"},
                    {"type": "future_content_block", "payload": {"keep": true}}
                ]
            }]
        });

        let error = convert_request(
            "openai:responses",
            "claude:messages",
            &body,
            &FormatContext::default(),
        )
        .expect_err("runtime conversion must fail closed for raw provider content blocks");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "input[0].content[1]"
        ));
    }

    #[test]
    fn openai_responses_rejects_official_multi_agent_incompatibilities() {
        let cases = [
            (
                "openai:responses:compact",
                json!({
                    "model": "gpt-5.6-sol",
                    "input": "hello",
                    "multi_agent": {"enabled": true}
                }),
                "multi_agent",
            ),
            (
                "openai:responses",
                json!({
                    "model": "gpt-5.6-sol",
                    "input": "hello",
                    "multi_agent": {"enabled": true},
                    "reasoning": {"effort": "high", "summary": "auto"}
                }),
                "reasoning.summary",
            ),
            (
                "openai:responses",
                json!({
                    "model": "gpt-5.6-sol",
                    "input": "hello",
                    "multi_agent": {"enabled": true},
                    "max_tool_calls": 8
                }),
                "max_tool_calls",
            ),
        ];

        for (format, body, expected_field) in cases {
            let error = convert_request_pure(format, format, &body)
                .expect_err("invalid multi-agent combination must be rejected");
            assert!(matches!(
                error,
                super::FormatError::InvalidTargetField { ref field, .. }
                    if field == expected_field
            ));
        }
    }

    #[test]
    fn openai_responses_multi_agent_disabled_does_not_trigger_incompatibilities() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "input": "hello",
            "multi_agent": {"enabled": false},
            "reasoning": {"effort": "high", "summary": "auto"},
            "max_tool_calls": 8
        });

        let converted = convert_request_pure("openai:responses", "openai:responses", &body)
            .expect("disabled multi-agent config must not activate incompatibility rules");

        assert_eq!(converted.value["multi_agent"]["enabled"], false);
    }

    #[test]
    fn openai_prompt_cache_contract_converts_responses_to_chat_without_loss() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "store": false,
            "user": "user_123",
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"},
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "stable text",
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    },
                    {
                        "type": "input_image",
                        "image_url": "https://example.com/stable.png",
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    },
                    {
                        "type": "input_file",
                        "file_id": "file_123",
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    }
                ]
            }]
        });

        let converted = convert_request_pure("openai:responses", "openai:chat", &body)
            .expect("official Responses cache contract should map to Chat")
            .value;

        assert_eq!(
            converted["prompt_cache_options"],
            body["prompt_cache_options"]
        );
        assert_eq!(converted["store"], false);
        assert_eq!(converted["user"], "user_123");
        let parts = converted["messages"][0]["content"]
            .as_array()
            .expect("chat content should remain structured");
        assert_eq!(parts.len(), 3);
        assert!(parts
            .iter()
            .all(|part| { part["prompt_cache_breakpoint"] == json!({"mode": "explicit"}) }));
    }

    #[test]
    fn openai_prompt_cache_contract_converts_chat_to_responses_without_loss() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "store": false,
            "user": "user_123",
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"},
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": "stable text",
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    },
                    {
                        "type": "image_url",
                        "image_url": {"url": "https://example.com/stable.png"},
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    },
                    {
                        "type": "file",
                        "file": {"file_id": "file_123"},
                        "prompt_cache_breakpoint": {"mode": "explicit"}
                    }
                ]
            }]
        });

        let converted = convert_request_pure("openai:chat", "openai:responses", &body)
            .expect("official Chat cache contract should map to Responses")
            .value;

        assert_eq!(
            converted["prompt_cache_options"],
            body["prompt_cache_options"]
        );
        assert_eq!(converted["store"], false);
        assert_eq!(converted["user"], "user_123");
        let parts = converted["input"][0]["content"]
            .as_array()
            .expect("Responses content should remain structured");
        assert_eq!(parts.len(), 3);
        assert!(parts
            .iter()
            .all(|part| { part["prompt_cache_breakpoint"] == json!({"mode": "explicit"}) }));
    }

    #[test]
    fn openai_prompt_cache_contract_is_preserved_for_compact() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "prompt_cache_key": "tenant:compact",
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"},
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "stable text",
                    "prompt_cache_breakpoint": {"mode": "explicit"}
                }]
            }]
        });

        let converted = convert_request_pure("openai:chat", "openai:responses:compact", &body)
            .expect("Compact supports the GPT-5.6 prompt cache contract")
            .value;

        assert_eq!(converted["prompt_cache_key"], "tenant:compact");
        assert_eq!(
            converted["prompt_cache_options"],
            json!({"mode": "explicit", "ttl": "30m"})
        );
        assert_eq!(
            converted["input"][0]["content"][0]["prompt_cache_breakpoint"],
            json!({"mode": "explicit"})
        );
    }

    #[test]
    fn openai_prompt_cache_contract_uses_gpt_5_6_options_and_rejects_invalid_values() {
        let retention = json!({
            "model": "gpt-5.6-sol",
            "messages": [{"role": "user", "content": "hello"}],
            "prompt_cache_retention": "24h"
        });
        let error = convert_request_pure("openai:chat", "openai:responses", &retention)
            .expect_err("GPT-5.6 uses prompt_cache_options.ttl");
        assert!(matches!(
            error,
            super::FormatError::InvalidTargetField { ref field, .. }
                if field == "prompt_cache_retention"
        ));

        let invalid_options = json!({
            "model": "gpt-5.6-sol",
            "input": "hello",
            "prompt_cache_options": {"mode": "explicit", "ttl": "1h"}
        });
        let error = convert_request_pure("openai:responses", "openai:chat", &invalid_options)
            .expect_err("cross-format conversion must validate cache option enums");
        assert!(matches!(
            error,
            super::FormatError::InvalidEnumValue { ref field, .. }
                if field == "prompt_cache_options.ttl"
        ));
    }

    #[test]
    fn openai_prompt_cache_contract_preserves_earlier_model_retention() {
        let body = json!({
            "model": "gpt-5.5",
            "input": "hello",
            "prompt_cache_retention": "24h"
        });
        let converted = convert_request_pure("openai:responses", "openai:chat", &body)
            .expect("GPT-5.5 24h retention should map between OpenAI APIs")
            .value;

        assert_eq!(converted["prompt_cache_retention"], "24h");
    }

    #[test]
    fn runtime_openai_prompt_cache_contract_uses_mapped_provider_model() {
        let options = json!({
            "model": "gpt-5.5",
            "messages": [{"role": "user", "content": "hello"}],
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"}
        });
        let converted = convert_request(
            "openai:chat",
            "openai:responses",
            &options,
            &FormatContext::default().with_mapped_model("gpt-5.6-sol"),
        )
        .expect("mapped GPT-5.6 provider model should accept prompt_cache_options");
        assert_eq!(
            converted["prompt_cache_options"],
            options["prompt_cache_options"]
        );

        let error = convert_request(
            "openai:chat",
            "openai:responses",
            &options,
            &FormatContext::default().with_mapped_model("gpt-5.5"),
        )
        .expect_err("mapped earlier provider model should reject prompt_cache_options");
        assert!(matches!(
            error,
            super::FormatError::InvalidTargetField { ref field, .. }
                if field == "prompt_cache_options"
        ));

        let retention = json!({
            "model": "gpt-5.6-sol",
            "input": "hello",
            "prompt_cache_retention": "24h"
        });
        let converted = convert_request(
            "openai:responses",
            "openai:chat",
            &retention,
            &FormatContext::default().with_mapped_model("gpt-5.5"),
        )
        .expect("mapped earlier provider model should accept its retention contract");
        assert_eq!(converted["prompt_cache_retention"], "24h");

        let error = convert_request(
            "openai:responses",
            "openai:chat",
            &retention,
            &FormatContext::default().with_mapped_model("gpt-5.6-luna"),
        )
        .expect_err("mapped GPT-5.6 provider model uses prompt_cache_options.ttl");
        assert!(matches!(
            error,
            super::FormatError::InvalidTargetField { ref field, .. }
                if field == "prompt_cache_retention"
        ));
    }

    #[test]
    fn openai_cross_format_store_only_maps_false() {
        for (source, target, body) in [
            (
                "openai:chat",
                "openai:responses",
                json!({
                    "model": "gpt-5.6-sol",
                    "messages": [{"role": "user", "content": "hello"}],
                    "store": true
                }),
            ),
            (
                "openai:responses",
                "openai:chat",
                json!({
                    "model": "gpt-5.6-sol",
                    "input": "hello",
                    "store": true
                }),
            ),
        ] {
            let error = convert_request_pure(source, target, &body)
                .expect_err("store=true has different behavior across the OpenAI APIs");
            assert!(matches!(
                error,
                super::FormatError::LossyConversionBlocked { ref field, .. }
                    if field == "store"
            ));
        }
    }

    #[test]
    fn openai_chat_store_is_validated_then_omitted_for_compact() {
        for store in [true, false] {
            let converted = convert_request_pure(
                "openai:chat",
                "openai:responses:compact",
                &json!({
                    "model": "gpt-5.6-sol",
                    "messages": [{"role": "user", "content": "hello"}],
                    "store": store
                }),
            )
            .expect("Compact should validate and omit the transport-ineligible store field");
            assert!(converted.value.get("store").is_none());
        }

        let error = convert_request_pure(
            "openai:chat",
            "openai:responses:compact",
            &json!({
                "model": "gpt-5.6-sol",
                "messages": [{"role": "user", "content": "hello"}],
                "store": "true"
            }),
        )
        .expect_err("Compact should reject a non-boolean store field");
        assert!(matches!(
            error,
            super::FormatError::InvalidTargetField { ref field, .. }
                if field == "store"
        ));
    }

    #[test]
    fn openai_chat_audio_cache_breakpoint_fails_closed_for_responses() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "input_audio",
                    "input_audio": {"data": "ZmFrZQ==", "format": "mp3"},
                    "prompt_cache_breakpoint": {"mode": "explicit"}
                }]
            }]
        });

        let error = convert_request_pure("openai:chat", "openai:responses", &body)
            .expect_err("Responses does not support cache breakpoints on audio blocks");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "messages[].content[].prompt_cache_breakpoint"
        ));
    }

    #[test]
    fn openai_chat_assistant_text_cache_breakpoint_fails_closed_for_responses() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "messages": [{
                "role": "assistant",
                "content": [{
                    "type": "text",
                    "text": "previous output",
                    "prompt_cache_breakpoint": {"mode": "explicit"}
                }]
            }]
        });

        let error = convert_request_pure("openai:chat", "openai:responses", &body)
            .expect_err("assistant text maps to output_text, which cannot carry a breakpoint");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "messages[].content[].prompt_cache_breakpoint"
        ));
    }

    #[test]
    fn runtime_responses_cross_format_blocks_advanced_reasoning_and_caller_fields() {
        for (body, expected_field) in [
            (
                json!({
                    "model": "gpt-5.6-sol",
                    "input": "hello",
                    "reasoning": {"effort": "high", "mode": "pro"}
                }),
                "thinking.openai_responses.mode",
            ),
            (
                json!({
                    "model": "gpt-5.6-sol",
                    "input": "hello",
                    "reasoning": {"effort": "high", "context": "all_turns"}
                }),
                "thinking.openai_responses.context",
            ),
            (
                json!({
                    "model": "gpt-5.6-sol",
                    "input": "hello",
                    "multi_agent": {"enabled": true}
                }),
                "multi_agent",
            ),
            (
                json!({
                    "model": "gpt-5.6-sol",
                    "input": [{
                        "type": "function_call",
                        "call_id": "call_1",
                        "name": "lookup",
                        "arguments": "{}",
                        "caller": {"type": "multi_agent", "id": "agent_1"}
                    }]
                }),
                "input[0].caller",
            ),
            (
                json!({
                    "model": "gpt-5.6-sol",
                    "input": [{
                        "type": "web_search_call",
                        "id": "search_1",
                        "action": {"query": "latest docs"}
                    }]
                }),
                "input[0]",
            ),
        ] {
            let error = convert_request(
                "openai:responses",
                "openai:chat",
                &body,
                &FormatContext::default(),
            )
            .expect_err("runtime conversion must fail closed for provider-only fields");
            assert!(
                matches!(
                    error,
                    super::FormatError::LossyConversionBlocked { ref field, .. }
                        if field == expected_field
                ),
                "unexpected conversion error for {expected_field}: {error:?}"
            );
        }
    }

    #[test]
    fn pure_openai_chat_to_claude_blocks_target_unsupported_generation_field() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
            "top_logprobs": 2
        });

        let error = convert_request_pure("openai:chat", "claude:messages", &body)
            .expect_err("target-unsupported generation field should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "top_logprobs"
        ));
    }

    #[test]
    fn pure_openai_chat_to_gemini_blocks_unknown_content_part_loss() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "input_video",
                    "input_video": {"url": "https://example.com/movie.mp4"}
                }]
            }]
        });

        let error = convert_request_pure("openai:chat", "gemini:generate_content", &body)
            .expect_err("unknown content part should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "messages[].content[].type"
        ));
    }

    #[test]
    fn pure_openai_responses_to_chat_blocks_unmapped_context_management() {
        let body = json!({
            "model": "gpt-source",
            "input": [{"role": "user", "content": "hello"}],
            "context_management": []
        });

        let error = convert_request_pure("openai:responses", "openai:chat", &body)
            .expect_err("Responses context_management should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "openai_responses.context_management"
        ));
    }

    #[test]
    fn pure_claude_to_openai_chat_blocks_container_loss() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 64,
            "container": "container_123"
        });

        let error = convert_request_pure("claude:messages", "openai:chat", &body)
            .expect_err("Claude container should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "claude.container"
        ));
    }

    #[test]
    fn pure_gemini_to_openai_chat_blocks_service_tier_loss() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "serviceTier": "priority"
        });

        let error = convert_request_pure("gemini:generate_content", "openai:chat", &body)
            .expect_err("Gemini serviceTier should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "gemini.serviceTier"
        ));
    }

    #[test]
    fn pure_openai_cross_format_preserves_reasoning_effort_contract() {
        for effort in ["none", "low", "medium", "high", "xhigh", "max"] {
            let chat = json!({
                "model": "gpt-5.6-sol",
                "messages": [{"role": "user", "content": "hello"}],
                "reasoning_effort": effort
            });
            let responses = convert_request_pure("openai:chat", "openai:responses", &chat)
                .expect("Chat effort should convert to Responses")
                .value;
            assert_eq!(responses["reasoning"]["effort"], effort);

            let responses = json!({
                "model": "gpt-5.6-sol",
                "input": [{"role": "user", "content": "hello"}],
                "reasoning": {"effort": effort}
            });
            let chat = convert_request_pure("openai:responses", "openai:chat", &responses)
                .expect("Responses effort should convert to Chat")
                .value;
            assert_eq!(chat["reasoning_effort"], effort);
        }
    }

    #[test]
    fn runtime_openai_cross_format_preserves_gpt_5_6_reasoning_efforts() {
        for target in ["openai:responses", "openai:responses:compact"] {
            for effort in ["none", "low", "medium", "high", "xhigh", "max"] {
                let chat = json!({
                    "model": "deployment-alias",
                    "messages": [{"role": "user", "content": "hello"}],
                    "reasoning_effort": effort
                });
                let responses = convert_request(
                    "openai:chat",
                    target,
                    &chat,
                    &FormatContext::default().with_mapped_model("gpt-5.6-sol"),
                )
                .expect("runtime Chat effort should convert to Responses");
                assert_eq!(responses["reasoning"]["effort"], effort, "target: {target}");
            }
        }
    }

    #[test]
    fn runtime_reasoning_effort_uses_concrete_mapped_model_as_authoritative_capability() {
        let alias_to_gpt_5_6 = json!({
            "model": "deployment-alias",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "max"
        });
        let converted = convert_request(
            "openai:chat",
            "openai:responses",
            &alias_to_gpt_5_6,
            &FormatContext::default().with_mapped_model("gpt-5.6-sol"),
        )
        .expect("a concrete GPT-5.6 mapped target should accept max");
        assert_eq!(converted["model"], "gpt-5.6-sol");
        assert_eq!(converted["reasoning"]["effort"], "max");

        let gpt_5_6_to_gpt_5_4 = json!({
            "model": "gpt-5.6-sol",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "max"
        });
        let error = convert_request(
            "openai:chat",
            "openai:responses",
            &gpt_5_6_to_gpt_5_4,
            &FormatContext::default().with_mapped_model("gpt-5.4"),
        )
        .expect_err("a concrete GPT-5.4 mapped target must reject max");
        assert!(matches!(
            error,
            super::FormatError::InvalidTargetField { ref field, .. }
                if field == "reasoning_effort"
        ));
    }

    #[test]
    fn pure_conversion_reasoning_effort_uses_the_concrete_mapped_model() {
        let alias_to_gpt_5_6 = json!({
            "model": "deployment-alias",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "max"
        });
        let converted = convert_request_pure_with_context(
            "openai:chat",
            "openai:responses",
            &alias_to_gpt_5_6,
            &FormatContext::default().with_mapped_model("gpt-5.6-sol"),
        )
        .expect("a concrete GPT-5.6 target should accept max");
        assert_eq!(converted.value["reasoning"]["effort"], "max");

        let gpt_5_6_to_gpt_5_4 = json!({
            "model": "gpt-5.6-sol",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "max"
        });
        let error = convert_request_pure_with_context(
            "openai:chat",
            "openai:responses",
            &gpt_5_6_to_gpt_5_4,
            &FormatContext::default().with_mapped_model("gpt-5.4"),
        )
        .expect_err("a concrete GPT-5.4 target must reject max");
        assert!(matches!(
            error,
            super::FormatError::InvalidTargetField { ref field, .. }
                if field == "reasoning_effort"
        ));
    }

    #[test]
    fn runtime_openai_cross_format_enforces_known_efforts_and_preserves_custom_efforts() {
        let alias_minimal = json!({
            "model": "deployment-alias",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "minimal"
        });
        for target in ["openai:responses", "openai:responses:compact"] {
            let error = convert_request(
                "openai:chat",
                target,
                &alias_minimal,
                &FormatContext::default().with_mapped_model("gpt-5.6-terra"),
            )
            .expect_err("mapped GPT-5.6 deployments must reject minimal effort");
            assert!(matches!(
                error,
                super::FormatError::InvalidTargetField { ref field, .. }
                    if field == "reasoning_effort"
            ));
        }

        let custom = json!({
            "model": "gpt-5.6-sol",
            "input": [{"role": "user", "content": "hello"}],
            "reasoning": {"effort": "future"}
        });
        for source in ["openai:responses", "openai:responses:compact"] {
            let converted = convert_request(
                source,
                "openai:chat",
                &custom,
                &FormatContext::default().with_mapped_model("gpt-5.6-sol"),
            )
            .expect("model-advertised effort should survive runtime conversion");
            assert_eq!(converted["reasoning_effort"], "future");
        }

        let ultra = json!({
            "model": "gpt-5.6-sol",
            "input": [{"role": "user", "content": "hello"}],
            "reasoning": {"effort": "ultra"}
        });
        for source in ["openai:responses", "openai:responses:compact"] {
            let error = convert_request(
                source,
                "openai:chat",
                &ultra,
                &FormatContext::default().with_mapped_model("gpt-5.6-sol"),
            )
            .expect_err("Codex local ultra preset should not enter the OpenAI wire contract");
            assert!(matches!(
                error,
                super::FormatError::InvalidEnumValue { ref field, ref value, .. }
                    if field == "reasoning.effort" && value == "ultra"
            ));
        }
    }

    #[test]
    fn pure_openai_cross_format_preserves_custom_reasoning_effort() {
        let body = json!({
            "model": "gpt-5.6-sol",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "future"
        });

        let converted = convert_request_pure("openai:chat", "openai:responses", &body)
            .expect("custom effort should remain wire-compatible");
        assert_eq!(converted.value["reasoning"]["effort"], "future");

        let ultra = json!({
            "model": "gpt-5.6-sol",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "ultra"
        });
        let error = convert_request_pure("openai:chat", "openai:responses", &ultra)
            .expect_err("Codex local ultra preset should not enter the OpenAI wire contract");
        assert!(matches!(
            error,
            super::FormatError::InvalidEnumValue { ref field, ref value, .. }
                if field == "reasoning_effort" && value == "ultra"
        ));
    }

    #[test]
    fn pure_openai_cross_format_rejects_gpt_5_6_minimal_effort() {
        let chat = json!({
            "model": "gpt-5.6-sol",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "minimal"
        });
        let error = convert_request_pure("openai:chat", "openai:responses", &chat)
            .expect_err("GPT-5.6 does not publish minimal as a supported effort");
        assert!(matches!(
            error,
            super::FormatError::InvalidTargetField { ref field, .. }
                if field == "reasoning_effort"
        ));

        let responses = json!({
            "model": "gpt-5.6-terra",
            "input": [{"role": "user", "content": "hello"}],
            "reasoning": {"effort": "minimal"}
        });
        let error = convert_request_pure("openai:responses", "openai:chat", &responses)
            .expect_err("GPT-5.6 does not publish minimal as a supported effort");
        assert!(matches!(
            error,
            super::FormatError::InvalidTargetField { ref field, .. }
                if field == "reasoning.effort"
        ));
    }

    #[test]
    fn pure_response_conversion_rejects_unknown_openai_chat_finish_reason() {
        let body = json!({
            "id": "chatcmpl_unknown_finish",
            "object": "chat.completion",
            "model": "gpt-source",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello"
                },
                "finish_reason": "future_reason"
            }]
        });

        let error = convert_response_pure("openai:chat", "claude:messages", &body)
            .expect_err("unknown OpenAI finish reason should fail closed cross-format");

        assert!(matches!(
            error,
            super::FormatError::InvalidEnumValue { ref field, ref value, .. }
                if field == "choices[].finish_reason" && value == "future_reason"
        ));
    }

    #[test]
    fn pure_response_conversion_blocks_known_but_unmappable_gemini_finish_reason() {
        let body = json!({
            "responseId": "resp_other_finish",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "index": 0,
                "content": {
                    "role": "model",
                    "parts": [{"text": "hello"}]
                },
                "finishReason": "OTHER"
            }]
        });

        let error = convert_response_pure("gemini:generate_content", "openai:chat", &body)
            .expect_err("unmappable Gemini finish reason should fail closed cross-format");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "candidates[].finishReason"
        ));
    }

    #[test]
    fn pure_response_conversion_preserves_openai_responses_content_filter_finish_reason() {
        let body = json!({
            "id": "resp_content_filter",
            "object": "response",
            "model": "gpt-source",
            "status": "incomplete",
            "incomplete_details": {
                "reason": "content_filter"
            },
            "output": [{
                "type": "message",
                "id": "msg_content_filter",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "blocked"
                }]
            }]
        });

        let converted = convert_response_pure("openai:responses", "openai:chat", &body)
            .expect("content_filter incomplete reason should map to Chat")
            .value;

        assert_eq!(converted["choices"][0]["finish_reason"], "content_filter");
    }

    #[test]
    fn pure_response_conversion_reports_response_field_mappings() {
        let body = json!({
            "id": "resp_report",
            "object": "response",
            "model": "gpt-source",
            "status": "completed",
            "output": [{
                "type": "message",
                "id": "msg_report",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "hello"
                }]
            }]
        });

        let converted = convert_response_pure("openai:responses", "openai:chat", &body)
            .expect("response conversion should succeed");

        assert!(converted.report.fields.iter().any(|field| {
            field.field == "output" && field.status == super::ConversionFieldStatus::Mapped
        }));
        assert!(converted.report.fields.iter().any(|field| {
            field.field == "choices" && field.status == super::ConversionFieldStatus::Mapped
        }));
    }

    #[test]
    fn pure_response_conversion_blocks_non_terminal_openai_responses_status() {
        let body = json!({
            "id": "resp_queued",
            "object": "response",
            "model": "gpt-source",
            "status": "queued",
            "output": [{
                "type": "message",
                "id": "msg_queued",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "not final"
                }]
            }]
        });

        let error = convert_response_pure("openai:responses", "openai:chat", &body)
            .expect_err("non-terminal Responses status should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. } if field == "status"
        ));
    }

    #[test]
    fn pure_openai_chat_response_same_format_preserves_unknown_finish_reason() {
        let body = json!({
            "id": "chatcmpl_unknown_finish",
            "object": "chat.completion",
            "model": "gpt-source",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello"
                },
                "finish_reason": "future_reason"
            }]
        });

        let converted = convert_response_pure("openai:chat", "openai:chat", &body)
            .expect("same-format response roundtrip should preserve unknown finish reason")
            .value;

        assert_eq!(converted["choices"][0]["finish_reason"], "future_reason");
    }

    #[test]
    fn pure_openai_responses_response_same_format_preserves_incomplete_status() {
        let body = json!({
            "id": "resp_incomplete",
            "object": "response",
            "model": "gpt-source",
            "status": "incomplete",
            "incomplete_details": {
                "reason": "max_output_tokens"
            },
            "output": [{
                "type": "message",
                "id": "msg_incomplete",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "partial"
                }]
            }]
        });

        let converted = convert_response_pure("openai:responses", "openai:responses", &body)
            .expect("same-format response roundtrip should preserve Responses status")
            .value;

        assert_eq!(converted["status"], "incomplete");
        assert_eq!(
            converted["incomplete_details"]["reason"],
            "max_output_tokens"
        );
    }

    #[test]
    fn pure_openai_responses_response_same_format_preserves_raw_output_items() {
        let body = json!({
            "id": "resp_raw_items",
            "object": "response",
            "model": "gpt-source",
            "status": "completed",
            "output": [
                {
                    "type": "file_search_call",
                    "id": "fs_123",
                    "status": "completed",
                    "queries": ["rust"],
                    "results": [{"file_id": "file_123", "text": "Rust"}]
                },
                {
                    "type": "code_interpreter_call",
                    "id": "ci_123",
                    "status": "completed",
                    "code": "print('hi')",
                    "outputs": []
                },
                {
                    "type": "message",
                    "id": "msg_123",
                    "status": "completed",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "done"}]
                }
            ]
        });

        let converted = convert_response_pure("openai:responses", "openai:responses", &body)
            .expect("same-format Responses response should preserve raw output items")
            .value;

        assert_eq!(converted["output"], body["output"]);
    }

    #[test]
    fn pure_openai_responses_response_preserves_ptc_and_blocks_lossy_targets() {
        let body = json!({
            "id": "resp_ptc",
            "object": "response",
            "model": "gpt-5.6-sol",
            "status": "completed",
            "output": [
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "status": "completed",
                    "name": "lookup",
                    "arguments": "{\"id\":7}",
                    "caller": {"type": "program", "id": "program_1"}
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "found",
                    "caller": {"type": "program", "id": "program_1"}
                }
            ]
        });

        let same_format = convert_response_pure("openai:responses", "openai:responses", &body)
            .expect("same-format response should preserve PTC output items")
            .value;
        assert_eq!(same_format["output"], body["output"]);

        for target in ["openai:chat", "claude:messages", "gemini:generate_content"] {
            let error = convert_response_pure("openai:responses", target, &body)
                .expect_err("target response format cannot preserve PTC caller provenance");
            assert!(matches!(
                error,
                super::FormatError::LossyConversionBlocked { ref field, .. }
                    if field.ends_with(".caller")
            ));
        }
    }

    #[test]
    fn chat_backed_function_call_metadata_is_only_droppable_for_chat_targets() {
        let body = json!({
            "id": "resp_chat_metadata",
            "object": "response",
            "model": "gpt-5",
            "status": "completed",
            "output": [{
                "type": "function_call",
                "id": "fc_chat_metadata",
                "call_id": "call_chat_metadata",
                "status": "completed",
                "name": "lookup",
                "arguments": "{}",
                "metadata": {"source": "chat"},
                "internal_chat_message_metadata_passthrough": {
                    "turn_id": "turn_123"
                }
            }]
        });

        let chat = convert_response_pure("openai:responses", "openai:chat", &body)
            .expect("Chat-backed metadata should be droppable for a Chat client")
            .value;
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_chat_metadata"
        );

        for target in ["claude:messages", "gemini:generate_content"] {
            let error = convert_response_pure("openai:responses", target, &body)
                .expect_err("non-Chat targets must reject Chat-private metadata");
            assert!(
                matches!(
                    error,
                    super::FormatError::LossyConversionBlocked { ref field, .. }
                        if field.ends_with(".metadata")
                            || field.contains("internal_chat_message_metadata_passthrough")
                ),
                "{target}: {error:?}"
            );
        }

        let mut unpaired = body;
        unpaired["output"][0]
            .as_object_mut()
            .expect("function call should be an object")
            .remove("internal_chat_message_metadata_passthrough");
        let error = convert_response_pure("openai:responses", "openai:chat", &unpaired)
            .expect_err("generic metadata without the Chat-private marker must be rejected");
        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field.ends_with(".metadata")
        ));
    }

    #[test]
    fn pure_openai_responses_response_cross_format_blocks_raw_output_items() {
        let body = json!({
            "id": "resp_raw_items",
            "object": "response",
            "model": "gpt-source",
            "status": "completed",
            "output": [{
                "type": "mcp_call",
                "id": "mcp_123",
                "status": "completed",
                "name": "lookup"
            }]
        });

        let error = convert_response_pure("openai:responses", "openai:chat", &body)
            .expect_err("cross-format raw Responses output items should fail closed");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "output[].type"
        ));
    }

    #[test]
    fn pure_openai_responses_response_refusal_only_maps_to_openai_chat() {
        let body = json!({
            "id": "resp_refusal",
            "object": "response",
            "model": "gpt-5.6-sol",
            "status": "completed",
            "output": [{
                "type": "message",
                "id": "msg_refusal",
                "status": "completed",
                "role": "assistant",
                "content": [{"type": "refusal", "refusal": "cannot comply"}]
            }]
        });

        let chat = convert_response_pure("openai:responses", "openai:chat", &body)
            .expect("OpenAI Chat has a refusal response field")
            .value;
        assert_eq!(chat["choices"][0]["message"]["refusal"], "cannot comply");

        for target in ["claude:messages", "gemini:generate_content"] {
            let error = convert_response_pure("openai:responses", target, &body)
                .expect_err("target response format cannot preserve refusal content");
            assert!(matches!(
                error,
                super::FormatError::LossyConversionBlocked { ref field, .. }
                    if field == "output[].type"
            ));
        }
    }

    #[test]
    fn pure_openai_responses_response_blocks_unknown_message_item_fields_cross_format() {
        let body = json!({
            "id": "resp_message_extension",
            "object": "response",
            "model": "gpt-5.6-sol",
            "status": "completed",
            "output": [{
                "type": "message",
                "id": "msg_extension",
                "status": "completed",
                "role": "assistant",
                "future_message_option": {"mode": "verified"},
                "content": [{"type": "output_text", "text": "done"}]
            }]
        });

        let same_format = convert_response_pure("openai:responses", "openai:responses", &body)
            .expect("same-format response should preserve message item fields")
            .value;
        assert_eq!(same_format["output"], body["output"]);

        for target in ["openai:chat", "claude:messages", "gemini:generate_content"] {
            let error = convert_response_pure("openai:responses", target, &body)
                .expect_err("target response format cannot preserve message item extensions");
            assert!(matches!(
                error,
                super::FormatError::LossyConversionBlocked { ref field, .. }
                    if field == "output[0].future_message_option"
            ));
        }
    }

    #[test]
    fn pure_openai_responses_response_preserves_usage_and_audits_unknown_details() {
        let body = json!({
            "id": "resp_usage_extension",
            "object": "response",
            "model": "gpt-5.6-sol",
            "status": "completed",
            "output": [],
            "usage": {
                "input_tokens": 120,
                "output_tokens": 30,
                "total_tokens": 150,
                "input_tokens_details": {
                    "cached_tokens": 40,
                    "future_cache_tokens": 3
                },
                "output_tokens_details": {
                    "reasoning_tokens": 20,
                    "future_reasoning_tokens": 2
                },
                "future_usage_option": "metered"
            }
        });

        let same_format = convert_response_pure("openai:responses", "openai:responses", &body)
            .expect("same-format response should preserve usage extensions")
            .value;
        assert_eq!(same_format["usage"], body["usage"]);

        for target in ["openai:chat", "claude:messages", "gemini:generate_content"] {
            let error = convert_response_pure("openai:responses", target, &body)
                .expect_err("target response format cannot preserve unknown usage details");
            assert!(matches!(
                error,
                super::FormatError::LossyConversionBlocked { ref field, .. }
                    if field == "usage.input_tokens_details.future_cache_tokens"
            ));
        }
    }

    #[test]
    fn pure_claude_response_same_format_preserves_unknown_stop_reason() {
        let body = json!({
            "id": "msg_unknown_stop",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet",
            "content": [{
                "type": "text",
                "text": "hello"
            }],
            "stop_reason": "future_reason",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 1,
                "output_tokens": 1
            }
        });

        let converted = convert_response_pure("claude:messages", "claude:messages", &body)
            .expect("same-format response roundtrip should preserve unknown stop reason")
            .value;

        assert_eq!(converted["stop_reason"], "future_reason");
        assert_eq!(converted["stop_sequence"], json!(null));
    }

    #[test]
    fn pure_gemini_response_same_format_preserves_unknown_finish_reason() {
        let body = json!({
            "responseId": "resp_unknown_finish",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "index": 0,
                "content": {
                    "role": "model",
                    "parts": [{"text": "hello"}]
                },
                "finishReason": "FUTURE_REASON"
            }]
        });

        let converted =
            convert_response_pure("gemini:generate_content", "gemini:generate_content", &body)
                .expect("same-format response roundtrip should preserve unknown finish reason")
                .value;

        assert_eq!(converted["candidates"][0]["finishReason"], "FUTURE_REASON");
    }

    #[test]
    fn pure_claude_same_format_roundtrip_preserves_raw_tool_input_schema() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": 64,
            "tools": [{
                "name": "lookup",
                "description": "Lookup",
                "input_schema": {"type": "object"}
            }]
        });

        let converted = convert_request_pure("claude:messages", "claude:messages", &body)
            .expect("same-format roundtrip should succeed")
            .value;

        assert_eq!(
            converted["tools"][0]["input_schema"],
            json!({"type": "object"})
        );
    }

    #[test]
    fn pure_gemini_same_format_roundtrip_preserves_raw_function_parameters() {
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
            "tools": [{
                "functionDeclarations": [{
                    "name": "lookup",
                    "description": "Lookup",
                    "parameters": {"type": "object"}
                }]
            }]
        });

        let converted =
            convert_request_pure("gemini:generate_content", "gemini:generate_content", &body)
                .expect("same-format roundtrip should succeed")
                .value;

        assert_eq!(
            converted["tools"][0]["functionDeclarations"][0]["parameters"],
            json!({"type": "object"})
        );
    }

    #[test]
    fn legacy_openai_responses_to_chat_does_not_leak_responses_only_extensions() {
        let body = json!({
            "model": "gpt-source",
            "input": [{"role": "user", "content": "hello"}],
            "include": ["reasoning.encrypted_content"],
            "previous_response_id": "resp_123",
            "stream": true
        });

        let converted = convert_request(
            "openai:responses",
            "openai:chat",
            &body,
            &FormatContext::default(),
        )
        .expect("legacy conversion should still emit a chat body");

        assert!(converted.get("stream").is_none());
        assert!(converted.get("include").is_none());
        assert!(converted.get("previous_response_id").is_none());
    }

    #[test]
    fn converts_openai_embedding_to_jina_without_chat_fields() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": ["alpha", "beta"],
            "dimensions": 2
        });
        let ctx = FormatContext::default().with_mapped_model("jina-embeddings-v3");

        let converted = convert_request("openai:embedding", "jina:embedding", &body, &ctx)
            .expect("embedding request conversion should succeed");

        assert_eq!(converted["model"], "jina-embeddings-v3");
        assert_eq!(converted["task"], "text-matching");
        assert_eq!(converted["input"], json!(["alpha", "beta"]));
        assert!(converted.get("messages").is_none());
    }

    #[test]
    fn converts_openai_embedding_to_gemini_and_doubao_payload_shapes() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": ["alpha", "beta"],
            "dimensions": 2
        });

        let gemini = convert_request(
            "openai:embedding",
            "gemini:embedding",
            &body,
            &FormatContext::default().with_mapped_model("gemini-embedding-001"),
        )
        .expect("gemini embedding conversion should succeed");
        assert!(gemini.get("model").is_none());
        assert_eq!(
            gemini["requests"][0]["model"],
            "models/gemini-embedding-001"
        );
        assert_eq!(
            gemini["requests"][0]["content"]["parts"][0]["text"],
            "alpha"
        );
        assert_eq!(gemini["requests"][0]["outputDimensionality"], 2);
        assert!(gemini.get("messages").is_none());

        let doubao = convert_request(
            "openai:embedding",
            "doubao:embedding",
            &body,
            &FormatContext::default().with_mapped_model("doubao-embedding-text-240515"),
        )
        .expect("doubao embedding conversion should succeed");
        assert_eq!(doubao["model"], "doubao-embedding-text-240515");
        assert_eq!(doubao["input"], json!(["alpha", "beta"]));
        assert!(doubao.get("messages").is_none());
    }

    #[test]
    fn converts_openai_embedding_to_aliyun_multimodal_payload_shape() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": [
                {"text": "white running shoes"},
                {"image": "https://example.com/shoe.png"},
                {"multi_images": ["https://example.com/a.png", "https://example.com/b.png"]}
            ],
            "dimensions": 1024,
            "parameters": {
                "enable_fusion": true,
                "res_level": 2,
                "max_video_frames": 64
            }
        });

        let converted = convert_request(
            "openai:embedding",
            "aliyun:multimodal_embedding",
            &body,
            &FormatContext::default().with_mapped_model("qwen3-vl-embedding"),
        )
        .expect("aliyun multimodal embedding conversion should succeed");

        assert_eq!(converted["model"], "qwen3-vl-embedding");
        assert_eq!(converted["input"]["contents"], body["input"]);
        assert_eq!(converted["parameters"]["dimension"], 1024);
        assert_eq!(converted["parameters"]["enable_fusion"], true);
        assert_eq!(converted["parameters"]["res_level"], 2);
        assert_eq!(converted["parameters"]["max_video_frames"], 64);
        assert!(converted.get("messages").is_none());
    }

    #[test]
    fn pure_embedding_conversion_parses_gemini_source_to_openai() {
        let body = json!({
            "model": "models/gemini-embedding-001",
            "content": {
                "parts": [{"text": "alpha"}]
            },
            "outputDimensionality": 768
        });

        let converted = convert_request_pure("gemini:embedding", "openai:embedding", &body)
            .expect("Gemini embedding source should parse")
            .value;

        assert_eq!(converted["model"], "models/gemini-embedding-001");
        assert_eq!(converted["input"], "alpha");
        assert_eq!(converted["dimensions"], 768);
    }

    #[test]
    fn pure_embedding_conversion_blocks_gemini_task_to_openai() {
        let body = json!({
            "model": "models/gemini-embedding-001",
            "content": {
                "parts": [{"text": "alpha"}]
            },
            "taskType": "RETRIEVAL_QUERY"
        });

        let error = convert_request_pure("gemini:embedding", "openai:embedding", &body)
            .expect_err("OpenAI embeddings have no task field");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. } if field == "task"
        ));
    }

    #[test]
    fn pure_embedding_conversion_parses_aliyun_text_source_to_openai() {
        let body = json!({
            "model": "qwen3-vl-embedding",
            "input": {
                "contents": [
                    {"text": "alpha"},
                    {"text": "beta"}
                ]
            },
            "parameters": {
                "dimension": 1024
            }
        });

        let converted =
            convert_request_pure("aliyun:multimodal_embedding", "openai:embedding", &body)
                .expect("Aliyun text embedding source should parse")
                .value;

        assert_eq!(converted["model"], "qwen3-vl-embedding");
        assert_eq!(converted["input"], json!(["alpha", "beta"]));
        assert_eq!(converted["dimensions"], 1024);
    }

    #[test]
    fn pure_embedding_conversion_blocks_token_input_to_gemini() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": [1, 2, 3]
        });

        let error = convert_request_pure("openai:embedding", "gemini:embedding", &body)
            .expect_err("Gemini cannot receive token-array embedding input");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. } if field == "input"
        ));
    }

    #[test]
    fn pure_embedding_conversion_blocks_openai_only_fields_to_doubao() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": "alpha",
            "encoding_format": "base64"
        });

        let error = convert_request_pure("openai:embedding", "doubao:embedding", &body)
            .expect_err("Doubao cannot carry OpenAI encoding_format");

        assert!(matches!(
            error,
            super::FormatError::LossyConversionBlocked { ref field, .. }
                if field == "encoding_format"
        ));
    }

    #[test]
    fn pure_embedding_conversion_blocks_unknown_provider_fields_cross_format() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": "alpha",
            "unknown_vendor_field": true
        });

        let error = convert_request_pure("openai:embedding", "jina:embedding", &body)
            .expect_err("unknown provider fields cannot be dropped cross-format");

        assert!(matches!(
            error,
            super::FormatError::UnsupportedField { ref field, .. }
                if field == "embedding.extensions"
        ));
    }

    #[test]
    fn pure_rerank_conversion_blocks_unknown_provider_fields_cross_format() {
        let body = json!({
            "model": "rerank-model",
            "query": "rust",
            "documents": ["rust book"],
            "unknown_vendor_field": true
        });

        let error = convert_request_pure("openai:rerank", "jina:rerank", &body)
            .expect_err("unknown rerank provider fields cannot be dropped cross-format");

        assert!(matches!(
            error,
            super::FormatError::UnsupportedField { ref field, .. }
                if field == "rerank.extensions"
        ));
    }

    #[test]
    fn aliyun_embedding_conversion_rejects_token_arrays() {
        let body = json!({
            "model": "text-embedding-3-small",
            "input": [1, 2, 3]
        });

        assert!(convert_request(
            "openai:embedding",
            "aliyun:multimodal_embedding",
            &body,
            &FormatContext::default().with_mapped_model("qwen3-vl-embedding"),
        )
        .is_err());
    }

    #[test]
    fn multimodal_embedding_conversion_is_aliyun_only() {
        let body = json!({
            "model": "qwen3-vl-embedding",
            "input": [
                {"text": "white running shoes"},
                {"image": "https://example.com/shoe.png"}
            ]
        });
        let ctx = FormatContext::default().with_mapped_model("qwen3-vl-embedding");

        assert!(convert_request("openai:embedding", "openai:embedding", &body, &ctx).is_err());
        assert!(convert_request("openai:embedding", "jina:embedding", &body, &ctx).is_err());
        assert!(convert_request("openai:embedding", "gemini:embedding", &body, &ctx).is_err());
        assert!(convert_request("openai:embedding", "doubao:embedding", &body, &ctx).is_err());
        assert!(convert_request(
            "openai:embedding",
            "aliyun:multimodal_embedding",
            &body,
            &ctx
        )
        .is_ok());
    }

    #[test]
    fn parses_aliyun_embedding_response_to_openai_shape() {
        let body = json!({
            "output": {
                "embeddings": [
                    {
                        "index": 0,
                        "embedding": [0.1, 0.2, 0.3],
                        "type": "fused"
                    }
                ]
            },
            "usage": {
                "input_tokens": 432,
                "input_tokens_details": {
                    "image_tokens": 402,
                    "text_tokens": 30
                },
                "output_tokens": 1,
                "total_tokens": 433
            },
            "request_id": "aliyun-request-1"
        });

        let canonical =
            crate::protocol::canonical::from_embedding_to_canonical_response(&body, "aliyun")
                .expect("aliyun embedding response should parse");
        let emitted =
            crate::protocol::canonical::canonical_to_embedding_response(&canonical, "openai")
                .expect("openai embedding response should emit");

        assert_eq!(emitted["request_id"], "aliyun-request-1");
        assert_eq!(emitted["data"][0]["embedding"], json!([0.1, 0.2, 0.3]));
        assert_eq!(emitted["data"][0]["type"], "fused");
        assert_eq!(emitted["usage"]["prompt_tokens"], 432);
        assert_eq!(emitted["usage"]["completion_tokens"], 1);
        assert_eq!(emitted["usage"]["total_tokens"], 433);
    }

    #[test]
    fn embedding_registry_parses_gemini_and_doubao_sources() {
        let gemini_body = json!({
            "model": "gemini-embedding-001",
            "content": {"parts": [{"text": "alpha"}]}
        });
        let doubao_body = json!({
            "model": "doubao-embedding",
            "input": ["alpha"]
        });
        let ctx = FormatContext::default();

        let gemini = convert_request("gemini:embedding", "openai:embedding", &gemini_body, &ctx)
            .expect("Gemini embedding source should parse");
        assert_eq!(gemini["model"], "gemini-embedding-001");
        assert_eq!(gemini["input"], "alpha");

        let doubao = convert_request("doubao:embedding", "openai:embedding", &doubao_body, &ctx)
            .expect("Doubao embedding source should parse");
        assert_eq!(doubao["model"], "doubao-embedding");
        assert_eq!(doubao["input"], json!(["alpha"]));
    }

    #[test]
    fn embedding_registry_rejects_chat_payload_for_embedding_format() {
        let body = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}]
        });
        let ctx = FormatContext::default();

        assert!(convert_request("openai:embedding", "jina:embedding", &body, &ctx).is_err());
    }

    #[test]
    fn converts_openai_rerank_to_jina_without_chat_fields() {
        let body = json!({
            "model": "rerank-source",
            "query": "best document",
            "documents": ["alpha", {"text": "beta"}],
            "top_n": 1,
            "return_documents": true
        });
        let ctx = FormatContext::default().with_mapped_model("jina-reranker-v2-base-multilingual");

        let converted = convert_request("openai:rerank", "jina:rerank", &body, &ctx)
            .expect("rerank request conversion should succeed");

        assert_eq!(converted["model"], "jina-reranker-v2-base-multilingual");
        assert_eq!(converted["query"], "best document");
        assert_eq!(converted["documents"], json!(["alpha", {"text": "beta"}]));
        assert_eq!(converted["top_n"], 1);
        assert_eq!(converted["return_documents"], true);
        assert!(converted.get("messages").is_none());
    }

    #[test]
    fn rerank_registry_rejects_invalid_payloads() {
        let ctx = FormatContext::default();
        for body in [
            json!({"model": "rerank", "documents": ["alpha"]}),
            json!({"model": "rerank", "query": "q", "documents": []}),
            json!({"model": "rerank", "query": "q", "documents": [""]}),
            json!({"model": "rerank", "query": "q", "documents": ["alpha"], "top_n": 0}),
        ] {
            assert!(convert_request("openai:rerank", "jina:rerank", &body, &ctx).is_err());
        }
    }

    #[test]
    fn field_coverage_matrix_covers_all_documented_provider_schema_fields() {
        let definitions = include_str!("../../../../../docs/api/provider-interface-definitions.md");
        let matrix = include_str!("../../../../../docs/api/format-field-coverage-matrix.md");
        let documented = parse_documented_schema_fields(definitions);
        let covered = parse_field_coverage_matrix_fields(matrix);

        let missing = documented
            .difference(&covered)
            .take(20)
            .cloned()
            .collect::<Vec<_>>();
        let extra = covered
            .difference(&documented)
            .take(20)
            .cloned()
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "field coverage matrix is missing documented schema fields: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "field coverage matrix contains fields not present in provider definitions: {extra:?}"
        );
        assert!(
            matrix.contains(&format!(
                "Total covered schema fields: {}.",
                documented.len()
            )),
            "field coverage matrix total must match provider-interface-definitions.md"
        );
        assert_field_coverage_statuses_are_explicit(matrix);
    }

    #[test]
    fn request_root_field_whitelists_cover_documented_generation_request_schemas() {
        let definitions = include_str!("../../../../../docs/api/provider-interface-definitions.md");
        let documented = parse_documented_schema_fields(definitions);

        for (format, provider, schema) in [
            (
                FormatId::OpenAiChat,
                "OpenAI",
                "CreateChatCompletionRequest",
            ),
            (FormatId::OpenAiResponses, "OpenAI", "CreateResponse"),
            (
                FormatId::OpenAiResponsesCompact,
                "OpenAI",
                "CompactResponseMethodPublicBody",
            ),
            (
                FormatId::ClaudeMessages,
                "Claude",
                "MessageCreateParamsBase",
            ),
            (
                FormatId::GeminiGenerateContent,
                "Gemini",
                "GenerateContentRequest",
            ),
        ] {
            let missing = documented
                .iter()
                .filter(|field| field.provider == provider && field.schema == schema)
                .filter(|field| {
                    !super::standard_request_root_field_is_audited(format, &field.field)
                })
                .map(|field| field.field.clone())
                .collect::<Vec<_>>();
            assert!(
                missing.is_empty(),
                "{provider} `{schema}` has root fields missing from runtime audit whitelist: {missing:?}"
            );
        }
    }

    #[test]
    fn format_conversion_audit_has_no_unresolved_field_coverage_markers() {
        let audit = include_str!("../../../../../docs/api/format-conversion-audit.md");
        for forbidden in [
            "strict audit pending",
            "Nested per-field",
            "field-by-field decision pending",
            "is still pending",
            "coverage exists, but strict audit is pending",
        ] {
            assert!(
                !audit.contains(forbidden),
                "format conversion audit still contains unresolved marker: {forbidden}"
            );
        }
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
                "registry should dispatch through formats::<provider>::<surface> adapters, found {forbidden}"
            );
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct DocumentedSchemaField {
        provider: String,
        schema: String,
        field: String,
    }

    fn parse_documented_schema_fields(
        source: &str,
    ) -> std::collections::BTreeSet<DocumentedSchemaField> {
        let mut fields = std::collections::BTreeSet::new();
        let mut provider: Option<&str> = None;
        let mut schema: Option<String> = None;

        for line in source.lines() {
            if line.starts_with("## ") {
                provider = if line.contains("OpenAI Schema") {
                    Some("OpenAI")
                } else if line.contains("Claude / Anthropic TypeScript") {
                    Some("Claude")
                } else if line.contains("Gemini Schema") {
                    Some("Gemini")
                } else {
                    None
                };
                schema = None;
                continue;
            }
            let Some(active_provider) = provider else {
                continue;
            };
            if let Some(schema_name) = markdown_code_heading(line) {
                schema = Some(schema_name);
                continue;
            }
            let Some(active_schema) = schema.as_ref() else {
                continue;
            };
            if !line.starts_with("| `") {
                continue;
            }
            let cells = split_markdown_row(line);
            if cells.len() < 4 || !matches!(cells[2].as_str(), "是" | "否") {
                continue;
            }
            fields.insert(DocumentedSchemaField {
                provider: active_provider.to_string(),
                schema: active_schema.clone(),
                field: strip_markdown_code(&cells[1]),
            });
        }

        fields
    }

    fn parse_field_coverage_matrix_fields(
        matrix: &str,
    ) -> std::collections::BTreeSet<DocumentedSchemaField> {
        let mut fields = std::collections::BTreeSet::new();
        for line in matrix.lines() {
            if !line.starts_with("| ") {
                continue;
            }
            let cells = split_markdown_row(line);
            if cells.len() < 10 || !matches!(cells[1].as_str(), "OpenAI" | "Claude" | "Gemini") {
                continue;
            }
            fields.insert(DocumentedSchemaField {
                provider: cells[1].to_string(),
                schema: strip_markdown_code(&cells[2]),
                field: strip_markdown_code(&cells[3]),
            });
        }
        fields
    }

    fn assert_field_coverage_statuses_are_explicit(matrix: &str) {
        const VALID_STATUSES: &[&str] = &[
            "native",
            "mapped",
            "mapped/lossy-blocked",
            "extension-preserved",
            "unaudited",
            "unsupported",
            "invalid-enum",
            "lossy-blocked",
            "not-in-conversion-surface",
        ];

        for line in matrix.lines() {
            if !line.starts_with("| ") {
                continue;
            }
            let cells = split_markdown_row(line);
            if cells.len() < 10 || !matches!(cells[1].as_str(), "OpenAI" | "Claude" | "Gemini") {
                continue;
            }
            for index in [7, 8, 9] {
                assert!(
                    VALID_STATUSES.contains(&cells[index].as_str()),
                    "field coverage matrix has invalid status `{}` in row `{line}`",
                    cells[index]
                );
            }
        }
    }

    fn markdown_code_heading(line: &str) -> Option<String> {
        line.strip_prefix("### `")
            .and_then(|rest| rest.split_once('`'))
            .map(|(value, _)| value.to_string())
    }

    fn strip_markdown_code(value: &str) -> String {
        value
            .trim()
            .strip_prefix('`')
            .and_then(|value| value.strip_suffix('`'))
            .unwrap_or_else(|| value.trim())
            .replace("\\|", "|")
    }

    fn split_markdown_row(line: &str) -> Vec<String> {
        let mut cells = Vec::new();
        let mut current = String::new();
        let mut escaped = false;
        for ch in line.chars() {
            if ch == '|' && !escaped {
                cells.push(current.trim().to_string());
                current.clear();
            } else {
                current.push(ch);
            }
            escaped = ch == '\\' && !escaped;
            if escaped && ch != '\\' {
                escaped = false;
            }
        }
        cells.push(current.trim().to_string());
        cells
    }
}
