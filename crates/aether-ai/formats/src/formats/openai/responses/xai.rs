use serde_json::{json, Map, Value};

const XAI_RESPONSES_UNSUPPORTED_BODY_FIELDS: &[&str] = &[
    "previous_response_id",
    "prompt_cache_retention",
    "safety_identifier",
    "stream_options",
    "stop",
    "metadata",
];
const XAI_WEB_SEARCH_TOOL_TYPE: &str = "web_search";
const XAI_IMAGE_GENERATION_TOOL_TYPE: &str = "image_generation";
const XAI_TOOL_SEARCH_TOOL_TYPE: &str = "tool_search";
const XAI_GROK_IMAGE_GENERATION_MIN: XaiGrokVersion = XaiGrokVersion { major: 4, minor: 6 };

#[derive(Clone, Copy)]
struct XaiGrokVersion {
    major: i32,
    minor: i32,
}

pub fn apply_xai_upstream_payload_edits(
    body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
) {
    apply_xai_upstream_payload_edits_with_client(
        body,
        provider_type,
        provider_api_format,
        None,
        None,
    );
}

pub fn apply_xai_upstream_payload_edits_with_client(
    body: &mut Value,
    provider_type: &str,
    provider_api_format: &str,
    client_api_format: Option<&str>,
    client_body: Option<&Value>,
) {
    if !provider_type.trim().eq_ignore_ascii_case("xai") {
        return;
    }
    normalize_xai_image_refs(body);
    if crate::is_openai_responses_family_format(provider_api_format) {
        restore_xai_web_search_from_client(body, client_api_format, client_body);
        sanitize_xai_responses_body(body);
    }
}

fn sanitize_xai_responses_body(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    for field in XAI_RESPONSES_UNSUPPORTED_BODY_FIELDS {
        object.remove(*field);
    }
    let keep_image_generation = object
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(xai_supports_native_image_generation);
    normalize_xai_tool_arrays(object, keep_image_generation);
    rewrite_xai_web_search_tool_choice(object);
    prune_xai_orphaned_tool_choice(object);
    rewrite_xai_image_generation_tool_choice(object);
    drop_tool_choice_without_tools(object);
    strip_unsupported_reasoning_effort(object);
    sanitize_xai_input_encrypted_content(object);
}

fn restore_xai_web_search_from_client(
    body: &mut Value,
    client_api_format: Option<&str>,
    client_body: Option<&Value>,
) {
    let Some(client_api_format) = client_api_format else {
        return;
    };
    let Some(client_body) = client_body else {
        return;
    };
    if !client_requests_web_search(client_api_format, client_body) {
        return;
    }
    ensure_xai_web_search_tool(body);
    // Claude names a hosted tool in tool_choice just like a client function.
    // Resolve that name against the original declaration, never by name alone.
    if crate::normalize_api_format_alias(client_api_format) == "claude:messages" {
        let choice = &client_body["tool_choice"];
        if choice["type"] == "tool"
            && choice["name"].as_str().is_some_and(|name| {
                request_tools(client_body)
                    .iter()
                    .any(|tool| is_web_search_tool(tool) && tool_name(tool) == Some(name))
            })
        {
            body["tool_choice"] = json!({"type": XAI_WEB_SEARCH_TOOL_TYPE});
        }
    }
}

fn client_requests_web_search(client_api_format: &str, client_body: &Value) -> bool {
    let format = crate::normalize_api_format_alias(client_api_format);
    match format.as_str() {
        "openai:chat" => {
            object_has_non_null_field(client_body, "web_search_options")
                || request_tools(client_body).iter().any(is_web_search_tool)
        }
        "claude:messages" => request_tools(client_body).iter().any(is_web_search_tool),
        "gemini:generate_content" => gemini_request_has_google_search(client_body),
        _ => false,
    }
}

fn gemini_request_has_google_search(body: &Value) -> bool {
    request_tools(body).iter().any(|tool| {
        tool.get("googleSearch").is_some()
            || tool.get("google_search").is_some()
            || tool
                .get("googleSearchRetrieval")
                .is_some_and(|value| !value.is_null())
    })
}

fn object_has_non_null_field(body: &Value, field: &str) -> bool {
    body.get(field).is_some_and(|value| !value.is_null())
}

