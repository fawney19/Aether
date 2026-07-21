use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::{
    formats::context::FormatContext,
    protocol::canonical::{
        canonical_blocks_to_claude, canonical_extension_object_mut,
        canonical_stop_reason_to_claude, canonical_usage_to_claude,
        claude_content_to_canonical_blocks, claude_extensions, claude_stop_reason_to_canonical,
        claude_usage_to_canonical, namespace_extension_object, CanonicalResponse,
        CanonicalResponseOutput, CanonicalRole,
    },
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
    let mut extensions = claude_extensions(
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
    );
    if let Some(raw_stop_reason) = body.get("stop_reason").cloned() {
        canonical_extension_object_mut(&mut extensions, "claude")
            .insert("raw_stop_reason".to_string(), raw_stop_reason);
    }
    if let Some(raw_stop_sequence) = body.get("stop_sequence").cloned() {
        canonical_extension_object_mut(&mut extensions, "claude")
            .insert("raw_stop_sequence".to_string(), raw_stop_sequence);
    }
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
        extensions,
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
    if let Some(claude) = canonical
        .extensions
        .get("claude")
        .and_then(Value::as_object)
    {
        if let Some(raw_stop_reason) = claude.get("raw_stop_reason").cloned() {
            response["stop_reason"] = raw_stop_reason;
        }
        if let Some(raw_stop_sequence) = claude.get("raw_stop_sequence").cloned() {
            response["stop_sequence"] = raw_stop_sequence;
        }
    }
    if let Some(object) = response.as_object_mut() {
        let mut extra = namespace_extension_object(&canonical.extensions, "claude", object);
        extra.remove("raw_stop_reason");
        extra.remove("raw_stop_sequence");
        object.extend(extra);
    }
    response
}
