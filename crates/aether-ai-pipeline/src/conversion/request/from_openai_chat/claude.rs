use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::super::to_openai_chat::{extract_openai_text_content, parse_openai_tool_result_content};
use super::shared::parse_openai_tool_arguments;
use crate::planner::openai::{
    copy_request_number_field, extract_openai_reasoning_effort,
    map_openai_reasoning_effort_to_claude_output, map_openai_reasoning_effort_to_thinking_budget,
    parse_openai_stop_sequences, resolve_openai_chat_max_tokens,
};

pub fn convert_openai_chat_request_to_claude_request(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    let request = body_json.as_object()?;
    let mut system_segments = Vec::new();
    let mut messages = Vec::new();

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
                    let blocks = convert_openai_content_to_claude_blocks(
                        message_object.get("content"),
                        ClaudeMessageRole::User,
                    )?;
                    if !blocks.is_empty() {
                        messages.push(build_claude_message("user", blocks));
                    }
                }
                "assistant" => {
                    let mut blocks = extract_openai_reasoning_to_claude_blocks(message_object);
                    blocks.extend(convert_openai_content_to_claude_blocks(
                        message_object.get("content"),
                        ClaudeMessageRole::Assistant,
                    )?);
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
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": tool_call_id,
                                "name": tool_name,
                                "input": tool_input,
                            }));
                        }
                    }
                    if !blocks.is_empty() {
                        messages.push(build_claude_message("assistant", blocks));
                    }
                }
                "tool" => {
                    let tool_use_id = message_object
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?
                        .to_string();
                    let tool_result =
                        parse_openai_tool_result_content(message_object.get("content"));
                    messages.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": tool_result,
                            "is_error": false,
                        }],
                    }));
                }
                _ => {}
            }
        }
    }

    let mut output = Map::new();
    output.insert("model".to_string(), Value::String(mapped_model.to_string()));
    output.insert(
        "messages".to_string(),
        Value::Array(compact_claude_messages(messages)),
    );
    output.insert(
        "max_tokens".to_string(),
        Value::from(resolve_openai_chat_max_tokens(request)),
    );

    let system_text = system_segments
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system_text.is_empty() {
        output.insert("system".to_string(), Value::String(system_text));
    }
    if upstream_is_stream {
        output.insert("stream".to_string(), Value::Bool(true));
    }
    copy_request_number_field(request, &mut output, "temperature");
    copy_request_number_field(request, &mut output, "top_p");
    copy_request_number_field(request, &mut output, "top_k");
    if let Some(stop_sequences) = parse_openai_stop_sequences(request.get("stop")) {
        output.insert("stop_sequences".to_string(), Value::Array(stop_sequences));
    }
    if let Some(tools) =
        convert_openai_tools_to_claude(request.get("tools"), request.get("web_search_options"))
    {
        output.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(tool_choice) = convert_openai_tool_choice_to_claude(
        request.get("tool_choice"),
        request.get("parallel_tool_calls"),
    ) {
        output.insert("tool_choice".to_string(), tool_choice);
    }
    if let Some(metadata) = request.get("metadata").cloned() {
        output.insert("metadata".to_string(), metadata);
    }
    if let Some(reasoning_effort) = extract_openai_reasoning_effort(request) {
        if let Some(thinking_budget) =
            map_openai_reasoning_effort_to_thinking_budget(reasoning_effort.as_str())
        {
            output.insert(
                "thinking".to_string(),
                json!({
                    "type": "enabled",
                    "budget_tokens": thinking_budget,
                }),
            );
        }
        if let Some(output_effort) =
            map_openai_reasoning_effort_to_claude_output(reasoning_effort.as_str())
        {
            output.insert(
                "output_config".to_string(),
                json!({
                    "effort": output_effort,
                }),
            );
        }
    }

    Some(Value::Object(output))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeMessageRole {
    User,
    Assistant,
}

fn convert_openai_content_to_claude_blocks(
    content: Option<&Value>,
    role: ClaudeMessageRole,
) -> Option<Vec<Value>> {
    match content {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Some(Vec::new())
            } else {
                Some(vec![json!({ "type": "text", "text": text })])
            }
        }
        Some(Value::Array(parts)) => {
            let mut blocks = Vec::new();
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
                                blocks.push(json!({ "type": "text", "text": text }));
                            }
                        }
                    }
                    "image_url" | "input_image" | "output_image" => {
                        let url = part_object
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
                            })
                            .filter(|value| !value.trim().is_empty())?;
                        if role == ClaudeMessageRole::User {
                            if let Some((media_type, data)) = parse_data_url(url.as_str()) {
                                blocks.push(json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": data,
                                    }
                                }));
                            } else {
                                blocks.push(json!({
                                    "type": "image",
                                    "source": {
                                        "type": "url",
                                        "url": url,
                                    }
                                }));
                            }
                        } else {
                            blocks.push(json!({
                                "type": "text",
                                "text": assistant_image_placeholder(url.as_str()),
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
                            if let Some((media_type, data)) = parse_data_url(file_data) {
                                blocks.push(json!({
                                    "type": "document",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
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
                            blocks.push(json!({
                                "type": "text",
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
                            blocks.push(json!({
                                "type": "document",
                                "source": {
                                    "type": "base64",
                                    "media_type": format!("audio/{format}"),
                                    "data": data,
                                }
                            }));
                        }
                    }
                    _ => {}
                }
            }
            Some(blocks)
        }
        _ => None,
    }
}

fn extract_openai_reasoning_to_claude_blocks(message: &Map<String, Value>) -> Vec<Value> {
    let mut blocks = Vec::new();
    if let Some(reasoning_parts) = message.get("reasoning_parts").and_then(Value::as_array) {
        for reasoning_part in reasoning_parts {
            let Some(reasoning_object) = reasoning_part.as_object() else {
                continue;
            };
            match reasoning_object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("thinking")
            {
                "thinking" => {
                    let thinking = reasoning_object
                        .get("thinking")
                        .or_else(|| reasoning_object.get("text"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default();
                    if thinking.is_empty() {
                        continue;
                    }
                    let mut block = Map::new();
                    block.insert("type".to_string(), Value::String("thinking".to_string()));
                    block.insert("thinking".to_string(), Value::String(thinking.to_string()));
                    if let Some(signature) = reasoning_object
                        .get("signature")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    {
                        block.insert(
                            "signature".to_string(),
                            Value::String(signature.to_string()),
                        );
                    }
                    blocks.push(Value::Object(block));
                }
                "redacted_thinking" => {
                    let data = reasoning_object
                        .get("data")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default();
                    if !data.is_empty() {
                        blocks.push(json!({
                            "type": "redacted_thinking",
                            "data": data,
                        }));
                    }
                }
                _ => {}
            }
        }
    }
    if !blocks.is_empty() {
        return blocks;
    }
    if let Some(reasoning_content) = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        blocks.push(json!({
            "type": "thinking",
            "thinking": reasoning_content,
        }));
    }
    blocks
}

fn convert_openai_tools_to_claude(
    tools: Option<&Value>,
    web_search_options: Option<&Value>,
) -> Option<Vec<Value>> {
    let mut converted = Vec::new();
    if let Some(tool_values) = tools.and_then(Value::as_array) {
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
            let mut converted_tool = Map::new();
            converted_tool.insert("name".to_string(), Value::String(name.to_string()));
            if let Some(description) = function.get("description").cloned() {
                converted_tool.insert("description".to_string(), description);
            }
            converted_tool.insert(
                "input_schema".to_string(),
                function
                    .get("parameters")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            );
            converted.push(Value::Object(converted_tool));
        }
    }
    if let Some(web_search_tool) =
        convert_openai_web_search_options_to_claude_tool(web_search_options)
    {
        converted.push(web_search_tool);
    }
    (!converted.is_empty()).then_some(converted)
}

fn convert_openai_web_search_options_to_claude_tool(
    web_search_options: Option<&Value>,
) -> Option<Value> {
    let web_search_options = web_search_options?.as_object()?;
    let mut tool = Map::new();
    tool.insert(
        "type".to_string(),
        Value::String("web_search_20250305".to_string()),
    );
    tool.insert("name".to_string(), Value::String("web_search".to_string()));
    if let Some(user_location) = web_search_options
        .get("user_location")
        .and_then(Value::as_object)
    {
        let approximate = user_location
            .get("approximate")
            .and_then(Value::as_object)
            .unwrap_or(user_location);
        let mut location = Map::new();
        location.insert("type".to_string(), Value::String("approximate".to_string()));
        for field in ["city", "country", "region", "timezone"] {
            if let Some(value) = approximate.get(field).cloned() {
                location.insert(field.to_string(), value);
            }
        }
        if location.len() > 1 {
            tool.insert("user_location".to_string(), Value::Object(location));
        }
    }
    if let Some(max_uses) = web_search_options
        .get("search_context_size")
        .and_then(Value::as_str)
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "low" => Some(1u64),
            "medium" => Some(5u64),
            "high" => Some(10u64),
            _ => None,
        })
    {
        tool.insert("max_uses".to_string(), Value::from(max_uses));
    }
    Some(Value::Object(tool))
}

fn convert_openai_tool_choice_to_claude(
    tool_choice: Option<&Value>,
    parallel_tool_calls: Option<&Value>,
) -> Option<Value> {
    let mut converted = match tool_choice {
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "none" => Some(json!({ "type": "none" })),
            "required" => Some(json!({ "type": "any" })),
            "auto" => Some(json!({ "type": "auto" })),
            _ => None,
        },
        Some(Value::Object(object)) => {
            let function_name = object
                .get("function")
                .and_then(Value::as_object)
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(json!({
                "type": "tool",
                "name": function_name,
            }))
        }
        Some(_) => None,
        None => None,
    };
    if let Some(parallel_tool_calls) = parallel_tool_calls.and_then(Value::as_bool) {
        if converted.is_none() {
            converted = Some(json!({ "type": "auto" }));
        }
        if let Some(object) = converted.as_mut().and_then(Value::as_object_mut) {
            let choice_type = object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if choice_type != "none" {
                object.insert(
                    "disable_parallel_tool_use".to_string(),
                    Value::Bool(!parallel_tool_calls),
                );
            }
        }
    }
    converted
}

fn compact_claude_messages(messages: Vec<Value>) -> Vec<Value> {
    let mut compact: Vec<Value> = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(last) = compact.last_mut() {
            let last_role = last
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if last_role == role {
                merge_claude_message_content(last, message);
                continue;
            }
        }
        compact.push(message);
    }
    if compact
        .first()
        .and_then(|value| value.get("role"))
        .and_then(Value::as_str)
        .is_some_and(|value| value == "assistant")
    {
        compact.insert(0, json!({ "role": "user", "content": "" }));
    }
    compact
}

