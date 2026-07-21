use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Map, Value};

use super::encode_tool_result_error;

use crate::{
    formats::context::FormatContext,
    protocol::canonical::{
        canonical_content_block_to_openai_responses_part, canonical_extension_object_mut,
        canonical_tool_use_to_openai_responses_item, canonical_usage_to_openai_responses_usage,
        flush_openai_responses_message_item, is_openai_responses_raw_block,
        is_openai_thinking_block, namespace_extension_object, openai_responses_extensions,
        openai_responses_item_extension_object, openai_responses_output_to_canonical,
        openai_responses_usage_to_canonical, openai_service_tier_extension, CanonicalContentBlock,
        CanonicalResponse, CanonicalResponseOutput, CanonicalRole, CanonicalStopReason,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE, OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
};

const AETHER_RESPONSES_RAW_OUTPUT_KEY: &str = "openai_responses_raw_output";

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
    let (content, output_extensions) = openai_responses_output_to_canonical(body.get("output"))?;
    let has_tool_use = content
        .iter()
        .any(|block| matches!(block, CanonicalContentBlock::ToolUse { .. }));
    let stop_reason = if has_tool_use {
        Some(CanonicalStopReason::ToolUse)
    } else {
        match body.get("status").and_then(Value::as_str) {
            Some("incomplete") => Some(openai_responses_incomplete_stop_reason(body)),
            Some("failed") => Some(CanonicalStopReason::Unknown),
            _ => Some(CanonicalStopReason::EndTurn),
        }
    };
    let mut extensions = openai_responses_extensions(
        body,
        &[
            "id", "object", "model", "output", "usage", "status", "error",
        ],
    );
    if let Some(raw_status) = body.get("status").cloned() {
        canonical_extension_object_mut(&mut extensions, OPENAI_RESPONSES_EXTENSION_NAMESPACE)
            .insert("raw_status".to_string(), raw_status);
    }
    if let Some(raw_output) = body.get("output").cloned() {
        canonical_extension_object_mut(&mut extensions, "aether")
            .insert(AETHER_RESPONSES_RAW_OUTPUT_KEY.to_string(), raw_output);
    }
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
            extensions: output_extensions,
        }],
        content,
        stop_reason,
        usage: openai_responses_usage_to_canonical(body.get("usage")),
        extensions,
    })
}

