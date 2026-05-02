use serde_json::{json, Map, Value};

use crate::{
    protocol::canonical::{
        canonical_response_format_to_openai, canonicalize_tool_arguments, media_data_or_url,
        namespace_extension_object, openai_content_text, openai_extensions,
        openai_response_format_to_canonical, openai_responses_extension,
        openai_responses_generation_config, openai_responses_input_to_canonical_messages,
        openai_responses_tool_choice_to_canonical, openai_responses_tools_to_canonical,
        CanonicalContentBlock, CanonicalInstruction, CanonicalRequest, CanonicalRole,
        CanonicalThinkingConfig, CanonicalToolChoice, CanonicalToolDefinition,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE, OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
    protocol::context::FormatContext,
    request::openai::map_thinking_budget_to_openai_reasoning_effort,
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_raw(body)
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    to_raw(
        request,
        ctx.mapped_model_or(request.model.as_str()),
        ctx.upstream_is_stream,
        false,
    )
}

pub fn to_compact(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    to_raw(
        request,
        ctx.mapped_model_or(request.model.as_str()),
        false,
        true,
    )
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

    if let Some(instructions) = request.get("instructions") {
        let text = openai_content_text(Some(instructions));
        if !text.trim().is_empty() {
            canonical.system = Some(text.clone());
            canonical.instructions.push(CanonicalInstruction {
                role: CanonicalRole::System,
                text,
                extensions: std::collections::BTreeMap::new(),
            });
        }
    }
    canonical.messages = openai_responses_input_to_canonical_messages(request.get("input"))?;
    canonical.generation = openai_responses_generation_config(request);
    canonical.tools = openai_responses_tools_to_canonical(request.get("tools"))?;
    canonical.tool_choice = openai_responses_tool_choice_to_canonical(request.get("tool_choice"));
    canonical.parallel_tool_calls = request.get("parallel_tool_calls").and_then(Value::as_bool);
    canonical.metadata = request.get("metadata").cloned();
    canonical.response_format = request
        .get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("format"))
        .and_then(|format| openai_response_format_to_canonical(Some(format)));
    if let Some(reasoning) = request.get("reasoning").and_then(Value::as_object) {
        let mut extensions = std::collections::BTreeMap::new();
        extensions.insert(
            OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
            Value::Object(reasoning.clone()),
        );
        canonical.thinking = Some(CanonicalThinkingConfig {
            enabled: true,
            budget_tokens: reasoning.get("budget_tokens").and_then(Value::as_u64),
            extensions,
        });
    }
    canonical.extensions = openai_extensions(
        request,
        &[
            "model",
            "instructions",
            "input",
            "max_output_tokens",
            "temperature",
            "top_p",
            "metadata",
            "tools",
            "tool_choice",
            "parallel_tool_calls",
            "text",
            "reasoning",
        ],
    );
    if let Some(raw) = canonical.extensions.remove("openai") {
        canonical
            .extensions
            .insert(OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(), raw);
    }
    if let Some(verbosity) = request
        .get("text")
        .and_then(Value::as_object)
        .and_then(|text| text.get("verbosity"))
        .cloned()
    {
        let entry = canonical
            .extensions
            .entry(OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Some(object) = entry.as_object_mut() {
            object.insert("verbosity".to_string(), verbosity);
        }
    }
    Some(canonical)
}

pub fn to_raw(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
    compact: bool,
) -> Option<Value> {
    let mut output = Map::new();
    output.insert("model".to_string(), Value::String(mapped_model.to_string()));

    if let Some(instructions) = canonical_instructions_to_responses(canonical) {
        output.insert("instructions".to_string(), instructions);
    }
    output.insert(
        "input".to_string(),
        Value::Array(canonical_messages_to_responses_input(canonical)?),
    );

    if upstream_is_stream && !compact {
        output.insert("stream".to_string(), Value::Bool(true));
    }
    if let Some(max_tokens) = canonical.generation.max_tokens {
        output.insert("max_output_tokens".to_string(), Value::from(max_tokens));
    }
    insert_number(&mut output, "temperature", canonical.generation.temperature);
    insert_number(&mut output, "top_p", canonical.generation.top_p);
    if let Some(top_logprobs) = canonical.generation.top_logprobs {
        output.insert("top_logprobs".to_string(), Value::from(top_logprobs));
    }
    if let Some(value) = canonical.parallel_tool_calls {
        output.insert("parallel_tool_calls".to_string(), Value::Bool(value));
    }
    if let Some(metadata) = canonical.metadata.clone() {
        output.insert("metadata".to_string(), metadata);
    }
    if let Some(text_config) = canonical_text_config_to_responses(canonical) {
        output.insert("text".to_string(), text_config);
    }
    if !canonical.tools.is_empty() {
        output.insert(
            "tools".to_string(),
            Value::Array(canonical_tools_to_responses(canonical)),
        );
    }
    if let Some(tool_choice) = canonical.tool_choice.as_ref() {
        output.insert(
            "tool_choice".to_string(),
            canonical_tool_choice_to_responses(tool_choice),
        );
    }
    if let Some(reasoning) = canonical
        .thinking
        .as_ref()
        .and_then(reasoning_config_to_responses)
    {
        output.insert("reasoning".to_string(), reasoning);
    }

    output.extend(namespace_extension_object(
        &canonical.extensions,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        &output,
    ));
    output.extend(namespace_extension_object(
        &canonical.extensions,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
        &output,
    ));
    output.remove("verbosity");
    Some(Value::Object(output))
}

fn canonical_instructions_to_responses(canonical: &CanonicalRequest) -> Option<Value> {
    let text = canonical
        .instructions
        .iter()
        .map(|instruction| instruction.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !text.trim().is_empty() {
        return Some(Value::String(text));
    }
    canonical
        .system
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .map(Value::String)
}

fn canonical_messages_to_responses_input(canonical: &CanonicalRequest) -> Option<Vec<Value>> {
    let mut input = Vec::new();
    for message in &canonical.messages {
        let role = match message.role {
            CanonicalRole::Assistant => "assistant",
            CanonicalRole::Tool | CanonicalRole::User | CanonicalRole::Unknown => "user",
            CanonicalRole::System | CanonicalRole::Developer => continue,
        };
        let mut content = Vec::new();
        for block in &message.content {
            match block {
                CanonicalContentBlock::ToolUse {
                    id,
                    name,
                    input: arguments,
                    ..
                } => {
                    flush_responses_message(&mut input, role, &mut content);
                    input.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": canonicalize_tool_arguments(arguments),
                    }));
                }
                CanonicalContentBlock::ToolResult {
                    tool_use_id,
                    output,
                    content_text,
                    ..
                } => {
                    flush_responses_message(&mut input, role, &mut content);
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": tool_use_id,
                        "output": responses_tool_result_output(output.as_ref(), content_text.as_deref()),
                    }));
                }
                CanonicalContentBlock::Thinking { .. } => {}
                other => {
                    if let Some(part) = canonical_block_to_responses_input_part(other, role) {
                        content.push(part);
                    }
                }
            }
        }
        flush_responses_message(&mut input, role, &mut content);
    }
    Some(input)
}

