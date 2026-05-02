use serde_json::{json, Map, Value};

use crate::{
    protocol::canonical::{
        canonical_extension_object_mut, gemini_extensions, gemini_part_to_canonical_block,
        gemini_stop_reason_to_canonical, gemini_usage_to_canonical, CanonicalContentBlock,
        CanonicalResponse, CanonicalResponseOutput, CanonicalRole, CanonicalStopReason,
        CanonicalUsage,
    },
    protocol::context::FormatContext,
};

pub fn from(body: &Value, _ctx: &FormatContext) -> Option<CanonicalResponse> {
    from_raw(body)
}

pub fn to(response: &CanonicalResponse, ctx: &FormatContext) -> Option<Value> {
    to_raw(response, &ctx.report_context_value())
}

pub fn from_raw(body_json: &Value) -> Option<CanonicalResponse> {
    let body = body_json.as_object()?;
    if body.contains_key("error") {
        return None;
    }

    let candidates = body.get("candidates")?.as_array()?;
    let mut outputs = Vec::new();
    for (fallback_index, candidate) in candidates.iter().enumerate() {
        let candidate_object = candidate.as_object()?;
        let parts = candidate_object
            .get("content")
            .and_then(Value::as_object)
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let content = parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| gemini_part_to_canonical_block(part, index))
            .collect::<Vec<_>>();
        let mut stop_reason = candidate_object
            .get("finishReason")
            .or_else(|| candidate_object.get("finish_reason"))
            .and_then(Value::as_str)
            .and_then(gemini_stop_reason_to_canonical);
        if content
            .iter()
            .any(|block| matches!(block, CanonicalContentBlock::ToolUse { .. }))
            && stop_reason
                .as_ref()
                .is_none_or(|reason| matches!(reason, CanonicalStopReason::EndTurn))
        {
            stop_reason = Some(CanonicalStopReason::ToolUse);
        }
        outputs.push(CanonicalResponseOutput {
            index: candidate_object
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(fallback_index),
            role: CanonicalRole::Assistant,
            content,
            stop_reason,
            extensions: Default::default(),
        });
    }
    let content = outputs
        .first()
        .map(|output| output.content.clone())
        .unwrap_or_default();
    let stop_reason = outputs
        .first()
        .and_then(|output| output.stop_reason.clone());

    let mut canonical = CanonicalResponse {
        id: body
            .get("responseId")
            .or_else(|| body.get("_v1internal_response_id"))
            .and_then(Value::as_str)
            .unwrap_or("gemini-local-finalize")
            .to_string(),
        model: body
            .get("modelVersion")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        outputs,
        content,
        stop_reason,
        usage: gemini_usage_to_canonical(body.get("usageMetadata")),
        extensions: gemini_extensions(
            body,
            &[
                "responseId",
                "_v1internal_response_id",
                "modelVersion",
                "candidates",
                "usageMetadata",
            ],
        ),
    };
    if let Some(candidates) = body.get("candidates").cloned() {
        canonical_extension_object_mut(&mut canonical.extensions, "gemini")
            .insert("raw_candidates".to_string(), candidates);
    }
    Some(canonical)
}

pub fn to_raw(canonical: &CanonicalResponse, report_context: &Value) -> Option<Value> {
    let mut response = canonical_to_gemini_response(canonical, report_context)?;
    if let Some(object) = response.as_object_mut() {
        if let Some(gemini) = canonical
            .extensions
            .get("gemini")
            .and_then(Value::as_object)
        {
            for (key, value) in gemini {
                if key == "raw_candidates" || object.contains_key(key) {
                    continue;
                }
                object.insert(key.clone(), value.clone());
            }
        }
    }
    Some(response)
}

fn canonical_to_gemini_response(
    canonical: &CanonicalResponse,
    report_context: &Value,
) -> Option<Value> {
    let outputs = if canonical.outputs.is_empty() {
        vec![CanonicalResponseOutput {
            index: 0,
            role: crate::protocol::canonical::CanonicalRole::Assistant,
            content: canonical.content.clone(),
            stop_reason: canonical.stop_reason.clone(),
            extensions: Default::default(),
        }]
    } else {
        canonical.outputs.clone()
    };
    let mut candidates = Vec::new();
    for output in outputs {
        let parts = canonical_blocks_to_gemini_parts(&output.content)?;
        candidates.push(json!({
            "index": output.index,
            "content": {
                "role": "model",
                "parts": parts,
            },
            "finishReason": canonical_stop_reason_to_gemini(
                output.stop_reason.as_ref().or(canonical.stop_reason.as_ref())
            ),
        }));
    }

    let mut response = Map::new();
    response.insert(
        "responseId".to_string(),
        Value::String(if canonical.id.trim().is_empty() {
            "resp-local-finalize".to_string()
        } else {
            canonical.id.clone()
        }),
    );
    response.insert(
        "modelVersion".to_string(),
        Value::String(
            if canonical.model.trim().is_empty() || canonical.model == "unknown" {
                report_context
                    .get("mapped_model")
                    .and_then(Value::as_str)
                    .or_else(|| report_context.get("model").and_then(Value::as_str))
                    .unwrap_or("unknown")
                    .to_string()
            } else {
                canonical.model.clone()
            },
        ),
    );
    response.insert("candidates".to_string(), Value::Array(candidates));
    if let Some(usage) = &canonical.usage {
        response.insert(
            "usageMetadata".to_string(),
            canonical_usage_to_gemini_usage_metadata(usage),
        );
    }
    Some(Value::Object(response))
}

