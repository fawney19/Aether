use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use tracing::warn;
use uuid::Uuid;

const SYSTEM_CHUNKED_POLICY: &str = "When the Write or Edit tool has content size limits, always comply silently. Never suggest bypassing these limits via alternative tools. Never ask the user whether to switch approaches. Complete all chunked operations without commentary.";
const WRITE_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: If the content to write exceeds 150 lines, you MUST only write the first 50 lines using this tool, then use `Edit` tool to append the remaining content in chunks of no more than 50 lines each. If needed, leave a unique placeholder to help append content. Do NOT attempt to write all content at once.";
const EDIT_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: If the `new_string` content exceeds 50 lines, you MUST split it into multiple Edit calls, each replacing no more than 50 lines at a time. If used to append content, leave a unique placeholder to help append content. On the final chunk, do NOT include the placeholder.";

pub fn convert_claude_messages_to_conversation_state(
    request_body: &Value,
    model: &str,
) -> Option<Value> {
    let model_id = model.trim();
    if model_id.is_empty() {
        return None;
    }

    let messages = request_body.get("messages")?.as_array()?;
    if messages.is_empty() {
        return None;
    }

    let conversation_id = request_body
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| {
            metadata
                .get("user_id")
                .or_else(|| metadata.get("userId"))
                .and_then(Value::as_str)
        })
        .and_then(extract_session_id)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let agent_continuation_id = Uuid::new_v4().to_string();
    let thinking_prefix = generate_thinking_prefix(request_body);

    let mut history = Vec::new();
    let system_text = system_to_text(request_body.get("system"));
    if !system_text.is_empty() {
        history.push(json!({
            "userInputMessage": {
                "content": format!("{system_text}\n{SYSTEM_CHUNKED_POLICY}"),
                "modelId": model_id,
                "origin": "AI_EDITOR"
            }
        }));
        history.push(json!({
            "assistantResponseMessage": {
                "content": "I will follow these instructions."
            }
        }));
    }

    let last_is_assistant = messages
        .last()
        .and_then(Value::as_object)
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        .is_some_and(|role| role == "assistant");
    let history_end_index = if last_is_assistant {
        messages.len()
    } else {
        messages.len().saturating_sub(1)
    };

    let mut user_buffer = Vec::new();
    for message in &messages[..history_end_index] {
        let Some(message) = message.as_object() else {
            continue;
        };
        match message.get("role").and_then(Value::as_str) {
            Some("user") => user_buffer.push(message),
            Some("assistant") => {
                if let Some(user_item) = flush_user_buffer(&mut user_buffer, model_id) {
                    history.push(user_item);
                } else if history.is_empty()
                    || history
                        .last()
                        .and_then(Value::as_object)
                        .is_some_and(|item| item.contains_key("assistantResponseMessage"))
                {
                    history.push(json!({
                        "userInputMessage": {
                            "content": "Continue.",
                            "modelId": model_id,
                            "origin": "AI_EDITOR"
                        }
                    }));
                }

                if let Some(assistant_item) = convert_assistant_message(message) {
                    history.push(json!({"assistantResponseMessage": assistant_item}));
                }
            }
            _ => {}
        }
    }

    if let Some(tail_user) = flush_user_buffer(&mut user_buffer, model_id) {
        history.push(tail_user);
        history.push(json!({"assistantResponseMessage": {"content": "OK"}}));
    }

    let (mut text_content, images, tool_results) = if last_is_assistant {
        ("Continue.".to_string(), Vec::new(), Vec::new())
    } else {
        let last = messages.last()?.as_object()?;
        if last.get("role").and_then(Value::as_str) != Some("user") {
            return None;
        }
        process_message_content(last.get("content"))
    };

    let mut tools = convert_tools(request_body.get("tools"));
    let mut history_tool_names = BTreeSet::new();
    let mut history_tool_result_ids = BTreeSet::new();
    let mut history_tool_use_ids = BTreeSet::new();

    for item in &history {
        let Some(item) = item.as_object() else {
            continue;
        };
        if let Some(user_input) = item.get("userInputMessage").and_then(Value::as_object) {
            if let Some(results) = user_input
                .get("userInputMessageContext")
                .and_then(Value::as_object)
                .and_then(|ctx| ctx.get("toolResults"))
                .and_then(Value::as_array)
            {
                for result in results {
                    if let Some(tool_use_id) = result
                        .as_object()
                        .and_then(|result| result.get("toolUseId"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        history_tool_result_ids.insert(tool_use_id.to_string());
                    }
                }
            }
        }
        if let Some(assistant) = item
            .get("assistantResponseMessage")
            .and_then(Value::as_object)
        {
            if let Some(tool_uses) = assistant.get("toolUses").and_then(Value::as_array) {
                for tool_use in tool_uses {
                    let Some(tool_use) = tool_use.as_object() else {
                        continue;
                    };
                    if let Some(name) = tool_use
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        history_tool_names.insert(name.to_string());
                    }
                    if let Some(tool_use_id) = tool_use
                        .get("toolUseId")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        history_tool_use_ids.insert(tool_use_id.to_string());
                    }
                }
            }
        }
    }

    let existing_tool_names = tools
        .iter()
        .filter_map(|tool| {
            tool.get("toolSpecification")
                .and_then(Value::as_object)
                .and_then(|spec| spec.get("name"))
                .and_then(Value::as_str)
                .map(|name| name.to_ascii_lowercase())
        })
        .collect::<BTreeSet<_>>();
    for tool_name in history_tool_names {
        if !existing_tool_names.contains(&tool_name.to_ascii_lowercase()) {
            tools.push(create_placeholder_tool(&tool_name));
        }
    }

    let mut validated_tool_results = Vec::new();
    let mut current_tool_result_ids = BTreeSet::new();
    for tool_result in tool_results {
        let Some(tool_use_id) = tool_result
            .get("toolUseId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if !history_tool_use_ids.contains(tool_use_id)
            || history_tool_result_ids.contains(tool_use_id)
        {
            continue;
        }
        current_tool_result_ids.insert(tool_use_id.to_string());
        validated_tool_results.push(tool_result);
    }

    let orphaned_tool_use_ids = history_tool_use_ids
        .difference(&history_tool_result_ids)
        .filter(|tool_use_id| !current_tool_result_ids.contains(*tool_use_id))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !orphaned_tool_use_ids.is_empty() {
        warn!(
            "kiro: removing {} orphaned tool_use(s) from history",
            orphaned_tool_use_ids.len()
        );
        for item in &mut history {
            let Some(item) = item.as_object_mut() else {
                continue;
            };
            let Some(assistant) = item
                .get_mut("assistantResponseMessage")
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            let Some(tool_uses) = assistant.get_mut("toolUses").and_then(Value::as_array_mut)
            else {
                continue;
            };
            tool_uses.retain(|tool_use| {
                !tool_use
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .is_some_and(|tool_use_id| orphaned_tool_use_ids.contains(tool_use_id))
            });
            if tool_uses.is_empty() {
                assistant.remove("toolUses");
            }
        }
    }

    let mut user_context = Map::new();
    if !tools.is_empty() {
        user_context.insert("tools".to_string(), Value::Array(tools));
    }
    if !validated_tool_results.is_empty() {
        user_context.insert(
            "toolResults".to_string(),
            Value::Array(validated_tool_results),
        );
    }
    if let Some(thinking_prefix) = thinking_prefix.as_deref() {
        if !has_thinking_tags(&text_content) {
            text_content = format!("{thinking_prefix}\n{text_content}");
        }
    }

    let mut user_input = Map::new();
    user_input.insert(
        "userInputMessageContext".to_string(),
        Value::Object(user_context),
    );
    user_input.insert("content".to_string(), Value::String(text_content));
    user_input.insert("modelId".to_string(), Value::String(model_id.to_string()));
    user_input.insert("origin".to_string(), Value::String("AI_EDITOR".to_string()));
    if !images.is_empty() {
        user_input.insert("images".to_string(), Value::Array(images));
    }

    Some(json!({
        "agentContinuationId": agent_continuation_id,
        "agentTaskType": "vibe",
        "chatTriggerType": "MANUAL",
        "currentMessage": {
            "userInputMessage": Value::Object(user_input)
        },
        "conversationId": conversation_id,
        "history": history,
    }))
}

