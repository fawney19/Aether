use serde_json::{json, Map, Value};

use super::shared::canonical_json_string;
use crate::planner::openai::map_thinking_budget_to_openai_reasoning_effort;

pub fn normalize_claude_request_to_openai_chat_request(body_json: &Value) -> Option<Value> {
    let request = body_json.as_object()?;
    let mut output = Map::new();
    let mut next_generated_tool_use_index = 0usize;
    if let Some(model) = request.get("model") {
        output.insert("model".to_string(), model.clone());
    }

    let mut messages = Vec::new();
    if let Some(system_text) = extract_claude_system_text(request.get("system")) {
        if !system_text.trim().is_empty() {
            messages.push(json!({
                "role": "system",
                "content": system_text,
            }));
        }
    }

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
                "user" => {
                    append_claude_user_message_to_openai_messages(
                        message_object.get("content"),
                        &mut messages,
                    )?;
                }
                "assistant" => {
                    messages.push(normalize_claude_assistant_message_to_openai_message(
                        message_object.get("content"),
                        &mut next_generated_tool_use_index,
                    )?);
                }
                _ => {}
            }
        }
    }
    output.insert("messages".to_string(), Value::Array(messages));

    if let Some(max_tokens) = request.get("max_tokens").cloned() {
        output.insert("max_completion_tokens".to_string(), max_tokens);
    }
    for passthrough_key in ["temperature", "top_p", "metadata", "stop", "stream"] {
        if let Some(value) = request.get(passthrough_key) {
            output.insert(passthrough_key.to_string(), value.clone());
        }
    }
    if output.get("stop").is_none() {
        if let Some(stop_sequences) = request
            .get("stop_sequences")
            .cloned()
            .filter(|value| !value.is_null())
        {
            output.insert("stop".to_string(), stop_sequences);
        }
    }
    if output.get("reasoning_effort").is_none() {
        if let Some(reasoning_effort) = extract_claude_output_reasoning_effort(request) {
            output.insert(
                "reasoning_effort".to_string(),
                Value::String(reasoning_effort.to_string()),
            );
        } else if let Some(thinking_budget) = request
            .get("thinking")
            .and_then(Value::as_object)
            .and_then(|thinking| thinking.get("budget_tokens"))
            .and_then(Value::as_u64)
        {
            output.insert(
                "reasoning_effort".to_string(),
                Value::String(
                    map_thinking_budget_to_openai_reasoning_effort(thinking_budget).to_string(),
                ),
            );
        }
    }
    if let Some(tools) = normalize_claude_tools_to_openai(request.get("tools"))? {
        output.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(web_search_options) = extract_claude_web_search_options(request.get("tools")) {
        output.insert("web_search_options".to_string(), web_search_options);
    }
    if let Some(tool_choice) = normalize_claude_tool_choice_to_openai(request.get("tool_choice"))? {
        output.insert("tool_choice".to_string(), tool_choice);
    }
    if let Some(parallel_tool_calls) =
        extract_claude_parallel_tool_calls(request.get("tool_choice"))
    {
        output.insert(
            "parallel_tool_calls".to_string(),
            Value::Bool(parallel_tool_calls),
        );
    }

    Some(Value::Object(output))
}

#[derive(Debug)]
enum ClaudeNormalizedBlock {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    ImageUrl(String),
    FileData(String),
    FileUrl(String),
    ToolUse {
        id: Option<String>,
        name: String,
        input: Option<Value>,
    },
    ToolResult {
        tool_use_id: String,
        content: Value,
    },
}

