use serde_json::{json, Value};

pub(crate) fn is_deepseek_provider(_provider_type: &str, base_url: &str) -> bool {
    let Some(host) = base_url_host(base_url) else {
        return false;
    };
    // 仅官方接口启用专用兼容；供应商类型和模型名称不能代表第三方接口的行为。
    host == "api.deepseek.com"
}

pub(crate) fn openai_responses_reasoning_replay_policy(
    provider_type: &str,
    base_url: &str,
    _provider_model: &str,
) -> crate::ai_serving::OpenAiResponsesReasoningReplayPolicy {
    if is_deepseek_provider(provider_type, base_url) {
        crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
    } else {
        crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
    }
}

pub(crate) fn apply_deepseek_tool_call_thinking_compat(
    provider_request_body: &mut Value,
    provider_type: &str,
    base_url: &str,
    provider_api_format: &str,
    original_request_body: Option<&Value>,
) {
    if !is_deepseek_provider(provider_type, base_url) {
        return;
    }

    match crate::ai_serving::normalize_api_format_alias(provider_api_format).as_str() {
        "openai:chat" => {
            apply_deepseek_openai_chat_thinking_compat(provider_request_body, original_request_body)
        }
        "claude:messages" => apply_deepseek_claude_messages_thinking_compat(
            provider_request_body,
            original_request_body,
        ),
        _ => {}
    }
}

