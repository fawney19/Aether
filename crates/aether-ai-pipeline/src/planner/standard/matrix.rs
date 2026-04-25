use std::borrow::Cow;

use aether_provider_transport::{
    apply_local_body_rules, build_transport_request_url, GatewayProviderTransportSnapshot,
    TransportRequestUrlParams,
};
use serde_json::Value;

use crate::conversion::request::{
    convert_openai_chat_request_to_claude_request, convert_openai_chat_request_to_gemini_request,
    convert_openai_chat_request_to_openai_cli_request,
    normalize_claude_request_to_openai_chat_request,
    normalize_gemini_request_to_openai_chat_request,
    normalize_openai_cli_request_to_openai_chat_request,
};

use super::{
    apply_openai_compact_special_body_edits, codex::apply_codex_openai_cli_special_body_edits,
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
    let canonical_request = normalize_standard_request_to_openai_chat_request_cow(
        body_json,
        client_api_format,
        request_path,
    )?;
    let mut provider_request_body = build_standard_request_body_from_canonical(
        canonical_request.as_ref(),
        mapped_model,
        provider_api_format,
        upstream_is_stream,
    )?;

    if !apply_local_body_rules(&mut provider_request_body, body_rules, Some(body_json)) {
        return None;
    }
    apply_codex_openai_cli_special_body_edits(
        &mut provider_request_body,
        provider_type,
        provider_api_format,
        body_rules,
        user_api_key_id,
    );
    apply_openai_compact_special_body_edits(&mut provider_request_body, provider_api_format);
    Some(provider_request_body)
}

pub fn build_standard_request_body_from_canonical(
    canonical_request: &Value,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    match provider_api_format.trim().to_ascii_lowercase().as_str() {
        "openai:chat" => build_local_openai_chat_request_body(
            canonical_request,
            mapped_model,
            upstream_is_stream,
        ),
        "openai:cli" => convert_openai_chat_request_to_openai_cli_request(
            canonical_request,
            mapped_model,
            upstream_is_stream,
            false,
        ),
        "openai:compact" => convert_openai_chat_request_to_openai_cli_request(
            canonical_request,
            mapped_model,
            false,
            true,
        ),
        "claude:chat" | "claude:cli" => convert_openai_chat_request_to_claude_request(
            canonical_request,
            mapped_model,
            upstream_is_stream,
        ),
        "gemini:chat" | "gemini:cli" => convert_openai_chat_request_to_gemini_request(
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
    match client_api_format.trim().to_ascii_lowercase().as_str() {
        "openai:chat" => Some(Cow::Borrowed(body_json)),
        "openai:cli" | "openai:compact" => {
            normalize_openai_cli_request_to_openai_chat_request(body_json).map(Cow::Owned)
        }
        "claude:chat" | "claude:cli" => {
            normalize_claude_request_to_openai_chat_request(body_json).map(Cow::Owned)
        }
        "gemini:chat" | "gemini:cli" => {
            normalize_gemini_request_to_openai_chat_request(body_json, request_path).map(Cow::Owned)
        }
        _ => None,
    }
}

pub fn build_standard_upstream_url(
    parts: &http::request::Parts,
    transport: &GatewayProviderTransportSnapshot,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<String> {
    build_transport_request_url(
        transport,
        TransportRequestUrlParams {
            provider_api_format,
            mapped_model: Some(mapped_model),
            upstream_is_stream,
            request_query: parts.uri.query(),
            kiro_api_region: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::build_standard_request_body;
    use serde_json::{json, Value};

    const STANDARD_SURFACES: &[&str] = &[
        "openai:chat",
        "openai:cli",
        "claude:chat",
        "claude:cli",
        "gemini:chat",
        "gemini:cli",
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
            "openai:cli" => (
                json!({
                    "model": "source-model",
                    "instructions": "Be concise.",
                    "input": "Hello matrix",
                    "max_output_tokens": 32
                }),
                "/v1/responses",
            ),
            "claude:chat" | "claude:cli" => (
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
            "gemini:chat" | "gemini:cli" => (
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
            "openai:chat" | "openai:cli" | "claude:chat" | "claude:cli" => {
                assert_eq!(
                    converted
                        .get("stream")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    upstream_is_stream,
                    "{provider_api_format} stream flag should follow upstream_is_stream"
                );
            }
            "gemini:chat" | "gemini:cli" => {
                assert!(
                    converted.get("stream").is_none(),
                    "gemini streaming is represented by endpoint URL, not request body"
                );
            }
            other => panic!("unexpected provider api format: {other}"),
        }
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
            "claude:chat",
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
            "gemini:chat",
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
            "gemini:chat",
            "claude-sonnet-4-5",
            "anthropic",
            "claude:chat",
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
            "claude:cli",
            "gemini-2.5-pro",
            "google",
            "gemini:cli",
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
            "openai:cli",
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
            "gemini:chat",
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
            "claude:chat",
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
}
