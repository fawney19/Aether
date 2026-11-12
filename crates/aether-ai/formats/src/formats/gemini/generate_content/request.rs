use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::{
    formats::{
        context::FormatContext,
        openai::shared::{
            map_openai_reasoning_effort_to_gemini_budget,
            map_thinking_budget_to_openai_reasoning_effort,
        },
        shared::model_directives::{
            gemini_model_supports_mixed_tools, gemini_model_uses_thinking_level, ReasoningEffort,
        },
    },
    protocol::canonical::{
        apply_gemini_request_extensions, canonical_extension_object_mut,
        canonical_openai_reasoning_effort, extract_gemini_model_from_path,
        gemini_contents_to_canonical_messages, gemini_extensions, gemini_generation_config,
        gemini_generation_config_extra, gemini_google_search_grounding,
        gemini_response_format_to_canonical, gemini_system_to_canonical_instructions,
        gemini_thinking_to_canonical, gemini_tool_choice_to_canonical, gemini_tools_to_canonical,
        gemini_value_by_case, is_cross_format_tool_result, CanonicalContentBlock, CanonicalMessage,
        CanonicalRequest, CanonicalResponseFormat, CanonicalRole, CanonicalToolChoice,
        CanonicalToolDefinition, OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
};

pub fn from(body: &Value, ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_raw(body, ctx.request_path.as_deref().unwrap_or_default())
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    to_raw_with_schema_policy(
        request,
        ctx.mapped_model_or(request.model.as_str()),
        ctx.upstream_is_stream,
        ctx.preserve_gemini_tool_schemas,
    )
}

pub fn from_raw(body_json: &Value, request_path: &str) -> Option<CanonicalRequest> {
    let request = body_json.as_object()?;
    let mut canonical = CanonicalRequest {
        model: request
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| extract_gemini_model_from_path(request_path))
            .unwrap_or_default(),
        ..CanonicalRequest::default()
    };

    canonical.instructions = gemini_system_to_canonical_instructions(
        request
            .get("systemInstruction")
            .or_else(|| request.get("system_instruction")),
    )?;
    let system_text = canonical
        .instructions
        .iter()
        .map(|instruction| instruction.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system_text.is_empty() {
        canonical.system = Some(system_text);
    }
    canonical.messages = gemini_contents_to_canonical_messages(request.get("contents"))?;
    canonical.generation = gemini_generation_config(
        request
            .get("generationConfig")
            .or_else(|| request.get("generation_config")),
    );
    canonical.thinking = gemini_thinking_to_canonical(
        request
            .get("generationConfig")
            .or_else(|| request.get("generation_config")),
    );
    canonical.response_format = gemini_response_format_to_canonical(
        request
            .get("generationConfig")
            .or_else(|| request.get("generation_config")),
    );
    let (tools, builtin_tools, web_search_options, raw_tools, google_search_grounding) =
        gemini_tools_to_canonical(request.get("tools"))?;
    canonical.tools = tools;
    canonical.tool_choice = gemini_tool_choice_to_canonical(
        request
            .get("toolConfig")
            .or_else(|| request.get("tool_config")),
    );

    canonical.extensions = gemini_extensions(
        request,
        &[
            "model",
            "systemInstruction",
            "system_instruction",
            "contents",
            "generationConfig",
            "generation_config",
            "tools",
            "toolConfig",
            "tool_config",
            "safetySettings",
            "safety_settings",
            "cachedContent",
            "cached_content",
            "stream",
        ],
    );
    if let Some(generation_config) = request
        .get("generationConfig")
        .or_else(|| request.get("generation_config"))
        .and_then(Value::as_object)
    {
        let gemini_extension = canonical_extension_object_mut(&mut canonical.extensions, "gemini");
        if let Some(thinking_config) =
            gemini_value_by_case(generation_config, "thinkingConfig", "thinking_config").cloned()
        {
            gemini_extension.insert("thinking_config".to_string(), thinking_config);
        }
        if let Some(response_modalities) = gemini_value_by_case(
            generation_config,
            "responseModalities",
            "response_modalities",
        )
        .cloned()
        {
            gemini_extension.insert("response_modalities".to_string(), response_modalities);
        }
        let extra = gemini_generation_config_extra(generation_config);
        if !extra.is_empty() {
            gemini_extension.insert("generation_config_extra".to_string(), Value::Object(extra));
        }
    }
    if let Some(value) = request
        .get("safetySettings")
        .or_else(|| request.get("safety_settings"))
        .cloned()
    {
        canonical_extension_object_mut(&mut canonical.extensions, "gemini")
            .insert("safety_settings".to_string(), value);
    }
    if let Some(value) = request
        .get("cachedContent")
        .or_else(|| request.get("cached_content"))
        .cloned()
    {
        canonical_extension_object_mut(&mut canonical.extensions, "gemini")
            .insert("cached_content".to_string(), value);
    }
    if let Some(raw_tools) = raw_tools {
        canonical_extension_object_mut(&mut canonical.extensions, "gemini")
            .insert("raw_tools".to_string(), raw_tools);
    }
    if !builtin_tools.is_empty() {
        canonical_extension_object_mut(&mut canonical.extensions, "gemini")
            .insert("builtin_tools".to_string(), Value::Array(builtin_tools));
    }
    if let Some(google_search_grounding) = google_search_grounding {
        let gemini_extension = canonical_extension_object_mut(&mut canonical.extensions, "gemini");
        gemini_extension.insert(
            "grounding".to_string(),
            json!({ "google_search": google_search_grounding }),
        );
    }
    if let Some(tool_config) = request
        .get("toolConfig")
        .or_else(|| request.get("tool_config"))
        .cloned()
    {
        canonical_extension_object_mut(&mut canonical.extensions, "gemini")
            .insert("raw_tool_config".to_string(), tool_config);
    }
    if let Some(web_search_options) = web_search_options {
        canonical_extension_object_mut(&mut canonical.extensions, "openai")
            .insert("web_search_options".to_string(), web_search_options);
    }
    Some(canonical)
}

pub fn to_raw(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    to_raw_with_schema_policy(canonical, mapped_model, upstream_is_stream, false)
}

fn to_raw_with_schema_policy(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    upstream_is_stream: bool,
    preserve_tool_schemas: bool,
) -> Option<Value> {
    let mut output = canonical_to_gemini_request_body(
        canonical,
        mapped_model,
        upstream_is_stream,
        preserve_tool_schemas,
    )?;
    apply_gemini_request_extensions(&mut output, &canonical.extensions)?;
    // These models ignore sampling controls and reject legacy candidate counts.
    // Native requests bypass conversion and retain their original validation.
    if crate::gemini_model_requires_function_response_ids(mapped_model) {
        if let Some(config) = output
            .get_mut("generationConfig")
            .and_then(Value::as_object_mut)
        {
            for key in [
                "temperature",
                "topP",
                "topK",
                "candidateCount",
                "frequencyPenalty",
                "presencePenalty",
            ] {
                config.remove(key);
            }
        }
    }
    if !canonical_has_raw_gemini_tools(canonical) {
        enable_server_side_tool_invocations_for_mixed_tools(&mut output, mapped_model)?;
    }
    Some(output)
}

fn canonical_has_raw_gemini_tools(canonical: &CanonicalRequest) -> bool {
    canonical
        .extensions
        .get("gemini")
        .and_then(Value::as_object)
        .is_some_and(|gemini| gemini.contains_key("raw_tools"))
}

fn enable_server_side_tool_invocations_for_mixed_tools(
    output: &mut Value,
    mapped_model: &str,
) -> Option<()> {
    let output_object = output.as_object_mut()?;
    let tools = output_object.get("tools").and_then(Value::as_array);
    let Some(tools) = tools else {
        return Some(());
    };
    if !gemini_tools_are_mixed(tools) {
        return Some(());
    }
    if !gemini_model_supports_mixed_tools(mapped_model) {
        return None;
    }
    ensure_server_side_tool_invocations_for_mixed_tools(output)
}

pub fn ensure_server_side_tool_invocations_for_mixed_tools(output: &mut Value) -> Option<()> {
    let output_object = output.as_object_mut()?;
    let tools = output_object.get("tools").and_then(Value::as_array);
    let Some(tools) = tools else {
        return Some(());
    };
    if !gemini_tools_are_mixed(tools) {
        return Some(());
    }

    let tool_config = output_object
        .entry("toolConfig".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;
    tool_config.remove("include_server_side_tool_invocations");
    tool_config.insert(
        "includeServerSideToolInvocations".to_string(),
        Value::Bool(true),
    );
    Some(())
}

pub(crate) fn canonical_has_mixed_gemini_tools(canonical: &CanonicalRequest) -> bool {
    // Only tool kinds matter here; do not lower/expand schemas just to count them.
    canonical_tools_to_gemini(canonical, true)
        .and_then(|tools| tools.as_array().cloned())
        .is_some_and(|tools| gemini_tools_are_mixed(&tools))
}

fn gemini_tools_are_mixed(tools: &[Value]) -> bool {
    let has_function_declarations = tools.iter().any(|tool| {
        tool.as_object().is_some_and(|tool| {
            tool.get("functionDeclarations")
                .or_else(|| tool.get("function_declarations"))
                .and_then(Value::as_array)
                .is_some_and(|declarations| !declarations.is_empty())
        })
    });
    let has_builtin_tools = tools.iter().any(|tool| {
        tool.as_object().is_some_and(|tool| {
            tool.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "functionDeclarations" | "function_declarations"
                )
            })
        })
    });
    has_function_declarations && has_builtin_tools
}

