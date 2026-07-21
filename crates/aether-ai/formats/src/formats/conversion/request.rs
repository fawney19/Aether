//! Pairwise request conversion helpers.
//!
//! These helpers keep the call sites readable while delegating wire-format
//! parsing and emitting to `formats::<format>::request` through the registry's
//! canonical IR path.

use serde_json::Value;

use crate::formats::{context::FormatContext, registry};

pub fn convert_openai_chat_request_to_claude_request(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    registry::convert_request(
        "openai:chat",
        "claude:messages",
        body_json,
        &request_context(mapped_model, upstream_is_stream),
    )
    .ok()
}

pub fn convert_openai_chat_request_to_gemini_request(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    registry::convert_request(
        "openai:chat",
        "gemini:generate_content",
        body_json,
        &request_context(mapped_model, upstream_is_stream),
    )
    .ok()
}

pub fn convert_openai_chat_request_to_openai_responses_request(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
    compact: bool,
) -> Option<Value> {
    let target_format = if compact {
        "openai:responses:compact"
    } else {
        "openai:responses"
    };
    registry::convert_request(
        "openai:chat",
        target_format,
        body_json,
        &request_context(mapped_model, upstream_is_stream),
    )
    .ok()
}

pub fn normalize_openai_responses_request_to_openai_chat_request(
    body_json: &Value,
) -> Option<Value> {
    registry::convert_request(
        "openai:responses",
        "openai:chat",
        body_json,
        &FormatContext::default(),
    )
    .ok()
}

pub fn normalize_claude_request_to_openai_chat_request(body_json: &Value) -> Option<Value> {
    registry::convert_request(
        "claude:messages",
        "openai:chat",
        body_json,
        &FormatContext::default(),
    )
    .ok()
}

pub fn normalize_gemini_request_to_openai_chat_request(
    body_json: &Value,
    request_path: &str,
) -> Option<Value> {
    registry::convert_request(
        "gemini:generate_content",
        "openai:chat",
        body_json,
        &FormatContext::default().with_request_path(request_path),
    )
    .ok()
}

pub fn extract_openai_text_content(content: Option<&Value>) -> Option<String> {
    match content {
        None | Some(Value::Null) => Some(String::new()),
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(parts)) => {
            let mut collected = Vec::new();
            for part in parts {
                let part_object = part.as_object()?;
                let part_type = part_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if matches!(part_type, "text" | "input_text") {
                    if let Some(text) = part_object.get("text").and_then(Value::as_str) {
                        if !text.trim().is_empty() {
                            collected.push(text.to_string());
                        }
                    }
                }
            }
            Some(collected.join("\n"))
        }
        _ => None,
    }
}

