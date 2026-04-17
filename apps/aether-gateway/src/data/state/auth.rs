use super::{
    AuthApiKeyLookupKey, CreateManagementTokenRecord, DataLayerError, GatewayAuthApiKeySnapshot,
    GatewayDataState, ManagementTokenListQuery, ProxyNodeHeartbeatMutation,
    ProxyNodeManualCreateMutation, ProxyNodeManualUpdateMutation, ProxyNodeRegistrationMutation,
    ProxyNodeRemoteConfigMutation, ProxyNodeTunnelStatusMutation, RegenerateManagementTokenSecret,
    StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot, StoredLdapModuleConfig,
    StoredManagementToken, StoredManagementTokenListPage, StoredManagementTokenWithUser,
    StoredOAuthProviderConfig, StoredOAuthProviderModuleConfig, StoredProxyNode,
    StoredProxyNodeEvent, StoredUserAuthRecord, StoredUserPreferenceRecord,
    StoredUserSessionRecord, StoredWalletSnapshot, UpdateManagementTokenRecord,
    UpsertOAuthProviderConfigRecord,
};
use crate::LocalMutationOutcome;
use aether_data::repository::auth::{
    read_resolved_auth_api_key_snapshot_by_key_hash,
    read_resolved_auth_api_key_snapshot_by_user_api_key_ids,
};
use futures_util::TryStreamExt;
use sqlx::Row;
use uuid::Uuid;

fn postgres_error(error: sqlx::Error) -> DataLayerError {
    DataLayerError::postgres(error)
}

trait SqlxResultExt<T> {
    fn map_postgres_err(self) -> Result<T, DataLayerError>;
}

impl<T> SqlxResultExt<T> for Result<T, sqlx::Error> {
    fn map_postgres_err(self) -> Result<T, DataLayerError> {
        self.map_err(postgres_error)
    }
}

fn row_get<T>(row: &sqlx::postgres::PgRow, column: &str) -> Result<T, DataLayerError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_postgres_err()
}

const FIND_USER_SESSION_SQL: &str = r#"
SELECT
    id,
    user_id,
    client_device_id,
    device_label,
    refresh_token_hash,
    prev_refresh_token_hash,
    rotated_at,
    last_seen_at,
    expires_at,
    revoked_at,
    revoke_reason,
    ip_address,
    user_agent,
    created_at,
    updated_at
FROM user_sessions
WHERE user_id = $1
  AND id = $2
LIMIT 1
"#;

const LIST_USER_SESSIONS_SQL: &str = r#"
SELECT
    id,
    user_id,
    client_device_id,
    device_label,
    refresh_token_hash,
    prev_refresh_token_hash,
    rotated_at,
    last_seen_at,
    expires_at,
    revoked_at,
    revoke_reason,
    ip_address,
    user_agent,
    created_at,
    updated_at
FROM user_sessions
WHERE user_id = $1
  AND revoked_at IS NULL
  AND expires_at > NOW()
ORDER BY last_seen_at DESC, created_at DESC
"#;

const TOUCH_USER_SESSION_SQL: &str = r#"
UPDATE user_sessions
SET
    last_seen_at = $3,
    ip_address = COALESCE($4, ip_address),
    user_agent = COALESCE($5, user_agent),
    updated_at = $3
WHERE user_id = $1
  AND id = $2
"#;

const UPDATE_USER_SESSION_DEVICE_LABEL_SQL: &str = r#"
UPDATE user_sessions
SET
    device_label = $3,
    updated_at = $4
WHERE user_id = $1
  AND id = $2
"#;

const READ_USER_PREFERENCES_SQL: &str = r#"
SELECT
    up.user_id,
    up.avatar_url,
    up.bio,
    up.default_provider_id,
    p.name AS default_provider_name,
    up.theme,
    up.language,
    up.timezone,
    up.email_notifications,
    up.usage_alerts,
    up.announcement_notifications
FROM user_preferences up
LEFT JOIN providers p
  ON p.id = up.default_provider_id
WHERE up.user_id = $1
LIMIT 1
"#;

const UPSERT_USER_PREFERENCES_SQL: &str = r#"
WITH upserted AS (
    INSERT INTO user_preferences (
        id,
        user_id,
        avatar_url,
        bio,
        default_provider_id,
        theme,
        language,
        timezone,
        email_notifications,
        usage_alerts,
        announcement_notifications
    )
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
    ON CONFLICT (user_id) DO UPDATE SET
        avatar_url = EXCLUDED.avatar_url,
        bio = EXCLUDED.bio,
        default_provider_id = EXCLUDED.default_provider_id,
        theme = EXCLUDED.theme,
        language = EXCLUDED.language,
        timezone = EXCLUDED.timezone,
        email_notifications = EXCLUDED.email_notifications,
        usage_alerts = EXCLUDED.usage_alerts,
        announcement_notifications = EXCLUDED.announcement_notifications,
        updated_at = NOW()
    RETURNING
        user_id,
        avatar_url,
        bio,
        default_provider_id,
        theme,
        language,
        timezone,
        email_notifications,
        usage_alerts,
        announcement_notifications
)
SELECT
    upserted.user_id,
    upserted.avatar_url,
    upserted.bio,
    upserted.default_provider_id,
    p.name AS default_provider_name,
    upserted.theme,
    upserted.language,
    upserted.timezone,
    upserted.email_notifications,
    upserted.usage_alerts,
    upserted.announcement_notifications
FROM upserted
LEFT JOIN providers p
  ON p.id = upserted.default_provider_id
"#;

const FIND_ACTIVE_PROVIDER_NAME_SQL: &str = r#"
SELECT name
FROM providers
WHERE id = $1
  AND is_active = TRUE
LIMIT 1
"#;

const REVOKE_ACTIVE_DEVICE_SESSIONS_SQL: &str = r#"
UPDATE user_sessions
SET
    revoked_at = $3,
    revoke_reason = 'replaced_by_new_login',
    updated_at = $3
WHERE user_id = $1
  AND client_device_id = $2
  AND revoked_at IS NULL
  AND expires_at > $3
"#;

