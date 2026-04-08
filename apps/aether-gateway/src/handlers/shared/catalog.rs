use crate::handlers::shared::{json_string_list, unix_secs_to_rfc3339};
use crate::AppState;
#[cfg(test)]
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_crypto::{decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext};
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use serde_json::json;

pub(crate) fn provider_catalog_key_supports_format(
    key: &StoredProviderCatalogKey,
    api_format: &str,
) -> bool {
    let Some(value) = key.api_formats.as_ref() else {
        return true;
    };
    let Some(values) = value.as_array() else {
        return true;
    };
    values
        .iter()
        .filter_map(serde_json::Value::as_str)
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(api_format))
}

pub(crate) fn decrypt_catalog_secret_with_fallbacks(
    encryption_key: Option<&str>,
    ciphertext: &str,
) -> Option<String> {
    let encryption_key = encryption_key.map(str::trim).unwrap_or("");
    if !encryption_key.is_empty() {
        if let Ok(value) = decrypt_python_fernet_ciphertext(encryption_key, ciphertext) {
            return Some(value);
        }
    }
    for env_key in ["AETHER_GATEWAY_DATA_ENCRYPTION_KEY", "ENCRYPTION_KEY"] {
        let Ok(fallback) = std::env::var(env_key) else {
            continue;
        };
        let fallback = fallback.trim();
        if fallback.is_empty() || fallback == encryption_key {
            continue;
        }
        if let Ok(value) = decrypt_python_fernet_ciphertext(fallback, ciphertext) {
            return Some(value);
        }
    }
    #[cfg(test)]
    if encryption_key != DEVELOPMENT_ENCRYPTION_KEY {
        if let Ok(value) = decrypt_python_fernet_ciphertext(DEVELOPMENT_ENCRYPTION_KEY, ciphertext)
        {
            return Some(value);
        }
    }
    None
}

pub(crate) fn effective_catalog_encryption_key(state: &AppState) -> Option<String> {
    let encryption_key = state.encryption_key().map(str::trim).unwrap_or("");
    if !encryption_key.is_empty() {
        return Some(encryption_key.to_string());
    }
    for env_key in ["AETHER_GATEWAY_DATA_ENCRYPTION_KEY", "ENCRYPTION_KEY"] {
        let Ok(candidate) = std::env::var(env_key) else {
            continue;
        };
        let candidate = candidate.trim();
        if !candidate.is_empty() {
            return Some(candidate.to_string());
        }
    }
    #[cfg(test)]
    {
        return Some(DEVELOPMENT_ENCRYPTION_KEY.to_string());
    }
    #[allow(unreachable_code)]
    None
}

pub(crate) fn encrypt_catalog_secret_with_fallbacks(
    state: &AppState,
    plaintext: &str,
) -> Option<String> {
    let encryption_key = effective_catalog_encryption_key(state)?;
    encrypt_python_fernet_plaintext(&encryption_key, plaintext).ok()
}

pub(crate) fn masked_catalog_api_key(state: &AppState, key: &StoredProviderCatalogKey) -> String {
    match key.auth_type.trim() {
        "service_account" | "vertex_ai" => "[Service Account]".to_string(),
        "oauth" => "[OAuth Token]".to_string(),
        _ => decrypt_catalog_secret_with_fallbacks(state.encryption_key(), &key.encrypted_api_key)
            .map(|value| {
                if value.len() <= 12 {
                    format!("{value}***")
                } else {
                    format!(
                        "{}***{}",
                        &value[..8],
                        &value[value.len().saturating_sub(4)..]
                    )
                }
            })
            .unwrap_or_else(|| "***ERROR***".to_string()),
    }
}

pub(crate) fn parse_catalog_auth_config_json(
    state: &AppState,
    key: &StoredProviderCatalogKey,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let ciphertext = key.encrypted_auth_config.as_deref()?.trim();
    if ciphertext.is_empty() {
        return None;
    }
    let plaintext = decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)?;
    serde_json::from_str::<serde_json::Value>(&plaintext)
        .ok()?
        .as_object()
        .cloned()
}

