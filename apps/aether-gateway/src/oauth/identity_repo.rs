use crate::handlers::shared::{
    decrypt_catalog_secret_with_fallbacks, module_available_from_env,
    system_config_bool as system_config_bool_with_default,
};
use crate::{AppState, GatewayError};
use aether_data::repository::oauth_providers::StoredOAuthProviderConfig;
use aether_data::repository::users::{StoredUserAuthRecord, StoredUserOAuthLinkSummary};
use aether_oauth::identity::{IdentityClaims, IdentityOAuthProviderConfig};
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

const LINUXDO_AUTHORIZE_URL: &str = "https://connect.linux.do/oauth2/authorize";
const LINUXDO_TOKEN_URL: &str = "https://connect.linux.do/oauth2/token";
const LINUXDO_USERINFO_URL: &str = "https://connect.linux.do/api/user";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct IdentityOAuthProviderSummary {
    pub(crate) provider_type: String,
    pub(crate) display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) icon_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct IdentityOAuthLinkSummary {
    pub(crate) provider_type: String,
    pub(crate) display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) icon_url: Option<String>,
    pub(crate) provider_username: Option<String>,
    pub(crate) provider_email: Option<String>,
    pub(crate) linked_at: Option<String>,
    pub(crate) last_login_at: Option<String>,
    pub(crate) provider_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentityOAuthAccountError {
    ProviderUnavailable,
    RegistrationDisabled,
    EmailExistsLocal,
    EmailIsLdap,
    EmailIsOauth,
    OAuthAlreadyBound,
    AlreadyBoundProvider,
    LastOAuthBinding,
    LastLoginMethod,
    Storage(String),
}

impl IdentityOAuthAccountError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::ProviderUnavailable | Self::Storage(_) => "provider_unavailable",
            Self::RegistrationDisabled => "registration_disabled",
            Self::EmailExistsLocal => "email_exists_local",
            Self::EmailIsLdap => "email_is_ldap",
            Self::EmailIsOauth => "email_is_oauth",
            Self::OAuthAlreadyBound => "oauth_already_bound",
            Self::AlreadyBoundProvider => "already_bound_provider",
            Self::LastOAuthBinding => "last_oauth_binding",
            Self::LastLoginMethod => "last_login_method",
        }
    }

    pub(crate) fn detail(&self) -> String {
        match self {
            Self::Storage(message) => message.clone(),
            _ => self.code().to_string(),
        }
    }
}