fn canonical_to_gemini_request_body(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    _upstream_is_stream: bool,
    preserve_tool_schemas: bool,
) -> Option<Value> {
    let mut output = Map::new();
    if !mapped_model.trim().is_empty() {
        output.insert(
            "model".to_string(),
            Value::String(mapped_model.trim().to_string()),
        );
    }
    output.insert(
        "contents".to_string(),
        Value::Array(compact_gemini_contents(
            canonical_messages_to_gemini_contents(&canonical.messages)?,
        )),
    );

    if let Some(system_instruction) = canonical_system_instruction(canonical) {
        output.insert("systemInstruction".to_string(), system_instruction);
    }
    if let Some(generation_config) = canonical_generation_config_to_gemini(canonical, mapped_model)
    {
        output.insert("generationConfig".to_string(), generation_config);
    }
    if let Some(tools) = canonical_tools_to_gemini(canonical, preserve_tool_schemas) {
        output.insert("tools".to_string(), tools);
    }
    if let Some(tool_config) = canonical_tool_choice_to_gemini(canonical.tool_choice.as_ref()) {
        output.insert("toolConfig".to_string(), tool_config);
    }
    Some(Value::Object(output))
}

fn canonical_system_instruction(canonical: &CanonicalRequest) -> Option<Value> {
    let text = canonical
        .instructions
        .iter()
        .map(|instruction| instruction.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let text = if text.trim().is_empty() {
        canonical.system.as_deref().unwrap_or_default().to_string()
    } else {
        text
    };
    (!text.trim().is_empty()).then(|| json!({ "parts": [{ "text": text }] }))
}

fn canonical_messages_to_gemini_contents(messages: &[CanonicalMessage]) -> Option<Vec<Value>> {
    let mut contents = Vec::new();
    let mut tool_name_by_id = BTreeMap::new();
    let mut pending_tool_use_ids = Vec::new();
    let mut message_index = 0;
    while message_index < messages.len() {
        let role = match messages[message_index].role {
            CanonicalRole::Assistant => "model",
            CanonicalRole::System | CanonicalRole::Developer => {
                message_index += 1;
                continue;
            }
            CanonicalRole::Tool | CanonicalRole::User | CanonicalRole::Unknown => "user",
        };
        let mut blocks = Vec::new();
        while message_index < messages.len() {
            let next_role = match messages[message_index].role {
                CanonicalRole::Assistant => Some("model"),
                CanonicalRole::Tool | CanonicalRole::User | CanonicalRole::Unknown => Some("user"),
                CanonicalRole::System | CanonicalRole::Developer => None,
            };
            match next_role {
                Some(next_role) if next_role == role => {
                    blocks.extend(messages[message_index].content.iter());
                    message_index += 1;
                }
                None => message_index += 1,
                Some(_) => break,
            }
        }

        let blocks = if role == "user" {
            let aligned = align_gemini_tool_results(blocks, &pending_tool_use_ids);
            pending_tool_use_ids.clear();
            aligned
        } else {
            pending_tool_use_ids = blocks
                .iter()
                .filter_map(|block| match block {
                    CanonicalContentBlock::ToolUse { id, .. } if !id.trim().is_empty() => {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .collect();
            blocks
        };
        let parts = canonical_blocks_to_gemini_parts(&blocks, &mut tool_name_by_id)?;
        if parts.is_empty() {
            continue;
        }
        contents.push(json!({
            "role": role,
            "parts": parts,
        }));
    }
    Some(contents)
}

fn align_gemini_tool_results<'a>(
    blocks: Vec<&'a CanonicalContentBlock>,
    pending_tool_use_ids: &[String],
) -> Vec<&'a CanonicalContentBlock> {
    if pending_tool_use_ids.is_empty() {
        return blocks;
    }

    let result_indexes = blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| match block {
            CanonicalContentBlock::ToolResult { extensions, .. }
                if is_cross_format_tool_result(extensions) =>
            {
                Some(index)
            }
            CanonicalContentBlock::ToolResult { .. } => None,
            _ => None,
        })
        .collect::<Vec<_>>();
    if result_indexes.len() != pending_tool_use_ids.len()
        || blocks
            .iter()
            .filter(|block| matches!(block, CanonicalContentBlock::ToolResult { .. }))
            .count()
            != result_indexes.len()
    {
        return blocks;
    }

    let mut ordered = Vec::with_capacity(blocks.len());
    let mut used = vec![false; result_indexes.len()];
    for pending_id in pending_tool_use_ids {
        if pending_id.trim().is_empty() {
            return blocks;
        }
        let Some((result_position, block_index)) =
            result_indexes
                .iter()
                .enumerate()
                .find(|(result_position, block_index)| {
                    if used[*result_position] {
                        return false;
                    }
                    matches!(
                        blocks[**block_index],
                        CanonicalContentBlock::ToolResult { ref tool_use_id, .. }
                            if tool_use_id == pending_id
                    )
                })
        else {
            return blocks;
        };
        used[result_position] = true;
        ordered.push(blocks[*block_index]);
    }
    ordered.extend(
        blocks
            .iter()
            .copied()
            .filter(|block| !matches!(block, CanonicalContentBlock::ToolResult { .. })),
    );
    ordered
}

fn canonical_blocks_to_gemini_parts(
    blocks: &[&CanonicalContentBlock],
    tool_name_by_id: &mut BTreeMap<String, String>,
) -> Option<Vec<Value>> {
    let mut parts = Vec::new();
    let mut saw_tool_use = false;
    for block in blocks {
        let is_first_tool_use =
            matches!(block, CanonicalContentBlock::ToolUse { .. }) && !saw_tool_use;
        if let Some(part) =
            canonical_block_to_gemini_part(block, tool_name_by_id, is_first_tool_use)?
        {
            parts.push(part);
        }
        saw_tool_use |= matches!(block, CanonicalContentBlock::ToolUse { .. });
    }
    Some(parts)
}

fn canonical_block_to_gemini_part(
    block: &CanonicalContentBlock,
    tool_name_by_id: &mut BTreeMap<String, String>,
    is_first_tool_use: bool,
) -> Option<Option<Value>> {
    match block {
        CanonicalContentBlock::Text { text, extensions } => {
            let mut part = json!({ "text": text });
            if let Some(signature) = extensions
                .get("gemini")
                .and_then(|value| {
                    value
                        .get("thoughtSignature")
                        .or_else(|| value.get("thought_signature"))
                })
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                part["thoughtSignature"] = Value::String(signature.to_string());
            }
            Some(Some(part))
        }
        CanonicalContentBlock::Thinking {
            text, signature, ..
        } => {
            if text.trim().is_empty() && signature.as_ref().is_none_or(String::is_empty) {
                return Some(None);
            }
            let mut part = Map::new();
            part.insert("text".to_string(), Value::String(text.clone()));
            part.insert("thought".to_string(), Value::Bool(true));
            if let Some(signature) = signature.as_ref().filter(|value| !value.is_empty()) {
                part.insert(
                    "thoughtSignature".to_string(),
                    Value::String(signature.clone()),
                );
            }
            Some(Some(Value::Object(part)))
        }
        CanonicalContentBlock::Image {
            data,
            url,
            media_type,
            ..
        } => Some(Some(canonical_media_to_gemini_part(
            media_type.as_deref().unwrap_or("image/png"),
            data.as_deref(),
            url.as_deref(),
        ))),
        CanonicalContentBlock::File {
            data,
            file_url,
            media_type,
            ..
        } => Some(Some(canonical_media_to_gemini_part(
            media_type.as_deref().unwrap_or("application/octet-stream"),
            data.as_deref(),
            file_url.as_deref(),
        ))),
        CanonicalContentBlock::Audio {
            data, media_type, ..
        } => Some(data.as_ref().map(|data| {
            json!({
                "inlineData": {
                    "mimeType": media_type.clone().unwrap_or_else(|| "audio/mpeg".to_string()),
                    "data": data,
                }
            })
        })),
        CanonicalContentBlock::ToolUse {
            id,
            name,
            input,
            extensions,
        } => {
            tool_name_by_id.insert(id.clone(), name.clone());
            let mut part = json!({
                "functionCall": {
                    "id": id,
                    "name": name,
                    "args": gemini_function_args(input),
                }
            });
            let signature = extensions
                .get("gemini")
                .and_then(Value::as_object)
                .and_then(|gemini| {
                    gemini
                        .get("thoughtSignature")
                        .or_else(|| gemini.get("thought_signature"))
                })
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .or_else(|| is_first_tool_use.then_some("skip_thought_signature_validator"));
            if let Some(signature) = signature {
                part.as_object_mut()?.insert(
                    "thoughtSignature".to_string(),
                    Value::String(signature.to_string()),
                );
            }
            Some(Some(part))
        }
        CanonicalContentBlock::ToolResult {
            tool_use_id,
            name,
            output,
            content_text,
            ..
        } => Some(Some(json!({
            "functionResponse": {
                "id": tool_use_id,
                "name": name.clone()
                    .or_else(|| tool_name_by_id.get(tool_use_id).cloned())
                    .unwrap_or_else(|| tool_use_id.clone()),
                "response": gemini_function_response(output.as_ref(), content_text.as_deref()),
            }
        }))),
        CanonicalContentBlock::Unknown { .. } => Some(None),
    }
}

