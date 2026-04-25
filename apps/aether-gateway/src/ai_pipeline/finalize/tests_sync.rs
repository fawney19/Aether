use std::collections::BTreeMap;

use axum::body::to_bytes;
use base64::Engine as _;
use serde_json::json;

use super::{
    aggregate_claude_stream_sync_response, aggregate_gemini_stream_sync_response,
    aggregate_openai_chat_stream_sync_response, aggregate_openai_cli_stream_sync_response,
    convert_claude_chat_response_to_openai_chat, convert_claude_cli_response_to_openai_cli,
    convert_gemini_chat_response_to_openai_chat, convert_gemini_cli_response_to_openai_cli,
    maybe_build_local_core_sync_finalize_response,
};
use crate::ai_pipeline::GatewayControlDecision;
use crate::ai_pipeline::{
    convert_openai_chat_response_to_openai_cli, convert_openai_cli_response_to_openai_chat,
};
use crate::usage::GatewaySyncReportRequest;

fn test_decision() -> GatewayControlDecision {
    GatewayControlDecision {
        public_path: "/v1/responses/compact".to_string(),
        public_query_string: None,
        route_class: Some("ai_public".to_string()),
        route_family: Some("openai".to_string()),
        route_kind: Some("compact".to_string()),
        auth_endpoint_signature: Some("openai:compact".to_string()),
        execution_runtime_candidate: true,
        auth_context: None,
        admin_principal: None,
        local_auth_rejection: None,
    }
}

fn crc32(data: &[u8]) -> u32 {
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

fn encode_string_header(name: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(7);
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    out
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

fn encode_kiro_event_frame(event_type: &str, payload: serde_json::Value) -> Vec<u8> {
    let mut headers = encode_string_header(":message-type", "event");
    headers.extend_from_slice(&encode_string_header(":event-type", event_type));
    let payload = serde_json::to_vec(&payload).expect("payload should encode");
    encode_frame(headers, payload)
}

fn encode_kiro_exception_frame(exception_type: &str) -> Vec<u8> {
    let mut headers = encode_string_header(":message-type", "exception");
    headers.extend_from_slice(&encode_string_header(":exception-type", exception_type));
    encode_frame(headers, Vec::new())
}

fn encode_kiro_error_frame(error_code: &str) -> Vec<u8> {
    let mut headers = encode_string_header(":message-type", "error");
    headers.extend_from_slice(&encode_string_header(":error-code", error_code));
    encode_frame(headers, Vec::new())
}

fn build_kiro_claude_cli_sync_finalize_payload(body_bytes: Vec<u8>) -> GatewaySyncReportRequest {
    GatewaySyncReportRequest {
        trace_id: "trace-kiro-cli-sync-local-finalize-123".to_string(),
        report_kind: "claude_cli_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "claude:cli",
            "provider_api_format": "claude:cli",
            "model": "claude-sonnet-4",
            "mapped_model": "claude-sonnet-4-upstream",
            "needs_conversion": false,
            "has_envelope": true,
            "envelope_name": "kiro:generateAssistantResponse",
            "original_request_body": {
                "model": "claude-sonnet-4",
                "messages": []
            }
        })),
        status_code: 200,
        headers: BTreeMap::from([(
            "content-type".to_string(),
            "application/vnd.amazon.eventstream".to_string(),
        )]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(body_bytes)),
        telemetry: None,
    }
}

#[test]
fn aggregates_openai_chat_stream_text_chunks_to_final_response() {
    let body = concat!(
        "data: {\"id\":\"chatcmpl_123\",\"object\":\"chat.completion.chunk\",\"created\":1,",
        "\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",",
        "\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl_123\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-5\",",
        "\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],",
        "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
        "data: [DONE]\n\n",
    );

    let result =
        aggregate_openai_chat_stream_sync_response(body.as_bytes()).expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "chatcmpl_123",
            "object": "chat.completion",
            "created": 1,
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello",
                },
                "finish_reason": "stop",
            }],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 2,
                "total_tokens": 3,
            },
        })
    );
}

#[test]
fn aggregates_openai_cli_stream_completed_event_to_final_response() {
    let body = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
    );

    let result =
        aggregate_openai_cli_stream_sync_response(body.as_bytes()).expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "resp_123",
            "object": "response",
            "model": "gpt-5",
            "status": "completed",
            "output": [{
                "type": "message",
                "id": "resp_123_msg",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello",
                    "annotations": []
                }]
            }],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 2,
                "total_tokens": 3,
            },
        })
    );
}

#[test]
fn aggregates_openai_cli_stream_tool_call_events_to_final_response() {
    let body = concat!(
        "event: response.created\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_tool_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
        "event: response.output_item.added\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"call_123\",\"call_id\":\"call_123\",\"name\":\"get_weather\",\"arguments\":\"\",\"status\":\"in_progress\"}}\n\n",
        "event: response.function_call_arguments.delta\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"item_id\":\"call_123\",\"call_id\":\"call_123\",\"delta\":\"{\\\"city\\\":\\\"SF\\\"}\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_tool_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
    );

    let result =
        aggregate_openai_cli_stream_sync_response(body.as_bytes()).expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "resp_tool_123",
            "object": "response",
            "model": "gpt-5",
            "status": "completed",
            "output": [{
                "type": "function_call",
                "id": "call_123",
                "call_id": "call_123",
                "name": "get_weather",
                "arguments": "{\"city\":\"SF\"}",
                "status": "in_progress",
            }],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 2,
                "total_tokens": 3,
            },
        })
    );
}

