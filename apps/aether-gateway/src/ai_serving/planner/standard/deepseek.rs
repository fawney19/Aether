use serde_json::{json, Value};

pub(crate) fn is_deepseek_provider(provider_type: &str, base_url: &str) -> bool {
    let provider_type = provider_type.trim().to_ascii_lowercase();
    if matches!(
        provider_type.as_str(),
        "deepseek" | "deepseek_openai" | "deepseek_anthropic" | "deepseek_compatible"
    ) {
        return true;
    }

    let host = base_url_host(base_url);
    host == "deepseek.com" || host.ends_with(".deepseek.com")
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

fn base_url_host(base_url: &str) -> String {
    let lower = base_url.trim().to_ascii_lowercase();
    let without_scheme = lower
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(lower.as_str());
    let without_userinfo = without_scheme
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(without_scheme);
    without_userinfo
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string()
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
        return;
    }

    let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
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
        if message_object
            .get("reasoning_content")
            .is_some_and(|value| !value.is_null())
        {
            continue;
        }
        message_object.insert(
            "reasoning_content".to_string(),
            Value::String(String::new()),
        );
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

    use super::{apply_deepseek_tool_call_thinking_compat, is_deepseek_provider};

    #[test]
    fn detects_deepseek_provider_by_type_or_host() {
        assert!(is_deepseek_provider(
            "deepseek",
            "https://relay.example.com"
        ));
        assert!(is_deepseek_provider(
            "custom",
            "https://api.deepseek.com/v1"
        ));
        assert!(!is_deepseek_provider(
            "custom",
            "https://example.com/deepseek"
        ));
    }

    #[test]
    fn openai_chat_deepseek_adds_thinking_and_empty_reasoning_content() {
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
        assert_eq!(body["messages"][1]["reasoning_content"], "");
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