fn canonical_media_to_gemini_part(
    media_type: &str,
    data: Option<&str>,
    url: Option<&str>,
) -> Value {
    if let Some(data) = data.filter(|value| !value.is_empty()) {
        return json!({
            "inlineData": {
                "mimeType": media_type,
                "data": data,
            }
        });
    }
    json!({
        "fileData": {
            "mimeType": media_type,
            "fileUri": url.unwrap_or_default(),
        }
    })
}

fn canonical_generation_config_to_gemini(
    canonical: &CanonicalRequest,
    mapped_model: &str,
) -> Option<Value> {
    let mut generation_config = Map::new();
    if let Some(value) = canonical.generation.max_tokens {
        generation_config.insert("maxOutputTokens".to_string(), Value::from(value));
    }
    insert_f64(
        &mut generation_config,
        "temperature",
        canonical.generation.temperature,
    );
    insert_f64(&mut generation_config, "topP", canonical.generation.top_p);
    if let Some(value) = canonical.generation.top_k {
        generation_config.insert("topK".to_string(), Value::from(value));
    }
    if let Some(value) = canonical.generation.n.filter(|value| *value > 1) {
        generation_config.insert("candidateCount".to_string(), Value::from(value));
    }
    if let Some(value) = canonical.generation.seed {
        generation_config.insert("seed".to_string(), Value::from(value));
    }
    if let Some(stop_sequences) = &canonical.generation.stop_sequences {
        generation_config.insert(
            "stopSequences".to_string(),
            Value::Array(stop_sequences.iter().cloned().map(Value::String).collect()),
        );
    }
    if let Some(response_format) = &canonical.response_format {
        apply_response_format_to_gemini_generation_config(&mut generation_config, response_format);
    }
    if let Some(thinking_config) = canonical.thinking.as_ref().and_then(|thinking| {
        thinking
            .extensions
            .get("gemini")
            .and_then(|value| value.get("thinking_config"))
            .cloned()
            .or_else(|| {
                let effort = canonical_openai_reasoning_effort(thinking);
                gemini_thinking_config_from_reasoning(mapped_model, effort, thinking.budget_tokens)
            })
    }) {
        generation_config.insert("thinkingConfig".to_string(), thinking_config);
    }
    (!generation_config.is_empty()).then_some(Value::Object(generation_config))
}

