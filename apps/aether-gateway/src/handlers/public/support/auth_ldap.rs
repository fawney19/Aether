use super::{
    decrypt_or_migrate_ldap_bind_password, ldap_config_is_enabled, module_available_from_env,
    normalize_auth_login_identifier, system_config_bool, AppState, GatewayError,
};

#[derive(Clone)]
pub(super) struct AuthLdapRuntimeConfig {
    server_url: String,
    bind_dn: String,
    bind_password: String,
    base_dn: String,
    user_search_filter: String,
    username_attr: String,
    email_attr: String,
    display_name_attr: String,
    use_starttls: bool,
    connect_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub(super) struct AuthLdapAuthenticatedUser {
    pub(super) username: String,
    pub(super) ldap_username: String,
    pub(super) ldap_dn: String,
    pub(super) email: String,
    pub(super) display_name: String,
}

pub(super) async fn auth_local_login_allowed_for_user(
    state: &AppState,
    user: Option<&aether_data::repository::users::StoredUserAuthRecord>,
) -> Result<bool, GatewayError> {
    let ldap_enabled_config = state
        .read_system_config_json_value("module.ldap.enabled")
        .await?;
    let ldap_config = state.get_ldap_module_config().await?;
    let ldap_enabled = module_available_from_env("LDAP_AVAILABLE", true)
        && system_config_bool(ldap_enabled_config.as_ref(), false)
        && ldap_config_is_enabled(ldap_config.as_ref());
    let ldap_exclusive = ldap_enabled
        && ldap_config
            .as_ref()
            .map(|config| config.is_exclusive)
            .unwrap_or(false);
    if !ldap_exclusive {
        return Ok(true);
    }
    Ok(user.is_some_and(|user| {
        user.role.eq_ignore_ascii_case("admin") && user.auth_source.eq_ignore_ascii_case("local")
    }))
}

fn auth_ldap_default_search_filter(username_attr: &str) -> String {
    format!("({username_attr}={{username}})")
}

fn auth_ldap_escape_filter(value: &str) -> Result<String, GatewayError> {
    use std::fmt::Write as _;

    let normalized = value.trim();
    if normalized.chars().count() > 128 {
        return Err(GatewayError::Internal(
            "ldap filter value too long".to_string(),
        ));
    }
    let mut escaped = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        match ch {
            '\\' => escaped.push_str(r"\5c"),
            '*' => escaped.push_str(r"\2a"),
            '(' => escaped.push_str(r"\28"),
            ')' => escaped.push_str(r"\29"),
            '\0' => escaped.push_str(r"\00"),
            '&' => escaped.push_str(r"\26"),
            '|' => escaped.push_str(r"\7c"),
            '=' => escaped.push_str(r"\3d"),
            '>' => escaped.push_str(r"\3e"),
            '<' => escaped.push_str(r"\3c"),
            '~' => escaped.push_str(r"\7e"),
            '!' => escaped.push_str(r"\21"),
            _ if ch.is_control() => {
                let _ = write!(&mut escaped, "\\{:02x}", ch as u32);
            }
            _ => escaped.push(ch),
        }
    }
    Ok(escaped)
}

fn auth_ldap_normalize_server_url(server_url: &str, use_starttls: bool) -> Option<String> {
    crate::handlers::shared::normalize_ldap_transport_server_url(server_url, use_starttls)
}

async fn read_auth_ldap_runtime_config(
    state: &AppState,
) -> Result<Option<AuthLdapRuntimeConfig>, GatewayError> {
    let ldap_enabled_config = state
        .read_system_config_json_value("module.ldap.enabled")
        .await?;
    let ldap_config = state.get_ldap_module_config().await?;
    let Some(config) = ldap_config.filter(|config| {
        module_available_from_env("LDAP_AVAILABLE", true)
            && system_config_bool(ldap_enabled_config.as_ref(), false)
            && ldap_config_is_enabled(Some(config))
    }) else {
        return Ok(None);
    };
    let Some(server_url) = auth_ldap_normalize_server_url(&config.server_url, config.use_starttls)
    else {
        return Ok(None);
    };
    let Some(bind_password) = decrypt_or_migrate_ldap_bind_password(state, &config).await? else {
        return Ok(None);
    };

    let username_attr = config
        .username_attr
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("uid")
        .to_string();
    let default_search_filter = auth_ldap_default_search_filter(&username_attr);
    Ok(Some(AuthLdapRuntimeConfig {
        server_url,
        bind_dn: config.bind_dn.trim().to_string(),
        bind_password,
        base_dn: config.base_dn.trim().to_string(),
        user_search_filter: config
            .user_search_filter
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(default_search_filter.as_str())
            .to_string(),
        username_attr,
        email_attr: config
            .email_attr
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("mail")
            .to_string(),
        display_name_attr: config
            .display_name_attr
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("displayName")
            .to_string(),
        use_starttls: config.use_starttls,
        connect_timeout_secs: u64::try_from(config.connect_timeout.unwrap_or(10).max(1))
            .unwrap_or(10),
    }))
}

