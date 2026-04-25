use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::super::to_openai_chat::{extract_openai_text_content, parse_openai_tool_result_content};
use super::shared::parse_openai_tool_arguments;
use crate::planner::openai::{
    copy_request_number_field_as, extract_openai_reasoning_effort,
    map_openai_reasoning_effort_to_gemini_budget, parse_openai_stop_sequences, value_as_u64,
};

pub fn convert_openai_chat_request_to_gemini_request(
    body_json: &Value,
    mapped_model: &str,
    _upstream_is_stream: bool,
) -> Option<Value> {
    let request = body_json.as_object()?;
    let mut system_segments = Vec::new();
    let mut tool_name_by_id = BTreeMap::new();
    let mut contents = Vec::new();

    if let Some(message_values) = request.get("messages").and_then(Value::as_array) {
        for message in message_values {
            let message_object = message.as_object()?;
            let role = message_object
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            match role.as_str() {
                "system" | "developer" => {
                    let text = extract_openai_text_content(message_object.get("content"))?;
                    if !text.trim().is_empty() {
                        system_segments.push(text);
                    }
                }
                "user" => {
                    let parts = convert_openai_content_to_gemini_parts(
                        message_object.get("content"),
                        OpenAiToGeminiRole::User,
                    )?;
                    if !parts.is_empty() {
                        contents.push(json!({
                            "role": "user",
                            "parts": parts,
                        }));
                    }
                }
                "assistant" => {
                    let mut parts = convert_openai_content_to_gemini_parts(
                        message_object.get("content"),
                        OpenAiToGeminiRole::Assistant,
                    )?;
                    let reasoning_parts = extract_openai_reasoning_to_gemini_parts(message_object);
                    if !reasoning_parts.is_empty() {
                        parts.splice(0..0, reasoning_parts);
                    }
                    if let Some(tool_calls) =
                        message_object.get("tool_calls").and_then(Value::as_array)
                    {
                        for tool_call in tool_calls {
                            let tool_call_object = tool_call.as_object()?;
                            let function = tool_call_object.get("function")?.as_object()?;
                            let tool_name = function
                                .get("name")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())?
                                .to_string();
                            let tool_call_id = tool_call_object
                                .get("id")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .map(ToOwned::to_owned)
                                .unwrap_or_else(|| format!("toolu_{}", Uuid::new_v4().simple()));
                            let tool_input =
                                parse_openai_tool_arguments(function.get("arguments"))?;
                            tool_name_by_id.insert(tool_call_id.clone(), tool_name.clone());
                            parts.push(json!({
                                "functionCall": {
                                    "name": tool_name,
                                    "args": tool_input,
                                    "id": tool_call_id,
                                }
                            }));
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(json!({
                            "role": "model",
                            "parts": parts,
                        }));
                    }
                }
                "tool" => {
                    let tool_use_id = message_object
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?
                        .to_string();
                    let tool_name = tool_name_by_id
                        .get(&tool_use_id)
                        .cloned()
                        .unwrap_or_else(|| tool_use_id.clone());
                    let tool_result =
                        parse_openai_tool_result_content(message_object.get("content"));
                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": tool_name,
                                "id": tool_use_id,
                                "response": {
                                    "result": tool_result,
                                },
                            }
                        }],
                    }));
                }
                _ => {}
            }
        }
    }

    let mut output = Map::new();
    if !mapped_model.trim().is_empty() {
        output.insert(
            "model".to_string(),
            Value::String(mapped_model.trim().to_string()),
        );
    }
    output.insert(
        "contents".to_string(),
        Value::Array(compact_gemini_contents(contents)),
    );
    let system_text = system_segments
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system_text.is_empty() {
        output.insert(
            "systemInstruction".to_string(),
            json!({ "parts": [{ "text": system_text }] }),
        );
    }

    let mut generation_config = Map::new();
    if let Some(max_tokens) = request
        .get("max_completion_tokens")
        .and_then(value_as_u64)
        .or_else(|| request.get("max_tokens").and_then(value_as_u64))
    {
        generation_config.insert("maxOutputTokens".to_string(), Value::from(max_tokens));
    }
    copy_request_number_field_as(
        request,
        &mut generation_config,
        "temperature",
        "temperature",
    );
    copy_request_number_field_as(request, &mut generation_config, "top_p", "topP");
    copy_request_number_field_as(request, &mut generation_config, "top_k", "topK");
    if let Some(candidate_count) = request
        .get("n")
        .and_then(value_as_u64)
        .filter(|value| *value > 1)
    {
        generation_config.insert("candidateCount".to_string(), Value::from(candidate_count));
    }
    if let Some(seed) = request.get("seed").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|raw| i64::try_from(raw).ok()))
    }) {
        generation_config.insert("seed".to_string(), Value::from(seed));
    }
    if let Some(stop_sequences) = parse_openai_stop_sequences(request.get("stop")) {
        generation_config.insert("stopSequences".to_string(), Value::Array(stop_sequences));
    }
    if let Some(reasoning_effort) = extract_openai_reasoning_effort(request) {
        if let Some(thinking_budget) =
            map_openai_reasoning_effort_to_gemini_budget(reasoning_effort.as_str())
        {
            generation_config.insert(
                "thinkingConfig".to_string(),
                json!({
                    "includeThoughts": true,
                    "thinkingBudget": thinking_budget,
                }),
            );
        }
    }
    if let Some(response_format) = request.get("response_format").and_then(Value::as_object) {
        if let Some(format_type) = response_format.get("type").and_then(Value::as_str) {
            match format_type {
                "json_schema" => {
                    generation_config.insert(
                        "responseMimeType".to_string(),
                        Value::String("application/json".to_string()),
                    );
                    if let Some(schema) = response_format
                        .get("json_schema")
                        .and_then(Value::as_object)
                        .and_then(|json_schema| json_schema.get("schema"))
                        .cloned()
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
    }
    if !generation_config.is_empty() {
        output.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }
    if let Some(tools) =
        convert_openai_tools_to_gemini(request.get("tools"), request.get("web_search_options"))
    {
        output.insert("tools".to_string(), tools);
    }
    if let Some(tool_config) = convert_openai_tool_choice_to_gemini(request.get("tool_choice")) {
        output.insert("toolConfig".to_string(), tool_config);
    }
    if let Some(extra_body) = request.get("extra_body").and_then(Value::as_object) {
        if let Some(google) = extra_body.get("google").and_then(Value::as_object) {
            let existing = output
                .entry("generationConfig".to_string())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()?;
            if let Some(response_modalities) = google.get("response_modalities").cloned() {
                existing.insert("responseModalities".to_string(), response_modalities);
            }
            if let Some(thinking_config) = google.get("thinking_config").cloned() {
                existing
                    .entry("thinkingConfig".to_string())
                    .or_insert(thinking_config);
            }
        }
        if let Some(gemini) = extra_body.get("gemini").and_then(Value::as_object) {
            let existing = output
                .entry("generationConfig".to_string())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()?;
            if let Some(extra_config) = gemini
                .get("generation_config_extra")
                .or_else(|| gemini.get("generationConfigExtra"))
                .and_then(Value::as_object)
            {
                for (key, value) in extra_config {
                    existing.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            if let Some(safety_settings) = gemini
                .get("safety_settings")
                .or_else(|| gemini.get("safetySettings"))
                .cloned()
            {
                output.insert("safetySettings".to_string(), safety_settings);
            }
            if let Some(cached_content) = gemini
                .get("cached_content")
                .or_else(|| gemini.get("cachedContent"))
                .cloned()
            {
                output.insert("cachedContent".to_string(), cached_content);
            }
        }
    }

    Some(Value::Object(output))
}

fn extract_openai_reasoning_to_gemini_parts(message: &Map<String, Value>) -> Vec<Value> {
    let mut parts = Vec::new();
    if let Some(reasoning_parts) = message.get("reasoning_parts").and_then(Value::as_array) {
        for reasoning_part in reasoning_parts {
            let Some(reasoning_object) = reasoning_part.as_object() else {
                continue;
            };
            if reasoning_object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value != "thinking")
            {
                continue;
            }
            let thinking = reasoning_object
                .get("thinking")
                .or_else(|| reasoning_object.get("text"))
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if thinking.is_empty() {
                continue;
            }
            let mut part = Map::new();
            part.insert("text".to_string(), Value::String(thinking.to_string()));
            part.insert("thought".to_string(), Value::Bool(true));
            if let Some(signature) = reasoning_object
                .get("signature")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                part.insert(
                    "thoughtSignature".to_string(),
                    Value::String(signature.to_string()),
                );
            }
            parts.push(Value::Object(part));
        }
    }
    if !parts.is_empty() {
        return parts;
    }
    if let Some(reasoning_content) = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(json!({
            "text": reasoning_content,
            "thought": true,
        }));
    }
    parts
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiToGeminiRole {
    User,
    Assistant,
}

fn convert_openai_content_to_gemini_parts(
    content: Option<&Value>,
    _role: OpenAiToGeminiRole,
) -> Option<Vec<Value>> {
    match content {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Some(Vec::new())
            } else {
                Some(vec![json!({ "text": text })])
            }
        }
        Some(Value::Array(parts)) => {
            let mut converted = Vec::new();
            for part in parts {
                let part_object = part.as_object()?;
                let part_type = part_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match part_type {
                    "text" | "input_text" | "output_text" => {
                        if let Some(text) = part_object.get("text").and_then(Value::as_str) {
                            if !text.trim().is_empty() {
                                converted.push(json!({ "text": text }));
                            }
                        }
                    }
                    "image_url" | "input_image" | "output_image" => {
                        let image = part_object
                            .get("image_url")
                            .and_then(|value| {
                                value.as_str().map(ToOwned::to_owned).or_else(|| {
                                    value
                                        .as_object()
                                        .and_then(|object| object.get("url"))
                                        .and_then(Value::as_str)
                                        .map(ToOwned::to_owned)
                                })
                            })
                            .or_else(|| {
                                part_object
                                    .get("url")
                                    .and_then(Value::as_str)
                                    .map(ToOwned::to_owned)
                            })?;
                        if let Some((mime_type, data)) = parse_data_url(image.as_str()) {
                            converted.push(json!({
                                "inlineData": {
                                    "mimeType": mime_type,
                                    "data": data,
                                }
                            }));
                        } else {
                            converted.push(json!({
                                "fileData": {
                                    "fileUri": image,
                                    "mimeType": guess_media_type_from_reference(image.as_str(), "image/jpeg"),
                                }
                            }));
                        }
                    }
                    "file" | "input_file" => {
                        let file_object = part_object
                            .get("file")
                            .and_then(Value::as_object)
                            .unwrap_or(part_object);
                        if let Some(file_data) =
                            file_object.get("file_data").and_then(Value::as_str)
                        {
                            if let Some((mime_type, data)) = parse_data_url(file_data) {
                                converted.push(json!({
                                    "inlineData": {
                                        "mimeType": mime_type,
                                        "data": data,
                                    }
                                }));
                            }
                        } else if let Some(file_id) = file_object
                            .get("file_id")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        {
                            converted.push(json!({
                                "text": format!("[File: {file_id}]"),
                            }));
                        }
                    }
                    "input_audio" => {
                        let audio_object = part_object
                            .get("input_audio")
                            .and_then(Value::as_object)
                            .unwrap_or(part_object);
                        let data = audio_object
                            .get("data")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        let format = audio_object
                            .get("format")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        if let (Some(data), Some(format)) = (data, format) {
                            converted.push(json!({
                                "inlineData": {
                                    "mimeType": format!("audio/{format}"),
                                    "data": data,
                                }
                            }));
                        }
                    }
                    _ => {}
                }
            }
            Some(converted)
        }
        _ => None,
    }
}