#[test]
fn aggregates_claude_stream_events_to_final_response() {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3-5-sonnet-latest\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":5,\"output_tokens\":7}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );

    let result =
        aggregate_claude_stream_sync_response(body.as_bytes()).expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "model": "claude-3-5-sonnet-latest",
            "content": [{
                "type": "text",
                "text": "hello"
            }],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 5,
                "output_tokens": 7
            }
        })
    );
}

#[test]
fn aggregates_gemini_stream_events_to_final_response() {
    let body = concat!(
        "data: {\"responseId\":\"resp_gemini_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"he\"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-1.5-flash\"}\n\n",
        "data: {\"responseId\":\"resp_gemini_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-1.5-flash\",\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":7,\"totalTokenCount\":12}}\n\n",
    );

    let result =
        aggregate_gemini_stream_sync_response(body.as_bytes()).expect("result should exist");

    assert_eq!(
        result,
        json!({
            "responseId": "resp_gemini_123",
            "candidates": [{
                "content": {
                    "parts": [{"text": "hello"}],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-1.5-flash",
            "usageMetadata": {
                "promptTokenCount": 5,
                "candidatesTokenCount": 7,
                "totalTokenCount": 12
            }
        })
    );
}

#[test]
fn converts_claude_chat_response_to_openai_chat_response() {
    let result = convert_claude_chat_response_to_openai_chat(
        &json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-upstream",
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "text", "text": " Claude"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 5,
                "output_tokens": 7
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "claude:chat",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "msg_123",
            "object": "chat.completion",
            "model": "claude-sonnet-4-upstream",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello Claude"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 7,
                "total_tokens": 12
            }
        })
    );
}

#[test]
fn converts_claude_chat_tool_use_to_openai_chat_tool_calls() {
    let result = convert_claude_chat_response_to_openai_chat(
        &json!({
            "id": "msg_tool_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-upstream",
            "content": [
                {"type": "text", "text": "Checking."},
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "get_weather",
                    "input": {"location": "Tokyo"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 5,
                "output_tokens": 7
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "claude:chat",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "msg_tool_123",
            "object": "chat.completion",
            "model": "claude-sonnet-4-upstream",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Checking.",
                    "tool_calls": [{
                        "id": "toolu_123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\":\"Tokyo\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 5,
                "completion_tokens": 7,
                "total_tokens": 12
            }
        })
    );
}

#[test]
fn converts_claude_chat_thinking_block_to_openai_reasoning_content() {
    let result = convert_claude_chat_response_to_openai_chat(
        &json!({
            "id": "msg_think_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-upstream",
            "content": [
                {"type": "thinking", "thinking": "Need to reason first."},
                {"type": "text", "text": "Final answer"}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 5,
                "output_tokens": 7
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "claude:chat",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result["choices"][0]["message"]["reasoning_content"],
        "Need to reason first."
    );
    assert_eq!(result["choices"][0]["message"]["content"], "Final answer");
}

#[test]
fn converts_gemini_chat_response_to_openai_chat_response() {
    let result = convert_gemini_chat_response_to_openai_chat(
        &json!({
            "responseId": "resp_123",
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello Gemini"}],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-2.5-pro-upstream",
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "gemini:chat",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "resp_123",
            "object": "chat.completion",
            "model": "gemini-2.5-pro-upstream",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello Gemini"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 2,
                "total_tokens": 3
            }
        })
    );
}

#[test]
fn converts_gemini_chat_function_call_to_openai_chat_tool_calls() {
    let result = convert_gemini_chat_response_to_openai_chat(
        &json!({
            "responseId": "resp_tool_123",
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Let me call a tool."},
                        {"functionCall": {"name": "get_weather", "args": {"city": "SF"}}}
                    ],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-2.5-pro-upstream",
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "gemini:chat",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "resp_tool_123",
            "object": "chat.completion",
            "model": "gemini-2.5-pro-upstream",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Let me call a tool.",
                    "tool_calls": [{
                        "id": "call_auto_1",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":\"SF\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 1,
                "completion_tokens": 2,
                "total_tokens": 3
            }
        })
    );
}

#[test]
fn converts_gemini_chat_thought_part_to_openai_reasoning_content() {
    let result = convert_gemini_chat_response_to_openai_chat(
        &json!({
            "responseId": "resp_think_123",
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Internal reasoning.", "thought": true},
                        {"text": "Visible answer"}
                    ],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-2.5-pro-upstream",
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "gemini:chat",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result["choices"][0]["message"]["reasoning_content"],
        "Internal reasoning."
    );
    assert_eq!(result["choices"][0]["message"]["content"], "Visible answer");
}

#[test]
fn converts_gemini_chat_inline_data_to_openai_chat_image_part() {
    let result = convert_gemini_chat_response_to_openai_chat(
        &json!({
            "responseId": "resp_img_123",
            "candidates": [{
                "content": {
                    "parts": [{
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": "iVBORw0KGgo="
                        }
                    }],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-2.5-pro-upstream",
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "gemini:chat",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result["choices"][0]["message"]["content"],
        json!([{
            "type": "image_url",
            "image_url": {
                "url": "data:image/png;base64,iVBORw0KGgo="
            }
        }])
    );
}

#[test]
fn converts_openai_cli_reasoning_item_to_openai_chat_reasoning_content() {
    let result = convert_openai_cli_response_to_openai_chat(
        &json!({
            "id": "resp_reason_123",
            "object": "response",
            "model": "gpt-5",
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{
                        "type": "summary_text",
                        "text": "Thinking summary."
                    }]
                },
                {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Final answer",
                        "annotations": []
                    }]
                }
            ],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 5,
                "total_tokens": 8
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "openai:cli",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result["choices"][0]["message"]["reasoning_content"],
        "Thinking summary."
    );
    assert_eq!(result["choices"][0]["message"]["content"], "Final answer");
}

#[test]
fn converts_openai_cli_output_image_to_openai_chat_image_part() {
    let result = convert_openai_cli_response_to_openai_chat(
        &json!({
            "id": "resp_img_cli_123",
            "object": "response",
            "model": "gpt-5",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_image",
                    "image_url": "data:image/png;base64,iVBORw0KGgo="
                }]
            }],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 5,
                "total_tokens": 8
            }
        }),
        &json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "openai:cli",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result["choices"][0]["message"]["content"],
        json!([{
            "type": "image_url",
            "image_url": {
                "url": "data:image/png;base64,iVBORw0KGgo="
            }
        }])
    );
}

#[test]
fn converts_openai_chat_image_part_to_openai_cli_output_image() {
    let result = convert_openai_chat_response_to_openai_cli(
        &json!({
            "id": "chatcmpl_img_123",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": [{
                        "type": "image_url",
                        "image_url": {
                            "url": "data:image/png;base64,iVBORw0KGgo="
                        }
                    }]
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 3,
                "completion_tokens": 5,
                "total_tokens": 8
            }
        }),
        &json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "openai:chat",
            "model": "gpt-5"
        }),
        false,
    )
    .expect("result should exist");

    assert_eq!(
        result["output"][0]["content"],
        json!([{
            "type": "output_image",
            "image_url": "data:image/png;base64,iVBORw0KGgo="
        }])
    );
}

