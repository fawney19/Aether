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

fn decode_sse_payloads(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|payload| serde_json::from_str(payload).expect("SSE payload should be JSON"))
        .collect()
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
fn kiro_stream_rewriter_keeps_identical_assistant_deltas() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = [
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "repeat"}),
        ),
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "repeat"}),
        ),
    ]
    .concat();

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let events = decode_sse_payloads(
        &String::from_utf8([first, rest].concat()).expect("utf8 should decode"),
    );
    let text_deltas = events
        .iter()
        .filter_map(|event| {
            (event["delta"]["type"] == "text_delta")
                .then(|| event["delta"]["text"].as_str())
                .flatten()
        })
        .collect::<Vec<_>>();

    assert_eq!(text_deltas, vec!["repeat", "repeat"]);
}

#[test]
fn kiro_stream_rewriter_closes_native_thinking_with_real_signature_before_text() {
    let report_context = kiro_report_context(true);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = [
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "preface text that opens a text block"}),
        ),
        encode_event_frame(
            "event",
            Some("reasoningContentEvent"),
            &json!({"signature": "native-signature"}),
        ),
        encode_event_frame(
            "event",
            Some("reasoningContentEvent"),
            &json!({"text": "native reasoning"}),
        ),
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "final answer"}),
        ),
    ]
    .concat();

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let events = decode_sse_payloads(
        &String::from_utf8([first, rest].concat()).expect("utf8 should decode"),
    );
    let thinking_index = events
        .iter()
        .find(|event| event["content_block"]["type"] == "thinking")
        .and_then(|event| event["index"].as_u64())
        .expect("native reasoning should start a thinking block");
    let thinking_start = events
        .iter()
        .position(|event| {
            event["type"] == "content_block_start"
                && event["index"].as_u64() == Some(thinking_index)
        })
        .expect("thinking block start should exist");
    let signature = events
        .iter()
        .position(|event| {
            event["delta"]["type"] == "signature_delta"
                && event["index"].as_u64() == Some(thinking_index)
                && event["delta"]["signature"] == "native-signature"
        })
        .expect("native signature should be emitted");
    let thinking_stop = events
        .iter()
        .position(|event| {
            event["type"] == "content_block_stop" && event["index"].as_u64() == Some(thinking_index)
        })
        .expect("thinking block should stop");
    let final_text = events
        .iter()
        .position(|event| event["delta"]["text"] == "final answer")
        .expect("final text should be emitted");

    assert!(
        events[..thinking_start].iter().any(|event| {
            event["type"] == "content_block_stop" && event["index"].as_u64() == Some(0)
        }),
        "native thinking must close the existing text block before it starts"
    );
    assert!(
        signature < thinking_stop,
        "signature must precede thinking stop"
    );
    assert!(
        thinking_stop < final_text,
        "thinking must stop before regular assistant text resumes"
    );
}

#[test]
fn kiro_stream_rewriter_keeps_native_signature_for_native_thinking_after_tag_thinking() {
    let report_context = kiro_report_context(true);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = [
        encode_event_frame(
            "event",
            Some("reasoningContentEvent"),
            &json!({"signature": "native-signature"}),
        ),
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "<thinking>legacy</thinking>\n\n"}),
        ),
        encode_event_frame(
            "event",
            Some("reasoningContentEvent"),
            &json!({"text": "native reasoning"}),
        ),
    ]
    .concat();

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let events = decode_sse_payloads(
        &String::from_utf8([first, rest].concat()).expect("utf8 should decode"),
    );
    let thinking_indices = events
        .iter()
        .filter(|event| event["content_block"]["type"] == "thinking")
        .filter_map(|event| event["index"].as_u64())
        .collect::<Vec<_>>();

    assert_eq!(
        thinking_indices.len(),
        2,
        "tag and native thinking each need a block"
    );
    let tag_signature = events.iter().find(|event| {
        event["delta"]["type"] == "signature_delta"
            && event["index"].as_u64() == Some(thinking_indices[0])
    });
    assert!(
        tag_signature
            .and_then(|event| event["delta"]["signature"].as_str())
            .is_some_and(|signature| !signature.is_empty() && signature != "native-signature"),
        "tag thinking needs its own compatibility signature"
    );
    assert!(events.iter().any(|event| {
        event["delta"]["type"] == "signature_delta"
            && event["index"].as_u64() == Some(thinking_indices[1])
            && event["delta"]["signature"] == "native-signature"
    }));
}

