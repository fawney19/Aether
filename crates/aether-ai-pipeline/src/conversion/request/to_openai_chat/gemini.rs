use serde_json::{json, Map, Value};

use super::shared::canonical_json_string;
use crate::planner::openai::map_thinking_budget_to_openai_reasoning_effort;

const GEMINI_MAPPED_GENERATION_CONFIG_KEYS: &[&str] = &[
    "maxOutputTokens",
    "max_output_tokens",
    "temperature",
    "topP",
    "top_p",
    "topK",
    "top_k",
    "candidateCount",
    "candidate_count",
    "seed",
    "stopSequences",
    "stop_sequences",
    "thinkingConfig",
    "thinking_config",
    "responseMimeType",
    "response_mime_type",
    "responseSchema",
    "response_schema",
    "responseModalities",
    "response_modalities",
];

pub fn normalize_gemini_request_to_openai_chat_request(
    body_json: &Value,
    request_path: &str,
) -> Option<Value> {
    let request = body_json.as_object()?;
    let mut output = Map::new();
    if let Some(model) = request
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        output.insert("model".to_string(), Value::String(model.to_string()));
    } else if let Some(model) = extract_gemini_model_from_path(request_path) {
        output.insert("model".to_string(), Value::String(model));
    }

    let mut messages = Vec::new();
    if let Some(system_text) = extract_gemini_system_text(
        request
            .get("systemInstruction")
            .or_else(|| request.get("system_instruction")),
    ) {
        if !system_text.trim().is_empty() {
            messages.push(json!({
                "role": "system",
                "content": system_text,
            }));
        }
    }

    if let Some(contents) = request.get("contents").and_then(Value::as_array) {
        for content in contents {
            let content_object = content.as_object()?;
            let role = content_object
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user")
                .trim()
                .to_ascii_lowercase();
            let parts = content_object.get("parts").and_then(Value::as_array)?;
            match role.as_str() {
                "model" => messages.push(normalize_gemini_model_parts_to_openai_message(parts)?),
                _ => append_gemini_user_parts_to_openai_messages(parts, &mut messages)?,
            }
        }
    }
    output.insert("messages".to_string(), Value::Array(messages));

    let generation_config = request
        .get("generationConfig")
        .or_else(|| request.get("generation_config"))
        .and_then(Value::as_object);
    let mut google_extra = Map::new();
    let mut gemini_extra = Map::new();
    if let Some(generation_config) = generation_config {
        if let Some(value) =
            generation_config_value(generation_config, "maxOutputTokens", "max_output_tokens")
                .cloned()
        {
            output.insert("max_completion_tokens".to_string(), value);
        }
        if let Some(value) = generation_config.get("temperature").cloned() {
            output.insert("temperature".to_string(), value);
        }
        if let Some(value) = generation_config_value(generation_config, "topP", "top_p").cloned() {
            output.insert("top_p".to_string(), value);
        }
        if let Some(value) = generation_config_value(generation_config, "topK", "top_k").cloned() {
            output.insert("top_k".to_string(), value);
        }
        if let Some(value) =
            generation_config_value(generation_config, "candidateCount", "candidate_count").cloned()
        {
            output.insert("n".to_string(), value);
        }
        if let Some(value) = generation_config.get("seed").cloned() {
            output.insert("seed".to_string(), value);
        }
        if let Some(value) =
            generation_config_value(generation_config, "stopSequences", "stop_sequences").cloned()
        {
            output.insert("stop".to_string(), value);
        }
        if let Some(thinking_config) =
            generation_config_value(generation_config, "thinkingConfig", "thinking_config")
                .and_then(Value::as_object)
        {
            google_extra.insert(
                "thinking_config".to_string(),
                Value::Object(thinking_config.clone()),
            );
        }
        if let Some(response_modalities) = generation_config_value(
            generation_config,
            "responseModalities",
            "response_modalities",
        )
        .cloned()
        {
            google_extra.insert("response_modalities".to_string(), response_modalities);
        }
        if let Some(thinking_budget) =
            generation_config_value(generation_config, "thinkingConfig", "thinking_config")
                .and_then(Value::as_object)
                .and_then(|thinking| {
                    thinking
                        .get("thinkingBudget")
                        .or_else(|| thinking.get("thinking_budget"))
                })
                .and_then(Value::as_u64)
        {
            output.insert(
                "reasoning_effort".to_string(),
                Value::String(
                    map_thinking_budget_to_openai_reasoning_effort(thinking_budget).to_string(),
                ),
            );
        }
        if generation_config_value(generation_config, "responseMimeType", "response_mime_type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "application/json")
        {
            let response_format = if let Some(schema) =
                generation_config_value(generation_config, "responseSchema", "response_schema")
            {
                json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": "response_schema",
                        "schema": schema,
                    }
                })
            } else {
                json!({ "type": "json_object" })
            };
            output.insert("response_format".to_string(), response_format);
        }

        let mut generation_config_extra = Map::new();
        for (key, value) in generation_config {
            if GEMINI_MAPPED_GENERATION_CONFIG_KEYS
                .iter()
                .any(|candidate| candidate == &key.as_str())
            {
                continue;
            }
            generation_config_extra.insert(key.clone(), value.clone());
        }
        if !generation_config_extra.is_empty() {
            gemini_extra.insert(
                "generation_config_extra".to_string(),
                Value::Object(generation_config_extra),
            );
        }
    }
    if let Some(value) = request.get("stream").cloned() {
        output.insert("stream".to_string(), value);
    }
    if let Some(value) = request
        .get("safetySettings")
        .or_else(|| request.get("safety_settings"))
        .cloned()
    {
        gemini_extra.insert("safety_settings".to_string(), value);
    }
    if let Some(value) = request
        .get("cachedContent")
        .or_else(|| request.get("cached_content"))
        .cloned()
    {
        gemini_extra.insert("cached_content".to_string(), value);
    }
    if let Some(tools) = normalize_gemini_tools_to_openai(request.get("tools"))? {
        output.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(web_search_options) = extract_gemini_web_search_options(request.get("tools")) {
        output.insert("web_search_options".to_string(), web_search_options);
    }
    if let Some(tool_choice) = normalize_gemini_tool_choice_to_openai(
        request
            .get("toolConfig")
            .or_else(|| request.get("tool_config")),
    )? {
        output.insert("tool_choice".to_string(), tool_choice);
    }
    if !google_extra.is_empty() || !gemini_extra.is_empty() {
        let mut extra_body = Map::new();
        if !google_extra.is_empty() {
            extra_body.insert("google".to_string(), Value::Object(google_extra));
        }
        if !gemini_extra.is_empty() {
            extra_body.insert("gemini".to_string(), Value::Object(gemini_extra));
        }
        output.insert("extra_body".to_string(), Value::Object(extra_body));
    }

    Some(Value::Object(output))
}

