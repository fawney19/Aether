use std::collections::{BTreeMap, VecDeque};

use serde_json::{json, Map, Value};

use crate::{
    formats::context::FormatContext,
    formats::openai::shared::{
        map_thinking_budget_to_openai_reasoning_effort, OpenAiResponsesReasoningEffort,
    },
    protocol::canonical::{
        canonical_response_format_to_openai, canonicalize_tool_arguments,
        is_claude_messages_request, is_claude_system_instruction, is_claude_thinking_block,
        is_claude_tool_result, media_data_or_url, namespace_extension_object, openai_content_text,
        openai_extensions, openai_response_format_to_canonical, openai_responses_extension,
        openai_responses_generation_config, openai_responses_input_to_canonical_messages,
        openai_responses_tool_choice_to_canonical, openai_responses_tools_to_canonical,
        CanonicalContentBlock, CanonicalInstruction, CanonicalRequest, CanonicalRole,
        CanonicalThinkingConfig, CanonicalToolChoice, CanonicalToolDefinition,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE, OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
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

    let instructions = canonical_instructions_to_responses(canonical);
    if let Some(instructions) = instructions.clone() {
        output.insert("instructions".to_string(), instructions);
    }
    let mut input = canonical_messages_to_responses_input(canonical)?;
    if let Some(developer_message) =
        claude_system_instructions_to_responses_developer_message(canonical)
    {
        input.insert(0, developer_message);
    }
    ensure_json_object_response_input_mentions_json(canonical, instructions.as_ref(), &mut input);
    output.insert("input".to_string(), Value::Array(input));

    if upstream_is_stream && !compact {
        output.insert("stream".to_string(), Value::Bool(true));
    }
    if let Some(max_tokens) = responses_max_output_tokens(canonical) {
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
    if let Some(reasoning) = canonical_reasoning_config_to_responses(canonical) {
        output.insert("reasoning".to_string(), reasoning);
    }

    output.extend(chat_openai_extension_object_to_responses(
        &canonical.extensions,
        &output,
    ));
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
    apply_claude_responses_request_defaults(canonical, mapped_model, &mut output);
    if compact {
        output.remove("stream");
    }
    output.remove("verbosity");
    Some(Value::Object(output))
}

fn chat_openai_extension_object_to_responses(
    extensions: &BTreeMap<String, Value>,
    existing: &Map<String, Value>,
) -> Map<String, Value> {
    const RESPONSES_COMPATIBLE_CHAT_FIELDS: &[&str] = &[
        "stream",
        "store",
        "service_tier",
        "safety_identifier",
        "prompt_cache_key",
    ];
    extensions
        .get("openai")
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter(|(key, _)| {
                    RESPONSES_COMPATIBLE_CHAT_FIELDS.contains(&key.as_str())
                        && !existing.contains_key(*key)
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn canonical_instructions_to_responses(canonical: &CanonicalRequest) -> Option<Value> {
    let text = canonical
        .instructions
        .iter()
        .filter(|instruction| !is_claude_system_instruction(instruction))
        .map(|instruction| instruction.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !text.trim().is_empty() {
        return Some(Value::String(text));
    }
    if canonical
        .instructions
        .iter()
        .any(is_claude_system_instruction)
    {
        return None;
    }
    canonical
        .system
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .map(Value::String)
}

fn claude_system_instructions_to_responses_developer_message(
    canonical: &CanonicalRequest,
) -> Option<Value> {
    let content = canonical
        .instructions
        .iter()
        .filter(|instruction| is_claude_system_instruction(instruction))
        .filter_map(claude_system_instruction_to_responses_part)
        .collect::<Vec<_>>();
    (!content.is_empty()).then(|| {
        json!({
            "type": "message",
            "role": "developer",
            "content": content,
        })
    })
}

fn claude_system_instruction_to_responses_part(
    instruction: &CanonicalInstruction,
) -> Option<Value> {
    if instruction.text.trim().is_empty() {
        return None;
    }
    let mut part = Map::new();
    part.insert("type".to_string(), Value::String("input_text".to_string()));
    part.insert("text".to_string(), Value::String(instruction.text.clone()));
    part.extend(namespace_extension_object(
        &instruction.extensions,
        "claude",
        &part,
    ));
    Some(Value::Object(part))
}

fn canonical_messages_to_responses_input(canonical: &CanonicalRequest) -> Option<Vec<Value>> {
    let mut input = Vec::new();
    let mut next_generated_tool_call_index = 0usize;
    let mut pending_tool_call_ids = VecDeque::new();
    for message in &canonical.messages {
        let role = match message.role {
            CanonicalRole::Assistant => "assistant",
            CanonicalRole::Tool | CanonicalRole::User | CanonicalRole::Unknown => "user",
            CanonicalRole::System | CanonicalRole::Developer => continue,
        };
        let mut content = Vec::new();
        let mut saw_tool_item = false;
        for block in &message.content {
            match block {
                CanonicalContentBlock::ToolUse {
                    id,
                    name,
                    input: arguments,
                    ..
                } => {
                    flush_responses_message(&mut input, role, &mut content);
                    saw_tool_item = true;
                    let call_id = responses_tool_call_id(id, &mut next_generated_tool_call_index);
                    let tool_name = responses_tool_name(name);
                    pending_tool_call_ids.push_back(call_id.clone());
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": tool_name,
                        "arguments": canonicalize_tool_arguments(arguments),
                    }));
                }
                CanonicalContentBlock::ToolResult {
                    tool_use_id,
                    output,
                    content_text,
                    extensions,
                    ..
                } => {
                    flush_responses_message(&mut input, role, &mut content);
                    saw_tool_item = true;
                    let (tool_output, extra_user_content) = responses_tool_result_payload(
                        output.as_ref(),
                        content_text.as_deref(),
                        extensions,
                    )?;
                    let call_id =
                        responses_tool_result_call_id(tool_use_id, &mut pending_tool_call_ids)?;
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": tool_output,
                    }));
                    if !extra_user_content.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "user",
                            "content": extra_user_content,
                        }));
                    }
                }
                CanonicalContentBlock::Thinking {
                    text, extensions, ..
                } => {
                    if is_claude_thinking_block(extensions) {
                        continue;
                    }
                    if role == "assistant" && !text.trim().is_empty() {
                        content.push(json!({
                            "type": "output_text",
                            "text": format!("<thinking>{text}</thinking>"),
                        }));
                    }
                }
                other => {
                    if let Some(part) = canonical_block_to_responses_input_part(other, role) {
                        content.push(part);
                    }
                }
            }
        }
        if content.is_empty() && !saw_tool_item {
            if role == "assistant" {
                input.push(json!({
                    "type": "message",
                    "role": role,
                    "content": [{
                        "type": "output_text",
                        "text": "",
                    }],
                }));
            } else {
                input.push(json!({
                    "type": "message",
                    "role": role,
                    "content": "",
                }));
            }
            continue;
        }
        flush_responses_message(&mut input, role, &mut content);
    }
    Some(input)
}