#[test]
fn converts_claude_cli_response_to_openai_cli_response() {
    let result = convert_claude_cli_response_to_openai_cli(
        &json!({
            "id": "msg_cli_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-code-upstream",
            "content": [
                {"type": "text", "text": "Hello"},
                {"type": "text", "text": " Claude CLI"}
            ],
            "usage": {
                "input_tokens": 4,
                "output_tokens": 6
            }
        }),
        &json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "claude:cli",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "msg_cli_123",
            "object": "response",
            "status": "completed",
            "model": "claude-code-upstream",
            "output": [{
                "type": "message",
                "id": "msg_cli_123_msg",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello Claude CLI",
                    "annotations": []
                }]
            }],
            "usage": {
                "input_tokens": 4,
                "output_tokens": 6,
                "total_tokens": 10
            }
        })
    );
}

#[test]
fn converts_claude_cli_tool_use_to_openai_cli_function_call() {
    let result = convert_claude_cli_response_to_openai_cli(
        &json!({
            "id": "msg_cli_tool_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-code-upstream",
            "content": [
                {"type": "text", "text": "Running tool."},
                {
                    "type": "tool_use",
                    "id": "tool_123",
                    "name": "read_file",
                    "input": {"path": "/tmp/test.txt"}
                }
            ],
            "usage": {
                "input_tokens": 4,
                "output_tokens": 6
            }
        }),
        &json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "claude:cli",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "msg_cli_tool_123",
            "object": "response",
            "status": "completed",
            "model": "claude-code-upstream",
            "output": [
                {
                    "type": "message",
                    "id": "msg_cli_tool_123_msg",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Running tool.",
                        "annotations": []
                    }]
                },
                {
                    "type": "function_call",
                    "id": "tool_123",
                    "call_id": "tool_123",
                    "name": "read_file",
                    "arguments": "{\"path\":\"/tmp/test.txt\"}"
                }
            ],
            "usage": {
                "input_tokens": 4,
                "output_tokens": 6,
                "total_tokens": 10
            }
        })
    );
}

#[test]
fn converts_gemini_cli_response_to_openai_cli_response() {
    let result = convert_gemini_cli_response_to_openai_cli(
        &json!({
            "responseId": "resp_cli_123",
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello Gemini CLI"}],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-cli-upstream",
            "usageMetadata": {
                "promptTokenCount": 3,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 10
            }
        }),
        &json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "gemini:cli",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "resp_cli_123",
            "object": "response",
            "status": "completed",
            "model": "gemini-cli-upstream",
            "output": [{
                "type": "message",
                "id": "resp_cli_123_msg",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello Gemini CLI",
                    "annotations": []
                }]
            }],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 7,
                "total_tokens": 10,
                "output_tokens_details": {
                    "reasoning_tokens": 2
                }
            }
        })
    );
}

