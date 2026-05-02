use std::borrow::Cow;

use aether_ai_formats::protocol::conversion::request::{
    convert_openai_chat_request_to_claude_request, convert_openai_chat_request_to_gemini_request,
    convert_openai_chat_request_to_openai_responses_request,
    normalize_claude_request_to_openai_chat_request,
    normalize_gemini_request_to_openai_chat_request,
    normalize_openai_responses_request_to_openai_chat_request,
};
use aether_ai_formats::protocol::registry::{convert_request, FormatContext};
use aether_ai_formats::provider_compat::proxy::rules::apply_local_body_rules;
use serde_json::Value;

use super::{
    apply_openai_responses_compact_special_body_edits,
    codex::apply_codex_openai_responses_special_body_edits,
    normalize::build_local_openai_chat_request_body,
};

#[allow(clippy::too_many_arguments)]
pub fn build_standard_request_body(
    body_json: &Value,
    client_api_format: &str,
    mapped_model: &str,
    provider_type: &str,
    provider_api_format: &str,
    request_path: &str,
    upstream_is_stream: bool,
    body_rules: Option<&Value>,
    user_api_key_id: Option<&str>,
) -> Option<Value> {
    let format_context = FormatContext::default()
        .with_mapped_model(mapped_model)
        .with_request_path(request_path)
        .with_upstream_stream(upstream_is_stream);
    let mut provider_request_body = convert_request(
        client_api_format,
        provider_api_format,
        body_json,
        &format_context,
    )
    .ok()?;

    if !apply_local_body_rules(&mut provider_request_body, body_rules, Some(body_json)) {
        return None;
    }
    apply_codex_openai_responses_special_body_edits(
        &mut provider_request_body,
        provider_type,
        provider_api_format,
        body_rules,
        user_api_key_id,
    );
    apply_openai_responses_compact_special_body_edits(
        &mut provider_request_body,
        provider_api_format,
    );
    Some(provider_request_body)
}

pub fn build_standard_request_body_from_canonical(
    canonical_request: &Value,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    match aether_ai_formats::normalize_api_format_alias(provider_api_format).as_str() {
        "openai:chat" => build_local_openai_chat_request_body(
            canonical_request,
            mapped_model,
            upstream_is_stream,
        ),
        "openai:responses" => convert_openai_chat_request_to_openai_responses_request(
            canonical_request,
            mapped_model,
            upstream_is_stream,
            false,
        ),
        "openai:responses:compact" => convert_openai_chat_request_to_openai_responses_request(
            canonical_request,
            mapped_model,
            false,
            true,
        ),
        "claude:messages" => convert_openai_chat_request_to_claude_request(
            canonical_request,
            mapped_model,
            upstream_is_stream,
        ),
        "gemini:generate_content" => convert_openai_chat_request_to_gemini_request(
            canonical_request,
            mapped_model,
            upstream_is_stream,
        ),
        _ => None,
    }
}

pub fn normalize_standard_request_to_openai_chat_request(
    body_json: &Value,
    client_api_format: &str,
    request_path: &str,
) -> Option<Value> {
    normalize_standard_request_to_openai_chat_request_cow(
        body_json,
        client_api_format,
        request_path,
    )
    .map(Cow::into_owned)
}