fn convert_openai_tools_to_gemini(
    tools: Option<&Value>,
    web_search_options: Option<&Value>,
) -> Option<Value> {
    let mut result_tools = Vec::new();
    let tool_values = tools.and_then(Value::as_array);
    let mut declarations = Vec::new();
    let mut google_search = web_search_options.is_some();
    let mut code_execution = false;
    let mut url_context = false;
    if let Some(tool_values) = tool_values {
        for tool in tool_values {
            let tool_object = tool.as_object()?;
            if tool_object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value != "function")
            {
                continue;
            }
            let function = tool_object.get("function")?.as_object()?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            match normalize_openai_builtin_gemini_tool_name(name) {
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
            let mut declaration = Map::new();
            declaration.insert("name".to_string(), Value::String(name.to_string()));
            if let Some(description) = function.get("description").cloned() {
                declaration.insert("description".to_string(), description);
            }
            declaration.insert(
                "parameters".to_string(),
                function
                    .get("parameters")
                    .cloned()
                    .map(|mut schema| {
                        clean_gemini_schema(&mut schema);
                        schema
                    })
                    .unwrap_or_else(|| json!({})),
            );
            declarations.push(Value::Object(declaration));
        }
    }
    if code_execution {
        result_tools.push(json!({ "codeExecution": {} }));
    }
    if google_search {
        result_tools.push(json!({ "googleSearch": {} }));
    }
    if url_context {
        result_tools.push(json!({ "urlContext": {} }));
    }
    if !declarations.is_empty() {
        result_tools.push(json!({ "functionDeclarations": declarations }));
    }
    (!result_tools.is_empty()).then_some(Value::Array(result_tools))
}