fn normalize_claude_content_blocks(content: &Value) -> Option<Vec<ClaudeNormalizedBlock>> {
    match content {
        Value::String(text) => Some(vec![ClaudeNormalizedBlock::Text(text.clone())]),
        Value::Array(blocks) => {
            let mut normalized = Vec::new();
            for block in blocks {
                let block = block.as_object()?;
                match block.get("type")?.as_str()? {
                    "text" => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        normalized.push(ClaudeNormalizedBlock::Text(text.to_string()));
                    }
                    "thinking" => {
                        let thinking = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .or_else(|| block.get("text").and_then(Value::as_str))
                            .unwrap_or_default();
                        normalized.push(ClaudeNormalizedBlock::Thinking {
                            text: thinking.to_string(),
                            signature: block
                                .get("signature")
                                .and_then(Value::as_str)
                                .filter(|value| !value.is_empty())
                                .map(ToOwned::to_owned),
                        });
                    }
                    "redacted_thinking" => {
                        let data = block
                            .get("data")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        normalized.push(ClaudeNormalizedBlock::RedactedThinking {
                            data: data.to_string(),
                        });
                    }
                    "image" => {
                        let source = block.get("source")?.as_object()?;
                        match source.get("type")?.as_str()? {
                            "base64" => {
                                let media_type = source
                                    .get("media_type")
                                    .and_then(Value::as_str)
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())?;
                                let data = source
                                    .get("data")
                                    .and_then(Value::as_str)
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())?;
                                normalized.push(ClaudeNormalizedBlock::ImageUrl(build_data_url(
                                    media_type, data,
                                )));
                            }
                            "url" => {
                                let url = source
                                    .get("url")
                                    .and_then(Value::as_str)
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())?
                                    .to_string();
                                normalized.push(ClaudeNormalizedBlock::ImageUrl(url));
                            }
                            _ => {}
                        }
                    }
                    "document" => {
                        let source = block.get("source")?.as_object()?;
                        match source.get("type")?.as_str()? {
                            "base64" => {
                                let media_type = source
                                    .get("media_type")
                                    .and_then(Value::as_str)
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())?;
                                let data = source
                                    .get("data")
                                    .and_then(Value::as_str)
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())?;
                                normalized.push(ClaudeNormalizedBlock::FileData(build_data_url(
                                    media_type, data,
                                )));
                            }
                            "url" => {
                                let url = source
                                    .get("url")
                                    .and_then(Value::as_str)
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())?
                                    .to_string();
                                normalized.push(ClaudeNormalizedBlock::FileUrl(url));
                            }
                            _ => {}
                        }
                    }
                    "tool_use" => {
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())?
                            .to_string();
                        normalized.push(ClaudeNormalizedBlock::ToolUse {
                            id: block
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|value| !value.is_empty())
                                .map(ToOwned::to_owned),
                            name,
                            input: block.get("input").cloned(),
                        });
                    }
                    "tool_result" => {
                        let tool_use_id = block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())?
                            .to_string();
                        let content = block.get("content").cloned().unwrap_or(Value::Null);
                        normalized.push(ClaudeNormalizedBlock::ToolResult {
                            tool_use_id,
                            content,
                        });
                    }
                    _ => {}
                }
            }
            Some(normalized)
        }
        _ => None,
    }
}

