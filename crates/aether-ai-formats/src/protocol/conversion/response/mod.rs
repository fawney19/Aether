//! Pairwise response conversion helpers.
//!
//! These helpers keep the call sites readable while delegating wire-format
//! parsing and emitting to `formats::<format>::response` through the registry's
//! canonical IR path.

use serde_json::{json, Value};

use crate::protocol::{context::FormatContext, registry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenAiResponsesResponseUsage {
    pub prompt_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

pub fn convert_claude_chat_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    registry::convert_response(
        "claude:messages",
        "openai:chat",
        body_json,
        &response_context(report_context),
    )
    .ok()
}

pub fn convert_gemini_chat_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    registry::convert_response(
        "gemini:generate_content",
        "openai:chat",
        body_json,
        &response_context(report_context),
    )
    .ok()
}

pub fn convert_openai_chat_response_to_claude_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    registry::convert_response(
        "openai:chat",
        "claude:messages",
        body_json,
        &response_context(report_context),
    )
    .ok()
}

pub fn convert_openai_chat_response_to_gemini_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    registry::convert_response(
        "openai:chat",
        "gemini:generate_content",
        body_json,
        &response_context(report_context),
    )
    .ok()
}

pub fn convert_openai_responses_response_to_openai_chat(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    registry::convert_response(
        "openai:responses",
        "openai:chat",
        body_json,
        &response_context(report_context),
    )
    .ok()
}

pub fn convert_openai_chat_response_to_openai_responses(
    body_json: &Value,
    report_context: &Value,
    compact: bool,
) -> Option<Value> {
    let target_format = if compact {
        "openai:responses:compact"
    } else {
        "openai:responses"
    };
    registry::convert_response(
        "openai:chat",
        target_format,
        body_json,
        &response_context(report_context),
    )
    .ok()
}

pub fn convert_claude_response_to_openai_responses(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    registry::convert_response(
        "claude:messages",
        "openai:responses",
        body_json,
        &response_context(report_context),
    )
    .ok()
}

pub fn convert_gemini_response_to_openai_responses(
    body_json: &Value,
    report_context: &Value,
) -> Option<Value> {
    registry::convert_response(
        "gemini:generate_content",
        "openai:responses",
        body_json,
        &response_context(report_context),
    )
    .ok()
}

pub fn build_openai_responses_response(
    response_id: &str,
    model: &str,
    text: &str,
    function_calls: Vec<Value>,
    prompt_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
) -> Value {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![json!({
            "type": "output_text",
            "text": text,
            "annotations": []
        })]
    };
    build_openai_responses_response_with_content(
        response_id,
        model,
        content,
        Vec::new(),
        function_calls,
        OpenAiResponsesResponseUsage {
            prompt_tokens,
            output_tokens,
            total_tokens,
        },
    )
}

pub fn build_openai_responses_response_with_reasoning(
    response_id: &str,
    model: &str,
    text: &str,
    reasoning_summaries: Vec<String>,
    function_calls: Vec<Value>,
    usage: OpenAiResponsesResponseUsage,
) -> Value {
    let content = if text.is_empty() {
        Vec::new()
    } else {
        vec![json!({
            "type": "output_text",
            "text": text,
            "annotations": []
        })]
    };
    build_openai_responses_response_with_content(
        response_id,
        model,
        content,
        reasoning_summaries,
        function_calls,
        usage,
    )
}

pub fn build_openai_responses_response_with_content(
    response_id: &str,
    model: &str,
    content: Vec<Value>,
    reasoning_summaries: Vec<String>,
    function_calls: Vec<Value>,
    usage: OpenAiResponsesResponseUsage,
) -> Value {
    let mut output = Vec::new();
    for (index, summary) in reasoning_summaries.into_iter().enumerate() {
        let trimmed = summary.trim();
        if trimmed.is_empty() {
            continue;
        }
        output.push(json!({
            "type": "reasoning",
            "id": format!("{response_id}_rs_{index}"),
            "status": "completed",
            "summary": [{
                "type": "summary_text",
                "text": trimmed,
            }]
        }));
    }
    if !content.is_empty() {
        output.push(json!({
            "type": "message",
            "id": format!("{response_id}_msg"),
            "role": "assistant",
            "status": "completed",
            "content": content
        }));
    }
    output.extend(function_calls);
    json!({
        "id": response_id,
        "object": "response",
        "status": "completed",
        "model": model,
        "output": output,
        "usage": {
            "input_tokens": usage.prompt_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens,
        }
    })
}

fn response_context(report_context: &Value) -> FormatContext {
    let mut context = FormatContext::default().with_report_context(report_context.clone());
    if let Some(model) = report_context
        .get("mapped_model")
        .and_then(Value::as_str)
        .or_else(|| report_context.get("model").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
    {
        context = context.with_mapped_model(model);
    }
    context
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        convert_claude_chat_response_to_openai_chat,
        convert_openai_chat_response_to_openai_responses,
    };

    #[test]
    fn pairwise_response_helper_routes_through_registry() {
        let body = json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "model": "gpt-source",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hello"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
        });

        let converted = convert_openai_chat_response_to_openai_responses(&body, &json!({}), false)
            .expect("responses response");

        assert_eq!(converted["object"], "response");
        assert_eq!(converted["output"][0]["type"], "message");
    }

    #[test]
    fn pairwise_response_helper_uses_report_context_model_fallback() {
        let body = json!({
            "id": "msg-test",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 1, "output_tokens": 2}
        });

        let converted = convert_claude_chat_response_to_openai_chat(
            &body,
            &json!({"mapped_model": "gpt-target"}),
        )
        .expect("openai chat response");

        assert_eq!(converted["model"], "gpt-target");
        assert_eq!(converted["choices"][0]["message"]["content"], "hello");
    }
}