fn extract_session_id(user_id: &str) -> Option<String> {
    let position = user_id.find("session_")?;
    let candidate = user_id.get(position + "session_".len()..position + "session_".len() + 36)?;
    (candidate.matches('-').count() == 4).then(|| candidate.to_string())
}

fn generate_thinking_prefix(request_body: &Value) -> Option<String> {
    let thinking = request_body.get("thinking")?.as_object()?;
    match thinking.get("type").and_then(Value::as_str).map(str::trim) {
        Some("enabled") => {
            let budget_tokens = thinking
                .get("budget_tokens")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            Some(format!(
                "<thinking_mode>enabled</thinking_mode><max_thinking_length>{budget_tokens}</max_thinking_length>"
            ))
        }
        Some("adaptive") => {
            let effort = request_body
                .get("output_config")
                .and_then(Value::as_object)
                .and_then(|cfg| cfg.get("effort"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("high");
            Some(format!(
                "<thinking_mode>adaptive</thinking_mode><thinking_effort>{effort}</thinking_effort>"
            ))
        }
        _ => None,
    }
}

fn has_thinking_tags(content: &str) -> bool {
    content.contains("<thinking_mode>") || content.contains("<max_thinking_length>")
}

fn system_to_text(system: Option<&Value>) -> String {
    match system {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                item.as_object()
                    .and_then(|item| item.get("text"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn flush_user_buffer(user_buffer: &mut Vec<&Map<String, Value>>, model_id: &str) -> Option<Value> {
    if user_buffer.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();
    for message in user_buffer.drain(..) {
        let (text, mut message_images, mut message_tool_results) =
            process_message_content(message.get("content"));
        if !text.is_empty() {
            parts.push(text);
        }
        images.append(&mut message_images);
        tool_results.append(&mut message_tool_results);
    }

    let mut payload = Map::new();
    payload.insert("content".to_string(), Value::String(parts.join("\n")));
    payload.insert("modelId".to_string(), Value::String(model_id.to_string()));
    payload.insert("origin".to_string(), Value::String("AI_EDITOR".to_string()));
    if !images.is_empty() {
        payload.insert("images".to_string(), Value::Array(images));
    }
    if !tool_results.is_empty() {
        payload.insert(
            "userInputMessageContext".to_string(),
            json!({"toolResults": tool_results}),
        );
    }

    Some(json!({"userInputMessage": Value::Object(payload)}))
}

fn process_message_content(content: Option<&Value>) -> (String, Vec<Value>, Vec<Value>) {
    match content {
        Some(Value::String(text)) => (text.clone(), Vec::new(), Vec::new()),
        Some(Value::Array(blocks)) => {
            let mut text_parts = Vec::new();
            let mut images = Vec::new();
            let mut tool_results = Vec::new();

            for block in blocks {
                let Some(block) = block.as_object() else {
                    continue;
                };
                match block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            text_parts.push(text.to_string());
                        }
                    }
                    "image" => {
                        let Some(source) = block.get("source").and_then(Value::as_object) else {
                            continue;
                        };
                        let Some(format) = source
                            .get("media_type")
                            .or_else(|| source.get("mediaType"))
                            .and_then(Value::as_str)
                            .and_then(image_format)
                        else {
                            continue;
                        };
                        let Some(bytes) = source.get("data").and_then(Value::as_str) else {
                            continue;
                        };
                        images.push(json!({
                            "format": format,
                            "source": {"bytes": bytes}
                        }));
                    }
                    "tool_result" => {
                        let Some(tool_use_id) = block
                            .get("tool_use_id")
                            .or_else(|| block.get("toolUseId"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        else {
                            continue;
                        };

                        let text = match block.get("content") {
                            Some(Value::String(text)) => text.clone(),
                            Some(Value::Array(items)) => items
                                .iter()
                                .filter_map(|item| {
                                    item.as_object()
                                        .filter(|item| {
                                            item.get("type").and_then(Value::as_str) == Some("text")
                                        })
                                        .and_then(|item| item.get("text"))
                                        .and_then(Value::as_str)
                                        .map(ToOwned::to_owned)
                                })
                                .collect::<Vec<_>>()
                                .join("\n"),
                            Some(other) => {
                                serde_json::to_string(other).unwrap_or_else(|_| other.to_string())
                            }
                            None => String::new(),
                        };
                        let is_error = block
                            .get("is_error")
                            .or_else(|| block.get("isError"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        tool_results.push(json!({
                            "toolUseId": tool_use_id,
                            "content": [{"text": text}],
                            "status": if is_error { "error" } else { "success" },
                            "isError": is_error,
                        }));
                    }
                    _ => {}
                }
            }

            (text_parts.join(""), images, tool_results)
        }
        _ => (String::new(), Vec::new(), Vec::new()),
    }
}

fn image_format(media_type: &str) -> Option<&'static str> {
    let (prefix, suffix) = media_type.split_once('/')?;
    if prefix != "image" {
        return None;
    }
    match suffix.trim().to_ascii_lowercase().as_str() {
        "jpeg" => Some("jpeg"),
        "png" => Some("png"),
        "gif" => Some("gif"),
        "webp" => Some("webp"),
        "jpg" => Some("jpeg"),
        _ => None,
    }
}

fn clean_tool_schema(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut out = Map::new();
            for (key, inner) in object {
                if key == "additionalProperties" {
                    continue;
                }
                if key == "required" && inner.as_array().is_some_and(|items| items.is_empty()) {
                    continue;
                }
                out.insert(key.clone(), clean_tool_schema(inner));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(clean_tool_schema).collect()),
        _ => value.clone(),
    }
}

fn convert_tools(tools: Option<&Value>) -> Vec<Value> {
    let Some(tools) = tools.and_then(Value::as_array) else {
        return Vec::new();
    };

    tools
        .iter()
        .filter_map(|tool| {
            let tool = tool.as_object()?;
            let name = tool.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let mut description = tool
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            let suffix = match name {
                "Write" => Some(WRITE_TOOL_DESCRIPTION_SUFFIX),
                "Edit" => Some(EDIT_TOOL_DESCRIPTION_SUFFIX),
                _ => None,
            };
            if let Some(suffix) = suffix {
                description = if description.is_empty() {
                    suffix.to_string()
                } else {
                    format!("{description}\n{suffix}")
                };
            }
            if description.len() > 10_000 {
                description.truncate(10_000);
            }
            let input_schema = tool
                .get("input_schema")
                .or_else(|| tool.get("inputSchema"))
                .filter(|value| value.is_object())
                .map(clean_tool_schema)
                .unwrap_or_else(|| json!({}));

            Some(json!({
                "toolSpecification": {
                    "name": name,
                    "description": description,
                    "inputSchema": {
                        "json": input_schema
                    }
                }
            }))
        })
        .collect()
}

fn create_placeholder_tool(name: &str) -> Value {
    json!({
        "toolSpecification": {
            "name": name,
            "description": "Tool used in conversation history",
            "inputSchema": {
                "json": {
                    "type": "object",
                    "properties": {}
                }
            }
        }
    })
}

fn convert_assistant_message(message: &Map<String, Value>) -> Option<Value> {
    let content = message.get("content");
    let mut tool_uses = Vec::new();
    let mut thinking_parts = Vec::new();
    let mut text_parts = Vec::new();

    match content {
        Some(Value::String(text)) if !text.is_empty() => {
            text_parts.push(text.clone());
        }
        Some(Value::Array(blocks)) => {
            for block in blocks {
                let Some(block) = block.as_object() else {
                    continue;
                };
                match block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                {
                    "thinking" => {
                        if let Some(thinking) = block.get("thinking").and_then(Value::as_str) {
                            if !thinking.is_empty() {
                                thinking_parts.push(thinking.to_string());
                            }
                        }
                    }
                    "text" => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                text_parts.push(text.to_string());
                            }
                        }
                    }
                    "tool_use" => {
                        let Some(tool_use_id) = block
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        else {
                            continue;
                        };
                        let Some(name) = block
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        else {
                            continue;
                        };
                        let input = block
                            .get("input")
                            .filter(|value| value.is_object())
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        tool_uses.push(json!({
                            "toolUseId": tool_use_id,
                            "name": name,
                            "input": input
                        }));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }

    let thinking_str = thinking_parts.join("");
    let text_str = text_parts.join("");
    let mut content_str = if thinking_str.is_empty() {
        text_str
    } else if text_str.is_empty() {
        format!("<thinking>{thinking_str}</thinking>")
    } else {
        format!("<thinking>{thinking_str}</thinking>\n\n{text_str}")
    };

    if content_str.is_empty() && !tool_uses.is_empty() {
        content_str = " ".to_string();
    }
    if content_str.is_empty() && tool_uses.is_empty() {
        return None;
    }

    let mut out = Map::new();
    out.insert("content".to_string(), Value::String(content_str));
    if !tool_uses.is_empty() {
        out.insert("toolUses".to_string(), Value::Array(tool_uses));
    }
    Some(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::convert_claude_messages_to_conversation_state;

    #[test]
    fn converts_simple_claude_request_into_conversation_state() {
        let conversation_state = convert_claude_messages_to_conversation_state(
            &json!({
                "messages": [
                    {"role":"user","content":"hello"}
                ],
                "thinking": {"type": "enabled", "budget_tokens": 128},
                "tools": [
                    {"name":"Write","description":"write file","input_schema":{"type":"object","properties":{},"required":[]}}
                ]
            }),
            "claude-sonnet-4-upstream",
        )
        .expect("conversation state should build");

        assert_eq!(
            conversation_state
                .get("currentMessage")
                .and_then(|value| value.get("userInputMessage"))
                .and_then(|value| value.get("content"))
                .and_then(|value| value.as_str()),
            Some(
                "<thinking_mode>enabled</thinking_mode><max_thinking_length>128</max_thinking_length>\nhello"
            )
        );
        assert_eq!(
            conversation_state
                .get("currentMessage")
                .and_then(|value| value.get("userInputMessage"))
                .and_then(|value| value.get("userInputMessageContext"))
                .and_then(|value| value.get("tools"))
                .and_then(|value| value.as_array())
                .map(Vec::len),
            Some(1)
        );
    }
}