fn append_claude_user_message_to_openai_messages(
    content: Option<&Value>,
    messages: &mut Vec<Value>,
) -> Option<()> {
    let Some(content) = content else {
        return Some(());
    };
    let mut pending_parts = Vec::new();
    for block in normalize_claude_content_blocks(content)? {
        match block {
            ClaudeNormalizedBlock::Text(text) | ClaudeNormalizedBlock::Thinking { text, .. } => {
                push_openai_text_part(&mut pending_parts, text);
            }
            ClaudeNormalizedBlock::RedactedThinking { .. } => {}
            ClaudeNormalizedBlock::ImageUrl(url) => {
                pending_parts.push(build_openai_image_part(url));
            }
            ClaudeNormalizedBlock::FileData(file_data) => {
                pending_parts.push(build_openai_file_part(file_data));
            }
            ClaudeNormalizedBlock::FileUrl(url) => {
                push_openai_text_part(&mut pending_parts, format!("[File: {url}]"));
            }
            ClaudeNormalizedBlock::ToolResult {
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
            ClaudeNormalizedBlock::ToolUse { .. } => {}
        }
    }
    flush_openai_user_content_parts(&mut pending_parts, messages);
    Some(())
}

fn normalize_claude_assistant_message_to_openai_message(
    content: Option<&Value>,
    next_generated_tool_use_index: &mut usize,
) -> Option<Value> {
    let mut reasoning_segments = Vec::new();
    let mut reasoning_parts = Vec::new();
    let mut content_parts = Vec::new();
    let mut tool_calls = Vec::new();
    if let Some(content) = content {
        for block in normalize_claude_content_blocks(content)? {
            match block {
                ClaudeNormalizedBlock::Text(text) => {
                    push_openai_text_part(&mut content_parts, text);
                }
                ClaudeNormalizedBlock::Thinking { text, signature } => {
                    if !text.trim().is_empty() {
                        reasoning_segments.push(text.clone());
                    }
                    let mut reasoning_part = Map::new();
                    reasoning_part
                        .insert("type".to_string(), Value::String("thinking".to_string()));
                    reasoning_part.insert("thinking".to_string(), Value::String(text));
                    if let Some(signature) = signature {
                        reasoning_part.insert("signature".to_string(), Value::String(signature));
                    }
                    reasoning_parts.push(Value::Object(reasoning_part));
                }
                ClaudeNormalizedBlock::RedactedThinking { data } => {
                    if data.trim().is_empty() {
                        continue;
                    }
                    reasoning_parts.push(json!({
                        "type": "redacted_thinking",
                        "data": data,
                    }));
                }
                ClaudeNormalizedBlock::ImageUrl(url) => {
                    content_parts.push(build_openai_image_part(url));
                }
                ClaudeNormalizedBlock::FileData(file_data) => {
                    content_parts.push(build_openai_file_part(file_data));
                }
                ClaudeNormalizedBlock::FileUrl(url) => {
                    push_openai_text_part(&mut content_parts, format!("[File: {url}]"));
                }
                ClaudeNormalizedBlock::ToolUse { id, name, input } => {
                    let tool_use_id = id.unwrap_or_else(|| {
                        let generated = format!("toolu_auto_{next_generated_tool_use_index}");
                        *next_generated_tool_use_index += 1;
                        generated
                    });
                    tool_calls.push(json!({
                        "id": tool_use_id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": canonical_json_string(input.unwrap_or(Value::Object(Map::new()))),
                        }
                    }));
                }
                ClaudeNormalizedBlock::ToolResult { .. } => {}
            }
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

fn build_data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

fn extract_claude_output_reasoning_effort(request: &Map<String, Value>) -> Option<&'static str> {
    match request
        .get("output_config")
        .and_then(Value::as_object)
        .and_then(|config| config.get("effort"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_ascii_lowercase()
        .as_str()
    {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" | "xhigh" => Some("xhigh"),
        _ => None,
    }
}

fn extract_claude_system_text(system: Option<&Value>) -> Option<String> {
    let system = system?;
    let text = match system {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let mut segments = Vec::new();
            for block in blocks {
                let block = block.as_object()?;
                if block.get("type").and_then(Value::as_str).unwrap_or("text") == "text" {
                    let text = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if !text.trim().is_empty() {
                        segments.push(text.to_string());
                    }
                }
            }
            segments.join("\n\n")
        }
        _ => return None,
    };
    Some(strip_claude_billing_header(&text))
}

fn strip_claude_billing_header(text: &str) -> String {
    let trimmed = text.trim();
    let prefix = "x-anthropic-billing-header:";
    if !trimmed.to_ascii_lowercase().starts_with(prefix) {
        return trimmed.to_string();
    }
    let remainder = trimmed
        .split_once('\n')
        .map(|(_, rest)| rest.trim_start())
        .unwrap_or_default();
    remainder.trim_start_matches('\n').trim().to_string()
}

fn normalize_claude_tools_to_openai(tools: Option<&Value>) -> Option<Option<Vec<Value>>> {
    let Some(tools) = tools else {
        return Some(None);
    };
    let tools = tools.as_array()?;
    let mut normalized = Vec::new();
    for tool in tools {
        let tool = tool.as_object()?;
        if tool
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value.starts_with("web_search"))
        {
            continue;
        }
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let mut function = Map::new();
        function.insert("name".to_string(), Value::String(name.to_string()));
        if let Some(description) = tool.get("description").and_then(Value::as_str) {
            if !description.trim().is_empty() {
                function.insert(
                    "description".to_string(),
                    Value::String(description.trim().to_string()),
                );
            }
        }
        function.insert(
            "parameters".to_string(),
            tool.get("input_schema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object"})),
        );
        normalized.push(json!({
            "type": "function",
            "function": Value::Object(function),
        }));
    }
    if normalized.is_empty() {
        Some(None)
    } else {
        Some(Some(normalized))
    }
}