fn ensure_xai_web_search_tool(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    if tools_array(object).iter().any(is_web_search_tool) {
        return;
    }
    let tools = object
        .entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(tools) = tools.as_array_mut() {
        tools.push(json!({ "type": XAI_WEB_SEARCH_TOOL_TYPE }));
    }
}

fn normalize_xai_tool_arrays(object: &mut Map<String, Value>, keep_image_generation: bool) {
    if let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) {
        *tools = normalize_xai_tool_list(tools, keep_image_generation);
        if tools.is_empty() {
            object.remove("tools");
        }
    }
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in input {
        let Some(item_object) = item.as_object_mut() else {
            continue;
        };
        if item_object.get("type").and_then(Value::as_str) != Some("additional_tools") {
            continue;
        }
        if let Some(tools) = item_object.get_mut("tools").and_then(Value::as_array_mut) {
            *tools = normalize_xai_tool_list(tools, keep_image_generation);
        }
    }
}

fn normalize_xai_tool_list(tools: &[Value], keep_image_generation: bool) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| normalize_xai_tool(tool, keep_image_generation))
        .collect()
}

fn normalize_xai_tool(tool: &Value, keep_image_generation: bool) -> Option<Value> {
    let Some(object) = tool.as_object() else {
        return Some(tool.clone());
    };
    let tool_type = tool_type(tool).unwrap_or("function");
    if tool_type == XAI_TOOL_SEARCH_TOOL_TYPE {
        return None;
    }
    if tool_type == XAI_IMAGE_GENERATION_TOOL_TYPE && !keep_image_generation {
        return None;
    }
    if tool_type == "custom" && tool_name(tool).is_some_and(|name| name == "apply_patch") {
        return None;
    }

    let mut next = object.clone();
    if tool_type.starts_with("web_search") {
        next.insert(
            "type".to_string(),
            Value::String(XAI_WEB_SEARCH_TOOL_TYPE.to_string()),
        );
        next.remove("name");
        next.remove("external_web_access");
        return Some(Value::Object(next));
    }
    if tool_type == "custom" {
        next.insert("type".to_string(), Value::String("function".to_string()));
        if let Some(custom) = next.remove("custom") {
            if let Some(custom_object) = custom.as_object() {
                for (key, value) in custom_object {
                    next.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        if !next.contains_key("parameters") {
            next.insert(
                "parameters".to_string(),
                json!({"type": "object", "properties": {}}),
            );
        }
        return Some(Value::Object(next));
    }
    if tool_type == "function" && !next.contains_key("parameters") {
        next.insert(
            "parameters".to_string(),
            json!({"type": "object", "properties": {}}),
        );
    }
    Some(Value::Object(next))
}

fn rewrite_xai_web_search_tool_choice(object: &mut Map<String, Value>) {
    let Some(choice) = object.get("tool_choice").cloned() else {
        return;
    };
    let Some(choice_type) = choice.as_object().and_then(|value| {
        value
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .map(str::to_ascii_lowercase)
    }) else {
        return;
    };
    if is_web_search_choice_type(&choice_type) {
        object.insert(
            "tool_choice".to_string(),
            json!({
                "type": "allowed_tools",
                "mode": "required",
                "tools": [{ "type": XAI_WEB_SEARCH_TOOL_TYPE }]
            }),
        );
    }
}

fn rewrite_xai_image_generation_tool_choice(object: &mut Map<String, Value>) {
    let has_image_generation = tools_array(object)
        .iter()
        .any(|tool| tool_type(tool).is_some_and(|value| value == XAI_IMAGE_GENERATION_TOOL_TYPE));
    if !has_image_generation {
        return;
    }
    let Some(choice) = object.get("tool_choice").cloned() else {
        return;
    };
    // xAI's allowed_tools schema cannot contain image_generation. Preserve an
    // image-only restriction before filtering image entries out of mixed lists.
    let image_only = is_allowed_tools_image_generation_only(&choice);
    if choice["type"] == XAI_IMAGE_GENERATION_TOOL_TYPE || image_only {
        let mode = if image_only && choice["mode"] == "auto" {
            "auto"
        } else {
            "required"
        };
        keep_only_image_generation_tools(object);
        object.insert("tool_choice".to_string(), Value::String(mode.to_string()));
    } else if choice["type"] == "allowed_tools" {
        filter_image_generation_from_allowed_tools(object);
    }
}

fn is_allowed_tools_image_generation_only(choice: &Value) -> bool {
    let Some(object) = choice.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("allowed_tools") {
        return false;
    }
    let Some(tools) = object.get("tools").and_then(Value::as_array) else {
        return false;
    };
    !tools.is_empty()
        && tools.iter().all(|tool| {
            tool_type(tool).is_some_and(|value| value == XAI_IMAGE_GENERATION_TOOL_TYPE)
        })
}

fn keep_only_image_generation_tools(object: &mut Map<String, Value>) {
    let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };
    tools.retain(|tool| {
        tool_type(tool).is_some_and(|value| value == XAI_IMAGE_GENERATION_TOOL_TYPE)
    });
}

fn filter_image_generation_from_allowed_tools(object: &mut Map<String, Value>) {
    let Some(choice) = object.get_mut("tool_choice").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(tools) = choice.get_mut("tools").and_then(Value::as_array_mut) else {
        return;
    };
    tools
        .retain(|tool| tool_type(tool).is_none_or(|value| value != XAI_IMAGE_GENERATION_TOOL_TYPE));
}

fn is_web_search_choice_type(value: &str) -> bool {
    value == XAI_WEB_SEARCH_TOOL_TYPE || value.starts_with("web_search")
}

fn prune_xai_orphaned_tool_choice(object: &mut Map<String, Value>) {
    let available = collect_available_tool_choice_keys(object);
    let Some(choice) = object.get("tool_choice").cloned() else {
        return;
    };
    if choice.as_str().is_some() {
        return;
    }
    let Some(choice_object) = choice.as_object() else {
        object.remove("tool_choice");
        return;
    };
    let choice_type = choice_object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if choice_type == "allowed_tools" {
        let Some(allowed) = choice_object.get("tools").and_then(Value::as_array) else {
            object.remove("tool_choice");
            return;
        };
        let kept = allowed
            .iter()
            .filter(|tool| tool_matches_available(tool, &available))
            .cloned()
            .collect::<Vec<_>>();
        if kept.is_empty() {
            object.remove("tool_choice");
            return;
        }
        if let Some(choice) = object.get_mut("tool_choice").and_then(Value::as_object_mut) {
            choice.insert("tools".to_string(), Value::Array(kept));
        }
        return;
    }
    if choice_type.is_empty() {
        return;
    }
    if !tool_matches_available(&choice, &available) {
        object.remove("tool_choice");
    }
}

fn collect_available_tool_choice_keys(object: &Map<String, Value>) -> Vec<ToolChoiceKey> {
    let mut keys = Vec::new();
    collect_tool_choice_keys(tools_array(object), &mut keys);
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                collect_tool_choice_keys(
                    item.get("tools")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                    &mut keys,
                );
            }
        }
    }
    keys
}