fn gemini_thinking_config_from_reasoning(
    mapped_model: &str,
    effort: Option<&str>,
    budget_tokens: Option<u64>,
) -> Option<Value> {
    if gemini_model_uses_thinking_level(mapped_model) {
        let level = effort
            .and_then(ReasoningEffort::parse)
            .or_else(|| {
                budget_tokens
                    .map(map_thinking_budget_to_openai_reasoning_effort)
                    .and_then(ReasoningEffort::parse)
            })
            .map(|effort| effort.as_gemini_level_value_for_model(mapped_model))?;
        return Some(json!({
            "includeThoughts": true,
            "thinkingLevel": level,
        }));
    }

    let budget =
        budget_tokens.or_else(|| effort.and_then(map_openai_reasoning_effort_to_gemini_budget))?;
    Some(json!({
        "includeThoughts": true,
        "thinkingBudget": budget,
    }))
}

fn apply_response_format_to_gemini_generation_config(
    generation_config: &mut Map<String, Value>,
    response_format: &CanonicalResponseFormat,
) {
    match response_format.format_type.as_str() {
        "json_schema" => {
            generation_config.insert(
                "responseMimeType".to_string(),
                Value::String("application/json".to_string()),
            );
            if let Some(schema) = response_format
                .json_schema
                .as_ref()
                .and_then(|value| value.get("schema"))
                .cloned()
                .or_else(|| response_format.json_schema.clone())
            {
                let mut schema = schema;
                clean_gemini_schema(&mut schema);
                generation_config.insert("responseSchema".to_string(), schema);
            }
        }
        "json_object" => {
            generation_config.insert(
                "responseMimeType".to_string(),
                Value::String("application/json".to_string()),
            );
        }
        _ => {}
    }
}