#[test]
fn converts_gemini_cli_function_call_to_openai_cli_function_call() {
    let result = convert_gemini_cli_response_to_openai_cli(
        &json!({
            "responseId": "resp_cli_tool_123",
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Need a tool."},
                        {"functionCall": {"name": "get_weather", "args": {"location": "Tokyo"}}}
                    ],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-cli-upstream",
            "usageMetadata": {
                "promptTokenCount": 3,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 10
            }
        }),
        &json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "gemini:cli",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result,
        json!({
            "id": "resp_cli_tool_123",
            "object": "response",
            "status": "completed",
            "model": "gemini-cli-upstream",
            "output": [
                {
                    "type": "message",
                    "id": "resp_cli_tool_123_msg",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Need a tool.",
                        "annotations": []
                    }]
                },
                {
                    "type": "function_call",
                    "id": "call_auto_1",
                    "call_id": "call_auto_1",
                    "name": "get_weather",
                    "arguments": "{\"location\":\"Tokyo\"}"
                }
            ],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 7,
                "total_tokens": 10,
                "output_tokens_details": {
                    "reasoning_tokens": 2
                }
            }
        })
    );
}

#[test]
fn converts_gemini_cli_inline_data_to_openai_cli_output_image() {
    let result = convert_gemini_cli_response_to_openai_cli(
        &json!({
            "responseId": "resp_cli_img_123",
            "candidates": [{
                "content": {
                    "parts": [{
                        "inlineData": {
                            "mimeType": "image/png",
                            "data": "iVBORw0KGgo="
                        }
                    }],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-cli-upstream",
            "usageMetadata": {
                "promptTokenCount": 3,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 10
            }
        }),
        &json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "gemini:cli",
            "model": "gpt-5"
        }),
    )
    .expect("result should exist");

    assert_eq!(
        result["output"][0]["content"],
        json!([{
            "type": "output_image",
            "image_url": "data:image/png;base64,iVBORw0KGgo="
        }])
    );
}

#[test]
fn local_finalize_handles_openai_compact_cross_format_sync_response() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-compact-sync-123".to_string(),
        report_kind: "openai_compact_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:compact",
            "provider_api_format": "gemini:cli",
            "model": "gpt-5",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_json: Some(json!({
            "responseId": "resp_cli_123",
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello Gemini CLI"}],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-cli-upstream",
            "usageMetadata": {
                "promptTokenCount": 3,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 10
            }
        })),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-compact-sync-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    assert_eq!(outcome.response.status(), 200);
    let report = outcome
        .background_report
        .expect("compact cross-format should downgrade to success report");
    assert_eq!(report.report_kind, "openai_cli_sync_success");
    assert_eq!(
        report.client_body_json.expect("client body should exist")["object"],
        "response"
    );
}

#[test]
fn local_finalize_handles_openai_compact_cross_format_function_call_response() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-compact-tool-123".to_string(),
        report_kind: "openai_compact_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:compact",
            "provider_api_format": "gemini:cli",
            "model": "gpt-5",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_json: Some(json!({
            "responseId": "resp_cli_tool_123",
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Need a tool."},
                        {"functionCall": {"name": "get_weather", "args": {"location": "Tokyo"}}}
                    ],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-cli-upstream",
            "usageMetadata": {
                "promptTokenCount": 3,
                "candidatesTokenCount": 5,
                "thoughtsTokenCount": 2,
                "totalTokenCount": 10
            }
        })),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-compact-tool-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("compact tool-call should downgrade to success report");
    let client_body = report.client_body_json.expect("client body should exist");
    assert_eq!(client_body["object"], "response");
    assert_eq!(client_body["output"][1]["type"], "function_call");
}

#[test]
fn local_finalize_handles_openai_cli_openai_family_sync_response_even_when_conversion_flagged() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-cli-family-conversion-123".to_string(),
        report_kind: "openai_cli_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "openai:compact",
            "model": "gpt-5",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_json: Some(json!({
            "id": "resp_cli_family_123",
            "object": "response",
            "status": "completed",
            "model": "gpt-5",
            "output": [{
                "type": "message",
                "id": "resp_cli_family_123_msg",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "Hello OpenAI family",
                    "annotations": []
                }]
            }],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 5,
                "total_tokens": 8
            }
        })),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-cli-family-conversion-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    assert_eq!(outcome.response.status(), 200);
    let report = outcome
        .background_report
        .expect("same-family finalize should downgrade to success report");
    assert_eq!(report.report_kind, "openai_cli_sync_success");
    assert_eq!(
        report.body_json.expect("provider body should exist")["id"],
        "resp_cli_family_123"
    );
}

#[test]
fn local_finalize_handles_openai_chat_stream_response_from_openai_chat() {
    let body = concat!(
        "data: {\"id\":\"chatcmpl_stream_direct_123\",\"object\":\"chat.completion.chunk\",\"created\":1,",
        "\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl_stream_direct_123\",\"object\":\"chat.completion.chunk\",",
        "\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl_stream_direct_123\",\"object\":\"chat.completion.chunk\",",
        "\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-chat-stream-sync-123".to_string(),
        report_kind: "openai_chat_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "openai:chat",
            "model": "gpt-5",
            "mapped_model": "gpt-5",
            "needs_conversion": false,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(body.as_bytes())),
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-chat-stream-sync-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("stream finalize should downgrade to success report");
    assert_eq!(report.report_kind, "openai_chat_sync_success");
    let provider_body = report.body_json.expect("provider body should exist");
    assert_eq!(provider_body["object"], "chat.completion");
    assert_eq!(
        provider_body["choices"][0]["message"]["content"],
        "Hello world"
    );
}

