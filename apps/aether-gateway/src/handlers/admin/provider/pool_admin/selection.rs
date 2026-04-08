use crate::handlers::admin::shared::decrypt_catalog_secret_with_fallbacks;
use crate::AppState;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

fn admin_pool_reason_indicates_ban(reason: &str) -> bool {
    let normalized = reason.trim().to_ascii_lowercase();
    !normalized.is_empty()
        && [
            "banned",
            "forbidden",
            "blocked",
            "suspend",
            "deactivated",
            "disabled",
            "verification",
            "workspace",
            "受限",
            "封",
            "禁",
        ]
        .iter()
        .any(|hint| normalized.contains(hint))
}

pub(super) fn admin_pool_normalize_text(value: impl AsRef<str>) -> String {
    value.as_ref().trim().to_ascii_lowercase()
}

fn admin_pool_parse_auth_config_json(
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

fn admin_pool_derive_oauth_plan_type(
    state: &AppState,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
) -> Option<String> {
    let normalize = |value: &str| {
        let mut text = value.trim().to_string();
        if text.is_empty() {
            return None;
        }
        let provider_type = provider_type.trim().to_ascii_lowercase();
        if !provider_type.is_empty() && text.to_ascii_lowercase().starts_with(&provider_type) {
            text = text[provider_type.len()..]
                .trim_matches(|ch: char| [' ', ':', '-', '_'].contains(&ch))
                .to_string();
        }
        if text.is_empty() {
            None
        } else {
            Some(text.to_ascii_lowercase())
        }
    };

    if key.auth_type.trim() != "oauth" {
        return None;
    }

    if let Some(upstream_metadata) = key
        .upstream_metadata
        .as_ref()
        .and_then(serde_json::Value::as_object)
    {
        let provider_bucket = upstream_metadata
            .get(&provider_type.trim().to_ascii_lowercase())
            .and_then(serde_json::Value::as_object);
        for source in provider_bucket
            .into_iter()
            .chain(std::iter::once(upstream_metadata))
        {
            for plan_key in [
                "plan_type",
                "tier",
                "subscription_title",
                "subscription_plan",
            ] {
                if let Some(value) = source.get(plan_key).and_then(serde_json::Value::as_str) {
                    if let Some(normalized) = normalize(value) {
                        return Some(normalized);
                    }
                }
            }
        }
    }

    if let Some(auth_config) = admin_pool_parse_auth_config_json(state, key) {
        for plan_key in ["plan_type", "tier", "plan", "subscription_plan"] {
            if let Some(value) = auth_config
                .get(plan_key)
                .and_then(serde_json::Value::as_str)
            {
                if let Some(normalized) = normalize(value) {
                    return Some(normalized);
                }
            }
        }
    }

    None
}

fn admin_pool_has_proxy(key: &StoredProviderCatalogKey) -> bool {
    match key.proxy.as_ref() {
        Some(serde_json::Value::Object(values)) => !values.is_empty(),
        Some(serde_json::Value::String(value)) => !value.trim().is_empty(),
        Some(serde_json::Value::Bool(value)) => *value,
        Some(serde_json::Value::Number(_)) => true,
        Some(serde_json::Value::Array(values)) => !values.is_empty(),
        _ => false,
    }
}

fn admin_pool_is_oauth_invalid(key: &StoredProviderCatalogKey) -> bool {
    if key.auth_type.trim() != "oauth" {
        return false;
    }
    if key
        .oauth_invalid_reason
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    key.expires_at_unix_secs
        .is_some_and(|value| value > 0 && value <= chrono::Utc::now().timestamp().max(0) as u64)
}

pub(super) fn admin_pool_matches_quick_selector(
    state: &AppState,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    selector: &str,
) -> bool {
    match selector {
        "banned" => admin_pool_key_is_known_banned(key),
        "oauth_invalid" => admin_pool_is_oauth_invalid(key),
        "proxy_unset" => !admin_pool_has_proxy(key),
        "proxy_set" => admin_pool_has_proxy(key),
        "disabled" => !key.is_active,
        "enabled" => key.is_active,
        "plan_free" => admin_pool_derive_oauth_plan_type(state, key, provider_type)
            .is_some_and(|value| value.contains("free")),
        "plan_team" => admin_pool_derive_oauth_plan_type(state, key, provider_type)
            .is_some_and(|value| value.contains("team")),
        "no_5h_limit" | "no_weekly_limit" => false,
        _ => false,
    }
}

pub(super) fn admin_pool_matches_search(
    state: &AppState,
    key: &StoredProviderCatalogKey,
    provider_type: &str,
    search: Option<&str>,
) -> bool {
    let Some(search) = search else {
        return true;
    };
    let search = admin_pool_normalize_text(search);
    if search.is_empty() {
        return true;
    }

    let oauth_plan_type = admin_pool_derive_oauth_plan_type(state, key, provider_type);
    let mut search_fields = vec![
        key.id.clone(),
        key.name.clone(),
        key.auth_type.clone(),
        if key.is_active {
            "已启用".to_string()
        } else {
            "已禁用".to_string()
        },
        if admin_pool_has_proxy(key) {
            "独立代理".to_string()
        } else {
            "未配置代理".to_string()
        },
    ];
    if let Some(reason) = key.oauth_invalid_reason.as_ref() {
        search_fields.push(reason.clone());
    }
    if let Some(note) = key.note.as_ref() {
        search_fields.push(note.clone());
    }
    if let Some(plan_type) = oauth_plan_type {
        search_fields.push(plan_type);
    }

    search_fields
        .into_iter()
        .any(|value| admin_pool_normalize_text(&value).contains(&search))
}

pub(super) fn admin_pool_key_is_known_banned(key: &StoredProviderCatalogKey) -> bool {
    if key
        .oauth_invalid_reason
        .as_deref()
        .is_some_and(admin_pool_reason_indicates_ban)
    {
        return true;
    }

    let Some(account) = key
        .status_snapshot
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .and_then(|snapshot| snapshot.get("account"))
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };

    if !account
        .get("blocked")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return false;
    }

    account
        .get("code")
        .and_then(serde_json::Value::as_str)
        .is_some_and(admin_pool_reason_indicates_ban)
        || account
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(admin_pool_reason_indicates_ban)
}

pub(super) fn admin_pool_sort_keys(keys: &mut [StoredProviderCatalogKey]) {
    keys.sort_by(|left, right| {
        left.internal_priority
            .cmp(&right.internal_priority)
            .then(left.name.cmp(&right.name))
            .then(left.id.cmp(&right.id))
    });
}