fn collect_tool_choice_keys(tools: &[Value], keys: &mut Vec<ToolChoiceKey>) {
    for tool in tools {
        let Some(tool_type) = tool_type(tool) else {
            continue;
        };
        if matches!(tool_type, "function" | "custom") {
            if let Some(name) = tool_name(tool) {
                keys.push(ToolChoiceKey::Named {
                    name: name.to_ascii_lowercase(),
                });
            }
            continue;
        }
        keys.push(ToolChoiceKey::Hosted(tool_type.to_ascii_lowercase()));
    }
}

fn tool_matches_available(choice: &Value, available: &[ToolChoiceKey]) -> bool {
    let Some(object) = choice.as_object() else {
        return false;
    };
    let choice_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if matches!(choice_type.as_str(), "function" | "custom" | "tool") {
        let Some(name) = tool_choice_name(object) else {
            return false;
        };
        return available.iter().any(|key| {
            matches!(
                key,
                ToolChoiceKey::Named { name: available_name, .. }
                    if available_name == &name.to_ascii_lowercase()
            )
        });
    }
    if is_web_search_choice_type(&choice_type) {
        return available.iter().any(
            |key| matches!(key, ToolChoiceKey::Hosted(value) if value == XAI_WEB_SEARCH_TOOL_TYPE),
        );
    }
    available
        .iter()
        .any(|key| matches!(key, ToolChoiceKey::Hosted(value) if value == &choice_type))
}

#[derive(Clone, Debug)]
enum ToolChoiceKey {
    Named { name: String },
    Hosted(String),
}