#[test]
fn local_finalize_handles_openai_cli_cross_format_stream_response_from_gemini() {
    let body = concat!(
        "data: {\"responseId\":\"resp_cli_stream_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}],\"role\":\"model\"},\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\"}\n\n",
        "data: {\"responseId\":\"resp_cli_stream_123\",\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello Gemini CLI\"}],\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],\"modelVersion\":\"gemini-2.5-pro-upstream\",\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":3,\"totalTokenCount\":5}}\n\n",
    );
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-cli-xfmt-stream-123".to_string(),
        report_kind: "openai_cli_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "gemini:cli",
            "model": "gpt-5",
            "mapped_model": "gemini-2.5-pro-upstream",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(body.as_bytes())),
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-cli-xfmt-stream-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("cross-format stream finalize should downgrade to success report");
    assert_eq!(report.report_kind, "openai_cli_sync_success");
    let client_body = report.client_body_json.expect("client body should exist");
    assert_eq!(client_body["object"], "response");
    assert_eq!(
        client_body["output"][0]["content"][0]["text"],
        "Hello Gemini CLI"
    );
}

#[test]
fn local_finalize_handles_openai_compact_openai_family_stream_response_even_when_conversion_flagged(
) {
    let body = concat!(
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_cli_family_stream_123\",\"object\":\"response\",\"model\":\"gpt-5\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
    );
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-compact-family-stream-123".to_string(),
        report_kind: "openai_compact_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:compact",
            "provider_api_format": "openai:cli",
            "model": "gpt-5",
            "mapped_model": "gpt-5",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(body.as_bytes())),
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-compact-family-stream-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("same-family stream finalize should downgrade to success report");
    assert_eq!(report.report_kind, "openai_cli_sync_success");
    let provider_body = report.body_json.expect("provider body should exist");
    assert_eq!(provider_body["object"], "response");
    assert_eq!(provider_body["status"], "completed");
}

#[test]
fn local_finalize_preserves_provider_stream_body_for_same_format_stream_aggregated_to_sync() {
    let body = concat!(
        "event: response.output_text.delta\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"Hello sync\"}\n\n",
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_cli_samefmt_stream_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"msg_123\",\"role\":\"assistant\",\"status\":\"completed\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello sync\",\"annotations\":[]}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":2,\"total_tokens\":3}}}\n\n",
    );
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-cli-samefmt-stream-123".to_string(),
        report_kind: "openai_cli_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:cli",
            "provider_api_format": "openai:cli",
            "model": "gpt-5.4",
            "mapped_model": "gpt-5.4",
            "needs_conversion": false,
            "has_envelope": false,
            "upstream_is_stream": true,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "text/event-stream".to_string())]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(body.as_bytes())),
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-cli-samefmt-stream-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("same-format stream finalize should downgrade to success report");
    let client_body = report.client_body_json.expect("client body should exist");
    assert!(report.body_json.is_none());
    assert_eq!(
        report.body_base64,
        Some(base64::engine::general_purpose::STANDARD.encode(body.as_bytes()))
    );
    assert_eq!(client_body["object"], "response");
    assert_eq!(client_body["status"], "completed");
    assert_eq!(client_body["output"][0]["content"][0]["text"], "Hello sync");
}

#[test]
fn local_finalize_handles_openai_chat_cross_format_sync_response_from_claude() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-chat-xfmt-claude-sync-123".to_string(),
        report_kind: "openai_chat_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "claude:chat",
            "model": "gpt-5",
            "mapped_model": "claude-sonnet-4",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_json: Some(json!({
            "id": "msg_claude_direct_123",
            "type": "message",
            "model": "claude-sonnet-4",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hello Claude"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 2,
                "output_tokens": 3
            }
        })),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-chat-xfmt-claude-sync-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("cross-format finalize should downgrade to success report");
    assert_eq!(report.report_kind, "openai_chat_sync_success");
    assert_eq!(
        report.body_json.expect("provider body should exist")["id"],
        "msg_claude_direct_123"
    );
    let client_body = report.client_body_json.expect("client body should exist");
    assert_eq!(client_body["object"], "chat.completion");
    assert_eq!(
        client_body["choices"][0]["message"]["content"],
        "Hello Claude"
    );
}

#[test]
fn local_finalize_handles_openai_chat_cross_format_sync_response_from_gemini() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-chat-xfmt-gemini-sync-123".to_string(),
        report_kind: "openai_chat_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "gemini:chat",
            "model": "gpt-5",
            "mapped_model": "gemini-2.5-pro",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_json: Some(json!({
            "responseId": "resp_gemini_direct_123",
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello Gemini"}],
                    "role": "model"
                },
                "finishReason": "STOP",
                "index": 0
            }],
            "modelVersion": "gemini-2.5-pro-upstream",
            "usageMetadata": {
                "promptTokenCount": 1,
                "candidatesTokenCount": 2,
                "totalTokenCount": 3
            }
        })),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-chat-xfmt-gemini-sync-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("cross-format finalize should downgrade to success report");
    assert_eq!(report.report_kind, "openai_chat_sync_success");
    assert_eq!(
        report.body_json.expect("provider body should exist")["responseId"],
        "resp_gemini_direct_123"
    );
    let client_body = report.client_body_json.expect("client body should exist");
    assert_eq!(client_body["object"], "chat.completion");
    assert_eq!(
        client_body["choices"][0]["message"]["content"],
        "Hello Gemini"
    );
    assert_eq!(client_body["usage"]["completion_tokens"], 2);
    assert_eq!(client_body["usage"]["total_tokens"], 3);
}