fn convert_openai_tool_choice_to_gemini(tool_choice: Option<&Value>) -> Option<Value> {
    let tool_choice = tool_choice?;
    match tool_choice {
        Value::String(value) => {
            let mode = match value.trim().to_ascii_lowercase().as_str() {
                "none" => "NONE",
                "required" => "ANY",
                "auto" => "AUTO",
                _ => return None,
            };
            Some(json!({
                "functionCallingConfig": {
                    "mode": mode,
                }
            }))
        }
        Value::Object(object) => {
            let function_name = object
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(json!({
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": [function_name],
                }
            }))
        }
        _ => None,
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
            let last_role = last
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if last_role == role {
                if let Some(last_parts) = last.get_mut("parts").and_then(Value::as_array_mut) {
                    last_parts.extend(parts);
                }
                continue;
            }
        }
        compact.push(content);
    }
    compact
}

const ALLOWED_SCHEMA_FIELDS: &[&str] = &[
    "type",
    "description",
    "properties",
    "required",
    "items",
    "enum",
    "title",
];

const CONSTRAINT_FIELDS: &[(&str, &str)] = &[
    ("minLength", "minLen"),
    ("maxLength", "maxLen"),
    ("pattern", "pattern"),
    ("minimum", "min"),
    ("maximum", "max"),
    ("multipleOf", "multipleOf"),
    ("exclusiveMinimum", "exclMin"),
    ("exclusiveMaximum", "exclMax"),
    ("minItems", "minItems"),
    ("maxItems", "maxItems"),
    ("format", "format"),
];