#[derive(Debug)]
enum GeminiNormalizedPart {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    ImageUrl(String),
    FileData(String),
    FileUrl(String),
    AudioData {
        data: String,
        format: String,
    },
    ToolUse {
        id: Option<String>,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Value,
    },
}

fn append_gemini_user_parts_to_openai_messages(
    parts: &[Value],
    messages: &mut Vec<Value>,
) -> Option<()> {
    let mut pending_parts = Vec::new();
    for part in normalize_gemini_parts(parts)? {
        match part {
            GeminiNormalizedPart::Text(text) | GeminiNormalizedPart::Thinking { text, .. } => {
                push_openai_text_part(&mut pending_parts, text);
            }
            GeminiNormalizedPart::ImageUrl(url) => {
                pending_parts.push(build_openai_image_part(url));
            }
            GeminiNormalizedPart::FileData(file_data) => {
                pending_parts.push(build_openai_file_part(file_data));
            }
            GeminiNormalizedPart::FileUrl(url) => {
                push_openai_text_part(&mut pending_parts, format!("[File: {url}]"));
            }
            GeminiNormalizedPart::AudioData { data, format } => {
                pending_parts.push(build_openai_audio_part(data, format));
            }
            GeminiNormalizedPart::ToolResult {
                tool_use_id,
                content,
            } => {
                flush_openai_user_content_parts(&mut pending_parts, messages);
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                }));
            }
            GeminiNormalizedPart::ToolUse { .. } => {}
        }
    }
    flush_openai_user_content_parts(&mut pending_parts, messages);
    Some(())
}

