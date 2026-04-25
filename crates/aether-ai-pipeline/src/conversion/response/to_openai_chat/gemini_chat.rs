use serde_json::{json, Map, Value};

use super::shared::{
    build_generated_tool_call_id, canonicalize_tool_arguments, extract_gemini_image_url,
};

pub fn convert_gemini_chat_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let body = body_json.as_object()?;
    let candidates = body.get("candidates")?.as_array()?;
    let mut choices = Vec::new();
    for candidate in candidates {
        let candidate = candidate.as_object()?;
        let content = candidate.get("content")?.as_object()?;
        let parts = content.get("parts")?.as_array()?;
        let mut text = String::new();
        let mut content_parts = Vec::new();
        let mut reasoning_content = String::new();
        let mut reasoning_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut has_non_text_content = false;
        for (index, part) in parts.iter().enumerate() {
            let part = part.as_object()?;
            if let Some(piece) = part.get("text").and_then(Value::as_str) {
                if part
                    .get("thought")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    reasoning_content.push_str(piece);
                    let mut reasoning_part = Map::new();
                    reasoning_part
                        .insert("type".to_string(), Value::String("thinking".to_string()));
                    reasoning_part.insert("thinking".to_string(), Value::String(piece.to_string()));
                    if let Some(signature) = part
                        .get("thoughtSignature")
                        .or_else(|| part.get("thought_signature"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                    {
                        reasoning_part.insert(
                            "signature".to_string(),
                            Value::String(signature.to_string()),
                        );
                    }
                    reasoning_parts.push(Value::Object(reasoning_part));
                } else {
                    text.push_str(piece);
                    content_parts.push(json!({
                        "type": "text",
                        "text": piece,
                    }));
                }
            } else if let Some(function_call) = part.get("functionCall").and_then(Value::as_object)
            {
                let tool_name = function_call.get("name")?.as_str()?;
                let tool_id = function_call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| build_generated_tool_call_id(index));
                let arguments = canonicalize_tool_arguments(function_call.get("args").cloned());
                tool_calls.push(json!({
                    "id": tool_id,
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": arguments,
                    }
                }));
            } else if let Some(rendered_text) = render_gemini_textual_part(part) {
                text.push_str(rendered_text.as_str());
                content_parts.push(json!({
                    "type": "text",
                    "text": rendered_text,
                }));
            } else if let Some(content_part) = convert_gemini_part_to_openai_content_part(part) {
                if content_part.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(piece) = content_part.get("text").and_then(Value::as_str) {
                        text.push_str(piece);
                    }
                } else {
                    has_non_text_content = true;
                }
                content_parts.push(content_part);
            } else {
                continue;
            }
        }
        let mut finish_reason = match candidate.get("finishReason").and_then(Value::as_str) {
            Some("STOP") => Some("stop"),
            Some("MAX_TOKENS") => Some("length"),
            Some(
                "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "OTHER",
            ) => Some("content_filter"),
            Some(other) if !other.is_empty() => Some(other),
            _ => None,
        };
        if !tool_calls.is_empty() && finish_reason.is_none_or(|reason| reason == "stop") {
            finish_reason = Some("tool_calls");
        }
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
        choices.push(json!({
            "index": candidate.get("index").and_then(Value::as_u64).unwrap_or(0),
            "message": Value::Object(message),
            "finish_reason": finish_reason,
        }));
    }
    let usage = body.get("usageMetadata").and_then(Value::as_object);
    let prompt_tokens = usage
        .and_then(|value| value.get("promptTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning_tokens = usage
        .and_then(|value| value.get("thoughtsTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion_tokens = usage
        .and_then(|value| value.get("candidatesTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + reasoning_tokens;
    let total_tokens = usage
        .and_then(|value| value.get("totalTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(prompt_tokens + completion_tokens);
    let model = body
        .get("modelVersion")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("mapped_model").and_then(Value::as_str))
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or("unknown");
    let id = body
        .get("responseId")
        .or_else(|| body.get("_v1internal_response_id"))
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-local-finalize");
    Some(json!({
        "id": id,
        "object": "chat.completion",
        "model": model,
        "choices": choices,
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": total_tokens,
        }
    }))
    .map(|mut response| {
        if reasoning_tokens > 0 {
            response["usage"]["completion_tokens_details"] =
                json!({ "reasoning_tokens": reasoning_tokens });
        }
        response
    })
}

fn render_gemini_textual_part(part: &Map<String, Value>) -> Option<String> {
    if let Some(code) = part.get("executableCode").and_then(Value::as_object) {
        let language = code
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let source = code.get("code").and_then(Value::as_str).unwrap_or_default();
        return Some(format!("```{language}\n{source}\n```"));
    }
    if let Some(result) = part.get("codeExecutionResult").and_then(Value::as_object) {
        let output = result
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Some(format!("```output\n{output}\n```"));
    }
    None
}

fn convert_gemini_part_to_openai_content_part(part: &Map<String, Value>) -> Option<Value> {
    if let Some(image_url) = extract_gemini_image_url(part) {
        return Some(json!({
            "type": "image_url",
            "image_url": {
                "url": image_url,
            }
        }));
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
        if let Some(format) = mime_type.strip_prefix("audio/") {
            return Some(json!({
                "type": "input_audio",
                "input_audio": {
                    "data": data,
                    "format": format,
                }
            }));
        }
        return Some(json!({
            "type": "file",
            "file": {
                "file_data": format!("data:{mime_type};base64,{data}"),
            }
        }));
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
        return Some(json!({
            "type": "text",
            "text": format!("[File: {file_uri}]"),
        }));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::convert_gemini_chat_response_to_openai_chat;
    use serde_json::json;

    #[test]
    fn preserves_gemini_candidates_reasoning_and_code_execution() {
        let response = json!({
            "responseId": "resp_123",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [
                {
                    "index": 0,
                    "finishReason": "RECITATION",
                    "content": {
                        "parts": [
                            { "text": "thinking", "thought": true, "thoughtSignature": "sig_123" },
                            { "executableCode": { "language": "python", "code": "print(1)" } },
                            { "codeExecutionResult": { "output": "1" } }
                        ]
                    }
                },
                {
                    "index": 1,
                    "finishReason": "STOP",
                    "content": {
                        "parts": [
                            { "functionCall": { "id": "call_1", "name": "lookup", "args": { "city": "Shanghai" } } }
                        ]
                    }
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 17
            }
        });

        let converted = convert_gemini_chat_response_to_openai_chat(&response, &json!({}))
            .expect("response should convert");

        assert_eq!(converted["choices"].as_array().expect("choices").len(), 2);
        assert_eq!(converted["choices"][0]["finish_reason"], "content_filter");
        assert_eq!(
            converted["choices"][0]["message"]["reasoning_content"],
            "thinking"
        );
        assert_eq!(
            converted["choices"][0]["message"]["reasoning_parts"],
            json!([
                {
                    "type": "thinking",
                    "thinking": "thinking",
                    "signature": "sig_123"
                }
            ])
        );
        let content = converted["choices"][0]["message"]["content"]
            .as_str()
            .expect("content should be string");
        assert!(content.contains("print(1)"));
        assert!(content.contains("```output\n1\n```"));
        assert_eq!(converted["choices"][1]["finish_reason"], "tool_calls");
        assert_eq!(
            converted["usage"]["completion_tokens_details"]["reasoning_tokens"],
            2
        );
        assert_eq!(converted["usage"]["completion_tokens"], 7);
    }

    #[test]
    fn preserves_gemini_multimodal_parts_in_openai_chat_response() {
        let response = json!({
            "responseId": "resp_mm_123",
            "modelVersion": "gemini-2.5-pro",
            "candidates": [{
                "index": 0,
                "finishReason": "STOP",
                "content": {
                    "parts": [
                        { "text": "Attached." },
                        { "inlineData": { "mimeType": "image/png", "data": "iVBORw0KGgo=" } },
                        { "inlineData": { "mimeType": "application/pdf", "data": "JVBERi0x" } },
                        { "inline_data": { "mime_type": "audio/mp3", "data": "SUQz" } },
                        { "fileData": { "fileUri": "https://example.com/report.pdf", "mimeType": "application/pdf" } }
                    ]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 4,
                "candidatesTokenCount": 2,
                "totalTokenCount": 6
            }
        });

        let converted = convert_gemini_chat_response_to_openai_chat(&response, &json!({}))
            .expect("response should convert");

        assert_eq!(
            converted["choices"][0]["message"]["content"],
            json!([
                { "type": "text", "text": "Attached." },
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
                },
                { "type": "text", "text": "[File: https://example.com/report.pdf]" }
            ])
        );
    }
}