fn canonical_tools_to_gemini(
    canonical: &CanonicalRequest,
    preserve_tool_schemas: bool,
) -> Option<Value> {
    let mut declarations = Vec::new();
    let mut tools = Vec::new();
    let mut google_search = canonical
        .extensions
        .get("openai")
        .and_then(Value::as_object)
        .is_some_and(|value| value.contains_key("web_search_options"));
    let mut google_search_payload = canonical_google_search_output_payload(canonical);
    if google_search_payload.is_some() {
        google_search = true;
    }
    let mut code_execution = false;
    let mut url_context = false;

    for tool in &canonical.tools {
        match canonical_tool_builtin_gemini_name(tool) {
            Some("googleSearch") => {
                google_search = true;
                continue;
            }
            Some("codeExecution") => {
                code_execution = true;
                continue;
            }
            Some("urlContext") => {
                url_context = true;
                continue;
            }
            Some(_) => continue,
            None => {}
        }
        if tool
            .extensions
            .get("openai_responses")
            .or_else(|| {
                tool.extensions
                    .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
            })
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|tool_type| tool_type.starts_with("web_search"))
        {
            google_search = true;
            continue;
        }
        declarations.push(canonical_tool_to_gemini_declaration(
            tool,
            preserve_tool_schemas,
        ));
    }
    let mut emitted_google_search = false;
    let mut emitted_code_execution = false;
    let mut emitted_url_context = false;
    if let Some(builtin_tools) = canonical
        .extensions
        .get("gemini")
        .and_then(Value::as_object)
        .and_then(|value| value.get("builtin_tools"))
        .and_then(Value::as_array)
    {
        for builtin_tool in builtin_tools {
            let Some(tool_object) = builtin_tool.as_object() else {
                tools.push(builtin_tool.clone());
                continue;
            };
            let mut emitted_builtin_portion = false;
            if let Some(grounding) = gemini_google_search_grounding(tool_object) {
                google_search = true;
                if google_search_payload.is_none() {
                    google_search_payload = Some(grounding.output_payload);
                }
                if !emitted_google_search {
                    tools.push(json!({
                        "googleSearch": google_search_payload.clone().unwrap_or_else(|| json!({}))
                    }));
                    emitted_google_search = true;
                }
                emitted_builtin_portion = true;
            }
            if let Some(tool) =
                gemini_builtin_tool_by_case(tool_object, "codeExecution", "code_execution")
            {
                if !emitted_code_execution {
                    tools.push(tool);
                    emitted_code_execution = true;
                }
                emitted_builtin_portion = true;
            }
            if let Some(tool) =
                gemini_builtin_tool_by_case(tool_object, "urlContext", "url_context")
            {
                if !emitted_url_context {
                    tools.push(tool);
                    emitted_url_context = true;
                }
                emitted_builtin_portion = true;
            }
            if let Some(tool) = gemini_unhandled_builtin_tool_portion(tool_object) {
                tools.push(tool);
            } else if !emitted_builtin_portion {
                tools.push(builtin_tool.clone());
            }
        }
    }
    if code_execution && !emitted_code_execution {
        tools.push(json!({ "codeExecution": {} }));
    }
    if google_search && !emitted_google_search {
        tools.push(json!({
            "googleSearch": google_search_payload.unwrap_or_else(|| json!({}))
        }));
    }
    if url_context && !emitted_url_context {
        tools.push(json!({ "urlContext": {} }));
    }
    if !declarations.is_empty() {
        tools.push(json!({ "functionDeclarations": declarations }));
    }
    (!tools.is_empty()).then_some(Value::Array(tools))
}

fn canonical_google_search_output_payload(canonical: &CanonicalRequest) -> Option<Value> {
    let google_search = canonical
        .extensions
        .get("gemini")
        .and_then(Value::as_object)
        .and_then(|value| value.get("grounding"))
        .and_then(Value::as_object)
        .and_then(|value| value.get("google_search"))
        .and_then(Value::as_object)?;
    google_search
        .get("legacy")
        .and_then(Value::as_bool)
        .filter(|legacy| *legacy)
        .map(|_| json!({}))
        .or_else(|| google_search.get("payload").cloned())
}

