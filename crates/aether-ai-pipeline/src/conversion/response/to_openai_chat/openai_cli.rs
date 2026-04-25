use serde_json::{json, Map, Value};

use super::shared::{build_generated_tool_call_id, canonicalize_tool_arguments};

pub fn convert_openai_cli_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let body = body_json.as_object()?;
    let mut text = String::new();
    let mut content_parts = Vec::new();
    let mut reasoning_content = String::new();
    let mut tool_calls = Vec::new();
    let mut annotations = Vec::new();
    let mut refusal = Vec::new();
    let mut has_non_text_content = false;

    if let Some(output_items) = body.get("output").and_then(Value::as_array) {
        for (index, item) in output_items.iter().enumerate() {
            let item_object = item.as_object()?;
            let item_type = item_object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            match item_type.as_str() {
                "message" => {
                    if let Some(content) = item_object.get("content").and_then(Value::as_array) {
                        for part in content {
                            let part_object = part.as_object()?;
                            let part_type = part_object
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .trim()
                                .to_ascii_lowercase();
                            if matches!(part_type.as_str(), "output_text" | "text") {
                                if let Some(piece) = part_object.get("text").and_then(Value::as_str)
                                {
                                    let annotation_offset = text.chars().count() as i64;
                                    if let Some(raw_annotations) =
                                        part_object.get("annotations").and_then(Value::as_array)
                                    {
                                        annotations.extend(raw_annotations.iter().map(
                                            |annotation| {
                                                offset_annotation_indices(
                                                    annotation,
                                                    annotation_offset,
                                                )
                                            },
                                        ));
                                    }
                                    text.push_str(piece);
                                    content_parts.push(json!({
                                        "type": "text",
                                        "text": piece,
                                    }));
                                }
                            } else if part_type == "refusal" {
                                if let Some(piece) =
                                    part_object.get("refusal").and_then(Value::as_str)
                                {
                                    if !piece.trim().is_empty() {
                                        refusal.push(piece.to_string());
                                    }
                                }
                            } else if matches!(part_type.as_str(), "output_image" | "image_url") {
                                if let Some((image_url, detail)) =
                                    extract_openai_response_image(part_object)
                                {
                                    let mut image = Map::new();
                                    image.insert("url".to_string(), Value::String(image_url));
                                    if let Some(detail) = detail {
                                        image.insert("detail".to_string(), Value::String(detail));
                                    }
                                    content_parts.push(json!({
                                        "type": "image_url",
                                        "image_url": image,
                                    }));
                                    has_non_text_content = true;
                                }
                            } else if part_type == "file" {
                                if let Some(file_part) = extract_openai_response_file(part_object) {
                                    content_parts.push(file_part);
                                    has_non_text_content = true;
                                }
                            } else if part_type == "input_audio" {
                                if let Some(audio_part) =
                                    extract_openai_response_input_audio(part_object)
                                {
                                    content_parts.push(audio_part);
                                    has_non_text_content = true;
                                }
                            }
                        }
                    }
                }
                "reasoning" => {
                    if let Some(summary_items) =
                        item_object.get("summary").and_then(Value::as_array)
                    {
                        for summary in summary_items {
                            let summary_object = summary.as_object()?;
                            if summary_object
                                .get("type")
                                .and_then(Value::as_str)
                                .is_some_and(|value| value == "summary_text")
                            {
                                if let Some(piece) =
                                    summary_object.get("text").and_then(Value::as_str)
                                {
                                    reasoning_content.push_str(piece);
                                }
                            }
                        }
                    }
                }
                "function_call" => {
                    let tool_name = item_object
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    let tool_id = item_object
                        .get("call_id")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .or_else(|| {
                            item_object
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|value| !value.is_empty())
                        })
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| build_generated_tool_call_id(index));
                    tool_calls.push(json!({
                        "id": tool_id,
                        "type": "function",
                        "function": {
                            "name": tool_name,
                            "arguments": canonicalize_tool_arguments(item_object.get("arguments").cloned()),
                        }
                    }));
                }
                "output_text" | "text" => {
                    if let Some(piece) = item_object.get("text").and_then(Value::as_str) {
                        text.push_str(piece);
                        content_parts.push(json!({
                            "type": "text",
                            "text": piece,
                        }));
                    }
                }
                "output_image" | "image_url" => {
                    if let Some((image_url, detail)) = extract_openai_response_image(item_object) {
                        let mut image = Map::new();
                        image.insert("url".to_string(), Value::String(image_url));
                        if let Some(detail) = detail {
                            image.insert("detail".to_string(), Value::String(detail));
                        }
                        content_parts.push(json!({
                            "type": "image_url",
                            "image_url": image,
                        }));
                        has_non_text_content = true;
                    }
                }
                _ => {}
            }
        }
    }

    let finish_reason = if tool_calls.is_empty() {
        Some("stop")
    } else {
        Some("tool_calls")
    };
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("mapped_model").and_then(Value::as_str))
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or("unknown");
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-local-openai-cli");
    let created = body.get("created_at").and_then(Value::as_i64).or_else(|| {
        body.get("created_at")
            .and_then(Value::as_u64)
            .map(|value| value as i64)
    });
    let service_tier = body.get("service_tier").cloned().or_else(|| {
        report_context
            .get("original_request_body")
            .and_then(Value::as_object)
            .and_then(|request| request.get("service_tier"))
            .cloned()
    });

    let usage = body.get("usage").and_then(Value::as_object);
    let prompt_tokens = usage
        .and_then(|value| value.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let completion_tokens = usage
        .and_then(|value| value.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = usage
        .and_then(|value| value.get("total_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(prompt_tokens + completion_tokens);

    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    if content_parts.is_empty() && !tool_calls.is_empty() {
        message.insert("content".to_string(), Value::Null);
    } else if has_non_text_content {
        message.insert("content".to_string(), Value::Array(content_parts));
    } else {
        message.insert("content".to_string(), Value::String(text));
    }
    if !reasoning_content.trim().is_empty() {
        message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning_content),
        );
    }
    if !refusal.is_empty() {
        message.insert("refusal".to_string(), Value::String(refusal.join("\n")));
    }
    if !annotations.is_empty() {
        message.insert("annotations".to_string(), Value::Array(annotations));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }

    let mut response = json!({
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
    });
    if let Some(created) = created {
        response["created"] = Value::from(created);
    }
    if let Some(service_tier) = service_tier {
        response["service_tier"] = service_tier;
    }
    if let Some(input_details) = usage
        .and_then(|value| value.get("input_tokens_details"))
        .cloned()
    {
        response["usage"]["prompt_tokens_details"] = input_details;
    }
    if let Some(output_details) = usage
        .and_then(|value| value.get("output_tokens_details"))
        .cloned()
    {
        response["usage"]["completion_tokens_details"] = output_details;
    }

    Some(response)
}

fn extract_openai_response_image(
    part_object: &Map<String, Value>,
) -> Option<(String, Option<String>)> {
    let image_url = part_object
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
    let detail = part_object
        .get("detail")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            part_object
                .get("image_url")
                .and_then(Value::as_object)
                .and_then(|image| image.get("detail"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        });
    Some((image_url, detail))
}

fn extract_openai_response_file(part_object: &Map<String, Value>) -> Option<Value> {
    let file_object = part_object
        .get("file")
        .and_then(Value::as_object)
        .unwrap_or(part_object);
    let mut file = Map::new();
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

fn extract_openai_response_input_audio(part_object: &Map<String, Value>) -> Option<Value> {
    let audio_object = part_object
        .get("input_audio")
        .and_then(Value::as_object)
        .unwrap_or(part_object);
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

fn offset_annotation_indices(annotation: &Value, offset: i64) -> Value {
    let Some(object) = annotation.as_object() else {
        return annotation.clone();
    };
    let mut adjusted = object.clone();
    for key in [
        "start_index",
        "end_index",
        "start_char",
        "end_char",
        "index",
    ] {
        if let Some(value) = adjusted.get(key).and_then(Value::as_i64) {
            adjusted.insert(key.to_string(), Value::from(value + offset));
        }
    }
    Value::Object(adjusted)
}

#[cfg(test)]
mod tests {
    use super::convert_openai_cli_response_to_openai_chat;
    use serde_json::json;

    #[test]
    fn preserves_created_refusal_annotations_and_usage_details_when_converting_to_chat() {
        let response = json!({
            "id": "resp_123",
            "object": "response",
            "created_at": 1741476542i64,
            "model": "gpt-5",
            "service_tier": "flex",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_text",
                        "text": "Hello",
                        "annotations": [{"type": "file_citation", "start_index": 0, "end_index": 5}]
                    },
                    {"type": "refusal", "refusal": "partial refusal"}
                ]
            }],
            "usage": {
                "input_tokens": 10,
                "input_tokens_details": {"cached_tokens": 2},
                "output_tokens": 4,
                "output_tokens_details": {"reasoning_tokens": 1},
                "total_tokens": 14
            }
        });

        let converted = convert_openai_cli_response_to_openai_chat(&response, &json!({}))
            .expect("responses response should convert to chat");

        assert_eq!(converted["created"], 1741476542i64);
        assert_eq!(converted["service_tier"], "flex");
        assert_eq!(converted["choices"][0]["message"]["content"], "Hello");
        assert_eq!(
            converted["choices"][0]["message"]["refusal"],
            "partial refusal"
        );
        assert_eq!(
            converted["choices"][0]["message"]["annotations"],
            json!([{"type": "file_citation", "start_index": 0, "end_index": 5}])
        );
        assert_eq!(
            converted["usage"]["prompt_tokens_details"],
            json!({"cached_tokens": 2})
        );
        assert_eq!(
            converted["usage"]["completion_tokens_details"],
            json!({"reasoning_tokens": 1})
        );
    }

    #[test]
    fn preserves_file_and_audio_parts_when_converting_to_chat() {
        let response = json!({
            "id": "resp_mm_123",
            "object": "response",
            "model": "gpt-5",
            "output": [{
                "type": "message",
                "role": "assistant",
                "content": [
                    { "type": "output_text", "text": "Attached." },
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
            }],
            "usage": {
                "input_tokens": 4,
                "output_tokens": 2,
                "total_tokens": 6
            }
        });

        let converted = convert_openai_cli_response_to_openai_chat(&response, &json!({}))
            .expect("responses response should convert to chat");

        assert_eq!(
            converted["choices"][0]["message"]["content"],
            json!([
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
            ])
        );
    }
}
