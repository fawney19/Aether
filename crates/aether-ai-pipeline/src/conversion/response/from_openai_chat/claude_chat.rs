use serde_json::{json, Value};

use super::shared::{build_generated_tool_call_id, parse_openai_function_arguments};

pub fn convert_openai_chat_response_to_claude_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let body = body_json.as_object()?;
    let choices = body.get("choices")?.as_array()?;
    let first_choice = choices.first()?.as_object()?;
    let message = first_choice.get("message")?.as_object()?;
    let mut content = extract_openai_reasoning_to_claude_blocks(message);
    content.extend(convert_openai_assistant_content_to_claude_blocks(
        message.get("content"),
    )?);
    if let Some(tool_call_values) = message.get("tool_calls").and_then(Value::as_array) {
        for (index, tool_call) in tool_call_values.iter().enumerate() {
            let tool_call = tool_call.as_object()?;
            let function = tool_call.get("function")?.as_object()?;
            let tool_name = function
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let tool_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| build_generated_tool_call_id(index));
            let input = parse_openai_function_arguments(function.get("arguments"))?;
            content.push(json!({
                "type": "tool_use",
                "id": tool_id,
                "name": tool_name,
                "input": input,
            }));
        }
    }
    if content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": "",
        }));
    }

    let stop_reason = match first_choice.get("finish_reason").and_then(Value::as_str) {
        Some("stop") | None => "end_turn",
        Some("length") => "max_tokens",
        Some("tool_calls") | Some("function_call") => "tool_use",
        Some("content_filter") => "content_filtered",
        Some(other) => other,
    };
    let usage = body.get("usage").and_then(Value::as_object);
    let input_tokens = usage
        .and_then(|value| value.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|value| value.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("mapped_model").and_then(Value::as_str))
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or("unknown");
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("msg-local-finalize");

    Some(json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        }
    }))
    .map(|mut response| {
        if let Some(cached_tokens) = usage
            .and_then(|value| value.get("prompt_tokens_details"))
            .and_then(Value::as_object)
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
        {
            response["usage"]["cache_read_input_tokens"] = Value::from(cached_tokens);
        }
        if let Some(cached_creation_tokens) = usage
            .and_then(|value| value.get("prompt_tokens_details"))
            .and_then(Value::as_object)
            .and_then(|details| details.get("cached_creation_tokens"))
            .and_then(Value::as_u64)
        {
            response["usage"]["cache_creation_input_tokens"] = Value::from(cached_creation_tokens);
        }
        response
    })
}

fn extract_openai_reasoning_to_claude_blocks(
    message: &serde_json::Map<String, Value>,
) -> Vec<Value> {
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
                    let mut block = serde_json::Map::new();
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

fn convert_openai_assistant_content_to_claude_blocks(
    content: Option<&Value>,
) -> Option<Vec<Value>> {
    match content {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::String(text)) => {
            if text.trim().is_empty() {
                Some(Vec::new())
            } else {
                Some(vec![json!({
                    "type": "text",
                    "text": text,
                })])
            }
        }
        Some(Value::Array(parts)) => {
            let mut blocks = Vec::new();
            for part in parts {
                let part = part.as_object()?;
                let part_type = part
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                match part_type.as_str() {
                    "text" | "output_text" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            if !text.trim().is_empty() {
                                blocks.push(json!({
                                    "type": "text",
                                    "text": text,
                                }));
                            }
                        }
                    }
                    "image_url" | "output_image" => {
                        if let Some(url) = extract_openai_image_url(part) {
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
                        }
                    }
                    "file" => {
                        let file = part.get("file").and_then(Value::as_object).unwrap_or(part);
                        if let Some(file_data) = file.get("file_data").and_then(Value::as_str) {
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
                        } else if let Some(file_id) = file
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
                        let audio = part
                            .get("input_audio")
                            .and_then(Value::as_object)
                            .unwrap_or(part);
                        let data = audio
                            .get("data")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty());
                        let format = audio
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

fn extract_openai_image_url(part: &serde_json::Map<String, Value>) -> Option<String> {
    part.get("image_url")
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
            part.get("url")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
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

#[cfg(test)]
mod tests {
    use super::convert_openai_chat_response_to_claude_chat;
    use serde_json::json;

    #[test]
    fn preserves_openai_reasoning_and_cache_usage_in_claude_response() {
        let response = json!({
            "id": "chatcmpl_123",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "hello",
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
                    ]
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 7,
                "prompt_tokens_details": {
                    "cached_tokens": 3,
                    "cached_creation_tokens": 2
                }
            }
        });

        let converted = convert_openai_chat_response_to_claude_chat(&response, &json!({}))
            .expect("response should convert");

        assert_eq!(converted["content"][0]["type"], "thinking");
        assert_eq!(converted["content"][0]["thinking"], "step by step");
        assert_eq!(converted["content"][0]["signature"], "sig_123");
        assert_eq!(converted["content"][1]["type"], "redacted_thinking");
        assert_eq!(converted["usage"]["cache_read_input_tokens"], 3);
        assert_eq!(converted["usage"]["cache_creation_input_tokens"], 2);
    }

    #[test]
    fn converts_openai_multipart_content_into_claude_blocks() {
        let response = json!({
            "id": "chatcmpl_img_123",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "See attached." },
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
                "finish_reason": "stop"
            }]
        });

        let converted = convert_openai_chat_response_to_claude_chat(&response, &json!({}))
            .expect("response should convert");

        assert_eq!(converted["content"][0]["type"], "text");
        assert_eq!(converted["content"][1]["type"], "image");
        assert_eq!(converted["content"][1]["source"]["media_type"], "image/png");
        assert_eq!(converted["content"][2]["type"], "document");
        assert_eq!(
            converted["content"][2]["source"]["media_type"],
            "application/pdf"
        );
    }
}