fn flush_responses_message(input: &mut Vec<Value>, role: &str, content: &mut Vec<Value>) {
    if content.is_empty() {
        return;
    }
    input.push(json!({
        "type": "message",
        "role": role,
        "content": std::mem::take(content),
    }));
}

fn canonical_block_to_responses_input_part(
    block: &CanonicalContentBlock,
    role: &str,
) -> Option<Value> {
    match block {
        CanonicalContentBlock::Text { text, .. } => {
            if text.is_empty() {
                return None;
            }
            Some(json!({
                "type": if role == "assistant" { "output_text" } else { "input_text" },
                "text": text,
            }))
        }
        CanonicalContentBlock::Image {
            data,
            url,
            media_type,
            detail,
            ..
        } => {
            let mut item = Map::new();
            item.insert(
                "type".to_string(),
                Value::String(if role == "assistant" {
                    "output_image".to_string()
                } else {
                    "input_image".to_string()
                }),
            );
            item.insert(
                "image_url".to_string(),
                Value::String(media_data_or_url(media_type, data, url)),
            );
            if let Some(detail) = detail {
                item.insert("detail".to_string(), Value::String(detail.clone()));
            }
            Some(Value::Object(item))
        }
        CanonicalContentBlock::File {
            data,
            file_id,
            file_url,
            media_type,
            filename,
            ..
        } => {
            let mut item = Map::new();
            item.insert("type".to_string(), Value::String("input_file".to_string()));
            if let Some(value) = file_id {
                item.insert("file_id".to_string(), Value::String(value.clone()));
            }
            if data.is_some() || file_url.is_some() {
                item.insert(
                    "file_data".to_string(),
                    Value::String(media_data_or_url(media_type, data, file_url)),
                );
            }
            if let Some(value) = filename {
                item.insert("filename".to_string(), Value::String(value.clone()));
            }
            (item.len() > 1).then_some(Value::Object(item))
        }
        CanonicalContentBlock::Audio { data, format, .. } => Some(json!({
            "type": "input_audio",
            "input_audio": {
                "data": data.clone().unwrap_or_default(),
                "format": format.clone().unwrap_or_else(|| "mp3".to_string()),
            }
        })),
        CanonicalContentBlock::Unknown {
            raw_type, payload, ..
        } if raw_type == "refusal" => payload
            .get("refusal")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(|text| json!({ "type": "refusal", "refusal": text })),
        CanonicalContentBlock::Thinking { .. }
        | CanonicalContentBlock::ToolUse { .. }
        | CanonicalContentBlock::ToolResult { .. }
        | CanonicalContentBlock::Unknown { .. } => None,
    }
}

