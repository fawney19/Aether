use std::borrow::Cow;
use std::collections::BTreeMap;

use aether_ai_formats::formats::conversion::request::{
    convert_openai_chat_request_to_claude_request, convert_openai_chat_request_to_gemini_request,
    convert_openai_chat_request_to_openai_responses_request,
    normalize_claude_request_to_openai_chat_request,
    normalize_gemini_request_to_openai_chat_request,
    normalize_openai_responses_request_to_openai_chat_request,
    normalize_openai_responses_request_to_openai_chat_request_with_history_scope,
};
use aether_ai_formats::{request_conversion_kind, FormatContext, RequestConversionKind};
use serde_json::{json, Value};

use crate::formats::openai::responses::history::expand_previous_response_for_chat;
use crate::formats::shared::model_directives::apply_model_directive_overrides_from_request;
use crate::protocol::canonical::{
    openai_responses_input_to_canonical_messages, CanonicalContentBlock,
};

fn is_responses_shaped_body_on_chat_endpoint(body_json: &Value) -> bool {
    body_json
        .as_object()
        .is_some_and(|object| !object.contains_key("messages") && object.contains_key("input"))
}

pub fn is_claude_messages_shaped_body_on_openai_chat_endpoint(body_json: &Value) -> bool {
    let Some(request) = body_json.as_object() else {
        return false;
    };
    if !request.contains_key("messages") {
        return false;
    }
    request
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| tools.iter().any(is_claude_native_tool_definition))
        || request
            .get("messages")
            .and_then(Value::as_array)
            .is_some_and(|messages| messages.iter().any(message_has_claude_tool_block))
}

fn is_claude_native_tool_definition(tool: &Value) -> bool {
    tool.as_object().is_some_and(|tool_object| {
        tool_object.contains_key("input_schema") && !tool_object.contains_key("function")
    })
}

fn message_has_claude_tool_block(message: &Value) -> bool {
    message
        .as_object()
        .and_then(|object| object.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|parts| parts.iter().any(is_claude_tool_content_block))
}

fn is_claude_tool_content_block(part: &Value) -> bool {
    part.as_object()
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|block_type| matches!(block_type, "tool_use" | "tool_result"))
}

fn chat_compatible_body_for_openai_chat_endpoint(body_json: &Value) -> Option<Cow<'_, Value>> {
    if is_responses_shaped_body_on_chat_endpoint(body_json) {
        return normalize_openai_responses_request_to_openai_chat_request(body_json)
            .map(Cow::Owned);
    }
    Some(Cow::Borrowed(body_json))
}

/// Re-attaches Gemini tool-call thought signatures lost on a Responses -> Chat hop.
///
/// Responses clients replay Gemini signatures through reasoning `encrypted_content`
/// carriers, but OpenAI Chat has no field for them. Without this, the Chat -> Gemini
/// leg falls back to the `skip_thought_signature_validator` placeholder on every
/// historical function call. Signatures are matched back by call id.
pub(crate) fn restore_gemini_tool_signatures_from_openai_responses(
    gemini_body: &mut Value,
    responses_body: &Value,
    history_scope: Option<&str>,
) -> usize {
    let signatures = openai_responses_gemini_tool_signatures(responses_body, history_scope);
    if signatures.is_empty() {
        return 0;
    }
    let Some(contents) = gemini_body
        .get_mut("contents")
        .and_then(Value::as_array_mut)
    else {
        return 0;
    };
    let mut restored = 0usize;
    for part in contents
        .iter_mut()
        .filter_map(|content| content.get_mut("parts").and_then(Value::as_array_mut))
        .flatten()
    {
        let Some(signature) = part
            .get("functionCall")
            .and_then(|call| call.get("id"))
            .and_then(Value::as_str)
            .and_then(|id| signatures.get(id))
        else {
            continue;
        };
        if let Some(part) = part.as_object_mut() {
            part.insert(
                "thoughtSignature".to_string(),
                Value::String(signature.clone()),
            );
            restored += 1;
        }
    }
    restored
}

pub(crate) fn is_openai_responses_source_format(api_format: &str) -> bool {
    matches!(
        aether_ai_formats::normalize_api_format_alias(api_format).as_str(),
        "openai:responses" | "openai:responses:compact"
    )
}