fn merge_claude_message_content(target: &mut Value, message: Value) {
    let Some(target_object) = target.as_object_mut() else {
        return;
    };
    let incoming_content = message.get("content").cloned().unwrap_or(Value::Null);
    let merged_blocks = extract_claude_content_blocks(target_object.get("content"))
        .into_iter()
        .chain(extract_claude_content_blocks(Some(&incoming_content)))
        .collect::<Vec<_>>();
    target_object.insert(
        "content".to_string(),
        simplify_claude_content(merged_blocks),
    );
}

fn build_claude_message(role: &str, blocks: Vec<Value>) -> Value {
    json!({
        "role": role,
        "content": simplify_claude_content(blocks),
    })
}

fn simplify_claude_content(blocks: Vec<Value>) -> Value {
    if blocks.is_empty() {
        return Value::String(String::new());
    }
    let mut text_values = Vec::new();
    for block in &blocks {
        let Some(block_object) = block.as_object() else {
            return Value::Array(blocks);
        };
        if block_object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "text")
        {
            if let Some(text) = block_object.get("text").and_then(Value::as_str) {
                text_values.push(text.to_string());
                continue;
            }
        }
        return Value::Array(blocks);
    }
    Value::String(text_values.join("\n"))
}

fn extract_claude_content_blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) if !text.is_empty() => vec![json!({
            "type": "text",
            "text": text,
        })],
        Some(Value::Array(blocks)) => blocks.clone(),
        _ => Vec::new(),
    }
}