#[test]
fn kiro_stream_rewriter_closes_empty_tag_thinking_before_tool_use() {
    let report_context = kiro_report_context(true);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = [
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "<thinking>"}),
        ),
        encode_event_frame(
            "event",
            Some("toolUseEvent"),
            &json!({
                "name": "get_weather",
                "toolUseId": "tool_after_thinking",
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
    let events = decode_sse_payloads(
        &String::from_utf8([first, rest].concat()).expect("utf8 should decode"),
    );
    let thinking_index = events
        .iter()
        .find(|event| event["content_block"]["type"] == "thinking")
        .and_then(|event| event["index"].as_u64())
        .expect("tag thinking should start a block");
    let signature = events
        .iter()
        .position(|event| {
            event["delta"]["type"] == "signature_delta"
                && event["index"].as_u64() == Some(thinking_index)
        })
        .expect("tag thinking needs a compatibility signature");
    let thinking_stop = events
        .iter()
        .position(|event| {
            event["type"] == "content_block_stop" && event["index"].as_u64() == Some(thinking_index)
        })
        .expect("tag thinking should stop");
    let tool_start = events
        .iter()
        .position(|event| event["content_block"]["type"] == "tool_use")
        .expect("tool use should start");

    assert!(
        signature < thinking_stop,
        "signature must precede thinking stop"
    );
    assert!(
        thinking_stop < tool_start,
        "thinking must close before the tool use starts"
    );
}

#[test]
fn kiro_stream_rewriter_downgrades_native_reasoning_to_text_when_thinking_is_disabled() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("reasoningContentEvent"),
        &json!({"text": "visible reasoning", "signature": "ignored"}),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");

    assert!(text.contains("visible reasoning"));
    assert!(!text.contains("signature_delta"));
}

#[test]
fn kiro_stream_rewriter_uses_metadata_stop_reason_and_preserves_credit_metering() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = [
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "Hello"}),
        ),
        encode_event_frame(
            "event",
            Some("metadataEvent"),
            &json!({"stopReason": "MAX_TOKENS"}),
        ),
        encode_event_frame(
            "event",
            Some("meteringEvent"),
            &json!({"unit": "credit", "unitPlural": "credits", "usage": 0.75}),
        ),
    ]
    .concat();

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let events = decode_sse_payloads(
        &String::from_utf8([first, rest].concat()).expect("utf8 should decode"),
    );
    let message_delta = events
        .iter()
        .find(|event| event["type"] == "message_delta")
        .expect("final message delta should exist");
    let usage = &message_delta["usage"];

    assert_eq!(message_delta["delta"]["stop_reason"], "max_tokens");
    assert_eq!(usage["credit_usage"], 0.75);
    assert_eq!(usage["credit_unit"], "credit");
    assert_eq!(usage["credit_unit_plural"], "credits");
    assert_eq!(usage["output_tokens"], 2, "credits are not token counts");
}

#[test]
fn kiro_stream_rewriter_restores_model_directive_display_model() {
    let report_context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "claude:messages",
        "envelope_name": "kiro:generateAssistantResponse",
        "model": "claude-sonnet-4.5-high",
        "mapped_model": "claude-sonnet-4.5"
    });
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let first = rewriter
        .push_chunk(
            &report_context,
            &encode_event_frame(
                "event",
                Some("assistantResponseEvent"),
                &json!({"content": "Hello"}),
            ),
        )
        .expect("rewrite should succeed");
    let text = String::from_utf8(first).expect("utf8 should decode");

    assert!(text.contains("\"model\":\"claude-sonnet-4.5-high\""));
    assert!(!text.contains("\"model\":\"claude-sonnet-4.5\""));
}

#[test]
fn kiro_stream_rewriter_emits_cache_usage_from_report_context() {
    let report_context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "claude:messages",
        "envelope_name": "kiro:generateAssistantResponse",
        "mapped_model": "claude-sonnet-4.5",
        "input_tokens": 100,
        "cache_creation_input_tokens": 25,
        "cache_read_input_tokens": 40
    });
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let first = rewriter
        .push_chunk(
            &report_context,
            &encode_event_frame(
                "event",
                Some("assistantResponseEvent"),
                &json!({"content": "Hello"}),
            ),
        )
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");

    assert_eq!(text.matches("\"input_tokens\":35").count(), 2);
    assert_eq!(
        text.matches("\"cache_creation_input_tokens\":25").count(),
        2
    );
    assert_eq!(text.matches("\"cache_read_input_tokens\":40").count(), 2);
}

