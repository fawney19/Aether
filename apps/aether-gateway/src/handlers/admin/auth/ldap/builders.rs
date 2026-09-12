use super::shared::*;
use crate::handlers::admin::request::AdminAppState;
use crate::GatewayError;
use aether_data::repository::auth_modules::{LdapBindPasswordUpdate, StoredLdapModuleConfig};
use serde::Deserialize;

#[derive(Deserialize)]
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

#[derive(Default, Deserialize)]
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

#[derive(Clone)]
pub(super) struct AdminLdapConnectionTestConfig {
    server_url: String,
    bind_dn: String,
    bind_password: String,
    base_dn: String,
    use_starttls: bool,
    connect_timeout: i32,
}

pub(super) struct AdminLdapConfigUpdate {
    pub(super) expected: Option<StoredLdapModuleConfig>,
    pub(super) replacement: StoredLdapModuleConfig,
    pub(super) bind_password_update: LdapBindPasswordUpdate,
}

pub(super) async fn build_admin_ldap_update_config(
    state: &AdminAppState<'_>,
    payload: AdminLdapConfigUpdateRequest,
) -> Result<AdminLdapConfigUpdate, String> {
    let server_url = admin_ldap_trim_required(payload.server_url, "LDAP 服务器地址不能为空")?;
    let server_url = admin_ldap_normalize_server_url(&server_url, payload.use_starttls)
        .ok_or_else(|| {
            "LDAP 服务器地址必须使用 ldaps://，或在启用 StartTLS 时使用 ldap://；不得包含凭据、查询参数或片段"
                .to_string()
        })?;
    let bind_dn = admin_ldap_trim_required(payload.bind_dn, "绑定 DN 不能为空")?;
    let base_dn = admin_ldap_trim_required(payload.base_dn, "Base DN 不能为空")?;
    admin_ldap_validate_distinguished_name(&bind_dn, "绑定 DN")?;
    admin_ldap_validate_distinguished_name(&base_dn, "Base DN")?;
    let user_search_filter =
        admin_ldap_trim_required(payload.user_search_filter, "搜索过滤器不能为空")?;
    admin_ldap_validate_search_filter(&user_search_filter)?;
    let username_attr = admin_ldap_trim_required(payload.username_attr, "用户名属性不能为空")?;
    let email_attr = admin_ldap_trim_required(payload.email_attr, "邮箱属性不能为空")?;
    let display_name_attr =
        admin_ldap_trim_required(payload.display_name_attr, "显示名称属性不能为空")?;
    admin_ldap_validate_attribute_description(&username_attr, "用户名属性")?;
    admin_ldap_validate_attribute_description(&email_attr, "邮箱属性")?;
    admin_ldap_validate_attribute_description(&display_name_attr, "显示名称属性")?;
    if !(1..=60).contains(&payload.connect_timeout) {
        return Err("连接超时时间必须在 1 到 60 秒之间".to_string());
    }

    let mut existing = state
        .get_ldap_module_config()
        .await
        .map_err(|err| format!("{err:?}"))?;
    if payload.bind_password.is_none() {
        if let Some(config) = existing.as_ref() {
            crate::handlers::shared::decrypt_or_migrate_ldap_bind_password(state.app(), config)
                .await
                .map_err(|_| "已保存的 LDAP 绑定密码无法解密".to_string())?;
            existing = state
                .get_ldap_module_config()
                .await
                .map_err(|err| format!("{err:?}"))?;
        }
    }
    let requested_bind_password = payload.bind_password;
    let is_new_config = existing.is_none();
    let will_have_password = match requested_bind_password.as_deref() {
        Some(value) => !value.trim().is_empty(),
        None => existing
            .as_ref()
            .and_then(|config| config.bind_password_encrypted.as_deref())
            .map(str::trim)
            .is_some_and(|value: &str| !value.is_empty()),
    };
    if is_new_config && !will_have_password {
        return Err("首次配置 LDAP 时必须设置绑定密码".to_string());
    }

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

    let replacement = StoredLdapModuleConfig {
        server_url,
        bind_dn,
        // The repository ignores this field for config mutations. Password changes are
        // carried exclusively by `bind_password_update`, so Preserve never copies an old
        // ciphertext into the replacement record.
        bind_password_encrypted: None,
        base_dn,
        user_search_filter: Some(user_search_filter),
        username_attr: Some(username_attr),
        email_attr: Some(email_attr),
        display_name_attr: Some(display_name_attr),
        is_enabled: payload.is_enabled,
        is_exclusive: payload.is_exclusive,
        use_starttls: payload.use_starttls,
        connect_timeout: Some(payload.connect_timeout),
    };
    let bind_password_update = match requested_bind_password {
        Some(value) if value.is_empty() => LdapBindPasswordUpdate::Clear,
        Some(value) => {
            let password = admin_ldap_trim_required(value, "绑定密码不能为空")?;
            let ciphertext = state
                .encrypt_ldap_bind_password(&replacement, &password)
                .ok_or_else(|| "LDAP 绑定密码加密失败，请检查 Rust 数据加密配置".to_string())?;
            LdapBindPasswordUpdate::Set(ciphertext)
        }
        None => {
            if let Some(existing_config) = existing.as_ref() {
                if existing_config
                    .bind_password_encrypted
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                    && !crate::handlers::shared::ldap_bind_password_binding_matches(
                        existing_config,
                        &replacement,
                    )
                    .unwrap_or(false)
                {
                    return Err(
                        "修改 LDAP 服务器、StartTLS、bind DN 或 Base DN 时必须重新提供绑定密码"
                            .to_string(),
                    );
                }
            }
            LdapBindPasswordUpdate::Preserve
        }
    };
    Ok(AdminLdapConfigUpdate {
        expected: existing,
        replacement,
        bind_password_update,
    })
}