const CREATE_USER_SESSION_SQL: &str = r#"
INSERT INTO user_sessions (
    id,
    user_id,
    client_device_id,
    device_label,
    device_type,
    ip_address,
    user_agent,
    refresh_token_hash,
    last_seen_at,
    expires_at,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
RETURNING
    id,
    user_id,
    client_device_id,
    device_label,
    refresh_token_hash,
    prev_refresh_token_hash,
    rotated_at,
    last_seen_at,
    expires_at,
    revoked_at,
    revoke_reason,
    ip_address,
    user_agent,
    created_at,
    updated_at
"#;

const ROTATE_USER_SESSION_REFRESH_SQL: &str = r#"
UPDATE user_sessions
SET
    prev_refresh_token_hash = $3,
    rotated_at = $4,
    refresh_token_hash = $5,
    expires_at = $6,
    last_seen_at = $4,
    ip_address = COALESCE($7, ip_address),
    user_agent = COALESCE($8, user_agent),
    updated_at = $4
WHERE user_id = $1
  AND id = $2
"#;

const REVOKE_USER_SESSION_SQL: &str = r#"
UPDATE user_sessions
SET
    revoked_at = $3,
    revoke_reason = $4,
    updated_at = $3
WHERE user_id = $1
  AND id = $2
"#;

const REVOKE_ALL_USER_SESSIONS_SQL: &str = r#"
UPDATE user_sessions
SET
    revoked_at = $2,
    revoke_reason = $3,
    updated_at = $2
WHERE user_id = $1
  AND revoked_at IS NULL
"#;

const CREATE_LOCAL_USER_SQL: &str = r#"
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
    $3,
    $4,
    $5,
    'user'::userrole,
    'local'::authsource,
    TRUE,
    FALSE,
    NOW(),
    NOW(),
    NULL
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

const CREATE_LOCAL_USER_WITH_SETTINGS_SQL: &str = r#"
INSERT INTO users (
    id,
    email,
    email_verified,
    username,
    password_hash,
    role,
    auth_source,
    allowed_providers,
    allowed_api_formats,
    allowed_models,
    rate_limit,
    is_active,
    is_deleted,
    created_at,
    updated_at,
    last_login_at
)
VALUES (
    $1,
    $2,
    $3,
    $4,
    $5,
    $6::userrole,
    'local'::authsource,
    $7,
    $8,
    $9,
    $10,
    TRUE,
    FALSE,
    NOW(),
    NOW(),
    NULL
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

const FIND_LDAP_AUTH_USER_BY_DN_SQL: &str = r#"
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
    last_login_at,
    ldap_dn,
    ldap_username
FROM users
WHERE auth_source = 'ldap'::authsource
  AND ldap_dn = $1
LIMIT 1
FOR UPDATE
"#;

const FIND_LDAP_AUTH_USER_BY_LDAP_USERNAME_SQL: &str = r#"
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
    last_login_at,
    ldap_dn,
    ldap_username
FROM users
WHERE auth_source = 'ldap'::authsource
  AND ldap_username = $1
LIMIT 1
FOR UPDATE
"#;

const FIND_LDAP_AUTH_USER_BY_EMAIL_SQL: &str = r#"
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
    last_login_at,
    ldap_dn,
    ldap_username
FROM users
WHERE email = $1
LIMIT 1
FOR UPDATE
"#;

const CHECK_AUTH_USER_EMAIL_TAKEN_SQL: &str = r#"
SELECT 1
FROM users
WHERE email = $1
  AND id <> $2
LIMIT 1
"#;

const CHECK_AUTH_USER_USERNAME_TAKEN_SQL: &str = r#"
SELECT 1
FROM users
WHERE username = $1
LIMIT 1
"#;

const CHECK_AUTH_USER_USERNAME_TAKEN_EXCLUDING_SQL: &str = r#"
SELECT 1
FROM users
WHERE username = $1
  AND id <> $2
LIMIT 1
"#;

const UPDATE_LOCAL_AUTH_USER_PROFILE_SQL: &str = r#"
UPDATE users
SET
    email = COALESCE($2, email),
    username = COALESCE($3, username),
    updated_at = NOW()
WHERE id = $1
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

const UPDATE_LOCAL_AUTH_USER_PASSWORD_SQL: &str = r#"
UPDATE users
SET
    password_hash = $2,
    updated_at = $3
WHERE id = $1
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

const UPDATE_LOCAL_AUTH_USER_ADMIN_FIELDS_SQL: &str = r#"
UPDATE users
SET
    role = CASE
        WHEN $2::BOOLEAN AND $3 IS NOT NULL THEN $3::userrole
        ELSE role
    END,
    allowed_providers = CASE
        WHEN $4::BOOLEAN THEN $5::json
        ELSE allowed_providers
    END,
    allowed_api_formats = CASE
        WHEN $6::BOOLEAN THEN $7::json
        ELSE allowed_api_formats
    END,
    allowed_models = CASE
        WHEN $8::BOOLEAN THEN $9::json
        ELSE allowed_models
    END,
    rate_limit = CASE
        WHEN $10::BOOLEAN AND $11 IS NOT NULL THEN $11
        ELSE rate_limit
    END,
    is_active = CASE
        WHEN $12::BOOLEAN AND $13 IS NOT NULL THEN $13
        ELSE is_active
    END,
    updated_at = NOW()
WHERE id = $1
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

const UPDATE_LDAP_AUTH_USER_SQL: &str = r#"
UPDATE users
SET
    email = $2,
    email_verified = TRUE,
    ldap_dn = COALESCE($3, ldap_dn),
    ldap_username = COALESCE($4, ldap_username),
    last_login_at = $5,
    updated_at = $5
WHERE id = $1
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

const CREATE_LDAP_USER_SQL: &str = r#"
INSERT INTO users (
    id,
    email,
    email_verified,
    username,
    password_hash,
    role,
    auth_source,
    ldap_dn,
    ldap_username,
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
    'ldap'::authsource,
    $4,
    $5,
    TRUE,
    FALSE,
    $6,
    $6,
    $6
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

const TOUCH_AUTH_USER_LAST_LOGIN_SQL: &str = r#"
UPDATE users
SET
    last_login_at = $2,
    updated_at = $2
WHERE id = $1
"#;

const UPDATE_USER_MODEL_CAPABILITY_SETTINGS_SQL: &str = r#"
UPDATE users
SET
    model_capability_settings = $2,
    updated_at = NOW()
WHERE id = $1
RETURNING model_capability_settings
"#;

const COUNT_ACTIVE_ADMIN_USERS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM users
WHERE role = 'admin'::userrole
  AND is_deleted IS FALSE
  AND is_active IS TRUE
"#;

const COUNT_ACTIVE_LOCAL_ADMIN_USERS_WITH_VALID_PASSWORD_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM users
WHERE role = 'admin'::userrole
  AND auth_source = 'local'::authsource
  AND is_deleted IS FALSE
  AND is_active IS TRUE
  AND password_hash ~ '^\$2[aby]\$\d{2}\$.{53}$'
"#;

const COUNT_PENDING_USER_REFUNDS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM refund_requests rr
JOIN wallets w
  ON w.id = rr.wallet_id
LEFT JOIN api_keys ak
  ON ak.id = w.api_key_id
WHERE (w.user_id = $1 OR ak.user_id = $1)
  AND rr.status = ANY($2::TEXT[])
"#;

const COUNT_PENDING_USER_PAYMENT_ORDERS_SQL: &str = r#"
SELECT COUNT(*) AS total
FROM payment_orders po
JOIN wallets w
  ON w.id = po.wallet_id
LEFT JOIN api_keys ak
  ON ak.id = w.api_key_id
WHERE (w.user_id = $1 OR ak.user_id = $1)
  AND po.status = ANY($2::TEXT[])
"#;

const DELETE_LOCAL_AUTH_USER_SQL: &str = r#"
DELETE FROM users
WHERE id = $1
"#;

const UPDATE_AUTH_USER_WALLET_LIMIT_MODE_SQL: &str = r#"
UPDATE wallets
SET
    limit_mode = $2,
    updated_at = NOW()
WHERE user_id = $1
RETURNING
    id,
    user_id,
    api_key_id,
    CAST(balance AS DOUBLE PRECISION) AS balance,
    CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
    limit_mode,
    currency,
    status,
    CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
    CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
    CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
    CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
    CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
"#;

const UPDATE_AUTH_API_KEY_WALLET_LIMIT_MODE_SQL: &str = r#"
UPDATE wallets
SET
    limit_mode = $2,
    updated_at = NOW()
WHERE api_key_id = $1
RETURNING
    id,
    user_id,
    api_key_id,
    CAST(balance AS DOUBLE PRECISION) AS balance,
    CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
    limit_mode,
    currency,
    status,
    CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
    CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
    CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
    CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
    CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
"#;

const UPDATE_AUTH_USER_WALLET_SNAPSHOT_SQL: &str = r#"
UPDATE wallets
SET
    balance = $2,
    gift_balance = $3,
    limit_mode = $4,
    currency = $5,
    status = $6,
    total_recharged = $7,
    total_consumed = $8,
    total_refunded = $9,
    total_adjusted = $10,
    updated_at = COALESCE(TO_TIMESTAMP($11::DOUBLE PRECISION), updated_at)
WHERE user_id = $1
RETURNING
    id,
    user_id,
    api_key_id,
    CAST(balance AS DOUBLE PRECISION) AS balance,
    CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
    limit_mode,
    currency,
    status,
    CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
    CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
    CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
    CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
    CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
"#;

const UPDATE_AUTH_API_KEY_WALLET_SNAPSHOT_SQL: &str = r#"
UPDATE wallets
SET
    balance = $2,
    gift_balance = $3,
    limit_mode = $4,
    currency = $5,
    status = $6,
    total_recharged = $7,
    total_consumed = $8,
    total_refunded = $9,
    total_adjusted = $10,
    updated_at = COALESCE(TO_TIMESTAMP($11::DOUBLE PRECISION), updated_at)
WHERE api_key_id = $1
RETURNING
    id,
    user_id,
    api_key_id,
    CAST(balance AS DOUBLE PRECISION) AS balance,
    CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
    limit_mode,
    currency,
    status,
    CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
    CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
    CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
    CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
    CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
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
    $5,
    NOW(),
    NOW()
)
RETURNING
    id,
    user_id,
    api_key_id,
    CAST(balance AS DOUBLE PRECISION) AS balance,
    CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
    limit_mode,
    currency,
    status,
    CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
    CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
    CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
    CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
    CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
"#;

const CREATE_AUTH_API_KEY_WALLET_SQL: &str = r#"
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
    NULL,
    $2,
    0,
    $3,
    $4,
    'USD',
    'active',
    0,
    0,
    0,
    $5,
    NOW(),
    NOW()
)
RETURNING
    id,
    user_id,
    api_key_id,
    CAST(balance AS DOUBLE PRECISION) AS balance,
    CAST(gift_balance AS DOUBLE PRECISION) AS gift_balance,
    limit_mode,
    currency,
    status,
    CAST(total_recharged AS DOUBLE PRECISION) AS total_recharged,
    CAST(total_consumed AS DOUBLE PRECISION) AS total_consumed,
    CAST(total_refunded AS DOUBLE PRECISION) AS total_refunded,
    CAST(total_adjusted AS DOUBLE PRECISION) AS total_adjusted,
    CAST(EXTRACT(EPOCH FROM updated_at) AS BIGINT) AS updated_at_unix_secs
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

const CREATE_AUTH_API_KEY_WALLET_GIFT_TX_SQL: &str = r#"
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
    '独立余额 Key 初始赠款',
    NOW()
)
"#;