pub(crate) async fn list_enabled_identity_oauth_providers(
    state: &AppState,
) -> Result<Vec<IdentityOAuthProviderSummary>, GatewayError> {
    if !identity_oauth_module_enabled(state).await? {
        return Ok(Vec::new());
    }

    let mut providers = state
        .list_oauth_provider_configs()
        .await?
        .into_iter()
        .filter(|provider| provider.is_enabled)
        .map(|provider| IdentityOAuthProviderSummary {
            provider_type: provider.provider_type,
            display_name: provider.display_name,
            icon_url: provider.icon_url,
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.provider_type.cmp(&right.provider_type));
    Ok(providers)
}

pub(crate) async fn get_enabled_identity_oauth_provider_config(
    state: &AppState,
    provider_type: &str,
) -> Result<Option<IdentityOAuthProviderConfig>, IdentityOAuthAccountError> {
    if !identity_oauth_module_enabled(state)
        .await
        .map_err(|err| IdentityOAuthAccountError::Storage(format!("{err:?}")))?
    {
        return Ok(None);
    }

    let provider_type = provider_type.trim().to_ascii_lowercase();
    let config = state
        .get_oauth_provider_config(&provider_type)
        .await
        .map_err(|err| IdentityOAuthAccountError::Storage(format!("{err:?}")))?;
    let Some(config) = config.filter(|config| config.is_enabled) else {
        return Ok(None);
    };
    stored_provider_config_to_identity_config(state, config).map(Some)
}

async fn identity_oauth_module_enabled(state: &AppState) -> Result<bool, GatewayError> {
    if !module_available_from_env("OAUTH_AVAILABLE", true) {
        return Ok(false);
    }

    let enabled = state
        .read_system_config_json_value("module.oauth.enabled")
        .await?;
    Ok(system_config_bool_with_default(enabled.as_ref(), false))
}

pub(crate) async fn list_identity_oauth_links(
    state: &AppState,
    user_id: &str,
) -> Result<Vec<IdentityOAuthLinkSummary>, GatewayError> {
    state
        .data
        .list_user_oauth_links(user_id)
        .await
        .map_err(data_gateway_error)?
        .into_iter()
        .map(map_link_summary)
        .collect()
}

pub(crate) async fn list_bindable_identity_oauth_providers(
    state: &AppState,
    user_id: &str,
) -> Result<Vec<IdentityOAuthProviderSummary>, GatewayError> {
    let linked = list_identity_oauth_links(state, user_id)
        .await?
        .into_iter()
        .map(|link| link.provider_type)
        .collect::<std::collections::BTreeSet<_>>();
    let providers = list_enabled_identity_oauth_providers(state)
        .await?
        .into_iter()
        .filter(|provider| !linked.contains(&provider.provider_type))
        .collect();
    Ok(providers)
}

pub(crate) async fn resolve_identity_oauth_login_user(
    state: &AppState,
    claims: &IdentityClaims,
) -> Result<StoredUserAuthRecord, IdentityOAuthAccountError> {
    let now = Utc::now();
    if let Some(user) = state
        .data
        .find_oauth_linked_user(&claims.provider_type, &claims.subject)
        .await
        .map_err(repo_data_error)?
    {
        state
            .data
            .touch_oauth_link(
                &claims.provider_type,
                &claims.subject,
                claims.username.as_deref(),
                claims.email.as_deref(),
                Some(claims.raw.clone()),
                now,
            )
            .await
            .map_err(repo_data_error)?;
        return Ok(user);
    }

    let email = normalize_identity_email(claims.email.as_deref());
    if let Some(email) = email.as_deref() {
        if let Some(existing) = state
            .data
            .find_active_user_auth_by_email_ci(email)
            .await
            .map_err(repo_data_error)?
        {
            return Err(match existing.auth_source.to_ascii_lowercase().as_str() {
                "local" => IdentityOAuthAccountError::EmailExistsLocal,
                "ldap" => IdentityOAuthAccountError::EmailIsLdap,
                _ => IdentityOAuthAccountError::EmailIsOauth,
            });
        }
    }

    let registration_enabled = state
        .read_system_config_json_value("enable_registration")
        .await
        .map_err(|err| IdentityOAuthAccountError::Storage(format!("{err:?}")))?
        .as_ref()
        .map(system_config_bool)
        .unwrap_or(false);
    if !registration_enabled {
        return Err(IdentityOAuthAccountError::RegistrationDisabled);
    }

    let initial_gift = state
        .read_system_config_json_value("default_user_initial_gift_usd")
        .await
        .map_err(|err| IdentityOAuthAccountError::Storage(format!("{err:?}")))?
        .as_ref()
        .map(|value| system_config_f64(value, 10.0))
        .unwrap_or(10.0);

    let username = unique_oauth_username(state, claims).await?;
    let user = state
        .data
        .create_oauth_auth_user(email, username, now)
        .await
        .map_err(repo_data_error)?
        .ok_or_else(|| IdentityOAuthAccountError::Storage("oauth user not created".to_string()))?;
    match state
        .initialize_auth_user_wallet(&user.id, initial_gift, false)
        .await
    {
        Ok(Some(_wallet)) => {}
        Ok(None) => {
            let _ = state.delete_local_auth_user(&user.id).await;
            return Err(IdentityOAuthAccountError::ProviderUnavailable);
        }
        Err(err) => {
            let _ = state.delete_local_auth_user(&user.id).await;
            return Err(IdentityOAuthAccountError::Storage(format!("{err:?}")));
        }
    }
    if let Err(err) = state
        .assign_default_group_to_self_registered_user(&user.id)
        .await
    {
        let _ = state.delete_local_auth_user(&user.id).await;
        return Err(IdentityOAuthAccountError::Storage(format!("{err:?}")));
    }
    if let Err(err) = upsert_oauth_link(state, &user.id, claims, now).await {
        let _ = state.delete_local_auth_user(&user.id).await;
        return Err(err);
    }
    Ok(user)
}

pub(crate) async fn bind_identity_oauth_to_user(
    state: &AppState,
    user: &StoredUserAuthRecord,
    claims: &IdentityClaims,
) -> Result<(), IdentityOAuthAccountError> {
    if user.auth_source.eq_ignore_ascii_case("ldap") {
        return Err(IdentityOAuthAccountError::EmailIsLdap);
    }
    if let Some(owner) = state
        .data
        .find_oauth_link_owner(&claims.provider_type, &claims.subject)
        .await
        .map_err(repo_data_error)?
    {
        if owner != user.id {
            return Err(IdentityOAuthAccountError::OAuthAlreadyBound);
        }
    }
    if state
        .data
        .has_user_oauth_provider_link(&user.id, &claims.provider_type)
        .await
        .map_err(repo_data_error)?
    {
        return Err(IdentityOAuthAccountError::AlreadyBoundProvider);
    }
    upsert_oauth_link(state, &user.id, claims, Utc::now()).await?;
    Ok(())
}

pub(crate) async fn unbind_identity_oauth(
    state: &AppState,
    user: &StoredUserAuthRecord,
    provider_type: &str,
) -> Result<bool, IdentityOAuthAccountError> {
    if user.auth_source.eq_ignore_ascii_case("ldap") {
        return Err(IdentityOAuthAccountError::EmailIsLdap);
    }
    let link_count = state
        .data
        .count_user_oauth_links(&user.id)
        .await
        .map_err(repo_data_error)?;
    if user.auth_source.eq_ignore_ascii_case("oauth") && link_count <= 1 {
        return Err(IdentityOAuthAccountError::LastOAuthBinding);
    }
    if !user.auth_source.eq_ignore_ascii_case("local") && link_count <= 1 {
        return Err(IdentityOAuthAccountError::LastLoginMethod);
    }
    state
        .data
        .delete_user_oauth_link(&user.id, provider_type.trim())
        .await
        .map_err(repo_data_error)
}

fn stored_provider_config_to_identity_config(
    state: &AppState,
    config: StoredOAuthProviderConfig,
) -> Result<IdentityOAuthProviderConfig, IdentityOAuthAccountError> {
    let defaults = identity_provider_defaults(&config.provider_type);
    let authorization_url = config
        .authorization_url_override
        .clone()
        .or_else(|| defaults.map(|defaults| defaults.0.to_string()))
        .filter(|value| !value.trim().is_empty())
        .ok_or(IdentityOAuthAccountError::ProviderUnavailable)?;
    let token_url = config
        .token_url_override
        .clone()
        .or_else(|| defaults.map(|defaults| defaults.1.to_string()))
        .filter(|value| !value.trim().is_empty())
        .ok_or(IdentityOAuthAccountError::ProviderUnavailable)?;
    let userinfo_url = config
        .userinfo_url_override
        .clone()
        .or_else(|| defaults.map(|defaults| defaults.2.to_string()));
    let client_secret = match config.client_secret_encrypted.as_deref() {
        Some(ciphertext) => Some(
            decrypt_catalog_secret_with_fallbacks(state.encryption_key(), ciphertext)
                .ok_or(IdentityOAuthAccountError::ProviderUnavailable)?,
        ),
        None => None,
    };

    Ok(IdentityOAuthProviderConfig {
        provider_type: config.provider_type,
        display_name: config.display_name,
        authorization_url,
        token_url,
        userinfo_url,
        client_id: config.client_id,
        client_secret,
        scopes: config.scopes.unwrap_or_default(),
        redirect_uri: config.redirect_uri,
        frontend_callback_url: config.frontend_callback_url,
        attribute_mapping: config.attribute_mapping,
        extra_config: config.extra_config,
    })
}

fn identity_provider_defaults(
    provider_type: &str,
) -> Option<(&'static str, &'static str, &'static str)> {
    match provider_type.trim().to_ascii_lowercase().as_str() {
        "linuxdo" => Some((
            LINUXDO_AUTHORIZE_URL,
            LINUXDO_TOKEN_URL,
            LINUXDO_USERINFO_URL,
        )),
        _ => None,
    }
}

fn map_link_summary(
    row: StoredUserOAuthLinkSummary,
) -> Result<IdentityOAuthLinkSummary, GatewayError> {
    Ok(IdentityOAuthLinkSummary {
        provider_type: row.provider_type,
        display_name: row.display_name,
        icon_url: row.icon_url,
        provider_username: row.provider_username,
        provider_email: row.provider_email,
        linked_at: row.linked_at.map(|value| value.to_rfc3339()),
        last_login_at: row.last_login_at.map(|value| value.to_rfc3339()),
        provider_enabled: row.provider_enabled,
    })
}

async fn unique_oauth_username(
    state: &AppState,
    claims: &IdentityClaims,
) -> Result<String, IdentityOAuthAccountError> {
    let base = normalize_oauth_username(
        claims
            .username
            .as_deref()
            .or(claims.display_name.as_deref())
            .or_else(|| {
                claims
                    .email
                    .as_deref()
                    .and_then(|email| email.split('@').next())
            })
            .unwrap_or("oauth_user"),
    );
    for attempt in 0..8 {
        let candidate = if attempt == 0 {
            base.clone()
        } else {
            format!(
                "{}_{}",
                base.chars().take(20).collect::<String>(),
                short_uuid()
            )
        };
        let taken = state
            .data
            .find_user_auth_by_username(&candidate)
            .await
            .map_err(repo_data_error)?
            .is_some();
        if !taken {
            return Ok(candidate);
        }
    }
    Ok(format!("oauth_{}", short_uuid()))
}

async fn upsert_oauth_link(
    state: &AppState,
    user_id: &str,
    claims: &IdentityClaims,
    now: chrono::DateTime<Utc>,
) -> Result<(), IdentityOAuthAccountError> {
    state
        .data
        .upsert_user_oauth_link(
            user_id,
            &claims.provider_type,
            &claims.subject,
            claims.username.as_deref(),
            claims.email.as_deref(),
            Some(claims.raw.clone()),
            now,
        )
        .await
        .map_err(repo_data_error)
}

fn normalize_identity_email(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn normalize_oauth_username(value: &str) -> String {
    let mut normalized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    while normalized.contains("__") {
        normalized = normalized.replace("__", "_");
    }
    normalized = normalized
        .trim_matches(|ch| matches!(ch, '_' | '-' | '.'))
        .chars()
        .take(30)
        .collect();
    if normalized.len() < 3 || is_reserved_username(&normalized) {
        normalized = format!("oauth_{}", short_uuid());
    }
    normalized
}

fn is_reserved_username(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "admin" | "root" | "system" | "api" | "test" | "demo" | "user" | "guest" | "bot"
    )
}

fn short_uuid() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

fn system_config_bool(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64().is_some_and(|value| value != 0),
        Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        _ => false,
    }
}

fn system_config_f64(value: &Value, default: f64) -> f64 {
    match value {
        Value::Number(value) => value.as_f64().unwrap_or(default),
        Value::String(value) => value.trim().parse::<f64>().unwrap_or(default),
        _ => default,
    }
}

fn repo_data_error(error: aether_data::DataLayerError) -> IdentityOAuthAccountError {
    IdentityOAuthAccountError::Storage(error.to_string())
}

fn data_gateway_error(error: aether_data::DataLayerError) -> GatewayError {
    GatewayError::Internal(error.to_string())
}
