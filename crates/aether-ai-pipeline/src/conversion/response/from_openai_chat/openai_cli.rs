use serde_json::{json, Value};

use super::shared::{
    build_openai_cli_response_with_content, canonicalize_tool_arguments, OpenAiCliResponseUsage,
};

pub fn convert_openai_chat_response_to_openai_cli(
    body_json: &Value,
    report_context: &Value,
    compact: bool,
) -> Option<Value> {
    let body = body_json.as_object()?;
    let choices = body.get("choices")?.as_array()?;
    let first_choice = choices.first()?.as_object()?;
    let message = first_choice.get("message")?.as_object()?;
    let mut message_content = Vec::new();
    let mut reasoning_summaries = Vec::new();
    let message_annotations = message.get("annotations").cloned();
    match message.get("content") {
        Some(Value::String(value)) => {
            if !value.is_empty() {
                let mut item = json!({
                    "type": "output_text",
                    "text": value,
                    "annotations": []
                });
                if let Some(annotations) = message_annotations.clone() {
                    item["annotations"] = annotations;
                }
                message_content.push(item);
            }
        }
        Some(Value::Array(parts)) => {
            let text_part_count = parts
                .iter()
                .filter_map(Value::as_object)
                .filter(|part| {
                    matches!(
                        part.get("type").and_then(Value::as_str).unwrap_or_default(),
                        "text" | "output_text"
                    )
                })
                .count();
            for part in parts {
                let part = part.as_object()?;
                let part_type = part
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                if matches!(part_type.as_str(), "text" | "output_text") {
                    if let Some(piece) = part.get("text").and_then(Value::as_str) {
                        let mut item = json!({
                            "type": "output_text",
                            "text": piece,
                            "annotations": []
                        });
                        if text_part_count == 1 {
                            if let Some(annotations) = message_annotations.clone() {
                                item["annotations"] = annotations;
                            }
                        }
                        message_content.push(item);
                    }
                } else if matches!(part_type.as_str(), "image_url" | "output_image") {
                    if let Some(image_url) = part
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
                        })
                    {
                        let mut image_part = json!({
                            "type": "output_image",
                            "image_url": image_url,
                        });
                        if let Some(detail) =
                            part.get("detail").and_then(Value::as_str).or_else(|| {
                                part.get("image_url")
                                    .and_then(Value::as_object)
                                    .and_then(|image| image.get("detail"))
                                    .and_then(Value::as_str)
                            })
                        {
                            image_part["detail"] = Value::String(detail.to_string());
                        }
                        message_content.push(image_part);
                    }
                } else if matches!(part_type.as_str(), "file" | "input_file") {
                    if let Some(file_part) = build_openai_cli_file_part(part) {
                        message_content.push(file_part);
                    }
                } else if part_type == "input_audio" {
                    if let Some(audio_part) = build_openai_cli_input_audio_part(part) {
                        message_content.push(audio_part);
                    }
                }
            }
        }
        Some(Value::Null) | None => {}
        _ => return None,
    }
    if let Some(refusal) = message.get("refusal").and_then(Value::as_str) {
        if !refusal.trim().is_empty() {
            message_content.push(json!({
                "type": "refusal",
                "refusal": refusal,
            }));
        }
    }
    if let Some(reasoning_content) = message.get("reasoning_content").and_then(Value::as_str) {
        if !reasoning_content.trim().is_empty() {
            reasoning_summaries.push(reasoning_content.to_string());
        }
    }

    let mut function_calls = Vec::new();
    if let Some(tool_call_values) = message.get("tool_calls").and_then(Value::as_array) {
        for tool_call in tool_call_values {
            let tool_call = tool_call.as_object()?;
            let function = tool_call.get("function")?.as_object()?;
            let tool_name = function
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            function_calls.push(json!({
                "type": "function_call",
                "id": tool_call.get("id").cloned().unwrap_or(Value::Null),
                "call_id": tool_call.get("id").cloned().unwrap_or(Value::Null),
                "name": tool_name,
                "arguments": canonicalize_tool_arguments(function.get("arguments").cloned()),
            }));
        }
    }

    let usage = body.get("usage").and_then(Value::as_object);
    let prompt_tokens = usage
        .and_then(|value| value.get("prompt_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|value| value.get("completion_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .and_then(|value| value.get("total_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(prompt_tokens + output_tokens);
    let response_id = if compact {
        body.get("id")
            .and_then(Value::as_str)
            .map(|value| value.replace("chatcmpl", "resp"))
            .unwrap_or_else(|| "resp-local-finalize".to_string())
    } else {
        body.get("id")
            .and_then(Value::as_str)
            .map(|value| value.replace("chatcmpl", "resp"))
            .unwrap_or_else(|| "resp-local-finalize".to_string())
    };
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("mapped_model").and_then(Value::as_str))
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or("unknown");

    let mut response = build_openai_cli_response_with_content(
        &response_id,
        model,
        message_content,
        reasoning_summaries,
        function_calls,
        OpenAiCliResponseUsage {
            prompt_tokens,
            output_tokens,
            total_tokens,
        },
    );

    if let Some(created) = body.get("created").and_then(Value::as_i64).or_else(|| {
        body.get("created")
            .and_then(Value::as_u64)
            .map(|value| value as i64)
    }) {
        response["created_at"] = Value::from(created);
    }
    if let Some(service_tier) = body.get("service_tier").cloned().or_else(|| {
        report_context
            .get("original_request_body")
            .and_then(Value::as_object)
            .and_then(|request| request.get("service_tier"))
            .cloned()
    }) {
        response["service_tier"] = service_tier;
    }
    if let Some(request_object) = report_context
        .get("original_request_body")
        .and_then(Value::as_object)
    {
        for key in [
            "instructions",
            "max_output_tokens",
            "parallel_tool_calls",
            "previous_response_id",
            "reasoning",
            "store",
            "temperature",
            "text",
            "tool_choice",
            "tools",
            "top_p",
            "truncation",
            "user",
            "metadata",
        ] {
            if let Some(value) = request_object.get(key) {
                response[key] = value.clone();
            }
        }
    }
    if let Some(prompt_details) = usage
        .and_then(|value| value.get("prompt_tokens_details"))
        .cloned()
    {
        response["usage"]["input_tokens_details"] = prompt_details;
    }
    if let Some(completion_details) = usage
        .and_then(|value| value.get("completion_tokens_details"))
        .cloned()
    {
        response["usage"]["output_tokens_details"] = completion_details;
    }

    Some(response)
}

fn build_openai_cli_file_part(part: &serde_json::Map<String, Value>) -> Option<Value> {
    let file_object = part.get("file").and_then(Value::as_object).unwrap_or(part);
    let mut file = serde_json::Map::new();
    for key in ["file_data", "file_id", "filename"] {
        if let Some(value) = file_object
            .get(key)
            .cloned()
            .filter(|value| !value.is_null())
        {
            file.insert(key.to_string(), value);
        }
    }
    if file.is_empty() {
        return None;
    }
    Some(json!({
        "type": "file",
        "file": Value::Object(file),
    }))
}

fn build_openai_cli_input_audio_part(part: &serde_json::Map<String, Value>) -> Option<Value> {
    let audio_object = part
        .get("input_audio")
        .and_then(Value::as_object)
        .unwrap_or(part);
    let data = audio_object
        .get("data")
        .cloned()
        .filter(|value| value.as_str().is_some_and(|value| !value.trim().is_empty()))?;
    let format = audio_object
        .get("format")
        .cloned()
        .filter(|value| value.as_str().is_some_and(|value| !value.trim().is_empty()))?;
    Some(json!({
        "type": "input_audio",
        "input_audio": {
            "data": data,
            "format": format,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::convert_openai_chat_response_to_openai_cli;
    use serde_json::json;

    #[test]
    fn preserves_created_refusal_request_echo_and_usage_details_when_converting_to_responses() {
        let response = json!({
            "id": "chatcmpl_123",
            "object": "chat.completion",
            "created": 1741569952i64,
            "model": "gpt-5",
            "service_tier": "default",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello",
                    "refusal": "partial refusal",
                    "annotations": [{"type": "url_citation", "start_index": 0, "end_index": 5}]
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 19,
                "completion_tokens": 10,
                "total_tokens": 29,
                "prompt_tokens_details": {"cached_tokens": 0},
                "completion_tokens_details": {"reasoning_tokens": 0}
            }
        });
        let report_context = json!({
            "original_request_body": {
                "instructions": "Be concise.",
                "max_output_tokens": 32,
                "parallel_tool_calls": true,
                "reasoning": {"effort": "medium"},
                "store": true,
                "temperature": 1.0,
                "text": {"format": {"type": "text"}},
                "tool_choice": "auto",
                "tools": [],
                "top_p": 1.0,
                "truncation": "disabled",
                "user": null,
                "metadata": {}
            }
        });

        let converted =
            convert_openai_chat_response_to_openai_cli(&response, &report_context, false)
                .expect("chat response should convert to responses");

        assert_eq!(converted["created_at"], 1741569952i64);
        assert_eq!(converted["service_tier"], "default");
        assert_eq!(converted["instructions"], "Be concise.");
        assert_eq!(converted["max_output_tokens"], 32);
        assert_eq!(converted["parallel_tool_calls"], true);
        assert_eq!(converted["text"], json!({"format": {"type": "text"}}));
        assert_eq!(converted["top_p"], 1.0);
        assert_eq!(
            converted["output"][0]["content"],
            json!([
                {
                    "type": "output_text",
                    "text": "Hello",
                    "annotations": [{"type": "url_citation", "start_index": 0, "end_index": 5}]
                },
                {
                    "type": "refusal",
                    "refusal": "partial refusal"
                }
            ])
        );
        assert_eq!(
            converted["usage"]["input_tokens_details"],
            json!({"cached_tokens": 0})
        );
        assert_eq!(
            converted["usage"]["output_tokens_details"],
            json!({"reasoning_tokens": 0})
        );
    }

    #[test]
    fn preserves_file_and_audio_parts_when_converting_to_responses() {
        let response = json!({
            "id": "chatcmpl_mm_123",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "Attached." },
                        {
                            "type": "file",
                            "file": {
                                "file_data": "data:application/pdf;base64,JVBERi0x",
                                "filename": "report.pdf"
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
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 4,
                "completion_tokens": 2,
                "total_tokens": 6
            }
        });

        let converted = convert_openai_chat_response_to_openai_cli(&response, &json!({}), false)
            .expect("chat response should convert to responses");

        assert_eq!(
            converted["output"][0]["content"],
            json!([
                {
                    "type": "output_text",
                    "text": "Attached.",
                    "annotations": []
                },
                {
                    "type": "file",
                    "file": {
                        "file_data": "data:application/pdf;base64,JVBERi0x",
                        "filename": "report.pdf"
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
    }
}