#[test]
fn local_finalize_handles_openai_chat_cross_format_sync_response_from_openai_cli() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-chat-xfmt-openai-cli-sync-123".to_string(),
        report_kind: "openai_chat_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:chat",
            "provider_api_format": "openai:cli",
            "model": "gpt-5.4",
            "mapped_model": "gpt-5.4",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_json: Some(json!({
            "id": "resp_cli_direct_123",
            "object": "response",
            "status": "completed",
            "model": "gpt-5.4",
            "output": [
                {
                    "type": "message",
                    "id": "msg_cli_direct_123",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": "Hello CLI",
                        "annotations": []
                    }]
                },
                {
                    "type": "function_call",
                    "id": "fc_cli_direct_123",
                    "call_id": "call_weather_123",
                    "name": "get_weather",
                    "arguments": "{\"city\":\"SF\"}"
                }
            ],
            "usage": {
                "input_tokens": 2,
                "output_tokens": 3,
                "total_tokens": 5
            }
        })),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-chat-xfmt-openai-cli-sync-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("cross-format finalize should downgrade to success report");
    assert_eq!(report.report_kind, "openai_chat_sync_success");
    assert_eq!(
        report.body_json.expect("provider body should exist")["id"],
        "resp_cli_direct_123"
    );
    let client_body = report.client_body_json.expect("client body should exist");
    assert_eq!(client_body["object"], "chat.completion");
    assert_eq!(client_body["choices"][0]["message"]["content"], "Hello CLI");
    assert_eq!(
        client_body["choices"][0]["message"]["tool_calls"][0]["id"],
        "call_weather_123"
    );
    assert_eq!(
        client_body["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "get_weather"
    );
    assert_eq!(
        client_body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"city\":\"SF\"}"
    );
    assert_eq!(client_body["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(client_body["usage"]["completion_tokens"], 3);
    assert_eq!(client_body["usage"]["total_tokens"], 5);
}

#[test]
fn local_finalize_handles_claude_chat_cross_format_sync_response_from_openai_chat() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-claude-chat-xfmt-openai-sync-123".to_string(),
        report_kind: "claude_chat_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "claude:chat",
            "provider_api_format": "openai:chat",
            "model": "claude-sonnet-4-5",
            "mapped_model": "gpt-5",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_json: Some(json!({
            "id": "chatcmpl_openai_to_claude_123",
            "object": "chat.completion",
            "model": "gpt-5",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello OpenAI to Claude"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 2,
                "completion_tokens": 3,
                "total_tokens": 5
            }
        })),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-claude-chat-xfmt-openai-sync-123",
        &GatewayControlDecision {
            public_path: "/v1/messages".to_string(),
            public_query_string: None,
            route_class: Some("ai_public".to_string()),
            route_family: Some("claude".to_string()),
            route_kind: Some("chat".to_string()),
            auth_endpoint_signature: Some("claude:chat".to_string()),
            execution_runtime_candidate: true,
            auth_context: None,
            admin_principal: None,
            local_auth_rejection: None,
        },
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("cross-format finalize should downgrade to success report");
    assert_eq!(report.report_kind, "claude_chat_sync_success");
    let client_body = report.client_body_json.expect("client body should exist");
    assert_eq!(client_body["type"], "message");
    assert_eq!(client_body["role"], "assistant");
    assert_eq!(client_body["content"][0]["text"], "Hello OpenAI to Claude");
    assert_eq!(client_body["usage"]["input_tokens"], 2);
    assert_eq!(client_body["usage"]["output_tokens"], 3);
}

#[test]
fn local_finalize_handles_gemini_cli_cross_format_sync_response_from_claude_cli() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-gemini-cli-xfmt-claude-sync-123".to_string(),
        report_kind: "gemini_cli_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "gemini:cli",
            "provider_api_format": "claude:cli",
            "model": "gemini-cli",
            "mapped_model": "claude-code",
            "needs_conversion": true,
            "has_envelope": false,
        })),
        status_code: 200,
        headers: BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        body_json: Some(json!({
            "id": "msg_claude_cli_to_gemini_123",
            "type": "message",
            "model": "claude-code",
            "role": "assistant",
            "content": [{
                "type": "text",
                "text": "Hello Claude CLI to Gemini"
            }],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 4,
                "output_tokens": 6
            }
        })),
        client_body_json: None,
        body_base64: None,
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-gemini-cli-xfmt-claude-sync-123",
        &GatewayControlDecision {
            public_path: "/v1beta/models/gemini-cli:generateContent".to_string(),
            public_query_string: None,
            route_class: Some("ai_public".to_string()),
            route_family: Some("gemini".to_string()),
            route_kind: Some("cli".to_string()),
            auth_endpoint_signature: Some("gemini:cli".to_string()),
            execution_runtime_candidate: true,
            auth_context: None,
            admin_principal: None,
            local_auth_rejection: None,
        },
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("cross-format finalize should downgrade to success report");
    assert_eq!(report.report_kind, "gemini_cli_sync_success");
    let client_body = report.client_body_json.expect("client body should exist");
    assert_eq!(
        client_body["candidates"][0]["content"]["parts"][0]["text"],
        "Hello Claude CLI to Gemini"
    );
    assert_eq!(client_body["usageMetadata"]["promptTokenCount"], 4);
    assert_eq!(client_body["usageMetadata"]["candidatesTokenCount"], 6);
}

