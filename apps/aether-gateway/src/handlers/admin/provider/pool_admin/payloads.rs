use crate::handlers::admin::provider::shared::support::{
    AdminProviderPoolConfig, AdminProviderPoolRuntimeState,
};
use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;

pub(super) fn admin_pool_api_formats(key: &StoredProviderCatalogKey) -> Vec<String> {
    key.api_formats
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn admin_pool_string_list(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let values = value
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn admin_pool_json_object(
    value: Option<&serde_json::Value>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    value
        .and_then(serde_json::Value::as_object)
        .cloned()
        .filter(|value| !value.is_empty())
}

fn admin_pool_health_score(key: &StoredProviderCatalogKey) -> f64 {
    let scores = key
        .health_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .map(|formats| {
            formats
                .values()
                .filter_map(serde_json::Value::as_object)
                .filter_map(|item| item.get("health_score"))
                .filter_map(serde_json::Value::as_f64)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if scores.is_empty() {
        1.0
    } else {
        scores.into_iter().fold(1.0, f64::min)
    }
}

fn admin_pool_circuit_breaker_open(key: &StoredProviderCatalogKey) -> bool {
    key.circuit_breaker_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .map(|formats| {
            formats
                .values()
                .filter_map(serde_json::Value::as_object)
                .any(|item| {
                    item.get("open")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                })
        })
        .unwrap_or(false)
}

fn admin_pool_scheduling_payload(
    key: &StoredProviderCatalogKey,
    cooldown_reason: Option<&str>,
    cooldown_ttl_seconds: Option<u64>,
    health_score: f64,
    circuit_breaker_open: bool,
) -> (String, String, String, Vec<serde_json::Value>) {
    if !key.is_active {
        return (
            "blocked".to_string(),
            "inactive".to_string(),
            "已禁用".to_string(),
            vec![json!({
                "code": "inactive",
                "label": "已禁用",
                "blocking": true,
                "source": "manual",
                "ttl_seconds": serde_json::Value::Null,
                "detail": serde_json::Value::Null,
            })],
        );
    }
    if let Some(reason) = cooldown_reason {
        return (
            "degraded".to_string(),
            "cooldown".to_string(),
            "冷却中".to_string(),
            vec![json!({
                "code": "cooldown",
                "label": "冷却中",
                "blocking": true,
                "source": "pool",
                "ttl_seconds": cooldown_ttl_seconds,
                "detail": reason,
            })],
        );
    }
    if circuit_breaker_open {
        return (
            "degraded".to_string(),
            "circuit_breaker".to_string(),
            "熔断中".to_string(),
            vec![json!({
                "code": "circuit_breaker",
                "label": "熔断中",
                "blocking": true,
                "source": "health",
                "ttl_seconds": serde_json::Value::Null,
                "detail": serde_json::Value::Null,
            })],
        );
    }
    if health_score < 0.5 {
        return (
            "degraded".to_string(),
            "health_low".to_string(),
            "健康度较低".to_string(),
            vec![json!({
                "code": "health_low",
                "label": "健康度较低",
                "blocking": false,
                "source": "health",
                "ttl_seconds": serde_json::Value::Null,
                "detail": serde_json::Value::Null,
            })],
        );
    }
    (
        "available".to_string(),
        "available".to_string(),
        "可用".to_string(),
        Vec::new(),
    )
}

pub(super) fn build_admin_pool_key_payload(
    key: &StoredProviderCatalogKey,
    runtime: &AdminProviderPoolRuntimeState,
    pool_config: Option<AdminProviderPoolConfig>,
) -> serde_json::Value {
    let cooldown_reason = runtime.cooldown_reason_by_key.get(&key.id).cloned();
    let cooldown_ttl_seconds = cooldown_reason
        .as_ref()
        .and_then(|_| runtime.cooldown_ttl_by_key.get(&key.id).copied());
    let health_score = admin_pool_health_score(key);
    let circuit_breaker_open = admin_pool_circuit_breaker_open(key);
    let (scheduling_status, scheduling_reason, scheduling_label, scheduling_reasons) =
        admin_pool_scheduling_payload(
            key,
            cooldown_reason.as_deref(),
            cooldown_ttl_seconds,
            health_score,
            circuit_breaker_open,
        );

    json!({
        "key_id": key.id,
        "key_name": key.name,
        "is_active": key.is_active,
        "auth_type": key.auth_type,
        "status_snapshot": key.status_snapshot.clone().unwrap_or_else(|| json!({})),
        "health_score": health_score,
        "circuit_breaker_open": circuit_breaker_open,
        "api_formats": admin_pool_api_formats(key),
        "rate_multipliers": admin_pool_json_object(key.rate_multipliers.as_ref()),
        "internal_priority": key.internal_priority,
        "rpm_limit": key.rpm_limit,
        "cache_ttl_minutes": key.cache_ttl_minutes,
        "max_probe_interval_minutes": key.max_probe_interval_minutes,
        "note": key.note,
        "allowed_models": admin_pool_string_list(key.allowed_models.as_ref()),
        "capabilities": admin_pool_json_object(key.capabilities.as_ref()),
        "auto_fetch_models": key.auto_fetch_models,
        "locked_models": admin_pool_string_list(key.locked_models.as_ref()),
        "model_include_patterns": admin_pool_string_list(key.model_include_patterns.as_ref()),
        "model_exclude_patterns": admin_pool_string_list(key.model_exclude_patterns.as_ref()),
        "proxy": key.proxy.clone(),
        "fingerprint": key.fingerprint.clone(),
        "cooldown_reason": cooldown_reason,
        "cooldown_ttl_seconds": cooldown_ttl_seconds,
        "cost_window_usage": runtime.cost_window_usage_by_key.get(&key.id).copied().unwrap_or(0),
        "cost_limit": pool_config.map(|config| config.cost_limit_per_key_tokens),
        "request_count": key.request_count.unwrap_or(0),
        "total_tokens": 0,
        "total_cost_usd": "0.00000000",
        "sticky_sessions": runtime.sticky_sessions_by_key.get(&key.id).copied().unwrap_or(0),
        "lru_score": runtime.lru_score_by_key.get(&key.id).copied(),
        "created_at": key.created_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "last_used_at": key.last_used_at_unix_secs.and_then(unix_secs_to_rfc3339),
        "scheduling_status": scheduling_status,
        "scheduling_reason": scheduling_reason,
        "scheduling_label": scheduling_label,
        "scheduling_reasons": scheduling_reasons,
    })
}
