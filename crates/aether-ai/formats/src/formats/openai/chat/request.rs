use serde_json::{json, Map, Value};

use crate::{
    formats::{context::FormatContext, openai::namespace::NamespaceToolAliases},
    protocol::canonical::{
        canonical_extension_object_mut, canonical_message_to_openai_chat_messages,
        canonical_response_format_to_openai, canonical_tool_choice_to_openai,
        canonical_tool_is_openai_custom, canonical_tool_to_openai, is_claude_tool_result,
        is_openai_custom_tool_call, namespace_extension_object, openai_content_text,
        openai_extensions, openai_generation_config, openai_message_content_blocks,
        openai_response_format_to_canonical, openai_responses_extension, openai_role_to_canonical,
        openai_tool_choice_raw_to_chat, openai_tool_choice_to_canonical, openai_tools_to_canonical,
        write_openai_generation_config, CanonicalContentBlock, CanonicalInstruction,
        CanonicalMessage, CanonicalRequest, CanonicalRole, CanonicalThinkingConfig,
        CanonicalToolChoice, CanonicalToolDefinition, OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_raw(body)
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    let mut body = to_raw(request)?;
    force_stream_options(&mut body, ctx.upstream_is_stream);
    Some(body)
}

pub(crate) fn to_raw(canonical: &CanonicalRequest) -> Option<Value> {
    let namespace_tool_aliases = NamespaceToolAliases::from_canonical_tools(&canonical.tools);
    if canonical_request_has_unrepresentable_claude_tool_result_for_openai_chat(canonical)
        || canonical_request_has_unrepresentable_namespace_tools_for_openai_chat(
            canonical,
            &namespace_tool_aliases,
        )
    {
        return None;
    }
    Some(to_raw_with_namespace_aliases(
        canonical,
        &namespace_tool_aliases,
    ))
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

fn to_raw_with_namespace_aliases(
    canonical: &CanonicalRequest,
    namespace_tool_aliases: &NamespaceToolAliases,
) -> Value {
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
        let mut message = message.clone();
        rewrite_namespaced_tool_uses_for_openai_chat(&mut message, namespace_tool_aliases);
        messages.extend(canonical_message_to_openai_chat_messages(&message));
    }
    output.insert("messages".to_string(), Value::Array(messages));

    write_openai_generation_config(&mut output, &canonical.generation);
    if !canonical.tools.is_empty() {
        let mut tools = Vec::new();
        for (tool_index, tool) in canonical.tools.iter().enumerate() {
            if namespace_tool_aliases.is_representable_namespace_tool(tool_index) {
                tools.extend(
                    namespace_tool_aliases
                        .tools_for_source(tool_index)
                        .map(|tool| tool.to_openai_chat_tool()),
                );
            } else {
                tools.push(canonical_tool_to_openai(tool));
            }
        }
        output.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(tool_choice) =
        canonical_tool_choice_to_openai_for_request(canonical, namespace_tool_aliases)
    {
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

fn canonical_tool_choice_to_openai_for_request(
    canonical: &CanonicalRequest,
    namespace_tool_aliases: &NamespaceToolAliases,
) -> Option<Value> {
    canonical
        .tool_choice
        .as_ref()
        .map(|tool_choice| {
            if let CanonicalToolChoice::Tool { name } = tool_choice {
                if let NamespaceNameResolution::Alias(alias) =
                    resolve_namespace_child_name(name, &canonical.tools, namespace_tool_aliases)
                {
                    return json!({
                        "type": "function",
                        "function": { "name": alias },
                    });
                }
            }
            canonical_tool_choice_to_openai_for_tools(tool_choice, &canonical.tools)
        })
        .or_else(|| {
            raw_tool_choice_extension(canonical).and_then(|raw| {
                raw_tool_choice_to_openai_chat_for_request(
                    raw,
                    &canonical.tools,
                    namespace_tool_aliases,
                )
            })
        })
}

fn raw_tool_choice_to_openai_chat_for_request(
    raw: &Value,
    tools: &[CanonicalToolDefinition],
    aliases: &NamespaceToolAliases,
) -> Option<Value> {
    let mut raw = raw.clone();
    resolve_qualified_tool_choices(&mut raw, aliases)?;
    let mut choice = openai_tool_choice_raw_to_chat(&raw);
    rewrite_namespace_tool_choice_names(&mut choice, tools, aliases).then_some(choice)
}

fn resolve_qualified_tool_choices(
    choice: &mut Value,
    aliases: &NamespaceToolAliases,
) -> Option<()> {
    if choice.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        if let Some(tools) = choice.get_mut("tools").and_then(Value::as_array_mut) {
            for tool in tools {
                resolve_qualified_tool_choices(tool, aliases)?;
            }
        }
    } else if let Some(namespace) = choice.get("namespace") {
        let alias = aliases.chat_name(namespace.as_str()?, choice.get("name")?.as_str()?)?;
        let kind = choice.get("type")?.as_str()?;
        if kind
            != if aliases.is_wrapped_custom(alias) {
                "custom"
            } else {
                "function"
            }
        {
            return None;
        }
        *choice = json!({"type":"function", "name":alias});
    }
    Some(())
}

pub(crate) fn raw_tool_choice_extension_is_representable_for_openai_chat(
    canonical: &CanonicalRequest,
) -> bool {
    let aliases = NamespaceToolAliases::from_canonical_tools(&canonical.tools);
    raw_tool_choice_extension(canonical).is_some_and(|raw| {
        raw_tool_choice_to_openai_chat_for_request(raw, &canonical.tools, &aliases).is_some()
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamespaceNameResolution<'a> {
    Unchanged,
    Alias(&'a str),
    Ambiguous,
}

fn resolve_namespace_child_name<'a>(
    name: &'a str,
    tools: &[CanonicalToolDefinition],
    aliases: &'a NamespaceToolAliases,
) -> NamespaceNameResolution<'a> {
    let namespace_children = aliases.namespace_children_named(name).collect::<Vec<_>>();
    let ordinary_matches = tools
        .iter()
        .enumerate()
        .filter(|(index, tool)| !aliases.is_namespace_tool(*index) && tool.name == name)
        .count();
    let namespace_parent_matches = tools
        .iter()
        .enumerate()
        .filter(|(index, tool)| aliases.is_namespace_tool(*index) && tool.name == name)
        .count();

    match (
        namespace_children.as_slice(),
        ordinary_matches,
        namespace_parent_matches,
    ) {
        ([], 0, 0) | ([], 1, _) => NamespaceNameResolution::Unchanged,
        ([], 0, _) => NamespaceNameResolution::Ambiguous,
        ([child], 0, _) => NamespaceNameResolution::Alias(child.chat_name.as_str()),
        _ => NamespaceNameResolution::Ambiguous,
    }
}

fn rewrite_namespace_tool_choice_names(
    choice: &mut Value,
    tools: &[CanonicalToolDefinition],
    aliases: &NamespaceToolAliases,
) -> bool {
    if let Some(choice) = choice.as_str() {
        return matches!(choice, "none" | "auto" | "required");
    }
    let Some(choice) = choice.as_object_mut() else {
        return false;
    };
    let choice_type = choice
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match choice_type {
        "function" => {
            if !object_has_exact_keys(choice, &["type", "function"]) {
                return false;
            }
            choice
                .get_mut("function")
                .and_then(Value::as_object_mut)
                .is_some_and(|function| {
                    object_has_exact_keys(function, &["name"])
                        && rewrite_named_function_choice(function, tools, aliases)
                })
        }
        "custom" => rewrite_custom_tool_choice(choice, tools, aliases),
        "allowed_tools" => {
            if !object_has_exact_keys(choice, &["type", "allowed_tools"]) {
                return false;
            }
            let Some(allowed) = choice
                .get_mut("allowed_tools")
                .and_then(Value::as_object_mut)
            else {
                return false;
            };
            if !object_has_exact_keys(allowed, &["mode", "tools"])
                || !allowed
                    .get("mode")
                    .and_then(Value::as_str)
                    .is_some_and(|mode| matches!(mode, "auto" | "required"))
            {
                return false;
            }
            let Some(allowed_tools) = allowed.get_mut("tools").and_then(Value::as_array_mut) else {
                return false;
            };
            allowed_tools.iter_mut().all(|tool| {
                let Some(tool) = tool.as_object_mut() else {
                    return false;
                };
                match tool.get("type").and_then(Value::as_str) {
                    Some("function") => {
                        object_has_exact_keys(tool, &["type", "function"])
                            && tool
                                .get_mut("function")
                                .and_then(Value::as_object_mut)
                                .is_some_and(|function| {
                                    object_has_exact_keys(function, &["name"])
                                        && rewrite_named_function_choice(function, tools, aliases)
                                })
                    }
                    Some("custom") => rewrite_custom_tool_choice(tool, tools, aliases),
                    _ => false,
                }
            })
        }
        _ => false,
    }
}

fn rewrite_custom_tool_choice(
    choice: &mut Map<String, Value>,
    tools: &[CanonicalToolDefinition],
    aliases: &NamespaceToolAliases,
) -> bool {
    let Some(custom) = choice.get("custom").and_then(Value::as_object) else {
        return false;
    };
    if !object_has_exact_keys(choice, &["type", "custom"])
        || !object_has_exact_keys(custom, &["name"])
        || !valid_named_choice(custom)
    {
        return false;
    }
    match resolve_namespace_child_name(custom["name"].as_str().unwrap().trim(), tools, aliases) {
        NamespaceNameResolution::Unchanged => true,
        NamespaceNameResolution::Alias(alias) if aliases.is_wrapped_custom(alias) => {
            let function = json!({"name":alias});
            choice.remove("custom");
            choice.insert("type".into(), "function".into());
            choice.insert("function".into(), function);
            true
        }
        _ => false,
    }
}

fn object_has_exact_keys(object: &Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && object.keys().all(|key| keys.contains(&key.as_str()))
}

fn valid_named_choice(choice: &Map<String, Value>) -> bool {
    choice
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|name| !name.is_empty())
}

fn rewrite_named_function_choice(
    function: &mut Map<String, Value>,
    tools: &[CanonicalToolDefinition],
    aliases: &NamespaceToolAliases,
) -> bool {
    let Some(name) = function
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
    else {
        return false;
    };
    match resolve_namespace_child_name(&name, tools, aliases) {
        NamespaceNameResolution::Unchanged => true,
        NamespaceNameResolution::Alias(alias) => {
            function.insert("name".to_string(), Value::String(alias.to_string()));
            true
        }
        NamespaceNameResolution::Ambiguous => false,
    }
}

fn rewrite_namespaced_tool_uses_for_openai_chat(
    message: &mut CanonicalMessage,
    aliases: &NamespaceToolAliases,
) {
    for block in &mut message.content {
        let CanonicalContentBlock::ToolUse {
            name,
            input,
            extensions,
            ..
        } = block
        else {
            continue;
        };
        let NamespaceField::Name(namespace) = namespace_field(extensions) else {
            continue;
        };
        if let Some(alias) = aliases.chat_name(namespace, name) {
            *name = alias.to_string();
            if aliases.is_wrapped_custom(alias) {
                *input = json!({"input": input});
                if let Some(marker) = extensions.get_mut("aether").and_then(Value::as_object_mut) {
                    marker.remove("source");
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamespaceField<'a> {
    Absent,
    Name(&'a str),
    Invalid,
}

fn namespace_field(extensions: &std::collections::BTreeMap<String, Value>) -> NamespaceField<'_> {
    let Some(responses) = openai_responses_extension(extensions).and_then(Value::as_object) else {
        return NamespaceField::Absent;
    };
    let Some(namespace) = responses.get("namespace") else {
        return NamespaceField::Absent;
    };
    namespace
        .as_str()
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty())
        .map(NamespaceField::Name)
        .unwrap_or(NamespaceField::Invalid)
}

fn canonical_request_has_unrepresentable_namespace_tools_for_openai_chat(
    request: &CanonicalRequest,
    aliases: &NamespaceToolAliases,
) -> bool {
    if aliases.has_invalid_namespace_tools() {
        return true;
    }

    if let Some(CanonicalToolChoice::Tool { name }) = &request.tool_choice {
        if resolve_namespace_child_name(name, &request.tools, aliases)
            == NamespaceNameResolution::Ambiguous
        {
            return true;
        }
    } else if let Some(raw) = raw_tool_choice_extension(request) {
        if raw_tool_choice_to_openai_chat_for_request(raw, &request.tools, aliases).is_none() {
            return true;
        }
    }

    request.messages.iter().any(|message| {
        message.content.iter().any(|block| {
            let CanonicalContentBlock::ToolUse {
                name,
                input,
                extensions,
                ..
            } = block
            else {
                return false;
            };
            match namespace_field(extensions) {
                NamespaceField::Absent => false,
                NamespaceField::Invalid => true,
                NamespaceField::Name(namespace) => {
                    aliases.chat_name(namespace, name).is_none_or(|alias| {
                        let wrapped = aliases.is_wrapped_custom(alias);
                        wrapped != is_openai_custom_tool_call(extensions)
                            || (wrapped && !input.is_string())
                    }) || !namespace_tool_use_sidecars_are_chat_representable(extensions, namespace)
                }
            }
        })
    })
}

fn namespace_tool_use_sidecars_are_chat_representable(
    extensions: &std::collections::BTreeMap<String, Value>,
    expected_namespace: &str,
) -> bool {
    [
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    ]
    .into_iter()
    .filter_map(|provider_namespace| extensions.get(provider_namespace))
    .all(|provider_fields| {
        let Some(provider_fields) = provider_fields.as_object() else {
            return false;
        };
        if !provider_fields
            .keys()
            .all(|key| matches!(key.as_str(), "namespace" | "item_id" | "status"))
        {
            return false;
        }
        let namespace_is_consistent = provider_fields.get("namespace").is_none_or(|namespace| {
            namespace
                .as_str()
                .map(str::trim)
                .is_some_and(|namespace| namespace == expected_namespace)
        });
        let item_id_is_valid = provider_fields.get("item_id").is_none_or(|item_id| {
            item_id
                .as_str()
                .map(str::trim)
                .is_some_and(|item_id| !item_id.is_empty())
        });
        let status_is_discardable = provider_fields
            .get("status")
            .is_none_or(|status| status.as_str() == Some("completed"));
        namespace_is_consistent && item_id_is_valid && status_is_discardable
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::openai::responses;

    #[test]
    fn responses_namespace_expands_definition_and_maps_history_and_named_choice() {
        let body = json!({
            "model": "gpt-source",
            "input": [
                {
                    "type": "function_call",
                    "id": "fc_report",
                    "call_id": "call_report",
                    "namespace": "mcp__reports",
                    "name": "write_report",
                    "arguments": "{\"report_path\":\"reports/finding.md\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_report",
                    "output": "created"
                }
            ],
            "tools": [{
                "type": "namespace",
                "name": "mcp__reports",
                "description": "Reporting tools",
                "tools": [{
                    "type": "function",
                    "name": "write_report",
                    "description": "Create a report",
                    "parameters": {
                        "type": "object",
                        "properties": {"report_path": {"type": "string"}},
                        "required": ["report_path"],
                        "additionalProperties": false
                    },
                    "strict": true
                }]
            }],
            "tool_choice": {"type": "function", "name": "write_report"}
        });
        let canonical = responses::request::from_raw(&body).expect("Responses request");
        let chat = to(&canonical, &FormatContext::default()).expect("Chat request");

        assert_eq!(chat["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(chat["tools"][0]["function"]["name"], "write_report");
        assert_eq!(
            chat["tools"][0]["function"]["parameters"],
            body["tools"][0]["tools"][0]["parameters"]
        );
        assert_eq!(chat["tools"][0]["function"]["strict"], true);
        assert_eq!(chat["tool_choice"]["function"]["name"], "write_report");
        let historical_call = chat["messages"]
            .as_array()
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message.get("tool_calls").is_some())
            })
            .expect("historical tool call");
        assert_eq!(
            historical_call["tool_calls"][0]["function"]["name"],
            "write_report"
        );
    }

    #[test]
    fn responses_namespace_named_choice_fails_closed_when_child_name_is_ambiguous() {
        let body = json!({
            "model": "gpt-source",
            "input": "write it",
            "tools": [
                {
                    "type": "namespace",
                    "name": "first",
                    "description": "First tools",
                    "tools": [{
                        "type": "function",
                        "name": "write_report",
                        "parameters": {"type": "object"}
                    }]
                },
                {
                    "type": "namespace",
                    "name": "second",
                    "description": "Second tools",
                    "tools": [{
                        "type": "function",
                        "name": "write_report",
                        "parameters": {"type": "object"}
                    }]
                }
            ],
            "tool_choice": {"type": "function", "name": "write_report"}
        });
        let canonical = responses::request::from_raw(&body).expect("Responses request");

        assert!(to(&canonical, &FormatContext::default()).is_none());
    }

    #[test]
    fn responses_namespace_named_choice_allows_parent_and_child_to_share_a_name() {
        let body = json!({
            "model": "gpt-source",
            "input": "write it",
            "tools": [{
                "type": "namespace",
                "name": "reports",
                "description": "Reporting tools",
                "tools": [{
                    "type": "function",
                    "name": "reports",
                    "parameters": {"type": "object"}
                }]
            }],
            "tool_choice": {"type": "function", "name": "reports"}
        });
        let canonical = responses::request::from_raw(&body).expect("Responses request");
        let chat = to(&canonical, &FormatContext::default()).expect("Chat request");

        assert_eq!(chat["tools"][0]["function"]["name"], "reports");
        assert_eq!(chat["tool_choice"]["function"]["name"], "reports");
    }

    #[test]
    fn responses_namespace_allowed_tools_choice_uses_the_expanded_alias() {
        let long_name = format!("write_report_{}", "x".repeat(80));
        let body = json!({
            "model": "gpt-source",
            "input": "write it",
            "tools": [{
                "type": "namespace",
                "name": "reports",
                "description": "Reporting tools",
                "tools": [{
                    "type": "function",
                    "name": long_name,
                    "parameters": {"type": "object"}
                }]
            }],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "required",
                "tools": [{"type": "function", "name": long_name}]
            }
        });
        let canonical = responses::request::from_raw(&body).expect("Responses request");
        let chat = to(&canonical, &FormatContext::default()).expect("Chat request");
        let definition_alias = chat["tools"][0]["function"]["name"]
            .as_str()
            .expect("definition alias");
        let choice_alias = chat["tool_choice"]["allowed_tools"]["tools"][0]["function"]["name"]
            .as_str()
            .expect("choice alias");

        assert_eq!(choice_alias, definition_alias);
        assert!(definition_alias.len() <= 64);
    }

    #[test]
    fn responses_namespace_tool_choices_fail_closed_when_the_shape_is_malformed() {
        let choices = [
            json!({"type": "function"}),
            json!({
                "type": "allowed_tools",
                "tools": [{"type": "function", "name": "write_report"}]
            }),
            json!({"type": "allowed_tools", "mode": "required"}),
            json!({
                "type": "allowed_tools",
                "mode": "required",
                "tools": [42]
            }),
            json!({
                "type": "allowed_tools",
                "mode": "required",
                "tools": [{"type": "future_tool", "name": "write_report"}]
            }),
        ];

        for tool_choice in choices {
            let body = json!({
                "model": "gpt-source",
                "input": "write it",
                "tools": [{
                    "type": "namespace",
                    "name": "reports",
                    "description": "Reporting tools",
                    "tools": [{
                        "type": "function",
                        "name": "write_report",
                        "parameters": {"type": "object"}
                    }]
                }],
                "tool_choice": tool_choice
            });
            let canonical = responses::request::from_raw(&body).expect("Responses request");

            assert!(to_raw(&canonical).is_none());
            assert!(to(&canonical, &FormatContext::default()).is_none());
        }
    }

    #[test]
    fn responses_namespace_alias_avoids_ordinary_function_name_collisions() {
        let body = json!({
            "model": "gpt-source",
            "input": [{
                "type": "function_call",
                "call_id": "call_report",
                "namespace": "reports",
                "name": "write_report",
                "arguments": "{}"
            }],
            "tools": [
                {
                    "type": "function",
                    "name": "write_report",
                    "parameters": {"type": "object"}
                },
                {
                    "type": "function",
                    "name": "reports__write_report",
                    "parameters": {"type": "object"}
                },
                {
                    "type": "namespace",
                    "name": "reports",
                    "description": "Reporting tools",
                    "tools": [{
                        "type": "function",
                        "name": "write_report",
                        "parameters": {"type": "object"}
                    }]
                }
            ]
        });
        let canonical = responses::request::from_raw(&body).expect("Responses request");
        let chat = to(&canonical, &FormatContext::default()).expect("Chat request");
        let names = chat["tools"]
            .as_array()
            .expect("Chat tools")
            .iter()
            .map(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .expect("Chat function name")
            })
            .collect::<std::collections::BTreeSet<_>>();
        let namespace_alias = chat["tools"][2]["function"]["name"]
            .as_str()
            .expect("namespace alias");

        assert_eq!(names.len(), 3);
        assert!(names.contains("write_report"));
        assert!(names.contains("reports__write_report"));
        assert!(namespace_alias.starts_with("aether_ns_"));
        assert!(names.iter().all(|name| {
            name.len() <= 64
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }));
        assert_eq!(
            chat["messages"][0]["tool_calls"][0]["function"]["name"],
            namespace_alias
        );

        let mut ambiguous = body;
        ambiguous["tool_choice"] = json!({"type": "function", "name": "write_report"});
        let canonical = responses::request::from_raw(&ambiguous).expect("Responses request");
        assert!(to(&canonical, &FormatContext::default()).is_none());
    }

    #[test]
    fn responses_namespace_history_fails_closed_when_identity_is_unknown() {
        let body = json!({
            "model": "gpt-source",
            "input": [{
                "type": "function_call",
                "call_id": "call_report",
                "namespace": "unknown_namespace",
                "name": "write_report",
                "arguments": "{}"
            }],
            "tools": [{
                "type": "namespace",
                "name": "reports",
                "description": "Reporting tools",
                "tools": [{
                    "type": "function",
                    "name": "write_report",
                    "parameters": {"type": "object"}
                }]
            }]
        });
        let canonical = responses::request::from_raw(&body).expect("Responses request");

        assert!(to(&canonical, &FormatContext::default()).is_none());
    }

    #[test]
    fn namespace_history_sidecars_require_exact_keys_and_consistent_namespaces() {
        let valid = std::collections::BTreeMap::from([(
            OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
            json!({
                "namespace": "reports",
                "item_id": "fc_report",
                "status": "completed"
            }),
        )]);
        assert!(namespace_tool_use_sidecars_are_chat_representable(
            &valid, "reports"
        ));

        let mut conflicting = valid.clone();
        conflicting.insert(
            OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE.to_string(),
            json!({
                "namespace": "other_reports",
                "item_id": "fc_report",
                "status": "completed"
            }),
        );
        assert!(!namespace_tool_use_sidecars_are_chat_representable(
            &conflicting,
            "reports"
        ));

        let unknown = std::collections::BTreeMap::from([(
            OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
            json!({
                "namespace": "reports",
                "caller": "future-semantic-owner"
            }),
        )]);
        assert!(!namespace_tool_use_sidecars_are_chat_representable(
            &unknown, "reports"
        ));
    }
}
