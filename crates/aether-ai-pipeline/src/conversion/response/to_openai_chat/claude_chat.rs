use serde_json::{json, Map, Value};

use super::shared::{build_generated_tool_call_id, canonicalize_tool_arguments};

pub fn convert_claude_chat_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let body = body_json.as_object()?;
    let content = body.get("content")?.as_array()?;
    let mut text = String::new();
    let mut content_parts = Vec::new();
    let mut reasoning_content = String::new();
    let mut reasoning_parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut has_non_text_content = false;
    for (index, block) in content.iter().enumerate() {
        let block = block.as_object()?;
        match block.get("type")?.as_str()? {
            "text" => {
                let piece = block.get("text")?.as_str()?;
                push_openai_text_part(&mut text, &mut content_parts, piece);
            }
            "thinking" => {
                let piece = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .or_else(|| block.get("text").and_then(Value::as_str))
                    .unwrap_or_default();
                if !piece.is_empty() {
                    reasoning_content.push_str(piece);
                }
                let mut reasoning_part = Map::new();
                reasoning_part.insert("type".to_string(), Value::String("thinking".to_string()));
                reasoning_part.insert("thinking".to_string(), Value::String(piece.to_string()));
                if let Some(signature) = block
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    reasoning_part.insert(
                        "signature".to_string(),
                        Value::String(signature.to_string()),
                    );
                }
                reasoning_parts.push(Value::Object(reasoning_part));
            }
            "redacted_thinking" => {
                let data = block
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !data.is_empty() {
                    reasoning_parts.push(json!({
                        "type": "redacted_thinking",
                        "data": data,
                    }));
                }
            }
            "tool_use" => {
                let tool_name = block.get("name")?.as_str()?;
                let tool_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| build_generated_tool_call_id(index));
                let arguments = canonicalize_tool_arguments(block.get("input").cloned());
                tool_calls.push(json!({
                    "id": tool_id,
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": arguments,
                    }
                }));
            }
            "image" => {
                content_parts.push(convert_claude_image_block_to_openai_part(block)?);
                has_non_text_content = true;
            }
            "document" => {
                let part = convert_claude_document_block_to_openai_part(block)?;
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(piece) = part.get("text").and_then(Value::as_str) {
                        push_openai_text_part(&mut text, &mut content_parts, piece);
                    }
                } else {
                    content_parts.push(part);
                    has_non_text_content = true;
                }
            }
            _ => continue,
        }
    }
    let mut finish_reason = match body.get("stop_reason").and_then(Value::as_str) {
        Some("end_turn") | Some("stop_sequence") => Some("stop"),
        Some("max_tokens") => Some("length"),
        Some("tool_use") => Some("tool_calls"),
        Some(other) if !other.is_empty() => Some(other),
        _ => None,
    };
    if !tool_calls.is_empty() && finish_reason.is_none_or(|reason| reason == "stop") {
        finish_reason = Some("tool_calls");
    }
    let usage = body.get("usage").and_then(Value::as_object);
    let prompt_tokens = usage
        .and_then(|value| value.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion_tokens = usage
        .and_then(|value| value.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = prompt_tokens + completion_tokens;
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("mapped_model").and_then(Value::as_str))
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or("unknown");
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-local-finalize");
    let message_content = if content_parts.is_empty() && !tool_calls.is_empty() {
        Value::Null
    } else if has_non_text_content {
        Value::Array(content_parts)
    } else {
        Value::String(text)
    };
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), message_content);
    if !reasoning_content.trim().is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_content),
        );
    }
    if !reasoning_parts.is_empty() {
        message.insert("reasoning_parts".to_string(), Value::Array(reasoning_parts));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    Some(json!({
        "id": id,
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": total_tokens,
        }
    }))
    .map(|mut response| {
        if let Some(service_tier) = report_context
            .get("original_request_body")
            .and_then(Value::as_object)
            .and_then(|request| request.get("service_tier"))
            .cloned()
        {
            response["service_tier"] = service_tier;
        }
        let mut prompt_details = Map::new();
        if let Some(cached_tokens) = usage
            .and_then(|value| value.get("cache_read_input_tokens"))
            .and_then(Value::as_u64)
        {
            prompt_details.insert("cached_tokens".to_string(), Value::from(cached_tokens));
        }
        if let Some(cached_creation_tokens) = usage
            .and_then(|value| value.get("cache_creation_input_tokens"))
            .and_then(Value::as_u64)
        {
            prompt_details.insert(
                "cached_creation_tokens".to_string(),
                Value::from(cached_creation_tokens),
            );
        }
        if !prompt_details.is_empty() {
            response["usage"]["prompt_tokens_details"] = Value::Object(prompt_details);
        }
        response
    })
}