fn clean_gemini_schema(value: &mut Value) {
    if !value.is_object() {
        return;
    }

    let mut defs = Map::new();
    collect_all_defs(value, &mut defs);
    if let Some(object) = value.as_object_mut() {
        object.remove("$defs");
        object.remove("definitions");
    }
    let mut seen = Vec::new();
    flatten_refs(value, &defs, &mut seen);
    clean_schema_recursive(value, true);
}

fn collect_all_defs(value: &Value, defs: &mut Map<String, Value>) {
    match value {
        Value::Object(object) => {
            for defs_key in ["$defs", "definitions"] {
                if let Some(Value::Object(inner_defs)) = object.get(defs_key) {
                    for (key, inner) in inner_defs {
                        defs.entry(key.clone()).or_insert_with(|| inner.clone());
                    }
                }
            }
            for (key, inner) in object {
                if key != "$defs" && key != "definitions" {
                    collect_all_defs(inner, defs);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_all_defs(item, defs);
            }
        }
        _ => {}
    }
}

fn flatten_refs(value: &mut Value, defs: &Map<String, Value>, seen: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            let ref_path = object
                .remove("$ref")
                .and_then(|value| value.as_str().map(ToOwned::to_owned));
            if let Some(ref_path) = ref_path {
                let ref_name = ref_path.rsplit('/').next().unwrap_or_default().to_string();
                if seen.iter().any(|value| value == &ref_name) {
                    object
                        .entry("type".to_string())
                        .or_insert_with(|| Value::String("string".to_string()));
                    append_schema_hint(object, &format!("(Circular $ref: {ref_path})"));
                    return;
                }
                seen.push(ref_name.clone());
                if let Some(Value::Object(def_schema)) = defs.get(&ref_name) {
                    for (key, inner) in def_schema {
                        if !object.contains_key(key) {
                            object.insert(key.clone(), inner.clone());
                        }
                    }
                    flatten_refs(value, defs, seen);
                } else {
                    object
                        .entry("type".to_string())
                        .or_insert_with(|| Value::String("string".to_string()));
                    append_schema_hint(object, &format!("(Unresolved $ref: {ref_path})"));
                }
                seen.pop();
                return;
            }
            for inner in object.values_mut() {
                flatten_refs(inner, defs, seen);
            }
        }
        Value::Array(items) => {
            for item in items {
                flatten_refs(item, defs, seen);
            }
        }
        _ => {}
    }
}

