use crate::handlers::shared::decrypt_catalog_secret_with_fallbacks;
use crate::{AppState, GatewayError};
use aether_data::repository::oauth_providers::StoredOAuthProviderConfig;
use aether_data::repository::users::StoredUserAuthRecord;
use aether_oauth::identity::{IdentityClaims, IdentityOAuthProviderConfig};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

const LINUXDO_AUTHORIZE_URL: &str = "https://connect.linux.do/oauth2/authorize";
const LINUXDO_TOKEN_URL: &str = "https://connect.linux.do/oauth2/token";
const LINUXDO_USERINFO_URL: &str = "https://connect.linux.do/api/user";

const FIND_OAUTH_LINKED_USER_SQL: &str = r#"
SELECT
    users.id,
    users.email,
    users.email_verified,
    users.username,
    users.password_hash,
    users.role::text AS role,
    users.auth_source::text AS auth_source,
    users.allowed_providers,
    users.allowed_api_formats,
    users.allowed_models,
    users.is_active,
    users.is_deleted,
    users.created_at,
    users.last_login_at
FROM user_oauth_links
JOIN users ON users.id = user_oauth_links.user_id
WHERE user_oauth_links.provider_type = $1
  AND user_oauth_links.provider_user_id = $2
LIMIT 1
"#;

const FIND_USER_BY_EMAIL_SQL: &str = r#"
SELECT
    id,
    email,
    email_verified,
    username,
    password_hash,
    role::text AS role,
    auth_source::text AS auth_source,
    allowed_providers,
    allowed_api_formats,
    allowed_models,
    is_active,
    is_deleted,
    created_at,
    last_login_at
FROM users
WHERE LOWER(email) = LOWER($1)
  AND is_deleted IS FALSE
LIMIT 1
"#;

const CHECK_USERNAME_TAKEN_SQL: &str = r#"
SELECT id
FROM users
WHERE username = $1
LIMIT 1
"#;

const CREATE_OAUTH_USER_SQL: &str = r#"
INSERT INTO users (
    id,
    email,
    email_verified,
    username,
    password_hash,
    role,
    auth_source,
    is_active,
    is_deleted,
    created_at,
    updated_at,
    last_login_at
)
VALUES (
    $1,
    $2,
    TRUE,
    $3,
    NULL,
    'user'::userrole,
    'oauth'::authsource,
    TRUE,
    FALSE,
    $4,
    $4,
    $4
)
RETURNING
    id,
    email,
    email_verified,
    username,
    password_hash,
    role::text AS role,
    auth_source::text AS auth_source,
    allowed_providers,
    allowed_api_formats,
    allowed_models,
    is_active,
    is_deleted,
    created_at,
    last_login_at
"#;

const UPSERT_OAUTH_LINK_SQL: &str = r#"
INSERT INTO user_oauth_links (
    id,
    user_id,
    provider_type,
    provider_user_id,
    provider_username,
    provider_email,
    extra_data,
    linked_at,
    last_login_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8)
ON CONFLICT (user_id, provider_type) DO UPDATE
SET provider_user_id = EXCLUDED.provider_user_id,
    provider_username = EXCLUDED.provider_username,
    provider_email = EXCLUDED.provider_email,
    extra_data = EXCLUDED.extra_data,
    last_login_at = EXCLUDED.last_login_at
"#;

const TOUCH_OAUTH_LINK_SQL: &str = r#"
UPDATE user_oauth_links
SET provider_username = COALESCE($3, provider_username),
    provider_email = COALESCE($4, provider_email),
    extra_data = COALESCE($5, extra_data),
    last_login_at = $6
WHERE provider_type = $1
  AND provider_user_id = $2
"#;

const CREATE_AUTH_USER_WALLET_SQL: &str = r#"
INSERT INTO wallets (
    id,
    user_id,
    api_key_id,
    balance,
    gift_balance,
    limit_mode,
    currency,
    status,
    total_recharged,
    total_consumed,
    total_refunded,
    total_adjusted,
    created_at,
    updated_at
)
VALUES (
    $1,
    $2,
    NULL,
    0,
    $3,
    $4,
    'USD',
    'active',
    0,
    0,
    0,
    $3,
    NOW(),
    NOW()
)
"#;