#[cfg(test)]
fn authenticate_auth_ldap_user_mock(
    config: &AuthLdapRuntimeConfig,
    identifier: &str,
    password: &str,
) -> Option<AuthLdapAuthenticatedUser> {
    if !config.server_url.starts_with("mockldap://") {
        return None;
    }
    if password != "secret123" {
        return None;
    }
    let normalized = normalize_auth_login_identifier(identifier);
    let (username, email, display_name) = match normalized.as_str() {
        "alice" | "alice@example.com" => (
            "alice".to_string(),
            "alice@example.com".to_string(),
            "Alice LDAP".to_string(),
        ),
        "bob" | "bob@example.com" => (
            "bob".to_string(),
            "bob@example.com".to_string(),
            "Bob LDAP".to_string(),
        ),
        _ => return None,
    };
    Some(AuthLdapAuthenticatedUser {
        ldap_dn: format!("cn={username},dc=example,dc=com"),
        ldap_username: username.clone(),
        username,
        email,
        display_name,
    })
}

fn authenticate_auth_ldap_user_blocking(
    config: AuthLdapRuntimeConfig,
    identifier: String,
    password: String,
) -> Option<AuthLdapAuthenticatedUser> {
    #[cfg(test)]
    if let Some(user) = authenticate_auth_ldap_user_mock(&config, &identifier, &password) {
        return Some(user);
    }

    let settings = ldap3::LdapConnSettings::new()
        .set_conn_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
        .set_starttls(config.use_starttls && !config.server_url.starts_with("ldaps://"));
    let mut admin = ldap3::LdapConn::with_settings(settings.clone(), &config.server_url).ok()?;
    admin
        .simple_bind(&config.bind_dn, &config.bind_password)
        .ok()?
        .success()
        .ok()?;

    let escaped_identifier = auth_ldap_escape_filter(&identifier).ok()?;
    let search_filter = config
        .user_search_filter
        .replace("{username}", &escaped_identifier);
    let attrs = vec![
        config.username_attr.as_str(),
        config.email_attr.as_str(),
        config.display_name_attr.as_str(),
    ];
    let (entries, _result) = admin
        .search(
            &config.base_dn,
            ldap3::Scope::Subtree,
            &search_filter,
            attrs,
        )
        .ok()?
        .success()
        .ok()?;
    if entries.len() != 1 {
        let _ = admin.unbind();
        return None;
    }

    let entry = ldap3::SearchEntry::construct(entries[0].clone());
    let user_dn = entry.dn;
    let mut user = ldap3::LdapConn::with_settings(settings, &config.server_url).ok()?;
    user.simple_bind(&user_dn, &password).ok()?.success().ok()?;

    let ldap_username = entry
        .attrs
        .get(&config.username_attr)
        .and_then(|values| values.first())
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| identifier.clone());
    let email = entry
        .attrs
        .get(&config.email_attr)
        .and_then(|values| values.first())
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{ldap_username}@ldap.local"));
    let display_name = entry
        .attrs
        .get(&config.display_name_attr)
        .and_then(|values| values.first())
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| ldap_username.clone());

    let _ = admin.unbind();
    let _ = user.unbind();

    Some(AuthLdapAuthenticatedUser {
        username: ldap_username.clone(),
        ldap_username,
        ldap_dn: user_dn,
        email,
        display_name,
    })
}

pub(super) async fn authenticate_auth_ldap_user(
    state: &AppState,
    identifier: &str,
    password: &str,
) -> Result<Option<AuthLdapAuthenticatedUser>, GatewayError> {
    let Some(config) = read_auth_ldap_runtime_config(state).await? else {
        return Ok(None);
    };
    tokio::task::spawn_blocking({
        let identifier = identifier.to_string();
        let password = password.to_string();
        move || authenticate_auth_ldap_user_blocking(config, identifier, password)
    })
    .await
    .map_err(|err| GatewayError::Internal(err.to_string()))
}
