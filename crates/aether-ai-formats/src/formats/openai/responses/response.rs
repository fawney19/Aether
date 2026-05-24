use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::{
    formats::context::FormatContext,
    protocol::canonical::{
        canonical_content_block_to_openai_responses_part,
        canonical_usage_to_openai_responses_usage, canonicalize_tool_arguments,
        flush_openai_responses_message_item, namespace_extension_object,
        openai_responses_extensions, openai_responses_output_to_canonical_blocks,
        openai_usage_to_canonical, CanonicalContentBlock, CanonicalResponse,
        CanonicalResponseOutput, CanonicalRole, CanonicalStopReason,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE, OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalResponse> {
    from_raw(body)
}

pub fn to(response: &CanonicalResponse, ctx: &FormatContext) -> Option<Value> {
    Some(to_raw(response, &ctx.report_context_value(), false))
}

pub fn to_compact(response: &CanonicalResponse, ctx: &FormatContext) -> Option<Value> {
    Some(to_raw(response, &ctx.report_context_value(), true))
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalResponse> {
    let body = body_json.as_object()?;
    if body.get("error").is_some_and(|error| !error.is_null())
        || body.get("status").and_then(Value::as_str) == Some("failed")
    {
        return None;
    }
    let content = openai_responses_output_to_canonical_blocks(body.get("output"))?;
    let has_tool_use = content
        .iter()
        .any(|block| matches!(block, CanonicalContentBlock::ToolUse { .. }));
    let stop_reason = if has_tool_use {
        Some(CanonicalStopReason::ToolUse)
    } else {
        match body.get("status").and_then(Value::as_str) {
            Some("incomplete") => Some(CanonicalStopReason::MaxTokens),
            Some("failed") => Some(CanonicalStopReason::Unknown),
            _ => Some(CanonicalStopReason::EndTurn),
        }
    };
    Some(CanonicalResponse {
        id: body
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("resp-unknown")
            .to_string(),
        model: body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        outputs: vec![CanonicalResponseOutput {
            index: 0,
            role: CanonicalRole::Assistant,
            content: content.clone(),
            stop_reason: stop_reason.clone(),
            extensions: BTreeMap::new(),
        }],
        content,
        stop_reason,
        usage: openai_usage_to_canonical(body.get("usage")),
        extensions: openai_responses_extensions(
            body,
            &["id", "object", "model", "output", "usage", "status"],
        ),
    })
}

pub fn to_raw(canonical: &CanonicalResponse, report_context: &Value, _compact: bool) -> Value {
    let mut response = Map::new();
    let response_id = canonical.id.replace("chatcmpl", "resp");
    response.insert("id".to_string(), Value::String(response_id.clone()));
    response.insert("object".to_string(), Value::String("response".to_string()));
    response.insert("status".to_string(), Value::String("completed".to_string()));
    response.insert("model".to_string(), Value::String(canonical.model.clone()));

    let mut output = Vec::new();
    let mut message_content = Vec::new();
    let mut message_index = 0usize;
    for block in &canonical.content {
        match block {
            CanonicalContentBlock::Text { .. }
            | CanonicalContentBlock::File { .. }
            | CanonicalContentBlock::Audio { .. } => {
                if let Some(part) = canonical_content_block_to_openai_responses_part(block) {
                    message_content.push(part);
                }
            }
            CanonicalContentBlock::Image {
                data,
                url,
                media_type,
                extensions,
                ..
            } => {
                if image_block_is_generation_call(extensions) {
                    flush_openai_responses_message_item(
                        &mut output,
                        &mut message_content,
                        &response_id,
                        &mut message_index,
                    );
                    output.push(openai_responses_image_generation_call_item(
                        &response_id,
                        output.len(),
                        data,
                        url,
                        media_type,
                    ));
                } else if let Some(part) = canonical_content_block_to_openai_responses_part(block) {
                    message_content.push(part);
                }
            }
            CanonicalContentBlock::Thinking {
                text,
                encrypted_content,
                ..
            } => {
                flush_openai_responses_message_item(
                    &mut output,
                    &mut message_content,
                    &response_id,
                    &mut message_index,
                );
                let mut item = Map::new();
                item.insert("type".to_string(), Value::String("reasoning".to_string()));
                item.insert(
                    "id".to_string(),
                    Value::String(format!("{}_rs_{}", response_id, output.len())),
                );
                item.insert("status".to_string(), Value::String("completed".to_string()));
                if let Some(encrypted_content) =
                    encrypted_content.as_ref().filter(|value| !value.is_empty())
                {
                    item.insert(
                        "encrypted_content".to_string(),
                        Value::String(encrypted_content.clone()),
                    );
                }
                if !text.trim().is_empty() {
                    item.insert(
                        "summary".to_string(),
                        Value::Array(vec![json!({
                            "type": "summary_text",
                            "text": text,
                        })]),
                    );
                }
                output.push(Value::Object(item));
            }
            CanonicalContentBlock::ToolUse {
                id, name, input, ..
            } => {
                flush_openai_responses_message_item(
                    &mut output,
                    &mut message_content,
                    &response_id,
                    &mut message_index,
                );
                if is_responses_web_search_tool(name) {
                    output.push(json!({
                        "type": "web_search_call",
                        "id": id,
                        "status": "completed",
                        "action": {
                            "type": "search",
                            "query": web_search_query_from_value(input),
                        },
                    }));
                } else {
                    output.push(json!({
                        "type": "function_call",
                        "id": id,
                        "call_id": id,
                        "name": name,
                        "arguments": canonicalize_tool_arguments(input),
                    }));
                }
            }
            CanonicalContentBlock::ToolResult {
                tool_use_id,
                output: result_output,
                content_text,
                is_error,
                ..
            } => {
                flush_openai_responses_message_item(
                    &mut output,
                    &mut message_content,
                    &response_id,
                    &mut message_index,
                );
                let mut item = Map::new();
                item.insert(
                    "type".to_string(),
                    Value::String("function_call_output".to_string()),
                );
                item.insert("call_id".to_string(), Value::String(tool_use_id.clone()));
                item.insert(
                    "output".to_string(),
                    result_output
                        .clone()
                        .unwrap_or_else(|| Value::String(content_text.clone().unwrap_or_default())),
                );
                if *is_error {
                    item.insert("is_error".to_string(), Value::Bool(true));
                }
                output.push(Value::Object(item));
            }
            CanonicalContentBlock::Unknown {
                raw_type, payload, ..
            } if raw_type == "refusal" => {
                if let Some(text) = payload.get("refusal").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        message_content.push(json!({
                            "type": "refusal",
                            "refusal": text,
                        }));
                    }
                }
            }
            CanonicalContentBlock::Unknown { .. } => {}
        }
    }
    flush_openai_responses_message_item(
        &mut output,
        &mut message_content,
        &response_id,
        &mut message_index,
    );
    response.insert("output".to_string(), Value::Array(output));
    if let Some(usage) = &canonical.usage {
        response.insert(
            "usage".to_string(),
            canonical_usage_to_openai_responses_usage(usage),
        );
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
                response.insert(key.to_string(), value.clone());
            }
        }
        if let Some(service_tier) = request_object.get("service_tier").cloned() {
            response.insert("service_tier".to_string(), service_tier);
        }
    }
    response.extend(namespace_extension_object(
        &canonical.extensions,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        &response,
    ));
    response.extend(namespace_extension_object(
        &canonical.extensions,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
        &response,
    ));
    Value::Object(response)
}