fn clean_schema_recursive(value: &mut Value, is_schema_node: bool) -> bool {
    let Some(object) = value.as_object_mut() else {
        if let Some(items) = value.as_array_mut() {
            for item in items {
                clean_schema_recursive(item, is_schema_node);
            }
        }
        return false;
    };

    let mut is_nullable = false;
    merge_all_of(object);

    if (object.get("type").and_then(Value::as_str) == Some("object")
        || object.contains_key("properties"))
        && object.contains_key("items")
    {
        let items = object.remove("items");
        if let Some(Value::Object(items)) = items {
            let props = object
                .entry("properties".to_string())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut();
            if let Some(props) = props {
                for (key, inner) in items {
                    props.entry(key).or_insert(inner);
                }
            }
        }
    }

    let mut nullable_keys = Vec::new();
    if let Some(Value::Object(props)) = object.get_mut("properties") {
        for (key, inner) in props.iter_mut() {
            if clean_schema_recursive(inner, true) {
                nullable_keys.push(key.clone());
            }
        }
        if !object.contains_key("type") {
            object.insert("type".to_string(), Value::String("object".to_string()));
        }
    }
    if !nullable_keys.is_empty() {
        if let Some(Value::Array(required)) = object.get_mut("required") {
            required.retain(|item| {
                item.as_str()
                    .is_some_and(|value| !nullable_keys.iter().any(|candidate| candidate == value))
            });
            if required.is_empty() {
                object.remove("required");
            }
        }
    }

    if let Some(items) = object.get_mut("items") {
        if items.is_object() {
            clean_schema_recursive(items, true);
            if !object.contains_key("type") {
                object.insert("type".to_string(), Value::String("array".to_string()));
            }
        }
    }

    if !object.contains_key("properties") && !object.contains_key("items") {
        for (key, inner) in object.iter_mut() {
            if !matches!(key.as_str(), "anyOf" | "oneOf" | "allOf" | "enum" | "type")
                && (inner.is_object() || inner.is_array())
            {
                clean_schema_recursive(inner, false);
            }
        }
    }

    for combo_key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(combo)) = object.get_mut(combo_key) {
            for branch in combo {
                if branch.is_object() {
                    clean_schema_recursive(branch, true);
                }
            }
        }
    }

    let should_merge_union = object.get("type").is_none()
        || object.get("type").and_then(Value::as_str) == Some("object");
    if should_merge_union {
        let union = object
            .get("anyOf")
            .and_then(Value::as_array)
            .or_else(|| object.get("oneOf").and_then(Value::as_array))
            .cloned();
        if let Some(union) = union {
            let (best, all_types) = extract_best_schema_branch(&union);
            if let Some(Value::Object(best_object)) = best {
                for (key, inner) in best_object {
                    if key == "properties" {
                        let target = object
                            .entry("properties".to_string())
                            .or_insert_with(|| Value::Object(Map::new()))
                            .as_object_mut();
                        if let (Some(target), Value::Object(props)) = (target, inner) {
                            for (prop_key, prop_value) in props {
                                target.entry(prop_key).or_insert(prop_value);
                            }
                        }
                    } else if key == "required" {
                        let target = object
                            .entry("required".to_string())
                            .or_insert_with(|| Value::Array(Vec::new()))
                            .as_array_mut();
                        if let (Some(target), Value::Array(required)) = (target, inner) {
                            for required_value in required {
                                if !target.iter().any(|value| value == &required_value) {
                                    target.push(required_value);
                                }
                            }
                        }
                    } else if !object.contains_key(&key) {
                        object.insert(key, inner);
                    }
                }
                if all_types.len() > 1 {
                    append_schema_hint(object, &format!("Accepts: {}", all_types.join(" | ")));
                }
            }
        }
    }
    object.remove("anyOf");
    object.remove("oneOf");

    let is_not_schema_payload = object.contains_key("functionCall")
        || object.contains_key("functionResponse")
        || object.contains_key("function_call")
        || object.contains_key("function_response");
    let has_standard = object
        .keys()
        .any(|key| ALLOWED_SCHEMA_FIELDS.iter().any(|allowed| key == allowed));

    if is_schema_node && !has_standard && !object.is_empty() && !is_not_schema_payload {
        let keys = object.keys().cloned().collect::<Vec<_>>();
        let mut new_props = Map::new();
        for key in keys {
            if let Some(inner) = object.remove(&key) {
                new_props.insert(key, inner);
            }
        }
        for inner in new_props.values_mut() {
            if inner.is_object() {
                clean_schema_recursive(inner, true);
            }
        }
        object.insert("type".to_string(), Value::String("object".to_string()));
        object.insert("properties".to_string(), Value::Object(new_props));
    }

    let looks_like_schema = (is_schema_node || has_standard || object.contains_key("properties"))
        && !is_not_schema_payload;
    if looks_like_schema {
        move_constraints_to_description(object);
        let keys_to_remove = object
            .keys()
            .filter(|key| !ALLOWED_SCHEMA_FIELDS.iter().any(|allowed| *key == allowed))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys_to_remove {
            object.remove(&key);
        }

        if object.get("type").and_then(Value::as_str) == Some("object")
            && !object.contains_key("properties")
        {
            object.insert("properties".to_string(), Value::Object(Map::new()));
        }

        let valid_keys = object
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| props.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(Value::Array(required)) = object.get_mut("required") {
            required.retain(|item| {
                item.as_str()
                    .is_some_and(|value| valid_keys.iter().any(|candidate| candidate == value))
            });
            if required.is_empty() {
                object.remove("required");
            }
        }

        if !object.contains_key("type") {
            let inferred_type = if object.contains_key("enum") {
                "string"
            } else if object.contains_key("properties") {
                "object"
            } else if object.contains_key("items") {
                "array"
            } else {
                "string"
            };
            object.insert("type".to_string(), Value::String(inferred_type.to_string()));
        }

        let fallback_type = if object.contains_key("properties") {
            "object"
        } else if object.contains_key("items") {
            "array"
        } else {
            "string"
        };
        let selected_type = match object.get("type") {
            Some(Value::String(type_name)) => {
                let lower = type_name.to_ascii_lowercase();
                if lower == "null" {
                    is_nullable = true;
                    None
                } else {
                    Some(lower)
                }
            }
            Some(Value::Array(types)) => {
                let mut selected = None;
                for item in types {
                    if let Some(type_name) = item.as_str() {
                        let lower = type_name.to_ascii_lowercase();
                        if lower == "null" {
                            is_nullable = true;
                        } else if selected.is_none() {
                            selected = Some(lower);
                        }
                    }
                }
                selected
            }
            _ => None,
        };
        object.insert(
            "type".to_string(),
            Value::String(selected_type.unwrap_or_else(|| fallback_type.to_string())),
        );

        if is_nullable {
            append_schema_hint(object, "(nullable)");
        }

        if let Some(Value::Array(items)) = object.get_mut("enum") {
            for item in items.iter_mut() {
                if !item.is_string() {
                    *item = Value::String(match item {
                        Value::Null => "null".to_string(),
                        _ => item.to_string(),
                    });
                }
            }
        }
    }

    is_nullable
}

