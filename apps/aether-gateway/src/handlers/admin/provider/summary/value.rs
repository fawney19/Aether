use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::handlers::public::{request_candidate_event_unix_ms, request_candidate_status_label};
use crate::orchestration::codex_cyber_flag_passthrough_enabled;
use crate::provider_key_auth::provider_key_effective_api_formats;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn json_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
    }
}

fn endpoint_timestamp_or_now(value: Option<u64>, now_unix_secs: u64) -> serde_json::Value {
    unix_secs_to_rfc3339(value.unwrap_or(now_unix_secs))
        .map(serde_json::Value::String)
        .unwrap_or(serde_json::Value::Null)
}

fn json_number_as_f64(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    })
}

fn simulated_cache_summary(config: Option<&serde_json::Map<String, Value>>) -> (bool, f64, f64) {
    let simulated_cache = config
        .and_then(|cfg| cfg.get("simulated_cache"))
        .and_then(Value::as_object);
    let enabled = simulated_cache
        .and_then(|cfg| cfg.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let min_percent = simulated_cache
        .and_then(|cfg| json_number_as_f64(cfg.get("min_percent")))
        .unwrap_or(90.0)
        .clamp(0.0, 100.0);
    let max_percent = simulated_cache
        .and_then(|cfg| json_number_as_f64(cfg.get("max_percent")))
        .unwrap_or(100.0)
        .clamp(0.0, 100.0);

    (enabled, min_percent, max_percent)
}

pub(crate) fn build_admin_provider_summary_value(
    provider: &StoredProviderCatalogProvider,
    endpoints: &[StoredProviderCatalogEndpoint],
    keys: &[StoredProviderCatalogKey],
    quota_snapshot: Option<&aether_data_contracts::repository::quota::StoredProviderQuotaSnapshot>,
    model_stats: Option<
        &aether_data_contracts::repository::global_models::StoredProviderModelStats,
    >,
    active_global_model_ids: Vec<String>,
    now_unix_secs: u64,
) -> serde_json::Value {
    let total_endpoints = endpoints.len();
    let active_endpoints = endpoints
        .iter()
        .filter(|endpoint| endpoint.is_active)
        .count();
    let total_keys = keys.len();
    let active_keys = keys.iter().filter(|key| key.is_active).count();
    let total_models = model_stats
        .map(|stats| stats.total_models as usize)
        .unwrap_or(0);
    let active_models = model_stats
        .map(|stats| stats.active_models as usize)
        .unwrap_or(0);
    let api_formats = endpoints
        .iter()
        .map(|endpoint| endpoint.api_format.clone())
        .collect::<Vec<_>>();

    let format_to_endpoint_id = endpoints
        .iter()
        .map(|endpoint| (endpoint.api_format.clone(), endpoint.id.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut keys_by_endpoint = BTreeMap::<String, Vec<&StoredProviderCatalogKey>>::new();
    for endpoint in endpoints {
        keys_by_endpoint.entry(endpoint.id.clone()).or_default();
    }
    for key in keys {
        for api_format in
            provider_key_effective_api_formats(key, &provider.provider_type, endpoints)
        {
            if let Some(endpoint_id) = format_to_endpoint_id.get(&api_format) {
                keys_by_endpoint
                    .entry(endpoint_id.clone())
                    .or_default()
                    .push(key);
            }
        }
    }

    let mut endpoint_health_scores = Vec::with_capacity(endpoints.len());
    let endpoint_health_details = endpoints
        .iter()
        .map(|endpoint| {
            let endpoint_keys = keys_by_endpoint
                .get(&endpoint.id)
                .cloned()
                .unwrap_or_default();
            let health_score = if endpoint_keys.is_empty() {
                1.0
            } else {
                let mut scores = Vec::new();
                for key in &endpoint_keys {
                    let score = key
                        .health_by_format
                        .as_ref()
                        .and_then(|value| value.get(&endpoint.api_format))
                        .and_then(|value| value.get("health_score"))
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(1.0);
                    scores.push(score);
                }
                scores.iter().sum::<f64>() / scores.len() as f64
            };
            endpoint_health_scores.push(health_score);
            json!({
                "api_format": endpoint.api_format,
                "health_score": health_score,
                "is_active": endpoint.is_active,
                "total_keys": endpoint_keys.len(),
                "active_keys": endpoint_keys.iter().filter(|key| key.is_active).count(),
            })
        })
        .collect::<Vec<_>>();
    let avg_health_score = if endpoint_health_scores.is_empty() {
        1.0
    } else {
        endpoint_health_scores.iter().sum::<f64>() / endpoint_health_scores.len() as f64
    };
    let unhealthy_endpoints = endpoint_health_scores
        .iter()
        .filter(|score| **score < 0.5)
        .count();

    let provider_config = provider.config.clone();
    let config = provider_config
        .as_ref()
        .and_then(serde_json::Value::as_object);
    let provider_ops_config = config.and_then(|cfg| cfg.get("provider_ops"));
    let ops_configured = provider_ops_config.is_some_and(json_truthy);
    let ops_architecture_id = provider_ops_config
        .and_then(serde_json::Value::as_object)
        .and_then(|cfg| cfg.get("architecture_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let (simulated_cache_enabled, simulated_cache_min_percent, simulated_cache_max_percent) =
        simulated_cache_summary(config);
    let kiro_simulated_cache_enabled = simulated_cache_enabled
        || config
            .and_then(|cfg| cfg.get("kiro"))
            .and_then(serde_json::Value::as_object)
            .and_then(|cfg| cfg.get("simulated_cache_enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
    let ops_quota_alert_enabled = provider_ops_config
        .and_then(serde_json::Value::as_object)
        .and_then(|cfg| cfg.get("quota_alert"))
        .and_then(serde_json::Value::as_object)
        .and_then(|cfg| cfg.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let billing_type = quota_snapshot
        .map(|quota| quota.billing_type.clone())
        .or_else(|| provider.billing_type.clone());
    let monthly_quota_usd = quota_snapshot
        .and_then(|quota| quota.monthly_quota_usd)
        .or(provider.monthly_quota_usd);
    let monthly_used_usd = quota_snapshot
        .map(|quota| quota.monthly_used_usd)
        .or(provider.monthly_used_usd);
    let quota_reset_day = quota_snapshot
        .and_then(|quota| quota.quota_reset_day)
        .or(provider.quota_reset_day);
    let quota_last_reset_at = quota_snapshot
        .and_then(|quota| quota.quota_last_reset_at_unix_secs)
        .or(provider.quota_last_reset_at_unix_secs)
        .and_then(unix_secs_to_rfc3339);
    let quota_expires_at = quota_snapshot
        .and_then(|quota| quota.quota_expires_at_unix_secs)
        .or(provider.quota_expires_at_unix_secs)
        .and_then(unix_secs_to_rfc3339);

    let mut value = serde_json::Map::new();
    value.insert("id".to_string(), json!(provider.id.clone()));
    value.insert("name".to_string(), json!(provider.name.clone()));
    value.insert(
        "provider_type".to_string(),
        json!(provider.provider_type.clone()),
    );
    value.insert(
        "description".to_string(),
        json!(provider.description.clone()),
    );
    value.insert("website".to_string(), json!(provider.website.clone()));
    value.insert(
        "provider_priority".to_string(),
        json!(provider.provider_priority),
    );
    value.insert(
        "keep_priority_on_conversion".to_string(),
        json!(provider.keep_priority_on_conversion),
    );
    value.insert(
        "enable_format_conversion".to_string(),
        json!(provider.enable_format_conversion),
    );
    value.insert("is_active".to_string(), json!(provider.is_active));
    value.insert("billing_type".to_string(), json!(billing_type));
    value.insert("monthly_quota_usd".to_string(), json!(monthly_quota_usd));
    value.insert("monthly_used_usd".to_string(), json!(monthly_used_usd));
    value.insert("quota_reset_day".to_string(), json!(quota_reset_day));
    value.insert(
        "quota_last_reset_at".to_string(),
        json!(quota_last_reset_at),
    );
    value.insert("quota_expires_at".to_string(), json!(quota_expires_at));
    value.insert("max_retries".to_string(), json!(provider.max_retries));
    value.insert("proxy".to_string(), json!(provider.proxy.clone()));
    value.insert(
        "stream_first_byte_timeout".to_string(),
        json!(provider.stream_first_byte_timeout_secs),
    );
    value.insert(
        "request_timeout".to_string(),
        json!(provider.request_timeout_secs),
    );
    value.insert(
        "claude_code_advanced".to_string(),
        config
            .and_then(|cfg| cfg.get("claude_code_advanced"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    value.insert(
        "pool_advanced".to_string(),
        config
            .and_then(|cfg| cfg.get("pool_advanced"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    value.insert(
        "failover_rules".to_string(),
        config
            .and_then(|cfg| cfg.get("failover_rules"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    value.insert(
        "chat_pii_redaction".to_string(),
        config
            .and_then(|cfg| cfg.get("chat_pii_redaction"))
            .cloned()
            .unwrap_or(Value::Null),
    );
    value.insert("total_endpoints".to_string(), json!(total_endpoints));
    value.insert("active_endpoints".to_string(), json!(active_endpoints));
    value.insert("total_keys".to_string(), json!(total_keys));
    value.insert("active_keys".to_string(), json!(active_keys));
    value.insert("total_models".to_string(), json!(total_models));
    value.insert("active_models".to_string(), json!(active_models));
    value.insert(
        "global_model_ids".to_string(),
        json!(active_global_model_ids),
    );
    value.insert("avg_health_score".to_string(), json!(avg_health_score));
    value.insert(
        "unhealthy_endpoints".to_string(),
        json!(unhealthy_endpoints),
    );
    value.insert("api_formats".to_string(), json!(api_formats));
    value.insert(
        "endpoint_health_details".to_string(),
        json!(endpoint_health_details),
    );
    value.insert("ops_configured".to_string(), json!(ops_configured));
    value.insert(
        "ops_architecture_id".to_string(),
        json!(ops_architecture_id),
    );
    value.insert(
        "kiro_simulated_cache_enabled".to_string(),
        json!(kiro_simulated_cache_enabled),
    );
    value.insert(
        "simulated_cache_enabled".to_string(),
        json!(simulated_cache_enabled),
    );
    value.insert(
        "simulated_cache_min_percent".to_string(),
        json!(simulated_cache_min_percent),
    );
    value.insert(
        "simulated_cache_max_percent".to_string(),
        json!(simulated_cache_max_percent),
    );
    value.insert(
        "codex_cyber_flag_passthrough_enabled".to_string(),
        json!(codex_cyber_flag_passthrough_enabled(
            &provider.provider_type,
            provider.config.as_ref()
        )),
    );
    value.insert(
        "ops_quota_alert_enabled".to_string(),
        json!(ops_quota_alert_enabled),
    );
    value.insert(
        "created_at".to_string(),
        endpoint_timestamp_or_now(provider.created_at_unix_ms, now_unix_secs),
    );
    value.insert(
        "updated_at".to_string(),
        endpoint_timestamp_or_now(provider.updated_at_unix_secs, now_unix_secs),
    );
    Value::Object(value)
}