fn drop_tool_choice_without_tools(object: &mut Map<String, Value>) {
    if xai_request_has_tools(object) {
        return;
    }
    object.remove("tools");
    object.remove("tool_choice");
    object.remove("parallel_tool_calls");
}

fn xai_request_has_tools(object: &Map<String, Value>) -> bool {
    if !tools_array(object).is_empty() {
        return true;
    }
    object
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "additional_tools")
                && item
                    .get("tools")
                    .and_then(Value::as_array)
                    .is_some_and(|tools| !tools.is_empty())
        })
}

fn strip_unsupported_reasoning_effort(object: &mut Map<String, Value>) {
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if xai_model_supports_reasoning_effort(model) {
        return;
    }
    let Some(reasoning) = object.get_mut("reasoning") else {
        return;
    };
    let Some(reasoning_object) = reasoning.as_object_mut() else {
        return;
    };
    reasoning_object.remove("effort");
    if reasoning_object.is_empty() {
        object.remove("reasoning");
    }
}

pub fn xai_model_supports_reasoning_effort(model: &str) -> bool {
    let lowered = model.trim().to_ascii_lowercase();
    let name = lowered.rsplit('/').next().unwrap_or(lowered.as_str());
    if name.is_empty() || name.contains("non-reasoning") || name.contains("imagine") {
        return false;
    }
    name.starts_with("grok-3-mini")
        || name.starts_with("grok-4")
        || name.starts_with("grok-build")
        || name.starts_with("grok-composer")
}

pub fn xai_supports_native_image_generation(model: &str) -> bool {
    let lowered = model.trim().to_ascii_lowercase();
    let name = lowered.rsplit('/').next().unwrap_or(lowered.as_str());
    let Some(rest) = name.strip_prefix("grok-") else {
        return false;
    };
    if rest == "4.20" || rest.starts_with("4.20-") {
        return false;
    }
    parse_grok_version_prefix(rest).is_some_and(grok_version_at_least_image_generation)
}

fn parse_grok_version_prefix(rest: &str) -> Option<XaiGrokVersion> {
    let major_len = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    if major_len == 0 {
        return None;
    }
    let major = rest[..major_len].parse().ok()?;
    if major_len == rest.len() || !rest[major_len..].starts_with('.') {
        return Some(XaiGrokVersion { major, minor: -1 });
    }
    let after_dot = &rest[major_len + 1..];
    let minor_len = after_dot
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(after_dot.len());
    if minor_len == 0 {
        return Some(XaiGrokVersion { major, minor: -1 });
    }
    let minor = after_dot[..minor_len].parse().ok()?;
    Some(XaiGrokVersion { major, minor })
}

fn grok_version_at_least_image_generation(version: XaiGrokVersion) -> bool {
    let minor = if version.minor < 0 { 0 } else { version.minor };
    (version.major, minor)
        >= (
            XAI_GROK_IMAGE_GENERATION_MIN.major,
            XAI_GROK_IMAGE_GENERATION_MIN.minor,
        )
}

fn sanitize_xai_input_encrypted_content(object: &mut Map<String, Value>) {
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    let mut kept = Vec::new();
    for item in input.iter() {
        let Some(item_object) = item.as_object() else {
            kept.push(item.clone());
            continue;
        };
        let item_type = item_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if item_type != "reasoning" && item_type != "compaction" {
            kept.push(item.clone());
            continue;
        }
        let Some(encrypted) = item_object.get("encrypted_content") else {
            kept.push(item.clone());
            continue;
        };
        let valid = encrypted
            .as_str()
            .is_some_and(|value| !value.trim().is_empty());
        if valid {
            kept.push(item.clone());
            continue;
        }
        if item_type == "compaction" {
            continue;
        }
        let mut next = item_object.clone();
        next.remove("encrypted_content");
        kept.push(Value::Object(next));
    }
    *input = kept;
}

fn normalize_xai_image_refs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for key in ["image", "images", "reference_images"] {
                match object.get_mut(key) {
                    Some(Value::Array(items)) if key != "image" => {
                        for item in items {
                            normalize_xai_image_ref(item);
                        }
                    }
                    Some(item) if key == "image" => normalize_xai_image_ref(item),
                    _ => {}
                }
            }
            for child in object.values_mut() {
                normalize_xai_image_refs(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_xai_image_refs(item);
            }
        }
        _ => {}
    }
}