fn responses_tool_call_id(id: &str, next_generated_tool_call_index: &mut usize) -> String {
    let trimmed = id.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    let generated = format!("call_auto_{next_generated_tool_call_index}");
    *next_generated_tool_call_index += 1;
    generated
}

fn responses_tool_result_call_id(
    id: &str,
    pending_tool_call_ids: &mut VecDeque<String>,
) -> Option<String> {
    let trimmed = id.trim();
    if !trimmed.is_empty() {
        if let Some(position) = pending_tool_call_ids
            .iter()
            .position(|pending_id| pending_id == trimmed)
        {
            pending_tool_call_ids.remove(position);
        }
        return Some(trimmed.to_string());
    }
    pending_tool_call_ids.pop_front()
}

fn responses_tool_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn responses_max_output_tokens(canonical: &CanonicalRequest) -> Option<u64> {
    canonical.generation.max_tokens.map(|max_tokens| {
        if is_claude_messages_request(&canonical.extensions) && max_tokens < 128 {
            128
        } else {
            max_tokens
        }
    })
}

fn apply_claude_responses_request_defaults(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    output: &mut Map<String, Value>,
) {
    if !is_claude_messages_request(&canonical.extensions) {
        return;
    }
    if mapped_model
        .trim()
        .to_ascii_lowercase()
        .starts_with("gpt-5")
    {
        output.remove("temperature");
        output.remove("top_p");
    }
    output
        .entry("store".to_string())
        .or_insert_with(|| Value::Bool(false));
    output
        .entry("parallel_tool_calls".to_string())
        .or_insert_with(|| Value::Bool(true));
    let include = output
        .entry("include".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(include) = include.as_array_mut() {
        let encrypted_content = Value::String("reasoning.encrypted_content".to_string());
        if !include.iter().any(|value| value == &encrypted_content) {
            include.push(encrypted_content);
        }
    }
}

fn ensure_json_object_response_input_mentions_json(
    canonical: &CanonicalRequest,
    instructions: Option<&Value>,
    input: &mut Vec<Value>,
) {
    if !canonical
        .response_format
        .as_ref()
        .is_some_and(|format| format.format_type.eq_ignore_ascii_case("json_object"))
        || input.iter().any(value_contains_json_word)
        || !instructions.is_some_and(value_contains_json_word)
    {
        return;
    }
    input.insert(
        0,
        json!({
            "type": "message",
            "role": "developer",
            "content": [{
                "type": "input_text",
                "text": "Respond with JSON.",
            }],
        }),
    );
}

fn value_contains_json_word(value: &Value) -> bool {
    match value {
        Value::String(text) => text.to_ascii_lowercase().contains("json"),
        Value::Array(items) => items.iter().any(value_contains_json_word),
        Value::Object(object) => object.values().any(value_contains_json_word),
        _ => false,
    }
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

fn canonical_reasoning_config_to_responses(canonical: &CanonicalRequest) -> Option<Value> {
    let is_claude_request = is_claude_messages_request(&canonical.extensions);
    if !is_claude_request {
        return canonical
            .thinking
            .as_ref()
            .and_then(reasoning_config_to_responses);
    }

    let mut object = canonical
        .thinking
        .as_ref()
        .and_then(|thinking| openai_responses_extension(&thinking.extensions).cloned())
        .and_then(|value| match value {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .unwrap_or_default();
    let effort = canonical
        .thinking
        .as_ref()
        .and_then(|thinking| thinking.extensions.get("claude"))
        .and_then(|value| value.get("output_config"))
        .and_then(|value| value.get("effort"))
        .and_then(Value::as_str)
        .and_then(openai_responses_reasoning_effort)
        .unwrap_or("medium");
    object
        .entry("effort".to_string())
        .or_insert_with(|| Value::String(effort.to_string()));
    object
        .entry("summary".to_string())
        .or_insert_with(|| Value::String("auto".to_string()));
    Some(Value::Object(object))
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
                .and_then(|effort| {
                    let effort = openai_responses_reasoning_effort(effort)?;
                    Some(json!({
                        "effort": effort,
                    }))
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

fn openai_responses_reasoning_effort(effort: &str) -> Option<&'static str> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "max" => Some("xhigh"),
        value => {
            OpenAiResponsesReasoningEffort::parse(value).map(OpenAiResponsesReasoningEffort::as_str)
        }
    }
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
    if is_claude_messages_request(&canonical.extensions) {
        text.entry("verbosity".to_string())
            .or_insert_with(|| Value::String("medium".to_string()));
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
    out.insert(
        "parameters".to_string(),
        responses_tool_parameters_schema(tool.parameters.as_ref()),
    );
    if let Some(strict) = tool.strict {
        out.insert("strict".to_string(), Value::Bool(strict));
    }
    out.extend(namespace_extension_object(
        &tool.extensions,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        &out,
    ));
    Value::Object(out)
}

fn responses_tool_parameters_schema(parameters: Option<&Value>) -> Value {
    match parameters {
        Some(Value::Object(schema)) => {
            let mut schema = schema.clone();
            if schema
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "object")
                && !schema.contains_key("properties")
            {
                schema.insert("properties".to_string(), json!({}));
            }
            Value::Object(schema)
        }
        Some(Value::Null) | None => json!({"type": "object", "properties": {}}),
        Some(value) => value.clone(),
    }
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

fn responses_tool_result_payload(
    output: Option<&Value>,
    content_text: Option<&str>,
    extensions: &BTreeMap<String, Value>,
) -> Option<(Value, Vec<Value>)> {
    if is_claude_tool_result(extensions) {
        if let Some(Value::Array(parts)) = output {
            return claude_tool_result_parts_to_responses_payload(parts);
        }
    }
    Some((
        responses_tool_result_output(output, content_text),
        Vec::new(),
    ))
}

fn responses_tool_result_output(output: Option<&Value>, content_text: Option<&str>) -> Value {
    let text = match output {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) => String::new(),
        Some(value) => serde_json::to_string(value).unwrap_or_default(),
        None => content_text.unwrap_or_default().to_string(),
    };
    Value::String(non_empty_responses_tool_output(&text))
}

pub(crate) fn claude_tool_result_parts_are_openai_responses_representable(parts: &[Value]) -> bool {
    parts
        .iter()
        .all(claude_tool_result_part_is_openai_responses_representable)
}

fn claude_tool_result_part_is_openai_responses_representable(part: &Value) -> bool {
    let Some(part_object) = part.as_object() else {
        return false;
    };
    match part_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => true,
        "image" => claude_image_block_is_openai_responses_representable(part_object),
        "document" | "file" => claude_document_block_is_openai_responses_representable(part_object),
        _ => false,
    }
}

fn claude_image_block_is_openai_responses_representable(block: &Map<String, Value>) -> bool {
    let Some(source) = block.get("source").and_then(Value::as_object) else {
        return false;
    };
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" => claude_source_str(source, "data").is_some(),
        "url" => claude_source_str(source, "url").is_some(),
        _ => false,
    }
}

fn claude_document_block_is_openai_responses_representable(block: &Map<String, Value>) -> bool {
    let Some(source) = block.get("source").and_then(Value::as_object) else {
        return false;
    };
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" | "text" => claude_source_str(source, "data").is_some(),
        "url" => claude_source_str(source, "url").is_some(),
        _ => false,
    }
}

fn claude_tool_result_parts_to_responses_payload(parts: &[Value]) -> Option<(Value, Vec<Value>)> {
    let mut output_texts = Vec::new();
    let mut extra_user_content = Vec::new();

    for part in parts {
        let part_object = part.as_object()?;
        match part_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "text" => {
                if let Some(text) = part_object.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        output_texts.push(text.to_string());
                    }
                }
            }
            "image" => {
                if let Some(part) = claude_image_block_to_responses_input_part(part_object) {
                    extra_user_content.push(part);
                } else {
                    return None;
                }
            }
            "document" | "file" => {
                if let Some(text) = claude_text_document_block_to_responses_output_text(part_object)
                {
                    if !text.is_empty() {
                        output_texts.push(text.to_string());
                    }
                } else if let Some(part) =
                    claude_document_block_to_responses_input_part(part_object)
                {
                    extra_user_content.push(part);
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }

    Some((
        Value::String(non_empty_responses_tool_output(&output_texts.join("\n\n"))),
        extra_user_content,
    ))
}

fn claude_image_block_to_responses_input_part(block: &Map<String, Value>) -> Option<Value> {
    let source = block.get("source")?.as_object()?;
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" => {
            let media_type = claude_source_media_type(source).unwrap_or("image/png");
            let data = claude_source_str(source, "data")?;
            Some(json!({
                "type": "input_image",
                "image_url": format!("data:{media_type};base64,{data}"),
            }))
        }
        "url" => {
            let url = claude_source_str(source, "url")?;
            Some(json!({
                "type": "input_image",
                "image_url": url,
            }))
        }
        _ => None,
    }
}