fn openai_responses_gemini_tool_signatures(
    body_json: &Value,
    history_scope: Option<&str>,
) -> BTreeMap<String, String> {
    // Replayed history lives behind previous_response_id; the Chat hop expands
    // it the same way, so the call ids line up with the converted body.
    let expanded = body_json
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.trim().is_empty())
        .then(|| expand_previous_response_for_chat(body_json, history_scope).ok())
        .flatten();
    let body_json = expanded.as_ref().unwrap_or(body_json);
    let Some(messages) = openai_responses_input_to_canonical_messages(body_json.get("input"))
    else {
        return BTreeMap::new();
    };
    messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| {
            let CanonicalContentBlock::ToolUse { id, extensions, .. } = block else {
                return None;
            };
            let gemini = extensions.get("gemini")?.as_object()?;
            let signature = gemini
                .get("thoughtSignature")
                .or_else(|| gemini.get("thought_signature"))?
                .as_str()
                .filter(|value| !value.is_empty())?;
            Some((id.clone(), signature.to_string()))
        })
        .collect()
}

pub(crate) fn chat_compatible_body_for_standard_source<'a>(
    body_json: &'a Value,
    client_api_format: &str,
    history_scope: Option<&str>,
) -> Option<Cow<'a, Value>> {
    match aether_ai_formats::normalize_api_format_alias(client_api_format).as_str() {
        "openai:chat" => chat_compatible_body_for_openai_chat_endpoint(body_json),
        "openai:responses" | "openai:responses:compact" => {
            normalize_openai_responses_request_to_openai_chat_request_with_history_scope(
                body_json,
                history_scope,
            )
            .map(Cow::Owned)
        }
        "claude:messages" => {
            normalize_claude_request_to_openai_chat_request(body_json).map(Cow::Owned)
        }
        "gemini:generate_content" => {
            normalize_gemini_request_to_openai_chat_request(body_json, "").map(Cow::Owned)
        }
        _ => None,
    }
}

pub fn build_local_openai_chat_request_body(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    build_local_openai_chat_request_body_with_model_directives(
        body_json,
        mapped_model,
        upstream_is_stream,
        false,
    )
}

