use serde_json::{json, Value};

use super::shared::{build_generated_tool_call_id, parse_openai_function_arguments};

pub fn convert_openai_chat_response_to_gemini_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let body = body_json.as_object()?;
    let choices = body.get("choices")?.as_array()?;
    let mut candidates = Vec::new();
    for choice in choices {
        let choice = choice.as_object()?;
        let message = choice.get("message")?.as_object()?;
        let mut parts = extract_openai_reasoning_to_gemini_parts(message);
        parts.extend(convert_openai_assistant_content_to_gemini_parts(
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
                let call_id = tool_call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| build_generated_tool_call_id(index));
                parts.push(json!({
                    "functionCall": {
                        "id": call_id,
                        "name": tool_name,
                        "args": parse_openai_function_arguments(function.get("arguments"))?,
                    }
                }));
            }
        }
        if parts.is_empty() {
            parts.push(json!({ "text": "" }));
        }

        let mut finish_reason = match choice.get("finish_reason").and_then(Value::as_str) {
            Some("stop") | None => "STOP",
            Some("length") => "MAX_TOKENS",
            Some("content_filter") => "SAFETY",
            Some("tool_calls") | Some("function_call") => "STOP",
            Some(other) => other,
        };
        if parts.iter().any(|part| part.get("functionCall").is_some()) {
            finish_reason = "STOP";
        }
        candidates.push(json!({
            "content": {
                "role": "model",
                "parts": parts,
            },
            "finishReason": finish_reason,
            "index": choice.get("index").and_then(Value::as_u64).unwrap_or(0),
        }));
    }
    let usage = body.get("usage").and_then(Value::as_object);
    let prompt_tokens = usage
        .and_then(|value| value.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion_tokens = usage
        .and_then(|value| value.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .and_then(|value| value.get("completion_tokens_details"))
        .and_then(Value::as_object)
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let visible_completion_tokens = completion_tokens.saturating_sub(reasoning_tokens);
    let total_tokens = usage
        .and_then(|value| value.get("total_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(prompt_tokens + completion_tokens);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("mapped_model").and_then(Value::as_str))
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or("unknown");
    let response_id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("resp-local-finalize");

    Some(json!({
        "responseId": response_id,
        "modelVersion": model,
        "candidates": candidates,
        "usageMetadata": {
            "promptTokenCount": prompt_tokens,
            "candidatesTokenCount": visible_completion_tokens,
            "thoughtsTokenCount": reasoning_tokens,
            "totalTokenCount": total_tokens,
        }
    }))
}

fn extract_openai_reasoning_to_gemini_parts(
    message: &serde_json::Map<String, Value>,
) -> Vec<Value> {
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
            let mut part = serde_json::Map::new();
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

fn convert_openai_assistant_content_to_gemini_parts(content: Option<&Value>) -> Option<Vec<Value>> {
    match content {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::String(text)) => {
            if text.trim().is_empty() {
                Some(Vec::new())
            } else {
                Some(vec![json!({ "text": text })])
            }
        }
        Some(Value::Array(parts)) => {
            let mut converted = Vec::new();
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
                                converted.push(json!({ "text": text }));
                            }
                        }
                    }
                    "image_url" | "output_image" => {
                        let image_url = part
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
                                part.get("url")
                                    .and_then(Value::as_str)
                                    .map(ToOwned::to_owned)
                            })?;
                        if let Some((mime_type, data)) = parse_data_url(image_url.as_str()) {
                            converted.push(json!({
                                "inlineData": {
                                    "mimeType": mime_type,
                                    "data": data,
                                }
                            }));
                        } else {
                            converted.push(json!({
                                "fileData": {
                                    "fileUri": image_url,
                                    "mimeType": guess_media_type_from_reference(image_url.as_str(), "image/jpeg"),
                                }
                            }));
                        }
                    }
                    "file" | "input_file" => {
                        let file_object =
                            part.get("file").and_then(Value::as_object).unwrap_or(part);
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
                        let audio_object = part
                            .get("input_audio")
                            .and_then(Value::as_object)
                            .unwrap_or(part);
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

#[cfg(test)]
mod tests {
    use super::convert_openai_chat_response_to_gemini_chat;
    use serde_json::json;

    #[test]
    fn preserves_multiple_openai_choices_and_reasoning_tokens_for_gemini() {
        let response = json!({
            "id": "chatcmpl_123",
            "model": "gpt-5.4",
            "choices": [
                {
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
                            }
                        ]
                    },
                    "finish_reason": "stop"
                },
                {
                    "index": 1,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "lookup",
                                "arguments": "{\"city\":\"Shanghai\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 7,
                "total_tokens": 17,
                "completion_tokens_details": {
                    "reasoning_tokens": 2
                }
            }
        });

        let converted = convert_openai_chat_response_to_gemini_chat(&response, &json!({}))
            .expect("response should convert");

        assert_eq!(
            converted["candidates"]
                .as_array()
                .expect("candidates")
                .len(),
            2
        );
        assert_eq!(
            converted["candidates"][0]["content"]["parts"][0]["thought"],
            true
        );
        assert_eq!(
            converted["candidates"][0]["content"]["parts"][0]["thoughtSignature"],
            "sig_123"
        );
        assert_eq!(
            converted["candidates"][1]["content"]["parts"][0]["functionCall"]["name"],
            "lookup"
        );
        assert_eq!(converted["usageMetadata"]["candidatesTokenCount"], 5);
        assert_eq!(converted["usageMetadata"]["thoughtsTokenCount"], 2);
    }

    #[test]
    fn preserves_multimodal_openai_content_in_gemini_response() {
        let response = json!({
            "id": "chatcmpl_mm_123",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "reasoning_content": "step by step",
                    "content": [
                        { "type": "text", "text": "Attached." },
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
                        },
                        {
                            "type": "file",
                            "file": { "file_id": "file_123" }
                        }
                    ]
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 4,
                "completion_tokens": 3,
                "completion_tokens_details": { "reasoning_tokens": 1 },
                "total_tokens": 7
            }
        });

        let converted = convert_openai_chat_response_to_gemini_chat(&response, &json!({}))
            .expect("response should convert");

        assert_eq!(
            converted["candidates"][0]["content"]["parts"][0]["thought"],
            true
        );
        assert_eq!(
            converted["candidates"][0]["content"]["parts"][1],
            json!({ "text": "Attached." })
        );
        assert_eq!(
            converted["candidates"][0]["content"]["parts"][2],
            json!({
                "inlineData": {
                    "mimeType": "image/png",
                    "data": "iVBORw0KGgo="
                }
            })
        );
        assert_eq!(
            converted["candidates"][0]["content"]["parts"][3],
            json!({
                "inlineData": {
                    "mimeType": "application/pdf",
                    "data": "JVBERi0x"
                }
            })
        );
        assert_eq!(
            converted["candidates"][0]["content"]["parts"][4],
            json!({
                "inlineData": {
                    "mimeType": "audio/mp3",
                    "data": "SUQz"
                }
            })
        );
        assert_eq!(
            converted["candidates"][0]["content"]["parts"][5],
            json!({ "text": "[File: file_123]" })
        );
        assert_eq!(converted["usageMetadata"]["candidatesTokenCount"], 2);
        assert_eq!(converted["usageMetadata"]["thoughtsTokenCount"], 1);
    }
}