const CREATE_AUTH_USER_WALLET_GIFT_TX_SQL: &str = r#"
INSERT INTO wallet_transactions (
    id,
    wallet_id,
    category,
    reason_code,
    amount,
    balance_before,
    balance_after,
    recharge_balance_before,
    recharge_balance_after,
    gift_balance_before,
    gift_balance_after,
    link_type,
    link_id,
    operator_id,
    description,
    created_at
)
VALUES (
    $1,
    $2,
    'gift',
    'gift_initial',
    $3,
    0,
    $3,
    0,
    0,
    0,
    $3,
    'system_task',
    $4,
    NULL,
    '用户初始赠款',
    NOW()
)
"#;

const LIST_OAUTH_LINKS_SQL: &str = r#"
SELECT
    user_oauth_links.provider_type,
    oauth_providers.display_name,
    user_oauth_links.provider_username,
    user_oauth_links.provider_email,
    user_oauth_links.linked_at,
    user_oauth_links.last_login_at,
    oauth_providers.is_enabled AS provider_enabled
FROM user_oauth_links
JOIN oauth_providers
  ON oauth_providers.provider_type = user_oauth_links.provider_type
WHERE user_oauth_links.user_id = $1
ORDER BY user_oauth_links.linked_at ASC
"#;

const FIND_OAUTH_LINK_OWNER_SQL: &str = r#"
SELECT user_id
FROM user_oauth_links
WHERE provider_type = $1
  AND provider_user_id = $2
LIMIT 1
"#;

const FIND_USER_PROVIDER_LINK_OWNER_SQL: &str = r#"
SELECT user_id
FROM user_oauth_links
WHERE user_id = $1
  AND provider_type = $2
LIMIT 1
"#;

const COUNT_USER_OAUTH_LINKS_SQL: &str = r#"
SELECT COUNT(*)::bigint AS link_count
FROM user_oauth_links
WHERE user_id = $1
"#;