pub fn parse_openai_tool_result_content(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(raw)) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Value::String(String::new())
            } else {
                serde_json::from_str::<Value>(trimmed)
                    .unwrap_or_else(|_| Value::String(raw.clone()))
            }
        }
        Some(Value::Array(parts)) => {
            let texts = parts
                .iter()
                .filter_map(|part| {
                    part.as_object()
                        .and_then(|object| object.get("text"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .collect::<Vec<_>>();
            if texts.is_empty() {
                Value::Array(parts.clone())
            } else {
                Value::String(texts.join("\n"))
            }
        }
        Some(value) => value.clone(),
        None => Value::String(String::new()),
    }
}

fn request_context(mapped_model: &str, upstream_is_stream: bool) -> FormatContext {
    FormatContext::default()
        .with_mapped_model(mapped_model)
        .with_upstream_stream(upstream_is_stream)
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::formats::{context::FormatContext, registry};

    use super::{
        convert_openai_chat_request_to_claude_request,
        convert_openai_chat_request_to_openai_responses_request,
        normalize_claude_request_to_openai_chat_request,
        normalize_gemini_request_to_openai_chat_request,
        normalize_openai_responses_request_to_openai_chat_request,
    };

    #[test]
    fn pairwise_request_helper_routes_through_registry() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
        });

        let converted = convert_openai_chat_request_to_openai_responses_request(
            &body,
            "gpt-target",
            true,
            false,
        )
        .expect("responses request");

        assert_eq!(converted["model"], "gpt-target");
        assert_eq!(converted["stream"], true);
        assert_eq!(converted["input"][0]["type"], "message");
    }

    #[test]
    fn pairwise_request_helper_keeps_claude_shape() {
        let body = json!({
            "model": "gpt-source",
            "messages": [{"role": "user", "content": "hello"}],
        });

        let converted =
            convert_openai_chat_request_to_claude_request(&body, "claude-target", false)
                .expect("claude request");

        assert_eq!(converted["model"], "claude-target");
        assert_eq!(converted["messages"][0]["role"], "user");
    }

    #[test]
    fn request_normalizer_uses_format_adapter() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hello"}]}],
            "max_tokens": 128,
        });

        let converted =
            normalize_claude_request_to_openai_chat_request(&body).expect("openai chat request");

        assert_eq!(converted["model"], "claude-sonnet");
        assert_eq!(converted["messages"][0]["role"], "user");
        assert_eq!(converted["messages"][0]["content"], "hello");
    }

    #[test]
    fn claude_request_to_chat_preserves_max_reasoning_effort() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": "hello"}],
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "output_config": {"effort": "max"},
            "max_tokens": 128,
        });

        let converted =
            normalize_claude_request_to_openai_chat_request(&body).expect("openai chat request");

        assert_eq!(converted["reasoning_effort"], "max");
    }

    #[test]
    fn gemini_request_to_chat_preserves_xhigh_reasoning_effort() {
        let body = json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": "hello"}]
            }],
            "generationConfig": {
                "thinkingConfig": {"thinkingBudget": 8192}
            }
        });

        let converted = normalize_gemini_request_to_openai_chat_request(
            &body,
            "/v1beta/models/gemini-2.5-pro:generateContent",
        )
        .expect("openai chat request");

        assert_eq!(converted["reasoning_effort"], "xhigh");
    }

    #[test]
    fn responses_request_normalizer_keeps_tool_history_chat_safe() {
        let call_id_one = "call_weather_123";
        let call_id_two = "call_lookup_456";
        let tool_output_one = json!({
            "toolCallId": call_id_one,
            "input": {"city": "Hangzhou"},
            "output": {
                "content": [{"type": "text", "text": "sunny"}],
                "isError": false,
            },
        });
        let body = json!({
            "model": "glm-5.1",
            "input": [
                "weather now",
                {
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": "thinking first"}]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": "planning"
                },
                {
                    "type": "function_call",
                    "call_id": call_id_one,
                    "id": call_id_one,
                    "name": "mcp__mapsWeather",
                    "arguments": "{\"city\":\"Hangzhou\"}"
                },
                {
                    "type": "function_call",
                    "call_id": call_id_two,
                    "id": call_id_two,
                    "name": "mcp__lookupData",
                    "arguments": "{\"query\":\"museum\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": call_id_one,
                    "output": tool_output_one.to_string()
                },
                {
                    "type": "function_call_output",
                    "call_id": call_id_two,
                    "output": "done-2"
                }
            ]
        });

        let converted = normalize_openai_responses_request_to_openai_chat_request(&body)
            .expect("openai chat request");
        let messages = converted["messages"].as_array().expect("messages");

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "weather now");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["reasoning_content"], "thinking first");
        assert_eq!(messages[1]["content"], "planning");
        assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(messages[1]["tool_calls"][0]["id"], call_id_one);
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["name"],
            "mcp__mapsWeather"
        );
        assert_eq!(messages[1]["tool_calls"][1]["id"], call_id_two);
        assert_eq!(
            messages[1]["tool_calls"][1]["function"]["name"],
            "mcp__lookupData"
        );
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], call_id_one);
        let content = messages[2]["content"]
            .as_str()
            .expect("tool result content should stay a string");
        assert_eq!(
            serde_json::from_str::<Value>(content).expect("tool output json"),
            tool_output_one
        );
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], call_id_two);
        assert_eq!(messages[3]["content"], "done-2");
    }

    #[test]
    fn responses_request_normalizer_emits_empty_message_content_as_empty_string() {
        let body = json!({
            "model": "glm-5.1",
            "input": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": null
                }
            ]
        });

        let converted = normalize_openai_responses_request_to_openai_chat_request(&body)
            .expect("openai chat request");
        let messages = converted["messages"].as_array().expect("messages");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["content"], "");
    }

    #[test]
    fn responses_request_normalizer_preserves_official_chat_reasoning_effort_and_filters_extensions(
    ) {
        let body = json!({
            "model": "gpt-5.1",
            "input": "hello",
            "reasoning": {"effort": "xhigh"},
            "text": {"verbosity": "high"},
            "include": ["reasoning.encrypted_content"],
            "store": false,
            "service_tier": "priority",
            "prompt_cache_key": "cache_123",
            "safety_identifier": "user_123"
        });

        let converted = normalize_openai_responses_request_to_openai_chat_request(&body)
            .expect("openai chat request");

        assert_eq!(converted["reasoning_effort"], "xhigh");
        assert_eq!(converted["verbosity"], "high");
        assert_eq!(converted["service_tier"], "priority");
        assert_eq!(converted["prompt_cache_key"], "cache_123");
        assert_eq!(converted["safety_identifier"], "user_123");
        assert!(converted.get("include").is_none());
        assert_eq!(converted["store"], false);
        assert!(converted.get("text").is_none());
        assert!(converted.get("reasoning").is_none());
    }

    #[test]
    fn responses_request_normalizer_preserves_none_and_minimal_chat_reasoning_effort() {
        for effort in ["none", "minimal"] {
            let body = json!({
                "model": "gpt-5.1",
                "input": "hello",
                "reasoning": {"effort": effort},
            });

            let converted = normalize_openai_responses_request_to_openai_chat_request(&body)
                .expect("openai chat request");

            assert_eq!(converted["reasoning_effort"], effort);
        }
    }

    #[test]
    fn request_normalizer_preserves_multiple_claude_tool_results() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "lookup",
                            "input": {"query": "alpha"}
                        },
                        {
                            "type": "tool_use",
                            "id": "toolu_2",
                            "name": "lookup",
                            "input": {"query": "beta"}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_1",
                            "content": "alpha result"
                        },
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_2",
                            "content": [{"type": "text", "text": "beta result"}]
                        }
                    ]
                }
            ],
            "max_tokens": 128,
        });

        let converted =
            normalize_claude_request_to_openai_chat_request(&body).expect("openai chat request");
        let messages = converted["messages"].as_array().expect("messages");

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[0]["tool_calls"].as_array().unwrap().len(), 2);
        assert_eq!(messages[0]["tool_calls"][0]["id"], "toolu_1");
        assert_eq!(messages[0]["tool_calls"][1]["id"], "toolu_2");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "toolu_1");
        assert_eq!(messages[1]["content"], "alpha result");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "toolu_2");
        assert_eq!(messages[2]["content"], "beta result");
    }

    #[test]
    fn request_normalizer_preserves_claude_tool_result_order_around_text() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "before"},
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_1",
                        "content": "first"
                    },
                    {"type": "text", "text": "between"},
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_2",
                        "content": "second"
                    }
                ]
            }],
            "max_tokens": 128,
        });

        let converted =
            normalize_claude_request_to_openai_chat_request(&body).expect("openai chat request");
        let messages = converted["messages"].as_array().expect("messages");

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "before");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "toolu_1");
        assert_eq!(messages[1]["content"], "first");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "between");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "toolu_2");
        assert_eq!(messages[3]["content"], "second");
    }

    #[test]
    fn request_normalizer_marks_claude_error_tool_result_string_and_object_content() {
        let object_result = json!({"code": "ENOENT", "message": "missing"});
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_error_string",
                        "content": "lookup failed",
                        "is_error": true
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_error_empty",
                        "content": "",
                        "is_error": true
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_error_object",
                        "content": object_result,
                        "is_error": true
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_ok",
                        "content": "still ok"
                    }
                ]
            }],
            "max_tokens": 128,
        });

        let converted =
            normalize_claude_request_to_openai_chat_request(&body).expect("openai chat request");
        let messages = converted["messages"].as_array().expect("messages");

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["tool_call_id"], "toolu_error_string");
        assert_eq!(messages[0]["content"], "[tool error]\nlookup failed");

        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "toolu_error_empty");
        assert_eq!(messages[1]["content"], "[tool error]");

        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "toolu_error_object");
        let object_content = messages[2]["content"].as_str().expect("object content");
        let serialized_object = object_content
            .strip_prefix("[tool error]\n")
            .expect("error prefix");
        assert_eq!(
            serde_json::from_str::<Value>(serialized_object).expect("serialized object"),
            object_result
        );

        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "toolu_ok");
        assert_eq!(messages[3]["content"], "still ok");
    }

    #[test]
    fn request_normalizer_marks_claude_error_tool_result_multipart_image_content() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_error_image",
                    "content": [
                        {"type": "text", "text": "preview"},
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "aW1hZ2U="
                            }
                        }
                    ],
                    "is_error": true
                }]
            }],
            "max_tokens": 128,
        });

        let converted =
            normalize_claude_request_to_openai_chat_request(&body).expect("openai chat request");
        let messages = converted["messages"].as_array().expect("messages");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["tool_call_id"], "toolu_error_image");
        let content = messages[0]["content"]
            .as_array()
            .expect("multipart error content");
        assert_eq!(
            content.as_slice(),
            &[
                json!({"type": "text", "text": "[tool error]"}),
                json!({"type": "text", "text": "preview"}),
                json!({
                    "type": "image_url",
                    "image_url": {"url": "data:image/png;base64,aW1hZ2U="}
                }),
            ]
        );
    }

    #[test]
    fn request_normalizer_preserves_legal_openai_tool_content_for_claude_variants() {
        let anthropic_blocks = json!([
            {"type": "text", "text": "preview"},
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/jpeg",
                    "data": "aGVsbG8="
                }
            },
            {
                "type": "image",
                "source": {
                    "type": "url",
                    "url": "https://example.com/image.jpg"
                }
            },
            {
                "type": "document",
                "source": {
                    "type": "base64",
                    "media_type": "application/pdf",
                    "data": "JVBERi0x"
                }
            },
            {
                "type": "document",
                "source": {
                    "type": "url",
                    "url": "https://example.com/report.pdf"
                }
            },
            {
                "type": "document",
                "source": {
                    "type": "text",
                    "media_type": "text/plain",
                    "data": "document body"
                }
            }
        ]);
        let object_result = json!({"answer": 42, "ok": true});
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_object",
                        "content": object_result
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_text_blocks",
                        "content": [
                            {"type": "text", "text": "line one"},
                            {"type": "text", "text": "line two"}
                        ]
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_anthropic_blocks",
                        "content": anthropic_blocks
                    }
                ]
            }],
            "max_tokens": 128,
        });

        let converted =
            normalize_claude_request_to_openai_chat_request(&body).expect("openai chat request");
        let messages = converted["messages"].as_array().expect("messages");

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "tool");
        assert_eq!(messages[0]["tool_call_id"], "toolu_object");
        let object_content = messages[0]["content"].as_str().expect("object content");
        assert_eq!(
            serde_json::from_str::<Value>(object_content).expect("serialized object"),
            object_result
        );

        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "toolu_text_blocks");
        assert_eq!(messages[1]["content"], "line one\n\nline two");

        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "toolu_anthropic_blocks");
        let block_content = messages[2]["content"]
            .as_array()
            .expect("multipart anthropic block content");
        assert_eq!(
            block_content.as_slice(),
            &[
                json!({"type": "text", "text": "preview"}),
                json!({
                    "type": "image_url",
                    "image_url": {"url": "data:image/jpeg;base64,aGVsbG8="}
                }),
                json!({
                    "type": "image_url",
                    "image_url": {"url": "https://example.com/image.jpg"}
                }),
                json!({
                    "type": "file",
                    "file": {"file_data": "data:application/pdf;base64,JVBERi0x"}
                }),
                json!({"type": "text", "text": "[File: https://example.com/report.pdf]"}),
                json!({"type": "text", "text": "document body"}),
            ]
        );
        let block_content_json = Value::Array(block_content.clone()).to_string();
        assert!(!block_content_json.contains("\"source\""));
        assert!(block_content_json.contains("document body"));
        assert!(!block_content_json.contains("content omitted"));
    }

    #[test]
    fn claude_request_to_responses_uses_developer_system_and_sub2api_defaults() {
        let body = json!({
            "model": "claude-sonnet",
            "system": [{
                "type": "text",
                "text": "Be exact.",
                "cache_control": {"type": "ephemeral"}
            }],
            "messages": [
                {"role": "user", "content": "hello"},
                {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "private plan", "signature": "sig_hidden"},
                        {"type": "text", "text": "visible answer"},
                        {
                            "type": "tool_use",
                            "id": "toolu_calc",
                            "name": "calc",
                            "input": {"x": 1}
                        }
                    ]
                }
            ],
            "tools": [
                {"name": "implicit_empty", "description": "empty"},
                {"name": "object_empty", "input_schema": {"type": "object"}}
            ],
            "thinking": {"type": "enabled", "budget_tokens": 4096},
            "temperature": 0.2,
            "top_p": 0.9,
            "max_tokens": 10,
        });

        let converted = registry::convert_request(
            "claude:messages",
            "openai:responses",
            &body,
            &FormatContext::default().with_mapped_model("gpt-5.1"),
        )
        .expect("responses request");

        assert_eq!(converted["model"], "gpt-5.1");
        assert!(converted.get("temperature").is_none());
        assert!(converted.get("top_p").is_none());
        assert!(converted.get("instructions").is_none());
        assert_eq!(converted["text"]["verbosity"], "medium");
        assert_eq!(converted["reasoning"]["effort"], "medium");
        assert_eq!(converted["reasoning"]["summary"], "auto");
        assert_eq!(converted["max_output_tokens"], 128);
        assert_eq!(converted["store"], false);
        assert_eq!(converted["parallel_tool_calls"], true);
        assert!(converted["include"]
            .as_array()
            .expect("include")
            .iter()
            .any(|value| value.as_str() == Some("reasoning.encrypted_content")));

        let input = converted["input"].as_array().expect("responses input");
        assert_eq!(input[0]["role"], "developer");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][0]["text"], "Be exact.");
        assert_eq!(
            input[0]["content"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );
        let input_json = Value::Array(input.clone()).to_string();
        assert!(input_json.contains("visible answer"));
        assert!(!input_json.contains("private plan"));
        assert!(!input_json.contains("sig_hidden"));

        let tools = converted["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 2);
        for tool in tools {
            assert_eq!(tool["parameters"]["type"], "object");
            assert!(tool["parameters"]["properties"].is_object());
        }
    }

    #[test]
    fn claude_request_to_responses_preserves_in_message_system_guidance_as_developer_item() {
        let body = json!({
            "model": "claude-sonnet",
            "system": [{
                "type": "text",
                "text": "Be exact."
            }],
            "messages": [
                {"role": "user", "content": "hello"},
                {
                    "role": "system",
                    "content": "x-anthropic-billing-header: internal-billing-marker\nSessionStart hook additional context: follow the house style."
                },
                {"role": "assistant", "content": "visible answer"},
                {"role": "user", "content": "continue"}
            ],
            "max_tokens": 128
        });

        let converted = registry::convert_request(
            "claude:messages",
            "openai:responses",
            &body,
            &FormatContext::default().with_mapped_model("gpt-5.1"),
        )
        .expect("responses request");

        let input = converted["input"].as_array().expect("responses input");
        assert_eq!(input.len(), 5);
        assert_eq!(input[0]["role"], "developer");
        assert_eq!(input[0]["content"][0]["text"], "Be exact.");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[1]["content"][0]["text"], "hello");
        assert_eq!(input[2]["role"], "developer");
        assert_eq!(
            input[2]["content"][0]["text"],
            "SessionStart hook additional context: follow the house style."
        );
        assert!(!input[2]["content"][0]["text"]
            .as_str()
            .expect("developer guidance text")
            .contains("x-anthropic-billing-header:"));
        assert_eq!(input[3]["role"], "assistant");
        assert_eq!(input[3]["content"][0]["text"], "visible answer");
        assert_eq!(input[4]["role"], "user");
        assert_eq!(input[4]["content"][0]["text"], "continue");
        assert!(converted.get("instructions").is_none());
    }

    #[test]
    fn openai_responses_same_format_preserves_content_extensions() {
        let body = json!({
            "model": "gpt-5.1",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "stable project brief",
                    "cache_control": {"type": "ephemeral"}
                }]
            }],
            "prompt_cache_key": "cache_123"
        });

        let converted = registry::convert_request(
            "openai:responses",
            "openai:responses",
            &body,
            &FormatContext::default(),
        )
        .expect("responses request");

        assert_eq!(converted["prompt_cache_key"], "cache_123");
        assert_eq!(
            converted["input"][0]["content"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );
    }

    #[test]
    fn claude_output_config_effort_controls_responses_reasoning() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{"role": "user", "content": "hello"}],
            "thinking": {"type": "enabled", "budget_tokens": 1024},
            "output_config": {"effort": "max"},
            "max_tokens": 128,
        });

        let converted = registry::convert_request(
            "claude:messages",
            "openai:responses",
            &body,
            &FormatContext::default(),
        )
        .expect("responses request");

        assert_eq!(converted["reasoning"]["effort"], "max");
        assert_eq!(converted["reasoning"]["summary"], "auto");
    }

    #[test]
    fn responses_to_claude_defaults_max_tokens_and_omits_false_is_error() {
        let body = json!({
            "model": "gpt-5",
            "input": [
                {
                    "type": "function_call_output",
                    "call_id": "toolu_ok",
                    "output": "ok",
                    "is_error": false
                },
                {
                    "type": "function_call_output",
                    "call_id": "toolu_bad",
                    "output": "bad",
                    "is_error": true
                }
            ]
        });

        let converted = registry::convert_request(
            "openai:responses",
            "claude:messages",
            &body,
            &FormatContext::default(),
        )
        .expect("claude request");

        assert_eq!(converted["max_tokens"], 8192);
        let messages_json = converted["messages"].to_string();
        assert!(!messages_json.contains("\"is_error\":false"));
        assert!(messages_json.contains("\"is_error\":true"));
    }

    #[test]
    fn claude_request_to_responses_splits_tool_result_media_from_output() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [
                {
                    "role": "user",
                    "content": "Describe the file"
                },
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_read",
                        "name": "Read",
                        "input": {"file_path": "/tmp/photo.png"}
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_read",
                        "content": [
                            {"type": "text", "text": "File metadata: 800x600 PNG"},
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "AAAA"
                                }
                            }
                        ]
                    }]
                }
            ],
            "max_tokens": 128,
        });

        let converted = registry::convert_request(
            "claude:messages",
            "openai:responses",
            &body,
            &FormatContext::default(),
        )
        .expect("responses request");
        let input = converted["input"].as_array().expect("responses input");

        assert_eq!(input.len(), 4);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "toolu_read");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "toolu_read");
        assert_eq!(input[2]["output"], "File metadata: 800x600 PNG");
        assert_eq!(input[3]["role"], "user");
        assert_eq!(input[3]["content"][0]["type"], "input_image");
        assert_eq!(
            input[3]["content"][0]["image_url"],
            "data:image/png;base64,AAAA"
        );
    }

    #[test]
    fn claude_request_to_responses_encodes_error_tool_results_in_output() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_error_string",
                        "content": "lookup failed",
                        "is_error": true
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_error_empty",
                        "content": "",
                        "is_error": true
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_error_object",
                        "content": {"code": "ENOENT"},
                        "is_error": true
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_error_image",
                        "content": [
                            {"type": "text", "text": "preview failed"},
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "AAAA"
                                }
                            }
                        ],
                        "is_error": true
                    }
                ]
            }],
            "max_tokens": 128,
        });

        let converted = registry::convert_request(
            "claude:messages",
            "openai:responses",
            &body,
            &FormatContext::default(),
        )
        .expect("responses request");
        let input = converted["input"].as_array().expect("responses input");

        assert_eq!(input[0]["output"], "[tool error]\nlookup failed");
        assert_eq!(input[1]["output"], "[tool error]");
        assert_eq!(input[2]["output"], "[tool error]\n{\"code\":\"ENOENT\"}");
        assert_eq!(input[3]["output"], "[tool error]\npreview failed");
        assert_eq!(input[4]["role"], "user");
        assert_eq!(input[4]["content"][0]["type"], "input_image");
        assert_eq!(
            input[4]["content"][0]["image_url"],
            "data:image/png;base64,AAAA"
        );
        assert!(input.iter().all(|item| item.get("is_error").is_none()));
    }

    #[test]
    fn claude_request_to_responses_rejects_unrepresentable_tool_result_blocks() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_read",
                    "content": [{
                        "type": "image",
                        "source": {
                            "type": "unsupported",
                            "media_type": "image/png",
                            "data": "AAAA"
                        }
                    }]
                }]
            }],
            "max_tokens": 128,
        });

        let error = registry::convert_request(
            "claude:messages",
            "openai:responses",
            &body,
            &FormatContext::default(),
        )
        .expect_err("unrepresentable Claude tool_result block should fail closed");

        assert!(matches!(
            error,
            registry::FormatError::LossyConversionBlocked {
                ref source_format,
                ref target_format,
                ref field,
                ..
            } if source_format == "claude:messages"
                && target_format == "openai:responses"
                && field == "messages[].content[].tool_result.content"
        ));
    }

    #[test]
    fn claude_request_to_openai_chat_rejects_unrepresentable_tool_result_blocks() {
        let body = json!({
            "model": "claude-sonnet",
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_read",
                    "content": [{
                        "type": "image",
                        "source": {
                            "type": "unsupported",
                            "media_type": "image/png",
                            "data": "AAAA"
                        }
                    }]
                }]
            }],
            "max_tokens": 128,
        });

        let error = registry::convert_request(
            "claude:messages",
            "openai:chat",
            &body,
            &FormatContext::default(),
        )
        .expect_err("unrepresentable Claude tool_result block should fail closed for Chat");

        assert!(matches!(
            error,
            registry::FormatError::LossyConversionBlocked {
                ref source_format,
                ref target_format,
                ref field,
                ..
            } if source_format == "claude:messages"
                && target_format == "openai:chat"
                && field == "messages[].content[].tool_result.content"
        ));
    }
}
