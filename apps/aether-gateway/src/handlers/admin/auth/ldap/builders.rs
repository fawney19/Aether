use super::shared::*;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct AdminLdapConfigUpdateRequest {
    server_url: String,
    bind_dn: String,
    #[serde(default)]
    bind_password: Option<String>,
    base_dn: String,
    #[serde(default = "admin_ldap_default_search_filter")]
    user_search_filter: String,
    #[serde(default = "admin_ldap_default_username_attr")]
    username_attr: String,
    #[serde(default = "admin_ldap_default_email_attr")]
    email_attr: String,
    #[serde(default = "admin_ldap_default_display_name_attr")]
    display_name_attr: String,
    #[serde(default)]
    is_enabled: bool,
    #[serde(default)]
    is_exclusive: bool,
    #[serde(default)]
    use_starttls: bool,
    #[serde(default = "admin_ldap_default_connect_timeout")]
    connect_timeout: i32,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct AdminLdapConfigTestRequest {
    #[serde(default)]
    server_url: Option<String>,
    #[serde(default)]
    bind_dn: Option<String>,
    #[serde(default)]
    bind_password: Option<String>,
    #[serde(default)]
    base_dn: Option<String>,
    #[serde(default)]
    user_search_filter: Option<String>,
    #[serde(default)]
    username_attr: Option<String>,
    #[serde(default)]
    email_attr: Option<String>,
    #[serde(default)]
    display_name_attr: Option<String>,
    #[serde(default)]
    is_enabled: Option<bool>,
    #[serde(default)]
    is_exclusive: Option<bool>,
    #[serde(default)]
    use_starttls: Option<bool>,
    #[serde(default)]
    connect_timeout: Option<i32>,
}

#[derive(Debug, Clone)]
pub(super) struct AdminLdapConnectionTestConfig {
    server_url: String,
    bind_dn: String,
    bind_password: String,
    base_dn: String,
    use_starttls: bool,
    connect_timeout: i32,
}

pub(super) async fn build_admin_ldap_update_config(
    state: &AdminAppState<'_>,
    payload: AdminLdapConfigUpdateRequest,
) -> Result<aether_data::repository::auth_modules::StoredLdapModuleConfig, String> {
    let server_url = admin_ldap_trim_required(payload.server_url, "LDAP 服务器地址不能为空")?;
    let bind_dn = admin_ldap_trim_required(payload.bind_dn, "绑定 DN 不能为空")?;
    let base_dn = admin_ldap_trim_required(payload.base_dn, "Base DN 不能为空")?;
    let user_search_filter =
        admin_ldap_trim_required(payload.user_search_filter, "搜索过滤器不能为空")?;
    admin_ldap_validate_search_filter(&user_search_filter)?;
    let username_attr = admin_ldap_trim_required(payload.username_attr, "用户名属性不能为空")?;
    let email_attr = admin_ldap_trim_required(payload.email_attr, "邮箱属性不能为空")?;
    let display_name_attr =
        admin_ldap_trim_required(payload.display_name_attr, "显示名称属性不能为空")?;
    if !(1..=60).contains(&payload.connect_timeout) {
        return Err("连接超时时间必须在 1 到 60 秒之间".to_string());
    }

    let existing = state
        .get_ldap_module_config()
        .await
        .map_err(|err| format!("{err:?}"))?;
    let bind_password_update_requested = payload
        .bind_password
        .as_ref()
        .is_some_and(|value| !value.is_empty());
    let bind_password = match payload.bind_password {
        Some(value) if value.is_empty() => Some(String::new()),
        Some(value) => Some(admin_ldap_trim_required(value, "绑定密码不能为空")?),
        None => None,
    };
    let is_new_config = existing.is_none();
    if is_new_config && bind_password.as_deref().unwrap_or("").is_empty() {
        return Err("首次配置 LDAP 时必须设置绑定密码".to_string());
    }

    let will_have_password = bind_password
        .as_ref()
        .map(|value| !value.is_empty())
        .unwrap_or_else(|| {
            existing
                .as_ref()
                .and_then(|config| config.bind_password_encrypted.as_deref())
                .map(str::trim)
                .is_some_and(|value: &str| !value.is_empty())
        });

    if payload.is_exclusive && !payload.is_enabled {
        return Err("仅允许 LDAP 登录 需要先启用 LDAP 认证".to_string());
    }
    if payload.is_enabled && !will_have_password {
        return Err("启用 LDAP 认证 需要先设置绑定密码".to_string());
    }
    if payload.is_exclusive && !will_have_password {
        return Err("仅允许 LDAP 登录 需要先设置绑定密码".to_string());
    }
    if payload.is_enabled && payload.is_exclusive {
        let local_admin_count = state
            .count_active_local_admin_users_with_valid_password()
            .await
            .map_err(|err| format!("{err:?}"))?;
        if local_admin_count < 1 {
            return Err(
                "启用 LDAP 独占模式前，必须至少保留 1 个有效的本地管理员账户（含有效密码）作为紧急恢复通道"
                    .to_string(),
            );
        }
    }

    let bind_password_encrypted = match bind_password {
        Some(value) if value.is_empty() => None,
        Some(value) => state.encrypt_catalog_secret_with_fallbacks(&value),
        None => existing.and_then(|config| config.bind_password_encrypted),
    };
    if bind_password_update_requested && bind_password_encrypted.is_none() {
        return Err("LDAP 绑定密码加密失败，请检查 Rust 数据加密配置".to_string());
    }

    Ok(
        aether_data::repository::auth_modules::StoredLdapModuleConfig {
            server_url,
            bind_dn,
            bind_password_encrypted,
            base_dn,
            user_search_filter: Some(user_search_filter),
            username_attr: Some(username_attr),
            email_attr: Some(email_attr),
            display_name_attr: Some(display_name_attr),
            is_enabled: payload.is_enabled,
            is_exclusive: payload.is_exclusive,
            use_starttls: payload.use_starttls,
            connect_timeout: Some(payload.connect_timeout),
        },
    )
}

pub(super) async fn build_admin_ldap_test_config(
    state: &AdminAppState<'_>,
    payload: AdminLdapConfigTestRequest,
) -> Result<Option<AdminLdapConnectionTestConfig>, String> {
    if let Some(value) = payload.user_search_filter.as_deref() {
        admin_ldap_validate_search_filter(value.trim())?;
    }
    if let Some(connect_timeout) = payload.connect_timeout {
        if !(1..=60).contains(&connect_timeout) {
            return Err("连接超时时间必须在 1 到 60 秒之间".to_string());
        }
    }

    let saved = state
        .get_ldap_module_config()
        .await
        .map_err(|err| format!("{err:?}"))?;
    let mut server_url = saved
        .as_ref()
        .map(|config| config.server_url.trim().to_string())
        .filter(|value: &String| !value.is_empty());
    let mut bind_dn = saved
        .as_ref()
        .map(|config| config.bind_dn.trim().to_string())
        .filter(|value: &String| !value.is_empty());
    let mut base_dn = saved
        .as_ref()
        .map(|config| config.base_dn.trim().to_string())
        .filter(|value: &String| !value.is_empty());
    let mut use_starttls = saved
        .as_ref()
        .map(|config| config.use_starttls)
        .unwrap_or(false);
    let mut connect_timeout = saved
        .as_ref()
        .and_then(|config| config.connect_timeout)
        .unwrap_or_else(admin_ldap_default_connect_timeout);
    let mut bind_password = saved
        .as_ref()
        .and_then(|config| admin_ldap_read_saved_bind_password(state, config));

    if let Some(value) = payload.server_url {
        server_url = Some(admin_ldap_trim_required(value, "LDAP 服务器地址不能为空")?);
    }
    if let Some(value) = payload.bind_dn {
        bind_dn = Some(admin_ldap_trim_required(value, "绑定 DN 不能为空")?);
    }
    if let Some(value) = payload.base_dn {
        base_dn = Some(admin_ldap_trim_required(value, "Base DN 不能为空")?);
    }
    if let Some(value) = payload.bind_password {
        bind_password = Some(admin_ldap_trim_required(value, "绑定密码不能为空")?);
    }
    if let Some(value) = payload.use_starttls {
        use_starttls = value;
    }
    if let Some(value) = payload.connect_timeout {
        connect_timeout = value;
    }

    let mut missing = Vec::new();
    if server_url.is_none() {
        missing.push("server_url");
    }
    if bind_dn.is_none() {
        missing.push("bind_dn");
    }
    if base_dn.is_none() {
        missing.push("base_dn");
    }
    if bind_password.is_none() {
        missing.push("bind_password");
    }
    if !missing.is_empty() {
        return Ok(None);
    }

    Ok(Some(AdminLdapConnectionTestConfig {
        server_url: server_url.expect("server_url already checked"),
        bind_dn: bind_dn.expect("bind_dn already checked"),
        bind_password: bind_password.expect("bind_password already checked"),
        base_dn: base_dn.expect("base_dn already checked"),
        use_starttls,
        connect_timeout,
    }))
}

pub(super) async fn admin_ldap_test_connection(
    config: AdminLdapConnectionTestConfig,
) -> Result<(bool, String), GatewayError> {
    #[cfg(test)]
    if config.server_url.starts_with("mockldap://") {
        return Ok((
            config.bind_password == "secret123",
            if config.bind_password == "secret123" {
                "连接成功".to_string()
            } else {
                ADMIN_LDAP_TEST_FAILURE_MESSAGE.to_string()
            },
        ));
    }

    tokio::task::spawn_blocking(move || admin_ldap_test_connection_blocking(config))
        .await
        .map_err(|err| GatewayError::Internal(err.to_string()))
}

fn admin_ldap_test_connection_blocking(config: AdminLdapConnectionTestConfig) -> (bool, String) {
    let Some(server_url): Option<String> = admin_ldap_normalize_server_url(&config.server_url)
    else {
        return (false, ADMIN_LDAP_TEST_FAILURE_MESSAGE.to_string());
    };
    let timeout_secs = u64::try_from(config.connect_timeout.max(1)).unwrap_or(10);
    let settings = ldap3::LdapConnSettings::new()
        .set_conn_timeout(std::time::Duration::from_secs(timeout_secs))
        .set_starttls(config.use_starttls && !server_url.starts_with("ldaps://"));
    let Ok(mut conn) = ldap3::LdapConn::with_settings(settings, &server_url) else {
        return (false, ADMIN_LDAP_TEST_FAILURE_MESSAGE.to_string());
    };

    let bind_result = conn
        .simple_bind(&config.bind_dn, &config.bind_password)
        .and_then(|response| response.success());
    let _ = conn.unbind();
    if bind_result.is_ok() {
        (true, "连接成功".to_string())
    } else {
        (false, ADMIN_LDAP_TEST_FAILURE_MESSAGE.to_string())
    }
}

fn admin_ldap_trim_required(value: String, detail: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(detail.to_string());
    }
    Ok(trimmed.to_string())
}

