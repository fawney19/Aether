use serde_json::Value;

use crate::response::AiSurfaceFinalizeError;

pub fn map_claude_stop_reason(
    stop_reason: Option<&str>,
    has_tool_calls: bool,
) -> Option<&'static str> {
    let mapped = match stop_reason {
        Some("end_turn") | Some("stop_sequence") => Some("stop"),
        Some("max_tokens") => Some("length"),
        Some("tool_use") => Some("tool_calls"),
        Some("pause_turn") => Some("stop"),
        _ => None,
    };
    if has_tool_calls && mapped.is_none_or(|value| value == "stop") {
        Some("tool_calls")
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
