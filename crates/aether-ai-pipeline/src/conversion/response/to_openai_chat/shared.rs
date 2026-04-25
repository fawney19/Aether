use serde_json::{Map, Value};

pub(super) fn build_generated_tool_call_id(index: usize) -> String {
    format!("call_auto_{index}")
}

pub(super) fn canonicalize_tool_arguments(value: Option<Value>) -> String {
    match value {
        Some(Value::String(text)) => text,
        Some(other) => serde_json::to_string(&other).unwrap_or_else(|_| "null".to_string()),
        None => "{}".to_string(),
    }
}

pub(super) fn extract_gemini_image_url(part: &Map<String, Value>) -> Option<String> {
    if let Some(inline_data) = part
        .get("inlineData")
        .or_else(|| part.get("inline_data"))
        .and_then(Value::as_object)
    {
        let mime_type = inline_data
            .get("mimeType")
            .or_else(|| inline_data.get("mime_type"))
            .and_then(Value::as_str)?;
        if !mime_type.starts_with("image/") {
            return None;
        }
        let data = inline_data.get("data").and_then(Value::as_str)?;
        return Some(format!("data:{mime_type};base64,{data}"));
    }
    let file_data = part
        .get("fileData")
        .or_else(|| part.get("file_data"))
        .and_then(Value::as_object)?;
    if file_data
        .get("mimeType")
        .or_else(|| file_data.get("mime_type"))
        .and_then(Value::as_str)
        .is_some_and(|mime_type| !mime_type.starts_with("image/"))
    {
        return None;
    }
    file_data
        .get("fileUri")
        .or_else(|| file_data.get("file_uri"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}
