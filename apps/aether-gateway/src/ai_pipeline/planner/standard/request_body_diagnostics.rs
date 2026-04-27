use serde_json::Value;

use crate::ai_pipeline::planner::{CandidateFailureDiagnostic, CandidateFailureDiagnosticKind};

pub(crate) fn request_body_build_failure_extra_data(
    body_json: &Value,
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<Value> {
    let diagnostic =
        diagnose_request_body_build_failure(body_json, client_api_format, provider_api_format)?;
    Some(
        diagnostic
            .formats(client_api_format, provider_api_format)
            .source(request_body_build_source(
                client_api_format,
                provider_api_format,
            ))
            .to_extra_data(),
    )
}

pub(crate) fn same_format_provider_request_body_failure_extra_data(
    body_json: &Value,
    provider_api_format: &str,
    body_rules: Option<&Value>,
    context: &str,
) -> Option<Value> {
    let diagnostic =
        diagnose_same_format_provider_request_body_failure(body_json, body_rules, context)?;
    Some(
        diagnostic
            .formats(provider_api_format, provider_api_format)
            .source(context)
            .to_extra_data(),
    )
}

type RequestBodyBuildDiagnostic = CandidateFailureDiagnostic;

fn diagnose_request_body_build_failure(
    body_json: &Value,
    client_api_format: &str,
    provider_api_format: &str,
) -> Option<RequestBodyBuildDiagnostic> {
    if !body_json.is_object() {
        return Some(diagnostic("$", "请求体必须是 JSON object"));
    }

    if is_openai_responses_client_format(client_api_format) {
        if let Some(diagnostic) = diagnose_openai_responses_request(body_json) {
            return Some(diagnostic);
        }
        return Some(diagnostic(
            "$",
            "OpenAI Responses 请求体初步结构检查通过；失败可能发生在后续跨格式转换或 Body 规则应用",
        ));
    }

    if client_api_format == "openai:chat"
        && (provider_api_format.starts_with("claude:")
            || provider_api_format.starts_with("gemini:"))
    {
        return diagnose_openai_chat_cross_format_request(body_json, provider_api_format);
    }

    Some(diagnostic(
        "$",
        "请求体转换失败；当前转换器未返回更细的字段路径",
    ))
}

fn is_openai_responses_client_format(client_api_format: &str) -> bool {
    crate::ai_pipeline::is_openai_responses_family_format(client_api_format)
}

fn diagnose_same_format_provider_request_body_failure(
    body_json: &Value,
    body_rules: Option<&Value>,
    context: &str,
) -> Option<RequestBodyBuildDiagnostic> {
    if !body_json.is_object() {
        return Some(diagnostic("$", "反代请求体必须是 JSON object"));
    }
    if body_rules.is_some_and(|rules| !rules.is_array()) {
        return Some(diagnostic(
            "$.endpoint.body_rules",
            "Endpoint Body 规则必须是数组，本地反代无法应用该配置",
        ));
    }
    match context {
        "kiro_envelope" => Some(diagnostic(
            "$",
            "Kiro 反代请求体包装失败；请检查 Kiro auth_config 与 Endpoint Body 规则",
        )),
        "antigravity_envelope" => Some(diagnostic(
            "$",
            "Antigravity 反代请求体包装失败；请检查请求体是否满足该传输封装要求",
        )),
        _ => Some(diagnostic(
            "$",
            "反代请求体构建失败；当前路径未返回更细的字段信息",
        )),
    }
}

fn diagnose_openai_chat_cross_format_request(
    body_json: &Value,
    provider_api_format: &str,
) -> Option<RequestBodyBuildDiagnostic> {
    let request = body_json.as_object()?;

    if let Some(messages) = request.get("messages") {
        let Some(messages) = messages.as_array() else {
            return Some(diagnostic(
                "$.messages",
                "OpenAI Chat 的 messages 必须是数组",
            ));
        };
        for (message_index, message) in messages.iter().enumerate() {
            let Some(message_object) = message.as_object() else {
                return Some(diagnostic(
                    format!("$.messages[{message_index}]"),
                    "message 必须是 object",
                ));
            };
            let role = message_object
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            match role.as_str() {
                "system" | "developer" => {
                    if let Some(diagnostic) = diagnose_openai_text_content(
                        message_object.get("content"),
                        format!("$.messages[{message_index}].content"),
                    ) {
                        return Some(diagnostic);
                    }
                }
                "user" | "assistant" => {
                    if let Some(diagnostic) = diagnose_openai_content_blocks(
                        message_object.get("content"),
                        format!("$.messages[{message_index}].content"),
                        role.as_str(),
                    ) {
                        return Some(diagnostic);
                    }
                    if role == "assistant" {
                        if let Some(diagnostic) = diagnose_openai_assistant_tool_calls(
                            message_object.get("tool_calls"),
                            format!("$.messages[{message_index}].tool_calls"),
                        ) {
                            return Some(diagnostic);
                        }
                    }
                }
                "tool" => {
                    let valid_tool_call_id = message_object
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .is_some_and(|value| !value.is_empty());
                    if !valid_tool_call_id {
                        return Some(diagnostic(
                            format!("$.messages[{message_index}].tool_call_id"),
                            "tool 消息必须包含非空 tool_call_id",
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    if let Some(diagnostic) = diagnose_openai_tools(request.get("tools"), provider_api_format) {
        return Some(diagnostic);
    }
    diagnose_openai_tool_choice(request.get("tool_choice"))
}

fn diagnose_openai_responses_request(body_json: &Value) -> Option<RequestBodyBuildDiagnostic> {
    let request = body_json.as_object()?;

    if let Some(diagnostic) = diagnose_openai_responses_text_content(
        request.get("instructions"),
        "$.instructions".to_string(),
    ) {
        return Some(diagnostic);
    }

    if let Some(diagnostic) = diagnose_openai_responses_input(request.get("input")) {
        return Some(diagnostic);
    }
    if let Some(diagnostic) = diagnose_openai_responses_tools(request.get("tools")) {
        return Some(diagnostic);
    }
    diagnose_openai_responses_tool_choice(request.get("tool_choice"))
}

fn diagnose_openai_responses_input(input: Option<&Value>) -> Option<RequestBodyBuildDiagnostic> {
    let Some(input) = input else {
        return None;
    };
    match input {
        Value::Null | Value::String(_) => None,
        Value::Array(items) => {
            for (item_index, item) in items.iter().enumerate() {
                if item.is_string() {
                    continue;
                }
                let item_path = format!("$.input[{item_index}]");
                let Some(item_object) = item.as_object() else {
                    return Some(diagnostic(
                        item_path,
                        "OpenAI Responses input 数组项必须是 string 或 object",
                    ));
                };
                let item_type = item_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message")
                    .trim()
                    .to_ascii_lowercase();
                match item_type.as_str() {
                    "message" => {
                        let role = item_object
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("user")
                            .trim()
                            .to_ascii_lowercase();
                        if role == "system" || role == "developer" {
                            if let Some(diagnostic) = diagnose_openai_responses_text_content(
                                item_object.get("content"),
                                format!("{item_path}.content"),
                            ) {
                                return Some(diagnostic);
                            }
                        } else if let Some(diagnostic) = diagnose_openai_responses_message_content(
                            item_object.get("content"),
                            format!("{item_path}.content"),
                        ) {
                            return Some(diagnostic);
                        }
                    }
                    "function_call" => {
                        let valid_name = item_object
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .is_some_and(|value| !value.is_empty());
                        if !valid_name {
                            return Some(diagnostic(
                                format!("{item_path}.name"),
                                "function_call 必须包含非空 name",
                            ));
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        _ => Some(diagnostic(
            "$.input",
            "OpenAI Responses input 必须是 string、array 或 null",
        )),
    }
}

fn diagnose_openai_responses_text_content(
    content: Option<&Value>,
    path: String,
) -> Option<RequestBodyBuildDiagnostic> {
    match content {
        None | Some(Value::Null) | Some(Value::String(_)) => None,
        Some(Value::Array(parts)) => {
            for (part_index, part) in parts.iter().enumerate() {
                if !part.is_object() {
                    return Some(diagnostic(
                        format!("{path}[{part_index}]"),
                        "文本 content 数组项必须是 object",
                    ));
                }
            }
            None
        }
        Some(_) => Some(diagnostic(
            path,
            "文本 content 必须是 string、array 或 null",
        )),
    }
}

fn diagnose_openai_responses_message_content(
    content: Option<&Value>,
    path: String,
) -> Option<RequestBodyBuildDiagnostic> {
    match content {
        None | Some(Value::Null) | Some(Value::String(_)) => None,
        Some(Value::Array(parts)) => {
            for (part_index, part) in parts.iter().enumerate() {
                let part_path = format!("{path}[{part_index}]");
                let Some(part_object) = part.as_object() else {
                    return Some(diagnostic(part_path, "message content 数组项必须是 object"));
                };
                let part_type = part_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                if matches!(
                    part_type.as_str(),
                    "input_image" | "output_image" | "image_url"
                ) && image_part_url(part_object).is_none()
                {
                    return Some(diagnostic(
                        part_path,
                        "图片 content 缺少 image_url/url，无法规范化为 OpenAI Chat 图片内容",
                    ));
                }
            }
            None
        }
        Some(_) => None,
    }
}

fn diagnose_openai_text_content(
    content: Option<&Value>,
    path: String,
) -> Option<RequestBodyBuildDiagnostic> {
    match content {
        None | Some(Value::Null) | Some(Value::String(_)) => None,
        Some(Value::Array(parts)) => {
            for (part_index, part) in parts.iter().enumerate() {
                if !part.is_object() {
                    return Some(diagnostic(
                        format!("{path}[{part_index}]"),
                        "content 数组项必须是 object",
                    ));
                }
            }
            None
        }
        Some(_) => Some(diagnostic(path, "content 必须是 string、array 或 null")),
    }
}

fn diagnose_openai_content_blocks(
    content: Option<&Value>,
    path: String,
    role: &str,
) -> Option<RequestBodyBuildDiagnostic> {
    match content {
        None | Some(Value::Null) | Some(Value::String(_)) => None,
        Some(Value::Array(parts)) => {
            for (part_index, part) in parts.iter().enumerate() {
                let part_path = format!("{path}[{part_index}]");
                let Some(part_object) = part.as_object() else {
                    return Some(diagnostic(part_path, "content 数组项必须是 object"));
                };
                let part_type = part_object
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if matches!(part_type, "image_url" | "input_image" | "output_image")
                    && role == "user"
                    && image_part_url(part_object).is_none()
                {
                    return Some(diagnostic(
                        part_path,
                        "图片 content 缺少 image_url/url，无法转换为 Claude image block",
                    ));
                }
            }
            None
        }
        Some(_) => Some(diagnostic(path, "content 必须是 string、array 或 null")),
    }
}

fn diagnose_openai_assistant_tool_calls(
    tool_calls: Option<&Value>,
    path: String,
) -> Option<RequestBodyBuildDiagnostic> {
    let Some(tool_calls) = tool_calls else {
        return None;
    };
    let Some(tool_calls) = tool_calls.as_array() else {
        return Some(diagnostic(path, "assistant.tool_calls 必须是数组"));
    };
    for (tool_call_index, tool_call) in tool_calls.iter().enumerate() {
        let tool_call_path = format!("{path}[{tool_call_index}]");
        let Some(tool_call_object) = tool_call.as_object() else {
            return Some(diagnostic(tool_call_path, "tool_call 必须是 object"));
        };
        let Some(function) = tool_call_object.get("function").and_then(Value::as_object) else {
            return Some(diagnostic(
                format!("{tool_call_path}.function"),
                "tool_call 必须包含 function object",
            ));
        };
        let valid_name = function
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        if !valid_name {
            return Some(diagnostic(
                format!("{tool_call_path}.function.name"),
                "tool_call.function.name 必须是非空字符串",
            ));
        }
    }
    None
}

fn diagnose_openai_tools(
    tools: Option<&Value>,
    provider_api_format: &str,
) -> Option<RequestBodyBuildDiagnostic> {
    let Some(tools) = tools else {
        return None;
    };
    let Some(tools) = tools.as_array() else {
        return Some(diagnostic("$.tools", "OpenAI Chat 的 tools 必须是数组"));
    };
    for (tool_index, tool) in tools.iter().enumerate() {
        let tool_path = format!("$.tools[{tool_index}]");
        let Some(tool_object) = tool.as_object() else {
            return Some(diagnostic(tool_path, "tool 必须是 object"));
        };
        if tool_object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value != "function")
        {
            continue;
        }
        let Some(function) = tool_object.get("function").and_then(Value::as_object) else {
            let native_tool_hint = if provider_api_format.starts_with("claude:") {
                "；如果这是 Claude 原生 tool，请改为 OpenAI function tool 格式"
            } else if provider_api_format.starts_with("gemini:") {
                "；如果这是 Gemini 原生 tool，请改为 OpenAI function tool 格式"
            } else {
                ""
            };
            return Some(diagnostic(
                format!("{tool_path}.function"),
                format!("OpenAI tool 必须包含 function object{native_tool_hint}"),
            ));
        };
        let valid_name = function
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        if !valid_name {
            return Some(diagnostic(
                format!("{tool_path}.function.name"),
                "OpenAI tool 的 function.name 必须是非空字符串",
            ));
        }
    }
    None
}

fn diagnose_openai_responses_tools(tools: Option<&Value>) -> Option<RequestBodyBuildDiagnostic> {
    let Some(tools) = tools else {
        return None;
    };
    let Some(tool_values) = tools.as_array() else {
        return None;
    };
    for (tool_index, tool) in tool_values.iter().enumerate() {
        let tool_path = format!("$.tools[{tool_index}]");
        let Some(tool_object) = tool.as_object() else {
            return Some(diagnostic(tool_path, "OpenAI Responses tool 必须是 object"));
        };
        let tool_type = tool_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .trim()
            .to_ascii_lowercase();
        if tool_type.starts_with("web_search")
            || tool_object.get("function").is_some()
            || tool_type != "function"
        {
            continue;
        }
        let valid_name = tool_object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        if !valid_name {
            return Some(diagnostic(
                format!("{tool_path}.name"),
                "OpenAI Responses function tool 必须包含非空 name",
            ));
        }
    }
    None
}

fn diagnose_openai_responses_tool_choice(
    tool_choice: Option<&Value>,
) -> Option<RequestBodyBuildDiagnostic> {
    let Some(Value::Object(object)) = tool_choice else {
        return None;
    };
    let is_cli_function_choice = object.get("function").is_none()
        && object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("function"));
    if !is_cli_function_choice {
        return None;
    }
    let valid_name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if valid_name {
        None
    } else {
        Some(diagnostic(
            "$.tool_choice.name",
            "OpenAI Responses tool_choice 指定 function 时必须包含非空 name",
        ))
    }
}

fn diagnose_openai_tool_choice(tool_choice: Option<&Value>) -> Option<RequestBodyBuildDiagnostic> {
    let Some(Value::Object(object)) = tool_choice else {
        return None;
    };
    let valid_name = object
        .get("function")
        .and_then(Value::as_object)
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if valid_name {
        None
    } else {
        Some(diagnostic(
            "$.tool_choice.function.name",
            "tool_choice 指定具体工具时必须包含非空 function.name",
        ))
    }
}

fn image_part_url(part_object: &serde_json::Map<String, Value>) -> Option<&str> {
    part_object
        .get("image_url")
        .and_then(|value| {
            value.as_str().or_else(|| {
                value
                    .as_object()
                    .and_then(|object| object.get("url"))
                    .and_then(Value::as_str)
            })
        })
        .or_else(|| part_object.get("url").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn diagnostic(path: impl Into<String>, message: impl Into<String>) -> RequestBodyBuildDiagnostic {
    CandidateFailureDiagnostic::new(
        CandidateFailureDiagnosticKind::RequestBodyBuild,
        path,
        message,
    )
}

fn request_body_build_source(client_api_format: &str, provider_api_format: &str) -> String {
    format!("{client_api_format}_to_{provider_api_format}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::request_body_build_failure_extra_data;

    #[test]
    fn openai_chat_to_claude_reports_claude_native_tool_shape() {
        let body = json!({
            "model": "gpt-5.4",
            "messages": [{ "role": "user", "content": "hello" }],
            "tools": [{
                "name": "read_file",
                "description": "Read a file",
                "input_schema": { "type": "object" }
            }]
        });

        let diagnostic = request_body_build_failure_extra_data(&body, "openai:chat", "claude:chat")
            .expect("diagnostic");

        assert_eq!(
            diagnostic["request_body_build_error"]["path"],
            "$.tools[0].function"
        );
        assert_eq!(
            diagnostic["failure_diagnostic"]["kind"],
            "request_body_build"
        );
        assert_eq!(
            diagnostic["failure_diagnostic"]["source"],
            "openai:chat_to_claude:chat"
        );
        assert!(diagnostic["request_body_build_error"]["message"]
            .as_str()
            .expect("message")
            .contains("Claude 原生 tool"));
    }

    #[test]
    fn openai_chat_to_claude_reports_invalid_message_content_part() {
        let body = json!({
            "model": "gpt-5.4",
            "messages": [{
                "role": "user",
                "content": ["not-an-object"]
            }]
        });

        let diagnostic = request_body_build_failure_extra_data(&body, "openai:chat", "claude:chat")
            .expect("diagnostic");

        assert_eq!(
            diagnostic["request_body_build_error"]["path"],
            "$.messages[0].content[0]"
        );
    }

    #[test]
    fn openai_chat_to_gemini_reports_gemini_native_tool_shape() {
        let body = json!({
            "model": "gpt-5.4",
            "messages": [{ "role": "user", "content": "hello" }],
            "tools": [{
                "functionDeclarations": [{
                    "name": "search",
                    "parameters": { "type": "object" }
                }]
            }]
        });

        let diagnostic = request_body_build_failure_extra_data(&body, "openai:chat", "gemini:chat")
            .expect("diagnostic");

        assert_eq!(
            diagnostic["request_body_build_error"]["path"],
            "$.tools[0].function"
        );
        assert!(diagnostic["request_body_build_error"]["message"]
            .as_str()
            .expect("message")
            .contains("Gemini 原生 tool"));
    }

    #[test]
    fn openai_responses_reports_invalid_function_call_name() {
        let body = json!({
            "model": "gpt-5.4",
            "input": [{
                "type": "function_call",
                "arguments": "{}"
            }]
        });

        let diagnostic =
            request_body_build_failure_extra_data(&body, "openai:responses", "claude:chat")
                .expect("diagnostic");

        assert_eq!(
            diagnostic["request_body_build_error"]["path"],
            "$.input[0].name"
        );
    }

    #[test]
    fn openai_responses_reports_invalid_tool_choice_name() {
        let body = json!({
            "model": "gpt-5.4",
            "input": "hello",
            "tool_choice": { "type": "function" }
        });

        let diagnostic =
            request_body_build_failure_extra_data(&body, "openai:responses", "gemini:chat")
                .expect("diagnostic");

        assert_eq!(
            diagnostic["request_body_build_error"]["path"],
            "$.tool_choice.name"
        );
    }

    #[test]
    fn same_format_provider_reports_non_object_body() {
        let diagnostic = super::same_format_provider_request_body_failure_extra_data(
            &json!("raw"),
            "openai:chat",
            None,
            "same_format",
        )
        .expect("diagnostic");

        assert_eq!(diagnostic["request_body_build_error"]["path"], "$");
        assert!(diagnostic["request_body_build_error"]["message"]
            .as_str()
            .expect("message")
            .contains("反代请求体必须是 JSON object"));
    }

    #[test]
    fn same_format_provider_reports_invalid_body_rules_shape() {
        let diagnostic = super::same_format_provider_request_body_failure_extra_data(
            &json!({ "model": "gpt-5.4" }),
            "openai:chat",
            Some(&json!({ "action": "set" })),
            "same_format",
        )
        .expect("diagnostic");

        assert_eq!(
            diagnostic["request_body_build_error"]["path"],
            "$.endpoint.body_rules"
        );
    }
}
