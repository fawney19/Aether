use serde_json::{json, Map, Value};

use crate::{
    formats::context::FormatContext,
    protocol::canonical::{
        canonical_extension_object_mut, canonical_message_to_openai_chat_messages,
        canonical_response_format_to_openai, canonical_tool_choice_to_openai,
        canonical_tool_is_openai_custom, canonical_tool_to_openai, is_claude_tool_result,
        namespace_extension_object, openai_content_text, openai_extensions,
        openai_generation_config, openai_message_content_blocks,
        openai_response_format_to_canonical, openai_responses_extension, openai_role_to_canonical,
        openai_tool_choice_raw_to_chat, openai_tool_choice_to_canonical, openai_tools_to_canonical,
        write_openai_generation_config, CanonicalContentBlock, CanonicalInstruction,
        CanonicalRequest, CanonicalRole, CanonicalThinkingConfig, CanonicalToolChoice,
        CanonicalToolDefinition, OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_raw(body)
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    if canonical_request_has_unrepresentable_claude_tool_result_for_openai_chat(request) {
        return None;
    }
    let mut body = to_raw(request);
    force_stream_options(&mut body, ctx.upstream_is_stream);
    Some(body)
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalRequest> {
    let request = body_json.as_object()?;
    let mut canonical = CanonicalRequest {
        model: request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ..CanonicalRequest::default()
    };

    if let Some(messages) = request.get("messages").and_then(Value::as_array) {
        for message in messages {
            let message_object = message.as_object()?;
            let role = openai_role_to_canonical(
                message_object
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            if matches!(role, CanonicalRole::System | CanonicalRole::Developer) {
                let text = openai_content_text(message_object.get("content"));
                canonical.instructions.push(CanonicalInstruction {
                    role,
                    text: text.clone(),
                    extensions: openai_extensions(message_object, &["role", "content"]),
                });
                if !text.trim().is_empty() {
                    canonical.system = Some(match canonical.system.take() {
                        Some(existing) if !existing.trim().is_empty() => {
                            format!("{existing}\n\n{text}")
                        }
                        _ => text,
                    });
                }
                continue;
            }
            canonical
                .messages
                .push(crate::protocol::canonical::CanonicalMessage {
                    role,
                    content: openai_message_content_blocks(message_object)?,
                    extensions: openai_extensions(
                        message_object,
                        &["role", "content", "tool_calls", "tool_call_id"],
                    ),
                });
        }
    }

    canonical.generation = openai_generation_config(request);
    canonical.tools = openai_tools_to_canonical(request.get("tools"))?;
    canonical.tool_choice = openai_tool_choice_to_canonical(request.get("tool_choice"));
    canonical.parallel_tool_calls = request.get("parallel_tool_calls").and_then(Value::as_bool);
    canonical.metadata = request.get("metadata").cloned();
    canonical.response_format = openai_response_format_to_canonical(request.get("response_format"));
    if let Some(reasoning_effort) = request.get("reasoning_effort").and_then(Value::as_str) {
        let mut extensions = std::collections::BTreeMap::new();
        extensions.insert(
            "openai".to_string(),
            json!({ "reasoning_effort": reasoning_effort }),
        );
        canonical.thinking = Some(CanonicalThinkingConfig {
            enabled: true,
            budget_tokens: None,
            extensions,
        });
    }
    canonical.extensions = openai_extensions(
        request,
        &[
            "model",
            "messages",
            "max_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "top_k",
            "stop",
            "tools",
            "parallel_tool_calls",
            "metadata",
            "response_format",
            "reasoning_effort",
            "n",
            "presence_penalty",
            "frequency_penalty",
            "seed",
            "logprobs",
            "top_logprobs",
        ],
    );
    if canonical.tool_choice.is_some() {
        remove_tool_choice_extension(&mut canonical.extensions, "openai");
    }
    if let Some(verbosity) = request.get("verbosity").cloned() {
        canonical_extension_object_mut(
            &mut canonical.extensions,
            OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        )
        .insert("verbosity".to_string(), verbosity);
    }
    Some(canonical)
}

pub fn to_raw(canonical: &CanonicalRequest) -> Value {
    let mut output = serde_json::Map::new();
    if !canonical.model.trim().is_empty() {
        output.insert("model".to_string(), Value::String(canonical.model.clone()));
    }

    let mut messages = Vec::new();
    for instruction in &canonical.instructions {
        let role = match instruction.role {
            CanonicalRole::Developer => "system",
            _ => "system",
        };
        if !instruction.text.trim().is_empty() {
            messages.push(json!({
                "role": role,
                "content": instruction.text,
            }));
        }
    }
    for message in &canonical.messages {
        messages.extend(canonical_message_to_openai_chat_messages(message));
    }
    output.insert("messages".to_string(), Value::Array(messages));

    write_openai_generation_config(&mut output, &canonical.generation);
    if !canonical.tools.is_empty() {
        output.insert(
            "tools".to_string(),
            Value::Array(
                canonical
                    .tools
                    .iter()
                    .map(canonical_tool_to_openai)
                    .collect(),
            ),
        );
    }
    if let Some(tool_choice) = canonical_tool_choice_to_openai_for_request(canonical) {
        output.insert("tool_choice".to_string(), tool_choice);
    }
    if let Some(value) = canonical.parallel_tool_calls {
        output.insert("parallel_tool_calls".to_string(), Value::Bool(value));
    }
    if let Some(metadata) = canonical.metadata.clone() {
        output.insert("metadata".to_string(), metadata);
    }
    if let Some(response_format) = &canonical.response_format {
        output.insert(
            "response_format".to_string(),
            canonical_response_format_to_openai(response_format),
        );
    }
    if let Some(thinking) = &canonical.thinking {
        if let Some(reasoning_effort) = thinking
            .extensions
            .get("openai")
            .and_then(|value| value.get("reasoning_effort"))
            .and_then(Value::as_str)
            .or_else(|| {
                openai_responses_extension(&thinking.extensions)
                    .and_then(|value| value.get("effort"))
                    .and_then(Value::as_str)
            })
            .and_then(openai_chat_reasoning_effort)
        {
            output.insert(
                "reasoning_effort".to_string(),
                Value::String(reasoning_effort.to_string()),
            );
        }
    }
    output.extend(namespace_extension_object(
        &canonical.extensions,
        "openai",
        &output,
    ));
    output.extend(chat_compatible_openai_responses_extension_object(
        &canonical.extensions,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        &output,
    ));
    output.extend(chat_compatible_openai_responses_extension_object(
        &canonical.extensions,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
        &output,
    ));
    Value::Object(output)
}

fn canonical_tool_choice_to_openai_for_request(canonical: &CanonicalRequest) -> Option<Value> {
    canonical
        .tool_choice
        .as_ref()
        .map(|tool_choice| canonical_tool_choice_to_openai_for_tools(tool_choice, &canonical.tools))
        .or_else(|| raw_tool_choice_extension(canonical).map(openai_tool_choice_raw_to_chat))
}

fn canonical_tool_choice_to_openai_for_tools(
    choice: &CanonicalToolChoice,
    tools: &[CanonicalToolDefinition],
) -> Value {
    match choice {
        CanonicalToolChoice::Tool { name }
            if tools
                .iter()
                .any(|tool| tool.name == *name && canonical_tool_is_openai_custom(tool)) =>
        {
            json!({
                "type": "custom",
                "custom": { "name": name },
            })
        }
        _ => canonical_tool_choice_to_openai(choice),
    }
}

fn raw_tool_choice_extension(canonical: &CanonicalRequest) -> Option<&Value> {
    canonical
        .extensions
        .get("openai")
        .and_then(|value| value.get("tool_choice"))
        .or_else(|| {
            openai_responses_extension(&canonical.extensions)
                .and_then(|value| value.get("tool_choice"))
        })
}

fn remove_tool_choice_extension(
    extensions: &mut std::collections::BTreeMap<String, Value>,
    namespace: &str,
) {
    let should_remove_namespace = extensions
        .get_mut(namespace)
        .and_then(Value::as_object_mut)
        .is_some_and(|object| {
            object.remove("tool_choice");
            object.is_empty()
        });
    if should_remove_namespace {
        extensions.remove(namespace);
    }
}

fn canonical_request_has_unrepresentable_claude_tool_result_for_openai_chat(
    request: &CanonicalRequest,
) -> bool {
    request.messages.iter().any(|message| {
        message.content.iter().any(|block| {
            let CanonicalContentBlock::ToolResult {
                output, extensions, ..
            } = block
            else {
                return false;
            };
            is_claude_tool_result(extensions)
                && output
                    .as_ref()
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        !claude_tool_result_parts_are_openai_chat_representable(parts)
                    })
        })
    })
}

pub(crate) fn claude_tool_result_parts_are_openai_chat_representable(parts: &[Value]) -> bool {
    parts
        .iter()
        .all(claude_tool_result_part_is_openai_chat_representable)
}

fn claude_tool_result_part_is_openai_chat_representable(part: &Value) -> bool {
    let Some(part_object) = part.as_object() else {
        return false;
    };
    match part_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => true,
        "image" => claude_image_block_is_openai_chat_representable(part_object),
        "document" | "file" => claude_document_block_is_openai_chat_representable(part_object),
        _ => false,
    }
}