fn map_user_session_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUserSessionRecord, DataLayerError> {
    StoredUserSessionRecord::new(
        row_get(row, "id")?,
        row_get(row, "user_id")?,
        row_get(row, "client_device_id")?,
        row_get(row, "device_label")?,
        row_get(row, "refresh_token_hash")?,
        row_get(row, "prev_refresh_token_hash")?,
        row_get(row, "rotated_at")?,
        row_get(row, "last_seen_at")?,
        row_get(row, "expires_at")?,
        row_get(row, "revoked_at")?,
        row_get(row, "revoke_reason")?,
        row_get(row, "ip_address")?,
        row_get(row, "user_agent")?,
        row_get(row, "created_at")?,
        row_get(row, "updated_at")?,
    )
}

fn map_user_auth_row(row: &sqlx::postgres::PgRow) -> Result<StoredUserAuthRecord, DataLayerError> {
    StoredUserAuthRecord::new(
        row_get(row, "id")?,
        row_get(row, "email")?,
        row_get(row, "email_verified")?,
        row_get(row, "username")?,
        row_get(row, "password_hash")?,
        row_get(row, "role")?,
        row_get(row, "auth_source")?,
        row_get(row, "allowed_providers")?,
        row_get(row, "allowed_api_formats")?,
        row_get(row, "allowed_models")?,
        row_get(row, "is_active")?,
        row_get(row, "is_deleted")?,
        row_get(row, "created_at")?,
        row_get(row, "last_login_at")?,
    )
}

async fn check_username_taken_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    username: &str,
) -> Result<bool, DataLayerError> {
    let row = sqlx::query(CHECK_AUTH_USER_USERNAME_TAKEN_SQL)
        .bind(username)
        .fetch_optional(&mut **tx)
        .await
        .map_postgres_err()?;
    Ok(row.is_some())
}

async fn find_ldap_auth_user_for_update_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ldap_dn: Option<&str>,
    ldap_username: Option<&str>,
    email: &str,
) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
    if let Some(ldap_dn) = ldap_dn.filter(|value| !value.trim().is_empty()) {
        let row = sqlx::query(FIND_LDAP_AUTH_USER_BY_DN_SQL)
            .bind(ldap_dn)
            .fetch_optional(&mut **tx)
            .await
            .map_postgres_err()?;
        if let Some(row) = row.as_ref() {
            return map_user_auth_row(row).map(Some);
        }
    }

    if let Some(ldap_username) = ldap_username.filter(|value| !value.trim().is_empty()) {
        let row = sqlx::query(FIND_LDAP_AUTH_USER_BY_LDAP_USERNAME_SQL)
            .bind(ldap_username)
            .fetch_optional(&mut **tx)
            .await
            .map_postgres_err()?;
        if let Some(row) = row.as_ref() {
            return map_user_auth_row(row).map(Some);
        }
    }

    let row = sqlx::query(FIND_LDAP_AUTH_USER_BY_EMAIL_SQL)
        .bind(email)
        .fetch_optional(&mut **tx)
        .await
        .map_postgres_err()?;
    row.as_ref().map(map_user_auth_row).transpose()
}

fn map_wallet_snapshot_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredWalletSnapshot, DataLayerError> {
    StoredWalletSnapshot::new(
        row_get(row, "id")?,
        row_get(row, "user_id")?,
        row_get(row, "api_key_id")?,
        row_get(row, "balance")?,
        row_get(row, "gift_balance")?,
        row_get(row, "limit_mode")?,
        row_get(row, "currency")?,
        row_get(row, "status")?,
        row_get(row, "total_recharged")?,
        row_get(row, "total_consumed")?,
        row_get(row, "total_refunded")?,
        row_get(row, "total_adjusted")?,
        row_get(row, "updated_at_unix_secs")?,
    )
}

