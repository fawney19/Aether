use serde_json::{json, Value};

use super::super::from_openai_chat::{
    build_openai_cli_response_with_reasoning, OpenAiCliResponseUsage,
};
use super::shared::{build_generated_tool_call_id, canonicalize_tool_arguments};

pub fn convert_claude_cli_response_to_openai_cli(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    let body = body_json.as_object()?;
    let content = body.get("content")?.as_array()?;
    let mut text = String::new();
    let mut reasoning_summaries = Vec::new();
    let mut function_calls = Vec::new();
    for (index, block) in content.iter().enumerate() {
        let block = block.as_object()?;
        match block.get("type")?.as_str()? {
            "text" => {
                text.push_str(block.get("text")?.as_str()?);
            }
            "thinking" => {
                if let Some(piece) = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .or_else(|| block.get("text").and_then(Value::as_str))
                {
                    if !piece.trim().is_empty() {
                        reasoning_summaries.push(piece.to_string());
                    }
                }
            }
            "tool_use" => {
                let tool_name = block.get("name")?.as_str()?;
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| build_generated_tool_call_id(index));
                let arguments = canonicalize_tool_arguments(block.get("input").cloned());
                function_calls.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": tool_name,
                    "arguments": arguments,
                }));
            }
            _ => continue,
        }
    }

    let usage = body.get("usage").and_then(Value::as_object);
    let prompt_tokens = usage
        .and_then(|value| value.get("input_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .and_then(|value| value.get("output_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_tokens = prompt_tokens + output_tokens;
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

    Some(build_openai_cli_response_with_reasoning(
        response_id,
        model,
        &text,
        reasoning_summaries,
        function_calls,
        OpenAiCliResponseUsage {
            prompt_tokens,
            output_tokens,
            total_tokens,
        },
    ))
}
