use std::collections::BTreeSet;

pub(crate) fn normalize_provider_type_input(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "custom" | "claude_code" | "kiro" | "codex" | "chatgpt_web" | "gemini_cli"
        | "antigravity" | "vertex_ai" | "grok" | "windsurf" | "orcarouter" => Ok(normalized),
        _ => Err(
            "provider_type 仅支持 custom / claude_code / kiro / codex / chatgpt_web / gemini_cli / antigravity / vertex_ai / grok / windsurf / orcarouter"
                .to_string(),
        ),
    }
}

pub(crate) fn normalize_api_format_list(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for value in values {
        let canonical = crate::ai_serving::normalize_api_format_alias(&value);
        if seen.insert(canonical.clone()) {
            normalized.push(canonical);
        }
    }
    normalized
}

pub(crate) fn normalize_api_format_json_object_keys(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = normalize_json_like_object(value, field_name)? else {
        return Ok(None);
    };
    let serde_json::Value::Object(map) = value else {
        return Ok(Some(value));
    };
    let mut normalized = serde_json::Map::new();
    for (key, value) in map {
        let canonical = crate::ai_serving::normalize_api_format_alias(&key);
        normalized.insert(canonical, value);
    }
    Ok(Some(serde_json::Value::Object(normalized)))
}

pub(crate) fn normalize_rate_multipliers(
    value: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = normalize_json_like_object(value, "rate_multipliers")? else {
        return Ok(None);
    };
    let serde_json::Value::Object(map) = value else {
        return Ok(Some(value));
    };
    let mut normalized = serde_json::Map::new();
    for (key, value) in map {
        let canonical = crate::ai_serving::normalize_api_format_alias(&key);
        let multiplier = value
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| format!("rate_multipliers.{canonical} 必须是大于或等于 0 的有限数值"))?;
        normalized.insert(canonical, serde_json::Value::from(multiplier));
    }
    if normalized.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::Value::Object(normalized)))
    }
}

pub(crate) fn normalize_auth_type_by_format(
    value: Option<serde_json::Value>,
    field_name: &str,
    api_formats: &[String],
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = normalize_json_like_object(value, field_name)? else {
        return Ok(None);
    };
    let serde_json::Value::Object(map) = value else {
        return Ok(Some(value));
    };
    let allowed = api_formats.iter().cloned().collect::<BTreeSet<_>>();
    let mut normalized = serde_json::Map::new();
    for (key, value) in map {
        let canonical = crate::ai_serving::normalize_api_format_alias(&key);
        if !allowed.is_empty() && !allowed.contains(&canonical) {
            return Err(format!("{field_name} 包含未选择的 API 格式: {canonical}"));
        }
        let Some(auth_type) = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Err(format!("{field_name}.{canonical} 必须是字符串"));
        };
        let auth_type = match auth_type.to_ascii_lowercase().as_str() {
            "api_key" | "apikey" | "api-key" => "api_key",
            "bearer" | "bearer_token" | "bearer-token" | "authorization" => "bearer",
            _ => return Err(format!("{field_name}.{canonical} 仅支持 api_key / bearer")),
        };
        normalized.insert(canonical, serde_json::Value::String(auth_type.to_string()));
    }
    if normalized.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::Value::Object(normalized)))
    }
}

pub(crate) fn normalize_allow_auth_channel_mismatch_formats(
    values: Option<Vec<String>>,
    field_name: &str,
    api_formats: &[String],
) -> Result<Option<serde_json::Value>, String> {
    let Some(values) = canonical_allow_auth_channel_mismatch_formats(values) else {
        return Ok(None);
    };
    let allowed = api_formats.iter().cloned().collect::<BTreeSet<_>>();
    for value in &values {
        if !allowed.is_empty() && !allowed.contains(value) {
            return Err(format!("{field_name} 包含未选择的 API 格式: {value}"));
        }
    }
    Ok(Some(json_string_array(values)))
}

pub(crate) fn reconcile_allow_auth_channel_mismatch_formats(
    values: Option<Vec<String>>,
    api_formats: &[String],
) -> Option<serde_json::Value> {
    let values = canonical_allow_auth_channel_mismatch_formats(values)?;
    let allowed = api_formats.iter().cloned().collect::<BTreeSet<_>>();
    Some(json_string_array(
        values
            .into_iter()
            .filter(|value| allowed.contains(value))
            .collect(),
    ))
}