fn gemini_builtin_tool_by_case(
    tool_object: &Map<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<Value> {
    let payload = tool_object
        .get(camel)
        .or_else(|| tool_object.get(snake))
        .map(gemini_builtin_tool_payload)?;
    Some(json!({ camel: payload }))
}

fn gemini_builtin_tool_payload(payload: &Value) -> Value {
    match payload {
        Value::Null => json!({}),
        value => value.clone(),
    }
}

fn gemini_unhandled_builtin_tool_portion(tool_object: &Map<String, Value>) -> Option<Value> {
    let builtin = tool_object
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "googleSearch"
                    | "google_search"
                    | "googleSearchRetrieval"
                    | "google_search_retrieval"
                    | "codeExecution"
                    | "code_execution"
                    | "urlContext"
                    | "url_context"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    (!builtin.is_empty()).then_some(Value::Object(builtin))
}

fn canonical_tool_to_gemini_declaration(
    tool: &CanonicalToolDefinition,
    preserve_tool_schema: bool,
) -> Value {
    let mut declaration = Map::new();
    declaration.insert("name".to_string(), Value::String(tool.name.clone()));
    if let Some(description) = &tool.description {
        declaration.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    let raw_parameters = tool
        .extensions
        .get("gemini")
        .and_then(Value::as_object)
        .and_then(|value| value.get("raw_parameters"))
        .cloned();
    declaration.insert(
        "parameters".to_string(),
        raw_parameters
            .clone()
            .or_else(|| tool.parameters.clone())
            .map(|mut schema| {
                if raw_parameters.is_none() && !preserve_tool_schema {
                    clean_gemini_schema(&mut schema);
                }
                schema
            })
            .unwrap_or_else(|| json!({})),
    );
    Value::Object(declaration)
}

fn canonical_tool_choice_to_gemini(choice: Option<&CanonicalToolChoice>) -> Option<Value> {
    let choice = choice?;
    let mode = match choice {
        CanonicalToolChoice::Auto => "AUTO",
        CanonicalToolChoice::None => "NONE",
        CanonicalToolChoice::Required | CanonicalToolChoice::Tool { .. } => "ANY",
    };
    let mut function_calling_config = Map::new();
    function_calling_config.insert("mode".to_string(), Value::String(mode.to_string()));
    if let CanonicalToolChoice::Tool { name } = choice {
        function_calling_config.insert(
            "allowedFunctionNames".to_string(),
            Value::Array(vec![Value::String(name.clone())]),
        );
    }
    Some(json!({
        "functionCallingConfig": Value::Object(function_calling_config),
    }))
}

fn gemini_function_args(input: &Value) -> Value {
    match input {
        Value::Object(_) => input.clone(),
        Value::Null => json!({}),
        other => json!({ "value": other.clone() }),
    }
}

fn gemini_function_response(output: Option<&Value>, content_text: Option<&str>) -> Value {
    match output {
        Some(value) => json!({ "result": value }),
        None => json!({ "result": content_text.unwrap_or_default() }),
    }
}

fn compact_gemini_contents(contents: Vec<Value>) -> Vec<Value> {
    let mut compact: Vec<Value> = Vec::new();
    for content in contents {
        let role = content
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let parts = content
            .get("parts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if parts.is_empty() {
            continue;
        }
        if let Some(last) = compact.last_mut() {
            let last_role = last.get("role").and_then(Value::as_str).unwrap_or_default();
            if last_role == role {
                if let Some(last_parts) = last
                    .as_object_mut()
                    .and_then(|object| object.get_mut("parts"))
                    .and_then(Value::as_array_mut)
                {
                    last_parts.extend(parts);
                    continue;
                }
            }
        }
        compact.push(json!({
            "role": role,
            "parts": parts,
        }));
    }
    compact
}

/// Promote a canonical tool to a Gemini builtin only when it is a bare marker.
///
/// Clients declare ordinary function tools whose names collide with the builtin
/// spellings — Claude Code ships a client-side `WebSearch` tool with a full
/// `input_schema`. Matching on the name alone dropped those declarations and
/// replaced them with server-side grounding, so the model could never call the
/// tool the client actually implements. A declared schema means the caller
/// expects to execute the call itself, so such tools stay function declarations.
fn canonical_tool_builtin_gemini_name(tool: &CanonicalToolDefinition) -> Option<&'static str> {
    if tool
        .parameters
        .as_ref()
        .is_some_and(|parameters| !parameters.is_null())
    {
        return None;
    }
    normalize_gemini_builtin_tool_name(&tool.name)
}

fn normalize_gemini_builtin_tool_name(name: &str) -> Option<&'static str> {
    match name
        .trim()
        .replace(['_', '-', ' '], "")
        .to_ascii_lowercase()
        .as_str()
    {
        "googlesearch" | "websearch" | "websearchpreview" => Some("googleSearch"),
        "codeexecution" => Some("codeExecution"),
        "urlcontext" => Some("urlContext"),
        _ => None,
    }
}

fn insert_f64(output: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value.and_then(serde_json::Number::from_f64) {
        output.insert(key.to_string(), Value::Number(value));
    }
}

fn clean_gemini_schema(value: &mut Value) {
    let root = value.clone();
    *value = json_schema_to_gemini_schema(&root, &root, &mut BTreeSet::new());
}

fn json_schema_to_gemini_schema(
    value: &Value,
    root: &Value,
    resolving_refs: &mut BTreeSet<String>,
) -> Value {
    let Some(object) = value.as_object() else {
        return json!({});
    };

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        if let Some(pointer) = reference.strip_prefix('#') {
            if resolving_refs.insert(reference.to_string()) {
                if let Some(resolved) = root.pointer(pointer).and_then(Value::as_object) {
                    let mut merged = resolved.clone();
                    for (key, value) in object {
                        if key != "$ref" {
                            merged.insert(key.clone(), value.clone());
                        }
                    }
                    let schema = clean_gemini_schema_object(&merged, root, resolving_refs);
                    resolving_refs.remove(reference);
                    return Value::Object(schema);
                }
                resolving_refs.remove(reference);
            }
        }
    }

    Value::Object(clean_gemini_schema_object(object, root, resolving_refs))
}