fn claude_document_block_to_responses_input_part(block: &Map<String, Value>) -> Option<Value> {
    let source = block.get("source")?.as_object()?;
    let file_data = match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "base64" => {
            let media_type = claude_source_media_type(source).unwrap_or("application/octet-stream");
            let data = claude_source_str(source, "data")?;
            format!("data:{media_type};base64,{data}")
        }
        "url" => claude_source_str(source, "url")?.to_string(),
        _ => return None,
    };

    let mut part = Map::new();
    part.insert("type".to_string(), Value::String("input_file".to_string()));
    part.insert("file_data".to_string(), Value::String(file_data));
    if let Some(filename) = block
        .get("title")
        .or_else(|| block.get("name"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        part.insert("filename".to_string(), Value::String(filename.to_string()));
    }
    Some(Value::Object(part))
}

fn claude_text_document_block_to_responses_output_text(block: &Map<String, Value>) -> Option<&str> {
    let source = block.get("source")?.as_object()?;
    match source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => claude_source_str(source, "data"),
        _ => None,
    }
}

fn claude_source_media_type(source: &Map<String, Value>) -> Option<&str> {
    source
        .get("media_type")
        .or_else(|| source.get("mime_type"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn claude_source_str<'a>(source: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    source
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn non_empty_responses_tool_output(text: &str) -> String {
    if text.is_empty() {
        "(empty)".to_string()
    } else {
        text.to_string()
    }
}

fn insert_number(output: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value.and_then(serde_json::Number::from_f64) {
        output.insert(key.to_string(), Value::Number(value));
    }
}

#[cfg(test)]
mod tests {
    use super::to_raw;
    use crate::protocol::canonical::{
        CanonicalContentBlock, CanonicalMessage, CanonicalRequest, CanonicalResponseFormat,
        CanonicalRole,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    fn claude_tool_result_extensions() -> BTreeMap<String, serde_json::Value> {
        let mut extensions = BTreeMap::new();
        extensions.insert(
            "aether".to_string(),
            json!({ "source": "claude_tool_result" }),
        );
        extensions
    }

    #[test]
    fn json_object_response_injects_json_hint_as_developer_input_when_only_instructions_have_it() {
        let request = CanonicalRequest {
            model: "gpt-5.5".to_string(),
            system: Some("Please answer in JSON.".to_string()),
            messages: vec![CanonicalMessage {
                role: CanonicalRole::User,
                content: vec![CanonicalContentBlock::Text {
                    text: "hello".to_string(),
                    extensions: Default::default(),
                }],
                extensions: Default::default(),
            }],
            response_format: Some(CanonicalResponseFormat {
                format_type: "json_object".to_string(),
                json_schema: None,
                extensions: Default::default(),
            }),
            ..CanonicalRequest::default()
        };

        let body = to_raw(&request, "gpt-5.5", false, false).expect("responses body");

        assert_eq!(body["text"]["format"]["type"], json!("json_object"));
        assert_eq!(body["instructions"], json!("Please answer in JSON."));
        let input = body["input"].as_array().expect("input");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], json!("developer"));
        assert!(input[0]["content"][0]["text"]
            .as_str()
            .expect("hint text")
            .to_ascii_lowercase()
            .contains("json"));
        assert_eq!(input[1]["role"], json!("user"));
        assert_eq!(input[1]["content"][0]["text"], json!("hello"));
    }

    #[test]
    fn responses_request_preserves_empty_chat_messages() {
        let request = CanonicalRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![
                CanonicalMessage {
                    role: CanonicalRole::User,
                    content: vec![CanonicalContentBlock::Text {
                        text: String::new(),
                        extensions: Default::default(),
                    }],
                    extensions: Default::default(),
                },
                CanonicalMessage {
                    role: CanonicalRole::Assistant,
                    content: Vec::new(),
                    extensions: Default::default(),
                },
            ],
            ..CanonicalRequest::default()
        };

        let body = to_raw(&request, "gpt-5.5", false, false).expect("responses body");

        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"], "");
        assert_eq!(body["input"][1]["role"], "assistant");
        assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
        assert_eq!(body["input"][1]["content"][0]["text"], "");
    }

    #[test]
    fn responses_request_uses_empty_marker_for_empty_tool_output() {
        let request = CanonicalRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![CanonicalMessage {
                role: CanonicalRole::Tool,
                content: vec![CanonicalContentBlock::ToolResult {
                    tool_use_id: "call_empty".to_string(),
                    name: None,
                    output: Some(json!("")),
                    content_text: None,
                    is_error: false,
                    extensions: Default::default(),
                }],
                extensions: Default::default(),
            }],
            ..CanonicalRequest::default()
        };

        let body = to_raw(&request, "gpt-5.5", false, false).expect("responses body");

        assert_eq!(body["input"].as_array().expect("input").len(), 1);
        assert_eq!(body["input"][0]["type"], "function_call_output");
        assert_eq!(body["input"][0]["call_id"], "call_empty");
        assert_eq!(body["input"][0]["output"], "(empty)");
    }

    #[test]
    fn responses_request_replaces_empty_tool_call_identifiers() {
        let request = CanonicalRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![
                CanonicalMessage {
                    role: CanonicalRole::Assistant,
                    content: vec![CanonicalContentBlock::ToolUse {
                        id: "   ".to_string(),
                        name: "".to_string(),
                        input: json!({"q": "rust"}),
                        extensions: Default::default(),
                    }],
                    extensions: Default::default(),
                },
                CanonicalMessage {
                    role: CanonicalRole::Tool,
                    content: vec![CanonicalContentBlock::ToolResult {
                        tool_use_id: "".to_string(),
                        name: None,
                        output: Some(json!({"ok": true})),
                        content_text: None,
                        is_error: false,
                        extensions: Default::default(),
                    }],
                    extensions: Default::default(),
                },
            ],
            ..CanonicalRequest::default()
        };

        let body = to_raw(&request, "gpt-5.5", false, false).expect("responses body");

        assert_eq!(body["input"].as_array().expect("input").len(), 2);
        assert_eq!(body["input"][0]["type"], "function_call");
        assert_eq!(body["input"][0]["call_id"], "call_auto_0");
        assert_eq!(body["input"][0]["name"], "unknown");
        assert_eq!(body["input"][0]["arguments"], "{\"q\":\"rust\"}");
        assert_eq!(body["input"][1]["type"], "function_call_output");
        assert_eq!(body["input"][1]["call_id"], "call_auto_0");
    }

    #[test]
    fn responses_request_assigns_empty_tool_result_identifiers_from_pending_tool_calls_in_order() {
        let request = CanonicalRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![
                CanonicalMessage {
                    role: CanonicalRole::Assistant,
                    content: vec![
                        CanonicalContentBlock::ToolUse {
                            id: "call_a".to_string(),
                            name: "lookup_a".to_string(),
                            input: json!({"q": "a"}),
                            extensions: Default::default(),
                        },
                        CanonicalContentBlock::ToolUse {
                            id: "call_b".to_string(),
                            name: "lookup_b".to_string(),
                            input: json!({"q": "b"}),
                            extensions: Default::default(),
                        },
                    ],
                    extensions: Default::default(),
                },
                CanonicalMessage {
                    role: CanonicalRole::Tool,
                    content: vec![
                        CanonicalContentBlock::ToolResult {
                            tool_use_id: " ".to_string(),
                            name: None,
                            output: Some(json!("result a")),
                            content_text: None,
                            is_error: false,
                            extensions: Default::default(),
                        },
                        CanonicalContentBlock::ToolResult {
                            tool_use_id: "".to_string(),
                            name: None,
                            output: Some(json!("result b")),
                            content_text: None,
                            is_error: false,
                            extensions: Default::default(),
                        },
                    ],
                    extensions: Default::default(),
                },
            ],
            ..CanonicalRequest::default()
        };

        let body = to_raw(&request, "gpt-5.5", false, false).expect("responses body");

        assert_eq!(body["input"][0]["call_id"], "call_a");
        assert_eq!(body["input"][1]["call_id"], "call_b");
        assert_eq!(body["input"][2]["call_id"], "call_a");
        assert_eq!(body["input"][2]["output"], "result a");
        assert_eq!(body["input"][3]["call_id"], "call_b");
        assert_eq!(body["input"][3]["output"], "result b");
    }

    #[test]
    fn responses_request_rejects_orphan_empty_tool_result_identifier() {
        let request = CanonicalRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![CanonicalMessage {
                role: CanonicalRole::Tool,
                content: vec![CanonicalContentBlock::ToolResult {
                    tool_use_id: " ".to_string(),
                    name: None,
                    output: Some(json!({"ok": true})),
                    content_text: None,
                    is_error: false,
                    extensions: Default::default(),
                }],
                extensions: Default::default(),
            }],
            ..CanonicalRequest::default()
        };

        assert!(to_raw(&request, "gpt-5.5", false, false).is_none());
    }

    #[test]
    fn responses_request_preserves_claude_text_document_tool_result_content() {
        let request = CanonicalRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![CanonicalMessage {
                role: CanonicalRole::Tool,
                content: vec![CanonicalContentBlock::ToolResult {
                    tool_use_id: "call_doc".to_string(),
                    name: None,
                    output: Some(json!([
                        {"type": "text", "text": "preview"},
                        {
                            "type": "document",
                            "source": {
                                "type": "text",
                                "media_type": "text/plain",
                                "data": "document body"
                            }
                        }
                    ])),
                    content_text: None,
                    is_error: false,
                    extensions: claude_tool_result_extensions(),
                }],
                extensions: Default::default(),
            }],
            ..CanonicalRequest::default()
        };

        let body = to_raw(&request, "gpt-5.5", false, false).expect("responses body");

        assert_eq!(body["input"][0]["type"], "function_call_output");
        assert_eq!(body["input"][0]["output"], "preview\n\ndocument body");
        assert!(!body.to_string().contains("content omitted"));
    }

    #[test]
    fn responses_request_rejects_unrepresentable_claude_tool_result_blocks() {
        let request = CanonicalRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![CanonicalMessage {
                role: CanonicalRole::Tool,
                content: vec![CanonicalContentBlock::ToolResult {
                    tool_use_id: "call_img".to_string(),
                    name: None,
                    output: Some(json!([{
                        "type": "image",
                        "source": {
                            "type": "unsupported",
                            "media_type": "image/png",
                            "data": "AAAA"
                        }
                    }])),
                    content_text: None,
                    is_error: false,
                    extensions: claude_tool_result_extensions(),
                }],
                extensions: Default::default(),
            }],
            ..CanonicalRequest::default()
        };

        assert!(to_raw(&request, "gpt-5.5", false, false).is_none());
    }
}