#[test]
fn kiro_stream_rewriter_keeps_estimated_input_when_context_usage_is_cache_only() {
    let report_context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "claude:messages",
        "envelope_name": "kiro:generateAssistantResponse",
        "mapped_model": "claude-sonnet-4.5",
        "input_tokens": 24_344,
        "cache_creation_input_tokens": 293,
        "cache_read_input_tokens": 23_935
    });
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = [
        encode_event_frame(
            "event",
            Some("assistantResponseEvent"),
            &json!({"content": "Hello"}),
        ),
        encode_event_frame(
            "event",
            Some("contextUsageEvent"),
            &json!({"contextUsagePercentage": 12.114}),
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

    assert_eq!(text.matches("\"input_tokens\":116").count(), 2);
    assert!(!text.contains("\"input_tokens\":0"));
    assert_eq!(
        text.matches("\"cache_creation_input_tokens\":293").count(),
        2
    );
    assert_eq!(text.matches("\"cache_read_input_tokens\":23935").count(), 2);
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
fn kiro_stream_rewriter_preserves_todowrite_arguments_alias() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("toolUseEvent"),
        &json!({
            "name": "todowrite",
            "toolUseId": "tool_todos_123",
            "arguments": {
                "todos": [{
                    "content": "Implement Kiro compatibility",
                    "status": "in_progress",
                    "priority": "high"
                }]
            },
            "stop": true
        }),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");

    assert!(text.contains("\"name\":\"todowrite\""));
    assert!(text.contains(
        "\"partial_json\":\"{\\\"todos\\\":[{\\\"content\\\":\\\"Implement Kiro compatibility\\\""
    ));
}

#[test]
fn kiro_stream_rewriter_prefers_arguments_over_empty_input_placeholder() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("toolUseEvent"),
        &json!({
            "name": "read",
            "toolUseId": "tool_read_123",
            "input": "",
            "arguments": {"filePath": "/workspace/Cargo.toml"},
            "stop": true
        }),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");

    assert!(text.contains("\"name\":\"read\""));
    assert!(text.contains("\"partial_json\":\"{\\\"file_path\\\":\\\"/workspace/Cargo.toml\\\"}\""));
}

#[test]
fn kiro_stream_rewriter_reads_nested_tool_use_identity_and_arguments() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("toolUseEvent"),
        &json!({
            "toolUse": {
                "name": "Read",
                "toolUseId": "tool_nested_read",
                "arguments": {"filePath": "/workspace/Cargo.toml"},
                "stop": true
            }
        }),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");

    assert!(text.contains("\"id\":\"tool_nested_read\""));
    assert!(text.contains("\"name\":\"Read\""));
    assert!(text.contains("\"partial_json\":\"{\\\"file_path\\\":\\\"/workspace/Cargo.toml\\\"}\""));
}

#[test]
fn kiro_stream_rewriter_only_rewrites_complete_read_tool_json() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("toolUseEvent"),
        &json!({
            "name": "read",
            "toolUseId": "tool_read_fragment",
            "input": "{\"filePath\":\"/workspace/Cargo",
            "stop": true
        }),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");

    assert!(text.contains("filePath"));
    assert!(!text.contains("file_path"));
}

#[test]
fn kiro_stream_rewriter_restores_native_read_file_path_after_complete_json() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("toolUseEvent"),
        &json!({
            "name": "read_file",
            "toolUseId": "tool_native_read_file",
            "input": {"path": "/workspace/Cargo.toml"},
            "stop": true
        }),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");

    assert!(text.contains("\"partial_json\":\"{\\\"file_path\\\":\\\"/workspace/Cargo.toml\\\"}"));
    assert!(!text.contains("\\\"path\\\""));
}

#[test]
fn kiro_stream_rewriter_keeps_fragmented_native_read_file_json_unchanged() {
    let report_context = kiro_report_context(false);
    let mut rewriter = KiroToClaudeCliStreamState::new(&report_context);
    let chunk = encode_event_frame(
        "event",
        Some("toolUseEvent"),
        &json!({
            "name": "readFile",
            "toolUseId": "tool_native_read_file_fragment",
            "input": "{\"path\":\"/workspace/Cargo",
            "stop": true
        }),
    );

    let first = rewriter
        .push_chunk(&report_context, &chunk)
        .expect("rewrite should succeed");
    let rest = rewriter
        .finish(&report_context)
        .expect("finish should succeed");
    let text = String::from_utf8([first, rest].concat()).expect("utf8 should decode");

    assert!(text.contains("\\\"path\\\""));
    assert!(!text.contains("file_path"));
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
