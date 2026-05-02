use crate::handlers::admin::request::AdminAppState;
use aether_crypto::decrypt_python_fernet_ciphertext;
#[cfg(test)]
use aether_crypto::DEVELOPMENT_ENCRYPTION_KEY;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

pub(super) fn admin_monitoring_masked_user_api_key_prefix(
    state: &AdminAppState<'_>,
    ciphertext: Option<&str>,
) -> Option<String> {
    let Some(ciphertext) = ciphertext.map(str::trim).filter(|value| !value.is_empty()) else {
        return None;
    };
    let full_key = admin_monitoring_try_decrypt_secret(state, ciphertext)?;
    let prefix_len = full_key.len().min(10);
    let prefix = &full_key[..prefix_len];
    let suffix = if full_key.len() >= 4 {
        &full_key[full_key.len().saturating_sub(4)..]
    } else {
        ""
    };
    Some(format!("{prefix}...{suffix}"))
}

pub(super) fn admin_monitoring_masked_provider_key_prefix(
    state: &AdminAppState<'_>,
    key: &StoredProviderCatalogKey,
) -> Option<String> {
    match key.auth_type.trim() {
        "service_account" | "vertex_ai" => Some("[Service Account]".to_string()),
        "oauth" => Some("[OAuth Token]".to_string()),
        _ => {
            let full_key = key
                .encrypted_api_key
                .as_deref()
                .and_then(|ciphertext| admin_monitoring_try_decrypt_secret(state, ciphertext))?;
            if full_key.len() <= 12 {
                Some(format!("{full_key}***"))
            } else {
                Some(format!(
                    "{}***{}",
                    &full_key[..8],
                    &full_key[full_key.len().saturating_sub(4)..]
                ))
            }
        }
    }
}

fn admin_monitoring_try_decrypt_secret(
    state: &AdminAppState<'_>,
    ciphertext: &str,
) -> Option<String> {
    let ciphertext = ciphertext.trim();
    if ciphertext.is_empty() {
        return None;
    }
    let encryption_key = state.encryption_key().map(str::trim).unwrap_or("");
    if !encryption_key.is_empty() {
        if let Ok(value) = decrypt_python_fernet_ciphertext(encryption_key, ciphertext) {
            return Some(value);
        }
    }
    for env_key in ["AETHER_GATEWAY_DATA_ENCRYPTION_KEY", "ENCRYPTION_KEY"] {
        let Ok(candidate) = std::env::var(env_key) else {
            continue;
        };
        let candidate = candidate.trim();
        if candidate.is_empty() || candidate == encryption_key {
            continue;
        }
        if let Ok(value) = decrypt_python_fernet_ciphertext(candidate, ciphertext) {
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

pub(super) fn admin_monitoring_cache_affinity_sort_value(value: Option<&serde_json::Value>) -> f64 {
    let Some(value) = value else {
        return 0.0;
    };
    if let Some(number) = value.as_f64() {
        return number;
    }
    if let Some(number) = value.as_i64() {
        return number as f64;
    }
    if let Some(number) = value.as_u64() {
        return number as f64;
    }
    if let Some(text) = value.as_str() {
        if let Ok(number) = text.parse::<f64>() {
            return number;
        }
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
            return parsed.timestamp() as f64;
        }
    }
    0.0
}