fn canonical_tools_to_responses(canonical: &CanonicalRequest) -> Vec<Value> {
    let mut tools = canonical
        .tools
        .iter()
        .map(canonical_tool_to_responses)
        .collect::<Vec<_>>();
    if let Some(extra_tools) = canonical
        .extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| {
            canonical
                .extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .and_then(Value::as_object)
        .and_then(|value| value.get("tools"))
        .and_then(Value::as_array)
    {
        tools.extend(extra_tools.iter().cloned());
    }
    tools
}

fn reasoning_config_to_responses(thinking: &CanonicalThinkingConfig) -> Option<Value> {
    openai_responses_extension(&thinking.extensions)
        .cloned()
        .or_else(|| {
            thinking
                .extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
                .cloned()
        })
        .or_else(|| {
            thinking
                .extensions
                .get("openai")
                .and_then(|value| value.get("reasoning_effort"))
                .and_then(Value::as_str)
                .map(|effort| {
                    json!({
                        "effort": if effort == "xhigh" { "high" } else { effort },
                    })
                })
        })
        .or_else(|| {
            thinking.budget_tokens.map(|budget_tokens| {
                json!({
                    "effort": map_thinking_budget_to_openai_reasoning_effort(budget_tokens),
                })
            })
        })
}

fn canonical_text_config_to_responses(canonical: &CanonicalRequest) -> Option<Value> {
    let mut text = Map::new();
    if let Some(response_format) = &canonical.response_format {
        text.insert(
            "format".to_string(),
            canonical_response_format_to_openai(response_format),
        );
    }
    if let Some(verbosity) = canonical
        .extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| {
            canonical
                .extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .and_then(Value::as_object)
        .and_then(|value| value.get("verbosity"))
        .cloned()
    {
        text.insert("verbosity".to_string(), verbosity);
    }
    (!text.is_empty()).then_some(Value::Object(text))
}

fn canonical_tool_to_responses(tool: &CanonicalToolDefinition) -> Value {
    if let Some(raw) = tool
        .extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| {
            tool.extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .filter(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|tool_type| {
                    tool_type == "custom" || tool_type.starts_with("web_search")
                })
        })
    {
        return raw.clone();
    }
    let mut out = Map::new();
    out.insert("type".to_string(), Value::String("function".to_string()));
    out.insert("name".to_string(), Value::String(tool.name.clone()));
    if let Some(description) = &tool.description {
        out.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    if let Some(parameters) = &tool.parameters {
        out.insert("parameters".to_string(), parameters.clone());
    }
    out.extend(namespace_extension_object(
        &tool.extensions,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        &out,
    ));
    Value::Object(out)
}

fn canonical_tool_choice_to_responses(choice: &CanonicalToolChoice) -> Value {
    match choice {
        CanonicalToolChoice::Auto => Value::String("auto".to_string()),
        CanonicalToolChoice::None => Value::String("none".to_string()),
        CanonicalToolChoice::Required => Value::String("required".to_string()),
        CanonicalToolChoice::Tool { name } => json!({
            "type": "function",
            "name": name,
        }),
    }
}

fn responses_tool_result_output(output: Option<&Value>, content_text: Option<&str>) -> Value {
    match output {
        Some(Value::String(text)) => Value::String(text.clone()),
        Some(value) => serde_json::to_string(value)
            .map(Value::String)
            .unwrap_or_else(|_| Value::String(String::new())),
        None => Value::String(content_text.unwrap_or_default().to_string()),
    }
}

fn insert_number(output: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value.and_then(serde_json::Number::from_f64) {
        output.insert(key.to_string(), Value::Number(value));
    }
}