fn parse_data_url(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(",")?;
    let media_type = meta.strip_suffix(";base64")?;
    if media_type.trim().is_empty() || data.trim().is_empty() {
        return None;
    }
    Some((media_type.to_string(), data.to_string()))
}

fn assistant_image_placeholder(url: &str) -> String {
    if url.starts_with("data:") {
        "[Image]".to_string()
    } else {
        format!("[Image: {url}]")
    }
}

#[cfg(test)]
mod tests {
    use super::convert_openai_chat_request_to_claude_request;
    use serde_json::json;

    #[test]
    fn maps_openai_web_search_options_to_claude_builtin_tool() {
        let request = json!({
            "model": "gpt-5.4",
            "messages": [
                { "role": "user", "content": "weather in shanghai" }
            ],
            "web_search_options": {
                "search_context_size": "medium",
                "user_location": {
                    "approximate": {
                        "city": "Shanghai",
                        "country": "CN",
                        "timezone": "Asia/Shanghai"
                    }
                }
            }
        });

        let converted =
            convert_openai_chat_request_to_claude_request(&request, "claude-sonnet-4-5", false)
                .expect("request should convert");

        assert_eq!(converted["tools"][0]["type"], "web_search_20250305");
        assert_eq!(converted["tools"][0]["name"], "web_search");
        assert_eq!(converted["tools"][0]["max_uses"], 5);
        assert_eq!(
            converted["tools"][0]["user_location"],
            json!({
                "type": "approximate",
                "city": "Shanghai",
                "country": "CN",
                "timezone": "Asia/Shanghai",
            })
        );
    }