fn push_openai_text_part(text: &mut String, content_parts: &mut Vec<Value>, piece: &str) {
    if piece.is_empty() {
        return;
    }
    text.push_str(piece);
    content_parts.push(json!({
        "type": "text",
        "text": piece,
    }));
}

fn convert_claude_image_block_to_openai_part(
    block: &serde_json::Map<String, Value>,
) -> Option<Value> {
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
            Some(json!({
                "type": "image_url",
                "image_url": {
                    "url": build_data_url(media_type, data),
                }
            }))
        }
        "url" => {
            let url = source
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(json!({
                "type": "image_url",
                "image_url": {
                    "url": url,
                }
            }))
        }
        _ => None,
    }
}

fn convert_claude_document_block_to_openai_part(
    block: &serde_json::Map<String, Value>,
) -> Option<Value> {
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
            if let Some(format) = media_type.strip_prefix("audio/") {
                return Some(json!({
                    "type": "input_audio",
                    "input_audio": {
                        "data": data,
                        "format": format,
                    }
                }));
            }
            Some(json!({
                "type": "file",
                "file": {
                    "file_data": build_data_url(media_type, data),
                }
            }))
        }
        "url" => {
            let url = source
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            Some(json!({
                "type": "text",
                "text": format!("[File: {url}]"),
            }))
        }
        _ => None,
    }
}

fn build_data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

#[cfg(test)]
mod tests {
    use super::convert_claude_chat_response_to_openai_chat;
    use serde_json::json;

    #[test]
    fn preserves_claude_reasoning_cache_usage_and_service_tier() {
        let response = json!({
            "id": "msg_123",
            "model": "claude-sonnet-4-5",
            "content": [
                { "type": "thinking", "thinking": "step by step", "signature": "sig_123" },
                { "type": "text", "text": "hello" }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "cache_read_input_tokens": 3,
                "cache_creation_input_tokens": 2
            }
        });
        let report_context = json!({
            "original_request_body": {
                "service_tier": "default"
            }
        });

        let converted = convert_claude_chat_response_to_openai_chat(&response, &report_context)
            .expect("response should convert");

        assert_eq!(
            converted["choices"][0]["message"]["reasoning_content"],
            "step by step"
        );
        assert_eq!(
            converted["choices"][0]["message"]["reasoning_parts"],
            json!([
                {
                    "type": "thinking",
                    "thinking": "step by step",
                    "signature": "sig_123"
                }
            ])
        );
        assert_eq!(
            converted["usage"]["prompt_tokens_details"]["cached_tokens"],
            3
        );
        assert_eq!(
            converted["usage"]["prompt_tokens_details"]["cached_creation_tokens"],
            2
        );
        assert_eq!(converted["service_tier"], "default");
    }

    #[test]
    fn converts_claude_multimodal_content_into_openai_chat_parts() {
        let response = json!({
            "id": "msg_multimodal_123",
            "model": "claude-sonnet-4-5",
            "content": [
                { "type": "thinking", "thinking": "step by step" },
                { "type": "text", "text": "See attached." },
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
                },
                {
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "audio/mp3",
                        "data": "SUQz"
                    }
                },
                {
                    "type": "tool_use",
                    "id": "call_1",
                    "name": "lookup",
                    "input": { "city": "Shanghai" }
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 9,
                "output_tokens": 4
            }
        });

        let converted = convert_claude_chat_response_to_openai_chat(&response, &json!({}))
            .expect("response should convert");

        assert_eq!(
            converted["choices"][0]["message"]["reasoning_content"],
            "step by step"
        );
        assert_eq!(
            converted["choices"][0]["message"]["content"],
            json!([
                { "type": "text", "text": "See attached." },
                {
                    "type": "image_url",
                    "image_url": {
                        "url": "https://example.com/cat.png"
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
            ])
        );
        assert_eq!(converted["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            converted["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"city\":\"Shanghai\"}"
        );
    }
}