fn merge_all_of(object: &mut Map<String, Value>) {
    let all_of = object.remove("allOf");
    let Some(Value::Array(all_of)) = all_of else {
        return;
    };

    let mut merged_props = Map::new();
    let mut merged_required = Vec::new();
    let mut other_fields = Map::new();

    for item in all_of {
        let Value::Object(item) = item else {
            continue;
        };
        if let Some(Value::Object(props)) = item.get("properties") {
            for (key, value) in props {
                merged_props.insert(key.clone(), value.clone());
            }
        }
        if let Some(Value::Array(required)) = item.get("required") {
            for value in required {
                if !merged_required.iter().any(|existing| existing == value) {
                    merged_required.push(value.clone());
                }
            }
        }
        for (key, value) in item {
            if !matches!(key.as_str(), "properties" | "required" | "allOf")
                && !other_fields.contains_key(&key)
            {
                other_fields.insert(key, value);
            }
        }
    }

    for (key, value) in other_fields {
        object.entry(key).or_insert(value);
    }
    if !merged_props.is_empty() {
        let target = object
            .entry("properties".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut();
        if let Some(target) = target {
            for (key, value) in merged_props {
                target.entry(key).or_insert(value);
            }
        }
    }
    if !merged_required.is_empty() {
        let target = object
            .entry("required".to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut();
        if let Some(target) = target {
            for value in merged_required {
                if !target.iter().any(|existing| existing == &value) {
                    target.push(value);
                }
            }
        }
    }
}

fn extract_best_schema_branch(union: &[Value]) -> (Option<Value>, Vec<String>) {
    let mut best = None;
    let mut best_score = -1;
    let mut all_types = Vec::new();

    for item in union {
        let score = score_schema_branch(item);
        if let Some(type_name) = schema_type_name(item) {
            if !all_types.iter().any(|existing| existing == type_name) {
                all_types.push(type_name.to_string());
            }
        }
        if score > best_score {
            best_score = score;
            best = Some(item.clone());
        }
    }

    (best, all_types)
}

fn score_schema_branch(value: &Value) -> i32 {
    let Some(object) = value.as_object() else {
        return 0;
    };
    if object.contains_key("properties")
        || object.get("type").and_then(Value::as_str) == Some("object")
    {
        return 3;
    }
    if object.contains_key("items") || object.get("type").and_then(Value::as_str) == Some("array") {
        return 2;
    }
    if object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value != "null")
    {
        return 1;
    }
    0
}

fn schema_type_name(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    object
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| object.contains_key("properties").then_some("object"))
        .or_else(|| object.contains_key("items").then_some("array"))
}

