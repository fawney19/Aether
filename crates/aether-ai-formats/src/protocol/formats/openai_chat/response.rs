use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::{
    protocol::canonical::{
        canonical_blocks_to_openai_chat_message, canonical_stop_reason_to_openai,
        canonical_usage_to_openai, openai_extensions, openai_finish_reason_to_canonical,
        openai_message_content_blocks, openai_usage_to_canonical, CanonicalContentBlock,
        CanonicalResponse, CanonicalResponseOutput, CanonicalRole,
        OPENAI_RESPONSES_EXTENSION_NAMESPACE, OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE,
    },
    protocol::context::FormatContext,
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalResponse> {
    from_raw(body)
}

pub fn to(response: &CanonicalResponse, ctx: &FormatContext) -> Option<Value> {
    let mut body = to_raw(response);
    if body.get("service_tier").is_none() {
        if let Some(service_tier) = ctx
            .report_context_value()
            .get("original_request_body")
            .and_then(Value::as_object)
            .and_then(|request| request.get("service_tier"))
            .cloned()
        {
            body["service_tier"] = service_tier;
        }
    }
    Some(body)
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalResponse> {
    let body = body_json.as_object()?;
    if body.contains_key("error") {
        return None;
    }
    let mut outputs = Vec::new();
    for (fallback_index, choice_value) in body
        .get("choices")
        .and_then(Value::as_array)?
        .iter()
        .enumerate()
    {
        let choice = choice_value.as_object()?;
        let message = choice.get("message").and_then(Value::as_object)?;
        let mut content = openai_message_content_blocks(message)?;
        if !content
            .iter()
            .any(|block| matches!(block, CanonicalContentBlock::Thinking { .. }))
        {
            if let Some(reasoning_content) = message
                .get("reasoning_content")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                content.insert(
                    0,
                    CanonicalContentBlock::Thinking {
                        text: reasoning_content.to_string(),
                        signature: None,
                        encrypted_content: None,
                        extensions: BTreeMap::new(),
                    },
                );
            }
        }
        let stop_reason =
            openai_finish_reason_to_canonical(choice.get("finish_reason").and_then(Value::as_str));
        outputs.push(CanonicalResponseOutput {
            index: choice
                .get("index")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .unwrap_or(fallback_index),
            role: CanonicalRole::Assistant,
            content,
            stop_reason,
            extensions: BTreeMap::new(),
        });
    }
    let first_output = outputs.first()?;
    let content = first_output.content.clone();
    let stop_reason = first_output.stop_reason.clone();
    Some(CanonicalResponse {
        id: body
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("chatcmpl-unknown")
            .to_string(),
        model: body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        outputs,
        content,
        stop_reason,
        usage: openai_usage_to_canonical(body.get("usage")),
        extensions: openai_extensions(
            body,
            &["id", "object", "model", "choices", "usage", "created"],
        ),
    })
}

pub fn to_raw(canonical: &CanonicalResponse) -> Value {
    let outputs: Vec<CanonicalResponseOutput> = if canonical.outputs.is_empty() {
        vec![CanonicalResponseOutput {
            index: 0,
            role: CanonicalRole::Assistant,
            content: canonical.content.clone(),
            stop_reason: canonical.stop_reason.clone(),
            extensions: BTreeMap::new(),
        }]
    } else {
        canonical.outputs.clone()
    };
    let choices: Vec<Value> = outputs
        .iter()
        .enumerate()
        .map(|(fallback_index, output)| {
            json!({
                "index": output.index,
                "message": canonical_blocks_to_openai_chat_message(&output.content),
                "finish_reason": canonical_stop_reason_to_openai(output.stop_reason.as_ref()),
            })
            .as_object()
            .map(|choice| {
                let mut choice = choice.clone();
                if output.index == 0 && fallback_index != 0 {
                    choice.insert("index".to_string(), Value::from(fallback_index as u64));
                }
                Value::Object(choice)
            })
            .unwrap_or_else(|| json!({}))
        })
        .collect();

    let mut response = json!({
        "id": canonical.id,
        "object": "chat.completion",
        "model": canonical.model,
        "choices": choices,
        "usage": canonical.usage.as_ref().map(canonical_usage_to_openai).unwrap_or_else(|| json!({
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
        })),
    });
    if let Some(created_at) = canonical
        .extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| {
            canonical
                .extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .and_then(|value| value.get("created_at"))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().map(|value| value as i64))
        })
    {
        response["created"] = Value::from(created_at);
    }
    if let Some(service_tier) = canonical
        .extensions
        .get(OPENAI_RESPONSES_EXTENSION_NAMESPACE)
        .or_else(|| {
            canonical
                .extensions
                .get(OPENAI_RESPONSES_LEGACY_EXTENSION_NAMESPACE)
        })
        .and_then(|value| value.get("service_tier"))
        .cloned()
    {
        response["service_tier"] = service_tier;
    }
    response
}