fn map_user_preference_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUserPreferenceRecord, DataLayerError> {
    let user_id = row_get::<String>(row, "user_id")?;
    if user_id.trim().is_empty() {
        return Err(DataLayerError::UnexpectedValue(
            "user_preferences.user_id is empty".to_string(),
        ));
    }

    Ok(StoredUserPreferenceRecord {
        user_id,
        avatar_url: row_get(row, "avatar_url")?,
        bio: row_get(row, "bio")?,
        default_provider_id: row_get(row, "default_provider_id")?,
        default_provider_name: row_get(row, "default_provider_name")?,
        theme: row_get(row, "theme")?,
        language: row_get(row, "language")?,
        timezone: row_get(row, "timezone")?,
        email_notifications: row_get(row, "email_notifications")?,
        usage_alerts: row_get(row, "usage_alerts")?,
        announcement_notifications: row_get(row, "announcement_notifications")?,
    })
}

fn normalize_optional_json_value(value: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match value {
        Some(serde_json::Value::Null) | None => None,
        Some(value) => Some(value),
    }
}

impl GatewayDataState {
    pub(crate) async fn is_other_user_auth_email_taken(
        &self,
        email: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        let row = sqlx::query(CHECK_AUTH_USER_EMAIL_TAKEN_SQL)
            .bind(email)
            .bind(user_id)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        Ok(row.is_some())
    }

    pub(crate) async fn is_other_user_auth_username_taken(
        &self,
        username: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        let row = sqlx::query(CHECK_AUTH_USER_USERNAME_TAKEN_EXCLUDING_SQL)
            .bind(username)
            .bind(user_id)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        Ok(row.is_some())
    }

    pub(crate) async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.find_user_auth_by_id(user_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        match &self.user_reader {
            Some(repository) => repository.find_user_auth_by_identifier(identifier).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn read_user_preferences(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        if let Some(store) = &self.user_preferences {
            return Ok(store
                .read()
                .expect("user preference store should lock")
                .get(user_id)
                .cloned());
        }

        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(READ_USER_PREFERENCES_SQL)
            .bind(user_id)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_preference_row).transpose()
    }

    pub(crate) async fn write_user_preferences(
        &self,
        preferences: &StoredUserPreferenceRecord,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        if let Some(store) = &self.user_preferences {
            store
                .write()
                .expect("user preference store should lock")
                .insert(preferences.user_id.clone(), preferences.clone());
            return Ok(Some(preferences.clone()));
        }

        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPSERT_USER_PREFERENCES_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(&preferences.user_id)
            .bind(preferences.avatar_url.as_deref())
            .bind(preferences.bio.as_deref())
            .bind(preferences.default_provider_id.as_deref())
            .bind(&preferences.theme)
            .bind(&preferences.language)
            .bind(&preferences.timezone)
            .bind(preferences.email_notifications)
            .bind(preferences.usage_alerts)
            .bind(preferences.announcement_notifications)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_preference_row).transpose()
    }

    pub(crate) async fn find_active_provider_name(
        &self,
        provider_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        if self.provider_catalog_reader.is_some() {
            let providers = self.list_provider_catalog_providers(true).await?;
            return Ok(providers
                .into_iter()
                .find(|provider| provider.id == provider_id)
                .map(|provider| provider.name));
        }

        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(FIND_ACTIVE_PROVIDER_NAME_SQL)
            .bind(provider_id)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        Ok(row.and_then(|row| row.try_get("name").ok()))
    }