pub(super) async fn build_admin_ldap_test_config(
    state: &AdminAppState<'_>,
    payload: AdminLdapConfigTestRequest,
) -> Result<Option<AdminLdapConnectionTestConfig>, String> {
    if let Some(value) = payload.user_search_filter.as_deref() {
        admin_ldap_validate_search_filter(value)?;
    }
    for (value, label) in [
        (payload.username_attr.as_deref(), "用户名属性"),
        (payload.email_attr.as_deref(), "邮箱属性"),
        (payload.display_name_attr.as_deref(), "显示名称属性"),
    ] {
        if let Some(value) = value {
            admin_ldap_validate_attribute_description(value.trim(), label)?;
        }
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
    let mut bind_password = match saved.as_ref() {
        Some(config) => {
            crate::handlers::shared::decrypt_or_migrate_ldap_bind_password(state.app(), config)
                .await
                .map_err(|_| "已保存的 LDAP 绑定密码无法解密".to_string())?
        }
        None => None,
    };

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

    let server_url = server_url.expect("server_url already checked");
    let server_url = admin_ldap_normalize_server_url(&server_url, use_starttls).ok_or_else(|| {
        "LDAP 服务器地址必须使用 ldaps://，或在启用 StartTLS 时使用 ldap://；不得包含凭据、查询参数或片段"
            .to_string()
    })?;
    let bind_dn = bind_dn.expect("bind_dn already checked");
    let base_dn = base_dn.expect("base_dn already checked");
    admin_ldap_validate_distinguished_name(&bind_dn, "绑定 DN")?;
    admin_ldap_validate_distinguished_name(&base_dn, "Base DN")?;

    Ok(Some(AdminLdapConnectionTestConfig {
        server_url,
        bind_dn,
        bind_password: bind_password.expect("bind_password already checked"),
        base_dn,
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
    let Some(server_url): Option<String> =
        admin_ldap_normalize_server_url(&config.server_url, config.use_starttls)
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
    crate::handlers::shared::ldap_search_filter_is_valid(value)
        .then_some(())
        .ok_or_else(|| {
            "搜索过滤器格式无效，必须包含 {username} 且使用唯一、有限的外层括号结构".to_string()
        })
}

fn admin_ldap_validate_distinguished_name(value: &str, label: &str) -> Result<(), String> {
    crate::handlers::shared::ldap_distinguished_name_is_valid(value)
        .then_some(())
        .ok_or_else(|| format!("{label}格式无效或过长"))
}

fn admin_ldap_validate_attribute_description(value: &str, label: &str) -> Result<(), String> {
    crate::handlers::shared::ldap_attribute_description_is_valid(value)
        .then_some(())
        .ok_or_else(|| format!("{label}必须是有效的 LDAP 属性名称"))
}