fn normalize_gemini_model_parts_to_openai_message(parts: &[Value]) -> Option<Value> {
    let mut reasoning_segments = Vec::new();
    let mut reasoning_parts = Vec::new();
    let mut content_parts = Vec::new();
    let mut tool_calls = Vec::new();
    for (index, part) in normalize_gemini_parts(parts)?.into_iter().enumerate() {
        match part {
            GeminiNormalizedPart::Text(text) => {
                push_openai_text_part(&mut content_parts, text);
            }
            GeminiNormalizedPart::Thinking { text, signature } => {
                if !text.trim().is_empty() {
                    reasoning_segments.push(text.clone());
                }
                let mut reasoning_part = Map::new();
                reasoning_part.insert("type".to_string(), Value::String("thinking".to_string()));
                reasoning_part.insert("thinking".to_string(), Value::String(text));
                if let Some(signature) = signature {
                    reasoning_part.insert("signature".to_string(), Value::String(signature));
                }
                reasoning_parts.push(Value::Object(reasoning_part));
            }
            GeminiNormalizedPart::ImageUrl(url) => {
                content_parts.push(build_openai_image_part(url));
            }
            GeminiNormalizedPart::FileData(file_data) => {
                content_parts.push(build_openai_file_part(file_data));
            }
            GeminiNormalizedPart::FileUrl(url) => {
                push_openai_text_part(&mut content_parts, format!("[File: {url}]"));
            }
            GeminiNormalizedPart::AudioData { data, format } => {
                content_parts.push(build_openai_audio_part(data, format));
            }
            GeminiNormalizedPart::ToolUse { id, name, input } => {
                let tool_id = id.unwrap_or_else(|| format!("toolu_{}_{}", name, index));
                tool_calls.push(json!({
                    "id": tool_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": canonical_json_string(input),
                    }
                }));
            }
            GeminiNormalizedPart::ToolResult { .. } => {}
        }
    }

    let mut assistant = Map::new();
    assistant.insert("role".to_string(), Value::String("assistant".to_string()));
    assistant.insert(
        "content".to_string(),
        match build_openai_content_value(content_parts) {
            Some(content) => content,
            None if !tool_calls.is_empty() => Value::Null,
            None => Value::String(String::new()),
        },
    );
    if !reasoning_segments.is_empty() {
        assistant.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_segments.join("")),
        );
    }
    if !reasoning_parts.is_empty() {
        assistant.insert("reasoning_parts".to_string(), Value::Array(reasoning_parts));
    }
    if !tool_calls.is_empty() {
        assistant.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    Some(Value::Object(assistant))
}

