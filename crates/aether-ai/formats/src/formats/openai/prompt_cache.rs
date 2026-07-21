use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiPromptCacheViolationKind {
    InvalidType,
    InvalidEnum,
    UnsupportedForModel,
    UnsupportedContentBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiPromptCacheContractViolation {
    pub kind: OpenAiPromptCacheViolationKind,
    pub field: String,
    pub value: Option<String>,
    pub reason: String,
}

pub fn validate_openai_prompt_cache_request(
    source_api_format: &str,
    provider_model: &str,
    body: &Value,
) -> Result<(), OpenAiPromptCacheContractViolation> {
    let source_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    validate_openai_prompt_cache_request_with_source_model(
        source_api_format,
        provider_model,
        source_model,
        body,
    )
}

pub fn resolve_openai_prompt_cache_ttl_minutes(
    provider_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body: &Value,
) -> Option<i64> {
    OpenAiPromptCacheApi::parse(provider_api_format)?;
    let request = body.as_object()?;
    let explicit_ttl = request
        .get("prompt_cache_options")
        .and_then(Value::as_object)
        .and_then(|options| options.get("ttl"))
        .and_then(Value::as_str);
    if explicit_ttl == Some("30m") {
        return Some(30);
    }

    let capability_model =
        crate::formats::shared::model_directives::openai_model_capability_identity(
            provider_model,
            source_model,
        );
    crate::openai_model_supports_prompt_cache_options(&capability_model).then_some(30)
}

pub(crate) fn validate_openai_prompt_cache_request_with_source_model(
    source_api_format: &str,
    provider_model: &str,
    source_model: &str,
    body: &Value,
) -> Result<(), OpenAiPromptCacheContractViolation> {
    let Some(api) = OpenAiPromptCacheApi::parse(source_api_format) else {
        return Ok(());
    };
    let Some(request) = body.as_object() else {
        return Ok(());
    };
    let capability_model =
        crate::formats::shared::model_directives::openai_model_capability_identity(
            provider_model,
            source_model,
        );
    let supports_prompt_cache_options =
        crate::formats::shared::model_directives::openai_model_capability_is_opaque(
            provider_model,
            source_model,
        ) || crate::openai_model_supports_prompt_cache_options(&capability_model);

    if let Some(options) = request
        .get("prompt_cache_options")
        .filter(|value| !value.is_null())
    {
        validate_prompt_cache_options(options, supports_prompt_cache_options)?;
    }
    if let Some(retention) = request
        .get("prompt_cache_retention")
        .filter(|value| !value.is_null())
    {
        validate_prompt_cache_retention(retention, &capability_model)?;
    }
    match api {
        OpenAiPromptCacheApi::Chat => {
            validate_chat_prompt_cache_breakpoints(request, supports_prompt_cache_options)
        }
        OpenAiPromptCacheApi::Responses => {
            validate_responses_prompt_cache_breakpoints(request, supports_prompt_cache_options)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiPromptCacheApi {
    Chat,
    Responses,
}

impl OpenAiPromptCacheApi {
    fn parse(api_format: &str) -> Option<Self> {
        match crate::normalize_api_format_alias(api_format).as_str() {
            "openai:chat" => Some(Self::Chat),
            "openai:responses" | "openai:responses:compact" => Some(Self::Responses),
            _ => None,
        }
    }
}

fn normalize_provider_model(model: &str) -> &str {
    model.trim().rsplit('/').next().unwrap_or_default()
}

fn validate_prompt_cache_options(
    value: &Value,
    supported_for_model: bool,
) -> Result<(), OpenAiPromptCacheContractViolation> {
    if !supported_for_model {
        return Err(unsupported_for_model(
            "prompt_cache_options",
            "provider model does not support prompt_cache_options",
        ));
    }
    let Some(options) = value.as_object() else {
        return Err(invalid_type(
            "prompt_cache_options",
            value,
            "prompt_cache_options must be an object",
        ));
    };
    if let Some(mode) = options.get("mode") {
        let Some(raw) = mode.as_str() else {
            return Err(invalid_type(
                "prompt_cache_options.mode",
                mode,
                "prompt_cache_options.mode must be a string",
            ));
        };
        if !matches!(raw, "implicit" | "explicit") {
            return Err(invalid_enum(
                "prompt_cache_options.mode",
                raw,
                "prompt_cache_options.mode supports implicit or explicit",
            ));
        }
    }
    if let Some(ttl) = options.get("ttl") {
        let Some(raw) = ttl.as_str() else {
            return Err(invalid_type(
                "prompt_cache_options.ttl",
                ttl,
                "prompt_cache_options.ttl must be a string",
            ));
        };
        if raw != "30m" {
            return Err(invalid_enum(
                "prompt_cache_options.ttl",
                raw,
                "prompt_cache_options.ttl supports 30m",
            ));
        }
    }
    Ok(())
}

fn validate_prompt_cache_retention(
    value: &Value,
    provider_model: &str,
) -> Result<(), OpenAiPromptCacheContractViolation> {
    if crate::openai_model_supports_prompt_cache_options(provider_model) {
        return Err(unsupported_for_model(
            "prompt_cache_retention",
            "provider model uses prompt_cache_options.ttl",
        ));
    }
    let Some(raw) = value.as_str() else {
        return Err(invalid_type(
            "prompt_cache_retention",
            value,
            "prompt_cache_retention must be a string",
        ));
    };
    if super::shared::OpenAiPromptCacheRetention::parse(raw).is_none() {
        return Err(invalid_enum(
            "prompt_cache_retention",
            raw,
            "prompt_cache_retention supports in_memory or 24h",
        ));
    }
    if gpt_5_5_retention_is_24h_only(provider_model) && raw != "24h" {
        return Err(invalid_enum(
            "prompt_cache_retention",
            raw,
            "GPT-5.5 family models support 24h retention",
        ));
    }
    Ok(())
}

fn gpt_5_5_retention_is_24h_only(model: &str) -> bool {
    let normalized = normalize_provider_model(model)
        .to_ascii_lowercase()
        .replace('_', "-");
    matches!(normalized.as_str(), "gpt-5.5" | "gpt-5.5-pro")
}

fn validate_chat_prompt_cache_breakpoints(
    request: &serde_json::Map<String, Value>,
    supported_for_model: bool,
) -> Result<(), OpenAiPromptCacheContractViolation> {
    let Some(messages) = request.get("messages").and_then(Value::as_array) else {
        return Ok(());
    };
    for (message_index, message) in messages.iter().enumerate() {
        let Some(content) = message
            .as_object()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for (content_index, part) in content.iter().enumerate() {
            let Some(part) = part.as_object() else {
                continue;
            };
            let Some(breakpoint) = part.get("prompt_cache_breakpoint") else {
                continue;
            };
            let block_type = part.get("type").and_then(Value::as_str);
            let supported = matches!(
                block_type,
                Some("text" | "image_url" | "input_audio" | "file" | "refusal")
            );
            validate_prompt_cache_breakpoint(
                breakpoint,
                &format!(
                    "messages[{message_index}].content[{content_index}].prompt_cache_breakpoint"
                ),
                supported,
                supported_for_model,
            )?;
        }
    }
    Ok(())
}

fn validate_responses_prompt_cache_breakpoints(
    request: &serde_json::Map<String, Value>,
    supported_for_model: bool,
) -> Result<(), OpenAiPromptCacheContractViolation> {
    let Some(input) = request.get("input").and_then(Value::as_array) else {
        return Ok(());
    };
    for (item_index, item) in input.iter().enumerate() {
        let Some(item) = item.as_object() else {
            continue;
        };
        if let Some(breakpoint) = item.get("prompt_cache_breakpoint") {
            validate_prompt_cache_breakpoint(
                breakpoint,
                &format!("input[{item_index}].prompt_cache_breakpoint"),
                responses_cache_breakpoint_block_is_supported(item),
                supported_for_model,
            )?;
        }
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (content_index, part) in content.iter().enumerate() {
            let Some(part) = part.as_object() else {
                continue;
            };
            let Some(breakpoint) = part.get("prompt_cache_breakpoint") else {
                continue;
            };
            validate_prompt_cache_breakpoint(
                breakpoint,
                &format!("input[{item_index}].content[{content_index}].prompt_cache_breakpoint"),
                responses_cache_breakpoint_block_is_supported(part),
                supported_for_model,
            )?;
        }
    }
    Ok(())
}

fn responses_cache_breakpoint_block_is_supported(block: &serde_json::Map<String, Value>) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("input_text" | "input_image" | "input_file")
    )
}

fn validate_prompt_cache_breakpoint(
    value: &Value,
    field: &str,
    supported_content_block: bool,
    supported_for_model: bool,
) -> Result<(), OpenAiPromptCacheContractViolation> {
    if !supported_for_model {
        return Err(unsupported_for_model(
            field,
            "provider model does not support prompt_cache_breakpoint",
        ));
    }
    if !supported_content_block {
        return Err(OpenAiPromptCacheContractViolation {
            kind: OpenAiPromptCacheViolationKind::UnsupportedContentBlock,
            field: field.to_string(),
            value: None,
            reason: "content block does not support prompt_cache_breakpoint".to_string(),
        });
    }
    let Some(breakpoint) = value.as_object() else {
        return Err(invalid_type(
            field,
            value,
            "prompt_cache_breakpoint must be an object",
        ));
    };
    let mode_field = format!("{field}.mode");
    let Some(mode) = breakpoint.get("mode") else {
        return Err(OpenAiPromptCacheContractViolation {
            kind: OpenAiPromptCacheViolationKind::InvalidType,
            field: mode_field,
            value: None,
            reason: "prompt_cache_breakpoint.mode is required".to_string(),
        });
    };
    let Some(raw) = mode.as_str() else {
        return Err(invalid_type(
            &mode_field,
            mode,
            "prompt_cache_breakpoint.mode must be a string",
        ));
    };
    if raw != "explicit" {
        return Err(invalid_enum(
            &mode_field,
            raw,
            "prompt_cache_breakpoint.mode supports explicit",
        ));
    }
    Ok(())
}

fn invalid_type(field: &str, value: &Value, reason: &str) -> OpenAiPromptCacheContractViolation {
    OpenAiPromptCacheContractViolation {
        kind: OpenAiPromptCacheViolationKind::InvalidType,
        field: field.to_string(),
        value: Some(value.to_string()),
        reason: reason.to_string(),
    }
}

fn invalid_enum(field: &str, value: &str, reason: &str) -> OpenAiPromptCacheContractViolation {
    OpenAiPromptCacheContractViolation {
        kind: OpenAiPromptCacheViolationKind::InvalidEnum,
        field: field.to_string(),
        value: Some(value.to_string()),
        reason: reason.to_string(),
    }
}

fn unsupported_for_model(field: &str, reason: &str) -> OpenAiPromptCacheContractViolation {
    OpenAiPromptCacheContractViolation {
        kind: OpenAiPromptCacheViolationKind::UnsupportedForModel,
        field: field.to_string(),
        value: None,
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        resolve_openai_prompt_cache_ttl_minutes, validate_openai_prompt_cache_request,
        OpenAiPromptCacheViolationKind,
    };

    #[test]
    fn resolves_effective_prompt_cache_ttl_from_current_openai_contract() {
        for body in [
            json!({"model": "client-alias"}),
            json!({
                "model": "client-alias",
                "prompt_cache_options": {"mode": "implicit"}
            }),
            json!({
                "model": "client-alias",
                "prompt_cache_options": {"mode": "explicit", "ttl": "30m"}
            }),
        ] {
            assert_eq!(
                resolve_openai_prompt_cache_ttl_minutes(
                    "openai:responses",
                    "gpt-5.6-sol",
                    "client-alias",
                    &body,
                ),
                Some(30)
            );
        }

        assert_eq!(
            resolve_openai_prompt_cache_ttl_minutes(
                "openai:chat",
                "deployment-alias",
                "gpt-5.6-terra",
                &json!({"model": "gpt-5.6-terra"}),
            ),
            Some(30)
        );
        assert_eq!(
            resolve_openai_prompt_cache_ttl_minutes(
                "openai:chat",
                "gpt-5.5",
                "gpt-5.6-terra",
                &json!({"model": "gpt-5.6-terra"}),
            ),
            None
        );
        assert_eq!(
            resolve_openai_prompt_cache_ttl_minutes(
                "claude:messages",
                "gpt-5.6-sol",
                "gpt-5.6-sol",
                &json!({"model": "gpt-5.6-sol"}),
            ),
            None
        );
    }

    #[test]
    fn gpt_5_6_accepts_current_prompt_cache_options_and_breakpoints() {
        for (format, body) in [
            (
                "openai:chat",
                json!({
                    "model": "client-alias",
                    "prompt_cache_options": {"mode": "implicit", "ttl": "30m"},
                    "messages": [{
                        "role": "user",
                        "content": [{
                            "type": "input_audio",
                            "input_audio": {"data": "ZmFrZQ==", "format": "mp3"},
                            "prompt_cache_breakpoint": {"mode": "explicit"}
                        }]
                    }]
                }),
            ),
            (
                "openai:responses",
                json!({
                    "model": "client-alias",
                    "prompt_cache_options": {"mode": "explicit", "ttl": "30m"},
                    "input": [{
                        "type": "message",
                        "role": "user",
                        "content": [{
                            "type": "input_file",
                            "file_id": "file_123",
                            "prompt_cache_breakpoint": {"mode": "explicit"}
                        }]
                    }]
                }),
            ),
        ] {
            validate_openai_prompt_cache_request(format, "gpt-5.6-sol", &body)
                .expect("GPT-5.6 prompt cache contract should be accepted");
        }
    }

    #[test]
    fn gpt_5_6_uses_prompt_cache_options_and_rejects_invalid_enums() {
        let retention_error = validate_openai_prompt_cache_request(
            "openai:chat",
            "gpt-5.6-sol",
            &json!({"prompt_cache_retention": "24h"}),
        )
        .expect_err("GPT-5.6 uses prompt_cache_options.ttl");
        assert_eq!(
            retention_error.kind,
            OpenAiPromptCacheViolationKind::UnsupportedForModel
        );

        let cases = [
            (
                json!({"prompt_cache_options": {"mode": "automatic"}}),
                "prompt_cache_options.mode",
                OpenAiPromptCacheViolationKind::InvalidEnum,
            ),
            (
                json!({"prompt_cache_options": {"ttl": "1h"}}),
                "prompt_cache_options.ttl",
                OpenAiPromptCacheViolationKind::InvalidEnum,
            ),
            (
                json!({
                    "messages": [{
                        "role": "user",
                        "content": [{
                            "type": "text",
                            "text": "stable",
                            "prompt_cache_breakpoint": {"mode": "implicit"}
                        }]
                    }]
                }),
                "messages[0].content[0].prompt_cache_breakpoint.mode",
                OpenAiPromptCacheViolationKind::InvalidEnum,
            ),
        ];

        for (body, field, kind) in cases {
            let error = validate_openai_prompt_cache_request("openai:chat", "gpt-5.6-sol", &body)
                .expect_err("invalid GPT-5.6 cache contract should fail");
            assert_eq!(error.field, field);
            assert_eq!(error.kind, kind);
        }
    }

    #[test]
    fn prompt_cache_capability_uses_the_provider_model() {
        let options = json!({
            "model": "gpt-5.5",
            "prompt_cache_options": {"mode": "explicit", "ttl": "30m"},
            "messages": [{"role": "user", "content": "hello"}]
        });
        validate_openai_prompt_cache_request("openai:chat", "gpt-5.6-terra", &options)
            .expect("mapped GPT-5.6 provider model should enable prompt_cache_options");
        let error = validate_openai_prompt_cache_request("openai:chat", "gpt-5.5", &options)
            .expect_err("mapped earlier provider model should reject prompt_cache_options");
        assert_eq!(
            error.kind,
            OpenAiPromptCacheViolationKind::UnsupportedForModel
        );
        let error =
            validate_openai_prompt_cache_request("openai:chat", "deployment-alias", &options)
                .expect_err("opaque provider model must not inherit source model capability");
        assert_eq!(
            error.kind,
            OpenAiPromptCacheViolationKind::UnsupportedForModel
        );

        let retention = json!({
            "model": "gpt-5.6-sol",
            "prompt_cache_retention": "24h",
            "messages": [{"role": "user", "content": "hello"}]
        });
        validate_openai_prompt_cache_request("openai:chat", "gpt-5.5", &retention)
            .expect("mapped earlier provider model should retain its retention contract");
        let error = validate_openai_prompt_cache_request("openai:chat", "gpt-5.6-luna", &retention)
            .expect_err("GPT-5.6 uses prompt_cache_options.ttl");
        assert_eq!(
            error.kind,
            OpenAiPromptCacheViolationKind::UnsupportedForModel
        );
    }

    #[test]
    fn prompt_cache_breakpoints_validate_supported_blocks_per_api() {
        let cases = [
            (
                "openai:chat",
                json!({
                    "messages": [{
                        "role": "user",
                        "content": [{
                            "type": "video_url",
                            "video_url": {"url": "https://example.com/video.mp4"},
                            "prompt_cache_breakpoint": {"mode": "explicit"}
                        }]
                    }]
                }),
                "messages[0].content[0].prompt_cache_breakpoint",
            ),
            (
                "openai:responses",
                json!({
                    "input": [{
                        "type": "message",
                        "role": "user",
                        "content": [{
                            "type": "input_audio",
                            "input_audio": {"data": "ZmFrZQ==", "format": "mp3"},
                            "prompt_cache_breakpoint": {"mode": "explicit"}
                        }]
                    }]
                }),
                "input[0].content[0].prompt_cache_breakpoint",
            ),
        ];

        for (format, body, field) in cases {
            let error = validate_openai_prompt_cache_request(format, "gpt-5.6-sol", &body)
                .expect_err("unsupported cache breakpoint block should fail");
            assert_eq!(error.field, field);
            assert_eq!(
                error.kind,
                OpenAiPromptCacheViolationKind::UnsupportedContentBlock
            );
        }
    }

    #[test]
    fn earlier_models_reject_breakpoint_options_and_validate_retention() {
        let options = json!({"prompt_cache_options": {"mode": "explicit"}});
        let error = validate_openai_prompt_cache_request("openai:responses", "gpt-5.5", &options)
            .expect_err("earlier model should reject prompt_cache_options");
        assert_eq!(
            error.kind,
            OpenAiPromptCacheViolationKind::UnsupportedForModel
        );

        validate_openai_prompt_cache_request(
            "openai:responses",
            "gpt-5.5-pro",
            &json!({"prompt_cache_retention": "24h"}),
        )
        .expect("GPT-5.5 supports 24h retention");
        let error = validate_openai_prompt_cache_request(
            "openai:responses",
            "gpt-5.5",
            &json!({"prompt_cache_retention": "in_memory"}),
        )
        .expect_err("GPT-5.5 only supports 24h retention");
        assert_eq!(error.kind, OpenAiPromptCacheViolationKind::InvalidEnum);
    }

    #[test]
    fn nullable_prompt_cache_fields_are_treated_as_unconfigured() {
        for format in [
            "openai:chat",
            "openai:responses",
            "openai:responses:compact",
        ] {
            validate_openai_prompt_cache_request(
                format,
                "gpt-5.6-sol",
                &json!({
                    "model": "gpt-5.6-sol",
                    "prompt_cache_options": null,
                    "prompt_cache_retention": null
                }),
            )
            .expect("nullable prompt cache fields should be omitted semantically");
        }
    }
}