    #[test]
    fn maps_parallel_tool_calls_to_disable_parallel_tool_use() {
        let request = json!({
            "model": "gpt-5.4",
            "messages": [
                { "role": "user", "content": "call tools if needed" }
            ],
            "parallel_tool_calls": true
        });

        let converted =
            convert_openai_chat_request_to_claude_request(&request, "claude-sonnet-4-5", false)
                .expect("request should convert");

        assert_eq!(
            converted["tool_choice"],
            json!({
                "type": "auto",
                "disable_parallel_tool_use": false,
            })
        );
    }

    #[test]
    fn converts_openai_multipart_reasoning_and_file_id_to_claude_request() {
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
                        },
                        {
                            "type": "redacted_thinking",
                            "data": "redacted_blob"
                        }
                    ],
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": "https://example.com/diagram.png"
                            }
                        },
                        { "type": "file", "file": { "file_id": "file_123" } },
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
            "reasoning_effort": "xhigh"
        });

        let converted =
            convert_openai_chat_request_to_claude_request(&request, "claude-sonnet-4-5", false)
                .expect("request should convert");

        assert_eq!(
            converted["messages"][0]["content"],
            json!([
                { "type": "text", "text": "Read this" },
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "iVBORw0KGgo="
                    }
                },
                {
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": "JVBERi0x"
                    }
                }
            ])
        );
        assert_eq!(converted["messages"][1]["content"][0]["type"], "thinking");
        assert_eq!(
            converted["messages"][1]["content"][0]["signature"],
            "sig_123"
        );
        assert_eq!(
            converted["messages"][1]["content"][1]["type"],
            "redacted_thinking"
        );
        assert_eq!(
            converted["messages"][1]["content"][2]["text"],
            "[Image: https://example.com/diagram.png]"
        );
        assert_eq!(
            converted["messages"][1]["content"][3]["text"],
            "[File: file_123]"
        );
        assert_eq!(
            converted["messages"][1]["content"][5]["input"],
            json!({"raw": "tokyo"})
        );
        assert_eq!(converted["thinking"]["budget_tokens"], 8192);
        assert_eq!(converted["output_config"]["effort"], "max");
    }
}