#[test]
fn local_finalize_handles_kiro_claude_cli_stream_tool_use_response() {
    let payload = build_kiro_claude_cli_sync_finalize_payload(
        [
            encode_kiro_event_frame("assistantResponseEvent", json!({"content": "Need a tool."})),
            encode_kiro_event_frame(
                "toolUseEvent",
                json!({
                    "name": "get_weather",
                    "toolUseId": "tool_123",
                    "input": {"city": "SF"},
                    "stop": true
                }),
            ),
        ]
        .concat(),
    );

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-kiro-cli-tool-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("same-format finalize should downgrade to success report");
    assert_eq!(report.report_kind, "claude_cli_sync_success");
    let body = report.body_json.expect("provider body should exist");
    assert_eq!(body["content"][0]["text"], "Need a tool.");
    assert_eq!(body["content"][1]["type"], "tool_use");
    assert_eq!(body["content"][1]["id"], "tool_123");
    assert_eq!(body["content"][1]["name"], "get_weather");
    assert_eq!(body["content"][1]["input"]["city"], "SF");
    assert_eq!(body["stop_reason"], "tool_use");
}

#[test]
fn local_finalize_handles_kiro_claude_cli_stream_content_length_exceeded_response() {
    let payload = build_kiro_claude_cli_sync_finalize_payload(
        [
            encode_kiro_event_frame(
                "assistantResponseEvent",
                json!({"content": "Hello from Kiro"}),
            ),
            encode_kiro_exception_frame("ContentLengthExceededException"),
        ]
        .concat(),
    );

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-kiro-cli-max-tokens-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should succeed")
    .expect("local finalize should match");

    let report = outcome
        .background_report
        .expect("same-format finalize should downgrade to success report");
    let body = report.body_json.expect("provider body should exist");
    assert_eq!(body["content"][0]["text"], "Hello from Kiro");
    assert_eq!(body["stop_reason"], "max_tokens");
}

#[test]
fn local_finalize_rejects_kiro_claude_cli_stream_upstream_error_frame() {
    let payload = build_kiro_claude_cli_sync_finalize_payload(
        [
            encode_kiro_event_frame(
                "assistantResponseEvent",
                json!({"content": "Partial output"}),
            ),
            encode_kiro_error_frame("ThrottlingException"),
        ]
        .concat(),
    );

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-kiro-cli-error-123",
        &test_decision(),
        &payload,
    )
    .expect("local finalize should not error");

    assert!(
        outcome.is_none(),
        "embedded stream errors should fall back to Python finalize instead of being reported as success"
    );
}

#[tokio::test]
async fn local_finalize_handles_openai_image_stream_response_from_output_item_done() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-image-finalize-123".to_string(),
        report_kind: "openai_image_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:image",
            "provider_api_format": "openai:image",
            "model": "gpt-image-2",
            "mapped_model": "gpt-5.4",
            "image_request": {
                "operation": "generate",
                "response_format": "b64_json",
                "output_format": "png"
            }
        })),
        status_code: 200,
        headers: BTreeMap::from([(
            "content-type".to_string(),
            "text/event-stream".to_string(),
        )]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(
            concat!(
                "event: response.created\n",
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img_123\",\"object\":\"response\",\"created_at\":1776839946,\"status\":\"in_progress\",\"model\":\"gpt-5.4\"}}\n\n",
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_123\",\"type\":\"image_generation_call\",\"status\":\"generating\",\"output_format\":\"png\",\"quality\":\"medium\",\"size\":\"1024x1536\",\"revised_prompt\":\"revised history prompt\",\"result\":\"aGVsbG8=\"}}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":2440,\"output_tokens\":184,\"total_tokens\":2624},\"tool_usage\":{\"image_gen\":{\"input_tokens\":171,\"input_tokens_details\":{\"image_tokens\":0,\"text_tokens\":171},\"output_tokens\":1372,\"output_tokens_details\":{\"image_tokens\":1372,\"text_tokens\":0},\"total_tokens\":1543}}}}\n\n"
            )
            .as_bytes(),
        )),
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-image-finalize-123",
        &test_decision(),
        &payload,
    )
    .expect("image finalize should succeed")
    .expect("image finalize should match");

    let response_body = to_bytes(outcome.response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    let response_json: serde_json::Value =
        serde_json::from_slice(&response_body).expect("response should be json");
    assert_eq!(response_json["created"], 1776839946);
    assert_eq!(response_json["data"][0]["b64_json"], "aGVsbG8=");
    assert_eq!(
        response_json["data"][0]["revised_prompt"],
        "revised history prompt"
    );
    assert_eq!(response_json["usage"]["input_tokens"], 171);
    assert_eq!(response_json["usage"]["output_tokens"], 1372);

    let report = outcome
        .background_report
        .expect("image finalize should emit success report");
    let provider_body = report.body_json.expect("provider body should exist");
    assert_eq!(provider_body["usage"]["input_tokens"], 171);
    assert_eq!(provider_body["usage"]["output_tokens"], 1372);
    assert_eq!(report.report_kind, "openai_image_sync_success");
    assert_eq!(
        report.client_body_json.expect("client body should exist")["data"][0]["b64_json"],
        "aGVsbG8="
    );
}

