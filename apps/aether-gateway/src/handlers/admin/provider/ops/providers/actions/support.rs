use serde_json::json;

use crate::handlers::shared::{
    canonicalize_provider_ops_base_url, resolve_provider_ops_same_origin_url,
};

pub(super) fn admin_provider_ops_checkin_data(
    reward: Option<f64>,
    streak_days: Option<i64>,
    next_reward: Option<f64>,
    message: Option<String>,
    extra: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    json!({
        "reward": reward,
        "streak_days": streak_days,
        "next_reward": next_reward,
        "message": message,
        "extra": extra,
    })
}

pub(super) fn admin_provider_ops_json_object_map(
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().unwrap_or_default()
}

pub(super) fn admin_provider_ops_request_url(
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    default_endpoint: &str,
) -> Result<String, String> {
    let endpoint = action_config
        .get("endpoint")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_endpoint);
    let destination = canonicalize_provider_ops_base_url(base_url).map_err(ToString::to_string)?;
    resolve_provider_ops_same_origin_url(&destination, endpoint).map_err(ToString::to_string)
}

pub(super) fn admin_provider_ops_request_method(
    action_config: &serde_json::Map<String, serde_json::Value>,
    default_method: &str,
) -> reqwest::Method {
    action_config
        .get("method")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| reqwest::Method::from_bytes(value.trim().as_bytes()).ok())
        .unwrap_or_else(|| {
            reqwest::Method::from_bytes(default_method.as_bytes()).unwrap_or(reqwest::Method::GET)
        })
}

pub(super) fn admin_provider_ops_parse_rfc3339_unix_secs(
    value: Option<&serde_json::Value>,
) -> Option<i64> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.timestamp())
}
