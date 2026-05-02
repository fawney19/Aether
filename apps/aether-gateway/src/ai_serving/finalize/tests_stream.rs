use serde_json::json;

use crate::ai_serving::maybe_bridge_standard_sync_json_to_stream;

use super::maybe_build_local_stream_rewriter;

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("utf8 should decode")
}

#[test]
fn standard_sync_bridge_converts_openai_chat_sync_json_to_openai_chat_sse() {
    let outcome = maybe_bridge_standard_sync_json_to_stream(
        &json!({
            "id": "chatcmpl_sync_123",
            "object": "chat.completion",
            "model": "gpt-5.4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello from sync bridge"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 2,
                "total_tokens": 3
            }
        }),
        "openai:chat",
        "openai:chat",
        None,
    )
    .expect("bridge should succeed")
    .expect("bridge should produce sse");

    let output_text = utf8(outcome.sse_body);
    assert!(output_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(output_text.contains("\"role\":\"assistant\""));
    assert!(output_text.contains("\"content\":\"Hello from sync bridge\""));
    assert!(output_text.contains("\"finish_reason\":\"stop\""));
    assert!(output_text.contains("data: [DONE]"));
    let summary = outcome
        .terminal_summary
        .expect("terminal summary should exist");
    assert_eq!(summary.response_id.as_deref(), Some("resp_sync_123"));
    assert_eq!(summary.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(summary.finish_reason.as_deref(), Some("stop"));
    assert_eq!(
        summary
            .standardized_usage
            .as_ref()
            .and_then(|usage| usage.dimensions.get("total_tokens"))
            .cloned(),
        Some(json!(3))
    );
}

#[test]
fn standard_sync_bridge_converts_claude_sync_json_to_openai_responses_sse() {
    let report_context = json!({
        "mapped_model": "claude-sonnet-4-5",
    });
    let outcome = maybe_bridge_standard_sync_json_to_stream(
        &json!({
            "id": "msg_sync_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{
                "type": "text",
                "text": "Hello from Claude sync"
            }],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 2,
                "output_tokens": 3
            }
        }),
        "claude:messages",
        "openai:responses",
        Some(&report_context),
    )
    .expect("bridge should succeed")
    .expect("bridge should produce sse");

    let output_text = utf8(outcome.sse_body);
    assert!(output_text.contains("event: response.created"));
    assert!(output_text.contains("event: response.output_text.delta"));
    assert!(output_text.contains("event: response.completed"));
    assert!(output_text.contains("\"text\":\"Hello from Claude sync\""));
    let summary = outcome
        .terminal_summary
        .expect("terminal summary should exist");
    assert_eq!(summary.response_id.as_deref(), Some("msg_sync_123"));
    assert_eq!(summary.model.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(summary.finish_reason.as_deref(), Some("stop"));
    assert_eq!(
        summary
            .standardized_usage
            .as_ref()
            .and_then(|usage| usage.dimensions.get("total_tokens"))
            .cloned(),
        Some(json!(5))
    );
}

