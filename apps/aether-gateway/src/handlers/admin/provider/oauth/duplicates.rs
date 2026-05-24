use crate::handlers::admin::request::AdminAppState;
use crate::provider_key_auth::provider_key_is_oauth_managed;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;
use std::time::{SystemTime, UNIX_EPOCH};

fn normalize_codex_plan_group_for_provider_oauth(
    plan_type: Option<&serde_json::Value>,
) -> Option<String> {
    let normalized = plan_type
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_ascii_lowercase();
    match normalized.as_str() {
        "free" => Some("free".to_string()),
        "team" | "plus" | "enterprise" => Some("team_plus_enterprise".to_string()),
        _ => None,
    }
}

fn normalize_provider_oauth_identity_value(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn is_openai_provider_oauth_provider_type(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|provider_type| {
            provider_type.eq_ignore_ascii_case("codex")
                || provider_type.eq_ignore_ascii_case("chatgpt_web")
        })
}

fn is_windsurf_provider_oauth_provider_type(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|provider_type| provider_type.eq_ignore_ascii_case("windsurf"))
}

fn match_codex_provider_oauth_identity(
    new_auth_config: &serde_json::Map<String, serde_json::Value>,
    existing_auth_config: &serde_json::Map<String, serde_json::Value>,
) -> Option<bool> {
    let new_provider_type = new_auth_config.get("provider_type");
    let existing_provider_type = existing_auth_config.get("provider_type");
    if !is_openai_provider_oauth_provider_type(new_provider_type)
        && !is_openai_provider_oauth_provider_type(existing_provider_type)
    {
        return None;
    }

    let new_account_user_id =
        normalize_provider_oauth_identity_value(new_auth_config.get("account_user_id"));
    let existing_account_user_id =
        normalize_provider_oauth_identity_value(existing_auth_config.get("account_user_id"));
    if let (Some(new_account_user_id), Some(existing_account_user_id)) =
        (new_account_user_id, existing_account_user_id)
    {
        return Some(new_account_user_id == existing_account_user_id);
    }

    let new_account_id = normalize_provider_oauth_identity_value(new_auth_config.get("account_id"));
    let existing_account_id =
        normalize_provider_oauth_identity_value(existing_auth_config.get("account_id"));
    let new_user_id = normalize_provider_oauth_identity_value(new_auth_config.get("user_id"));
    let existing_user_id =
        normalize_provider_oauth_identity_value(existing_auth_config.get("user_id"));
    let new_email = normalize_provider_oauth_identity_value(new_auth_config.get("email"));
    let existing_email = normalize_provider_oauth_identity_value(existing_auth_config.get("email"));

    if let (Some(new_account_id), Some(existing_account_id)) =
        (new_account_id.as_deref(), existing_account_id.as_deref())
    {
        if new_account_id != existing_account_id {
            return Some(false);
        }
    }

    if let (
        Some(new_account_id),
        Some(existing_account_id),
        Some(new_user_id),
        Some(existing_user_id),
    ) = (
        new_account_id.as_deref(),
        existing_account_id.as_deref(),
        new_user_id.as_deref(),
        existing_user_id.as_deref(),
    ) {
        return Some(new_account_id == existing_account_id && new_user_id == existing_user_id);
    }

    if let (
        Some(new_account_id),
        Some(existing_account_id),
        Some(new_email),
        Some(existing_email),
    ) = (
        new_account_id.as_deref(),
        existing_account_id.as_deref(),
        new_email.as_deref(),
        existing_email.as_deref(),
    ) {
        return Some(new_account_id == existing_account_id && new_email == existing_email);
    }

    None
}

fn match_windsurf_provider_oauth_identity(
    new_auth_config: &serde_json::Map<String, serde_json::Value>,
    existing_auth_config: &serde_json::Map<String, serde_json::Value>,
) -> Option<bool> {
    let new_provider_type = new_auth_config.get("provider_type");
    let existing_provider_type = existing_auth_config.get("provider_type");
    if !is_windsurf_provider_oauth_provider_type(new_provider_type)
        && !is_windsurf_provider_oauth_provider_type(existing_provider_type)
    {
        return None;
    }

    let new_account_id = normalize_provider_oauth_identity_value(new_auth_config.get("account_id"));
    let existing_account_id =
        normalize_provider_oauth_identity_value(existing_auth_config.get("account_id"));
    if let (Some(new_account_id), Some(existing_account_id)) =
        (new_account_id.as_deref(), existing_account_id.as_deref())
    {
        return Some(new_account_id == existing_account_id);
    }

    let new_credential_fingerprint =
        normalize_provider_oauth_identity_value(new_auth_config.get("credential_fingerprint"));
    let existing_credential_fingerprint =
        normalize_provider_oauth_identity_value(existing_auth_config.get("credential_fingerprint"));
    if let (Some(new_fingerprint), Some(existing_fingerprint)) = (
        new_credential_fingerprint.as_deref(),
        existing_credential_fingerprint.as_deref(),
    ) {
        return Some(new_fingerprint == existing_fingerprint);
    }

    None
}