fn extract_claude_web_search_options(tools: Option<&Value>) -> Option<Value> {
    let tools = tools?.as_array()?;
    for tool in tools {
        let tool = tool.as_object()?;
        let tool_type = tool
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if !tool_type.starts_with("web_search") {
            continue;
        }
        let found = true;
        let mut options = Map::new();
        if let Some(max_uses) = tool.get("max_uses").and_then(Value::as_u64) {
            let search_context_size = if max_uses <= 1 {
                "low"
            } else if max_uses <= 5 {
                "medium"
            } else {
                "high"
            };
            options.insert(
                "search_context_size".to_string(),
                Value::String(search_context_size.to_string()),
            );
        }
        if let Some(user_location) = tool.get("user_location").and_then(Value::as_object) {
            let mut approximate = Map::new();
            for field in ["city", "country", "region", "timezone"] {
                if let Some(value) = user_location.get(field).cloned() {
                    approximate.insert(field.to_string(), value);
                }
            }
            if !approximate.is_empty() {
                options.insert(
                    "user_location".to_string(),
                    json!({
                        "type": "approximate",
                        "approximate": approximate,
                    }),
                );
            }
        }
        if found {
            return Some(Value::Object(options));
        }
    }
    None
}

fn normalize_claude_tool_choice_to_openai(tool_choice: Option<&Value>) -> Option<Option<Value>> {
    let Some(tool_choice) = tool_choice else {
        return Some(None);
    };
    match tool_choice {
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Some(Value::String("auto".to_string()))),
            "any" => Some(Some(Value::String("required".to_string()))),
            "none" => Some(Some(Value::String("none".to_string()))),
            _ => Some(None),
        },
        Value::Object(value) => {
            if let Some(name) = value.get("name").and_then(Value::as_str) {
                return Some(Some(json!({
                    "type": "function",
                    "function": { "name": name }
                })));
            }
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match kind.trim().to_ascii_lowercase().as_str() {
                "auto" => Some(Some(Value::String("auto".to_string()))),
                "any" => Some(Some(Value::String("required".to_string()))),
                "none" => Some(Some(Value::String("none".to_string()))),
                "tool" => value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| {
                        Some(json!({
                            "type": "function",
                            "function": { "name": name }
                        }))
                    })
                    .or(Some(None)),
                _ => Some(None),
            }
        }
        _ => Some(None),
    }
}

fn extract_claude_parallel_tool_calls(tool_choice: Option<&Value>) -> Option<bool> {
    let tool_choice = tool_choice?.as_object()?;
    let choice_type = tool_choice
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if choice_type == "none" {
        return None;
    }
    tool_choice
        .get("disable_parallel_tool_use")
        .and_then(Value::as_bool)
        .map(|value| !value)
}

#[cfg(test)]
mod tests {
    use super::normalize_claude_request_to_openai_chat_request;
    use serde_json::json;

