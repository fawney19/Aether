use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::{
    protocol::canonical::{
        apply_gemini_request_extensions, canonical_extension_object_mut,
        canonical_openai_reasoning_effort, extract_gemini_model_from_path,
        gemini_contents_to_canonical_messages, gemini_extensions, gemini_generation_config,
        gemini_generation_config_extra, gemini_openai_extra_body,
        gemini_response_format_to_canonical, gemini_system_to_canonical_instructions,
        gemini_thinking_to_canonical, gemini_tool_choice_to_canonical, gemini_tools_to_canonical,
        gemini_value_by_case, CanonicalContentBlock, CanonicalMessage, CanonicalRequest,
        CanonicalResponseFormat, CanonicalRole, CanonicalToolChoice, CanonicalToolDefinition,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
    protocol::context::FormatContext,
    request::openai::map_openai_reasoning_effort_to_gemini_budget,
};

pub fn from(body: &Value, ctx: &FormatContext) -> Option<CanonicalRequest> {
    from_raw(body, ctx.request_path.as_deref().unwrap_or_default())
}

pub fn to(request: &CanonicalRequest, ctx: &FormatContext) -> Option<Value> {
    to_raw(
        request,
        ctx.mapped_model_or(request.model.as_str()),
        ctx.upstream_is_stream,
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
    let (tools, builtin_tools, web_search_options, raw_tools) =
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
    if let Some(tool_config) = request
        .get("toolConfig")
        .or_else(|| request.get("tool_config"))
        .cloned()
    {
        canonical_extension_object_mut(&mut canonical.extensions, "gemini")
            .insert("raw_tool_config".to_string(), tool_config);
    }
    if let Some(extra_body) = gemini_openai_extra_body(request) {
        canonical_extension_object_mut(&mut canonical.extensions, "openai")
            .insert("extra_body".to_string(), extra_body);
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
    let mut output = canonical_to_gemini_request_body(canonical, mapped_model, upstream_is_stream)?;
    apply_gemini_request_extensions(&mut output, &canonical.extensions)?;
    Some(output)
}

fn canonical_to_gemini_request_body(
    canonical: &CanonicalRequest,
    mapped_model: &str,
    _upstream_is_stream: bool,
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
    if let Some(generation_config) = canonical_generation_config_to_gemini(canonical) {
        output.insert("generationConfig".to_string(), generation_config);
    }
    if let Some(tools) = canonical_tools_to_gemini(canonical) {
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
    for message in messages {
        let role = match message.role {
            CanonicalRole::Assistant => "model",
            CanonicalRole::System | CanonicalRole::Developer => continue,
            CanonicalRole::Tool | CanonicalRole::User | CanonicalRole::Unknown => "user",
        };
        let parts = canonical_blocks_to_gemini_parts(&message.content, &mut tool_name_by_id)?;
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

fn canonical_blocks_to_gemini_parts(
    blocks: &[CanonicalContentBlock],
    tool_name_by_id: &mut BTreeMap<String, String>,
) -> Option<Vec<Value>> {
    let mut parts = Vec::new();
    for block in blocks {
        if let Some(part) = canonical_block_to_gemini_part(block, tool_name_by_id)? {
            parts.push(part);
        }
    }
    Some(parts)
}

fn canonical_block_to_gemini_part(
    block: &CanonicalContentBlock,
    tool_name_by_id: &mut BTreeMap<String, String>,
) -> Option<Option<Value>> {
    match block {
        CanonicalContentBlock::Text { text, .. } => Some(Some(json!({ "text": text }))),
        CanonicalContentBlock::Thinking {
            text, signature, ..
        } => {
            if text.trim().is_empty() {
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
            id, name, input, ..
        } => {
            tool_name_by_id.insert(id.clone(), name.clone());
            Some(Some(json!({
                "functionCall": {
                    "id": id,
                    "name": name,
                    "args": gemini_function_args(input),
                }
            })))
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

fn canonical_generation_config_to_gemini(canonical: &CanonicalRequest) -> Option<Value> {
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
                let budget = thinking.budget_tokens.or_else(|| {
                    canonical_openai_reasoning_effort(thinking)
                        .and_then(map_openai_reasoning_effort_to_gemini_budget)
                })?;
                Some(json!({
                    "includeThoughts": true,
                    "thinkingBudget": budget,
                }))
            })
    }) {
        generation_config.insert("thinkingConfig".to_string(), thinking_config);
    }
    (!generation_config.is_empty()).then_some(Value::Object(generation_config))
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

fn canonical_tools_to_gemini(canonical: &CanonicalRequest) -> Option<Value> {
    let mut declarations = Vec::new();
    let mut tools = Vec::new();
    let mut google_search = canonical
        .extensions
        .get("openai")
        .and_then(Value::as_object)
        .is_some_and(|value| value.contains_key("web_search_options"));
    let mut code_execution = false;
    let mut url_context = false;

    for tool in &canonical.tools {
        match normalize_gemini_builtin_tool_name(&tool.name) {
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
        declarations.push(canonical_tool_to_gemini_declaration(tool));
    }
    if code_execution {
        tools.push(json!({ "codeExecution": {} }));
    }
    if google_search {
        tools.push(json!({ "googleSearch": {} }));
    }
    if url_context {
        tools.push(json!({ "urlContext": {} }));
    }
    if !declarations.is_empty() {
        tools.push(json!({ "functionDeclarations": declarations }));
    }
    if let Some(builtin_tools) = canonical
        .extensions
        .get("gemini")
        .and_then(Value::as_object)
        .and_then(|value| value.get("builtin_tools"))
        .and_then(Value::as_array)
    {
        tools.extend(builtin_tools.iter().cloned());
    }
    (!tools.is_empty()).then_some(Value::Array(tools))
}

fn canonical_tool_to_gemini_declaration(tool: &CanonicalToolDefinition) -> Value {
    let mut declaration = Map::new();
    declaration.insert("name".to_string(), Value::String(tool.name.clone()));
    if let Some(description) = &tool.description {
        declaration.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    declaration.insert(
        "parameters".to_string(),
        tool.parameters
            .clone()
            .map(|mut schema| {
                clean_gemini_schema(&mut schema);
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
    match value {
        Value::Object(object) => {
            for inner in object.values_mut() {
                clean_gemini_schema(inner);
            }
            if object.get("type").and_then(Value::as_str) == Some("object")
                && !object.contains_key("properties")
            {
                object.insert("properties".to_string(), Value::Object(Map::new()));
            }
        }
        Value::Array(items) => {
            for item in items {
                clean_gemini_schema(item);
            }
        }
        _ => {}
    }
}