fn normalize_gemini_parts(parts: &[Value]) -> Option<Vec<GeminiNormalizedPart>> {
    let mut normalized = Vec::new();
    for part in parts {
        let part = part.as_object()?;
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            if part
                .get("thought")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                normalized.push(GeminiNormalizedPart::Thinking {
                    text: text.to_string(),
                    signature: part
                        .get("thoughtSignature")
                        .or_else(|| part.get("thought_signature"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                });
            } else {
                normalized.push(GeminiNormalizedPart::Text(text.to_string()));
            }
            continue;
        }
        if let Some(inline_data) = part
            .get("inlineData")
            .or_else(|| part.get("inline_data"))
            .and_then(Value::as_object)
        {
            let mime_type = inline_data
                .get("mimeType")
                .or_else(|| inline_data.get("mime_type"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let data = inline_data
                .get("data")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            if mime_type.starts_with("image/") {
                normalized.push(GeminiNormalizedPart::ImageUrl(build_data_url(
                    mime_type, data,
                )));
            } else if let Some(format) = mime_type.strip_prefix("audio/") {
                normalized.push(GeminiNormalizedPart::AudioData {
                    data: data.to_string(),
                    format: format.to_string(),
                });
            } else {
                normalized.push(GeminiNormalizedPart::FileData(build_data_url(
                    mime_type, data,
                )));
            }
            continue;
        }
        if let Some(file_data) = part
            .get("fileData")
            .or_else(|| part.get("file_data"))
            .and_then(Value::as_object)
        {
            let file_uri = file_data
                .get("fileUri")
                .or_else(|| file_data.get("file_uri"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let mime_type = file_data
                .get("mimeType")
                .or_else(|| file_data.get("mime_type"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if mime_type.is_some_and(|value| value.starts_with("image/")) {
                normalized.push(GeminiNormalizedPart::ImageUrl(file_uri.to_string()));
            } else {
                normalized.push(GeminiNormalizedPart::FileUrl(file_uri.to_string()));
            }
            continue;
        }
        if let Some(function_call) = part
            .get("functionCall")
            .or_else(|| part.get("function_call"))
            .and_then(Value::as_object)
        {
            let name = function_call
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?
                .to_string();
            normalized.push(GeminiNormalizedPart::ToolUse {
                id: function_call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                name,
                input: function_call
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Map::new())),
            });
            continue;
        }
        if let Some(function_response) = part
            .get("functionResponse")
            .or_else(|| part.get("function_response"))
            .and_then(Value::as_object)
        {
            let tool_use_id = function_response
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .or_else(|| {
                    function_response
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                })?;
            let response_value = function_response
                .get("response")
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new()));
            let content = match response_value {
                Value::Object(mut object) => object
                    .remove("result")
                    .unwrap_or_else(|| Value::Object(object)),
                other => other,
            };
            normalized.push(GeminiNormalizedPart::ToolResult {
                tool_use_id,
                content,
            });
        }
    }
    Some(normalized)
}

fn flush_openai_user_content_parts(pending_parts: &mut Vec<Value>, messages: &mut Vec<Value>) {
    let parts = std::mem::take(pending_parts);
    let Some(content) = build_openai_content_value(parts) else {
        return;
    };
    messages.push(json!({
        "role": "user",
        "content": content,
    }));
}

fn build_openai_content_value(parts: Vec<Value>) -> Option<Value> {
    if parts.is_empty() {
        return None;
    }
    if parts
        .iter()
        .all(|part| part.get("type").and_then(Value::as_str) == Some("text"))
    {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n");
        return Some(Value::String(text));
    }
    Some(Value::Array(parts))
}

fn push_openai_text_part(parts: &mut Vec<Value>, text: String) {
    if text.trim().is_empty() {
        return;
    }
    parts.push(json!({
        "type": "text",
        "text": text,
    }));
}

fn build_openai_image_part(url: String) -> Value {
    json!({
        "type": "image_url",
        "image_url": {
            "url": url,
        }
    })
}

fn build_openai_file_part(file_data: String) -> Value {
    json!({
        "type": "file",
        "file": {
            "file_data": file_data,
        }
    })
}

fn build_openai_audio_part(data: String, format: String) -> Value {
    json!({
        "type": "input_audio",
        "input_audio": {
            "data": data,
            "format": format,
        }
    })
}

fn build_data_url(mime_type: &str, data: &str) -> String {
    format!("data:{mime_type};base64,{data}")
}

fn generation_config_value<'a>(
    generation_config: &'a Map<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<&'a Value> {
    generation_config
        .get(camel)
        .or_else(|| generation_config.get(snake))
}

fn extract_gemini_system_text(system_instruction: Option<&Value>) -> Option<String> {
    let system_instruction = system_instruction?;
    match system_instruction {
        Value::String(text) => Some(text.trim().to_string()),
        Value::Object(object) => {
            let parts = object.get("parts")?.as_array()?;
            let mut segments = Vec::new();
            for part in parts {
                let part = part.as_object()?;
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        segments.push(text.to_string());
                    }
                }
            }
            Some(segments.join("\n\n"))
        }
        _ => None,
    }
}