    pub(crate) async fn find_user_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(FIND_USER_SESSION_SQL)
            .bind(user_id)
            .bind(session_id)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_session_row).transpose()
    }

    pub(crate) async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserSessionRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(Vec::new());
        };
        let mut rows = sqlx::query(LIST_USER_SESSIONS_SQL)
            .bind(user_id)
            .fetch(&pool);
        let mut sessions = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            sessions.push(map_user_session_row(&row)?);
        }
        Ok(sessions)
    }

    pub(crate) async fn create_user_session(
        &self,
        session: &StoredUserSessionRecord,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let now = session
            .created_at
            .or(session.updated_at)
            .or(session.last_seen_at)
            .unwrap_or_else(chrono::Utc::now);
        sqlx::query(REVOKE_ACTIVE_DEVICE_SESSIONS_SQL)
            .bind(&session.user_id)
            .bind(&session.client_device_id)
            .bind(now)
            .execute(&pool)
            .await
            .map_postgres_err()?;
        let row = sqlx::query(CREATE_USER_SESSION_SQL)
            .bind(&session.id)
            .bind(&session.user_id)
            .bind(&session.client_device_id)
            .bind(session.device_label.as_deref())
            .bind("unknown")
            .bind(session.ip_address.as_deref())
            .bind(session.user_agent.as_deref())
            .bind(&session.refresh_token_hash)
            .bind(session.last_seen_at.unwrap_or(now))
            .bind(session.expires_at.unwrap_or(now))
            .bind(session.created_at.unwrap_or(now))
            .bind(session.updated_at.unwrap_or(now))
            .fetch_one(&pool)
            .await
            .map_postgres_err()?;
        Ok(Some(map_user_session_row(&row)?))
    }

    pub(crate) async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPDATE_USER_MODEL_CAPABILITY_SETTINGS_SQL)
            .bind(user_id)
            .bind(settings)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        Ok(row
            .as_ref()
            .and_then(|row| row.try_get("model_capability_settings").ok())
            .and_then(normalize_optional_json_value))
    }

    pub(crate) async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email: Option<String>,
        username: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPDATE_LOCAL_AUTH_USER_PROFILE_SQL)
            .bind(user_id)
            .bind(email)
            .bind(username)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    pub(crate) async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPDATE_LOCAL_AUTH_USER_PASSWORD_SQL)
            .bind(user_id)
            .bind(password_hash)
            .bind(updated_at)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    #[allow(dead_code)]
    pub(crate) async fn create_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(CREATE_LOCAL_USER_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(email)
            .bind(email_verified)
            .bind(username)
            .bind(password_hash)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_local_auth_user_with_settings(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        role: String,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models: Option<Vec<String>>,
        rate_limit: Option<i32>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(CREATE_LOCAL_USER_WITH_SETTINGS_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(email)
            .bind(email_verified)
            .bind(username)
            .bind(password_hash)
            .bind(role)
            .bind(allowed_providers.map(serde_json::Value::from))
            .bind(allowed_api_formats.map(serde_json::Value::from))
            .bind(allowed_models.map(serde_json::Value::from))
            .bind(rate_limit)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_local_auth_user_admin_fields(
        &self,
        user_id: &str,
        role: Option<String>,
        allowed_providers_present: bool,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats_present: bool,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models_present: bool,
        allowed_models: Option<Vec<String>>,
        rate_limit: Option<i32>,
        is_active: Option<bool>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPDATE_LOCAL_AUTH_USER_ADMIN_FIELDS_SQL)
            .bind(user_id)
            .bind(role.is_some())
            .bind(role)
            .bind(allowed_providers_present)
            .bind(allowed_providers.map(serde_json::Value::from))
            .bind(allowed_api_formats_present)
            .bind(allowed_api_formats.map(serde_json::Value::from))
            .bind(allowed_models_present)
            .bind(allowed_models.map(serde_json::Value::from))
            .bind(rate_limit.is_some())
            .bind(rate_limit)
            .bind(is_active.is_some())
            .bind(is_active)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    pub(crate) async fn touch_auth_user_last_login(
        &self,
        user_id: &str,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        let result = sqlx::query(TOUCH_AUTH_USER_LAST_LOGIN_SQL)
            .bind(user_id)
            .bind(logged_in_at)
            .execute(&pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn get_or_create_ldap_auth_user(
        &self,
        email: String,
        username: String,
        ldap_dn: Option<String>,
        ldap_username: Option<String>,
        logged_in_at: chrono::DateTime<chrono::Utc>,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };

        let mut tx = pool.begin().await.map_postgres_err()?;
        let existing = find_ldap_auth_user_for_update_in_tx(
            &mut tx,
            ldap_dn.as_deref(),
            ldap_username.as_deref(),
            &email,
        )
        .await?;

        if let Some(existing) = existing {
            if existing.is_deleted || !existing.is_active {
                tx.commit().await.map_err(postgres_error)?;
                return Ok(None);
            }
            if !existing.auth_source.eq_ignore_ascii_case("ldap") {
                tx.commit().await.map_err(postgres_error)?;
                return Ok(None);
            }

            let email_changed = existing.email.as_deref() != Some(email.as_str());
            if email_changed {
                let taken = sqlx::query(CHECK_AUTH_USER_EMAIL_TAKEN_SQL)
                    .bind(&email)
                    .bind(&existing.id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_postgres_err()?;
                if taken.is_some() {
                    tx.commit().await.map_err(postgres_error)?;
                    return Ok(None);
                }
            }

            let row = sqlx::query(UPDATE_LDAP_AUTH_USER_SQL)
                .bind(&existing.id)
                .bind(&email)
                .bind(ldap_dn.as_deref())
                .bind(ldap_username.as_deref())
                .bind(logged_in_at)
                .fetch_one(&mut *tx)
                .await
                .map_postgres_err()?;
            tx.commit().await.map_err(postgres_error)?;
            return Ok(Some(map_user_auth_row(&row)?));
        }

        let base_username = ldap_username
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(username.as_str())
            .trim()
            .to_string();
        let mut candidate_username = base_username.clone();
        for _attempt in 0..3 {
            if check_username_taken_in_tx(&mut tx, &candidate_username).await? {
                let suffix = Uuid::new_v4().simple().to_string();
                candidate_username = format!(
                    "{}_ldap_{}{}",
                    base_username,
                    logged_in_at.timestamp(),
                    &suffix[..4]
                );
                continue;
            }

            let user_row = sqlx::query(CREATE_LDAP_USER_SQL)
                .bind(Uuid::new_v4().to_string())
                .bind(&email)
                .bind(&candidate_username)
                .bind(ldap_dn.as_deref())
                .bind(ldap_username.as_deref())
                .bind(logged_in_at)
                .fetch_one(&mut *tx)
                .await
                .map_postgres_err()?;
            let user = map_user_auth_row(&user_row)?;

            let gift_amount = if unlimited {
                0.0
            } else {
                initial_gift_usd.max(0.0)
            };
            let wallet_row = sqlx::query(CREATE_AUTH_USER_WALLET_SQL)
                .bind(Uuid::new_v4().to_string())
                .bind(&user.id)
                .bind(gift_amount)
                .bind(if unlimited { "unlimited" } else { "finite" })
                .bind(gift_amount)
                .fetch_one(&mut *tx)
                .await
                .map_postgres_err()?;
            let wallet = map_wallet_snapshot_row(&wallet_row)?;
            if gift_amount > 0.0 {
                sqlx::query(CREATE_AUTH_USER_WALLET_GIFT_TX_SQL)
                    .bind(Uuid::new_v4().to_string())
                    .bind(&wallet.id)
                    .bind(gift_amount)
                    .bind(&user.id)
                    .execute(&mut *tx)
                    .await
                    .map_postgres_err()?;
            }

            tx.commit().await.map_err(postgres_error)?;
            return Ok(Some(user));
        }

        tx.commit().await.map_err(postgres_error)?;
        Ok(None)
    }

    #[allow(dead_code)]
    pub(crate) async fn initialize_auth_user_wallet(
        &self,
        user_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let mut tx = pool.begin().await.map_postgres_err()?;
        let gift_amount = if unlimited {
            0.0
        } else {
            initial_gift_usd.max(0.0)
        };
        let row = sqlx::query(CREATE_AUTH_USER_WALLET_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(user_id)
            .bind(gift_amount)
            .bind(if unlimited { "unlimited" } else { "finite" })
            .bind(gift_amount)
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
        let wallet = map_wallet_snapshot_row(&row)?;
        if gift_amount > 0.0 {
            sqlx::query(CREATE_AUTH_USER_WALLET_GIFT_TX_SQL)
                .bind(Uuid::new_v4().to_string())
                .bind(&wallet.id)
                .bind(gift_amount)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        tx.commit().await.map_err(postgres_error)?;
        Ok(Some(wallet))
    }

    pub(crate) async fn initialize_auth_api_key_wallet(
        &self,
        api_key_id: &str,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let mut tx = pool.begin().await.map_postgres_err()?;
        let gift_amount = if unlimited {
            0.0
        } else {
            initial_gift_usd.max(0.0)
        };
        let row = sqlx::query(CREATE_AUTH_API_KEY_WALLET_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(api_key_id)
            .bind(gift_amount)
            .bind(if unlimited { "unlimited" } else { "finite" })
            .bind(gift_amount)
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
        let wallet = map_wallet_snapshot_row(&row)?;
        if gift_amount > 0.0 {
            sqlx::query(CREATE_AUTH_API_KEY_WALLET_GIFT_TX_SQL)
                .bind(Uuid::new_v4().to_string())
                .bind(&wallet.id)
                .bind(gift_amount)
                .bind(api_key_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        tx.commit().await.map_err(postgres_error)?;
        Ok(Some(wallet))
    }

    pub(crate) async fn update_auth_user_wallet_limit_mode(
        &self,
        user_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPDATE_AUTH_USER_WALLET_LIMIT_MODE_SQL)
            .bind(user_id)
            .bind(limit_mode)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_wallet_snapshot_row).transpose()
    }

    pub(crate) async fn update_auth_api_key_wallet_limit_mode(
        &self,
        api_key_id: &str,
        limit_mode: &str,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPDATE_AUTH_API_KEY_WALLET_LIMIT_MODE_SQL)
            .bind(api_key_id)
            .bind(limit_mode)
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_wallet_snapshot_row).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_auth_user_wallet_snapshot(
        &self,
        user_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPDATE_AUTH_USER_WALLET_SNAPSHOT_SQL)
            .bind(user_id)
            .bind(balance)
            .bind(gift_balance)
            .bind(limit_mode)
            .bind(currency)
            .bind(status)
            .bind(total_recharged)
            .bind(total_consumed)
            .bind(total_refunded)
            .bind(total_adjusted)
            .bind(updated_at_unix_secs.map(|value| value as i64))
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_wallet_snapshot_row).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_auth_api_key_wallet_snapshot(
        &self,
        api_key_id: &str,
        balance: f64,
        gift_balance: f64,
        limit_mode: &str,
        currency: &str,
        status: &str,
        total_recharged: f64,
        total_consumed: f64,
        total_refunded: f64,
        total_adjusted: f64,
        updated_at_unix_secs: Option<u64>,
    ) -> Result<Option<StoredWalletSnapshot>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let row = sqlx::query(UPDATE_AUTH_API_KEY_WALLET_SNAPSHOT_SQL)
            .bind(api_key_id)
            .bind(balance)
            .bind(gift_balance)
            .bind(limit_mode)
            .bind(currency)
            .bind(status)
            .bind(total_recharged)
            .bind(total_consumed)
            .bind(total_refunded)
            .bind(total_adjusted)
            .bind(updated_at_unix_secs.map(|value| value as i64))
            .fetch_optional(&pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_wallet_snapshot_row).transpose()
    }

    pub(crate) async fn count_active_admin_users(&self) -> Result<u64, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(0);
        };
        let row = sqlx::query(COUNT_ACTIVE_ADMIN_USERS_SQL)
            .fetch_one(&pool)
            .await
            .map_postgres_err()?;
        let total = row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64;
        Ok(total)
    }

    pub(crate) async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(0);
        };
        let row = sqlx::query(COUNT_ACTIVE_LOCAL_ADMIN_USERS_WITH_VALID_PASSWORD_SQL)
            .fetch_one(&pool)
            .await
            .map_postgres_err()?;
        let total = row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64;
        Ok(total)
    }

    pub(crate) async fn count_user_pending_refunds(
        &self,
        user_id: &str,
    ) -> Result<u64, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(0);
        };
        let statuses = vec![
            "pending_approval".to_string(),
            "approved".to_string(),
            "processing".to_string(),
        ];
        let row = sqlx::query(COUNT_PENDING_USER_REFUNDS_SQL)
            .bind(user_id)
            .bind(statuses)
            .fetch_one(&pool)
            .await
            .map_postgres_err()?;
        let total = row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64;
        Ok(total)
    }

    pub(crate) async fn count_user_pending_payment_orders(
        &self,
        user_id: &str,
    ) -> Result<u64, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(0);
        };
        let statuses = vec!["pending".to_string(), "paid".to_string()];
        let row = sqlx::query(COUNT_PENDING_USER_PAYMENT_ORDERS_SQL)
            .bind(user_id)
            .bind(statuses)
            .fetch_one(&pool)
            .await
            .map_postgres_err()?;
        let total = row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64;
        Ok(total)
    }

    pub(crate) async fn delete_local_auth_user(
        &self,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        let result = sqlx::query(DELETE_LOCAL_AUTH_USER_SQL)
            .bind(user_id)
            .execute(&pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub(crate) async fn register_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
        initial_gift_usd: f64,
        unlimited: bool,
    ) -> Result<Option<(StoredUserAuthRecord, StoredWalletSnapshot)>, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(None);
        };
        let mut tx = pool.begin().await.map_postgres_err()?;
        let user_row = sqlx::query(CREATE_LOCAL_USER_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(email)
            .bind(email_verified)
            .bind(username)
            .bind(password_hash)
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
        let user = map_user_auth_row(&user_row)?;
        let gift_amount = if unlimited {
            0.0
        } else {
            initial_gift_usd.max(0.0)
        };
        let wallet_row = sqlx::query(CREATE_AUTH_USER_WALLET_SQL)
            .bind(Uuid::new_v4().to_string())
            .bind(&user.id)
            .bind(gift_amount)
            .bind(if unlimited { "unlimited" } else { "finite" })
            .bind(gift_amount)
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
        let wallet = map_wallet_snapshot_row(&wallet_row)?;
        if gift_amount > 0.0 {
            sqlx::query(CREATE_AUTH_USER_WALLET_GIFT_TX_SQL)
                .bind(Uuid::new_v4().to_string())
                .bind(&wallet.id)
                .bind(gift_amount)
                .bind(&user.id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        tx.commit().await.map_err(postgres_error)?;
        Ok(Some((user, wallet)))
    }

    pub(crate) async fn touch_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        touched_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        let result = sqlx::query(TOUCH_USER_SESSION_SQL)
            .bind(user_id)
            .bind(session_id)
            .bind(touched_at)
            .bind(ip_address)
            .bind(user_agent.map(|value| value.chars().take(1000).collect::<String>()))
            .execute(&pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub(crate) async fn update_user_session_device_label(
        &self,
        user_id: &str,
        session_id: &str,
        device_label: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        let result = sqlx::query(UPDATE_USER_SESSION_DEVICE_LABEL_SQL)
            .bind(user_id)
            .bind(session_id)
            .bind(device_label.chars().take(120).collect::<String>())
            .bind(updated_at)
            .execute(&pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub(crate) async fn rotate_user_session_refresh_token(
        &self,
        user_id: &str,
        session_id: &str,
        previous_refresh_token_hash: &str,
        next_refresh_token_hash: &str,
        rotated_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        let result = sqlx::query(ROTATE_USER_SESSION_REFRESH_SQL)
            .bind(user_id)
            .bind(session_id)
            .bind(previous_refresh_token_hash)
            .bind(rotated_at)
            .bind(next_refresh_token_hash)
            .bind(expires_at)
            .bind(ip_address)
            .bind(user_agent.map(|value| value.chars().take(1000).collect::<String>()))
            .execute(&pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub(crate) async fn revoke_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<bool, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(false);
        };
        let result = sqlx::query(REVOKE_USER_SESSION_SQL)
            .bind(user_id)
            .bind(session_id)
            .bind(revoked_at)
            .bind(reason.chars().take(100).collect::<String>())
            .execute(&pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub(crate) async fn revoke_all_user_sessions(
        &self,
        user_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<u64, DataLayerError> {
        let Some(pool) = self.postgres_pool() else {
            return Ok(0);
        };
        let result = sqlx::query(REVOKE_ALL_USER_SESSIONS_SQL)
            .bind(user_id)
            .bind(revoked_at)
            .bind(reason.chars().take(100).collect::<String>())
            .execute(&pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected())
    }

    pub(crate) async fn list_enabled_oauth_module_providers(
        &self,
    ) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
        match &self.auth_module_reader {
            Some(repository) => repository.list_enabled_oauth_providers().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn get_ldap_module_config(
        &self,
    ) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        match &self.auth_module_reader {
            Some(repository) => repository.get_ldap_config().await,
            None => Ok(None),
        }
    }

    pub(crate) async fn upsert_ldap_module_config(
        &self,
        config: &StoredLdapModuleConfig,
    ) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        match &self.auth_module_writer {
            Some(repository) => repository.upsert_ldap_config(config).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_oauth_provider_configs(
        &self,
    ) -> Result<Vec<StoredOAuthProviderConfig>, DataLayerError> {
        match &self.oauth_provider_reader {
            Some(repository) => repository.list_oauth_provider_configs().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn get_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<Option<StoredOAuthProviderConfig>, DataLayerError> {
        match &self.oauth_provider_reader {
            Some(repository) => repository.get_oauth_provider_config(provider_type).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn count_locked_users_if_oauth_provider_disabled(
        &self,
        provider_type: &str,
        ldap_exclusive: bool,
    ) -> Result<usize, DataLayerError> {
        match &self.oauth_provider_reader {
            Some(repository) => {
                repository
                    .count_locked_users_if_provider_disabled(provider_type, ldap_exclusive)
                    .await
            }
            None => Ok(0),
        }
    }

    pub(crate) async fn upsert_oauth_provider_config(
        &self,
        record: &UpsertOAuthProviderConfigRecord,
    ) -> Result<Option<StoredOAuthProviderConfig>, DataLayerError> {
        match &self.oauth_provider_writer {
            Some(repository) => repository
                .upsert_oauth_provider_config(record)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_oauth_provider_config(
        &self,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.oauth_provider_writer {
            Some(repository) => repository.delete_oauth_provider_config(provider_type).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn list_management_tokens(
        &self,
        query: &ManagementTokenListQuery,
    ) -> Result<StoredManagementTokenListPage, DataLayerError> {
        match &self.management_token_reader {
            Some(repository) => repository.list_management_tokens(query).await,
            None => Ok(StoredManagementTokenListPage {
                items: Vec::new(),
                total: 0,
            }),
        }
    }

    pub(crate) async fn get_management_token_with_user(
        &self,
        token_id: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        match &self.management_token_reader {
            Some(repository) => repository.get_management_token_with_user(token_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn get_management_token_with_user_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        match &self.management_token_reader {
            Some(repository) => {
                repository
                    .get_management_token_with_user_by_hash(token_hash)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn create_management_token(
        &self,
        record: &CreateManagementTokenRecord,
    ) -> Result<LocalMutationOutcome<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => match repository.create_management_token(record).await {
                Ok(token) => Ok(LocalMutationOutcome::Applied(token)),
                Err(DataLayerError::InvalidInput(detail)) => {
                    Ok(LocalMutationOutcome::Invalid(detail))
                }
                Err(err) => Err(err),
            },
            None => Ok(LocalMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn update_management_token(
        &self,
        record: &UpdateManagementTokenRecord,
    ) -> Result<LocalMutationOutcome<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => match repository.update_management_token(record).await {
                Ok(Some(token)) => Ok(LocalMutationOutcome::Applied(token)),
                Ok(None) => Ok(LocalMutationOutcome::NotFound),
                Err(DataLayerError::InvalidInput(detail)) => {
                    Ok(LocalMutationOutcome::Invalid(detail))
                }
                Err(err) => Err(err),
            },
            None => Ok(LocalMutationOutcome::Unavailable),
        }
    }

    pub(crate) async fn delete_management_token(
        &self,
        token_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => repository.delete_management_token(token_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn record_management_token_usage(
        &self,
        token_id: &str,
        last_used_ip: Option<&str>,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => {
                repository
                    .record_management_token_usage(token_id, last_used_ip)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn find_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => repository.find_proxy_node(node_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_proxy_nodes(&self) -> Result<Vec<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => repository.list_proxy_nodes().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_proxy_node_events(
        &self,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredProxyNodeEvent>, DataLayerError> {
        match &self.proxy_node_reader {
            Some(repository) => repository.list_proxy_node_events(node_id, limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn register_proxy_node(
        &self,
        mutation: &ProxyNodeRegistrationMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.register_node(mutation).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn create_manual_proxy_node(
        &self,
        mutation: &ProxyNodeManualCreateMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.create_manual_node(mutation).await.map(Some),
            None => Ok(None),
        }
    }

    pub(crate) async fn update_manual_proxy_node(
        &self,
        mutation: &ProxyNodeManualUpdateMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.update_manual_node(mutation).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn reset_stale_proxy_node_tunnel_statuses(
        &self,
    ) -> Result<usize, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.reset_stale_tunnel_statuses().await,
            None => Ok(0),
        }
    }

    pub(crate) async fn apply_proxy_node_heartbeat(
        &self,
        mutation: &ProxyNodeHeartbeatMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.apply_heartbeat(mutation).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_proxy_node_tunnel_status(
        &self,
        mutation: &ProxyNodeTunnelStatusMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.update_tunnel_status(mutation).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn unregister_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.unregister_node(node_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_proxy_node(
        &self,
        node_id: &str,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.delete_node(node_id).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_proxy_node_remote_config(
        &self,
        mutation: &ProxyNodeRemoteConfigMutation,
    ) -> Result<Option<StoredProxyNode>, DataLayerError> {
        match &self.proxy_node_writer {
            Some(repository) => repository.update_remote_config(mutation).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn set_management_token_active(
        &self,
        token_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => {
                repository
                    .set_management_token_active(token_id, is_active)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn regenerate_management_token_secret(
        &self,
        mutation: &RegenerateManagementTokenSecret,
    ) -> Result<LocalMutationOutcome<StoredManagementToken>, DataLayerError> {
        match &self.management_token_writer {
            Some(repository) => match repository
                .regenerate_management_token_secret(mutation)
                .await
            {
                Ok(Some(token)) => Ok(LocalMutationOutcome::Applied(token)),
                Ok(None) => Ok(LocalMutationOutcome::NotFound),
                Err(DataLayerError::InvalidInput(detail)) => {
                    Ok(LocalMutationOutcome::Invalid(detail))
                }
                Err(err) => Err(err),
            },
            None => Ok(LocalMutationOutcome::Unavailable),
        }
    }

    pub(in crate::data) async fn find_auth_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.find_api_key_snapshot(key).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn list_auth_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_api_key_snapshots_by_ids(api_key_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_records_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_export_api_keys_by_user_ids(user_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_records_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_export_api_keys_by_ids(api_key_ids).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_records_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .list_export_api_keys_by_name_search(name_search)
                    .await
            }
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_standalone_records_page(
        &self,
        query: &aether_data::repository::auth::StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_export_standalone_api_keys_page(query).await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn count_auth_api_key_export_standalone_records(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.count_export_standalone_api_keys(is_active).await,
            None => Ok(0),
        }
    }

    pub(crate) async fn summarize_auth_api_key_export_records_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .summarize_export_api_keys_by_user_ids(user_ids, now_unix_secs)
                    .await
            }
            None => Ok(aether_data::repository::auth::AuthApiKeyExportSummary::default()),
        }
    }

    pub(crate) async fn summarize_auth_api_key_export_non_standalone_records(
        &self,
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .summarize_export_non_standalone_api_keys(now_unix_secs)
                    .await
            }
            None => Ok(aether_data::repository::auth::AuthApiKeyExportSummary::default()),
        }
    }

    pub(crate) async fn list_auth_api_key_export_standalone_records(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => repository.list_export_standalone_api_keys().await,
            None => Ok(Vec::new()),
        }
    }

    pub(crate) async fn summarize_auth_api_key_export_standalone_records(
        &self,
        now_unix_secs: u64,
    ) -> Result<aether_data::repository::auth::AuthApiKeyExportSummary, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .summarize_export_standalone_api_keys(now_unix_secs)
                    .await
            }
            None => Ok(aether_data::repository::auth::AuthApiKeyExportSummary::default()),
        }
    }

    pub(crate) async fn find_auth_api_key_export_standalone_record_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_reader {
            Some(repository) => {
                repository
                    .find_export_standalone_api_key_by_id(api_key_id)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn create_user_api_key(
        &self,
        record: aether_data::repository::auth::CreateUserApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.create_user_api_key(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn create_standalone_api_key(
        &self,
        record: aether_data::repository::auth::CreateStandaloneApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.create_standalone_api_key(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_user_api_key_basic(
        &self,
        record: aether_data::repository::auth::UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.update_user_api_key_basic(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn update_standalone_api_key_basic(
        &self,
        record: aether_data::repository::auth::UpdateStandaloneApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.update_standalone_api_key_basic(record).await,
            None => Ok(None),
        }
    }

    pub(crate) async fn set_user_api_key_active(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_active(user_id, api_key_id, is_active)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn set_standalone_api_key_active(
        &self,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_standalone_api_key_active(api_key_id, is_active)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn set_user_api_key_locked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_locked: bool,
    ) -> Result<bool, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_locked(user_id, api_key_id, is_locked)
                    .await
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn set_user_api_key_allowed_providers(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_allowed_providers(user_id, api_key_id, allowed_providers)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn set_user_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => {
                repository
                    .set_user_api_key_force_capabilities(user_id, api_key_id, force_capabilities)
                    .await
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn delete_user_api_key(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.delete_user_api_key(user_id, api_key_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn delete_standalone_api_key(
        &self,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        match &self.auth_api_key_writer {
            Some(repository) => repository.delete_standalone_api_key(api_key_id).await,
            None => Ok(false),
        }
    }

    pub(crate) async fn read_auth_api_key_snapshot(
        &self,
        user_id: &str,
        api_key_id: &str,
        now_unix_secs: u64,
    ) -> Result<Option<GatewayAuthApiKeySnapshot>, DataLayerError> {
        read_resolved_auth_api_key_snapshot_by_user_api_key_ids(
            self,
            user_id,
            api_key_id,
            now_unix_secs,
        )
        .await
    }

    pub(crate) async fn read_auth_api_key_snapshot_by_key_hash(
        &self,
        key_hash: &str,
        now_unix_secs: u64,
    ) -> Result<Option<GatewayAuthApiKeySnapshot>, DataLayerError> {
        read_resolved_auth_api_key_snapshot_by_key_hash(self, key_hash, now_unix_secs).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aether_data::repository::auth::{
        InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeyExportRecord,
        StoredAuthApiKeySnapshot,
    };

    use super::UPDATE_LOCAL_AUTH_USER_ADMIN_FIELDS_SQL;
    use crate::data::GatewayDataState;

    fn sample_snapshot(api_key_id: &str, user_id: &str) -> StoredAuthApiKeySnapshot {
        StoredAuthApiKeySnapshot::new(
            user_id.to_string(),
            "alice".to_string(),
            Some("alice@example.com".to_string()),
            "user".to_string(),
            "local".to_string(),
            true,
            false,
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5"])),
            api_key_id.to_string(),
            Some("default".to_string()),
            true,
            false,
            false,
            Some(60),
            Some(5),
            Some(200),
            Some(serde_json::json!(["openai"])),
            Some(serde_json::json!(["openai:chat"])),
            Some(serde_json::json!(["gpt-5"])),
        )
        .expect("snapshot should build")
    }

    #[test]
    fn update_local_auth_user_admin_fields_sql_casts_json_case_values() {
        assert!(UPDATE_LOCAL_AUTH_USER_ADMIN_FIELDS_SQL.contains("WHEN $4::BOOLEAN THEN $5::json"));
        assert!(UPDATE_LOCAL_AUTH_USER_ADMIN_FIELDS_SQL.contains("WHEN $6::BOOLEAN THEN $7::json"));
        assert!(UPDATE_LOCAL_AUTH_USER_ADMIN_FIELDS_SQL.contains("WHEN $8::BOOLEAN THEN $9::json"));
    }

    #[tokio::test]
    async fn data_state_lists_auth_api_key_export_records() {
        let repository = Arc::new(
            InMemoryAuthApiKeySnapshotRepository::seed(vec![
                (
                    Some("hash-user".to_string()),
                    sample_snapshot("key-user", "user-1"),
                ),
                (
                    Some("hash-standalone".to_string()),
                    sample_snapshot("key-standalone", "admin-1"),
                ),
            ])
            .with_export_records(vec![
                StoredAuthApiKeyExportRecord::new(
                    "user-1".to_string(),
                    "key-user".to_string(),
                    "hash-user".to_string(),
                    Some("enc-user".to_string()),
                    Some("default".to_string()),
                    None,
                    None,
                    Some(serde_json::json!(["gpt-5"])),
                    Some(60),
                    Some(5),
                    Some(serde_json::json!({"cache_1h": true})),
                    true,
                    Some(200),
                    false,
                    9,
                    1.75,
                    false,
                )
                .expect("user export record should build"),
                StoredAuthApiKeyExportRecord::new(
                    "admin-1".to_string(),
                    "key-standalone".to_string(),
                    "hash-standalone".to_string(),
                    Some("enc-standalone".to_string()),
                    Some("standalone".to_string()),
                    None,
                    None,
                    None,
                    None,
                    Some(1),
                    None,
                    true,
                    None,
                    true,
                    2,
                    0.5,
                    true,
                )
                .expect("standalone export record should build"),
            ]),
        );

        let state = GatewayDataState::with_auth_api_key_reader_for_tests(repository);

        let user_records = state
            .list_auth_api_key_export_records_by_user_ids(&["user-1".to_string()])
            .await
            .expect("user export records should load");
        assert_eq!(user_records.len(), 1);
        assert_eq!(user_records[0].api_key_id, "key-user");
        assert_eq!(user_records[0].total_requests, 9);

        let selected_records = state
            .list_auth_api_key_export_records_by_ids(&[
                "key-standalone".to_string(),
                "missing".to_string(),
                "key-user".to_string(),
            ])
            .await
            .expect("selected export records should load");
        assert_eq!(selected_records.len(), 2);
        assert_eq!(selected_records[0].api_key_id, "key-standalone");
        assert_eq!(selected_records[1].api_key_id, "key-user");

        let paged_records = state
            .list_auth_api_key_export_standalone_records_page(
                &aether_data::repository::auth::StandaloneApiKeyExportListQuery {
                    skip: 0,
                    limit: 10,
                    is_active: Some(true),
                },
            )
            .await
            .expect("paged standalone export records should load");
        assert_eq!(paged_records.len(), 1);
        assert_eq!(paged_records[0].api_key_id, "key-standalone");
        assert_eq!(
            state
                .count_auth_api_key_export_standalone_records(Some(true))
                .await
                .expect("standalone export count should load"),
            1
        );

        let standalone_records = state
            .list_auth_api_key_export_standalone_records()
            .await
            .expect("standalone export records should load");
        assert_eq!(standalone_records.len(), 1);
        assert_eq!(standalone_records[0].api_key_id, "key-standalone");
        assert!(standalone_records[0].is_standalone);
    }
}