fn openai_responses_incomplete_stop_reason(body: &Map<String, Value>) -> CanonicalStopReason {
    match body
        .get("incomplete_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
    {
        Some("content_filter") => CanonicalStopReason::ContentFiltered,
        Some("tool_calls") | Some("function_call") => CanonicalStopReason::ToolUse,
        _ => CanonicalStopReason::MaxTokens,
    }
}

pub fn to_raw(canonical: &CanonicalResponse, report_context: &Value, compact: bool) -> Value {
    let mut response = Map::new();
    let response_id = canonical.id.replace("chatcmpl", "resp");
    response.insert("id".to_string(), Value::String(response_id.clone()));
    response.insert(
        "object".to_string(),
        Value::String(if compact {
            "response.compaction".to_string()
        } else {
            "response".to_string()
        }),
    );
    if !compact {
        response.insert("status".to_string(), Value::String("completed".to_string()));
        response.insert("model".to_string(), Value::String(canonical.model.clone()));
        if let Some(raw_status) = canonical
            .extensions
            .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
            .or_else(|| {
                canonical
                    .extensions
                    .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
            })
            .and_then(Value::as_object)
            .and_then(|openai| openai.get("raw_status"))
            .cloned()
        {
            response.insert("status".to_string(), raw_status);
        }
    }

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
                extensions,
                ..
            } => {
                let encrypted_content = encrypted_content
                    .as_ref()
                    .filter(|value| !value.is_empty())
                    .filter(|_| is_openai_thinking_block(extensions));
                if text.trim().is_empty() && encrypted_content.is_none() {
                    continue;
                }
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
                if let Some(encrypted_content) = encrypted_content {
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
                id,
                name,
                input,
                extensions,
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
                    output.push(canonical_tool_use_to_openai_responses_item(
                        id, name, input, extensions,
                    ));
                }
            }
            CanonicalContentBlock::ToolResult {
                tool_use_id,
                output: result_output,
                content_text,
                is_error,
                extensions,
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
                    Value::String(
                        responses_tool_result_item_type(extensions)
                            .unwrap_or("function_call_output")
                            .to_string(),
                    ),
                );
                item.insert("call_id".to_string(), Value::String(tool_use_id.clone()));
                item.insert(
                    "output".to_string(),
                    encode_tool_result_error(
                        result_output.clone().unwrap_or_else(|| {
                            Value::String(content_text.clone().unwrap_or_default())
                        }),
                        *is_error,
                    ),
                );
                let extension_fields = openai_responses_item_extension_object(extensions, &item);
                item.extend(extension_fields);
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
            CanonicalContentBlock::Unknown {
                payload,
                extensions,
                ..
            } if is_openai_responses_raw_block(extensions) => {
                flush_openai_responses_message_item(
                    &mut output,
                    &mut message_content,
                    &response_id,
                    &mut message_index,
                );
                output.push(payload.clone());
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
    if let Some(raw_output) = canonical
        .extensions
        .get("aether")
        .and_then(Value::as_object)
        .and_then(|aether| aether.get(AETHER_RESPONSES_RAW_OUTPUT_KEY))
        .and_then(Value::as_array)
    {
        output.clone_from(raw_output);
    }
    response.insert("output".to_string(), Value::Array(output));
    if let Some(usage) = &canonical.usage {
        response.insert(
            "usage".to_string(),
            canonical_usage_to_openai_responses_usage(usage),
        );
    }
    if compact {
        let created_at = canonical
            .extensions
            .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
            .or_else(|| {
                canonical
                    .extensions
                    .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
            })
            .and_then(Value::as_object)
            .and_then(|openai| openai.get("created_at").or_else(|| openai.get("created")))
            .and_then(openai_responses_timestamp_value)
            .unwrap_or_else(openai_responses_current_timestamp);
        response.insert("created_at".to_string(), Value::from(created_at));
        return Value::Object(response);
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
    }
    if let Some(service_tier) = openai_service_tier_extension(&canonical.extensions).cloned() {
        response.insert("service_tier".to_string(), service_tier);
    }
    let mut extension_fields = namespace_extension_object(
        &canonical.extensions,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE,
        &response,
    );
    extension_fields.remove("raw_status");
    response.extend(extension_fields);
    let mut legacy_extension_fields = namespace_extension_object(
        &canonical.extensions,
        OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
        &response,
    );
    legacy_extension_fields.remove("raw_status");
    response.extend(legacy_extension_fields);
    ensure_modern_openai_responses_response_fields(&mut response);
    Value::Object(response)
}

fn responses_tool_result_item_type(extensions: &BTreeMap<String, Value>) -> Option<&str> {
    let item_type = extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| extensions.get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE))
        .and_then(|value| value.get("item_type"))
        .and_then(Value::as_str)?;
    matches!(
        item_type,
        "custom_tool_call_output"
            | "local_shell_call_output"
            | "shell_call_output"
            | "apply_patch_call_output"
            | "computer_call_output"
    )
    .then_some(item_type)
}

pub(crate) fn ensure_modern_openai_responses_response_fields(
    response: &mut Map<String, Value>,
) -> bool {
    let mut changed = false;
    if !response
        .get("output")
        .is_some_and(|value| matches!(value, Value::Array(_)))
    {
        response.insert("output".to_string(), Value::Array(Vec::new()));
        changed = true;
    }
    if !response.contains_key("created_at") {
        let created_at = response
            .get("created")
            .and_then(openai_responses_timestamp_value)
            .unwrap_or_else(openai_responses_current_timestamp);
        response.insert("created_at".to_string(), Value::from(created_at));
        changed = true;
    }
    if response
        .get("status")
        .and_then(Value::as_str)
        .is_none_or(|status| status == "completed")
        && !response.contains_key("completed_at")
    {
        let completed_at = response
            .get("created_at")
            .and_then(openai_responses_timestamp_value)
            .unwrap_or_else(openai_responses_current_timestamp);
        response.insert("completed_at".to_string(), Value::from(completed_at));
        changed = true;
    }
    if !response.contains_key("output_text") {
        let output_text = openai_responses_output_text_from_output(response.get("output"));
        response.insert("output_text".to_string(), Value::String(output_text));
        changed = true;
    }
    changed
}

pub(crate) fn openai_responses_output_text_from_output(output: Option<&Value>) -> String {
    output
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|part| {
            let part = part.as_object()?;
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("output_text" | "text")
            )
            .then(|| part.get("text").and_then(Value::as_str).unwrap_or_default())
        })
        .collect::<String>()
}