fn is_codex_cross_plan_group_non_duplicate(
    new_auth_config: &serde_json::Map<String, serde_json::Value>,
    existing_auth_config: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let new_provider_type = new_auth_config.get("provider_type");
    let existing_provider_type = existing_auth_config.get("provider_type");
    if !is_openai_provider_oauth_provider_type(new_provider_type)
        && !is_openai_provider_oauth_provider_type(existing_provider_type)
    {
        return false;
    }

    let new_group = normalize_codex_plan_group_for_provider_oauth(new_auth_config.get("plan_type"));
    let existing_group =
        normalize_codex_plan_group_for_provider_oauth(existing_auth_config.get("plan_type"));
    matches!(
        (new_group.as_deref(), existing_group.as_deref()),
        (Some(left), Some(right)) if left != right
    )
}

fn provider_oauth_invalid_reason_allows_replace(reason: &str) -> bool {
    reason.lines().map(str::trim).any(|line| {
        line.starts_with("[OAUTH_EXPIRED] ")
            || line.starts_with("[REFRESH_FAILED] ")
            || line.contains("Token 无效或已过期")
            || line.contains("refresh_token 无效、已过期或已撤销")
    })
}

fn existing_provider_oauth_key_is_replaceable(existing_key: &StoredProviderCatalogKey) -> bool {
    if !existing_key.is_active {
        return true;
    }

    let now_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    if existing_key
        .expires_at_unix_secs
        .is_some_and(|expires_at| expires_at <= now_unix_secs)
    {
        return true;
    }

    existing_key
        .oauth_invalid_reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .is_some_and(provider_oauth_invalid_reason_allows_replace)
}