const DELETE_USER_OAUTH_LINK_SQL: &str = r#"
DELETE FROM user_oauth_links
WHERE user_id = $1
  AND provider_type = $2
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct IdentityOAuthProviderSummary {
    pub(crate) provider_type: String,
    pub(crate) display_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct IdentityOAuthLinkSummary {
    pub(crate) provider_type: String,
    pub(crate) display_name: String,
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
    let mut providers = state
        .list_oauth_provider_configs()
        .await?
        .into_iter()
        .filter(|provider| provider.is_enabled)
        .map(|provider| IdentityOAuthProviderSummary {
            provider_type: provider.provider_type,
            display_name: provider.display_name,
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.provider_type.cmp(&right.provider_type));
    Ok(providers)
}

pub(crate) async fn get_enabled_identity_oauth_provider_config(
    state: &AppState,
    provider_type: &str,
) -> Result<Option<IdentityOAuthProviderConfig>, IdentityOAuthAccountError> {
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

pub(crate) async fn list_identity_oauth_links(
    state: &AppState,
    user_id: &str,
) -> Result<Vec<IdentityOAuthLinkSummary>, GatewayError> {
    let Some(pool) = state.postgres_pool() else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query(LIST_OAUTH_LINKS_SQL)
        .bind(user_id)
        .fetch_all(&pool)
        .await
        .map_err(sql_gateway_error)?;
    rows.iter().map(map_link_summary_row).collect()
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
    let Some(pool) = state.postgres_pool() else {
        return Err(IdentityOAuthAccountError::ProviderUnavailable);
    };
    let now = Utc::now();
    if let Some(row) = sqlx::query(FIND_OAUTH_LINKED_USER_SQL)
        .bind(&claims.provider_type)
        .bind(&claims.subject)
        .fetch_optional(&pool)
        .await
        .map_err(repo_sql_error)?
    {
        sqlx::query(TOUCH_OAUTH_LINK_SQL)
            .bind(&claims.provider_type)
            .bind(&claims.subject)
            .bind(claims.username.as_deref())
            .bind(claims.email.as_deref())
            .bind(Some(claims.raw.clone()))
            .bind(now)
            .execute(&pool)
            .await
            .map_err(repo_sql_error)?;
        return map_user_auth_row(&row).map_err(repo_data_error);
    }

    let email = normalize_identity_email(claims.email.as_deref());
    if let Some(email) = email.as_deref() {
        if let Some(row) = sqlx::query(FIND_USER_BY_EMAIL_SQL)
            .bind(email)
            .fetch_optional(&pool)
            .await
            .map_err(repo_sql_error)?
        {
            let existing = map_user_auth_row(&row).map_err(repo_data_error)?;
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

    let mut tx = pool.begin().await.map_err(repo_sql_error)?;
    let username = unique_oauth_username(&mut tx, claims).await?;
    let user_id = Uuid::new_v4().to_string();
    let row = sqlx::query(CREATE_OAUTH_USER_SQL)
        .bind(&user_id)
        .bind(email.as_deref())
        .bind(&username)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(repo_sql_error)?;
    let user = map_user_auth_row(&row).map_err(repo_data_error)?;

    create_initial_wallet_in_tx(&mut tx, &user.id, initial_gift).await?;
    upsert_oauth_link_in_tx(&mut tx, &user.id, claims, now).await?;
    tx.commit().await.map_err(repo_sql_error)?;
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
    let Some(pool) = state.postgres_pool() else {
        return Err(IdentityOAuthAccountError::ProviderUnavailable);
    };
    if let Some(row) = sqlx::query(FIND_OAUTH_LINK_OWNER_SQL)
        .bind(&claims.provider_type)
        .bind(&claims.subject)
        .fetch_optional(&pool)
        .await
        .map_err(repo_sql_error)?
    {
        let owner: String = row.try_get("user_id").map_err(repo_sql_error)?;
        if owner != user.id {
            return Err(IdentityOAuthAccountError::OAuthAlreadyBound);
        }
    }
    if sqlx::query(FIND_USER_PROVIDER_LINK_OWNER_SQL)
        .bind(&user.id)
        .bind(&claims.provider_type)
        .fetch_optional(&pool)
        .await
        .map_err(repo_sql_error)?
        .is_some()
    {
        return Err(IdentityOAuthAccountError::AlreadyBoundProvider);
    }
    let mut tx = pool.begin().await.map_err(repo_sql_error)?;
    upsert_oauth_link_in_tx(&mut tx, &user.id, claims, Utc::now()).await?;
    tx.commit().await.map_err(repo_sql_error)?;
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
    let Some(pool) = state.postgres_pool() else {
        return Err(IdentityOAuthAccountError::ProviderUnavailable);
    };
    let row = sqlx::query(COUNT_USER_OAUTH_LINKS_SQL)
        .bind(&user.id)
        .fetch_one(&pool)
        .await
        .map_err(repo_sql_error)?;
    let link_count: i64 = row.try_get("link_count").map_err(repo_sql_error)?;
    if user.auth_source.eq_ignore_ascii_case("oauth") && link_count <= 1 {
        return Err(IdentityOAuthAccountError::LastOAuthBinding);
    }
    if !user.auth_source.eq_ignore_ascii_case("local") && link_count <= 1 {
        return Err(IdentityOAuthAccountError::LastLoginMethod);
    }
    let result = sqlx::query(DELETE_USER_OAUTH_LINK_SQL)
        .bind(&user.id)
        .bind(provider_type.trim())
        .execute(&pool)
        .await
        .map_err(repo_sql_error)?;
    Ok(result.rows_affected() > 0)
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

fn map_link_summary_row(
    row: &sqlx::postgres::PgRow,
) -> Result<IdentityOAuthLinkSummary, GatewayError> {
    Ok(IdentityOAuthLinkSummary {
        provider_type: row.try_get("provider_type").map_err(sql_gateway_error)?,
        display_name: row.try_get("display_name").map_err(sql_gateway_error)?,
        provider_username: row
            .try_get("provider_username")
            .map_err(sql_gateway_error)?,
        provider_email: row.try_get("provider_email").map_err(sql_gateway_error)?,
        linked_at: row
            .try_get::<Option<DateTime<Utc>>, _>("linked_at")
            .map_err(sql_gateway_error)?
            .map(|value| value.to_rfc3339()),
        last_login_at: row
            .try_get::<Option<DateTime<Utc>>, _>("last_login_at")
            .map_err(sql_gateway_error)?
            .map(|value| value.to_rfc3339()),
        provider_enabled: row.try_get("provider_enabled").map_err(sql_gateway_error)?,
    })
}

fn map_user_auth_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUserAuthRecord, aether_data::DataLayerError> {
    StoredUserAuthRecord::new(
        row.try_get("id").map_err(data_unexpected)?,
        row.try_get("email").map_err(data_unexpected)?,
        row.try_get("email_verified").map_err(data_unexpected)?,
        row.try_get("username").map_err(data_unexpected)?,
        row.try_get("password_hash").map_err(data_unexpected)?,
        row.try_get("role").map_err(data_unexpected)?,
        row.try_get("auth_source").map_err(data_unexpected)?,
        row.try_get("allowed_providers").map_err(data_unexpected)?,
        row.try_get("allowed_api_formats")
            .map_err(data_unexpected)?,
        row.try_get("allowed_models").map_err(data_unexpected)?,
        row.try_get("is_active").map_err(data_unexpected)?,
        row.try_get("is_deleted").map_err(data_unexpected)?,
        row.try_get("created_at").map_err(data_unexpected)?,
        row.try_get("last_login_at").map_err(data_unexpected)?,
    )
}

async fn unique_oauth_username(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
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
        let taken = sqlx::query(CHECK_USERNAME_TAKEN_SQL)
            .bind(&candidate)
            .fetch_optional(&mut **tx)
            .await
            .map_err(repo_sql_error)?
            .is_some();
        if !taken {
            return Ok(candidate);
        }
    }
    Ok(format!("oauth_{}", short_uuid()))
}

async fn upsert_oauth_link_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: &str,
    claims: &IdentityClaims,
    now: DateTime<Utc>,
) -> Result<(), IdentityOAuthAccountError> {
    sqlx::query(UPSERT_OAUTH_LINK_SQL)
        .bind(Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(&claims.provider_type)
        .bind(&claims.subject)
        .bind(claims.username.as_deref())
        .bind(claims.email.as_deref())
        .bind(Some(claims.raw.clone()))
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(repo_sql_error)?;
    Ok(())
}

async fn create_initial_wallet_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: &str,
    initial_gift_usd: f64,
) -> Result<(), IdentityOAuthAccountError> {
    let gift_amount = initial_gift_usd.max(0.0);
    let wallet_id = Uuid::new_v4().to_string();
    sqlx::query(CREATE_AUTH_USER_WALLET_SQL)
        .bind(&wallet_id)
        .bind(user_id)
        .bind(gift_amount)
        .bind("finite")
        .execute(&mut **tx)
        .await
        .map_err(repo_sql_error)?;
    if gift_amount > 0.0 {
        sqlx::query(CREATE_AUTH_USER_WALLET_GIFT_TX_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(&wallet_id)
            .bind(gift_amount)
            .bind(user_id)
            .execute(&mut **tx)
            .await
            .map_err(repo_sql_error)?;
    }
    Ok(())
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

fn repo_sql_error(error: sqlx::Error) -> IdentityOAuthAccountError {
    IdentityOAuthAccountError::Storage(error.to_string())
}

fn repo_data_error(error: aether_data::DataLayerError) -> IdentityOAuthAccountError {
    IdentityOAuthAccountError::Storage(error.to_string())
}

fn sql_gateway_error(error: sqlx::Error) -> GatewayError {
    GatewayError::Internal(error.to_string())
}

fn data_unexpected(error: sqlx::Error) -> aether_data::DataLayerError {
    aether_data::DataLayerError::UnexpectedValue(error.to_string())
}
