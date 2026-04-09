use super::super::config::admin_provider_ops_uses_python_verify_fallback;
use super::super::verify::admin_provider_ops_value_as_f64;
use serde_json::json;

pub(super) fn admin_provider_ops_balance_data(
    total_granted: Option<f64>,
    total_used: Option<f64>,
    total_available: Option<f64>,
    currency: &str,
    extra: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    json!({
        "total_granted": total_granted,
        "total_used": total_used,
        "total_available": total_available,
        "expires_at": serde_json::Value::Null,
        "currency": currency,
        "extra": extra,
    })
}

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

pub(super) fn admin_provider_ops_action_config_object<'a>(
    provider_ops_config: &'a serde_json::Map<String, serde_json::Value>,
    action_type: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    provider_ops_config
        .get("actions")
        .and_then(serde_json::Value::as_object)
        .and_then(|actions| actions.get(action_type))
        .and_then(serde_json::Value::as_object)
        .and_then(|action| action.get("config"))
        .and_then(serde_json::Value::as_object)
}

pub(super) fn admin_provider_ops_default_action_config(
    architecture_id: &str,
    action_type: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let value = match (architecture_id, action_type) {
        ("generic_api", "query_balance") => {
            json!({ "endpoint": "/api/user/balance", "method": "GET" })
        }
        ("generic_api", "checkin") => {
            json!({ "endpoint": "/api/user/checkin", "method": "POST" })
        }
        ("new_api", "query_balance") => json!({
            "endpoint": "/api/user/self",
            "method": "GET",
            "quota_divisor": 500000,
            "checkin_endpoint": "/api/user/checkin",
            "currency": "USD",
        }),
        ("new_api", "checkin") => json!({ "endpoint": "/api/user/checkin", "method": "POST" }),
        ("cubence", "query_balance") => {
            json!({ "endpoint": "/api/v1/dashboard/overview", "method": "GET", "currency": "USD" })
        }
        ("yescode", "query_balance") => {
            json!({ "endpoint": "/api/v1/user/balance", "method": "GET", "currency": "USD" })
        }
        ("nekocode", "query_balance") => {
            json!({ "endpoint": "/api/usage/summary", "method": "GET", "currency": "USD" })
        }
        _ => return None,
    };
    value.as_object().cloned()
}

pub(super) fn admin_provider_ops_json_object_map(
    value: serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().unwrap_or_default()
}

pub(super) fn admin_provider_ops_resolved_action_config(
    architecture_id: &str,
    provider_ops_config: &serde_json::Map<String, serde_json::Value>,
    action_type: &str,
    request_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut resolved =
        admin_provider_ops_default_action_config(architecture_id, action_type).unwrap_or_default();
    if let Some(saved) = admin_provider_ops_action_config_object(provider_ops_config, action_type) {
        for (key, value) in saved {
            resolved.insert(key.clone(), value.clone());
        }
    }
    if let Some(overrides) = request_config {
        for (key, value) in overrides {
            resolved.insert(key.clone(), value.clone());
        }
    }
    (!resolved.is_empty()).then_some(resolved)
}

pub(super) fn admin_provider_ops_request_url(
    base_url: &str,
    action_config: &serde_json::Map<String, serde_json::Value>,
    default_endpoint: &str,
) -> String {
    let endpoint = action_config
        .get("endpoint")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_endpoint);
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("{}{}", base_url.trim_end_matches('/'), endpoint)
    }
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

pub(super) fn admin_provider_ops_is_cookie_auth_architecture(architecture_id: &str) -> bool {
    matches!(architecture_id, "cubence" | "yescode" | "nekocode")
}

pub(super) fn admin_provider_ops_should_use_rust_only_action_stub(
    architecture_id: &str,
    config: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    !matches!(
        architecture_id,
        "generic_api" | "new_api" | "cubence" | "yescode" | "nekocode"
    ) || admin_provider_ops_uses_python_verify_fallback(architecture_id, config)
}

#[allow(dead_code)]
pub(super) fn admin_provider_ops_quota_divisor(
    action_config: &serde_json::Map<String, serde_json::Value>,
) -> f64 {
    admin_provider_ops_value_as_f64(action_config.get("quota_divisor"))
        .filter(|value| *value > 0.0)
        .unwrap_or(500000.0)
}