fn move_constraints_to_description(object: &mut Map<String, Value>) {
    let hints = CONSTRAINT_FIELDS
        .iter()
        .filter_map(|(field, label)| object.get(*field).map(|value| format!("{label}: {value}")))
        .collect::<Vec<_>>();
    if !hints.is_empty() {
        append_schema_hint(object, &format!("[Constraint: {}]", hints.join(", ")));
    }
}

fn append_schema_hint(object: &mut Map<String, Value>, hint: &str) {
    let existing = object
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if existing.contains(hint) {
        return;
    }
    let next = if existing.trim().is_empty() {
        hint.to_string()
    } else {
        format!("{existing} {hint}")
    };
    object.insert("description".to_string(), Value::String(next));
}

fn parse_data_url(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(",")?;
    let mime_type = meta.strip_suffix(";base64")?;
    if mime_type.trim().is_empty() || data.trim().is_empty() {
        return None;
    }
    Some((mime_type.to_string(), data.to_string()))
}

fn guess_media_type_from_reference(reference: &str, default_mime: &str) -> String {
    let normalized = reference
        .split('?')
        .next()
        .unwrap_or(reference)
        .to_ascii_lowercase();
    if normalized.ends_with(".png") {
        "image/png".to_string()
    } else if normalized.ends_with(".gif") {
        "image/gif".to_string()
    } else if normalized.ends_with(".webp") {
        "image/webp".to_string()
    } else if normalized.ends_with(".jpg") || normalized.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if normalized.ends_with(".pdf") {
        "application/pdf".to_string()
    } else {
        default_mime.to_string()
    }
}