fn clean_gemini_schema_object(
    object: &Map<String, Value>,
    root: &Value,
    resolving_refs: &mut BTreeSet<String>,
) -> Map<String, Value> {
    let mut schema = Map::new();

    for key in ["title", "description", "format", "pattern"] {
        if let Some(value) = object.get(key).filter(|value| value.is_string()) {
            schema.insert(key.to_string(), value.clone());
        }
    }
    for key in ["default", "example"] {
        if let Some(value) = object.get(key) {
            schema.insert(key.to_string(), value.clone());
        }
    }
    for key in ["minimum", "maximum"] {
        if let Some(value) = object.get(key).filter(|value| value.is_number()) {
            schema.insert(key.to_string(), value.clone());
        }
    }
    for key in [
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "minProperties",
        "maxProperties",
    ] {
        if let Some(value) = object.get(key).and_then(gemini_int64_string) {
            schema.insert(key.to_string(), Value::String(value));
        }
    }
    if let Some(value) = object.get("nullable").filter(|value| value.is_boolean()) {
        schema.insert("nullable".to_string(), value.clone());
    }
    for key in ["required", "propertyOrdering"] {
        if let Some(values) = object.get(key).and_then(gemini_string_array) {
            schema.insert(key.to_string(), values);
        }
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        let values = values
            .iter()
            .filter(|value| value.is_string())
            .cloned()
            .collect::<Vec<_>>();
        if !values.is_empty() {
            schema.insert("enum".to_string(), Value::Array(values));
        }
    }
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        schema.insert(
            "properties".to_string(),
            Value::Object(
                properties
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.clone(),
                            json_schema_to_gemini_schema(value, root, resolving_refs),
                        )
                    })
                    .collect(),
            ),
        );
    }
    if let Some(items) = object.get("items") {
        schema.insert(
            "items".to_string(),
            json_schema_to_gemini_schema(items, root, resolving_refs),
        );
    }

    let explicit_any_of = object
        .get("anyOf")
        .or_else(|| object.get("oneOf"))
        .and_then(Value::as_array)
        .map(|items| {
            Value::Array(
                items
                    .iter()
                    .map(|item| json_schema_to_gemini_schema(item, root, resolving_refs))
                    .collect(),
            )
        });
    if let Some(any_of) = explicit_any_of {
        schema.insert("anyOf".to_string(), any_of);
    }

    match object.get("type") {
        Some(Value::String(schema_type)) => {
            schema.insert("type".to_string(), Value::String(schema_type.clone()));
        }
        Some(Value::Array(types)) => {
            let mut non_null_types = types
                .iter()
                .filter_map(Value::as_str)
                .filter(|schema_type| *schema_type != "null")
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            let mut seen_types = BTreeSet::new();
            non_null_types.retain(|schema_type| seen_types.insert(schema_type.clone()));
            let nullable = types.iter().any(|value| value.as_str() == Some("null"));

            match non_null_types.as_slice() {
                [schema_type] => {
                    schema.insert("type".to_string(), Value::String(schema_type.clone()));
                }
                [] if nullable => {
                    schema.insert("type".to_string(), Value::String("null".to_string()));
                }
                [] => {}
                _ if !schema.contains_key("anyOf") => {
                    schema.insert(
                        "anyOf".to_string(),
                        Value::Array(
                            non_null_types
                                .iter()
                                .map(|schema_type| json!({ "type": schema_type }))
                                .collect(),
                        ),
                    );
                }
                _ => {}
            }
            if nullable && !non_null_types.is_empty() {
                schema.insert("nullable".to_string(), Value::Bool(true));
            }
        }
        _ => {}
    }

    if schema.get("type").and_then(Value::as_str) == Some("object")
        && !schema.contains_key("properties")
    {
        schema.insert("properties".to_string(), Value::Object(Map::new()));
    }
    schema
}

