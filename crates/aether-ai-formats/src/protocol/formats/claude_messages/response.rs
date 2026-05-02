use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::{
    protocol::canonical::{
        canonical_blocks_to_claude, canonical_stop_reason_to_claude, canonical_usage_to_claude,
        claude_content_to_canonical_blocks, claude_extensions, claude_stop_reason_to_canonical,
        claude_usage_to_canonical, namespace_extension_object, CanonicalResponse,
        CanonicalResponseOutput, CanonicalRole,
    },
    protocol::context::FormatContext,
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalResponse> {
    from_raw(body)
}

pub fn to(response: &CanonicalResponse, _ctx: &FormatContext) -> Option<Value> {
    Some(to_raw(response))
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalResponse> {
    let body = body_json.as_object()?;
    if body.contains_key("error") || body.get("type").and_then(Value::as_str) == Some("error") {
        return None;
    }
    let content = claude_content_to_canonical_blocks(body.get("content"))?;
    let stop_reason =
        claude_stop_reason_to_canonical(body.get("stop_reason").and_then(Value::as_str));
    Some(CanonicalResponse {
        id: body
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("msg-unknown")
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
        usage: claude_usage_to_canonical(body.get("usage")),
        extensions: claude_extensions(
            body,
            &[
                "id",
                "type",
                "role",
                "model",
                "content",
                "stop_reason",
                "stop_sequence",
                "usage",
            ],
        ),
    })
}

pub fn to_raw(canonical: &CanonicalResponse) -> Value {
    let mut content = canonical_blocks_to_claude(&canonical.content, CanonicalRole::Assistant)
        .unwrap_or_default();
    if content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": "",
        }));
    }
    let mut response = json!({
        "id": canonical.id,
        "type": "message",
        "role": "assistant",
        "model": canonical.model,
        "content": content,
        "stop_reason": canonical_stop_reason_to_claude(canonical.stop_reason.as_ref()),
        "usage": canonical.usage.as_ref().map(canonical_usage_to_claude).unwrap_or_else(|| json!({
            "input_tokens": 0,
            "output_tokens": 0,
        })),
    });
    if let Some(object) = response.as_object_mut() {
        object.extend(namespace_extension_object(
            &canonical.extensions,
            "claude",
            object,
        ));
    }
    response
}
