use std::collections::BTreeSet;

use serde_json::{Map, Value};

pub(crate) fn normalize_string_list(values: Option<Vec<String>>) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values.into_iter().flatten() {
        let trimmed = value.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    (!out.is_empty()).then_some(out)
}

pub(crate) fn normalize_json_object(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Object(map) if map.is_empty() => Ok(None),
        serde_json::Value::Object(map) => Ok(Some(serde_json::Value::Object(map))),
        _ => Err(format!("{field_name} 必须是 JSON 对象")),
    }
}

pub(crate) fn normalize_json_array(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Array(items) if items.is_empty() => Ok(None),
        serde_json::Value::Array(items) => Ok(Some(serde_json::Value::Array(items))),
        _ => Err(format!("{field_name} 必须是 JSON 数组")),
    }
}

pub(crate) fn normalize_feature_settings(value: Option<Value>) -> Result<Option<Value>, String> {
    let Some(mut value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Object(ref mut settings) => {
            normalize_chat_pii_redaction_feature_settings(settings)?;
            if settings.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
        _ => Err("feature_settings 必须是对象".to_string()),
    }
}

pub(crate) fn deserialize_optional_json_patch<'de, D>(
    deserializer: D,
) -> Result<Option<Option<Value>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <Option<Value> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

fn normalize_chat_pii_redaction_feature_settings(
    settings: &mut Map<String, Value>,
) -> Result<(), String> {
    let Some(value) = settings.get_mut("chat_pii_redaction") else {
        return Ok(());
    };
    match value {
        Value::Null => {
            settings.remove("chat_pii_redaction");
            Ok(())
        }
        Value::Object(feature) => {
            normalize_chat_pii_redaction_feature_object(feature)?;
            if feature.is_empty() {
                settings.remove("chat_pii_redaction");
            }
            Ok(())
        }
        _ => Err("chat_pii_redaction 必须是对象".to_string()),
    }
}

fn normalize_chat_pii_redaction_feature_object(
    feature: &mut Map<String, Value>,
) -> Result<(), String> {
    for key in ["enabled", "inject_model_instruction"] {
        if let Some(value) = feature.get(key) {
            if !value.is_boolean() {
                return Err(format!("chat_pii_redaction.{key} 必须是布尔值"));
            }
        }
    }
    Ok(())
}