fn canonical_blocks_to_gemini_parts(blocks: &[CanonicalContentBlock]) -> Option<Vec<Value>> {
    let mut parts = Vec::new();
    for block in blocks {
        if let Some(part) = canonical_block_to_gemini_part(block)? {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        parts.push(json!({ "text": "" }));
    }
    Some(parts)
}

fn canonical_block_to_gemini_part(block: &CanonicalContentBlock) -> Option<Option<Value>> {
    match block {
        CanonicalContentBlock::Text { text, .. } => Some(Some(json!({ "text": text }))),
        CanonicalContentBlock::Thinking {
            text, signature, ..
        } => {
            if text.trim().is_empty() {
                return Some(None);
            }
            let mut part = Map::new();
            part.insert("text".to_string(), Value::String(text.clone()));
            part.insert("thought".to_string(), Value::Bool(true));
            if let Some(signature) = signature.as_ref().filter(|value| !value.is_empty()) {
                part.insert(
                    "thoughtSignature".to_string(),
                    Value::String(signature.clone()),
                );
            }
            Some(Some(Value::Object(part)))
        }
        CanonicalContentBlock::ToolUse {
            id, name, input, ..
        } => Some(Some(json!({
            "functionCall": {
                "id": id,
                "name": name,
                "args": gemini_function_args(input),
            }
        }))),
        CanonicalContentBlock::ToolResult {
            tool_use_id,
            name,
            output,
            content_text,
            ..
        } => Some(Some(json!({
            "functionResponse": {
                "id": tool_use_id,
                "name": name.clone().unwrap_or_else(|| tool_use_id.clone()),
                "response": gemini_function_response(output.as_ref(), content_text.as_deref()),
            }
        }))),
        CanonicalContentBlock::Image {
            data,
            url,
            media_type,
            ..
        } => Some(Some(canonical_media_to_gemini_part(
            media_type.as_deref().unwrap_or("image/png"),
            data.as_deref(),
            url.as_deref(),
        ))),
        CanonicalContentBlock::File {
            data,
            file_url,
            media_type,
            ..
        } => Some(Some(canonical_media_to_gemini_part(
            media_type.as_deref().unwrap_or("application/octet-stream"),
            data.as_deref(),
            file_url.as_deref(),
        ))),
        CanonicalContentBlock::Audio {
            data, media_type, ..
        } => Some(data.as_ref().map(|data| {
            json!({
                "inlineData": {
                    "mimeType": media_type.clone().unwrap_or_else(|| "audio/mpeg".to_string()),
                    "data": data,
                }
            })
        })),
        CanonicalContentBlock::Unknown { .. } => Some(None),
    }
}

fn canonical_media_to_gemini_part(
    media_type: &str,
    data: Option<&str>,
    url: Option<&str>,
) -> Value {
    if let Some(data) = data.filter(|value| !value.is_empty()) {
        return json!({
            "inlineData": {
                "mimeType": media_type,
                "data": data,
            }
        });
    }
    json!({
        "fileData": {
            "mimeType": media_type,
            "fileUri": url.unwrap_or_default(),
        }
    })
}

fn gemini_function_args(input: &Value) -> Value {
    match input {
        Value::Object(_) => input.clone(),
        Value::Null => json!({}),
        other => json!({ "value": other.clone() }),
    }
}

fn gemini_function_response(output: Option<&Value>, content_text: Option<&str>) -> Value {
    match output {
        Some(Value::Object(object)) => Value::Object(object.clone()),
        Some(value) => json!({ "result": value }),
        None => json!({ "result": content_text.unwrap_or_default() }),
    }
}

fn canonical_stop_reason_to_gemini(reason: Option<&CanonicalStopReason>) -> Value {
    Value::String(
        match reason {
            Some(CanonicalStopReason::MaxTokens) => "MAX_TOKENS",
            Some(CanonicalStopReason::ContentFiltered) | Some(CanonicalStopReason::Refusal) => {
                "SAFETY"
            }
            Some(CanonicalStopReason::Unknown) => "OTHER",
            _ => "STOP",
        }
        .to_string(),
    )
}

fn canonical_usage_to_gemini_usage_metadata(usage: &CanonicalUsage) -> Value {
    let mut out = Map::new();
    out.insert(
        "promptTokenCount".to_string(),
        Value::from(usage.input_tokens),
    );
    out.insert(
        "candidatesTokenCount".to_string(),
        Value::from(usage.output_tokens.saturating_sub(usage.reasoning_tokens)),
    );
    out.insert(
        "totalTokenCount".to_string(),
        Value::from(usage.total_tokens),
    );
    if usage.reasoning_tokens > 0 {
        out.insert(
            "thoughtsTokenCount".to_string(),
            Value::from(usage.reasoning_tokens),
        );
    }
    Value::Object(out)
}