fn admin_ldap_validate_search_filter(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("搜索过滤器不能为空".to_string());
    }
    if !value.contains("{username}") {
        return Err("搜索过滤器必须包含 {username} 占位符".to_string());
    }

    let mut depth = 0i32;
    let mut max_depth = 0i32;
    for ch in value.chars() {
        if ch == '(' {
            depth += 1;
            max_depth = max_depth.max(depth);
        } else if ch == ')' {
            depth -= 1;
            if depth < 0 {
                return Err("搜索过滤器括号不匹配".to_string());
            }
        }
    }
    if depth != 0 {
        return Err("搜索过滤器括号不匹配".to_string());
    }
    if max_depth > 5 {
        return Err("搜索过滤器嵌套层数过深（最多5层）".to_string());
    }
    if value.len() > 200 {
        return Err("搜索过滤器过长（最多200字符）".to_string());
    }
    Ok(())
}

fn admin_ldap_read_saved_bind_password(
    state: &AdminAppState<'_>,
    config: &aether_data::repository::auth_modules::StoredLdapModuleConfig,
) -> Option<String> {
    config
        .bind_password_encrypted
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| {
            state
                .decrypt_catalog_secret_with_fallbacks(value)
                .or_else(|| Some(value.to_string()))
        })
        .filter(|value| !value.trim().is_empty())
}