fn canonical_allow_auth_channel_mismatch_formats(
    values: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let values = values?;
    let mut seen = BTreeSet::new();
    Some(
        values
            .into_iter()
            .map(|value| crate::ai_serving::normalize_api_format_alias(&value))
            .filter(|value| !value.is_empty())
            .filter(|value| seen.insert(value.clone()))
            .collect(),
    )
}

fn json_string_array(values: Vec<String>) -> serde_json::Value {
    serde_json::Value::Array(values.into_iter().map(serde_json::Value::String).collect())
}

pub(crate) fn normalize_auth_type(value: Option<&str>) -> Result<String, String> {
    let auth_type = value.unwrap_or("api_key").trim().to_ascii_lowercase();
    match auth_type.as_str() {
        "api_key" | "service_account" | "oauth" | "bearer" => Ok(auth_type),
        _ => Err("auth_type 仅支持 api_key / service_account / oauth / bearer".to_string()),
    }
}

pub(crate) fn normalize_max_probe_interval_minutes(value: i32) -> Result<i32, String> {
    if (0..=32).contains(&value) {
        Ok(value)
    } else {
        Err("max_probe_interval_minutes 必须在 0 到 32 之间".to_string())
    }
}

pub(crate) fn normalize_pool_advanced_config(
    value: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        // `pool_advanced: {}` still means "enable pool mode with defaults".
        serde_json::Value::Object(map) => Ok(Some(serde_json::Value::Object(map))),
        _ => Err("pool_advanced 必须是 JSON 对象".to_string()),
    }
}

pub(crate) fn normalize_chat_pii_redaction_config(
    value: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(mut map) => {
            if map.len() != 1 || !map.contains_key("enabled") {
                return Err("chat_pii_redaction 仅支持 enabled 布尔配置".to_string());
            }
            let enabled = map
                .remove("enabled")
                .and_then(|value| value.as_bool())
                .ok_or_else(|| "chat_pii_redaction.enabled 必须是布尔值".to_string())?;
            Ok(Some(serde_json::json!({ "enabled": enabled })))
        }
        _ => Err("chat_pii_redaction 必须是 JSON 对象".to_string()),
    }
}

pub(crate) fn set_responses_websocket_enabled(
    config: &mut serde_json::Map<String, serde_json::Value>,
    enabled: bool,
) -> Result<(), String> {
    let mut responses = match config.remove("responses_websocket") {
        None => serde_json::Map::new(),
        Some(serde_json::Value::Object(config)) => config,
        Some(_) => return Err("config.responses_websocket 必须是 JSON 对象".to_string()),
    };
    responses.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
    config.insert(
        "responses_websocket".to_string(),
        serde_json::Value::Object(responses),
    );
    Ok(())
}

pub(crate) fn remove_responses_websocket_enabled(
    config: &mut serde_json::Map<String, serde_json::Value>,
) {
    let Some(serde_json::Value::Object(responses)) = config.get_mut("responses_websocket") else {
        return;
    };
    responses.remove("enabled");
    if responses.is_empty() {
        config.remove("responses_websocket");
    }
}

pub(crate) fn validate_responses_websocket_config(
    config: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(value) = config.get("responses_websocket") {
        let responses = value
            .as_object()
            .ok_or_else(|| "config.responses_websocket 必须是 JSON 对象".to_string())?;
        let enabled = responses
            .get("enabled")
            .ok_or_else(|| "config.responses_websocket.enabled 为必填布尔值".to_string())?;
        if !enabled.is_boolean() {
            return Err("config.responses_websocket.enabled 必须是布尔值".to_string());
        }
    }

    Ok(())
}