fn normalize_gemini_tools_to_openai(tools: Option<&Value>) -> Option<Option<Vec<Value>>> {
    let Some(tools) = tools else {
        return Some(None);
    };
    let tools = tools.as_array()?;
    let mut normalized = Vec::new();
    let mut has_code_execution = false;
    let mut has_url_context = false;
    for tool in tools {
        let tool = tool.as_object()?;
        if tool.get("codeExecution").is_some() || tool.get("code_execution").is_some() {
            has_code_execution = true;
        }
        if tool.get("urlContext").is_some() || tool.get("url_context").is_some() {
            has_url_context = true;
        }
        let declarations = tool
            .get("functionDeclarations")
            .or_else(|| tool.get("function_declarations"))
            .and_then(Value::as_array);
        let Some(declarations) = declarations else {
            continue;
        };
        for declaration in declarations {
            let declaration = declaration.as_object()?;
            let name = declaration
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let mut function = Map::new();
            function.insert("name".to_string(), Value::String(name.to_string()));
            if let Some(description) = declaration.get("description").and_then(Value::as_str) {
                if !description.trim().is_empty() {
                    function.insert(
                        "description".to_string(),
                        Value::String(description.trim().to_string()),
                    );
                }
            }
            function.insert(
                "parameters".to_string(),
                declaration
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"})),
            );
            normalized.push(json!({
                "type": "function",
                "function": Value::Object(function),
            }));
        }
    }
    if has_code_execution {
        normalized.push(build_openai_builtin_gemini_tool("codeExecution"));
    }
    if has_url_context {
        normalized.push(build_openai_builtin_gemini_tool("urlContext"));
    }
    if normalized.is_empty() {
        Some(None)
    } else {
        Some(Some(normalized))
    }
}

fn normalize_gemini_tool_choice_to_openai(tool_config: Option<&Value>) -> Option<Option<Value>> {
    let Some(tool_config) = tool_config else {
        return Some(None);
    };
    let tool_config = tool_config.as_object()?;
    let function_config = tool_config
        .get("functionCallingConfig")
        .or_else(|| tool_config.get("function_calling_config"))
        .and_then(Value::as_object)?;
    let mode = function_config
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    if let Some(name) = function_config
        .get("allowedFunctionNames")
        .or_else(|| function_config.get("allowed_function_names"))
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(Some(json!({
            "type": "function",
            "function": { "name": name }
        })));
    }
    match mode.as_str() {
        "NONE" => Some(Some(Value::String("none".to_string()))),
        "AUTO" => Some(Some(Value::String("auto".to_string()))),
        "ANY" | "REQUIRED" => Some(Some(Value::String("required".to_string()))),
        _ => Some(None),
    }
}

fn extract_gemini_web_search_options(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    for tool in tools {
        let tool = tool.as_object()?;
        if tool.get("googleSearch").is_some() || tool.get("google_search").is_some() {
            return Some(json!({}));
        }
    }
    None
}

fn build_openai_builtin_gemini_tool(name: &str) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "parameters": {
                "type": "object",
                "properties": {}
            }
        }
    })
}

