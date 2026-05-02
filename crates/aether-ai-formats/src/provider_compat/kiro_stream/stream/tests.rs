use crate::provider_compat::kiro_stream::kiro_crc32 as crc32;
use serde_json::{json, Value};

use super::KiroToClaudeCliStreamState;

fn encode_string_header(name: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(7);
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    out
}

fn encode_event_frame(message_type: &str, event_type: Option<&str>, payload: &Value) -> Vec<u8> {
    let mut headers = encode_string_header(":message-type", message_type);
    if let Some(event_type) = event_type {
        headers.extend_from_slice(&encode_string_header(":event-type", event_type));
    }
    let payload_bytes = serde_json::to_vec(payload).expect("payload should encode");
    encode_frame(headers, payload_bytes)
}

fn encode_frame(headers: Vec<u8>, payload: Vec<u8>) -> Vec<u8> {
    let total_len = 12 + headers.len() + payload.len() + 4;
    let header_len = headers.len();
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&(total_len as u32).to_be_bytes());
    out.extend_from_slice(&(header_len as u32).to_be_bytes());
    let prelude_crc = crc32(&out[..8]);
    out.extend_from_slice(&prelude_crc.to_be_bytes());
    out.extend_from_slice(&headers);
    out.extend_from_slice(&payload);
    let message_crc = crc32(&out);
    out.extend_from_slice(&message_crc.to_be_bytes());
    out
}

fn kiro_report_context(thinking_enabled: bool) -> Value {
    let mut context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "claude:messages",
        "envelope_name": "kiro:generateAssistantResponse",
        "mapped_model": "claude-sonnet-4.5"
    });
    if thinking_enabled {
        context["original_request_body"] = json!({
            "thinking": {
                "type": "enabled"
            }
        });
    }
    context
}

#[test]
fn kiro_stream_rewriter_converts_text_events_to_claude_sse() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = [
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "Hello from Kiro"}),
        ),
        encode_event_frame(
            "event",
            Some("contextUsageEvent"),
            &json!({"contextUsagePercentage": 1.0}),
        ),
    ]
    .concat();

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");
    assert!(text.contains("event: message_start"));
    assert!(text.contains("\"type\":\"content_block_delta\""));
    assert!(text.contains("Hello from Kiro"));
    assert!(text.contains("\"stop_reason\":\"end_turn\""));
    assert!(text.contains("\"input_tokens\":2000"));
}

#[test]
fn kiro_stream_rewriter_converts_tool_use_to_claude_events() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = [
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "Need a tool."}),
        ),
        encode_event_frame(
            "event",
            Some("toolUseEvent"),
            &json!({
                "name": "get_weather",
                "toolUseId": "tool_123",
                "input": {"city": "SF"},
                "stop": true
            }),
        ),
    ]
    .concat();

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");
    assert!(text.contains("\"type\":\"tool_use\""));
    assert!(text.contains("\"id\":\"tool_123\""));
    assert!(text.contains("\"name\":\"get_weather\""));
    assert!(text.contains("\"partial_json\":\"{\\\"city\\\":\\\"SF\\\"}\""));
    assert!(text.contains("\"stop_reason\":\"tool_use\""));
}

#[test]
fn kiro_stream_rewriter_handles_multibyte_text_without_thinking_tag() {
    let report_context = kiro_report_context(true);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("assistantResponseEvent"),
        &json!({"content": "\n\n你好！有"}),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");
    assert!(text.contains("\"type\":\"text_delta\""));
    assert!(text.contains("你好！有"));
}

#[test]
fn kiro_stream_rewriter_handles_multibyte_text_inside_thinking_block() {
    let report_context = kiro_report_context(true);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("assistantResponseEvent"),
        &json!({"content": "<thinking>\n\n你好！有"}),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");
    assert!(text.contains("\"type\":\"thinking_delta\""));
    assert!(text.contains("你好！有"));
}