fn claude_image_block_is_openai_chat_representable(block: &Map<String, Value>) -> bool {
    let Some(source) = block.get("source").and_then(Value::as_object) else {
        return false;
    };
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" => {
            non_empty_source_str(source, "media_type").is_some()
                && non_empty_source_str(source, "data").is_some()
        }
        "url" => non_empty_source_str(source, "url").is_some(),
        _ => false,
    }
}

fn claude_document_block_is_openai_chat_representable(block: &Map<String, Value>) -> bool {
    let Some(source) = block.get("source").and_then(Value::as_object) else {
        return false;
    };
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" => {
            non_empty_source_str(source, "media_type").is_some()
                && non_empty_source_str(source, "data").is_some()
        }
        "url" => non_empty_source_str(source, "url").is_some(),
        "text" => non_empty_source_str(source, "data").is_some(),
        _ => false,
    }
}

fn non_empty_source_str<'a>(source: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    source
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn openai_chat_reasoning_effort(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn chat_compatible_openai_responses_extension_object(
    extensions: &std::collections::BTreeMap<String, Value>,
    namespace: &str,
    existing: &Map<String, Value>,
) -> Map<String, Value> {
    namespace_extension_object(extensions, namespace, existing)
        .into_iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "verbosity"
                    | "store"
                    | "service_tier"
                    | "prompt_cache_key"
                    | "prompt_cache_options"
                    | "prompt_cache_retention"
                    | "safety_identifier"
                    | "user"
            )
        })
        .collect()
}

fn force_stream_options(body: &mut Value, upstream_is_stream: bool) {
    if !upstream_is_stream {
        return;
    }
    let Some(object) = body.as_object_mut() else {
        return;
    };
    object.insert("stream".to_string(), Value::Bool(true));
    match object.get_mut("stream_options") {
        Some(Value::Object(stream_options)) => {
            stream_options.insert("include_usage".to_string(), Value::Bool(true));
        }
        _ => {
            object.insert(
                "stream_options".to_string(),
                json!({
                    "include_usage": true,
                }),
            );
        }
    }
}