pub fn build_local_openai_chat_request_body_with_model_directives(
    body_json: &Value,
    mapped_model: &str,
    upstream_is_stream: bool,
    enable_model_directives: bool,
) -> Option<Value> {
    let chat_body = chat_compatible_body_for_openai_chat_endpoint(body_json)?;
    let request_body_object = chat_body.as_object()?;
    let mut provider_request_body = serde_json::Map::from_iter(
        request_body_object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    provider_request_body.insert("model".to_string(), Value::String(mapped_model.to_string()));
    if upstream_is_stream {
        provider_request_body.insert("stream".to_string(), Value::Bool(true));
        match provider_request_body.get_mut("stream_options") {
            Some(Value::Object(stream_options)) => {
                stream_options.insert("include_usage".to_string(), Value::Bool(true));
            }
            _ => {
                provider_request_body.insert(
                    "stream_options".to_string(),
                    json!({
                        "include_usage": true,
                    }),
                );
            }
        }
    }
    let mut provider_request_body = with_model_directive_overrides(
        Value::Object(provider_request_body),
        "openai:chat",
        mapped_model,
        body_json,
        None,
        enable_model_directives,
    );
    let require_body_stream_field = body_json
        .as_object()
        .is_some_and(|object| object.contains_key("stream"));
    crate::formats::shared::request::enforce_request_body_stream_field(
        &mut provider_request_body,
        "openai:chat",
        upstream_is_stream,
        require_body_stream_field,
    );
    Some(provider_request_body)
}

pub fn build_cross_format_openai_chat_request_body(
    body_json: &Value,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    build_cross_format_openai_chat_request_body_with_model_directives(
        body_json,
        mapped_model,
        provider_api_format,
        upstream_is_stream,
        false,
    )
}

/// Provider-aware entry point for gateway Chat planners. Keep private schema
/// conversion policy in the format crate while retaining legacy behavior elsewhere.
pub fn build_cross_format_openai_chat_request_body_with_provider_context(
    body_json: &Value,
    mapped_model: &str,
    provider_type: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    enable_model_directives: bool,
    history_scope: Option<&str>,
) -> Option<Value> {
    if super::standard_matrix::preserves_gemini_tool_schemas(provider_type, provider_api_format) {
        return super::standard_matrix::build_standard_request_body_with_model_directives(
            body_json,
            "openai:chat",
            mapped_model,
            provider_type,
            provider_api_format,
            "",
            upstream_is_stream,
            None,
            history_scope,
            enable_model_directives,
        );
    }
    build_cross_format_openai_chat_request_body_with_model_directives(
        body_json,
        mapped_model,
        provider_api_format,
        upstream_is_stream,
        enable_model_directives,
    )
}

pub fn build_cross_format_openai_chat_request_body_with_model_directives(
    body_json: &Value,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    enable_model_directives: bool,
) -> Option<Value> {
    let conversion_kind = request_conversion_kind("openai:chat", provider_api_format)?;
    let provider_request_body = match conversion_kind {
        RequestConversionKind::ToClaudeStandard => {
            if is_claude_messages_shaped_body_on_openai_chat_endpoint(body_json) {
                convert_claude_compatible_chat_endpoint_request(
                    body_json,
                    mapped_model,
                    provider_api_format,
                    upstream_is_stream,
                )?
            } else {
                let chat_body = chat_compatible_body_for_openai_chat_endpoint(body_json)?;
                convert_openai_chat_request_to_claude_request(
                    chat_body.as_ref(),
                    mapped_model,
                    upstream_is_stream,
                )?
            }
        }
        RequestConversionKind::ToGeminiStandard => {
            let chat_body = chat_compatible_body_for_openai_chat_endpoint(body_json)?;
            convert_openai_chat_request_to_gemini_request(
                chat_body.as_ref(),
                mapped_model,
                upstream_is_stream,
            )?
        }
        RequestConversionKind::ToOpenAiResponses => {
            if is_responses_shaped_body_on_chat_endpoint(body_json) {
                build_local_openai_responses_request_body_with_model_directives(
                    body_json,
                    mapped_model,
                    upstream_is_stream,
                    enable_model_directives,
                )?
            } else {
                convert_openai_chat_request_to_openai_responses_request(
                    body_json,
                    mapped_model,
                    upstream_is_stream,
                    false,
                )?
            }
        }
        _ => return None,
    };
    let mut provider_request_body = with_model_directive_overrides(
        provider_request_body,
        provider_api_format,
        mapped_model,
        body_json,
        None,
        enable_model_directives,
    );
    let require_body_stream_field = body_json
        .as_object()
        .is_some_and(|object| object.contains_key("stream"));
    crate::formats::shared::request::enforce_request_body_stream_field(
        &mut provider_request_body,
        provider_api_format,
        upstream_is_stream,
        require_body_stream_field,
    );
    Some(provider_request_body)
}

fn convert_claude_compatible_chat_endpoint_request(
    body_json: &Value,
    mapped_model: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    aether_ai_formats::convert_request(
        "claude:messages",
        provider_api_format,
        body_json,
        &FormatContext::default()
            .with_mapped_model(mapped_model)
            .with_upstream_stream(upstream_is_stream),
    )
    .ok()
}

pub fn build_local_openai_responses_request_body(
    body_json: &Value,
    mapped_model: &str,
    require_streaming: bool,
) -> Option<Value> {
    build_local_openai_responses_request_body_with_model_directives(
        body_json,
        mapped_model,
        require_streaming,
        false,
    )
}

pub fn build_local_openai_responses_request_body_with_model_directives(
    body_json: &Value,
    mapped_model: &str,
    require_streaming: bool,
    enable_model_directives: bool,
) -> Option<Value> {
    let request_body_object = body_json.as_object()?;
    let mut provider_request_body = serde_json::Map::from_iter(
        request_body_object
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    provider_request_body.insert("model".to_string(), Value::String(mapped_model.to_string()));
    if require_streaming {
        provider_request_body.insert("stream".to_string(), Value::Bool(true));
    }
    let mut provider_request_body = with_model_directive_overrides(
        Value::Object(provider_request_body),
        "openai:responses",
        mapped_model,
        body_json,
        None,
        enable_model_directives,
    );
    let require_body_stream_field = body_json
        .as_object()
        .is_some_and(|object| object.contains_key("stream"));
    crate::formats::shared::request::enforce_request_body_stream_field(
        &mut provider_request_body,
        "openai:responses",
        require_streaming,
        require_body_stream_field,
    );
    Some(provider_request_body)
}

pub fn build_cross_format_openai_responses_request_body(
    body_json: &Value,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
) -> Option<Value> {
    build_cross_format_openai_responses_request_body_with_model_directives(
        body_json,
        mapped_model,
        client_api_format,
        provider_api_format,
        upstream_is_stream,
        false,
    )
}

pub fn build_cross_format_openai_responses_request_body_with_model_directives(
    body_json: &Value,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    enable_model_directives: bool,
) -> Option<Value> {
    build_cross_format_openai_responses_request_body_with_model_directives_and_history_scope(
        body_json,
        mapped_model,
        client_api_format,
        provider_api_format,
        upstream_is_stream,
        enable_model_directives,
        None,
    )
}

/// Provider-aware Responses entry point; preserve history scoping and defer
/// private tool schema lowering without exposing provider policy to the gateway.
#[allow(clippy::too_many_arguments)]
pub fn build_cross_format_openai_responses_request_body_with_provider_context(
    body_json: &Value,
    mapped_model: &str,
    client_api_format: &str,
    provider_type: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    enable_model_directives: bool,
    history_scope: Option<&str>,
) -> Option<Value> {
    if super::standard_matrix::preserves_gemini_tool_schemas(provider_type, provider_api_format) {
        return super::standard_matrix::build_standard_request_body_with_model_directives(
            body_json,
            client_api_format,
            mapped_model,
            provider_type,
            provider_api_format,
            "",
            upstream_is_stream,
            None,
            history_scope,
            enable_model_directives,
        );
    }
    build_cross_format_openai_responses_request_body_with_model_directives_and_history_scope(
        body_json,
        mapped_model,
        client_api_format,
        provider_api_format,
        upstream_is_stream,
        enable_model_directives,
        history_scope,
    )
}

pub fn build_cross_format_openai_responses_request_body_with_model_directives_and_history_scope(
    body_json: &Value,
    mapped_model: &str,
    client_api_format: &str,
    provider_api_format: &str,
    upstream_is_stream: bool,
    enable_model_directives: bool,
    history_scope: Option<&str>,
) -> Option<Value> {
    let chat_like_request =
        chat_compatible_body_for_standard_source(body_json, client_api_format, history_scope)?;
    let conversion_kind = request_conversion_kind(client_api_format, provider_api_format)?;
    let provider_request_body = match conversion_kind {
        RequestConversionKind::ToOpenAIChat => {
            build_local_openai_chat_request_body_with_model_directives(
                chat_like_request.as_ref(),
                mapped_model,
                upstream_is_stream,
                enable_model_directives,
            )?
        }
        RequestConversionKind::ToOpenAiResponses => {
            convert_openai_chat_request_to_openai_responses_request(
                chat_like_request.as_ref(),
                mapped_model,
                upstream_is_stream,
                false,
            )?
        }
        RequestConversionKind::ToClaudeStandard => convert_openai_chat_request_to_claude_request(
            chat_like_request.as_ref(),
            mapped_model,
            upstream_is_stream,
        )?,
        RequestConversionKind::ToGeminiStandard => {
            let mut provider_request_body = convert_openai_chat_request_to_gemini_request(
                chat_like_request.as_ref(),
                mapped_model,
                upstream_is_stream,
            )?;
            if is_openai_responses_source_format(client_api_format) {
                restore_gemini_tool_signatures_from_openai_responses(
                    &mut provider_request_body,
                    body_json,
                    history_scope,
                );
            }
            provider_request_body
        }
    };
    let mut provider_request_body = with_model_directive_overrides(
        provider_request_body,
        provider_api_format,
        mapped_model,
        body_json,
        None,
        enable_model_directives,
    );
    let require_body_stream_field = body_json
        .as_object()
        .is_some_and(|object| object.contains_key("stream"));
    crate::formats::shared::request::enforce_request_body_stream_field(
        &mut provider_request_body,
        provider_api_format,
        upstream_is_stream,
        require_body_stream_field,
    );
    Some(provider_request_body)
}

fn with_model_directive_overrides(
    mut provider_request_body: Value,
    provider_api_format: &str,
    provider_model: &str,
    request_body: &Value,
    request_path: Option<&str>,
    enable_model_directives: bool,
) -> Value {
    if enable_model_directives {
        apply_model_directive_overrides_from_request(
            &mut provider_request_body,
            provider_api_format,
            provider_model,
            request_body,
            request_path,
        );
    }
    provider_request_body
}

#[cfg(test)]
mod tests {
    use super::build_local_openai_responses_request_body;
    use super::{
        build_cross_format_openai_chat_request_body_with_model_directives,
        build_cross_format_openai_responses_request_body, build_local_openai_chat_request_body,
        build_local_openai_chat_request_body_with_model_directives,
        build_local_openai_responses_request_body_with_model_directives,
    };
    use serde_json::{json, Value};

    #[test]
    fn provider_context_builders_preserve_private_schemas_and_legacy_routes() {
        use crate::api::{
            build_cross_format_openai_chat_request_body_with_provider_context as chat,
            build_cross_format_openai_responses_request_body_with_provider_context as responses,
        };
        let schema = json!({"type":"object", "properties":{"mode":{"const":"fast"}}});
        let chat_input = json!({"model":"client", "messages":[{"role":"user","content":"hi"}],
            "tools":[{"type":"function","function":{"name":"probe","parameters":schema}}]});
        let responses_input = json!({"model":"client", "input":"hi",
            "tools":[{"type":"function","name":"probe","parameters":schema}]});
        for provider in ["antigravity", " AnTiGrAvItY ", "gemini", "openai"] {
            for target in [
                "gemini:generate_content",
                "claude:messages",
                "openai:responses",
            ] {
                for stream in [false, true] {
                    for directives in [false, true] {
                        for input in [&chat_input, &responses_input] {
                            let actual = chat(
                                input,
                                "claude-test",
                                provider,
                                target,
                                stream,
                                directives,
                                Some("seam-test"),
                            );
                            let expected =
                                if super::super::standard_matrix::preserves_gemini_tool_schemas(
                                    provider, target,
                                ) {
                                    super::super::standard_matrix::build_standard_request_body_with_model_directives(
                                    input, "openai:chat", "claude-test", provider, target, "", stream, None, Some("seam-test"), directives)
                                } else {
                                    super::build_cross_format_openai_chat_request_body_with_model_directives(
                                    input, "claude-test", target, stream, directives)
                                };
                            assert!(actual.is_some(), "chat {provider} {target}");
                            assert_eq!(actual, expected);
                            if target == "gemini:generate_content" {
                                assert_eq!(
                                    actual.unwrap()["tools"][0]["functionDeclarations"][0]
                                        ["parameters"]
                                        == schema,
                                    provider.trim().eq_ignore_ascii_case("antigravity")
                                );
                            }
                        }
                        let actual = responses(
                            &responses_input,
                            "claude-test",
                            "openai:responses",
                            provider,
                            target,
                            stream,
                            directives,
                            Some("seam-test"),
                        );
                        let expected =
                            if super::super::standard_matrix::preserves_gemini_tool_schemas(
                                provider, target,
                            ) {
                                super::super::standard_matrix::build_standard_request_body_with_model_directives(
                                &responses_input, "openai:responses", "claude-test", provider, target, "", stream, None, Some("seam-test"), directives)
                            } else {
                                super::build_cross_format_openai_responses_request_body_with_model_directives_and_history_scope(
                                &responses_input, "claude-test", "openai:responses", target, stream, directives, Some("seam-test"))
                            };
                        // Same-format Responses uses the local builder, not this cross-format API.
                        assert_eq!(
                            actual.is_some(),
                            target != "openai:responses",
                            "responses {provider} {target}"
                        );
                        assert_eq!(actual, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn responses_to_gemini_chat_hop_keeps_replayed_tool_signatures() {
        use crate::api::build_cross_format_openai_responses_request_body_with_provider_context as responses;
        use crate::formats::openai::responses::{
            encode_gemini_tool_signature_carrier_with_direction,
            GeminiToolSignatureCarrierDirection,
        };
        let carrier = |signature: &str, direction| {
            encode_gemini_tool_signature_carrier_with_direction(signature, direction).unwrap()
        };
        let input = json!({
            "model": "client",
            "include": ["reasoning.encrypted_content"],
            "input": [
                {"role": "user", "content": [{"type": "input_text", "text": "go"}]},
                {"type": "reasoning", "summary": [],
                 "encrypted_content": carrier("sig-a", GeminiToolSignatureCarrierDirection::Next)},
                // omp replays reasoning text between the carrier and its call.
                {"type": "reasoning", "summary": [],
                 "content": [{"type": "reasoning_text", "text": "plan a"}]},
                {"type": "function_call", "call_id": "call_a", "name": "read",
                 "arguments": "{\"path\":\"a\"}"},
                {"type": "function_call_output", "call_id": "call_a", "output": "A"},
                {"type": "function_call", "call_id": "call_b", "name": "read",
                 "arguments": "{\"path\":\"b\"}"},
                {"type": "reasoning", "summary": [],
                 "encrypted_content": carrier("sig-b", GeminiToolSignatureCarrierDirection::Previous)},
                {"type": "function_call_output", "call_id": "call_b", "output": "B"}
            ],
            "tools": [{"type": "function", "name": "read",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}}]
        });
        for provider in ["antigravity", "gemini"] {
            let output = responses(
                &input,
                "gemini-test",
                "openai:responses",
                provider,
                "gemini:generate_content",
                true,
                false,
                Some("signature-test"),
            )
            .unwrap();
            let signatures = output["contents"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|content| content["parts"].as_array().unwrap())
                .filter(|part| part.get("functionCall").is_some())
                .map(|part| {
                    (
                        part["functionCall"]["id"].as_str().unwrap(),
                        part["thoughtSignature"].as_str().unwrap(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                signatures,
                vec![("call_a", "sig-a"), ("call_b", "sig-b")],
                "{provider}"
            );
        }
    }

    #[test]
    fn provider_context_builders_keep_scoped_responses_history() {
        use crate::api::{
            build_cross_format_openai_chat_request_body_with_provider_context as chat,
            build_cross_format_openai_responses_request_body_with_provider_context as responses,
            record_converted_response_history,
        };
        let response_id = "resp_provider_context_seam_history";
        let scope = "provider-context-seam-history";
        record_converted_response_history(&json!({
            "needs_conversion":true, "client_api_format":"openai:responses",
            "provider_api_format":"openai:chat", "api_key_id":scope,
            "original_request_body":{"model":"client", "input":"first"}
        }), &json!({"id":response_id, "status":"completed", "output":[{
            "type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"remembered"}]
        }]})).expect("seed scoped history");
        let input = json!({"model":"client", "previous_response_id":response_id, "input":"second"});
        for use_chat in [false, true] {
            let build = |history_scope| {
                if use_chat {
                    chat(
                        &input,
                        "claude-test",
                        "antigravity",
                        "gemini:generate_content",
                        true,
                        false,
                        history_scope,
                    )
                } else {
                    responses(
                        &input,
                        "claude-test",
                        "openai:responses",
                        "antigravity",
                        "gemini:generate_content",
                        true,
                        false,
                        history_scope,
                    )
                }
            };
            if use_chat {
                // The legacy Chat alternate-shape path does not hydrate scoped
                // Responses history. Preserve that behavior during this refactor.
                assert!(build(Some(scope)).is_none());
                continue;
            }
            let output = build(Some(scope)).expect("expand scoped history");
            assert_eq!(output["contents"][0]["parts"][0]["text"], "first");
            assert_eq!(output["contents"][1]["parts"][0]["text"], "remembered");
            assert_eq!(output["contents"][2]["parts"][0]["text"], "second");
            assert!(build(Some("different-seam-key")).is_none());
        }
    }

    fn object_keys(value: &Value) -> Vec<&str> {
        value
            .as_object()
            .expect("json object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn builds_openai_chat_cross_format_request_body_from_openai_responses_source() {
        let body_json = json!({
            "model": "gpt-5",
            "input": "hello",
        });

        let provider_request_body = build_cross_format_openai_responses_request_body(
            &body_json,
            "gpt-5-upstream",
            "openai:responses",
            "openai:chat",
            false,
        )
        .expect("openai responses to openai chat body should build");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["messages"][0]["role"], "user");
        assert_eq!(provider_request_body["messages"][0]["content"], "hello");
    }

    #[test]
    fn local_openai_responses_request_body_preserves_original_field_order() {
        let body_json: Value = serde_json::from_str(
            r#"{
                "model": "gpt-5",
                "include": ["reasoning.encrypted_content"],
                "input": [],
                "instructions": "Keep order"
            }"#,
        )
        .expect("request json should parse");

        let provider_request_body =
            build_local_openai_responses_request_body(&body_json, "gpt-5-upstream", false)
                .expect("openai responses body should build");

        assert_eq!(
            object_keys(&provider_request_body),
            vec!["model", "include", "input", "instructions"]
        );
        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
    }

    #[test]
    fn local_openai_chat_request_body_accepts_responses_shape_from_chat_endpoint() {
        let body_json = json!({
            "model": "gpt-5",
            "stream": true,
            "input": [{"role": "user", "content": "hello"}],
            "tools": [{
                "type": "function",
                "name": "Shell",
                "parameters": {"type": "object"},
                "strict": false
            }],
            "reasoning": {"effort": "high"}
        });

        let provider_request_body =
            build_local_openai_chat_request_body(&body_json, "gpt-5-upstream", true)
                .expect("responses-shaped chat body should build as chat");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["messages"][0]["role"], "user");
        assert_eq!(provider_request_body["messages"][0]["content"], "hello");
        assert_eq!(
            provider_request_body["tools"][0]["function"]["name"],
            "Shell"
        );
        assert_eq!(provider_request_body["reasoning_effort"], "high");
        assert_eq!(provider_request_body["stream"], true);
        assert_eq!(
            provider_request_body["stream_options"]["include_usage"],
            true
        );
    }

    #[test]
    fn cross_format_openai_chat_request_body_preserves_responses_shape_for_responses_target() {
        let body_json = json!({
            "model": "gpt-5",
            "stream": true,
            "input": [{"role": "user", "content": "hello"}],
            "include": ["reasoning.encrypted_content"],
            "stream_options": {"include_usage": true},
            "tools": [{
                "type": "function",
                "name": "Shell",
                "parameters": {"type": "object"},
                "strict": false
            }, {
                "type": "function",
                "parameters": {"type": "object"}
            }]
        });

        let provider_request_body =
            build_cross_format_openai_chat_request_body_with_model_directives(
                &body_json,
                "gpt-5-upstream",
                "openai:responses",
                false,
                false,
            )
            .expect("responses-shaped chat body should build as responses");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["input"][0]["role"], "user");
        assert_eq!(provider_request_body["input"][0]["content"], "hello");
        assert_eq!(provider_request_body["tools"][0]["name"], "Shell");
        assert_eq!(provider_request_body["tools"][0]["strict"], false);
        assert_eq!(provider_request_body["tools"][1]["type"], "function");
        assert_eq!(
            provider_request_body["include"][0],
            "reasoning.encrypted_content"
        );
        assert_eq!(
            provider_request_body["stream_options"]["include_usage"],
            true
        );
        assert_eq!(provider_request_body["stream"], false);
        assert!(provider_request_body.get("messages").is_none());
    }

    #[test]
    fn cross_format_openai_responses_body_preserves_chat_messages_for_chat_source() {
        let body_json = json!({
            "model": "gpt-5.5",
            "messages": [
                {"role": "system", "content": "Return a JSON object."},
                {"role": "user", "content": "Explain why this JSON patch failed."}
            ],
            "response_format": {"type": "json_object"}
        });

        let provider_request_body = build_cross_format_openai_responses_request_body(
            &body_json,
            "gpt-5.5-upstream",
            "openai:chat",
            "openai:responses",
            false,
        )
        .expect("openai chat to openai responses body should build");

        assert_eq!(provider_request_body["model"], "gpt-5.5-upstream");
        assert_eq!(
            provider_request_body["text"]["format"]["type"],
            "json_object"
        );
        assert_eq!(provider_request_body["input"][0]["role"], "user");
        assert_eq!(
            provider_request_body["input"][0]["content"][0]["text"],
            "Explain why this JSON patch failed."
        );
        assert!(provider_request_body.get("messages").is_none());
    }

    #[test]
    fn openai_chat_request_body_prefers_messages_when_messages_and_input_are_both_present() {
        let body_json = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "from messages"}],
            "input": [{"role": "user", "content": "from input"}]
        });

        let provider_request_body =
            build_cross_format_openai_chat_request_body_with_model_directives(
                &body_json,
                "gpt-5-upstream",
                "openai:responses",
                false,
                false,
            )
            .expect("normal chat body should still use messages");

        assert_eq!(
            provider_request_body["input"][0]["content"][0]["text"],
            "from messages"
        );
    }

    #[test]
    fn builds_streaming_local_openai_chat_request_body_with_include_usage() {
        let body_json = json!({
            "model": "gpt-5",
            "messages": [{
                "role": "user",
                "content": "hello"
            }]
        });

        let provider_request_body =
            build_local_openai_chat_request_body(&body_json, "gpt-5-upstream", true)
                .expect("openai chat body should build");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["stream"], true);
        assert_eq!(
            provider_request_body["stream_options"]["include_usage"],
            true
        );
    }

    #[test]
    fn local_openai_chat_request_body_overrides_client_stream_for_non_stream_upstream() {
        let body_json = json!({
            "model": "gpt-5",
            "messages": [{
                "role": "user",
                "content": "hello"
            }],
            "stream": true
        });

        let provider_request_body =
            build_local_openai_chat_request_body(&body_json, "gpt-5-upstream", false)
                .expect("openai chat body should build");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["stream"], false);
    }

    #[test]
    fn local_openai_responses_request_body_overrides_client_stream_for_non_stream_upstream() {
        let body_json = json!({
            "model": "gpt-5",
            "input": "hello",
            "stream": true
        });

        let provider_request_body =
            build_local_openai_responses_request_body(&body_json, "gpt-5-upstream", false)
                .expect("openai responses body should build");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["stream"], false);
    }

    #[test]
    fn cross_format_openai_chat_request_body_overrides_client_stream_for_non_stream_upstream() {
        let body_json = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": true
        });

        let claude = build_cross_format_openai_chat_request_body_with_model_directives(
            &body_json,
            "claude-sonnet-4-5",
            "claude:messages",
            false,
            false,
        )
        .expect("claude body should build");
        assert_eq!(claude["stream"], false);

        let responses = build_cross_format_openai_chat_request_body_with_model_directives(
            &body_json,
            "gpt-5-upstream",
            "openai:responses",
            false,
            false,
        )
        .expect("responses body should build");
        assert_eq!(responses["stream"], false);
    }

    #[test]
    fn cross_format_openai_chat_request_body_does_not_add_stream_false_for_plain_sync_body() {
        let body_json = json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "hello"}]
        });

        let claude = build_cross_format_openai_chat_request_body_with_model_directives(
            &body_json,
            "claude-sonnet-4-5",
            "claude:messages",
            false,
            false,
        )
        .expect("claude body should build");
        assert!(claude.get("stream").is_none());
    }

    #[test]
    fn cross_format_openai_responses_body_overrides_client_stream_for_non_stream_upstream() {
        let body_json = json!({
            "model": "gpt-5",
            "input": "hello",
            "stream": true
        });

        let provider_request_body = build_cross_format_openai_responses_request_body(
            &body_json,
            "claude-sonnet-4-5",
            "openai:responses",
            "claude:messages",
            false,
        )
        .expect("claude body should build");

        assert_eq!(provider_request_body["stream"], false);
    }

    #[test]
    fn local_openai_chat_request_body_applies_reasoning_effort_suffix() {
        let body_json = json!({
            "model": "gpt-5.4-xhigh",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "low"
        });

        let provider_request_body = build_local_openai_chat_request_body_with_model_directives(
            &body_json,
            "gpt-5-upstream",
            false,
            true,
        )
        .expect("openai chat body should build");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["reasoning_effort"], "xhigh");
    }

    #[test]
    fn local_openai_chat_request_body_leaves_model_directive_disabled_by_default() {
        let body_json = json!({
            "model": "gpt-5.4-xhigh",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "low"
        });

        let provider_request_body =
            build_local_openai_chat_request_body(&body_json, "gpt-5-upstream", false)
                .expect("openai chat body should build");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["reasoning_effort"], "low");
    }

    #[test]
    fn local_openai_responses_request_body_applies_reasoning_effort_suffix() {
        let body_json = json!({
            "model": "gpt-5.6-sol-max",
            "input": "hello",
            "reasoning": {"effort": "low", "summary": "auto"}
        });

        let provider_request_body =
            build_local_openai_responses_request_body_with_model_directives(
                &body_json,
                "gpt-5-upstream",
                false,
                true,
            )
            .expect("openai responses body should build");

        assert_eq!(provider_request_body["model"], "gpt-5-upstream");
        assert_eq!(provider_request_body["reasoning"]["summary"], "auto");
        assert_eq!(provider_request_body["reasoning"]["effort"], "max");
    }

    #[test]
    fn cross_format_request_body_applies_reasoning_effort_suffix() {
        let body_json = json!({
            "model": "gpt-5.4-high",
            "messages": [{"role": "user", "content": "hello"}],
            "reasoning_effort": "low"
        });

        let provider_request_body =
            build_cross_format_openai_chat_request_body_with_model_directives(
                &body_json,
                "claude-sonnet-4-5",
                "claude:messages",
                false,
                true,
            )
            .expect("claude body should build");

        assert_eq!(provider_request_body["model"], "claude-sonnet-4-5");
        assert_eq!(provider_request_body["output_config"]["effort"], "high");
        assert_eq!(provider_request_body["thinking"]["budget_tokens"], 4096);
    }

    #[test]
    fn streaming_local_openai_chat_request_body_preserves_stream_options_while_forcing_include_usage(
    ) {
        let body_json = json!({
            "model": "gpt-5",
            "messages": [{
                "role": "user",
                "content": "hello"
            }],
            "stream_options": {
                "include_usage": false,
                "extra": "keep-me"
            }
        });

        let provider_request_body =
            build_local_openai_chat_request_body(&body_json, "gpt-5-upstream", true)
                .expect("openai chat body should build");

        assert_eq!(
            provider_request_body["stream_options"]["include_usage"],
            true
        );
        assert_eq!(provider_request_body["stream_options"]["extra"], "keep-me");
    }

    #[test]
    fn cross_format_openai_chat_request_body_accepts_claude_native_messages_for_claude_target() {
        let body_json = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {"role": "user", "content": "lookup"},
                {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "checking"},
                        {
                            "type": "tool_use",
                            "id": "call_1",
                            "name": "lookup",
                            "input": {"q": "db"}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "call_1",
                        "content": {"rows": 1}
                    }]
                }
            ],
            "tools": [{
                "name": "lookup",
                "description": "Lookup data",
                "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}}
            }],
            "tool_choice": {"type": "auto"},
            "max_tokens": 128,
            "stream": true
        });

        let provider_request_body =
            build_cross_format_openai_chat_request_body_with_model_directives(
                &body_json,
                "claude-sonnet-4-5",
                "claude:messages",
                true,
                false,
            )
            .expect("claude-native chat endpoint body should build as claude messages");

        assert_eq!(provider_request_body["model"], "claude-sonnet-4-5");
        assert_eq!(provider_request_body["tools"][0]["name"], "lookup");
        assert_eq!(
            provider_request_body["tools"][0]["input_schema"]["properties"]["q"]["type"],
            "string"
        );
        assert_eq!(
            provider_request_body["messages"][1]["content"][1]["type"],
            "tool_use"
        );
        assert_eq!(
            provider_request_body["messages"][2]["content"][0]["type"],
            "tool_result"
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                provider_request_body["messages"][2]["content"][0]["content"]
                    .as_str()
                    .expect("object tool result content should be serialized for Claude")
            )
            .expect("serialized tool result content should remain JSON"),
            json!({"rows": 1})
        );
        assert_eq!(provider_request_body["tool_choice"]["type"], "auto");
        assert_eq!(provider_request_body["stream"], true);
    }
}