pub(crate) fn default_provider_key_status_snapshot() -> serde_json::Value {
    json!({
        "oauth": {
            "code": "none",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "expires_at": serde_json::Value::Null,
            "invalid_at": serde_json::Value::Null,
            "source": serde_json::Value::Null,
            "requires_reauth": false,
            "expiring_soon": false,
        },
        "account": {
            "code": "ok",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "blocked": false,
            "source": serde_json::Value::Null,
            "recoverable": false,
        },
        "quota": {
            "code": "unknown",
            "label": serde_json::Value::Null,
            "reason": serde_json::Value::Null,
            "exhausted": false,
            "usage_ratio": serde_json::Value::Null,
            "updated_at": serde_json::Value::Null,
            "reset_seconds": serde_json::Value::Null,
            "plan_type": serde_json::Value::Null,
        }
    })
}

pub(crate) fn provider_key_status_snapshot_payload(
    key: &StoredProviderCatalogKey,
) -> serde_json::Value {
    key.status_snapshot
        .clone()
        .filter(|value| value.is_object())
        .unwrap_or_else(default_provider_key_status_snapshot)
}

pub(crate) fn provider_key_health_summary(
    key: &StoredProviderCatalogKey,
) -> (
    f64,
    i64,
    Option<String>,
    bool,
    serde_json::Map<String, serde_json::Value>,
) {
    let health_by_format = key
        .health_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();
    let circuit_by_format = key
        .circuit_breaker_by_format
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut min_health_score = 1.0_f64;
    let mut max_consecutive = 0_i64;
    let mut last_failure_at: Option<String> = None;
    for value in health_by_format.values() {
        let score = value
            .get("health_score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0);
        min_health_score = min_health_score.min(score);
        let consecutive = value
            .get("consecutive_failures")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        max_consecutive = max_consecutive.max(consecutive);
        if let Some(last_failure) = value
            .get("last_failure_at")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
        {
            if last_failure_at
                .as_ref()
                .is_none_or(|current| last_failure > *current)
            {
                last_failure_at = Some(last_failure);
            }
        }
    }

    let any_circuit_open = circuit_by_format.values().any(|value| {
        value
            .get("open")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });

    (
        if health_by_format.is_empty() {
            1.0
        } else {
            min_health_score
        },
        max_consecutive,
        last_failure_at,
        any_circuit_open,
        circuit_by_format,
    )
}

fn normalize_catalog_oauth_plan_type(value: &str, provider_type: &str) -> Option<String> {
    let mut normalized = value.trim().to_string();
    if normalized.is_empty() {
        return None;
    }

    let provider_type = provider_type.trim().to_ascii_lowercase();
    if !provider_type.is_empty() && normalized.to_ascii_lowercase().starts_with(&provider_type) {
        normalized = normalized[provider_type.len()..]
            .trim_matches(|ch: char| [' ', ':', '-', '_'].contains(&ch))
            .to_string();
    }

    let normalized = normalized.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn catalog_oauth_plan_type_from_source(
    source: &serde_json::Map<String, serde_json::Value>,
    provider_type: &str,
    fields: &[&str],
) -> Option<String> {
    for field in fields {
        let Some(value) = source.get(*field).and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Some(normalized) = normalize_catalog_oauth_plan_type(value, provider_type) {
            return Some(normalized);
        }
    }
    None
}

fn derive_catalog_oauth_plan_type(
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    auth_config: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    if !key.auth_type.trim().eq_ignore_ascii_case("oauth") {
        return None;
    }

    let provider_type_key = provider_type.trim().to_ascii_lowercase();
    if let Some(upstream_metadata) = key
        .upstream_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
    {
        let provider_bucket = if provider_type_key.is_empty() {
            None
        } else {
            upstream_metadata
                .get(&provider_type_key)
                .and_then(serde_json::Value::as_object)
        };
        for source in provider_bucket
            .into_iter()
            .chain(std::iter::once(upstream_metadata))
        {
            if let Some(plan_type) = catalog_oauth_plan_type_from_source(
                source,
                provider_type,
                &[
                    "plan_type",
                    "tier",
                    "subscription_title",
                    "subscription_plan",
                    "plan",
                ],
            ) {
                return Some(plan_type);
            }
        }
    }

    auth_config.and_then(|source| {
        catalog_oauth_plan_type_from_source(
            source,
            provider_type,
            &["plan_type", "tier", "plan", "subscription_plan"],
        )
    })
}

pub(crate) fn build_admin_provider_key_response(
    state: &AppState,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    now_unix_secs: u64,
) -> serde_json::Value {
    let request_count = u64::from(key.request_count.unwrap_or(0));
    let success_count = u64::from(key.success_count.unwrap_or(0));
    let error_count = u64::from(key.error_count.unwrap_or(0));
    let total_response_time_ms = f64::from(key.total_response_time_ms.unwrap_or(0));
    let success_rate = if request_count > 0 {
        success_count as f64 / request_count as f64
    } else {
        0.0
    };
    let avg_response_time_ms = if success_count > 0 {
        total_response_time_ms / success_count as f64
    } else {
        0.0
    };
    let auth_config = parse_catalog_auth_config_json(state, key);
    let oauth_organizations = auth_config
        .as_ref()
        .and_then(|config| config.get("organizations"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let oauth_plan_type = derive_catalog_oauth_plan_type(key, provider_type, auth_config.as_ref());
    let (
        health_score,
        consecutive_failures,
        last_failure_at,
        circuit_breaker_open,
        circuit_by_format,
    ) = provider_key_health_summary(key);
    let circuit_sample = circuit_by_format
        .values()
        .find(|value| {
            value
                .get("open")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .or_else(|| circuit_by_format.values().next());
    let is_adaptive = key.rpm_limit.is_none();
    let effective_limit = if is_adaptive {
        key.learned_rpm_limit
    } else {
        key.rpm_limit
    };
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), json!(key.id));
    payload.insert("provider_id".to_string(), json!(key.provider_id));
    payload.insert(
        "api_formats".to_string(),
        serde_json::Value::Array(
            json_string_list(key.api_formats.as_ref())
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    payload.insert(
        "api_key_masked".to_string(),
        json!(masked_catalog_api_key(state, key)),
    );
    payload.insert("api_key_plain".to_string(), serde_json::Value::Null);
    payload.insert("auth_type".to_string(), json!(key.auth_type));
    payload.insert("name".to_string(), json!(key.name));
    payload.insert("rate_multipliers".to_string(), json!(key.rate_multipliers));
    payload.insert(
        "internal_priority".to_string(),
        json!(key.internal_priority),
    );
    payload.insert(
        "global_priority_by_format".to_string(),
        json!(key.global_priority_by_format),
    );
    payload.insert("rpm_limit".to_string(), json!(key.rpm_limit));
    payload.insert(
        "allowed_models".to_string(),
        serde_json::Value::Array(
            json_string_list(key.allowed_models.as_ref())
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    payload.insert("capabilities".to_string(), json!(key.capabilities));
    payload.insert(
        "oauth_expires_at".to_string(),
        json!(key.expires_at_unix_secs),
    );
    payload.insert(
        "oauth_email".to_string(),
        auth_config
            .as_ref()
            .and_then(|config| config.get("email"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert("oauth_plan_type".to_string(), json!(oauth_plan_type));
    payload.insert(
        "oauth_account_id".to_string(),
        auth_config
            .as_ref()
            .and_then(|config| config.get("account_id"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "oauth_account_name".to_string(),
        auth_config
            .as_ref()
            .and_then(|config| config.get("account_name"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "oauth_account_user_id".to_string(),
        auth_config
            .as_ref()
            .and_then(|config| config.get("account_user_id"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "oauth_organizations".to_string(),
        serde_json::Value::Array(oauth_organizations),
    );
    payload.insert(
        "oauth_invalid_at".to_string(),
        json!(key.oauth_invalid_at_unix_secs),
    );
    payload.insert(
        "oauth_invalid_reason".to_string(),
        json!(key.oauth_invalid_reason),
    );
    payload.insert(
        "status_snapshot".to_string(),
        provider_key_status_snapshot_payload(key),
    );
    payload.insert(
        "cache_ttl_minutes".to_string(),
        json!(key.cache_ttl_minutes),
    );
    payload.insert(
        "max_probe_interval_minutes".to_string(),
        json!(key.max_probe_interval_minutes),
    );
    payload.insert("health_by_format".to_string(), json!(key.health_by_format));
    payload.insert(
        "circuit_breaker_by_format".to_string(),
        json!(key.circuit_breaker_by_format),
    );
    payload.insert("health_score".to_string(), json!(health_score));
    payload.insert(
        "consecutive_failures".to_string(),
        json!(consecutive_failures),
    );
    payload.insert("last_failure_at".to_string(), json!(last_failure_at));
    payload.insert(
        "circuit_breaker_open".to_string(),
        json!(circuit_breaker_open),
    );
    payload.insert(
        "circuit_breaker_open_at".to_string(),
        circuit_sample
            .and_then(|value| value.get("open_at"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "next_probe_at".to_string(),
        circuit_sample
            .and_then(|value| value.get("next_probe_at"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "half_open_until".to_string(),
        circuit_sample
            .and_then(|value| value.get("half_open_until"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert(
        "half_open_successes".to_string(),
        json!(circuit_sample
            .and_then(|value| value.get("half_open_successes"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)),
    );
    payload.insert(
        "half_open_failures".to_string(),
        json!(circuit_sample
            .and_then(|value| value.get("half_open_failures"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)),
    );
    payload.insert(
        "request_results_window".to_string(),
        circuit_sample
            .and_then(|value| value.get("request_results_window"))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    payload.insert("request_count".to_string(), json!(request_count));
    payload.insert("success_count".to_string(), json!(success_count));
    payload.insert("error_count".to_string(), json!(error_count));
    payload.insert("success_rate".to_string(), json!(success_rate));
    payload.insert(
        "avg_response_time_ms".to_string(),
        json!(avg_response_time_ms),
    );
    payload.insert("is_active".to_string(), json!(key.is_active));
    payload.insert("is_adaptive".to_string(), json!(is_adaptive));
    payload.insert(
        "learned_rpm_limit".to_string(),
        json!(key.learned_rpm_limit),
    );
    payload.insert("effective_limit".to_string(), json!(effective_limit));
    payload.insert(
        "utilization_samples".to_string(),
        json!(key.utilization_samples),
    );
    payload.insert(
        "last_probe_increase_at".to_string(),
        json!(key
            .last_probe_increase_at_unix_secs
            .and_then(unix_secs_to_rfc3339)),
    );
    payload.insert(
        "concurrent_429_count".to_string(),
        json!(key.concurrent_429_count),
    );
    payload.insert("rpm_429_count".to_string(), json!(key.rpm_429_count));
    payload.insert(
        "last_429_at".to_string(),
        json!(key.last_429_at_unix_secs.and_then(unix_secs_to_rfc3339)),
    );
    payload.insert("last_429_type".to_string(), json!(key.last_429_type));
    payload.insert("note".to_string(), json!(key.note));
    payload.insert(
        "auto_fetch_models".to_string(),
        json!(key.auto_fetch_models),
    );
    payload.insert(
        "last_models_fetch_at".to_string(),
        json!(key
            .last_models_fetch_at_unix_secs
            .and_then(unix_secs_to_rfc3339)),
    );
    payload.insert(
        "last_models_fetch_error".to_string(),
        json!(key.last_models_fetch_error),
    );
    payload.insert("locked_models".to_string(), json!(key.locked_models));
    payload.insert(
        "model_include_patterns".to_string(),
        json!(key.model_include_patterns),
    );
    payload.insert(
        "model_exclude_patterns".to_string(),
        json!(key.model_exclude_patterns),
    );
    payload.insert(
        "upstream_metadata".to_string(),
        json!(key.upstream_metadata),
    );
    payload.insert("proxy".to_string(), json!(key.proxy));
    payload.insert("fingerprint".to_string(), json!(key.fingerprint));
    payload.insert(
        "last_used_at".to_string(),
        json!(key.last_used_at_unix_secs.and_then(unix_secs_to_rfc3339)),
    );
    payload.insert(
        "created_at".to_string(),
        json!(unix_secs_to_rfc3339(
            key.created_at_unix_secs.unwrap_or(now_unix_secs)
        )),
    );
    payload.insert(
        "updated_at".to_string(),
        json!(unix_secs_to_rfc3339(
            key.updated_at_unix_secs.unwrap_or(now_unix_secs)
        )),
    );
    serde_json::Value::Object(payload)
}
