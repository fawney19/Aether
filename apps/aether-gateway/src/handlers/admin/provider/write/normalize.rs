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

pub(crate) fn normalize_auth_type(value: Option<&str>) -> Result<String, String> {
    let auth_type = value.unwrap_or("api_key").trim().to_ascii_lowercase();
    match auth_type.as_str() {
        "api_key" | "service_account" | "oauth" => Ok(auth_type),
        _ => Err("auth_type 仅支持 api_key / service_account / oauth".to_string()),
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
        // Backward compatibility for older boolean payloads.
        serde_json::Value::Bool(true) => Ok(Some(serde_json::json!({}))),
        serde_json::Value::Bool(false) => Ok(None),
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
        "api_key" => &["gemini:chat"][..],
        "service_account" | "vertex_ai" => &["claude:chat", "gemini:chat"][..],
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

#[cfg(test)]
mod tests {
    use super::normalize_pool_advanced_config;
    use serde_json::json;

    #[test]
    fn normalize_pool_advanced_preserves_empty_object() {
        assert_eq!(
            normalize_pool_advanced_config(Some(json!({}))).expect("empty object should normalize"),
            Some(json!({}))
        );
    }

    #[test]
    fn normalize_pool_advanced_accepts_legacy_booleans() {
        assert_eq!(
            normalize_pool_advanced_config(Some(json!(true))).expect("true should normalize"),
            Some(json!({}))
        );
        assert_eq!(
            normalize_pool_advanced_config(Some(json!(false))).expect("false should normalize"),
            None
        );
    }
}