fn normalize_standard_request_to_openai_chat_request_cow<'a>(
    body_json: &'a Value,
    client_api_format: &str,
    request_path: &str,
) -> Option<Cow<'a, Value>> {
    match aether_ai_formats::normalize_api_format_alias(client_api_format).as_str() {
        "openai:chat" => Some(Cow::Borrowed(body_json)),
        "openai:responses" | "openai:responses:compact" => {
            normalize_openai_responses_request_to_openai_chat_request(body_json).map(Cow::Owned)
        }
        "claude:messages" => {
            normalize_claude_request_to_openai_chat_request(body_json).map(Cow::Owned)
        }
        "gemini:generate_content" => {
            normalize_gemini_request_to_openai_chat_request(body_json, request_path).map(Cow::Owned)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_standard_request_body, build_standard_request_body_from_canonical,
        normalize_standard_request_to_openai_chat_request,
    };
    use serde_json::{json, Value};

    const STANDARD_SURFACES: &[&str] = &[
        "openai:chat",
        "openai:responses",
        "claude:messages",
        "gemini:generate_content",
    ];

    fn sample_request_for(api_format: &str) -> (Value, &'static str) {
        match api_format {
            "openai:chat" => (
                json!({
                    "model": "source-model",
                    "messages": [
                        {"role": "system", "content": "Be concise."},
                        {"role": "user", "content": "Hello matrix"}
                    ],
                    "max_tokens": 32
                }),
                "/v1/chat/completions",
            ),
            "openai:responses" => (
                json!({
                    "model": "source-model",
                    "instructions": "Be concise.",
                    "input": "Hello matrix",
                    "max_output_tokens": 32
                }),
                "/v1/responses",
            ),
            "claude:messages" => (
                json!({
                    "model": "source-model",
                    "system": "Be concise.",
                    "messages": [{
                        "role": "user",
                        "content": [{"type": "text", "text": "Hello matrix"}]
                    }],
                    "max_tokens": 32
                }),
                "/v1/messages",
            ),
            "gemini:generate_content" => (
                json!({
                    "systemInstruction": {
                        "parts": [{"text": "Be concise."}]
                    },
                    "contents": [{
                        "role": "user",
                        "parts": [{"text": "Hello matrix"}]
                    }],
                    "generationConfig": {
                        "maxOutputTokens": 32
                    }
                }),
                "/v1beta/models/source-model:generateContent",
            ),
            other => panic!("unexpected api format: {other}"),
        }
    }

    fn assert_stream_flag(provider_api_format: &str, upstream_is_stream: bool, converted: &Value) {
        match provider_api_format {
            "openai:chat" | "openai:responses" | "claude:messages" => {
                assert_eq!(
                    converted
                        .get("stream")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    upstream_is_stream,
                    "{provider_api_format} stream flag should follow upstream_is_stream"
                );
            }
            "gemini:generate_content" => {
                assert!(
                    converted.get("stream").is_none(),
                    "gemini streaming is represented by endpoint URL, not request body"
                );
            }
            other => panic!("unexpected provider api format: {other}"),
        }
    }

    fn codex_default_body_rules() -> Value {
        json!([
            {"action":"drop","path":"max_output_tokens"},
            {"action":"drop","path":"temperature"},
            {"action":"drop","path":"top_p"},
            {"action":"set","path":"store","value":false},
            {
                "action":"set",
                "path":"instructions",
                "value":"You are GPT-5.",
                "condition":{"path":"instructions","op":"not_exists"}
            }
        ])
    }

    fn legacy_openai_responses_request_body(
        request: &Value,
        provider_api_format: &str,
        upstream_is_stream: bool,
    ) -> Value {
        let chat_canonical = normalize_standard_request_to_openai_chat_request(
            request,
            "openai:responses",
            "/v1/responses",
        )
        .expect("legacy openai responses normalization should succeed");
        build_standard_request_body_from_canonical(
            &chat_canonical,
            "mapped-model",
            provider_api_format,
            upstream_is_stream,
        )
        .expect("legacy openai responses target conversion should succeed")
    }

    fn legacy_openai_chat_request_body(
        request: &Value,
        provider_api_format: &str,
        upstream_is_stream: bool,
    ) -> Value {
        build_standard_request_body_from_canonical(
            request,
            "mapped-model",
            provider_api_format,
            upstream_is_stream,
        )
        .expect("legacy openai chat target conversion should succeed")
    }

    fn legacy_claude_request_body(
        request: &Value,
        provider_api_format: &str,
        upstream_is_stream: bool,
    ) -> Value {
        let chat_canonical = normalize_standard_request_to_openai_chat_request(
            request,
            "claude:messages",
            "/v1/messages",
        )
        .expect("legacy claude normalization should succeed");
        build_standard_request_body_from_canonical(
            &chat_canonical,
            "mapped-model",
            provider_api_format,
            upstream_is_stream,
        )
        .expect("legacy claude target conversion should succeed")
    }

    fn legacy_gemini_request_body(
        request: &Value,
        provider_api_format: &str,
        upstream_is_stream: bool,
    ) -> Value {
        let chat_canonical = normalize_standard_request_to_openai_chat_request(
            request,
            "gemini:generate_content",
            "/v1beta/models/source-model:generateContent",
        )
        .expect("legacy gemini normalization should succeed");
        build_standard_request_body_from_canonical(
            &chat_canonical,
            "mapped-model",
            provider_api_format,
            upstream_is_stream,
        )
        .expect("legacy gemini target conversion should succeed")
    }

    #[test]
    fn builds_request_body_for_all_standard_surface_pairs_in_sync_and_stream_modes() {
        for client_api_format in STANDARD_SURFACES {
            let (request, request_path) = sample_request_for(client_api_format);
            for provider_api_format in STANDARD_SURFACES {
                for upstream_is_stream in [false, true] {
                    let converted = build_standard_request_body(
                        &request,
                        client_api_format,
                        "mapped-model",
                        "custom",
                        provider_api_format,
                        request_path,
                        upstream_is_stream,
                        None,
                        None,
                    )
                    .unwrap_or_else(|| {
                        panic!(
                            "{client_api_format} -> {provider_api_format} should build with upstream_is_stream={upstream_is_stream}"
                        )
                    });

                    assert_stream_flag(provider_api_format, upstream_is_stream, &converted);
                    assert!(
                        converted.to_string().contains("Hello matrix"),
                        "{client_api_format} -> {provider_api_format} should retain user content"
                    );
                }
            }
        }
    }

    #[test]
    fn openai_responses_request_uses_typed_canonical_without_changing_target_payloads() {
        let request = json!({
            "model": "gpt-5",
            "instructions": "Be exact.",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": "Inspect this"},
                        {
                            "type": "input_image",
                            "image_url": "data:image/png;base64,iVBORw0KGgo=",
                            "detail": "high"
                        },
                        {
                            "type": "input_file",
                            "file_data": "data:application/pdf;base64,JVBERi0x",
                            "filename": "spec.pdf"
                        }
                    ]
                },
                {
                    "type": "function_call",
                    "call_id": "call_123",
                    "name": "lookup",
                    "arguments": "{\"q\":\"rust\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_123",
                    "output": "{\"ok\":true}"
                }
            ],
            "max_output_tokens": 64,
            "temperature": 0.2,
            "top_p": 0.9,
            "parallel_tool_calls": true,
            "tools": [{
                "type": "function",
                "name": "lookup",
                "description": "Lookup data",
                "parameters": {"type": "object"}
            }],
            "tool_choice": {"type": "function", "name": "lookup"},
            "reasoning": {"effort": "high"},
            "text": {
                "format": {
                    "type": "json_schema",
                    "json_schema": {"name": "answer", "schema": {"type": "object"}}
                },
                "verbosity": "low"
            },
            "metadata": {"trace": "abc"}
        });

        for provider_api_format in STANDARD_SURFACES {
            for upstream_is_stream in [false, true] {
                let converted = build_standard_request_body(
                    &request,
                    "openai:responses",
                    "mapped-model",
                    "custom",
                    provider_api_format,
                    "/v1/responses",
                    upstream_is_stream,
                    None,
                    None,
                )
                .expect("typed canonical route should build");
                let legacy = legacy_openai_responses_request_body(
                    &request,
                    provider_api_format,
                    upstream_is_stream,
                );
                assert_eq!(
                    converted, legacy,
                    "typed canonical openai:responses -> {provider_api_format} changed payload with upstream_is_stream={upstream_is_stream}"
                );
            }
        }
    }

    #[test]
    fn openai_chat_request_uses_typed_canonical_without_changing_target_payloads() {
        let request = json!({
            "model": "gpt-5",
            "messages": [
                {"role": "system", "content": "Be exact."},
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Inspect this"},
                        {
                            "type": "image_url",
                            "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}
                        }
                    ]
                },
                {
                    "role": "assistant",
                    "content": null,
                    "reasoning_parts": [{
                        "type": "thinking",
                        "thinking": "plan",
                        "signature": "sig_123"
                    }],
                    "tool_calls": [{
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{\"q\":\"rust\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_123",
                    "content": {"ok": true}
                }
            ],
            "max_completion_tokens": 64,
            "temperature": 0.2,
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "Lookup data",
                    "parameters": {"type": "object"}
                }
            }],
            "tool_choice": {"type": "function", "function": {"name": "lookup"}},
            "reasoning_effort": "medium",
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "answer", "schema": {"type": "object"}}
            }
        });

        for provider_api_format in STANDARD_SURFACES {
            for upstream_is_stream in [false, true] {
                let converted = build_standard_request_body(
                    &request,
                    "openai:chat",
                    "mapped-model",
                    "custom",
                    provider_api_format,
                    "/v1/chat/completions",
                    upstream_is_stream,
                    None,
                    None,
                )
                .expect("typed canonical openai chat route should build");
                let legacy = legacy_openai_chat_request_body(
                    &request,
                    provider_api_format,
                    upstream_is_stream,
                );
                assert_eq!(
                    converted, legacy,
                    "typed canonical openai:chat -> {provider_api_format} changed payload with upstream_is_stream={upstream_is_stream}"
                );
            }
        }
    }

    #[test]
    fn claude_request_uses_typed_canonical_without_changing_non_claude_target_payloads() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "system": "Be exact.",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Inspect this"},
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "iVBORw0KGgo="
                            }
                        },
                        {
                            "type": "document",
                            "source": {
                                "type": "base64",
                                "media_type": "application/pdf",
                                "data": "JVBERi0x"
                            }
                        }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "plan",
                            "signature": "sig_123"
                        },
                        {
                            "type": "tool_use",
                            "id": "toolu_123",
                            "name": "lookup",
                            "input": {"q": "rust"}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_123",
                        "content": {"ok": true}
                    }]
                }
            ],
            "max_tokens": 64,
            "temperature": 0.2,
            "top_p": 0.9,
            "tools": [{
                "name": "lookup",
                "description": "Lookup data",
                "input_schema": {"type": "object"}
            }],
            "tool_choice": {
                "type": "tool",
                "name": "lookup",
                "disable_parallel_tool_use": false
            },
            "metadata": {"trace": "abc"},
            "thinking": {"type": "enabled", "budget_tokens": 2048}
        });

        for provider_api_format in [
            "openai:chat",
            "openai:responses",
            "openai:responses:compact",
            "gemini:generate_content",
        ] {
            for upstream_is_stream in [false, true] {
                let converted = build_standard_request_body(
                    &request,
                    "claude:messages",
                    "mapped-model",
                    "custom",
                    provider_api_format,
                    "/v1/messages",
                    upstream_is_stream,
                    None,
                    None,
                )
                .expect("typed canonical claude route should build");
                let legacy =
                    legacy_claude_request_body(&request, provider_api_format, upstream_is_stream);
                assert_eq!(
                    converted, legacy,
                    "typed canonical claude:messages -> {provider_api_format} changed payload with upstream_is_stream={upstream_is_stream}"
                );
            }
        }
    }

    #[test]
    fn gemini_request_uses_typed_canonical_without_changing_non_gemini_target_payloads() {
        let request = json!({
            "systemInstruction": {
                "parts": [{"text": "Be exact."}]
            },
            "contents": [
                {
                    "role": "user",
                    "parts": [
                        {"text": "Inspect this"},
                        {"inlineData": {"mimeType": "image/png", "data": "iVBORw0KGgo="}}
                    ]
                },
                {
                    "role": "model",
                    "parts": [
                        {"text": "plan", "thought": true, "thoughtSignature": "sig_123"},
                        {"functionCall": {"id": "call_123", "name": "lookup", "args": {"q": "rust"}}}
                    ]
                },
                {
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "id": "call_123",
                            "name": "lookup",
                            "response": {"result": {"ok": true}}
                        }
                    }]
                }
            ],
            "generationConfig": {
                "maxOutputTokens": 64,
                "temperature": 0.2,
                "thinkingConfig": {"includeThoughts": true, "thinkingBudget": 2048}
            },
            "tools": [{
                "functionDeclarations": [{
                    "name": "lookup",
                    "description": "Lookup data",
                    "parameters": {"type": "object"}
                }]
            }],
            "toolConfig": {
                "functionCallingConfig": {
                    "mode": "ANY",
                    "allowedFunctionNames": ["lookup"]
                }
            }
        });

        for provider_api_format in [
            "openai:chat",
            "openai:responses",
            "openai:responses:compact",
            "claude:messages",
        ] {
            for upstream_is_stream in [false, true] {
                let converted = build_standard_request_body(
                    &request,
                    "gemini:generate_content",
                    "mapped-model",
                    "custom",
                    provider_api_format,
                    "/v1beta/models/source-model:generateContent",
                    upstream_is_stream,
                    None,
                    None,
                )
                .expect("typed canonical gemini route should build");
                let legacy =
                    legacy_gemini_request_body(&request, provider_api_format, upstream_is_stream);
                assert_eq!(
                    converted, legacy,
                    "typed canonical gemini:generate_content -> {provider_api_format} changed payload with upstream_is_stream={upstream_is_stream}"
                );
            }
        }
    }

    #[test]
    fn applies_codex_body_rules_for_all_standard_sources_to_openai_responses() {
        let body_rules = codex_default_body_rules();

        for client_api_format in STANDARD_SURFACES {
            let (mut request, request_path) = sample_request_for(client_api_format);
            if let Some(object) = request.as_object_mut() {
                object.insert("temperature".to_string(), json!(0.7));
                object.insert("top_p".to_string(), json!(0.8));
            }

            let converted = build_standard_request_body(
                &request,
                client_api_format,
                "gpt-5.5",
                "codex",
                "openai:responses",
                request_path,
                true,
                Some(&body_rules),
                Some("key-1"),
            )
            .unwrap_or_else(|| {
                panic!("{client_api_format} -> openai:responses should build with codex body rules")
            });

            assert_eq!(converted["model"], "gpt-5.5");
            assert_eq!(converted["stream"], true);
            assert_eq!(converted["store"], false);
            assert!(converted.get("max_output_tokens").is_none());
            assert!(converted.get("temperature").is_none());
            assert!(converted.get("top_p").is_none());
            assert!(
                converted.get("instructions").is_some(),
                "{client_api_format} -> openai:responses should keep or inject instructions"
            );
        }
    }

    #[test]
    fn builds_openai_chat_request_from_claude_chat_source() {
        let request = json!({
            "model": "claude-3-7-sonnet",
            "system": "You are concise.",
            "messages": [
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "Hello from Claude"}]
                }
            ],
            "max_tokens": 128
        });

        let converted = build_standard_request_body(
            &request,
            "claude:messages",
            "gpt-5",
            "openai",
            "openai:chat",
            "/v1/messages",
            false,
            None,
            None,
        )
        .expect("claude chat should convert to openai chat");

        assert_eq!(converted["model"], "gpt-5");
        assert_eq!(converted["messages"][0]["role"], "system");
        assert_eq!(converted["messages"][0]["content"], "You are concise.");
        assert_eq!(converted["messages"][1]["role"], "user");
        assert_eq!(converted["messages"][1]["content"], "Hello from Claude");
    }

    #[test]
    fn builds_streaming_openai_chat_request_from_gemini_chat_source_with_include_usage() {
        let request = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [{"text": "Hello from Gemini"}]
                }
            ]
        });

        let converted = build_standard_request_body(
            &request,
            "gemini:generate_content",
            "gpt-5",
            "openai",
            "openai:chat",
            "/v1beta/models/gemini-2.5-pro:streamGenerateContent",
            true,
            None,
            None,
        )
        .expect("gemini chat stream should convert to openai chat");

        assert_eq!(converted["model"], "gpt-5");
        assert_eq!(converted["stream"], true);
        assert_eq!(converted["stream_options"]["include_usage"], true);
        assert_eq!(converted["messages"][0]["role"], "user");
        assert_eq!(converted["messages"][0]["content"], "Hello from Gemini");
    }

    #[test]
    fn builds_claude_chat_request_from_gemini_chat_source() {
        let request = json!({
            "systemInstruction": {
                "parts": [{"text": "Be brief."}]
            },
            "contents": [
                {
                    "role": "user",
                    "parts": [{"text": "Hello from Gemini"}]
                }
            ]
        });

        let converted = build_standard_request_body(
            &request,
            "gemini:generate_content",
            "claude-sonnet-4-5",
            "anthropic",
            "claude:messages",
            "/v1beta/models/gemini-2.5-pro:generateContent",
            false,
            None,
            None,
        )
        .expect("gemini chat should convert to claude chat");

        assert_eq!(converted["model"], "claude-sonnet-4-5");
        assert_eq!(converted["messages"][0]["role"], "user");
        assert!(
            converted["messages"]
                .to_string()
                .contains("Hello from Gemini"),
            "converted claude payload should retain the gemini user text: {converted}"
        );
    }

    #[test]
    fn builds_gemini_cli_request_from_claude_cli_source() {
        let request = json!({
            "model": "claude-sonnet-4-5",
            "messages": [
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "Need CLI output"}]
                }
            ],
            "max_tokens": 64
        });

        let converted = build_standard_request_body(
            &request,
            "claude:messages",
            "gemini-2.5-pro",
            "google",
            "gemini:generate_content",
            "/v1/messages",
            false,
            None,
            None,
        )
        .expect("claude cli should convert to gemini cli");

        assert_eq!(converted["contents"][0]["role"], "user");
        assert_eq!(
            converted["contents"][0]["parts"][0]["text"],
            "Need CLI output"
        );
    }

    #[test]
    fn builds_openai_chat_request_from_openai_responses_source_with_chat_shape() {
        let request = json!({
            "model": "gpt-5",
            "instructions": "You are concise.",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_image",
                        "image_url": "https://example.com/cat.png",
                        "detail": "high"
                    },
                    {
                        "type": "input_file",
                        "file_data": "data:application/pdf;base64,JVBERi0x",
                        "filename": "spec.pdf"
                    },
                    {"type": "input_text", "text": "Summarize this"}
                ]
            }],
            "reasoning": {"effort": "high"},
            "text": {
                "format": {
                    "type": "json_schema",
                    "json_schema": {
                        "name": "answer_schema",
                        "schema": {
                            "type": "object",
                            "properties": {"answer": {"type": "string"}}
                        }
                    }
                }
            }
        });

        let converted = build_standard_request_body(
            &request,
            "openai:responses",
            "gpt-5",
            "openai",
            "openai:chat",
            "/v1/responses",
            false,
            None,
            None,
        )
        .expect("responses request should convert to chat completions");

        assert_eq!(converted["messages"][0]["role"], "system");
        assert_eq!(converted["messages"][0]["content"], "You are concise.");
        assert_eq!(converted["reasoning_effort"], "high");
        assert_eq!(
            converted["response_format"]["json_schema"]["name"],
            "answer_schema"
        );
        assert_eq!(converted["messages"][1]["content"][0]["type"], "image_url");
        assert_eq!(
            converted["messages"][1]["content"][0]["image_url"]["url"],
            "https://example.com/cat.png"
        );
        assert_eq!(
            converted["messages"][1]["content"][0]["image_url"]["detail"],
            "high"
        );
        assert_eq!(converted["messages"][1]["content"][1]["type"], "file");
        assert_eq!(
            converted["messages"][1]["content"][1]["file"]["filename"],
            "spec.pdf"
        );
    }

    #[test]
    fn builds_gemini_request_from_openai_chat_with_structured_output_and_images() {
        let request = json!({
            "model": "gpt-5",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": "data:image/png;base64,iVBORw0KGgo="
                        }
                    },
                    {"type": "text", "text": "Describe it"}
                ]
            }],
            "reasoning_effort": "medium",
            "n": 2,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "answer_schema",
                    "schema": {
                        "type": "object",
                        "properties": {"answer": {"type": "string"}}
                    }
                }
            },
            "web_search_options": {
                "search_context_size": "high"
            }
        });

        let converted = build_standard_request_body(
            &request,
            "openai:chat",
            "gemini-2.5-pro",
            "google",
            "gemini:generate_content",
            "/v1/chat/completions",
            false,
            None,
            None,
        )
        .expect("openai chat should convert to gemini");

        assert_eq!(
            converted["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            2048
        );
        assert_eq!(converted["generationConfig"]["candidateCount"], 2);
        assert_eq!(
            converted["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(
            converted["generationConfig"]["responseSchema"]["type"],
            "object"
        );
        assert_eq!(
            converted["contents"][0]["parts"][0]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(converted["tools"][0]["googleSearch"], json!({}));
    }

    #[test]
    fn builds_claude_request_from_openai_chat_with_thinking_and_data_url_image() {
        let request = json!({
            "model": "gpt-5",
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": "data:image/jpeg;base64,/9j/4AAQSk"
                        }
                    },
                    {"type": "text", "text": "What is this?"}
                ]
            }],
            "reasoning_effort": "low"
        });

        let converted = build_standard_request_body(
            &request,
            "openai:chat",
            "claude-sonnet-4-5",
            "anthropic",
            "claude:messages",
            "/v1/chat/completions",
            false,
            None,
            None,
        )
        .expect("openai chat should convert to claude");

        assert_eq!(converted["thinking"]["type"], "enabled");
        assert_eq!(converted["thinking"]["budget_tokens"], 1280);
        assert_eq!(
            converted["messages"][0]["content"][0]["source"]["type"],
            "base64"
        );
        assert_eq!(
            converted["messages"][0]["content"][0]["source"]["media_type"],
            "image/jpeg"
        );
    }

    #[test]
    fn openai_responses_nested_tools_survive_claude_messages_and_kiro_envelope_conversion() {
        let request = json!({
            "model": "gpt-5",
            "input": "Use the weather tool for Shanghai.",
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "city": {"type": "string"}
                        },
                        "required": []
                    }
                }
            }],
            "tool_choice": {
                "type": "function",
                "function": {"name": "get_weather"}
            }
        });

        let claude = build_standard_request_body(
            &request,
            "openai:responses",
            "claude-sonnet-4.6",
            "kiro",
            "claude:messages",
            "/v1/responses",
            true,
            None,
            None,
        )
        .expect("openai responses should convert to claude messages");
        assert_eq!(claude["tools"][0]["name"], "get_weather");
        assert_eq!(claude["tool_choice"]["name"], "get_weather");

        assert_eq!(claude["tools"][0]["name"], "get_weather");
        assert!(
            claude["tools"][0]["input_schema"].get("required").is_some(),
            "surface conversion should preserve the Claude tool schema before transport envelopes"
        );
    }
}