fn normalize_xai_image_ref(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let original_url = object
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let image_url = object.get("image_url").cloned();
    let resolved_url = original_url.clone().or_else(|| match image_url.as_ref() {
        Some(Value::String(url)) => {
            let trimmed = url.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(Value::Object(inner)) => inner
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        _ => None,
    });
    let Some(url) = resolved_url else {
        return;
    };
    if original_url.as_deref() == Some(url.as_str()) && image_url.is_none() {
        return;
    }
    object.insert("url".to_string(), Value::String(url));
    object.remove("image_url");
}

fn request_tools(body: &Value) -> &[Value] {
    body.get("tools")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn tools_array(object: &Map<String, Value>) -> &[Value] {
    object
        .get("tools")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn tool_type(tool: &Value) -> Option<&str> {
    tool.get("type").and_then(Value::as_str).map(str::trim)
}

fn tool_name(tool: &Value) -> Option<&str> {
    tool.get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            tool.get("function")
                .and_then(Value::as_object)
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            tool.get("custom")
                .and_then(Value::as_object)
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn tool_choice_name(choice: &Map<String, Value>) -> Option<&str> {
    choice
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| {
            choice
                .get("function")
                .and_then(Value::as_object)
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            choice
                .get("custom")
                .and_then(Value::as_object)
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn is_web_search_tool(tool: &Value) -> bool {
    tool_type(tool).is_some_and(is_web_search_choice_type)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        apply_xai_upstream_payload_edits, apply_xai_upstream_payload_edits_with_client,
        xai_model_supports_reasoning_effort, xai_supports_native_image_generation,
        XAI_RESPONSES_UNSUPPORTED_BODY_FIELDS,
    };

    #[test]
    fn xai_responses_edits_strip_continuation_fields_and_empty_tool_choice() {
        let mut body = json!({
            "model": "grok-4.6",
            "input": "hello",
            "previous_response_id": "resp_123",
            "prompt_cache_retention": "24h",
            "safety_identifier": "user-1",
            "stream_options": {"include_obfuscation": true},
            "stop": ["END"],
            "metadata": {
                "user_id": "{\"device_id\":\"dev-1\",\"account_uuid\":\"acct-1\",\"session_id\":\"sess-1\"}"
            },
            "include": ["reasoning.encrypted_content", "file_search_call.results"],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "tools": []
        });

        apply_xai_upstream_payload_edits(&mut body, "xai", "openai:responses");

        for field in XAI_RESPONSES_UNSUPPORTED_BODY_FIELDS {
            assert!(body.get(*field).is_none(), "{field} should be stripped");
        }
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("parallel_tool_calls").is_none());
        assert!(body.get("tools").is_none());
        assert_eq!(
            body["include"],
            json!(["reasoning.encrypted_content", "file_search_call.results"])
        );
        assert_eq!(body["model"], "grok-4.6");
        assert_eq!(body["input"], "hello");
    }

    #[test]
    fn xai_responses_edits_keep_reasoning_effort_for_thinking_models() {
        let mut body = json!({
            "model": "grok-4.6",
            "reasoning": {"effort": "high", "summary": "auto"}
        });
        apply_xai_upstream_payload_edits(&mut body, "xai", "openai:responses");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["summary"], "auto");
    }

    #[test]
    fn xai_responses_edits_strip_reasoning_effort_for_non_thinking_models() {
        let mut body = json!({
            "model": "grok-4.20-0309-non-reasoning",
            "reasoning": {"effort": "high"}
        });
        apply_xai_upstream_payload_edits(&mut body, "xai", "openai:responses");
        assert!(body.get("reasoning").is_none());
        assert!(!xai_model_supports_reasoning_effort(
            "grok-4.20-0309-non-reasoning"
        ));
        assert!(xai_model_supports_reasoning_effort("xai/grok-4.5"));
        assert!(!xai_model_supports_reasoning_effort("grok-imagine-image"));
    }

    #[test]
    fn xai_hosted_tool_choice_rewrites_web_search_and_image_generation() {
        let mut web_search = json!({
            "model": "grok-4.6",
            "tools": [{"type": "web_search_preview", "name": "web_search"}],
            "tool_choice": {"type": "web_search"}
        });
        apply_xai_upstream_payload_edits(&mut web_search, "xai", "openai:responses");
        assert_eq!(web_search["tools"][0]["type"], "web_search");
        assert!(web_search["tools"][0].get("name").is_none());
        assert_eq!(web_search["tool_choice"]["type"], "allowed_tools");
        assert_eq!(web_search["tool_choice"]["mode"], "required");
        assert_eq!(web_search["tool_choice"]["tools"][0]["type"], "web_search");

        let mut image = json!({
            "model": "grok-4.6",
            "tools": [
                {"type": "web_search"},
                {"type": "image_generation", "action": "generate"}
            ],
            "tool_choice": {"type": "image_generation"}
        });
        apply_xai_upstream_payload_edits(&mut image, "xai", "openai:responses");
        assert_eq!(image["tool_choice"], "required");
        assert_eq!(image["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(image["tools"][0]["type"], "image_generation");
    }

    #[test]
    fn xai_strips_image_generation_on_older_conversation_models() {
        let mut body = json!({
            "model": "grok-4.5",
            "tools": [
                {"type": "function", "name": "lookup", "parameters": {"type": "object"}},
                {"type": "image_generation"}
            ],
            "tool_choice": {"type": "image_generation"}
        });
        apply_xai_upstream_payload_edits(&mut body, "xai", "openai:responses");
        assert_eq!(body["tools"].as_array().map(Vec::len), Some(1));
        assert_eq!(body["tools"][0]["name"], "lookup");
        assert!(body.get("tool_choice").is_none());
        assert!(xai_supports_native_image_generation("grok-4.6"));
        assert!(!xai_supports_native_image_generation("grok-4.20-0309"));
        assert!(!xai_supports_native_image_generation("grok-4.5"));
    }

    #[test]
    fn xai_restores_web_search_from_chat_and_claude_clients() {
        let mut chat_body = json!({
            "model": "grok-4.6",
            "input": "search this"
        });
        apply_xai_upstream_payload_edits_with_client(
            &mut chat_body,
            "xai",
            "openai:responses",
            Some("openai:chat"),
            Some(&json!({
                "messages": [{"role": "user", "content": "news"}],
                "web_search_options": {"search_context_size": "high"}
            })),
        );
        assert_eq!(chat_body["tools"][0]["type"], "web_search");

        let mut claude_body = json!({
            "model": "grok-4.6",
            "input": "search this",
            "tools": [{
                "type": "function",
                "name": "lookup",
                "parameters": {"type": "object", "properties": {}}
            }],
            "tool_choice": {"type": "function", "name": "web_search"}
        });
        apply_xai_upstream_payload_edits_with_client(
            &mut claude_body,
            "xai",
            "openai:responses",
            Some("claude:messages"),
            Some(&json!({
                "tools": [
                    {"type": "web_search_20250305", "name": "web_search"},
                    {"name": "lookup", "input_schema": {"type": "object"}}
                ],
                "tool_choice": {"type": "tool", "name": "web_search"}
            })),
        );
        assert!(claude_body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|tool| tool["type"] == "web_search"));
        assert_eq!(claude_body["tool_choice"]["type"], "allowed_tools");
    }

    #[test]
    fn xai_image_refs_rewrite_openai_aliases_without_touching_chat_parts() {
        let mut body = json!({
            "model": "grok-imagine-image",
            "prompt": "edit this",
            "image": {"image_url": "https://cdn.example/a.png"},
            "reference_images": [
                {"image_url": {"url": "https://cdn.example/b.png"}}
            ],
            "input": [{
                "type": "message",
                "content": [{
                    "type": "image_url",
                    "image_url": {"url": "https://cdn.example/chat.png"}
                }]
            }]
        });

        apply_xai_upstream_payload_edits(&mut body, "xai", "openai:image");

        assert_eq!(body["image"]["url"], "https://cdn.example/a.png");
        assert!(body["image"].get("image_url").is_none());
        assert_eq!(
            body["reference_images"][0]["url"],
            "https://cdn.example/b.png"
        );
        assert_eq!(
            body["input"][0]["content"][0]["image_url"]["url"],
            "https://cdn.example/chat.png"
        );
    }

    #[test]
    fn other_providers_are_left_untouched() {
        let mut body = json!({
            "previous_response_id": "resp_123",
            "image": {"image_url": "https://cdn.example/a.png"}
        });
        apply_xai_upstream_payload_edits(&mut body, "codex", "openai:responses");
        assert_eq!(body["previous_response_id"], "resp_123");
        assert_eq!(body["image"]["image_url"], "https://cdn.example/a.png");
    }
}