fn normalize_openai_builtin_gemini_tool_name(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "googlesearch" => Some("googleSearch"),
        "codeexecution" => Some("codeExecution"),
        "urlcontext" => Some("urlContext"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::convert_openai_chat_request_to_gemini_request;
    use serde_json::json;

    #[test]
    fn maps_seed_and_builtin_gemini_tools_from_openai_request() {
        let request = json!({
            "model": "gpt-5.4",
            "messages": [
                { "role": "user", "content": "use tools" }
            ],
            "seed": 7,
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "googleSearch",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "codeExecution",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "urlContext",
                        "parameters": { "type": "object", "properties": {} }
                    }
                },
                {
                    "type": "function",
                    "function": {
                        "name": "lookupWeather",
                        "parameters": { "type": "object", "properties": { "city": { "type": "string" } } }
                    }
                }
            ]
        });

        let converted =
            convert_openai_chat_request_to_gemini_request(&request, "gemini-2.5-pro", false)
                .expect("request should convert");

        assert_eq!(converted["model"], "gemini-2.5-pro");
        assert_eq!(converted["generationConfig"]["seed"], 7);
        assert_eq!(converted["tools"][0], json!({ "codeExecution": {} }));
        assert_eq!(converted["tools"][1], json!({ "googleSearch": {} }));
        assert_eq!(converted["tools"][2], json!({ "urlContext": {} }));
        assert_eq!(
            converted["tools"][3],
            json!({
                "functionDeclarations": [
                    {
                        "name": "lookupWeather",
                        "parameters": { "type": "object", "properties": { "city": { "type": "string" } } }
                    }
                ]
            })
        );
    }

    #[test]
    fn deduplicates_google_search_when_web_search_options_are_also_present() {
        let request = json!({
            "model": "gpt-5.4",
            "messages": [
                { "role": "user", "content": "search" }
            ],
            "web_search_options": {},
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "googleSearch",
                        "parameters": { "type": "object", "properties": {} }
                    }
                }
            ]
        });

        let converted =
            convert_openai_chat_request_to_gemini_request(&request, "gemini-2.5-pro", false)
                .expect("request should convert");

        let tools = converted["tools"]
            .as_array()
            .expect("tools should be array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0], json!({ "googleSearch": {} }));
    }

    #[test]
    fn converts_openai_reasoning_multimodal_and_passthrough_to_gemini_request() {
        let request = json!({
            "model": "gpt-5.4",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "Read this" },
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": "data:image/png;base64,iVBORw0KGgo="
                            }
                        },
                        {
                            "type": "file",
                            "file": {
                                "file_data": "data:application/pdf;base64,JVBERi0x"
                            }
                        },
                        {
                            "type": "input_audio",
                            "input_audio": {
                                "data": "SUQz",
                                "format": "mp3"
                            }
                        }
                    ]
                },
                {
                    "role": "assistant",
                    "reasoning_content": "step by step",
                    "reasoning_parts": [
                        {
                            "type": "thinking",
                            "thinking": "step by step",
                            "signature": "sig_123"
                        }
                    ],
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": "https://example.com/cat.png"
                            }
                        },
                        {
                            "type": "file",
                            "file": { "file_id": "file_123" }
                        },
                        { "type": "text", "text": "done" }
                    ],
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "\"tokyo\""
                        }
                    }]
                }
            ],
            "reasoning_effort": "high",
            "extra_body": {
                "google": {
                    "response_modalities": ["TEXT", "IMAGE"],
                    "thinking_config": {
                        "includeThoughts": true,
                        "thinkingBudget": 2048
                    }
                },
                "gemini": {
                    "generation_config_extra": {
                        "presencePenalty": 0.5
                    },
                    "safety_settings": [
                        { "category": "HARM_CATEGORY_HATE_SPEECH" }
                    ],
                    "cached_content": "cached/abc"
                }
            }
        });

        let converted =
            convert_openai_chat_request_to_gemini_request(&request, "gemini-2.5-pro", false)
                .expect("request should convert");

        assert_eq!(
            converted["contents"][0],
            json!({
                "role": "user",
                "parts": [
                    { "text": "Read this" },
                    { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo=" } },
                    { "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0x" } },
                    { "inlineData": { "mimeType": "audio/mp3", "data": "SUQz" } }
                ]
            })
        );
        assert_eq!(converted["contents"][1]["role"], "model");
        assert_eq!(converted["contents"][1]["parts"][0]["thought"], true);
        assert_eq!(
            converted["contents"][1]["parts"][0]["thoughtSignature"],
            "sig_123"
        );
        assert_eq!(
            converted["contents"][1]["parts"][1],
            json!({
                "fileData": {
                    "fileUri": "https://example.com/cat.png",
                    "mimeType": "image/png"
                }
            })
        );
        assert_eq!(
            converted["contents"][1]["parts"][2]["text"],
            "[File: file_123]"
        );
        assert_eq!(
            converted["contents"][1]["parts"][4]["functionCall"]["args"],
            json!({ "raw": "tokyo" })
        );
        assert_eq!(
            converted["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            4096
        );
        assert_eq!(
            converted["generationConfig"]["responseModalities"],
            json!(["TEXT", "IMAGE"])
        );
        assert_eq!(converted["generationConfig"]["presencePenalty"], 0.5);
        assert_eq!(
            converted["safetySettings"],
            json!([{ "category": "HARM_CATEGORY_HATE_SPEECH" }])
        );
        assert_eq!(converted["cachedContent"], "cached/abc");
    }
}
