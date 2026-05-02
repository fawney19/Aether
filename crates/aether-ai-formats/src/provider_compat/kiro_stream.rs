use serde_json::{json, Value};

pub use self::state::KiroToClaudeCliStreamState;

mod state;

pub const KIRO_CONTEXT_WINDOW_TOKENS: f64 = 200_000.0;
pub const KIRO_MAX_THINKING_BUFFER: usize = 1024 * 1024;

const KIRO_QUOTE_CHARS: &str = "`\"'\\#!@$%^&*()-_=+[]{};:<>,.?/";

pub fn encode_kiro_sse_events(events: Vec<Value>) -> Result<Vec<u8>, serde_json::Error> {
    let mut output = Vec::new();
    for event in events {
        output.extend(encode_kiro_sse_event(&event)?);
    }
    Ok(output)
}

pub fn encode_kiro_sse_event(event: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let encoded = serde_json::to_string(event)?;
    if let Some(event_type) = event.get("type").and_then(Value::as_str) {
        Ok(format!("event: {event_type}\ndata: {encoded}\n\n").into_bytes())
    } else {
        Ok(format!("data: {encoded}\n\n").into_bytes())
    }
}

pub fn build_kiro_initial_sse_events(
    message_id: &str,
    model: &str,
    estimated_input_tokens: usize,
) -> Vec<Value> {
    vec![json!({
        "type": "message_start",
        "message": {
            "id": message_id,
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": model,
            "stop_reason": Value::Null,
            "stop_sequence": Value::Null,
            "usage": {
                "input_tokens": estimated_input_tokens as u64,
                "output_tokens": 1,
            },
        }
    })]
}

pub fn build_kiro_stream_error_sse_events(error_type: &str, message: &str) -> Vec<Value> {
    vec![json!({
        "type": "error",
        "error": {
            "type": error_type,
            "message": message,
        }
    })]
}

pub fn build_kiro_final_message_sse_events(
    stop_reason: &str,
    input_tokens: usize,
    output_tokens: usize,
) -> Vec<Value> {
    vec![
        json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": stop_reason,
                "stop_sequence": Value::Null,
            },
            "usage": {
                "input_tokens": input_tokens as u64,
                "output_tokens": output_tokens as u64,
            }
        }),
        json!({"type": "message_stop"}),
    ]
}

pub fn calculate_kiro_context_input_tokens(percentage: f64) -> usize {
    ((percentage * KIRO_CONTEXT_WINDOW_TOKENS) / 100.0) as usize
}

pub fn estimate_kiro_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut chinese = 0usize;
    let mut other = 0usize;
    for ch in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            chinese += 1;
        } else {
            other += 1;
        }
    }
    let chinese_tokens = (chinese * 2).div_ceil(3);
    let other_tokens = other.div_ceil(4);
    (chinese_tokens + other_tokens).max(1)
}

pub fn find_kiro_real_thinking_start_tag(buffer: &str) -> Option<usize> {
    let tag = "<thinking>";
    let mut search = 0usize;
    loop {
        let pos = buffer[search..].find(tag).map(|value| value + search)?;
        let has_before = pos > 0 && is_kiro_quote_char(buffer, pos - 1);
        let after_pos = pos + tag.len();
        let has_after = is_kiro_quote_char(buffer, after_pos);
        if !has_before && !has_after {
            return Some(pos);
        }
        search = pos + 1;
    }
}

pub fn find_kiro_real_thinking_end_tag(buffer: &str) -> Option<usize> {
    let tag = "</thinking>";
    let mut search = 0usize;
    loop {
        let pos = buffer[search..].find(tag).map(|value| value + search)?;
        let has_before = pos > 0 && is_kiro_quote_char(buffer, pos - 1);
        let after_pos = pos + tag.len();
        let has_after = is_kiro_quote_char(buffer, after_pos);
        if has_before || has_after {
            search = pos + 1;
            continue;
        }
        let after = &buffer[after_pos..];
        if after.len() < 2 {
            return None;
        }
        if after.starts_with("\n\n") {
            return Some(pos);
        }
        search = pos + 1;
    }
}

pub fn find_kiro_real_thinking_end_tag_at_buffer_end(buffer: &str) -> Option<usize> {
    let tag = "</thinking>";
    let mut search = 0usize;
    loop {
        let pos = buffer[search..].find(tag).map(|value| value + search)?;
        let has_before = pos > 0 && is_kiro_quote_char(buffer, pos - 1);
        let after_pos = pos + tag.len();
        let has_after = is_kiro_quote_char(buffer, after_pos);
        if has_before || has_after {
            search = pos + 1;
            continue;
        }
        if buffer[after_pos..].trim().is_empty() {
            return Some(pos);
        }
        search = pos + 1;
    }
}

pub fn kiro_crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

fn is_kiro_quote_char(buffer: &str, pos: usize) -> bool {
    buffer
        .as_bytes()
        .get(pos)
        .map(|byte| KIRO_QUOTE_CHARS.as_bytes().contains(byte))
        .unwrap_or(false)
}
