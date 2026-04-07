use serde_json::{json, Value};

use super::super::from_openai_chat::{
    build_openai_cli_response_with_content, OpenAiCliResponseUsage,
};
use super::shared::{
    build_generated_tool_call_id, canonicalize_tool_arguments, extract_gemini_image_url,
};

pub fn convert_gemini_cli_response_to_openai_cli(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let body = body_json.as_object()?;
    let candidates = body.get("candidates")?.as_array()?;
    let first_candidate = candidates.first()?.as_object()?;
    let content = first_candidate.get("content")?.as_object()?;
    let parts = content.get("parts")?.as_array()?;
    let mut message_content = Vec::new();
    let mut reasoning_summaries = Vec::new();
    let mut function_calls = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let part = part.as_object()?;
        if let Some(piece) = part.get("text").and_then(Value::as_str) {
            if part
                .get("thought")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                if !piece.trim().is_empty() {
                    reasoning_summaries.push(piece.to_string());
                }
            } else {
                message_content.push(json!({
                    "type": "output_text",
                    "text": piece,
                    "annotations": []
                }));
            }
        } else if let Some(function_call) = part.get("functionCall").and_then(Value::as_object) {
            let tool_name = function_call.get("name")?.as_str()?;
            let call_id = function_call
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| build_generated_tool_call_id(index));
            let arguments = canonicalize_tool_arguments(function_call.get("args").cloned());
            function_calls.push(json!({
                "type": "function_call",
                "call_id": call_id,
                "name": tool_name,
                "arguments": arguments,
            }));
        } else if let Some(image_url) = extract_gemini_image_url(part) {
            message_content.push(json!({
                "type": "output_image",
                "image_url": image_url,
            }));
        } else {
            continue;
        }
    }

    let usage = body.get("usageMetadata").and_then(Value::as_object);
    let prompt_tokens = usage
        .and_then(|value| value.get("promptTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .map(|value| {
            value
                .get("candidatesTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + value
                    .get("thoughtsTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
        })
        .unwrap_or(0);
    let total_tokens = usage
        .and_then(|value| value.get("totalTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or(prompt_tokens + output_tokens);
    let model = body
        .get("modelVersion")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("mapped_model").and_then(Value::as_str))
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .unwrap_or("unknown");
    let response_id = body
        .get("responseId")
        .or_else(|| body.get("_v1internal_response_id"))
        .and_then(Value::as_str)
        .unwrap_or("resp-local-finalize");

    Some(build_openai_cli_response_with_content(
        response_id,
        model,
        message_content,
        reasoning_summaries,
        function_calls,
        OpenAiCliResponseUsage {
            prompt_tokens,
            output_tokens,
            total_tokens,
        },
    ))
}