pub(crate) async fn find_duplicate_provider_oauth_key(
    state: &AdminAppState<'_>,
    provider_id: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    exclude_key_id: Option<&str>,
) -> Result<Option<StoredProviderCatalogKey>, String> {
    let new_email = normalize_provider_oauth_identity_value(auth_config.get("email"));
    let new_user_id = normalize_provider_oauth_identity_value(auth_config.get("user_id"));
    let new_account_id = normalize_provider_oauth_identity_value(auth_config.get("account_id"));
    let new_credential_fingerprint =
        normalize_provider_oauth_identity_value(auth_config.get("credential_fingerprint"));
    let new_auth_method = normalize_provider_oauth_identity_value(auth_config.get("auth_method"));
    let new_kiro_provider = normalize_provider_oauth_identity_value(auth_config.get("provider"));

    if new_email.is_none()
        && new_user_id.is_none()
        && new_account_id.is_none()
        && new_credential_fingerprint.is_none()
    {
        return Ok(None);
    }

    let existing_keys = state
        .list_provider_catalog_keys_by_provider_ids(&[provider_id.to_string()])
        .await
        .map_err(|err| format!("{err:?}"))?;

    let provider_type = auth_config
        .get("provider_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    for existing_key in existing_keys.into_iter().filter(|key| {
        provider_key_is_oauth_managed(key, provider_type.as_str())
            && exclude_key_id.is_none_or(|exclude| key.id != exclude)
    }) {
        let Some(existing_auth_config) = state.parse_catalog_auth_config_json(&existing_key) else {
            continue;
        };
        let existing_email =
            normalize_provider_oauth_identity_value(existing_auth_config.get("email"));
        let existing_user_id =
            normalize_provider_oauth_identity_value(existing_auth_config.get("user_id"));
        let existing_auth_method =
            normalize_provider_oauth_identity_value(existing_auth_config.get("auth_method"));
        let existing_kiro_provider =
            normalize_provider_oauth_identity_value(existing_auth_config.get("provider"));
        let is_windsurf = auth_config
            .get("provider_type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("windsurf"))
            || existing_auth_config
                .get("provider_type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("windsurf"));

        let mut is_duplicate = false;
        let codex_identity_match =
            match_codex_provider_oauth_identity(auth_config, &existing_auth_config);
        let windsurf_identity_match =
            match_windsurf_provider_oauth_identity(auth_config, &existing_auth_config);
        if let Some(codex_identity_match) = codex_identity_match {
            is_duplicate = codex_identity_match;
        } else if let Some(windsurf_identity_match) = windsurf_identity_match {
            is_duplicate = windsurf_identity_match;
        }

        if codex_identity_match.is_none()
            && windsurf_identity_match.is_none()
            && !is_duplicate
            && new_user_id.is_some()
            && existing_user_id.is_some()
            && new_user_id == existing_user_id
            && !is_codex_cross_plan_group_non_duplicate(auth_config, &existing_auth_config)
        {
            is_duplicate = true;
        }

        if codex_identity_match.is_none()
            && windsurf_identity_match.is_none()
            && !is_duplicate
            && !is_windsurf
            && new_email.is_some()
            && existing_email.is_some()
            && new_email == existing_email
        {
            let is_kiro = auth_config
                .get("provider_type")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("kiro"))
                || existing_auth_config
                    .get("provider_type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case("kiro"));
            if is_kiro {
                if new_auth_method.is_some()
                    && existing_auth_method.is_some()
                    && new_auth_method
                        .as_deref()
                        .zip(existing_auth_method.as_deref())
                        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
                    && new_kiro_provider
                        .as_deref()
                        .zip(existing_kiro_provider.as_deref())
                        .is_none_or(|(left, right)| left.eq_ignore_ascii_case(right))
                {
                    is_duplicate = true;
                }
            } else if !is_codex_cross_plan_group_non_duplicate(auth_config, &existing_auth_config) {
                is_duplicate = true;
            }
        }

        if !is_duplicate {
            continue;
        }
        if existing_provider_oauth_key_is_replaceable(&existing_key) {
            return Ok(Some(existing_key));
        }
        let identifier =
            normalize_provider_oauth_identity_value(auth_config.get("account_user_id"))
                .or_else(|| normalize_provider_oauth_identity_value(auth_config.get("account_id")))
                .or_else(|| {
                    normalize_provider_oauth_identity_value(
                        auth_config.get("credential_fingerprint"),
                    )
                    .map(|value| format!("fingerprint:{value}"))
                })
                .or_else(|| new_email.clone())
                .or_else(|| new_user_id.clone())
                .unwrap_or_default();
        return Err(format!(
            "该 OAuth 账号 ({identifier}) 已存在于当前 Provider 中（名称: {}）",
            existing_key.name
        ));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::match_windsurf_provider_oauth_identity;
    use serde_json::{json, Map, Value};

    fn auth_config(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("auth config object")
    }

    #[test]
    fn windsurf_identity_matches_account_id_without_email() {
        let new_auth_config = auth_config(json!({
            "provider_type": "windsurf",
            "auth_method": "api_key",
            "account_id": "acct-ws-1"
        }));
        let existing_auth_config = auth_config(json!({
            "provider_type": "windsurf",
            "auth_method": "browser",
            "account_id": "acct-ws-1"
        }));

        assert_eq!(
            match_windsurf_provider_oauth_identity(&new_auth_config, &existing_auth_config),
            Some(true)
        );
    }

    #[test]
    fn windsurf_identity_rejects_different_account_id() {
        let new_auth_config = auth_config(json!({
            "provider_type": "windsurf",
            "account_id": "acct-ws-1",
            "email": "same@example.com"
        }));
        let existing_auth_config = auth_config(json!({
            "provider_type": "windsurf",
            "account_id": "acct-ws-2",
            "email": "same@example.com"
        }));

        assert_eq!(
            match_windsurf_provider_oauth_identity(&new_auth_config, &existing_auth_config),
            Some(false)
        );
    }

    #[test]
    fn windsurf_identity_matches_credential_fingerprint_without_profile() {
        let new_auth_config = auth_config(json!({
            "provider_type": "windsurf",
            "auth_method": "api_key",
            "credential_fingerprint": "abcdef0123456789"
        }));
        let existing_auth_config = auth_config(json!({
            "provider_type": "windsurf",
            "auth_method": "browser",
            "credential_fingerprint": "abcdef0123456789"
        }));

        assert_eq!(
            match_windsurf_provider_oauth_identity(&new_auth_config, &existing_auth_config),
            Some(true)
        );
    }

    #[test]
    fn windsurf_identity_does_not_match_user_supplied_email_only() {
        let new_auth_config = auth_config(json!({
            "provider_type": "windsurf",
            "auth_method": "api_key",
            "email": "same@example.com",
            "email_verified": false
        }));
        let existing_auth_config = auth_config(json!({
            "provider_type": "windsurf",
            "auth_method": "api_key",
            "email": "same@example.com",
            "email_verified": false
        }));

        assert_eq!(
            match_windsurf_provider_oauth_identity(&new_auth_config, &existing_auth_config),
            None
        );
    }
}