fn gemini_int64_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn gemini_string_array(value: &Value) -> Option<Value> {
    let values = value.as_array()?;
    values.iter().all(Value::is_string).then(|| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CanonicalContentBlock;

    #[test]
    fn gemini_four_and_flash_38_convert_reasoning_and_omit_legacy_sampling() {
        for model in [
            "gemini-4-carbon",
            "gemini-4-argon",
            "gemini-4-hydrogen",
            "gemini-3.8-flash",
        ] {
            let canonical = crate::formats::openai::chat::request::from_raw(&json!({
                "model": model, "messages": [{"role":"user","content":"hello"}],
                "reasoning_effort":"high", "temperature":0.3, "top_p":0.8, "n":2
            }))
            .unwrap();
            let output = to_raw(&canonical, model, false).unwrap();
            assert_eq!(
                output["generationConfig"]["thinkingConfig"]["thinkingLevel"],
                "high"
            );
            assert!(output["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none());
            for key in ["temperature", "topP", "candidateCount"] {
                assert!(
                    output["generationConfig"].get(key).is_none(),
                    "{model}: {key}"
                );
            }
        }
        assert!(!gemini_model_uses_thinking_level("gemini-2.5-flash"));
    }

    #[test]
    fn hydrogen_preserves_xhigh_from_openai_reasoning() {
        for effort in ["xhigh", "max"] {
            let canonical = crate::formats::openai::chat::request::from_raw(&json!({
                "model": "gemini-4-hydrogen",
                "messages": [{"role":"user","content":"solve"}],
                "reasoning_effort": effort
            }))
            .unwrap();
            let output = to_raw(&canonical, "gemini-4-hydrogen", false).unwrap();
            assert_eq!(
                output["generationConfig"]["thinkingConfig"]["thinkingLevel"],
                "xhigh"
            );
            assert!(output["generationConfig"]["thinkingConfig"]
                .get("thinkingBudget")
                .is_none());
        }
    }

    #[test]
    fn roundtrip_keeps_text_and_empty_thought_signatures_separate() {
        let request = from_raw(
            &json!({"contents":[{"role":"model","parts":[
                {"text":"", "thought":true, "thoughtSignature":"opaque-thought"},
                {"text":"answer", "thoughtSignature":"opaque-text"},
                {"text":"unsigned"}
            ]}]}),
            "/v1beta/models/gemini-4-carbon:generateContent",
        )
        .unwrap();
        let output = to_raw(&request, "gemini-4-carbon", false).unwrap();
        let parts = output["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["thoughtSignature"], "opaque-thought");
        assert_eq!(parts[1]["thoughtSignature"], "opaque-text");
        assert!(parts[2].get("thoughtSignature").is_none());
    }

    #[test]
    fn canonical_tool_declaration_sanitizes_json_schema_for_gemini() {
        let declaration = canonical_tool_to_gemini_declaration(
            &CanonicalToolDefinition {
                name: "inspect".to_string(),
                description: None,
                parameters: Some(json!({
                    "$defs": {
                        "Target": {
                            "type": "object",
                            "properties": {
                                "secret": {
                                    "type": "string",
                                    "encrypted": true
                                }
                            },
                            "required": ["secret"],
                            "additionalProperties": false
                        }
                    },
                    "type": "object",
                    "properties": {
                        "target": {
                            "oneOf": [
                                {"$ref": "#/$defs/Target"},
                                {"type": "null"}
                            ]
                        },
                        "mode": {
                            "type": ["string", "null"],
                            "enum": [1, "fast"]
                        },
                        "value": {
                            "type": ["string", "integer"]
                        }
                    }
                })),
                strict: None,
                extensions: BTreeMap::new(),
            },
            false,
        );

        assert_eq!(
            declaration["parameters"],
            json!({
                "type": "object",
                "properties": {
                    "target": {
                        "anyOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "secret": {"type": "string"}
                                },
                                "required": ["secret"]
                            },
                            {"type": "null"}
                        ]
                    },
                    "mode": {
                        "type": "string",
                        "nullable": true,
                        "enum": ["fast"]
                    },
                    "value": {
                        "anyOf": [
                            {"type": "string"},
                            {"type": "integer"}
                        ]
                    }
                }
            })
        );
    }

    #[test]
    fn canonical_tool_result_to_gemini_request_preserves_function_response_id() {
        let mut tool_name_by_id = BTreeMap::new();
        tool_name_by_id.insert("call_1".to_string(), "lookup".to_string());

        let part = canonical_block_to_gemini_part(
            &CanonicalContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                name: None,
                output: Some(serde_json::json!({"ok": true})),
                content_text: None,
                is_error: false,
                extensions: BTreeMap::new(),
            },
            &mut tool_name_by_id,
            false,
        )
        .expect("part should be representable")
        .expect("part should not be omitted");

        let function_response = part
            .get("functionResponse")
            .and_then(Value::as_object)
            .expect("functionResponse should exist");

        assert_eq!(function_response["id"], "call_1");
        assert_eq!(function_response["name"], "lookup");
        assert_eq!(
            function_response["response"],
            serde_json::json!({"result": {"ok": true}})
        );
    }

    #[test]
    fn mixed_builtin_and_function_tools_require_gemini_three() {
        let canonical = CanonicalRequest {
            model: "gemini-2.5-pro".to_string(),
            tools: vec![CanonicalToolDefinition {
                name: "save_result".to_string(),
                description: None,
                parameters: Some(json!({"type": "object"})),
                strict: None,
                extensions: BTreeMap::new(),
            }],
            extensions: BTreeMap::from([(
                "gemini".to_string(),
                json!({"builtin_tools": [{"googleSearch": {}}]}),
            )]),
            ..CanonicalRequest::default()
        };

        assert!(to_raw(&canonical, "gemini-2.5-pro", false).is_none());
        assert!(to_raw(&canonical, "gemini-3-flash-preview", false).is_some());
    }

    #[test]
    fn client_declared_web_search_tool_stays_a_function_declaration() {
        let canonical = CanonicalRequest {
            model: "gemini-3-flash-preview".to_string(),
            tools: vec![CanonicalToolDefinition {
                name: "WebSearch".to_string(),
                description: Some("Search the web".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"],
                })),
                strict: None,
                extensions: BTreeMap::new(),
            }],
            ..CanonicalRequest::default()
        };

        for preserve_tool_schemas in [false, true] {
            let tools = canonical_tools_to_gemini(&canonical, preserve_tool_schemas)
                .expect("tools should be emitted");
            let tools = tools.as_array().expect("tools should be an array");

            assert!(
                tools.iter().all(|tool| tool.get("googleSearch").is_none()),
                "a client tool named WebSearch must not become server-side grounding: {tools:?}"
            );
            assert_eq!(
                tools[0]["functionDeclarations"][0]["name"], "WebSearch",
                "the client declaration must survive: {tools:?}"
            );
        }
    }

    #[test]
    fn schemaless_builtin_tool_name_still_maps_to_google_search() {
        let canonical = CanonicalRequest {
            model: "gemini-3-flash-preview".to_string(),
            tools: vec![CanonicalToolDefinition {
                name: "google_search".to_string(),
                description: None,
                parameters: None,
                strict: None,
                extensions: BTreeMap::new(),
            }],
            ..CanonicalRequest::default()
        };

        for preserve_tool_schemas in [false, true] {
            let tools = canonical_tools_to_gemini(&canonical, preserve_tool_schemas)
                .expect("tools should be emitted");
            let tools = tools.as_array().expect("tools should be an array");

            assert_eq!(tools.len(), 1, "{tools:?}");
            assert_eq!(tools[0]["googleSearch"], json!({}));
        }
    }
}