fn extract_gemini_model_from_path(path: &str) -> Option<String> {
    let marker = "/models/";
    let start = path.find(marker)? + marker.len();
    let tail = &path[start..];
    let end = tail.find(':').unwrap_or(tail.len());
    let model = tail[..end].trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_gemini_request_to_openai_chat_request;
    use serde_json::json;

    #[test]
    fn normalizes_gemini_seed_builtin_tools_and_specific_tool_choice() {
        let request = json!({
            "model": "gemini-2.5-pro",
            "contents": [
                {
                    "role": "user",
                    "parts": [{ "text": "use tools" }]
                }
            ],
            "generationConfig": {
                "maxOutputTokens": 256,
                "topK": 20,
                "seed": 7
            },
            "tools": [
                { "googleSearch": {} },
                { "codeExecution": {} },
                { "urlContext": {} },
                {
                    "functionDeclarations": [
                        {
                            "name": "lookupWeather",
                            "parameters": { "type": "object", "properties": { "city": { "type": "string" } } }
                        }
                    ]
                }
            ],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookupWeather"]
                }
            }
        });

        let normalized = normalize_gemini_request_to_openai_chat_request(
            &request,
            "/v1beta/models/gemini:generateContent",
        )
        .expect("request should convert");

        assert_eq!(normalized["max_completion_tokens"], 256);
        assert_eq!(normalized["top_k"], 20);
        assert_eq!(normalized["seed"], 7);
        assert_eq!(normalized["web_search_options"], json!({}));
        assert_eq!(
            normalized["tool_choice"],
            json!({
                "type": "function",
                "function": { "name": "lookupWeather" }
            })
        );
        assert_eq!(
            normalized["tools"],
            json!([
                {
                    "type": "function",
                    "function": {
                        "name": "lookupWeather",
                        "parameters": { "type": "object", "properties": { "city": { "type": "string" } } }
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
                }
            ])
        );
    }

    #[test]
    fn normalizes_gemini_multimodal_thought_and_passthrough_config() {
        let request = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [
                        { "text": "Look at these" },
                        { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo=" } },
                        { "inline_data": { "mime_type": "application/pdf", "data": "JVBERi0x" } },
                        { "inlineData": { "mimeType": "audio/mp3", "data": "SUQz" } },
                        {
                            "functionResponse": {
                                "name": "lookup",
                                "id": "call_1",
                                "response": { "result": { "city": "Shanghai" } }
                            }
                        }
                    ]
                },
                {
                    "role": "model",
                    "parts": [
                        { "text": "reasoning", "thought": true, "thoughtSignature": "sig_123" },
                        { "text": "done" },
                        { "fileData": { "fileUri": "https://example.com/cat.png", "mimeType": "image/png" } },
                        { "fileData": { "fileUri": "https://example.com/report.pdf", "mimeType": "application/pdf" } },
                        {
                            "functionCall": {
                                "name": "lookup",
                                "id": "call_1",
                                "args": { "city": "Shanghai" }
                            }
                        }
                    ]
                }
            ],
            "generation_config": {
                "max_output_tokens": 128,
                "stop_sequences": ["END"],
                "thinking_config": {
                    "includeThoughts": true,
                    "thinkingBudget": 4096
                },
                "responseModalities": ["TEXT", "IMAGE"],
                "candidate_count": 2,
                "presencePenalty": 0.5
            },
            "safetySettings": [{ "category": "HARM_CATEGORY_HATE_SPEECH" }],
            "cachedContent": "cached/123"
        });

        let normalized = normalize_gemini_request_to_openai_chat_request(
            &request,
            "/v1beta/models/gemini-2.5-pro:generateContent",
        )
        .expect("request should convert");

        assert_eq!(normalized["model"], "gemini-2.5-pro");
        assert_eq!(
            normalized["messages"][2]["reasoning_parts"],
            json!([
                {
                    "type": "thinking",
                    "thinking": "reasoning",
                    "signature": "sig_123"
                }
            ])
        );
        assert_eq!(
            normalized["messages"][0]["content"],
            json!([
                { "type": "text", "text": "Look at these" },
                {
                    "type": "image_url",
                    "image_url": { "url": "data:image/png;base64,iVBORw0KGgo=" }
                },
                {
                    "type": "file",
                    "file": { "file_data": "data:application/pdf;base64,JVBERi0x" }
                },
                {
                    "type": "input_audio",
                    "input_audio": { "data": "SUQz", "format": "mp3" }
                }
            ])
        );
        assert_eq!(normalized["messages"][1]["role"], "tool");
        assert_eq!(normalized["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(
            normalized["messages"][1]["content"],
            json!({ "city": "Shanghai" })
        );
        assert_eq!(normalized["messages"][2]["reasoning_content"], "reasoning");
        assert_eq!(
            normalized["messages"][2]["content"],
            json!([
                { "type": "text", "text": "done" },
                {
                    "type": "image_url",
                    "image_url": { "url": "https://example.com/cat.png" }
                },
                { "type": "text", "text": "[File: https://example.com/report.pdf]" }
            ])
        );
        assert_eq!(normalized["messages"][2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(normalized["max_completion_tokens"], 128);
        assert_eq!(normalized["n"], 2);
        assert_eq!(normalized["stop"], json!(["END"]));
        assert_eq!(normalized["reasoning_effort"], "high");
        assert_eq!(
            normalized["extra_body"]["google"]["response_modalities"],
            json!(["TEXT", "IMAGE"])
        );
        assert_eq!(
            normalized["extra_body"]["gemini"]["generation_config_extra"]["presencePenalty"],
            0.5
        );
        assert_eq!(
            normalized["extra_body"]["gemini"]["cached_content"],
            "cached/123"
        );
    }
}
