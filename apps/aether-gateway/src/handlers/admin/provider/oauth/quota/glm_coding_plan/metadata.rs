use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;

pub(super) fn merge_metadata_patch(
    target: &mut serde_json::Map<String, serde_json::Value>,
    patch: serde_json::Value,
) -> bool {
    if let Some(object) = patch.as_object() {
        for (key, value) in object {
            target.insert(key.clone(), value.clone());
        }
        !object.is_empty()
    } else {
        false
    }
}

pub(super) fn push_probe_warning(
    target: &mut serde_json::Map<String, serde_json::Value>,
    probe: &str,
    message: String,
) {
    let warnings = target
        .entry("probe_warnings".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if let Some(items) = warnings.as_array_mut() {
        items.push(json!({
            "probe": probe,
            "message": message,
        }));
    }
}

pub(super) fn missing_key_result(key: &StoredProviderCatalogKey) -> serde_json::Value {
    json!({
        "key_id": key.id,
        "key_name": key.name,
        "status": "error",
        "message": "缺少 GLM Coding Plan Token，请把 ANTHROPIC_AUTH_TOKEN 保存为 Key",
    })
}

pub(super) fn transport_unavailable_result(key: &StoredProviderCatalogKey) -> serde_json::Value {
    json!({
        "key_id": key.id,
        "key_name": key.name,
        "status": "error",
        "message": "Provider transport snapshot unavailable",
    })
}

pub(super) fn key_write_failed_result(key: &StoredProviderCatalogKey) -> serde_json::Value {
    json!({
        "key_id": key.id,
        "key_name": key.name,
        "status": "error",
        "message": "Key 状态写入失败",
    })
}

pub(super) fn business_error_reason(message: &str) -> String {
    let normalized = message.trim();
    if normalized.is_empty() {
        "GLM Coding Plan Token 无效".to_string()
    } else {
        normalized.to_string()
    }
}

pub(super) fn unix_now() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