#[tokio::test]
async fn local_finalize_returns_b64_json_even_when_url_response_format_requested() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-image-finalize-url-123".to_string(),
        report_kind: "openai_image_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:image",
            "provider_api_format": "openai:image",
            "model": "dall-e-3",
            "mapped_model": "gpt-5.4",
            "image_request": {
                "operation": "generate",
                "response_format": "url",
                "output_format": "webp"
            }
        })),
        status_code: 200,
        headers: BTreeMap::from([(
            "content-type".to_string(),
            "text/event-stream".to_string(),
        )]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(
            concat!(
                "event: response.created\n",
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_img_url_123\",\"object\":\"response\",\"created_at\":1776839946,\"status\":\"in_progress\",\"model\":\"gpt-5.4\"}}\n\n",
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_url_123\",\"type\":\"image_generation_call\",\"status\":\"completed\",\"output_format\":\"webp\",\"revised_prompt\":\"revised webp prompt\",\"result\":\"aGVsbG8=\"}}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_url_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[],\"tool_usage\":{\"image_gen\":{\"input_tokens\":11,\"output_tokens\":22,\"total_tokens\":33}}}}\n\n"
            )
            .as_bytes(),
        )),
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-image-finalize-url-123",
        &test_decision(),
        &payload,
    )
    .expect("image finalize should succeed")
    .expect("image finalize should match");

    let response_body = to_bytes(outcome.response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    let response_json: serde_json::Value =
        serde_json::from_slice(&response_body).expect("response should be json");
    assert_eq!(response_json["data"][0]["b64_json"], "aGVsbG8=");
    assert!(response_json["data"][0].get("url").is_none());
    assert_eq!(
        response_json["data"][0]["revised_prompt"],
        "revised webp prompt"
    );
}

#[tokio::test]
async fn local_finalize_defaults_gpt_image_stream_response_to_b64_json() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-image-finalize-default-b64-123".to_string(),
        report_kind: "openai_image_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:image",
            "provider_api_format": "openai:image",
            "model": "gpt-image-2",
            "mapped_model": "gpt-5.4",
            "image_request": {
                "operation": "generate",
                "output_format": "png"
            }
        })),
        status_code: 200,
        headers: BTreeMap::from([(
            "content-type".to_string(),
            "text/event-stream".to_string(),
        )]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(
            concat!(
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_default_123\",\"type\":\"image_generation_call\",\"output_format\":\"png\",\"result\":\"aGVsbG8=\"}}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_default_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[]}}\n\n"
            )
            .as_bytes(),
        )),
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-image-finalize-default-b64-123",
        &test_decision(),
        &payload,
    )
    .expect("image finalize should succeed")
    .expect("image finalize should match");

    let response_body = to_bytes(outcome.response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    let response_json: serde_json::Value =
        serde_json::from_slice(&response_body).expect("response should be json");
    assert_eq!(response_json["data"][0]["b64_json"], "aGVsbG8=");
    assert!(response_json["data"][0].get("url").is_none());
}

#[tokio::test]
async fn local_finalize_forces_gpt_image_stream_response_to_b64_json_even_when_url_requested() {
    let payload = GatewaySyncReportRequest {
        trace_id: "trace-openai-image-finalize-force-b64-123".to_string(),
        report_kind: "openai_image_sync_finalize".to_string(),
        report_context: Some(json!({
            "client_api_format": "openai:image",
            "provider_api_format": "openai:image",
            "model": "gpt-image-2",
            "mapped_model": "gpt-5.4",
            "image_request": {
                "operation": "generate",
                "response_format": "url",
                "output_format": "png"
            }
        })),
        status_code: 200,
        headers: BTreeMap::from([(
            "content-type".to_string(),
            "text/event-stream".to_string(),
        )]),
        body_json: None,
        client_body_json: None,
        body_base64: Some(base64::engine::general_purpose::STANDARD.encode(
            concat!(
                "event: response.output_item.done\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"ig_force_123\",\"type\":\"image_generation_call\",\"output_format\":\"png\",\"result\":\"aGVsbG8=\"}}\n\n",
                "event: response.completed\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_img_force_123\",\"object\":\"response\",\"model\":\"gpt-5.4\",\"status\":\"completed\",\"output\":[]}}\n\n"
            )
            .as_bytes(),
        )),
        telemetry: None,
    };

    let outcome = maybe_build_local_core_sync_finalize_response(
        "trace-openai-image-finalize-force-b64-123",
        &test_decision(),
        &payload,
    )
    .expect("image finalize should succeed")
    .expect("image finalize should match");

    let response_body = to_bytes(outcome.response.into_body(), usize::MAX)
        .await
        .expect("response body should read");
    let response_json: serde_json::Value =
        serde_json::from_slice(&response_body).expect("response should be json");
    assert_eq!(response_json["data"][0]["b64_json"], "aGVsbG8=");
    assert!(response_json["data"][0].get("url").is_none());
}