fn base_url_host(base_url: &str) -> Option<String> {
    let base_url = base_url.trim();
    if base_url.is_empty() {
        return None;
    }

    // Provider configuration historically accepted both absolute URLs and a
    // bare authority/path. Use a real URL parser for both forms: hand-parsing
    // userinfo with `rsplit_once('@')` can mistake an `@` in the path or query
    // for the authority delimiter and classify an attacker-controlled host as
    // `api.deepseek.com`.
    if let Ok(parsed) = url::Url::parse(base_url) {
        if let Some(host) = parsed
            .host_str()
            .filter(|_| matches!(parsed.scheme(), "http" | "https" | "ws" | "wss"))
        {
            return Some(host.to_ascii_lowercase());
        }
        if base_url.contains("://") {
            return None;
        }
    }

    url::Url::parse(&format!("https://{base_url}"))
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

fn source_disables_thinking(
    original_request_body: Option<&Value>,
    provider_request_body: &Value,
) -> bool {
    request_explicitly_disables_thinking(provider_request_body)
        || original_request_body.is_some_and(request_explicitly_disables_thinking)
}

fn request_explicitly_disables_thinking(body: &Value) -> bool {
    thinking_type(body).is_some_and(|value| value.eq_ignore_ascii_case("disabled"))
        || reasoning_effort(body).is_some_and(|value| value.eq_ignore_ascii_case("none"))
}

fn thinking_type(body: &Value) -> Option<&str> {
    body.get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn reasoning_effort(body: &Value) -> Option<&str> {
    body.get("reasoning_effort")
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("reasoning")
                .and_then(Value::as_object)
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn set_deepseek_thinking_type(body: &mut Value, thinking_type: &str) {
    let Some(object) = body.as_object_mut() else {
        return;
    };

    match object.get_mut("thinking") {
        Some(Value::Object(thinking)) => {
            thinking.insert("type".to_string(), Value::String(thinking_type.to_string()));
        }
        _ => {
            object.insert(
                "thinking".to_string(),
                json!({
                    "type": thinking_type,
                }),
            );
        }
    }
}

fn apply_deepseek_openai_chat_thinking_compat(
    provider_request_body: &mut Value,
    original_request_body: Option<&Value>,
) {
    // 携带 tools 时，所有历史 reasoning_content 都须完整回传，包括未调用工具的轮次。
    // 无 tools 时允许回传，且 prefix 续写需要保留输入；因此原样保留 messages，
    // 不删除思考内容，也不以空字符串冒充缺失内容，由上游校验请求是否完整。
    let disabled = source_disables_thinking(original_request_body, provider_request_body);
    set_deepseek_thinking_type(
        provider_request_body,
        if disabled { "disabled" } else { "enabled" },
    );

    let Some(object) = provider_request_body.as_object_mut() else {
        return;
    };
    if disabled {
        if reasoning_effort(&Value::Object(object.clone()))
            .is_some_and(|value| value.eq_ignore_ascii_case("none"))
        {
            object.remove("reasoning_effort");
        }
    }
}

fn apply_deepseek_claude_messages_thinking_compat(
    provider_request_body: &mut Value,
    original_request_body: Option<&Value>,
) {
    if source_disables_thinking(original_request_body, provider_request_body) {
        set_deepseek_thinking_type(provider_request_body, "disabled");
        return;
    }

    let Some(messages) = provider_request_body
        .get_mut("messages")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for message in messages {
        let Some(message_object) = message.as_object_mut() else {
            continue;
        };
        let is_assistant = message_object
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|role| role.trim().eq_ignore_ascii_case("assistant"));
        if !is_assistant {
            continue;
        }
        ensure_claude_assistant_message_has_thinking_block(message_object);
    }
}

fn ensure_claude_assistant_message_has_thinking_block(
    message: &mut serde_json::Map<String, Value>,
) {
    let thinking_block = json!({
        "type": "thinking",
        "thinking": "",
    });
    match message.get_mut("content") {
        Some(Value::Array(blocks)) => {
            if blocks.iter().any(is_claude_thinking_block) {
                return;
            }
            blocks.insert(0, thinking_block);
        }
        Some(Value::String(text)) => {
            let text = std::mem::take(text);
            message.insert(
                "content".to_string(),
                Value::Array(vec![
                    thinking_block,
                    json!({
                        "type": "text",
                        "text": text,
                    }),
                ]),
            );
        }
        Some(Value::Null) | None => {
            message.insert("content".to_string(), Value::Array(vec![thinking_block]));
        }
        Some(other) => {
            let existing = std::mem::take(other);
            message.insert(
                "content".to_string(),
                Value::Array(vec![thinking_block, existing]),
            );
        }
    }
}

fn is_claude_thinking_block(block: &Value) -> bool {
    block
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|block_type| block_type.trim().eq_ignore_ascii_case("thinking"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        apply_deepseek_tool_call_thinking_compat, is_deepseek_provider,
        openai_responses_reasoning_replay_policy,
    };

    #[test]
    fn detects_deepseek_provider_only_by_official_host() {
        assert!(!is_deepseek_provider(
            "deepseek",
            "https://relay.example.com"
        ));
        assert!(is_deepseek_provider(
            "custom",
            "https://api.deepseek.com/v1"
        ));
        assert!(is_deepseek_provider("custom", "api.deepseek.com/v1"));
        assert!(is_deepseek_provider("custom", "api.deepseek.com:443/v1"));
        assert!(!is_deepseek_provider("custom", "https://deepseek.com"));
        assert!(!is_deepseek_provider("custom", "deepseek.com/v1"));
        assert!(is_deepseek_provider(
            "custom",
            " HTTPS://API.DEEPSEEK.COM:443/beta "
        ));
        assert!(!is_deepseek_provider(
            "deepseek",
            "https://other.deepseek.com/v1"
        ));
        assert!(!is_deepseek_provider(
            "custom",
            "https://example.com/deepseek"
        ));
        assert!(!is_deepseek_provider(
            "custom",
            "https://api.deepseek.com.evil.example/v1"
        ));
        assert!(!is_deepseek_provider(
            "custom",
            "https://api.deepseek.com@evil.example/v1"
        ));
        assert!(!is_deepseek_provider(
            "custom",
            "https://evil.example/path@api.deepseek.com/v1"
        ));
        assert!(!is_deepseek_provider(
            "custom",
            "https://evil.example/?relay=@api.deepseek.com"
        ));
        assert!(!is_deepseek_provider("custom", "ftp://api.deepseek.com/v1"));
        assert!(!is_deepseek_provider("deepseek", ""));
        assert_eq!(
            openai_responses_reasoning_replay_policy(
                "deepseek",
                "https://deepseek.com/v1",
                "deepseek-chat",
            ),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
        );
        assert_eq!(
            openai_responses_reasoning_replay_policy(
                "custom",
                "https://api.deepseek.com/v1",
                "deepseek-v4-flash",
            ),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
        );
        assert_eq!(
            openai_responses_reasoning_replay_policy(
                "openai",
                "https://api.openai.com/v1",
                "gpt-5.6-sol",
            ),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
        );
        assert_eq!(
            openai_responses_reasoning_replay_policy(
                "custom",
                "https://api.b.ai/v1",
                "deepseek-v4-flash",
            ),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
        );
        assert_eq!(
            openai_responses_reasoning_replay_policy(
                "custom",
                "https://api.b.ai/v1",
                "not-deepseek-compatible",
            ),
            crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
        );
    }

    #[test]
    fn official_deepseek_host_enables_compat_without_type_or_model_hints() {
        for base_url in [
            "https://api.deepseek.com/v1",
            "https://api.deepseek.com/beta",
        ] {
            let mut body = json!({
                "model": "mapped-model",
                "messages": [{"role": "assistant", "content": "answer"}]
            });

            apply_deepseek_tool_call_thinking_compat(
                &mut body,
                "custom",
                base_url,
                "openai:chat",
                None,
            );

            assert_eq!(body["thinking"]["type"], "enabled");
            assert_eq!(
                openai_responses_reasoning_replay_policy("custom", base_url, "mapped-model"),
                crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::DeepSeekOpaque
            );
        }
    }

    #[test]
    fn custom_deepseek_host_preserves_production_shaped_opaque_reasoning_replay() {
        let reasoning_items = (0..66)
            .map(|index| {
                json!({
                    "type": "reasoning",
                    "encrypted_content": format!("550e8400-e29b-41d4-a716-{index:012}"),
                    "content": [{
                        "type": "reasoning_text",
                        "text": format!("opaque DeepSeek reasoning {index}")
                    }]
                })
            })
            .collect::<Vec<_>>();
        let request = json!({
            "model": "deepseek-v4-flash",
            "input": reasoning_items.clone(),
            "future_request_field": {"preserve": true}
        });
        let replay_policy = openai_responses_reasoning_replay_policy(
            "custom",
            "https://api.deepseek.com/v1",
            "deepseek-v4-flash",
        );
        let mut provider_body = crate::ai_serving::build_standard_request_body_with_model_directives_and_request_headers_and_reasoning_replay_policy(
            &request,
            "openai:responses",
            "deepseek-v4-flash",
            "custom",
            "openai:responses",
            "/v1/responses",
            false,
            None,
            None,
            None,
            false,
            replay_policy,
        )
        .expect("custom DeepSeek Responses body should build");
        crate::ai_serving::finalize_openai_provider_request_with_codex_model_capabilities_and_reasoning_replay_policy(
            &mut provider_body,
            crate::ai_serving::OpenAiProviderRequestFinalization {
                source_api_format: "openai:responses",
                provider_api_format: "openai:responses",
                provider_type: "custom",
                provider_model: "deepseek-v4-flash",
                source_model: "deepseek-v4-flash",
                body_rules: None,
                upstream_is_stream: false,
                require_body_stream_field: false,
            },
            None,
            replay_policy,
        )
        .expect("custom DeepSeek finalization should accept opaque reasoning replay");
        assert_eq!(provider_body["input"].as_array().map(Vec::len), Some(66));
        assert_eq!(provider_body["future_request_field"]["preserve"], true);

        let mut deepseek = json!({"input": reasoning_items.clone()});
        let mut openai = json!({"input": reasoning_items});

        assert_eq!(
            crate::ai_serving::strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut deepseek,
                "openai:responses",
                openai_responses_reasoning_replay_policy(
                    "custom",
                    "https://api.deepseek.com/v1",
                    "deepseek-v4-flash",
                ),
            ),
            0
        );
        assert_eq!(deepseek["input"].as_array().map(Vec::len), Some(66));

        assert_eq!(
            crate::ai_serving::strip_incompatible_openai_responses_reasoning_items_with_policy(
                &mut openai,
                "openai:responses",
                openai_responses_reasoning_replay_policy(
                    "openai",
                    "https://api.openai.com/v1",
                    "gpt-5.6-sol",
                ),
            ),
            66
        );
        assert_eq!(openai["input"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn openai_chat_deepseek_enables_thinking_without_fabricating_reasoning() {
        let mut body = json!({
            "model": "deepseek-chat",
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{}"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": "{}"}
            ]
        });

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "deepseek",
            "https://api.deepseek.com/v1",
            "openai:chat",
            None,
        );

        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn custom_relay_deepseek_model_preserves_chat_request() {
        let mut body = json!({
            "model": "deepseek-v4-flash",
            "messages": [
                {"role": "user", "content": "inspect the repository"},
                {"role": "assistant", "content": null, "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "inspect", "arguments": "{}"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": "done"}
            ]
        });
        let original = body.clone();

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "custom",
            "https://api.b.ai/v1",
            "openai:chat",
            None,
        );

        assert_eq!(body, original);
    }

    #[test]
    fn third_party_hosts_ignore_deepseek_type_and_model_hints() {
        for provider_type in [
            "custom",
            "deepseek",
            "deepseek_openai",
            "deepseek_anthropic",
            "deepseek_compatible",
        ] {
            for provider_model in [
                "other-model",
                "deepseek-chat",
                "deepseek-reasoner",
                "deepseek-v3",
                "deepseek-v4-flash",
                "vendor/deepseek-chat",
                "vendor:deepseek-reasoner",
            ] {
                let base_url = "https://relay.example.com/v1";
                assert!(!is_deepseek_provider(provider_type, base_url));
                assert_eq!(
                    openai_responses_reasoning_replay_policy(
                        provider_type,
                        base_url,
                        provider_model
                    ),
                    crate::ai_serving::OpenAiResponsesReasoningReplayPolicy::OpenAiItemIds
                );

                for api_format in ["openai:chat", "claude:messages"] {
                    let original = json!({
                        "model": provider_model,
                        "messages": [{
                            "role": "assistant",
                            "content": "answer",
                            "reasoning_content": "original plan"
                        }]
                    });
                    let mut body = original.clone();

                    apply_deepseek_tool_call_thinking_compat(
                        &mut body,
                        provider_type,
                        base_url,
                        api_format,
                        None,
                    );

                    assert_eq!(
                        body, original,
                        "{provider_type} / {provider_model} / {api_format}"
                    );
                }
            }
        }
    }

    #[test]
    fn openai_chat_deepseek_preserves_history_without_tools() {
        let mut body = json!({
            "model": "deepseek-chat",
            "messages": [
                {"role": "user", "content": "Compare 9.11 and 9.8"},
                {
                    "role": "assistant",
                    "content": "9.8 is greater",
                    "reasoning_content": "Compare the decimal places.\n9.80 > 9.11."
                },
                {"role": "user", "content": "Explain again"},
                {"role": "assistant", "content": "Compare 9.80 with 9.11"}
            ]
        });
        let messages = body["messages"].clone();

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "deepseek",
            "https://api.deepseek.com/v1",
            "openai:chat",
            None,
        );

        assert_eq!(body["messages"], messages);
    }

    #[test]
    fn openai_chat_deepseek_preserves_reasoning_across_all_tool_turns() {
        let mut body = json!({
            "model": "deepseek-chat",
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "parameters": {"type": "object", "properties": {}}
                }
            }],
            "messages": [
                {"role": "user", "content": "What is the weather?"},
                {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "Check the weather before answering.\nKeep this full plan.",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{}"}
                    }]
                },
                {"role": "tool", "tool_call_id": "call_1", "content": "Cloudy"},
                {
                    "role": "assistant",
                    "content": "It is cloudy",
                    "reasoning_content": "The weather result is available; summarize it."
                },
                {"role": "user", "content": "Should I take an umbrella?"},
                {
                    "role": "assistant",
                    "content": "An umbrella may be useful",
                    "reasoning_content": "Use the previous weather result without another tool call."
                },
                {"role": "user", "content": "Why?"}
            ]
        });
        let messages = body["messages"].clone();
        let tools = body["tools"].clone();

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "deepseek",
            "https://api.deepseek.com/v1",
            "openai:chat",
            None,
        );

        assert_eq!(body["messages"], messages);
        assert_eq!(body["tools"], tools);
        assert_eq!(body["thinking"]["type"], "enabled");
    }

    #[test]
    fn openai_chat_deepseek_does_not_fabricate_missing_tool_reasoning() {
        for tools in [
            json!([]),
            json!([{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "parameters": {"type": "object", "properties": {}}
                }
            }]),
        ] {
            let mut body = json!({
                "model": "deepseek-chat",
                "tools": tools,
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": "missing"},
                    {"role": "assistant", "content": "null", "reasoning_content": null},
                    {"role": "assistant", "content": "empty", "reasoning_content": ""},
                    {"role": "assistant", "content": "answer", "reasoning_content": "original plan"}
                ]
            });
            let messages = body["messages"].clone();

            apply_deepseek_tool_call_thinking_compat(
                &mut body,
                "deepseek",
                "https://api.deepseek.com/v1",
                "openai:chat",
                None,
            );

            assert_eq!(body["messages"], messages);
        }
    }

    #[test]
    fn openai_chat_deepseek_preserves_reasoning_prefix_without_tools() {
        let mut body = json!({
            "model": "deepseek-chat",
            "messages": [
                {"role": "user", "content": "What is 1 + 1?"},
                {
                    "role": "assistant",
                    "prefix": true,
                    "content": "",
                    "reasoning_content": "Start by adding one to one."
                }
            ]
        });
        let messages = body["messages"].clone();

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "deepseek",
            "https://api.deepseek.com/beta",
            "openai:chat",
            None,
        );

        assert_eq!(body["messages"], messages);
        assert_eq!(body["thinking"]["type"], "enabled");
    }

    #[test]
    fn custom_relay_non_deepseek_model_is_not_rewritten() {
        let original = json!({
            "model": "not-deepseek-compatible",
            "messages": [{"role": "assistant", "content": "done"}]
        });
        let mut body = original.clone();

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "custom",
            "https://api.b.ai/v1",
            "openai:chat",
            None,
        );

        assert_eq!(body, original);
    }

    #[test]
    fn openai_chat_deepseek_honors_disabled_thinking() {
        let original = json!({"reasoning_effort": "none"});
        let mut body = json!({
            "model": "deepseek-chat",
            "reasoning_effort": "none",
            "messages": [
                {"role": "assistant", "content": "hi"}
            ]
        });

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "deepseek",
            "https://api.deepseek.com/v1",
            "openai:chat",
            Some(&original),
        );

        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("reasoning_effort").is_none());
        assert!(body["messages"][0].get("reasoning_content").is_none());
    }

    #[test]
    fn claude_messages_deepseek_prepends_empty_thinking_block() {
        let mut body = json!({
            "model": "deepseek-3.2",
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "call_1", "name": "lookup", "input": {}}
                ]}
            ]
        });

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "deepseek",
            "https://api.deepseek.com",
            "claude:messages",
            None,
        );

        assert_eq!(body["messages"][1]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][1]["content"][0]["thinking"], "");
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
    }

    #[test]
    fn claude_messages_deepseek_converts_string_assistant_content_to_blocks() {
        let mut body = json!({
            "model": "deepseek-3.2",
            "messages": [{
                "role": "assistant",
                "content": "done"
            }]
        });

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "deepseek",
            "https://api.deepseek.com",
            "claude:messages",
            None,
        );

        assert_eq!(body["messages"][0]["content"][0]["type"], "thinking");
        assert_eq!(body["messages"][0]["content"][1]["type"], "text");
        assert_eq!(body["messages"][0]["content"][1]["text"], "done");
    }

    #[test]
    fn claude_messages_deepseek_preserves_existing_thinking_block() {
        let mut body = json!({
            "model": "deepseek-3.2",
            "messages": [{
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "plan", "signature": "sig"},
                    {"type": "text", "text": "answer"}
                ]
            }]
        });

        apply_deepseek_tool_call_thinking_compat(
            &mut body,
            "deepseek",
            "https://api.deepseek.com",
            "claude:messages",
            None,
        );

        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][0]["content"][0]["thinking"], "plan");
        assert_eq!(body["messages"][0]["content"][0]["signature"], "sig");
    }
}