fn image_block_is_generation_call(extensions: &BTreeMap<String, Value>) -> bool {
    extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| extensions.get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE))
        .and_then(|value| value.get("item_type"))
        .and_then(Value::as_str)
        .is_some_and(|value| value == "image_generation_call")
}

fn openai_responses_image_generation_call_item(
    response_id: &str,
    index: usize,
    data: &Option<String>,
    url: &Option<String>,
    media_type: &Option<String>,
) -> Value {
    let mut item = Map::new();
    item.insert(
        "id".to_string(),
        Value::String(format!("{response_id}_ig_{index}")),
    );
    item.insert(
        "type".to_string(),
        Value::String("image_generation_call".to_string()),
    );
    item.insert("status".to_string(), Value::String("completed".to_string()));
    item.insert("action".to_string(), Value::String("generate".to_string()));
    item.insert(
        "output_format".to_string(),
        Value::String(openai_responses_output_format_from_mime_type(
            media_type.as_deref().unwrap_or("image/png"),
        )),
    );
    if let Some(data) = data.as_ref().filter(|value| !value.trim().is_empty()) {
        item.insert("result".to_string(), Value::String(data.clone()));
    } else if let Some(url) = url.as_ref().filter(|value| !value.trim().is_empty()) {
        item.insert("url".to_string(), Value::String(url.clone()));
    } else {
        item.insert("result".to_string(), Value::String(String::new()));
    }
    Value::Object(item)
}

fn openai_responses_output_format_from_mime_type(mime_type: &str) -> String {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
    .to_string()
}

fn is_responses_web_search_tool(name: &str) -> bool {
    matches!(name, "web_search" | "web_search_preview")
}

fn web_search_query_from_value(input: &Value) -> String {
    input
        .get("query")
        .and_then(Value::as_str)
        .or_else(|| input.as_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_response_builder_emits_web_search_call_for_web_search_tool_use() {
        let response = CanonicalResponse {
            id: "resp_test".to_string(),
            model: "gpt-5-5-low".to_string(),
            content: vec![CanonicalContentBlock::ToolUse {
                id: "call_ws_1".to_string(),
                name: "web_search".to_string(),
                input: json!({"query": "today tech"}),
                extensions: BTreeMap::new(),
            }],
            outputs: Vec::new(),
            stop_reason: Some(CanonicalStopReason::ToolUse),
            usage: None,
            extensions: BTreeMap::new(),
        };

        let body = to_raw(&response, &json!({}), false);

        assert_eq!(body["output"][0]["type"], "web_search_call");
        assert_eq!(body["output"][0]["id"], "call_ws_1");
        assert_eq!(body["output"][0]["status"], "completed");
        assert_eq!(body["output"][0]["action"]["type"], "search");
        assert_eq!(body["output"][0]["action"]["query"], "today tech");
    }

    #[test]
    fn responses_response_parser_reads_web_search_call_as_tool_use() {
        let body = json!({
            "id": "resp_test",
            "model": "gpt-5-5-low",
            "status": "incomplete",
            "output": [{
                "type": "web_search_call",
                "id": "call_ws_1",
                "status": "completed",
                "action": {"type": "search", "query": "today tech"}
            }]
        });

        let canonical = from_raw(&body).expect("response should parse");

        assert!(
            matches!(canonical.content.first(), Some(CanonicalContentBlock::ToolUse {
            id,
            name,
            input,
            ..
        }) if id == "call_ws_1" && name == "web_search" && input["query"] == "today tech")
        );
    }
}
