use crate::handlers::admin::shared::unix_secs_to_rfc3339;
use crate::handlers::public::{request_candidate_event_unix_ms, request_candidate_status_label};
use crate::provider_key_auth::provider_key_effective_api_formats;
use aether_data_contracts::repository::candidates::{
    RequestCandidateStatus, StoredRequestCandidate,
};
use aether_data_contracts::repository::provider_catalog::{
    StoredProviderCatalogEndpoint, StoredProviderCatalogKey, StoredProviderCatalogProvider,
};
use serde_json::json;
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

    json!({
        "id": provider.id.clone(),
        "name": provider.name.clone(),
        "provider_type": provider.provider_type.clone(),
        "description": provider.description.clone(),
        "website": provider.website.clone(),
        "provider_priority": provider.provider_priority,
        "keep_priority_on_conversion": provider.keep_priority_on_conversion,
        "enable_format_conversion": provider.enable_format_conversion,
        "is_active": provider.is_active,
        "billing_type": billing_type,
        "monthly_quota_usd": monthly_quota_usd,
        "monthly_used_usd": monthly_used_usd,
        "quota_reset_day": quota_reset_day,
        "quota_last_reset_at": quota_last_reset_at,
        "quota_expires_at": quota_expires_at,
        "max_retries": provider.max_retries,
        "proxy": provider.proxy.clone(),
        "stream_first_byte_timeout": provider.stream_first_byte_timeout_secs,
        "request_timeout": provider.request_timeout_secs,
        "claude_code_advanced": config.and_then(|cfg| cfg.get("claude_code_advanced")).cloned(),
        "pool_advanced": config.and_then(|cfg| cfg.get("pool_advanced")).cloned(),
        "failover_rules": config.and_then(|cfg| cfg.get("failover_rules")).cloned(),
        "chat_pii_redaction": config.and_then(|cfg| cfg.get("chat_pii_redaction")).cloned(),
        "total_endpoints": total_endpoints,
        "active_endpoints": active_endpoints,
        "total_keys": total_keys,
        "active_keys": active_keys,
        "total_models": total_models,
        "active_models": active_models,
        "global_model_ids": active_global_model_ids,
        "avg_health_score": avg_health_score,
        "unhealthy_endpoints": unhealthy_endpoints,
        "api_formats": api_formats,
        "endpoint_health_details": endpoint_health_details,
        "ops_configured": ops_configured,
        "ops_architecture_id": ops_architecture_id,
        "created_at": endpoint_timestamp_or_now(provider.created_at_unix_ms, now_unix_secs),
        "updated_at": endpoint_timestamp_or_now(provider.updated_at_unix_secs, now_unix_secs),
    })
}
