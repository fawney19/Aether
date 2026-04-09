use crate::handlers::admin::request::AdminAppState;
use aether_data_contracts::repository::provider_catalog::StoredProviderCatalogKey;

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

fn is_codex_provider_oauth_provider_type(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .is_some_and(|provider_type| provider_type.eq_ignore_ascii_case("codex"))
}

fn match_codex_provider_oauth_identity(
    new_auth_config: &serde_json::Map<String, serde_json::Value>,
    existing_auth_config: &serde_json::Map<String, serde_json::Value>,
) -> Option<bool> {
    let new_provider_type = new_auth_config.get("provider_type");
    let existing_provider_type = existing_auth_config.get("provider_type");
    if !is_codex_provider_oauth_provider_type(new_provider_type)
        && !is_codex_provider_oauth_provider_type(existing_provider_type)
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

fn is_codex_cross_plan_group_non_duplicate(
    new_auth_config: &serde_json::Map<String, serde_json::Value>,
    existing_auth_config: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let new_provider_type = new_auth_config.get("provider_type");
    let existing_provider_type = existing_auth_config.get("provider_type");
    if !is_codex_provider_oauth_provider_type(new_provider_type)
        && !is_codex_provider_oauth_provider_type(existing_provider_type)
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

pub(crate) async fn find_duplicate_provider_oauth_key(
    state: &AdminAppState<'_>,
    provider_id: &str,
    auth_config: &serde_json::Map<String, serde_json::Value>,
    exclude_key_id: Option<&str>,
) -> Result<Option<StoredProviderCatalogKey>, String> {
    let new_email = normalize_provider_oauth_identity_value(auth_config.get("email"));
    let new_user_id = normalize_provider_oauth_identity_value(auth_config.get("user_id"));
    let new_auth_method = normalize_provider_oauth_identity_value(auth_config.get("auth_method"));

    if new_email.is_none() && new_user_id.is_none() {
        return Ok(None);
    }

    let existing_keys = state
        .list_provider_catalog_keys_by_provider_ids(&[provider_id.to_string()])
        .await
        .map_err(|err| format!("{err:?}"))?;

    for existing_key in existing_keys.into_iter().filter(|key| {
        key.auth_type.trim().eq_ignore_ascii_case("oauth")
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

        let mut is_duplicate = false;
        let codex_identity_match =
            match_codex_provider_oauth_identity(auth_config, &existing_auth_config);
        if let Some(codex_identity_match) = codex_identity_match {
            is_duplicate = codex_identity_match;
        }

        if codex_identity_match.is_none()
            && !is_duplicate
            && new_user_id.is_some()
            && existing_user_id.is_some()
            && new_user_id == existing_user_id
            && !is_codex_cross_plan_group_non_duplicate(auth_config, &existing_auth_config)
        {
            is_duplicate = true;
        }

        if codex_identity_match.is_none()
            && !is_duplicate
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
        if !existing_key.is_active {
            return Ok(Some(existing_key));
        }
        let identifier =
            normalize_provider_oauth_identity_value(auth_config.get("account_user_id"))
                .or_else(|| normalize_provider_oauth_identity_value(auth_config.get("account_id")))
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