#[test]
fn antigravity_stream_rewriter_unwraps_and_injects_tool_ids() {
    let report_context = json!({
        "has_envelope": true,
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "gemini:generate_content",
        "envelope_name": "antigravity:v1internal",
        "needs_conversion": false,
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"SF\"}}}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"claude-sonnet-4-5\"},\"responseId\":\"resp_123\"}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = String::from_utf8(output).expect("text should be utf8");
    assert!(output_text.contains("\"_v1internal_response_id\":\"resp_123\""));
    assert!(output_text.contains("\"id\":\"call_get_weather_0\""));
    assert!(output_text.contains("\"modelVersion\":\"claude-sonnet-4-5\""));
}

#[test]
fn openai_image_stream_rewriter_emits_completed_event_for_generate() {
    let report_context = json!({
        "provider_api_format": "openai:image",
        "client_api_format": "openai:image",
        "needs_conversion": false,
        "image_request": {
            "operation": "generate"
        }
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");

    let first = rewriter
        .push_chunk(
            concat!(
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"result\":\"aGVsbG8=\"}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    assert!(first.is_empty());

    let second = rewriter
        .push_chunk(
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"tool_usage\":{\"image_gen\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(second);
    assert!(output_text.contains("event: image_generation.completed"));
    assert!(output_text.contains("\"type\":\"image_generation.completed\""));
    assert!(output_text.contains("\"b64_json\":\"aGVsbG8=\""));
    assert!(output_text.contains("\"input_tokens\":1"));
    assert!(!output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_image_stream_rewriter_maps_responses_partial_image_events() {
    let report_context = json!({
        "provider_api_format": "openai:image",
        "client_api_format": "openai:image",
        "needs_conversion": false,
        "image_request": {
            "operation": "generate",
            "partial_images": 1
        }
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");

    let partial = rewriter
        .push_chunk(
            concat!(
                "event: response.image_generation_call.partial_image\n",
                "data: {\"type\":\"response.image_generation_call.partial_image\",\"partial_image_index\":0,\"partial_image_b64\":\"cGFydGlhbA==\"}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let partial_text = utf8(partial);
    assert!(partial_text.contains("event: image_generation.partial_image"));
    assert!(partial_text.contains("\"type\":\"image_generation.partial_image\""));
    assert!(partial_text.contains("\"b64_json\":\"cGFydGlhbA==\""));
    assert!(partial_text.contains("\"partial_image_index\":0"));
    assert!(!partial_text.contains("response.image_generation_call.partial_image"));

    let done = rewriter
        .push_chunk(
            concat!(
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"result\":\"ZmluYWw=\"}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    assert!(done.is_empty());

    let completed = rewriter
        .push_chunk(
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":5,\"total_tokens\":9}}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let completed_text = utf8(completed);
    assert!(completed_text.contains("event: image_generation.completed"));
    assert!(completed_text.contains("\"type\":\"image_generation.completed\""));
    assert!(completed_text.contains("\"b64_json\":\"ZmluYWw=\""));
    assert!(completed_text.contains("\"total_tokens\":9"));
}

#[test]
fn openai_image_stream_rewriter_reads_final_image_from_completed_response_output() {
    let report_context = json!({
        "provider_api_format": "openai:image",
        "client_api_format": "openai:image",
        "needs_conversion": false,
        "image_request": {
            "operation": "generate"
        }
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");

    let completed = rewriter
        .push_chunk(
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\"},{\"type\":\"image_generation_call\",\"result\":\"ZnJvbV9vdXRwdXQ=\"}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let completed_text = utf8(completed);
    assert!(completed_text.contains("event: image_generation.completed"));
    assert!(completed_text.contains("\"b64_json\":\"ZnJvbV9vdXRwdXQ=\""));
    assert!(completed_text.contains("\"total_tokens\":3"));
}

#[test]
fn openai_image_stream_rewriter_maps_upstream_error_to_generation_failed() {
    let report_context = json!({
        "provider_api_format": "openai:image",
        "client_api_format": "openai:image",
        "needs_conversion": false,
        "image_request": {
            "operation": "generate"
        }
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");

    let output = rewriter
        .push_chunk(
            concat!(
                "event: error\n",
                "data: {\"type\":\"error\",\"error\":{\"type\":\"input-images\",\"code\":\"rate_limit_exceeded\",\"message\":\"Rate limit reached for gpt-image-2\",\"param\":null}}\n\n",
                "event: response.failed\n",
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"Rate limit reached for gpt-image-2\"}}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: image_generation.failed"));
    assert_eq!(
        output_text
            .matches("event: image_generation.failed")
            .count(),
        1
    );
    assert!(output_text.contains("\"type\":\"image_generation.failed\""));
    assert!(output_text.contains("\"type\":\"input-images\""));
    assert!(output_text.contains("\"code\":\"rate_limit_exceeded\""));
    assert!(output_text.contains("\"message\":\"Rate limit reached for gpt-image-2\""));
    assert!(!output_text.contains("response.failed"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_image_stream_rewriter_maps_response_failed_to_edit_failed() {
    let report_context = json!({
        "provider_api_format": "openai:image",
        "client_api_format": "openai:image",
        "needs_conversion": false,
        "image_request": {
            "operation": "edit"
        }
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");

    let output = rewriter
        .push_chunk(
            concat!(
                "event: response.failed\n",
                "data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"rate_limit_exceeded\",\"message\":\"slow down\"}}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: image_edit.failed"));
    assert!(output_text.contains("\"type\":\"image_edit.failed\""));
    assert!(output_text.contains("\"type\":\"rate_limit_exceeded\""));
    assert!(output_text.contains("\"code\":\"rate_limit_exceeded\""));
    assert!(output_text.contains("\"message\":\"slow down\""));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_image_stream_rewriter_emits_partial_and_completed_events_for_edit() {
    let report_context = json!({
        "provider_api_format": "openai:image",
        "client_api_format": "openai:image",
        "needs_conversion": false,
        "image_request": {
            "operation": "edit",
            "partial_images": 2
        }
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");

    let partial = rewriter
        .push_chunk(
            concat!(
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"id\":\"ig_edit_123\",\"type\":\"image_generation_call\",\"result\":\"d29ybGQ=\"}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let partial_text = utf8(partial);
    assert!(partial_text.contains("event: image_edit.partial_image"));
    assert!(partial_text.contains("\"type\":\"image_edit.partial_image\""));
    assert!(partial_text.contains("\"b64_json\":\"d29ybGQ=\""));
    assert!(partial_text.contains("\"partial_image_index\":1"));

    let completed = rewriter
        .push_chunk(
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":5,\"total_tokens\":9}}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let completed_text = utf8(completed);
    assert!(completed_text.contains("event: image_edit.completed"));
    assert!(completed_text.contains("\"type\":\"image_edit.completed\""));
    assert!(completed_text.contains("\"b64_json\":\"d29ybGQ=\""));
    assert!(completed_text.contains("\"total_tokens\":9"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn gemini_cli_v1internal_stream_rewriter_unwraps_response_object() {
    let report_context = json!({
        "has_envelope": true,
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "gemini:generate_content",
        "envelope_name": "gemini_cli:v1internal",
        "needs_conversion": false,
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello Gemini CLI\"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-cli-2.5\"}}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = String::from_utf8(output).expect("text should be utf8");
    assert_eq!(
        output_text,
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello Gemini CLI\"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-cli-2.5\"}\n\n"
    );
}

#[test]
fn openai_chat_error_to_openai_responses_stream_rewriter_converts_to_response_failed() {
    let report_context = json!({
        "provider_api_format": "openai:chat",
        "client_api_format": "openai:responses",
        "needs_conversion": true,
        "mapped_model": "gpt-5.4",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"error\":{\"message\":\"bad request\",\"type\":\"invalid_request_error\",\"code\":\"invalid_request\"}}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: response.failed"));
    assert!(output_text.contains("\"sequence_number\":1"));
    assert!(output_text.contains("\"message\":\"bad request\""));
    assert!(output_text.contains("\"type\":\"invalid_request_error\""));
    assert!(!output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn claude_error_to_openai_responses_stream_rewriter_converts_to_response_failed() {
    let report_context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "openai:responses",
        "needs_conversion": true,
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            concat!(
                "event: error\n",
                "data: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"slow down\",\"code\":\"rate_limit\"}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: response.failed"));
    assert!(output_text.contains("\"sequence_number\":1"));
    assert!(output_text.contains("\"message\":\"slow down\""));
    assert!(output_text.contains("\"type\":\"rate_limit_error\""));
    assert!(!output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn gemini_error_to_openai_responses_stream_rewriter_converts_to_response_failed() {
    let report_context = json!({
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "openai:responses",
        "needs_conversion": true,
        "mapped_model": "gemini-2.5-pro",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"error\":{\"code\":429,\"message\":\"quota exceeded\",\"status\":\"RESOURCE_EXHAUSTED\"}}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: response.failed"));
    assert!(output_text.contains("\"sequence_number\":1"));
    assert!(output_text.contains("\"message\":\"quota exceeded\""));
    assert!(output_text.contains("\"type\":\"rate_limit_error\""));
    assert!(!output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_chat_error_to_claude_chat_stream_rewriter_uses_error_event_line() {
    let report_context = json!({
        "provider_api_format": "openai:chat",
        "client_api_format": "claude:messages",
        "needs_conversion": true,
        "mapped_model": "gpt-5.4",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"error\":{\"message\":\"bad request\",\"type\":\"invalid_request_error\",\"code\":\"invalid_request\"}}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: error"));
    assert!(output_text.contains("\"type\":\"error\""));
    assert!(output_text.contains("\"message\":\"bad request\""));
    assert!(output_text.contains("\"code\":\"invalid_request\""));
    assert!(!output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_chat_error_to_gemini_chat_stream_rewriter_keeps_data_only_error() {
    let report_context = json!({
        "provider_api_format": "openai:chat",
        "client_api_format": "gemini:generate_content",
        "needs_conversion": true,
        "mapped_model": "gpt-5.4",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"error\":{\"message\":\"rate limited\",\"type\":\"rate_limit_error\",\"code\":\"rate_limit\"}}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.starts_with("data: {\"error\":"));
    assert!(!output_text.contains("event: "));
    assert!(output_text.contains("\"message\":\"rate limited\""));
    assert!(output_text.contains("\"status\":\"RESOURCE_EXHAUSTED\""));
    assert!(!output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn claude_to_openai_chat_stream_rewriter_converts_text_deltas() {
    let report_context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "openai:chat",
        "needs_conversion": true,
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"model\":\"claude-sonnet-4-5\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = String::from_utf8(output).expect("utf8 should decode");
    assert!(output_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(output_text.contains("\"role\":\"assistant\""));
    assert!(output_text.contains("\"content\":\"Hello\""));
    assert!(output_text.contains("\"finish_reason\":\"stop\""));
    assert!(output_text.contains("data: [DONE]"));
}

#[test]
fn claude_to_openai_chat_stream_rewriter_converts_tool_use_to_tool_calls() {
    let report_context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "openai:chat",
        "needs_conversion": true,
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_tool_claude_chat_stream_123\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Need a tool.\"}}\n\n",
                "event: content_block_stop\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_123\",\"name\":\"get_weather\",\"input\":{\"location\":\"Tokyo\"}}}\n\n",
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = String::from_utf8(output).expect("utf8 should decode");
    assert!(output_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(output_text.contains("\"role\":\"assistant\""));
    assert!(output_text.contains("\"tool_calls\":[{"));
    assert!(output_text.contains("\"id\":\"tool_123\""));
    assert!(output_text.contains("\"name\":\"get_weather\""));
    assert!(output_text.contains("\\\"location\\\":\\\"Tokyo\\\""));
    assert!(output_text.contains("\"finish_reason\":\"tool_calls\""));
    assert!(output_text.contains("data: [DONE]"));
}

#[test]
fn gemini_to_openai_chat_stream_rewriter_buffers_and_converts_text() {
    let report_context = json!({
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "openai:chat",
        "needs_conversion": true,
        "mapped_model": "gemini-2.5-pro",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let first = rewriter
        .push_chunk(
            b"data: {\"responseId\":\"resp_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-2.5-pro\"}\n\n",
        )
        .expect("rewrite should succeed");
    let first_text = utf8(first);
    assert!(first_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(first_text.contains("\"role\":\"assistant\""));
    assert!(first_text.contains("\"content\":\"Hello \""));
    assert!(!first_text.contains("data: [DONE]"));
    let second = rewriter
        .push_chunk(
            b"data: {\"responseId\":\"resp_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Gemini\"}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro\"}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(second);
    assert!(output_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(output_text.contains("\"content\":\"Gemini\""));
    assert!(output_text.contains("\"finish_reason\":\"stop\""));
    assert!(output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn gemini_to_openai_chat_stream_rewriter_buffers_and_converts_function_call() {
    let report_context = json!({
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "openai:chat",
        "needs_conversion": true,
        "mapped_model": "gemini-2.5-pro",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"responseId\":\"resp_tool_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Need a tool.\"},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"SF\"}}}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro\"}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(output_text.contains("\"role\":\"assistant\""));
    assert!(output_text.contains("\"content\":\"Need a tool.\""));
    assert!(output_text.contains("\"tool_calls\":[{"));
    assert!(output_text.contains("\"name\":\"get_weather\""));
    assert!(output_text.contains("\\\"city\\\":\\\"SF\\\""));
    assert!(output_text.contains("\"finish_reason\":\"tool_calls\""));
    assert!(output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_responses_to_openai_chat_stream_rewriter_converts_text_deltas_immediately() {
    let report_context = json!({
        "provider_api_format": "openai:responses",
        "client_api_format": "openai:chat",
        "needs_conversion": true,
        "mapped_model": "gpt-5.4",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let created = rewriter
        .push_chunk(
            concat!(
                "event: response.created\n",
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_cli_stream_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"in_progress\"}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let created_text = String::from_utf8(created).expect("utf8 should decode");
    assert!(created_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(created_text.contains("\"role\":\"assistant\""));
    assert!(!created_text.contains("data: [DONE]"));

    let delta = rewriter
        .push_chunk(
            concat!(
                "event: response.output_text.delta\n",
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello Codex\"}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let delta_text = String::from_utf8(delta).expect("utf8 should decode");
    assert!(delta_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(delta_text.contains("\"content\":\"Hello Codex\""));
    assert!(!delta_text.contains("data: [DONE]"));

    let completed = rewriter
        .push_chunk(
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_cli_stream_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_cli_stream_123\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello Codex\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let completed_text = String::from_utf8(completed).expect("utf8 should decode");
    assert!(completed_text.contains("\"finish_reason\":\"stop\""));
    assert!(
        completed_text.contains("\"choices\":[]"),
        "{completed_text}"
    );
    assert!(
        completed_text.contains("\"prompt_tokens\":1"),
        "{completed_text}"
    );
    assert!(
        completed_text.contains("\"completion_tokens\":2"),
        "{completed_text}"
    );
    assert!(
        completed_text.contains("\"total_tokens\":3"),
        "{completed_text}"
    );
    assert!(completed_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_responses_to_openai_chat_stream_rewriter_converts_completed_event_without_buffering() {
    let report_context = json!({
        "provider_api_format": "openai:responses",
        "client_api_format": "openai:chat",
        "needs_conversion": true,
        "mapped_model": "gpt-5.4",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            concat!(
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_cli_stream_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_cli_stream_123\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello Codex\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = String::from_utf8(output).expect("utf8 should decode");
    assert!(output_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(output_text.contains("\"role\":\"assistant\""));
    assert!(output_text.contains("\"content\":\"Hello Codex\""));
    assert!(output_text.contains("\"finish_reason\":\"stop\""));
    assert!(output_text.contains("\"choices\":[]"), "{output_text}");
    assert!(output_text.contains("\"prompt_tokens\":1"), "{output_text}");
    assert!(
        output_text.contains("\"completion_tokens\":2"),
        "{output_text}"
    );
    assert!(output_text.contains("\"total_tokens\":3"), "{output_text}");
    assert!(output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn antigravity_gemini_to_openai_chat_stream_rewriter_unwraps_and_converts_function_call() {
    let report_context = json!({
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "openai:chat",
        "needs_conversion": true,
        "has_envelope": true,
        "envelope_name": "antigravity:v1internal",
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"response\":{\"responseId\":\"resp_antigravity_chat_tool_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Need a tool.\"},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"SF\"}}}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"claude-sonnet-4-5\"},\"responseId\":\"resp_antigravity_chat_tool_123\"}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("\"object\":\"chat.completion.chunk\""));
    assert!(output_text.contains("\"content\":\"Need a tool.\""));
    assert!(output_text.contains("\"tool_calls\""));
    assert!(output_text.contains("\"name\":\"get_weather\""));
    assert!(output_text.contains("\"finish_reason\":\"tool_calls\""));
    assert!(output_text.contains("data: [DONE]"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn antigravity_gemini_to_openai_responses_stream_rewriter_unwraps_and_converts_function_call() {
    let report_context = json!({
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "openai:responses",
        "needs_conversion": true,
        "envelope_name": "antigravity:v1internal",
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"response\":{\"responseId\":\"resp_antigravity_cli_tool_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Need a tool.\"},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"city\":\"SF\"}}}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"claude-sonnet-4-5\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}},\"responseId\":\"resp_antigravity_cli_tool_123\"}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: response.created"));
    assert!(output_text.contains("event: response.output_text.delta"));
    assert!(output_text.contains("event: response.output_item.added"));
    assert!(output_text.contains("event: response.function_call_arguments.delta"));
    assert!(output_text.contains("event: response.completed"));
    assert!(output_text.contains("\"type\":\"function_call\""));
    assert!(output_text.contains("\"name\":\"get_weather\""));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn gemini_to_openai_responses_stream_rewriter_buffers_and_converts_to_completed_event() {
    let report_context = json!({
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "openai:responses",
        "needs_conversion": true,
        "mapped_model": "gemini-2.5-pro",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let first = rewriter
        .push_chunk(
            b"data: {\"responseId\":\"resp_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-2.5-pro\"}\n\n",
        )
        .expect("rewrite should succeed");
    let first_text = utf8(first);
    assert!(first_text.contains("event: response.created"));
    assert!(first_text.contains("event: response.output_text.delta"));
    assert!(first_text.contains("\"delta\":\"Hello \""));
    let second = rewriter
        .push_chunk(
            b"data: {\"responseId\":\"resp_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Gemini CLI\"}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(second);
    assert!(output_text.contains("event: response.output_text.delta"));
    assert!(output_text.contains("event: response.completed"));
    assert!(output_text.contains("\"type\":\"response.completed\""));
    assert!(output_text.contains("\"object\":\"response\""));
    assert!(output_text.contains("\"delta\":\"Gemini CLI\""));
    assert!(output_text.contains("\"text\":\"Hello Gemini CLI\""));
    assert!(output_text.contains("\"total_tokens\":5"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn claude_to_openai_responses_stream_rewriter_buffers_and_converts_to_completed_event() {
    let report_context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "openai:responses",
        "needs_conversion": true,
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_123\",\"model\":\"claude-sonnet-4-5\"}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello Claude CLI\"}}\n\n",
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":2,\"output_tokens\":3}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: response.created"));
    assert!(output_text.contains("event: response.output_text.delta"));
    assert!(output_text.contains("event: response.completed"));
    assert!(output_text.contains("\"type\":\"response.completed\""));
    assert!(output_text.contains("\"object\":\"response\""));
    assert!(output_text.contains("\"text\":\"Hello Claude CLI\""));
    assert!(output_text.contains("\"total_tokens\":5"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn claude_to_openai_responses_stream_rewriter_converts_tool_use_to_function_call() {
    let report_context = json!({
        "provider_api_format": "claude:messages",
        "client_api_format": "openai:responses",
        "needs_conversion": true,
        "mapped_model": "claude-sonnet-4-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_tool_123\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"Running tool.\"}}\n\n",
                "event: content_block_stop\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_123\",\"name\":\"read_file\",\"input\":{\"path\":\"/tmp/test.txt\"}}}\n\n",
                "event: content_block_stop\n",
                "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"input_tokens\":4,\"output_tokens\":6}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: response.created"));
    assert!(output_text.contains("event: response.output_text.delta"));
    assert!(output_text.contains("\"delta\":\"Running tool.\""));
    assert!(output_text.contains("event: response.output_item.added"));
    assert!(output_text.contains("event: response.function_call_arguments.delta"));
    assert!(output_text.contains("event: response.completed"));
    assert!(output_text.contains("\"type\":\"response.completed\""));
    assert!(output_text.contains("\"type\":\"function_call\""));
    assert!(output_text.contains("\"call_id\":\"tool_123\""));
    assert!(output_text.contains("\"name\":\"read_file\""));
    assert!(output_text.contains("\\\"path\\\":\\\"/tmp/test.txt\\\""));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn gemini_to_openai_responses_stream_rewriter_converts_function_call_to_completed_event() {
    let report_context = json!({
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "openai:responses",
        "needs_conversion": true,
        "mapped_model": "gemini-2.5-pro",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"responseId\":\"resp_tool_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Need a tool.\"},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"location\":\"Tokyo\"}}}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: response.created"));
    assert!(output_text.contains("event: response.output_item.added"));
    assert!(output_text.contains("event: response.function_call_arguments.delta"));
    assert!(output_text.contains("event: response.completed"));
    assert!(output_text.contains("\"type\":\"response.completed\""));
    assert!(output_text.contains("\"type\":\"function_call\""));
    assert!(output_text.contains("\"name\":\"get_weather\""));
    assert!(output_text.contains("\\\"location\\\":\\\"Tokyo\\\""));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn gemini_to_openai_responses_compact_stream_rewriter_converts_function_call_to_completed_event() {
    let report_context = json!({
        "provider_api_format": "gemini:generate_content",
        "client_api_format": "openai:responses:compact",
        "needs_conversion": true,
        "mapped_model": "gemini-2.5-pro",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let output = rewriter
        .push_chunk(
            b"data: {\"responseId\":\"resp_tool_compact_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Need a tool.\"},{\"functionCall\":{\"name\":\"get_weather\",\"args\":{\"location\":\"Tokyo\"}}}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
        )
        .expect("rewrite should succeed");
    let output_text = utf8(output);
    assert!(output_text.contains("event: response.created"));
    assert!(output_text.contains("event: response.output_item.added"));
    assert!(output_text.contains("event: response.function_call_arguments.delta"));
    assert!(output_text.contains("event: response.completed"));
    assert!(output_text.contains("\"type\":\"response.completed\""));
    assert!(output_text.contains("\"type\":\"function_call\""));
    assert!(output_text.contains("\"name\":\"get_weather\""));
    assert!(output_text.contains("\\\"location\\\":\\\"Tokyo\\\""));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_chat_to_claude_chat_stream_rewriter_converts_via_standard_matrix() {
    let report_context = json!({
        "provider_api_format": "openai:chat",
        "client_api_format": "claude:messages",
        "needs_conversion": true,
        "mapped_model": "gpt-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let first = rewriter
        .push_chunk(
            "data: {\"id\":\"chatcmpl_std_claude_123\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello Claude\"},\"finish_reason\":null}]}\n\n"
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let first_text = utf8(first);
    assert!(first_text.contains("event: message_start"));
    assert!(first_text.contains("event: content_block_start"));
    assert!(first_text.contains("event: content_block_delta"));
    assert!(first_text.contains("\"text\":\"Hello Claude\""));
    assert!(first_text.contains("\"usage\":{\"input_tokens\":0,\"output_tokens\":0}"));

    let second = rewriter
        .push_chunk(
            concat!(
                "data: {\"id\":\"chatcmpl_std_claude_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
                "data: [DONE]\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(second);
    assert!(output_text.contains("event: content_block_stop"));
    assert!(output_text.contains("event: message_delta"));
    assert!(output_text.contains("event: message_stop"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}

#[test]
fn openai_chat_to_claude_chat_stream_rewriter_injects_default_usage_when_finish_chunk_lacks_usage()
{
    let report_context = json!({
        "provider_api_format": "openai:chat",
        "client_api_format": "claude:messages",
        "needs_conversion": true,
        "mapped_model": "gpt-5.4",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let first = rewriter
        .push_chunk(
            "data: {\"id\":\"chatcmpl_usage_missing_123\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"gpt-5.4\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n"
                .as_bytes(),
        )
        .expect("rewrite should succeed");
    let first_text = utf8(first);
    assert!(first_text.contains("event: message_start"));

    let second = rewriter
        .push_chunk(
            concat!(
                "data: {\"id\":\"chatcmpl_usage_missing_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5.4\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(second);
    assert!(output_text.is_empty(), "{output_text}");
    let final_text = utf8(rewriter.finish().expect("finish should succeed"));
    assert!(final_text.contains("event: message_delta"));
    assert!(final_text.contains("\"stop_reason\":\"end_turn\""));
    assert!(final_text.contains("\"usage\":{\"input_tokens\":0,\"output_tokens\":0}"));
    assert!(final_text.contains("event: message_stop"));
}

#[test]
fn openai_chat_to_gemini_cli_stream_rewriter_converts_via_standard_matrix() {
    let report_context = json!({
        "provider_api_format": "openai:chat",
        "client_api_format": "gemini:generate_content",
        "needs_conversion": true,
        "mapped_model": "gpt-5",
    });
    let mut rewriter =
        maybe_build_local_stream_rewriter(Some(&report_context)).expect("rewriter should exist");
    let first = rewriter
        .push_chunk(
            "data: {\"id\":\"chatcmpl_std_gemini_cli_123\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello Gemini CLI\"},\"finish_reason\":null}]}\n\n"
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let first_text = utf8(first);
    assert!(first_text.contains("\"responseId\":\"chatcmpl_std_gemini_cli_123\""));
    assert!(first_text.contains("\"candidates\""));
    assert!(first_text.contains("\"text\":\"Hello Gemini CLI\""));

    let second = rewriter
        .push_chunk(
            concat!(
                "data: {\"id\":\"chatcmpl_std_gemini_cli_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\n",
                "data: [DONE]\n\n"
            )
            .as_bytes(),
        )
        .expect("rewrite should succeed");
    let output_text = utf8(second);
    assert!(output_text.contains("\"finishReason\":\"STOP\""));
    assert!(output_text.contains("\"totalTokenCount\":5"));
    assert!(rewriter.finish().expect("finish should succeed").is_empty());
}
