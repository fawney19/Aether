use serde_json::Value;

use crate::formats::shared::AiSurfaceFinalizeError;

pub fn map_claude_stop_reason(stop_reason: Option<&str>, has_tool_calls: bool) -> Option<String> {
    let mapped = match stop_reason {
        Some("end_turn") | Some("stop_sequence") => Some("stop".to_string()),
        Some("max_tokens") => Some("length".to_string()),
        Some("tool_use") => Some("tool_calls".to_string()),
        Some("pause_turn") => Some("stop".to_string()),
        Some(other) if !other.trim().is_empty() => Some(other.to_string()),
        _ => None,
    };
    if has_tool_calls && mapped.as_deref().is_none_or(|value| value == "stop") {
        Some("tool_calls".to_string())
    } else {
        mapped
    }
}

pub fn encode_done_sse() -> Vec<u8> {
    b"data: [DONE]\n\n".to_vec()
}

pub fn encode_json_sse(
    event: Option<&str>,
    value: &Value,
) -> Result<Vec<u8>, AiSurfaceFinalizeError> {
    let mut out = Vec::new();
    if let Some(event) = event.filter(|value| !value.trim().is_empty()) {
        out.extend_from_slice(b"event: ");
        out.extend_from_slice(event.as_bytes());
        out.push(b'\n');
    }
    out.extend_from_slice(b"data: ");
    out.extend(serde_json::to_vec(value).map_err(AiSurfaceFinalizeError::from)?);
    out.extend_from_slice(b"\n\n");
    Ok(out)
}
