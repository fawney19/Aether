pub(crate) fn codex_adapter_feature_enabled(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_object)
        .and_then(|settings| settings.get("codex_adapter"))
        .and_then(serde_json::Value::as_object)
        .and_then(|feature| feature.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub(crate) fn normalize_codex_adapter_feature_settings(
    settings: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let Some(value) = settings.remove("codex_adapter") else {
        return Ok(());
    };
    let Some(object) = value.as_object() else {
        return Err("codex_adapter 必须是对象".to_string());
    };
    let Some(enabled) = object.get("enabled").and_then(serde_json::Value::as_bool) else {
        return Err("codex_adapter.enabled 必须是布尔值".to_string());
    };
    if enabled {
        settings.insert(
            "codex_adapter".to_string(),
            serde_json::json!({ "enabled": true }),
        );
    }
    Ok(())
}
