use std::collections::BTreeSet;

pub(crate) fn normalize_provider_type_input(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "custom" | "claude_code" | "kiro" | "codex" | "gemini_cli" | "antigravity"
        | "vertex_ai" => Ok(normalized),
        _ => Err(
            "provider_type 仅支持 custom / claude_code / kiro / codex / gemini_cli / antigravity / vertex_ai"
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
    let Some(values) = values else {
        return Ok(None);
    };
    let allowed = api_formats.iter().cloned().collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for value in values {
        let canonical = crate::ai_serving::normalize_api_format_alias(&value);
        if canonical.is_empty() {
            continue;
        }
        if !allowed.is_empty() && !allowed.contains(&canonical) {
            return Err(format!("{field_name} 包含未选择的 API 格式: {canonical}"));
        }
        if seen.insert(canonical.clone()) {
            normalized.push(serde_json::Value::String(canonical));
        }
    }
    if normalized.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::Value::Array(normalized)))
    }
}

pub(crate) fn normalize_auth_type(value: Option<&str>) -> Result<String, String> {
    let auth_type = value.unwrap_or("api_key").trim().to_ascii_lowercase();
    match auth_type.as_str() {
        "api_key" | "service_account" | "oauth" | "bearer" => Ok(auth_type),
        _ => Err("auth_type 仅支持 api_key / service_account / oauth / bearer".to_string()),
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

pub(crate) fn validate_vertex_api_formats(
    provider_type: &str,
    auth_type: &str,
    api_formats: &[String],
) -> Result<(), String> {
    if !provider_type.trim().eq_ignore_ascii_case("vertex_ai") {
        return Ok(());
    }

    let allowed = match auth_type {
        "api_key" => &["gemini:generate_content"][..],
        "service_account" | "vertex_ai" => &["claude:messages", "gemini:generate_content"][..],
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
        normalize_api_format_json_object_keys, normalize_api_format_list, normalize_auth_type,
        normalize_auth_type_by_format, normalize_pool_advanced_config, validate_vertex_api_formats,
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
    fn normalize_auth_type_supports_bearer() {
        assert_eq!(
            normalize_auth_type(Some("bearer")).expect("bearer should normalize"),
            "bearer"
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
    fn validate_vertex_api_formats_uses_canonical_message_formats() {
        assert!(validate_vertex_api_formats(
            "vertex_ai",
            "service_account",
            &[
                "claude:messages".to_string(),
                "gemini:generate_content".to_string()
            ],
        )
        .is_ok());
        assert!(validate_vertex_api_formats(
            "vertex_ai",
            "service_account",
            &["claude:chat".to_string()],
        )
        .is_err());
    }
}