pub(crate) fn validate_vertex_api_formats(
    provider_type: &str,
    auth_type: &str,
    api_formats: &[String],
) -> Result<(), String> {
    if !provider_type.trim().eq_ignore_ascii_case("vertex_ai") {
        return Ok(());
    }

    let allowed = match auth_type {
        "api_key" => &["gemini:generate_content", "gemini:embedding"][..],
        "service_account" | "vertex_ai" => &["gemini:generate_content", "gemini:embedding"][..],
        _ => return Ok(()),
    };
    let invalid = api_formats
        .iter()
        .filter(|value| !allowed.contains(&value.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if invalid.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Vertex {auth_type} 不支持以下 API 格式: {}；允许: {}",
        invalid.join(", "),
        allowed.join(", ")
    ))
}

fn normalize_json_like_object(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(map) => Ok(Some(serde_json::Value::Object(map))),
        _ => Err(format!("{field_name} 必须是 JSON 对象")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_allow_auth_channel_mismatch_formats, normalize_api_format_json_object_keys,
        normalize_api_format_list, normalize_auth_type, normalize_auth_type_by_format,
        normalize_chat_pii_redaction_config, normalize_pool_advanced_config,
        normalize_provider_type_input, normalize_rate_multipliers,
        reconcile_allow_auth_channel_mismatch_formats, remove_responses_websocket_enabled,
        set_responses_websocket_enabled, validate_responses_websocket_config,
        validate_vertex_api_formats,
    };
    use serde_json::json;

    #[test]
    fn normalize_pool_advanced_preserves_empty_object() {
        assert_eq!(
            normalize_pool_advanced_config(Some(json!({}))).expect("empty object should normalize"),
            Some(json!({}))
        );
    }

    #[test]
    fn rate_multipliers_require_non_negative_finite_numbers() {
        assert_eq!(
            normalize_rate_multipliers(Some(json!({" OPENAI:RESPONSES ": 1.25})))
                .expect("valid multiplier should normalize"),
            Some(json!({"openai:responses": 1.25}))
        );
        for value in [
            json!({"openai:responses": -0.1}),
            json!({"openai:responses": "1.0"}),
        ] {
            assert!(normalize_rate_multipliers(Some(value)).is_err());
        }
    }

    #[test]
    fn normalize_pool_advanced_rejects_legacy_booleans() {
        assert_eq!(
            normalize_pool_advanced_config(Some(json!(true))).unwrap_err(),
            "pool_advanced 必须是 JSON 对象"
        );
        assert_eq!(
            normalize_pool_advanced_config(Some(json!(false))).unwrap_err(),
            "pool_advanced 必须是 JSON 对象"
        );
    }

    #[test]
    fn normalize_chat_pii_redaction_requires_enabled_boolean_only() {
        assert_eq!(
            normalize_chat_pii_redaction_config(Some(json!({ "enabled": true })))
                .expect("chat pii redaction should normalize"),
            Some(json!({ "enabled": true }))
        );
        assert_eq!(
            normalize_chat_pii_redaction_config(Some(
                json!({ "enabled": true, "entities": ["email"] })
            ))
            .unwrap_err(),
            "chat_pii_redaction 仅支持 enabled 布尔配置"
        );
        assert_eq!(
            normalize_chat_pii_redaction_config(Some(json!({ "enabled": "yes" }))).unwrap_err(),
            "chat_pii_redaction.enabled 必须是布尔值"
        );
    }

    #[test]
    fn responses_websocket_setting_is_available_to_explicitly_enabled_providers() {
        let mut config = serde_json::Map::new();
        set_responses_websocket_enabled(&mut config, true)
            .expect("Responses setting should be accepted");
        assert_eq!(
            config.get("responses_websocket"),
            Some(&json!({"enabled": true}))
        );
        validate_responses_websocket_config(&config).expect("Responses setting should validate");

        remove_responses_websocket_enabled(&mut config);
        assert!(config.get("responses_websocket").is_none());
    }

    #[test]
    fn normalize_auth_type_supports_bearer() {
        assert_eq!(
            normalize_auth_type(Some("bearer")).expect("bearer should normalize"),
            "bearer"
        );
    }

    #[test]
    fn normalize_provider_type_supports_chatgpt_web() {
        assert_eq!(
            normalize_provider_type_input(" ChatGPT_Web ").expect("type should normalize"),
            "chatgpt_web"
        );
    }

    #[test]
    fn normalize_provider_type_supports_grok() {
        assert_eq!(
            normalize_provider_type_input(" Grok ").expect("type should normalize"),
            "grok"
        );
    }

    #[test]
    fn normalize_api_format_list_dedupes_canonical_formats() {
        assert_eq!(
            normalize_api_format_list(vec![
                "claude:messages".to_string(),
                "claude:messages".to_string(),
                "gemini:generate_content".to_string(),
                "openai:image".to_string(),
            ]),
            vec![
                "claude:messages".to_string(),
                "gemini:generate_content".to_string(),
                "openai:image".to_string(),
            ]
        );
    }

    #[test]
    fn normalize_api_format_json_object_keys_keeps_canonical_keys() {
        assert_eq!(
            normalize_api_format_json_object_keys(
                Some(json!({
                    "claude:messages": 2,
                    "gemini:generate_content": 3,
                    "openai:video": 4
                })),
                "rate_multipliers",
            )
            .expect("object should normalize"),
            Some(json!({
                "claude:messages": 2,
                "gemini:generate_content": 3,
                "openai:video": 4
            }))
        );
    }

    #[test]
    fn normalize_auth_type_by_format_accepts_per_format_bearer_override() {
        assert_eq!(
            normalize_auth_type_by_format(
                Some(json!({
                    "claude:messages": "bearer",
                    "gemini:generate_content": "api-key"
                })),
                "auth_type_by_format",
                &[
                    "claude:messages".to_string(),
                    "gemini:generate_content".to_string(),
                ],
            )
            .expect("auth map should normalize"),
            Some(json!({
                "claude:messages": "bearer",
                "gemini:generate_content": "api_key"
            }))
        );
    }

    #[test]
    fn normalize_allow_auth_channel_mismatch_formats_preserves_explicit_empty_array() {
        assert_eq!(
            normalize_allow_auth_channel_mismatch_formats(
                Some(Vec::new()),
                "allow_auth_channel_mismatch_formats",
                &["claude:messages".to_string()],
            )
            .expect("empty array should normalize"),
            Some(json!([]))
        );
    }

    #[test]
    fn normalize_allow_auth_channel_mismatch_formats_normalizes_and_dedupes_values() {
        assert_eq!(
            normalize_allow_auth_channel_mismatch_formats(
                Some(vec![
                    "claude:messages".to_string(),
                    "CLAUDE:MESSAGES".to_string(),
                    " claude:messages ".to_string(),
                ]),
                "allow_auth_channel_mismatch_formats",
                &["claude:messages".to_string()],
            )
            .expect("format list should normalize"),
            Some(json!(["claude:messages"]))
        );
    }

    #[test]
    fn reconcile_allow_auth_channel_mismatch_formats_keeps_only_selected_formats() {
        assert_eq!(
            reconcile_allow_auth_channel_mismatch_formats(
                Some(vec![
                    "OPENAI:EMBEDDING".to_string(),
                    "gemini:generate_content".to_string(),
                    " GEMINI:GENERATE_CONTENT ".to_string(),
                ]),
                &["gemini:generate_content".to_string()],
            ),
            Some(json!(["gemini:generate_content"]))
        );
        assert_eq!(
            reconcile_allow_auth_channel_mismatch_formats(
                Some(vec!["openai:embedding".to_string()]),
                &["gemini:generate_content".to_string()],
            ),
            Some(json!([]))
        );
    }

    #[test]
    fn validate_vertex_api_formats_rejects_unimplemented_anthropic_transport() {
        assert!(validate_vertex_api_formats(
            "vertex_ai",
            "service_account",
            &["claude:messages".to_string()],
        )
        .is_err());
    }

    #[test]
    fn validate_vertex_api_formats_allows_gemini_embedding() {
        assert!(validate_vertex_api_formats(
            "vertex_ai",
            "api_key",
            &[
                "gemini:generate_content".to_string(),
                "gemini:embedding".to_string()
            ],
        )
        .is_ok());
        assert!(validate_vertex_api_formats(
            "vertex_ai",
            "service_account",
            &[
                "gemini:generate_content".to_string(),
                "gemini:embedding".to_string()
            ],
        )
        .is_ok());
    }
}