    #[test]
    fn assigns_deterministic_tool_use_ids_when_claude_blocks_omit_ids() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "search",
                            "input": {"query": "alpha"}
                        },
                        {
                            "type": "tool_use",
                            "name": "search",
                            "input": {"query": "beta"}
                        }
                    ]
                }
            ]
        });

        let first = normalize_claude_request_to_openai_chat_request(&request)
            .expect("request should convert");
        let second = normalize_claude_request_to_openai_chat_request(&request)
            .expect("request should convert");

        assert_eq!(first, second);
        assert_eq!(first["messages"][0]["tool_calls"][0]["id"], "toolu_auto_0");
        assert_eq!(first["messages"][0]["tool_calls"][1]["id"], "toolu_auto_1");
    }

    #[test]
    fn preserves_explicit_claude_tool_use_ids() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_explicit_1",
                            "name": "search",
                            "input": {"query": "alpha"}
                        }
                    ]
                }
            ]
        });

        let normalized = normalize_claude_request_to_openai_chat_request(&request)
            .expect("request should convert");

        assert_eq!(
            normalized["messages"][0]["tool_calls"][0]["id"],
            "toolu_explicit_1"
        );
    }

    #[test]
    fn extracts_claude_web_search_and_parallel_settings() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "tools": [
                {
                    "type": "web_search_20250305",
                    "name": "web_search",
                    "max_uses": 10,
                    "user_location": {
                        "type": "approximate",
                        "city": "Shanghai",
                        "country": "CN",
                        "timezone": "Asia/Shanghai"
                    }
                }
            ],
            "tool_choice": {
                "type": "auto",
                "disable_parallel_tool_use": true
            },
            "messages": [
                {
                    "role": "user",
                    "content": "find something"
                }
            ]
        });

        let normalized = normalize_claude_request_to_openai_chat_request(&request)
            .expect("request should convert");

        assert_eq!(
            normalized["web_search_options"]["search_context_size"],
            "high"
        );
        assert_eq!(
            normalized["web_search_options"]["user_location"],
            json!({
                "type": "approximate",
                "approximate": {
                    "city": "Shanghai",
                    "country": "CN",
                    "timezone": "Asia/Shanghai"
                }
            })
        );
        assert_eq!(normalized["parallel_tool_calls"], false);
        assert!(normalized.get("tools").is_none());
    }

    #[test]
    fn normalizes_claude_media_thinking_and_stop_sequences() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "See attachment" },
                        {
                            "type": "image",
                            "source": {
                                "type": "url",
                                "url": "https://example.com/cat.png"
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
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "need context first",
                            "signature": "sig_123"
                        },
                        { "type": "redacted_thinking", "data": "redacted_blob" },
                        { "type": "text", "text": "Working on it" }
                    ]
                }
            ],
            "stop_sequences": ["END"],
            "output_config": { "effort": "max" }
        });

        let normalized = normalize_claude_request_to_openai_chat_request(&request)
            .expect("request should convert");

        assert_eq!(normalized["stop"], json!(["END"]));
        assert_eq!(normalized["reasoning_effort"], "xhigh");
        assert_eq!(
            normalized["messages"][0]["content"],
            json!([
                { "type": "text", "text": "See attachment" },
                {
                    "type": "image_url",
                    "image_url": { "url": "https://example.com/cat.png" }
                },
                {
                    "type": "file",
                    "file": {
                        "file_data": "data:application/pdf;base64,JVBERi0x"
                    }
                }
            ])
        );
        assert_eq!(
            normalized["messages"][1]["reasoning_content"],
            "need context first"
        );
        assert_eq!(
            normalized["messages"][1]["reasoning_parts"],
            json!([
                {
                    "type": "thinking",
                    "thinking": "need context first",
                    "signature": "sig_123"
                },
                {
                    "type": "redacted_thinking",
                    "data": "redacted_blob"
                }
            ])
        );
        assert_eq!(normalized["messages"][1]["content"], "Working on it");
    }

    #[test]
    fn preserves_default_claude_web_search_tool_as_empty_openai_options() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "tools": [
                {
                    "type": "web_search_20250305",
                    "name": "web_search"
                }
            ],
            "messages": [
                {
                    "role": "user",
                    "content": "find something"
                }
            ]
        });

        let normalized = normalize_claude_request_to_openai_chat_request(&request)
            .expect("request should convert");

        assert_eq!(normalized["web_search_options"], json!({}));
        assert!(normalized.get("tools").is_none());
    }
}