pub(crate) fn openai_responses_current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn openai_responses_timestamp_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
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
        assert_eq!(body["output_text"], "");
        assert!(body["created_at"].as_i64().is_some());
        assert!(body["completed_at"].as_i64().is_some());
    }

    #[test]
    fn responses_response_builder_emits_modern_output_text_and_preserves_source_fields() {
        let mut extensions = BTreeMap::new();
        extensions.insert(
            OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
            json!({
                "created_at": 111,
                "completed_at": 222,
                "output_text": "source text",
                "conversation": {"id": "conv_123"}
            }),
        );
        let response = CanonicalResponse {
            id: "resp_text".to_string(),
            model: "gpt-5".to_string(),
            content: vec![CanonicalContentBlock::Text {
                text: "generated text".to_string(),
                extensions: BTreeMap::new(),
            }],
            outputs: Vec::new(),
            stop_reason: Some(CanonicalStopReason::EndTurn),
            usage: None,
            extensions,
        };

        let body = to_raw(&response, &json!({}), false);

        assert_eq!(body["output_text"], "source text");
        assert_eq!(body["created_at"], 111);
        assert_eq!(body["completed_at"], 222);
        assert_eq!(body["conversation"]["id"], "conv_123");
    }

    #[test]
    fn responses_response_builder_encodes_tool_errors_in_output() {
        let response = CanonicalResponse {
            id: "resp_tool_error".to_string(),
            model: "gpt-5.6-sol".to_string(),
            content: vec![CanonicalContentBlock::ToolResult {
                tool_use_id: "call_error".to_string(),
                name: None,
                output: Some(json!("command failed")),
                content_text: None,
                is_error: true,
                extensions: BTreeMap::new(),
            }],
            outputs: Vec::new(),
            stop_reason: Some(CanonicalStopReason::EndTurn),
            usage: None,
            extensions: BTreeMap::new(),
        };

        let body = to_raw(&response, &json!({}), false);
        let item = &body["output"][0];

        assert_eq!(item["type"], "function_call_output");
        assert_eq!(item["call_id"], "call_error");
        assert_eq!(item["output"], "[tool error]\ncommand failed");
        assert!(item.get("is_error").is_none());
    }

    #[test]
    fn compact_response_builder_emits_the_compaction_resource_shape() {
        let mut extensions = BTreeMap::new();
        extensions.insert(
            OPENAI_RESPONSES_EXTENSION_NAMESPACE.to_string(),
            json!({"created_at": 123}),
        );
        let response = CanonicalResponse {
            id: "resp_compact".to_string(),
            model: "gpt-5.6-sol".to_string(),
            content: vec![CanonicalContentBlock::Text {
                text: "compacted context".to_string(),
                extensions: BTreeMap::new(),
            }],
            outputs: Vec::new(),
            stop_reason: Some(CanonicalStopReason::EndTurn),
            usage: None,
            extensions,
        };

        let body = to_raw(&response, &json!({}), true);
        let keys = body
            .as_object()
            .expect("Compact response should be an object")
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(body["object"], "response.compaction");
        assert_eq!(body["created_at"], 123);
        assert_eq!(
            keys,
            std::collections::BTreeSet::from(["created_at", "id", "object", "output"])
        );
    }

    #[test]
    fn responses_response_parser_preserves_encrypted_reasoning_without_summary() {
        let body = json!({
            "id": "resp_test",
            "model": "gpt-5",
            "status": "completed",
            "output": [{
                "type": "reasoning",
                "id": "rs_1",
                "status": "completed",
                "summary": [],
                "encrypted_content": "openai-opaque"
            }]
        });

        let canonical = from_raw(&body).expect("response should parse");

        assert!(matches!(
            canonical.content.first(),
            Some(CanonicalContentBlock::Thinking {
                text,
                encrypted_content,
                ..
            }) if text.is_empty() && encrypted_content.as_deref() == Some("openai-opaque")
        ));

        let rebuilt = to_raw(&canonical, &json!({}), false);
        assert_eq!(rebuilt["output"][0]["type"], "reasoning");
        assert_eq!(
            rebuilt["output"][0]["encrypted_content"],
            json!("openai-opaque")
        );
    }

    #[test]
    fn responses_response_builder_does_not_emit_claude_redacted_as_openai_encrypted_content() {
        let mut extensions = BTreeMap::new();
        extensions.insert("aether".to_string(), json!({"source": "claude_thinking"}));
        let response = CanonicalResponse {
            id: "msg_claude".to_string(),
            model: "claude-sonnet".to_string(),
            content: vec![CanonicalContentBlock::Thinking {
                text: String::new(),
                signature: None,
                encrypted_content: Some("{\"type\":\"redacted_thinking\",\"v\":5}".to_string()),
                extensions,
            }],
            outputs: Vec::new(),
            stop_reason: Some(CanonicalStopReason::EndTurn),
            usage: None,
            extensions: BTreeMap::new(),
        };

        let body = to_raw(&response, &json!({}), false);

        assert!(body["output"].as_array().expect("output").is_empty());
        assert!(!body.to_string().contains("encrypted_content"));
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
