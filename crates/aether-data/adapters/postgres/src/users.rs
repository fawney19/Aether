use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use aether_data_contracts::repository::users::{
    is_valid_bcrypt_hash, last_oauth_unbind_denial, normalize_user_group_name,
    BindUserOAuthLinkOutcome, BindUserOAuthLinkSessionExpectation, DeleteUserOAuthLinkOutcome,
    LdapAuthUserProvisioningOutcome, ResolveOAuthLinkedUserOutcome, StoredUserAuthRecord,
    StoredUserExportRow, StoredUserGroup, StoredUserGroupMember, StoredUserGroupMembership,
    StoredUserOAuthLinkSummary, StoredUserPreferenceRecord, StoredUserSessionRecord,
    StoredUserSummary, UpsertUserGroupRecord, UserExportListQuery, UserExportSortBy,
    UserExportSummary, UserReadRepository, LAST_ACTIVE_ADMIN_DELETE_DENIED,
    LAST_ACTIVE_ADMIN_UPDATE_DENIED,
};
use aether_data_contracts::DataLayerError;

use crate::error::SqlxResultExt;

const LIST_USERS_BY_IDS_SQL: &str = r#"
SELECT
  id,
  username,
  email,
  role::text AS role,
  is_active,
  is_deleted
FROM users
WHERE id = ANY($1::text[])
ORDER BY id ASC
"#;

const POSTGRES_LOCK_ACTIVE_ADMINS_SQL: &str = r#"
SELECT id
FROM users
WHERE role = 'admin'::userrole
  AND is_active IS TRUE
  AND is_deleted IS FALSE
ORDER BY id
FOR UPDATE
"#;

const POSTGRES_DELETE_USER_IF_WALLET_ABSENT_SQL: &str = r#"
DELETE FROM users
WHERE id = $1
  AND NOT EXISTS (
    SELECT 1
    FROM wallets AS wallet
    WHERE wallet.user_id = $2
       OR EXISTS (
         SELECT 1
         FROM api_keys AS api_key
         WHERE api_key.id = wallet.api_key_id
           AND api_key.user_id = $3
       )
  )
"#;

const POSTGRES_DELETE_USER_API_KEYS_SQL: &str = "DELETE FROM api_keys WHERE user_id = $1";

const POSTGRES_DELETE_USER_DEPENDENTS_SQL: &[&str] = &[
    "DELETE FROM usage_request_admissions WHERE subject_id = $1",
    "DELETE FROM usage_cost_reservations WHERE subject_id = $1",
    "DELETE FROM gemini_file_mappings WHERE user_id = $1",
    "DELETE FROM api_key_provider_mappings WHERE api_key_id IN (SELECT id FROM api_keys WHERE user_id = $1)",
    POSTGRES_DELETE_USER_API_KEYS_SQL,
    "DELETE FROM management_tokens WHERE user_id = $1",
    "DELETE FROM user_sessions WHERE user_id = $1",
    "DELETE FROM user_oauth_links WHERE user_id = $1",
    "DELETE FROM user_group_members WHERE user_id = $1",
    "DELETE FROM user_preferences WHERE user_id = $1",
    "DELETE FROM user_invite_codes WHERE user_id = $1",
    "DELETE FROM announcement_reads WHERE user_id = $1",
];

const POSTGRES_PREPARE_USER_FACTS_FOR_DELETION_SQL: &[&str] = &[
    "UPDATE referral_rewards SET status = CASE WHEN status IN ('pending', 'failed', 'applying') THEN 'voided' ELSE status END, failure_reason = NULL, admin_note = NULL, updated_at = NOW() WHERE $1 IN (inviter_user_id, invitee_user_id)",
    "UPDATE referral_rewards SET failure_reason = NULL, admin_note = NULL, updated_at = NOW() WHERE admin_operator_id = $1",
    "UPDATE user_referrals SET invite_code_snapshot = 'deleted-user', source_json = NULL, updated_at = NOW() WHERE $1 IN (inviter_user_id, invitee_user_id)",
    "UPDATE user_plan_entitlements SET status = CASE WHEN status = 'active' THEN 'revoked' ELSE status END, expires_at = LEAST(expires_at, NOW()), updated_at = NOW() WHERE user_id = $1",
    "UPDATE wallets SET status = 'disabled', updated_at = NOW() WHERE user_id = $1",
    "UPDATE wallets SET status = 'disabled', updated_at = NOW() WHERE api_key_id IN (SELECT id FROM api_keys WHERE user_id = $1)",
    "UPDATE audit_logs SET description = 'deleted user event', ip_address = NULL, user_agent = NULL, event_metadata = NULL, error_message = NULL WHERE user_id = $1",
    "UPDATE audit_logs SET description = 'deleted API key event', ip_address = NULL, user_agent = NULL, event_metadata = NULL, error_message = NULL WHERE api_key_id IN (SELECT id FROM api_keys WHERE user_id = $1)",
    "UPDATE wallet_transactions SET description = NULL WHERE wallet_id IN (SELECT id FROM wallets WHERE user_id = $1)",
    "UPDATE wallet_transactions SET description = NULL WHERE wallet_id IN (SELECT id FROM wallets WHERE api_key_id IN (SELECT id FROM api_keys WHERE user_id = $1))",
    "UPDATE wallet_transactions SET description = NULL WHERE operator_id = $1",
    "UPDATE payment_callbacks SET payload = NULL, error_message = NULL WHERE EXISTS (SELECT 1 FROM payment_orders AS history_order WHERE history_order.user_id = $1 AND (history_order.id = payment_callbacks.payment_order_id OR (payment_callbacks.order_no IS NOT NULL AND history_order.order_no = payment_callbacks.order_no)))",
    "UPDATE payment_callbacks SET payload = NULL, error_message = NULL WHERE EXISTS (SELECT 1 FROM payment_orders AS history_order JOIN wallets AS history_wallet ON history_wallet.id = history_order.wallet_id WHERE history_wallet.user_id = $1 AND (history_order.id = payment_callbacks.payment_order_id OR (payment_callbacks.order_no IS NOT NULL AND history_order.order_no = payment_callbacks.order_no)))",
    "UPDATE payment_callbacks SET payload = NULL, error_message = NULL WHERE EXISTS (SELECT 1 FROM payment_orders AS history_order JOIN wallets AS history_wallet ON history_wallet.id = history_order.wallet_id WHERE history_wallet.api_key_id IN (SELECT id FROM api_keys WHERE user_id = $1) AND (history_order.id = payment_callbacks.payment_order_id OR (payment_callbacks.order_no IS NOT NULL AND history_order.order_no = payment_callbacks.order_no)))",
    "UPDATE payment_orders SET gateway_response = NULL WHERE user_id = $1",
    "UPDATE payment_orders SET gateway_response = NULL WHERE wallet_id IN (SELECT id FROM wallets WHERE api_key_id IN (SELECT id FROM api_keys WHERE user_id = $1))",
    "UPDATE refund_requests SET reason = NULL, payout_reference = NULL, payout_proof = NULL, failure_reason = NULL WHERE user_id = $1",
    "UPDATE refund_requests SET reason = NULL, payout_reference = NULL, payout_proof = NULL, failure_reason = NULL WHERE $1 IN (requested_by, approved_by, processed_by)",
    "UPDATE refund_requests SET reason = NULL, payout_reference = NULL, payout_proof = NULL, failure_reason = NULL WHERE wallet_id IN (SELECT id FROM wallets WHERE user_id = $1)",
    "UPDATE refund_requests SET reason = NULL, payout_reference = NULL, payout_proof = NULL, failure_reason = NULL WHERE wallet_id IN (SELECT id FROM wallets WHERE api_key_id IN (SELECT id FROM api_keys WHERE user_id = $1))",
    "UPDATE redeem_code_batches SET description = NULL WHERE created_by = $1",
];

const POSTGRES_ANONYMIZE_USER_HISTORY_SQL: &[&str] = &[
    "UPDATE request_candidates SET username = NULL, api_key_name = NULL WHERE user_id = $1",
    "UPDATE video_tasks SET username = NULL, api_key_name = NULL WHERE user_id = $1",
    "UPDATE usage SET username = NULL, api_key_name = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_summary SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily_model SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily_provider SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily_api_format SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily_model_provider SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily_cost_savings SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily_cost_savings_provider SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily_cost_savings_model SET username = NULL WHERE user_id = $1",
    "UPDATE stats_user_daily_cost_savings_model_provider SET username = NULL WHERE user_id = $1",
];

const POSTGRES_ANONYMIZE_USER_API_KEY_HISTORY_SQL: &str =
    "UPDATE stats_daily_api_key SET api_key_name = NULL WHERE api_key_id IN (SELECT id FROM api_keys WHERE user_id = $1)";

const LIST_USERS_BY_USERNAME_SEARCH_SQL: &str = r#"
SELECT
  id,
  username,
  email,
  role::text AS role,
  is_active,
  is_deleted
FROM users
WHERE is_deleted IS FALSE
  AND LOWER(username) LIKE $1
ORDER BY id ASC
"#;

const LIST_NON_ADMIN_EXPORT_USERS_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  rate_limit,
  rate_limit_mode,
  model_capability_settings,
  feature_settings,
  is_active
FROM users
WHERE is_deleted IS FALSE
  AND role::text != 'admin'
ORDER BY id ASC
"#;

const LIST_EXPORT_USERS_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  rate_limit,
  rate_limit_mode,
  model_capability_settings,
  feature_settings,
  is_active
FROM users
WHERE is_deleted IS FALSE
ORDER BY id ASC
"#;

const LIST_EXPORT_USERS_PAGE_PREFIX: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  rate_limit,
  rate_limit_mode,
  model_capability_settings,
  feature_settings,
  is_active
FROM users
WHERE is_deleted IS FALSE
"#;

const SUMMARIZE_EXPORT_USERS_SQL: &str = r#"
SELECT
  COUNT(*)::BIGINT AS total,
  COUNT(*) FILTER (WHERE is_active = TRUE)::BIGINT AS active
FROM users
WHERE is_deleted IS FALSE
"#;

const COUNT_ACTIVE_ADMIN_USERS_SQL: &str = r#"
SELECT COUNT(*)::BIGINT AS total
FROM users
WHERE role = 'admin'::userrole
  AND is_deleted IS FALSE
  AND is_active IS TRUE
"#;

const LIST_ACTIVE_LOCAL_ADMIN_PASSWORD_HASHES_SQL: &str = r#"
SELECT password_hash
FROM users
WHERE role = 'admin'::userrole
  AND auth_source = 'local'::authsource
  AND is_deleted IS FALSE
  AND is_active IS TRUE
  AND password_hash IS NOT NULL
"#;

const FIND_EXPORT_USER_BY_ID_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  rate_limit,
  rate_limit_mode,
  model_capability_settings,
  feature_settings,
  is_active
FROM users
WHERE is_deleted IS FALSE
  AND id = $1
LIMIT 1
"#;

const FIND_USER_AUTH_BY_ID_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  is_active,
  is_deleted,
  security_version,
  created_at,
  last_login_at
FROM users
WHERE id = $1
LIMIT 1
"#;

const LIST_USER_AUTH_BY_IDS_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  is_active,
  is_deleted,
  security_version,
  created_at,
  last_login_at
FROM users
WHERE id = ANY($1::text[])
ORDER BY id ASC
"#;

const FIND_USER_AUTH_BY_IDENTIFIER_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  is_active,
  is_deleted,
  security_version,
  created_at,
  last_login_at
FROM users
WHERE email = $1 OR username = $1
LIMIT 1
"#;

const FIND_USER_AUTH_BY_EMAIL_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  is_active,
  is_deleted,
  security_version,
  created_at,
  last_login_at
FROM users
WHERE email = $1
LIMIT 1
"#;

const FIND_ACTIVE_USER_AUTH_BY_EMAIL_CI_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  is_active,
  is_deleted,
  security_version,
  created_at,
  last_login_at
FROM users
WHERE LOWER(email) = LOWER($1)
  AND is_deleted IS FALSE
LIMIT 1
"#;

const FIND_USER_AUTH_BY_USERNAME_SQL: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role::text AS role,
  auth_source::text AS auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  is_active,
  is_deleted,
  security_version,
  created_at,
  last_login_at
FROM users
WHERE username = $1
LIMIT 1
"#;

const LIST_USER_OAUTH_LINKS_SQL: &str = r#"
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
  users.allowed_providers_mode,
  users.allowed_api_formats,
  users.allowed_api_formats_mode,
  users.allowed_models,
  users.allowed_models_mode,
  users.is_active,
  users.is_deleted,
  users.security_version,
  users.created_at,
  users.last_login_at
FROM user_oauth_links
JOIN users ON users.id = user_oauth_links.user_id
WHERE user_oauth_links.provider_type = $1
  AND user_oauth_links.provider_user_id = $2
LIMIT 1
"#;

const TOUCH_OAUTH_LINK_SQL: &str = r#"
UPDATE user_oauth_links
SET provider_username = COALESCE($3, provider_username),
    provider_email = COALESCE($4, provider_email),
    extra_data = COALESCE($5::json, extra_data),
    last_login_at = $6
WHERE provider_type = $1
  AND provider_user_id = $2
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
ON CONFLICT DO NOTHING
"#;

const TOUCH_AUTH_USER_LAST_LOGIN_SQL: &str = r#"
UPDATE users
SET
  last_login_at = $2,
  updated_at = $2
WHERE id = $1
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
    announcement_notifications,
    created_at,
    updated_at
  ) VALUES (
    $1,
    $2,
    $3,
    $4,
    $5,
    $6,
    $7,
    $8,
    $9,
    $10,
    $11,
    NOW(),
    NOW()
  )
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

const FIND_USER_SESSION_SQL: &str = r#"
SELECT
  id, user_id, security_version, client_device_id, device_label, refresh_token_hash,
  prev_refresh_token_hash, rotated_at, last_seen_at, expires_at, revoked_at,
  revoke_reason, ip_address, user_agent, created_at, updated_at
FROM user_sessions
WHERE user_id = $1 AND id = $2
LIMIT 1
"#;

const LIST_USER_SESSIONS_SQL: &str = r#"
SELECT
  id, user_id, security_version, client_device_id, device_label, refresh_token_hash,
  prev_refresh_token_hash, rotated_at, last_seen_at, expires_at, revoked_at,
  revoke_reason, ip_address, user_agent, created_at, updated_at
FROM user_sessions
WHERE user_id = $1
  AND revoked_at IS NULL
  AND expires_at > NOW()
ORDER BY last_seen_at DESC, created_at DESC
"#;

const REVOKE_ACTIVE_DEVICE_SESSIONS_SQL: &str = r#"
UPDATE user_sessions
SET revoked_at = $3, revoke_reason = 'replaced_by_new_login', updated_at = $3
WHERE user_id = $1
  AND client_device_id = $2
  AND revoked_at IS NULL
  AND expires_at > $3
"#;

const CREATE_USER_SESSION_SQL: &str = r#"
INSERT INTO user_sessions (
  id, user_id, security_version, client_device_id, device_label, device_type,
  ip_address, user_agent, refresh_token_hash, last_seen_at, expires_at, created_at, updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
RETURNING
  id, user_id, security_version, client_device_id, device_label, refresh_token_hash,
  prev_refresh_token_hash, rotated_at, last_seen_at, expires_at, revoked_at,
  revoke_reason, ip_address, user_agent, created_at, updated_at
"#;

const TOUCH_USER_SESSION_SQL: &str = r#"
UPDATE user_sessions
SET last_seen_at = $3,
    ip_address = COALESCE($4, ip_address),
    user_agent = COALESCE($5, user_agent),
    updated_at = $3
WHERE user_id = $1 AND id = $2
"#;

const UPDATE_USER_SESSION_DEVICE_LABEL_SQL: &str = r#"
UPDATE user_sessions
SET device_label = $3, updated_at = $4
WHERE user_id = $1 AND id = $2
"#;

const ROTATE_USER_SESSION_REFRESH_SQL: &str = r#"
UPDATE user_sessions
SET prev_refresh_token_hash = $3,
    rotated_at = $4,
    refresh_token_hash = $5,
    expires_at = $6,
    last_seen_at = $4,
    ip_address = COALESCE($7, ip_address),
    user_agent = COALESCE($8, user_agent),
    updated_at = $4
WHERE user_id = $1
  AND id = $2
  AND refresh_token_hash = $3
  AND revoked_at IS NULL
  AND expires_at > $4
"#;

const REVOKE_USER_SESSION_SQL: &str = r#"
UPDATE user_sessions
SET revoked_at = $3, revoke_reason = $4, updated_at = $3
WHERE user_id = $1 AND id = $2
"#;

const REVOKE_ALL_USER_SESSIONS_SQL: &str = r#"
UPDATE user_sessions
SET revoked_at = $2, revoke_reason = $3, updated_at = $2
WHERE user_id = $1 AND revoked_at IS NULL
"#;

const USER_GROUP_COLUMNS: &str = r#"
SELECT
  id,
  name,
  normalized_name,
  description,
  priority,
  allowed_providers,
  allowed_providers_mode,
  provider_key_policies,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  rate_limit,
  rate_limit_mode,
  created_at,
  updated_at
FROM user_groups
"#;

const USER_GROUP_MEMBER_COLUMNS: &str = r#"
SELECT
  user_group_members.group_id,
  users.id AS user_id,
  users.username,
  users.email,
  users.role::text AS role,
  users.is_active,
  users.is_deleted,
  user_group_members.created_at
FROM user_group_members
JOIN users ON users.id = user_group_members.user_id
"#;

#[derive(Debug, Clone)]
pub struct SqlxUserReadRepository {
    pool: PgPool,
}

impl SqlxUserReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        collect_query_rows(
            sqlx::query(LIST_USERS_BY_IDS_SQL)
                .bind(user_ids)
                .fetch(&self.pool),
            map_user_row,
        )
        .await
    }

    pub async fn list_users_by_username_search(
        &self,
        username_search: &str,
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        let username_search = username_search.trim();
        if username_search.is_empty() {
            return Ok(Vec::new());
        }

        collect_query_rows(
            sqlx::query(LIST_USERS_BY_USERNAME_SEARCH_SQL)
                .bind(format!("%{}%", username_search.to_ascii_lowercase()))
                .fetch(&self.pool),
            map_user_row,
        )
        .await
    }

    pub async fn list_non_admin_export_users(
        &self,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        collect_query_rows(
            sqlx::query(LIST_NON_ADMIN_EXPORT_USERS_SQL).fetch(&self.pool),
            map_user_export_row,
        )
        .await
    }

    pub async fn list_export_users(&self) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        collect_query_rows(
            sqlx::query(LIST_EXPORT_USERS_SQL).fetch(&self.pool),
            map_user_export_row,
        )
        .await
    }

    pub async fn list_user_groups(&self) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(USER_GROUP_COLUMNS);
        builder.push(" ORDER BY name ASC, id ASC");
        collect_query_rows(builder.build().fetch(&self.pool), map_user_group_row).await
    }

    pub async fn find_user_group_by_id(
        &self,
        group_id: &str,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(USER_GROUP_COLUMNS);
        builder
            .push(" WHERE id = ")
            .push_bind(group_id)
            .push(" LIMIT 1");
        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_group_row).transpose()
    }

    pub async fn list_user_groups_by_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        if group_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Postgres>::new(USER_GROUP_COLUMNS);
        builder.push(" WHERE id IN (");
        {
            let mut separated = builder.separated(", ");
            for group_id in group_ids {
                separated.push_bind(group_id);
            }
        }
        builder.push(") ORDER BY name ASC, id ASC");
        collect_query_rows(builder.build().fetch(&self.pool), map_user_group_row).await
    }

    pub async fn create_user_group(
        &self,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        let id = uuid::Uuid::new_v4().to_string();
        let name = normalize_user_group_name(&record.name);
        let normalized_name = name.to_ascii_lowercase();
        let result = sqlx::query(
            r#"
INSERT INTO user_groups (
  id, name, normalized_name, description, priority,
  allowed_providers, allowed_providers_mode,
  provider_key_policies,
  allowed_api_formats, allowed_api_formats_mode,
  allowed_models, allowed_models_mode,
  rate_limit, rate_limit_mode
)
VALUES ($1, $2, $3, $4, $5, $6::json, $7, $8::json, $9::json, $10, $11::json, $12, $13, $14)
"#,
        )
        .bind(&id)
        .bind(name)
        .bind(normalized_name)
        .bind(record.description)
        .bind(record.priority)
        .bind(record.allowed_providers.map(serde_json::Value::from))
        .bind(record.allowed_providers_mode)
        .bind(
            serde_json::to_value(record.provider_key_policies)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        )
        .bind(record.allowed_api_formats.map(serde_json::Value::from))
        .bind(record.allowed_api_formats_mode)
        .bind(record.allowed_models.map(serde_json::Value::from))
        .bind(record.allowed_models_mode)
        .bind(record.rate_limit)
        .bind(record.rate_limit_mode)
        .execute(&self.pool)
        .await;
        match result {
            Ok(_) => self.find_user_group_by_id(&id).await,
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Err(
                DataLayerError::InvalidInput("duplicate user group name".to_string()),
            ),
            Err(err) => Err(err).map_postgres_err(),
        }
    }

    pub async fn update_user_group(
        &self,
        group_id: &str,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        let name = normalize_user_group_name(&record.name);
        let normalized_name = name.to_ascii_lowercase();
        let result = sqlx::query(
            r#"
UPDATE user_groups
SET name = $2,
    normalized_name = $3,
    description = $4,
    priority = $5,
    allowed_providers = $6::json,
    allowed_providers_mode = $7,
    provider_key_policies = $8::json,
    allowed_api_formats = $9::json,
    allowed_api_formats_mode = $10,
    allowed_models = $11::json,
    allowed_models_mode = $12,
    rate_limit = $13,
    rate_limit_mode = $14,
    updated_at = now()
WHERE id = $1
"#,
        )
        .bind(group_id)
        .bind(name)
        .bind(normalized_name)
        .bind(record.description)
        .bind(record.priority)
        .bind(record.allowed_providers.map(serde_json::Value::from))
        .bind(record.allowed_providers_mode)
        .bind(
            serde_json::to_value(record.provider_key_policies)
                .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?,
        )
        .bind(record.allowed_api_formats.map(serde_json::Value::from))
        .bind(record.allowed_api_formats_mode)
        .bind(record.allowed_models.map(serde_json::Value::from))
        .bind(record.allowed_models_mode)
        .bind(record.rate_limit)
        .bind(record.rate_limit_mode)
        .execute(&self.pool)
        .await;
        match result {
            Ok(result) if result.rows_affected() == 0 => Ok(None),
            Ok(_) => self.find_user_group_by_id(group_id).await,
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Err(
                DataLayerError::InvalidInput("duplicate user group name".to_string()),
            ),
            Err(err) => Err(err).map_postgres_err(),
        }
    }

    /// Restore a group under a row lock so the snapshot comparison and write
    /// cannot be separated by a concurrent administrator update.
    pub async fn restore_user_group_if_matches(
        &self,
        expected: &StoredUserGroup,
        restored: &StoredUserGroup,
    ) -> Result<bool, DataLayerError> {
        if expected.id != restored.id || expected.id.trim().is_empty() {
            return Ok(false);
        }

        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let mut builder = QueryBuilder::<Postgres>::new(USER_GROUP_COLUMNS);
        builder
            .push(" WHERE id = ")
            .push_bind(&expected.id)
            .push(" FOR UPDATE");
        let row = builder
            .build()
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
        let Some(row) = row else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        let current = map_user_group_row(&row)?;
        if &current != expected {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }

        let result = sqlx::query(
            r#"
UPDATE user_groups
SET name = $2,
    normalized_name = $3,
    description = $4,
    priority = $5,
    allowed_providers = $6::json,
    allowed_providers_mode = $7,
    allowed_api_formats = $8::json,
    allowed_api_formats_mode = $9,
    allowed_models = $10::json,
    allowed_models_mode = $11,
    rate_limit = $12,
    rate_limit_mode = $13,
    created_at = $14,
    updated_at = $15
WHERE id = $1
"#,
        )
        .bind(&restored.id)
        .bind(&restored.name)
        .bind(&restored.normalized_name)
        .bind(&restored.description)
        .bind(restored.priority)
        .bind(
            restored
                .allowed_providers
                .clone()
                .map(serde_json::Value::from),
        )
        .bind(&restored.allowed_providers_mode)
        .bind(
            restored
                .allowed_api_formats
                .clone()
                .map(serde_json::Value::from),
        )
        .bind(&restored.allowed_api_formats_mode)
        .bind(restored.allowed_models.clone().map(serde_json::Value::from))
        .bind(&restored.allowed_models_mode)
        .bind(restored.rate_limit)
        .bind(&restored.rate_limit_mode)
        .bind(restored.created_at)
        .bind(restored.updated_at)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        if result.rows_affected() != 1 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        tx.commit().await.map_postgres_err()?;
        Ok(true)
    }

    pub async fn delete_user_group(&self, group_id: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query("DELETE FROM user_groups WHERE id = $1")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_user_group_members(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(USER_GROUP_MEMBER_COLUMNS);
        builder
            .push(" WHERE user_group_members.group_id = ")
            .push_bind(group_id)
            .push(" ORDER BY users.username ASC, users.id ASC");
        collect_query_rows(builder.build().fetch(&self.pool), map_user_group_member_row).await
    }

    pub async fn replace_user_group_members(
        &self,
        group_id: &str,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        // Serialize membership replacement with the per-user CAS path by locking all affected
        // users in deterministic order before deleting or inserting membership rows.
        let mut locked_user_ids = normalized_ids(user_ids);
        let existing_user_ids = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM user_group_members WHERE group_id = $1 ORDER BY user_id",
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_postgres_err()?;
        locked_user_ids.extend(existing_user_ids);
        locked_user_ids.sort();
        locked_user_ids.dedup();
        if !locked_user_ids.is_empty() {
            sqlx::query("SELECT id FROM users WHERE id = ANY($1::text[]) ORDER BY id FOR UPDATE")
                .bind(&locked_user_ids)
                .fetch_all(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        sqlx::query("DELETE FROM user_group_members WHERE group_id = $1")
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        for user_id in normalized_ids(user_ids) {
            sqlx::query(
                "INSERT INTO user_group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT (group_id, user_id) DO NOTHING",
            )
            .bind(group_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        }
        tx.commit().await.map_postgres_err()?;
        self.list_user_group_members(group_id).await
    }

    pub async fn list_user_groups_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(USER_GROUP_COLUMNS);
        builder
            .push(" WHERE id IN (SELECT group_id FROM user_group_members WHERE user_id = ")
            .push_bind(user_id)
            .push(") ORDER BY name ASC, id ASC");
        collect_query_rows(builder.build().fetch(&self.pool), map_user_group_row).await
    }

    pub async fn list_user_group_memberships_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMembership>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
SELECT
  user_group_members.user_id,
  user_groups.id AS group_id,
  user_groups.name AS group_name,
  user_groups.priority AS group_priority,
  user_group_members.created_at
FROM user_group_members
JOIN user_groups ON user_groups.id = user_group_members.group_id
WHERE user_group_members.user_id IN (
"#,
        );
        {
            let mut separated = builder.separated(", ");
            for user_id in user_ids {
                separated.push_bind(user_id);
            }
        }
        builder.push(
            ") ORDER BY user_group_members.user_id ASC, user_groups.name ASC, user_groups.id ASC",
        );
        collect_query_rows(
            builder.build().fetch(&self.pool),
            map_user_group_membership_row,
        )
        .await
    }

    pub async fn replace_user_groups_for_user(
        &self,
        user_id: &str,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let user_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?;
        if user_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(Vec::new());
        }
        sqlx::query("DELETE FROM user_group_members WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        for group_id in normalized_ids(group_ids) {
            sqlx::query(
                "INSERT INTO user_group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT (group_id, user_id) DO NOTHING",
            )
            .bind(group_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        }
        tx.commit().await.map_postgres_err()?;
        self.list_user_groups_for_user(user_id).await
    }

    pub async fn restore_user_groups_if_matches(
        &self,
        user_id: &str,
        expected_group_ids: &[String],
        restored_group_ids: &[String],
    ) -> Result<bool, DataLayerError> {
        let expected = normalized_ids(expected_group_ids);
        let restored = normalized_ids(restored_group_ids);
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let user_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?;
        if user_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        let current = sqlx::query_scalar::<_, String>(
            "SELECT group_id FROM user_group_members WHERE user_id = $1 ORDER BY group_id ASC FOR UPDATE",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_postgres_err()?;
        if current != expected {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        if !restored.is_empty() {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM user_groups WHERE id = ANY($1::text[])")
                    .bind(&restored)
                    .fetch_one(&mut *tx)
                    .await
                    .map_postgres_err()?;
            if count != restored.len() as i64 {
                tx.rollback().await.map_postgres_err()?;
                return Ok(false);
            }
        }
        sqlx::query("DELETE FROM user_group_members WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        for group_id in restored {
            sqlx::query(
                "INSERT INTO user_group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT (group_id, user_id) DO NOTHING",
            )
            .bind(group_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        }
        tx.commit().await.map_postgres_err()?;
        Ok(true)
    }

    pub async fn add_user_to_group(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let user_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?;
        if user_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        let result = sqlx::query(
            "INSERT INTO user_group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT (group_id, user_id) DO NOTHING",
        )
        .bind(group_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        tx.commit().await.map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn list_export_users_page(
        &self,
        query: &UserExportListQuery,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(LIST_EXPORT_USERS_PAGE_PREFIX);

        if let Some(role) = query.role.as_deref() {
            builder
                .push(" AND LOWER(role::text) = ")
                .push_bind(role.trim().to_ascii_lowercase());
        }
        if let Some(is_active) = query.is_active {
            builder.push(" AND is_active = ").push_bind(is_active);
        }
        if let Some(group_id) = query
            .group_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            builder.push(" AND id IN (SELECT user_id FROM user_group_members WHERE group_id = ");
            builder.push_bind(group_id);
            builder.push(")");
        }
        if let Some(search) = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let pattern = format!("%{}%", search.to_ascii_lowercase());
            builder
                .push(" AND (LOWER(id) LIKE ")
                .push_bind(pattern.clone())
                .push(" OR LOWER(username) LIKE ")
                .push_bind(pattern.clone())
                .push(" OR LOWER(COALESCE(email, '')) LIKE ")
                .push_bind(pattern)
                .push(")");
        }

        match query.sort_by {
            UserExportSortBy::CreatedAt => {
                builder
                    .push(" ORDER BY created_at ")
                    .push(if query.sort_order.is_desc() {
                        "DESC"
                    } else {
                        "ASC"
                    })
                    .push(", id ASC");
            }
            UserExportSortBy::Id => {
                builder.push(" ORDER BY id ASC");
            }
        }

        builder
            .push(" OFFSET ")
            .push_bind(i64::try_from(query.skip).map_err(|_| {
                DataLayerError::InvalidInput(format!("invalid user export skip: {}", query.skip))
            })?)
            .push(" LIMIT ")
            .push_bind(i64::try_from(query.limit).map_err(|_| {
                DataLayerError::InvalidInput(format!("invalid user export limit: {}", query.limit))
            })?);

        let query = builder.build();
        collect_query_rows(query.fetch(&self.pool), map_user_export_row).await
    }

    pub async fn count_export_users(
        &self,
        query: &UserExportListQuery,
    ) -> Result<u64, DataLayerError> {
        let mut builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(*)::BIGINT AS total FROM users");
        builder.push(" WHERE is_deleted IS FALSE");

        if let Some(role) = query.role.as_deref() {
            builder
                .push(" AND LOWER(role::text) = ")
                .push_bind(role.trim().to_ascii_lowercase());
        }
        if let Some(is_active) = query.is_active {
            builder.push(" AND is_active = ").push_bind(is_active);
        }
        if let Some(group_id) = query
            .group_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            builder.push(" AND id IN (SELECT user_id FROM user_group_members WHERE group_id = ");
            builder.push_bind(group_id);
            builder.push(")");
        }
        if let Some(search) = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let pattern = format!("%{}%", search.to_ascii_lowercase());
            builder
                .push(" AND (LOWER(id) LIKE ")
                .push_bind(pattern.clone())
                .push(" OR LOWER(username) LIKE ")
                .push_bind(pattern.clone())
                .push(" OR LOWER(COALESCE(email, '')) LIKE ")
                .push_bind(pattern)
                .push(")");
        }

        let row = builder
            .build()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64)
    }

    pub async fn summarize_export_users(&self) -> Result<UserExportSummary, DataLayerError> {
        let row = sqlx::query(SUMMARIZE_EXPORT_USERS_SQL)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(UserExportSummary {
            total: row.try_get::<i64, _>("total").map_postgres_err()?.max(0) as u64,
            active: row.try_get::<i64, _>("active").map_postgres_err()?.max(0) as u64,
        })
    }

    pub async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserExportRow>, DataLayerError> {
        let row = sqlx::query(FIND_EXPORT_USER_BY_ID_SQL)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_export_row).transpose()
    }

    pub async fn list_user_auth_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserAuthRecord>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        collect_query_rows(
            sqlx::query(LIST_USER_AUTH_BY_IDS_SQL)
                .bind(user_ids)
                .fetch(&self.pool),
            map_user_auth_row,
        )
        .await
    }

    pub async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let row = sqlx::query(FIND_USER_AUTH_BY_ID_SQL)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    pub async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let row = sqlx::query(FIND_USER_AUTH_BY_IDENTIFIER_SQL)
            .bind(identifier)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    pub async fn find_user_auth_by_email(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let row = sqlx::query(FIND_USER_AUTH_BY_EMAIL_SQL)
            .bind(email)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    pub async fn find_active_user_auth_by_email_ci(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let row = sqlx::query(FIND_ACTIVE_USER_AUTH_BY_EMAIL_CI_SQL)
            .bind(email)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    pub async fn find_user_auth_by_username(
        &self,
        username: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let row = sqlx::query(FIND_USER_AUTH_BY_USERNAME_SQL)
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    pub async fn list_user_oauth_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserOAuthLinkSummary>, DataLayerError> {
        collect_query_rows(
            sqlx::query(LIST_USER_OAUTH_LINKS_SQL)
                .bind(user_id)
                .fetch(&self.pool),
            map_oauth_link_summary_row,
        )
        .await
    }

    pub async fn find_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let row = sqlx::query(FIND_OAUTH_LINKED_USER_SQL)
            .bind(provider_type)
            .bind(provider_user_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_auth_row).transpose()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn resolve_enabled_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        verified_email: Option<&str>,
        touched_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<ResolveOAuthLinkedUserOutcome, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let provider_enabled: Option<bool> = sqlx::query_scalar(
            "SELECT is_enabled FROM oauth_providers WHERE provider_type = $1 FOR UPDATE",
        )
        .bind(provider_type)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if provider_enabled != Some(true) {
            tx.rollback().await.map_postgres_err()?;
            return Ok(ResolveOAuthLinkedUserOutcome::ProviderUnavailable);
        }
        let row = sqlx::query(&format!(
            "{FIND_OAUTH_LINKED_USER_SQL} FOR UPDATE OF users, user_oauth_links"
        ))
        .bind(provider_type)
        .bind(provider_user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        let Some(row) = row else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(ResolveOAuthLinkedUserOutcome::NotLinked);
        };
        let mut user = map_user_auth_row(&row)?;
        sqlx::query(TOUCH_OAUTH_LINK_SQL)
            .bind(provider_type)
            .bind(provider_user_id)
            .bind(provider_username)
            .bind(provider_email)
            .bind(extra_data)
            .bind(touched_at)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        if let Some(verified_email) = verified_email {
            let result = sqlx::query(
                "UPDATE users SET email_verified = TRUE, updated_at = $3 WHERE id = $1 AND email_verified IS FALSE AND LOWER(TRIM(email)) = LOWER(TRIM($2))",
            )
            .bind(&user.id)
            .bind(verified_email)
            .bind(touched_at)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
            if result.rows_affected() == 1 {
                user.email_verified = true;
            }
        }
        tx.commit().await.map_postgres_err()?;
        Ok(ResolveOAuthLinkedUserOutcome::Linked(user))
    }

    pub async fn touch_oauth_link(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        touched_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(TOUCH_OAUTH_LINK_SQL)
            .bind(provider_type)
            .bind(provider_user_id)
            .bind(provider_username)
            .bind(provider_email)
            .bind(extra_data)
            .bind(touched_at)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn create_oauth_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let user_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"
INSERT INTO users (
  id, email, email_verified, username, password_hash, role, auth_source,
  allowed_providers_mode, allowed_api_formats_mode, allowed_models_mode, rate_limit_mode,
  is_active, is_deleted, created_at, updated_at, last_login_at
)
VALUES (
  $1, $2, $3, $4, NULL, 'user'::userrole, 'oauth'::authsource,
  'inherit', 'inherit', 'inherit', 'inherit',
  TRUE, FALSE, $5, $5, $5
)
"#,
        )
        .bind(&user_id)
        .bind(email)
        .bind(email_verified)
        .bind(username)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        self.find_user_auth_by_id(&user_id).await
    }

    pub async fn find_oauth_link_owner(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        sqlx::query_scalar(FIND_OAUTH_LINK_OWNER_SQL)
            .bind(provider_type)
            .bind(provider_user_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()
    }

    pub async fn has_user_oauth_provider_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let owner: Option<String> = sqlx::query_scalar(FIND_USER_PROVIDER_LINK_OWNER_SQL)
            .bind(user_id)
            .bind(provider_type)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(owner.is_some())
    }

    pub async fn count_user_oauth_links(&self, user_id: &str) -> Result<u64, DataLayerError> {
        let row = sqlx::query(COUNT_USER_OAUTH_LINKS_SQL)
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(row
            .try_get::<i64, _>("link_count")
            .map_postgres_err()?
            .max(0) as u64)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn bind_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        linked_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<BindUserOAuthLinkOutcome, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let provider_enabled = sqlx::query_scalar::<_, bool>(
            "SELECT is_enabled FROM oauth_providers WHERE provider_type = $1 FOR UPDATE",
        )
        .bind(provider_type)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        let Some(provider_enabled) = provider_enabled else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(BindUserOAuthLinkOutcome::ProviderNotFound);
        };
        if !provider_enabled {
            tx.rollback().await.map_postgres_err()?;
            return Ok(BindUserOAuthLinkOutcome::ProviderDisabled);
        }
        let user_exists =
            sqlx::query_scalar::<_, i32>("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?
                .is_some();
        if !user_exists {
            tx.rollback().await.map_postgres_err()?;
            return Ok(BindUserOAuthLinkOutcome::UserNotFound);
        }
        if let Some(owner) = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM user_oauth_links WHERE provider_type = $1 AND provider_user_id = $2 LIMIT 1",
        )
        .bind(provider_type)
        .bind(provider_user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?
        {
            tx.rollback().await.map_postgres_err()?;
            return Ok(if owner == user_id {
                BindUserOAuthLinkOutcome::IdentityAlreadyBoundToUser
            } else {
                BindUserOAuthLinkOutcome::IdentityBoundToAnotherUser
            });
        }
        if sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM user_oauth_links WHERE user_id = $1 AND provider_type = $2 LIMIT 1",
        )
        .bind(user_id)
        .bind(provider_type)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?
        .is_some()
        {
            tx.rollback().await.map_postgres_err()?;
            return Ok(BindUserOAuthLinkOutcome::UserAlreadyLinkedProvider);
        }
        let inserted = sqlx::query(UPSERT_OAUTH_LINK_SQL)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(user_id)
            .bind(provider_type)
            .bind(provider_user_id)
            .bind(provider_username)
            .bind(provider_email)
            .bind(extra_data)
            .bind(linked_at)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        let outcome = if inserted.rows_affected() == 1 {
            BindUserOAuthLinkOutcome::Bound
        } else if let Some(owner) = sqlx::query_scalar::<_, String>(
            "SELECT user_id FROM user_oauth_links WHERE provider_type = $1 AND provider_user_id = $2 LIMIT 1",
        )
        .bind(provider_type)
        .bind(provider_user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?
        {
            if owner == user_id {
                BindUserOAuthLinkOutcome::IdentityAlreadyBoundToUser
            } else {
                BindUserOAuthLinkOutcome::IdentityBoundToAnotherUser
            }
        } else {
            BindUserOAuthLinkOutcome::UserAlreadyLinkedProvider
        };
        tx.commit().await.map_postgres_err()?;
        Ok(outcome)
    }

    pub async fn upgrade_oauth_email_verification_if_matches(
        &self,
        user_id: &str,
        verified_email: &str,
        verified_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE users
SET email_verified = TRUE,
    updated_at = $3
WHERE id = $1
  AND email_verified IS FALSE
  AND LOWER(TRIM(email)) = LOWER(TRIM($2))
"#,
        )
        .bind(user_id)
        .bind(verified_email)
        .bind(verified_at)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn delete_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        local_password_login_allowed: bool,
    ) -> Result<DeleteUserOAuthLinkOutcome, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let provider_exists: Option<String> = sqlx::query_scalar(
            "SELECT provider_type FROM oauth_providers WHERE provider_type = $1 FOR UPDATE",
        )
        .bind(provider_type)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if provider_exists.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(DeleteUserOAuthLinkOutcome::NotFound);
        }
        let user = sqlx::query(
            "SELECT auth_source::text AS auth_source, password_hash FROM users WHERE id = $1 FOR UPDATE",
        )
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
        let Some(user) = user else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(DeleteUserOAuthLinkOutcome::NotFound);
        };
        let auth_source = user
            .try_get::<String, _>("auth_source")
            .map_postgres_err()?;
        let password_hash = user
            .try_get::<Option<String>, _>("password_hash")
            .map_postgres_err()?;
        let provider_types = sqlx::query_scalar::<_, String>(
            "SELECT provider_type FROM user_oauth_links WHERE user_id = $1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_postgres_err()?;
        if !provider_types.iter().any(|value| value == provider_type) {
            tx.rollback().await.map_postgres_err()?;
            return Ok(DeleteUserOAuthLinkOutcome::NotFound);
        }
        let enabled_provider_types = sqlx::query_scalar::<_, String>(
            r#"
SELECT user_oauth_links.provider_type
FROM user_oauth_links
JOIN oauth_providers
  ON oauth_providers.provider_type = user_oauth_links.provider_type
WHERE user_oauth_links.user_id = $1
  AND oauth_providers.is_enabled IS TRUE
FOR UPDATE OF user_oauth_links
"#,
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await
        .map_postgres_err()?;
        let has_remaining_enabled_oauth_link = enabled_provider_types
            .iter()
            .any(|value| value != provider_type);
        if !has_remaining_enabled_oauth_link {
            if let Some(outcome) = last_oauth_unbind_denial(
                &auth_source,
                password_hash.as_deref(),
                local_password_login_allowed,
            ) {
                tx.rollback().await.map_postgres_err()?;
                return Ok(outcome);
            }
        }
        let result = sqlx::query(DELETE_USER_OAUTH_LINK_SQL)
            .bind(user_id)
            .bind(provider_type)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        if result.rows_affected() != 1 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(DeleteUserOAuthLinkOutcome::NotFound);
        }
        tx.commit().await.map_postgres_err()?;
        Ok(DeleteUserOAuthLinkOutcome::Deleted)
    }

    pub async fn get_or_create_ldap_auth_user(
        &self,
        email: String,
        username: String,
        ldap_dn: Option<String>,
        ldap_username: Option<String>,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<LdapAuthUserProvisioningOutcome>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let existing = find_postgres_ldap_user_for_update(
            &mut tx,
            ldap_dn.as_deref(),
            ldap_username.as_deref(),
            &email,
        )
        .await?;
        if let Some(existing) = existing {
            if existing.is_deleted
                || !existing.is_active
                || !existing.auth_source.eq_ignore_ascii_case("ldap")
            {
                tx.commit().await.map_err(crate::error::postgres_error)?;
                return Ok(None);
            }
            if existing.email.as_deref() != Some(email.as_str()) {
                let taken =
                    sqlx::query("SELECT 1 FROM users WHERE email = $1 AND id <> $2 LIMIT 1")
                        .bind(&email)
                        .bind(&existing.id)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_postgres_err()?;
                if taken.is_some() {
                    tx.commit().await.map_err(crate::error::postgres_error)?;
                    return Ok(None);
                }
            }
            let row = sqlx::query(
                r#"
UPDATE users
SET email = $2,
    email_verified = TRUE,
    ldap_dn = COALESCE($3, ldap_dn),
    ldap_username = COALESCE($4, ldap_username),
    last_login_at = $5,
    updated_at = $5
WHERE id = $1
RETURNING
  id, email, email_verified, username, password_hash, role::text AS role,
  auth_source::text AS auth_source, allowed_providers, allowed_providers_mode,
  allowed_api_formats, allowed_api_formats_mode, allowed_models, allowed_models_mode,
  is_active, is_deleted, security_version, created_at, last_login_at
"#,
            )
            .bind(&existing.id)
            .bind(&email)
            .bind(ldap_dn.as_deref())
            .bind(ldap_username.as_deref())
            .bind(logged_in_at)
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
            tx.commit().await.map_err(crate::error::postgres_error)?;
            return Ok(Some(LdapAuthUserProvisioningOutcome {
                user: map_user_auth_row(&row)?,
                created: false,
            }));
        }

        let base_username = ldap_username
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(username.as_str())
            .trim()
            .to_string();
        let mut candidate_username = base_username.clone();
        for _attempt in 0..3 {
            let taken = sqlx::query("SELECT 1 FROM users WHERE username = $1 LIMIT 1")
                .bind(&candidate_username)
                .fetch_optional(&mut *tx)
                .await
                .map_postgres_err()?;
            if taken.is_some() {
                let suffix = uuid::Uuid::new_v4().simple().to_string();
                candidate_username = format!(
                    "{}_ldap_{}{}",
                    base_username,
                    logged_in_at.timestamp(),
                    &suffix[..4]
                );
                continue;
            }
            let row = sqlx::query(
                r#"
INSERT INTO users (
  id, email, email_verified, username, password_hash, role, auth_source,
  ldap_dn, ldap_username, is_active, is_deleted, created_at, updated_at, last_login_at
)
VALUES ($1, $2, TRUE, $3, NULL, 'user'::userrole, 'ldap'::authsource, $4, $5, TRUE, FALSE, $6, $6, $6)
RETURNING
  id, email, email_verified, username, password_hash, role::text AS role,
  auth_source::text AS auth_source, allowed_providers, allowed_providers_mode,
  allowed_api_formats, allowed_api_formats_mode, allowed_models, allowed_models_mode,
  is_active, is_deleted, security_version, created_at, last_login_at
"#,
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(&email)
            .bind(&candidate_username)
            .bind(ldap_dn.as_deref())
            .bind(ldap_username.as_deref())
            .bind(logged_in_at)
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
            tx.commit().await.map_err(crate::error::postgres_error)?;
            return Ok(Some(LdapAuthUserProvisioningOutcome {
                user: map_user_auth_row(&row)?,
                created: true,
            }));
        }
        tx.commit().await.map_err(crate::error::postgres_error)?;
        Ok(None)
    }

    pub async fn count_active_admin_users(&self) -> Result<u64, DataLayerError> {
        let total: i64 = sqlx::query_scalar(COUNT_ACTIVE_ADMIN_USERS_SQL)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(total.max(0) as u64)
    }

    pub async fn touch_auth_user_last_login(
        &self,
        user_id: &str,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(TOUCH_AUTH_USER_LAST_LOGIN_SQL)
            .bind(user_id)
            .bind(logged_in_at)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email_present: bool,
        email: Option<String>,
        email_verified: Option<bool>,
        username: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE users
SET email = CASE WHEN $2 THEN $3 ELSE email END,
    email_verified = COALESCE($4, email_verified),
    username = COALESCE($5, username),
    updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(user_id)
        .bind(email_present)
        .bind(email)
        .bind(email_verified)
        .bind(username)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find_user_auth_by_id(user_id).await
    }

    // This operation compares and restores four correlated snapshots in one
    // transaction. Keep the explicit arguments visible at the call site so a
    // future restore cannot accidentally omit one consistency boundary.
    #[allow(clippy::too_many_arguments)]
    pub async fn restore_local_auth_user_state_if_matches(
        &self,
        expected_auth: &StoredUserAuthRecord,
        restored_auth: &StoredUserAuthRecord,
        expected_export: &StoredUserExportRow,
        restored_export: &StoredUserExportRow,
        expected_model_capability_settings: Option<&serde_json::Value>,
        restored_model_capability_settings: Option<serde_json::Value>,
        expected_feature_settings: Option<&serde_json::Value>,
        restored_feature_settings: Option<serde_json::Value>,
    ) -> Result<bool, DataLayerError> {
        if expected_auth.id != restored_auth.id
            || expected_export.id != expected_auth.id
            || restored_export.id != restored_auth.id
        {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let active_admin_ids = sqlx::query_scalar::<_, String>(POSTGRES_LOCK_ACTIVE_ADMINS_SQL)
            .fetch_all(&mut *tx)
            .await
            .map_postgres_err()?;
        let auth_row = sqlx::query(&format!("{FIND_USER_AUTH_BY_ID_SQL} FOR UPDATE"))
            .bind(&expected_auth.id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
        let export_row = sqlx::query(&format!("{FIND_EXPORT_USER_BY_ID_SQL} FOR UPDATE"))
            .bind(&expected_auth.id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
        let (Some(auth_row), Some(export_row)) = (auth_row, export_row) else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        let current_auth = map_user_auth_row(&auth_row)?;
        let current_export = map_user_export_row(&export_row)?;
        if !current_auth.matches_restore_state(expected_auth)
            || !current_export.matches_restore_state(expected_export)
            || current_export.rate_limit != expected_export.rate_limit
            || current_export.rate_limit_mode != expected_export.rate_limit_mode
            || current_export.model_capability_settings.as_ref()
                != expected_model_capability_settings
            || current_export.feature_settings.as_ref() != expected_feature_settings
        {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        let removes_active_admin = current_auth.role.eq_ignore_ascii_case("admin")
            && current_auth.is_active
            && !current_auth.is_deleted
            && (!restored_auth.role.eq_ignore_ascii_case("admin") || !restored_auth.is_active);
        if removes_active_admin && active_admin_ids.len() <= 1 {
            tx.rollback().await.map_postgres_err()?;
            return Err(DataLayerError::InvalidInput(
                LAST_ACTIVE_ADMIN_UPDATE_DENIED.to_string(),
            ));
        }
        let security_state_changed = expected_auth.role != restored_auth.role
            || expected_auth.is_active != restored_auth.is_active;
        let allowed_providers = restored_auth
            .allowed_providers
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_api_formats = restored_auth
            .allowed_api_formats
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let allowed_models = restored_auth
            .allowed_models
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| DataLayerError::UnexpectedValue(err.to_string()))?;
        let result = sqlx::query(
            r#"
UPDATE users
SET email = $2,
    email_verified = $3,
    username = $4,
    role = $5::userrole,
    allowed_providers = $6::json,
    allowed_providers_mode = $7,
    allowed_api_formats = $8::json,
    allowed_api_formats_mode = $9,
    allowed_models = $10::json,
    allowed_models_mode = $11,
    rate_limit = $12,
    rate_limit_mode = $13,
    model_capability_settings = $14::json,
    feature_settings = $15::jsonb,
    is_active = $16,
    security_version = security_version + CASE WHEN $17 THEN 1 ELSE 0 END,
    updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(&expected_auth.id)
        .bind(&restored_auth.email)
        .bind(restored_auth.email_verified)
        .bind(&restored_auth.username)
        .bind(&restored_auth.role)
        .bind(allowed_providers)
        .bind(&restored_auth.allowed_providers_mode)
        .bind(allowed_api_formats)
        .bind(&restored_auth.allowed_api_formats_mode)
        .bind(allowed_models)
        .bind(&restored_auth.allowed_models_mode)
        .bind(restored_export.rate_limit)
        .bind(&restored_export.rate_limit_mode)
        .bind(restored_model_capability_settings.clone())
        .bind(restored_feature_settings.clone())
        .bind(restored_auth.is_active)
        .bind(security_state_changed)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        if result.rows_affected() != 1 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        if security_state_changed {
            sqlx::query(
                "UPDATE user_sessions SET revoked_at = NOW(), revoke_reason = 'user_security_state_changed', updated_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL",
            )
            .bind(&expected_auth.id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
            sqlx::query(
                "UPDATE api_keys SET is_active = FALSE, updated_at = NOW() WHERE user_id = $1 AND is_active IS TRUE",
            )
            .bind(&expected_auth.id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
            sqlx::query(
                "UPDATE management_tokens SET is_active = FALSE, updated_at = NOW() WHERE user_id = $1 AND is_active IS TRUE",
            )
            .bind(&expected_auth.id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        }
        tx.commit().await.map_postgres_err()?;
        Ok(true)
    }

    pub async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE users
SET password_hash = $2,
    security_version = security_version + 1,
    updated_at = $3
WHERE id = $1
"#,
        )
        .bind(user_id)
        .bind(password_hash)
        .bind(updated_at)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find_user_auth_by_id(user_id).await
    }

    pub async fn restore_local_auth_user_password_hash_if_matches(
        &self,
        user_id: &str,
        expected_password_hash: Option<&str>,
        password_hash: Option<String>,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE users
SET password_hash = $2,
    security_version = security_version + 1,
    updated_at = $3
WHERE id = $1
  AND (($4::TEXT IS NULL AND password_hash IS NULL) OR password_hash = $4)
"#,
        )
        .bind(user_id)
        .bind(password_hash)
        .bind(updated_at)
        .bind(expected_password_hash)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn reset_local_auth_user_password_and_revoke_sessions(
        &self,
        user_id: &str,
        password_hash: String,
        changed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let updated = sqlx::query(
            "UPDATE users SET password_hash = $2, security_version = security_version + 1, updated_at = $3 WHERE id = $1 AND is_deleted IS FALSE",
        )
        .bind(user_id)
        .bind(password_hash)
        .bind(changed_at)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        if updated.rows_affected() != 1 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE user_sessions SET revoked_at = $2, revoke_reason = 'admin_password_reset', updated_at = $2 WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(changed_at)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        tx.commit().await.map_postgres_err()?;
        Ok(true)
    }

    pub async fn change_local_auth_password_and_revoke_sessions(
        &self,
        user_id: &str,
        current_session_id: &str,
        expected_password_hash: Option<&str>,
        next_password_hash: String,
        changed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let row = sqlx::query(
            r#"
SELECT password_hash, is_active, is_deleted
FROM users
WHERE id = $1
FOR UPDATE
"#,
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        let Some(row) = row else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        let stored_password_hash = row
            .try_get::<Option<String>, _>("password_hash")
            .map_postgres_err()?;
        let is_active = row.try_get::<bool, _>("is_active").map_postgres_err()?;
        let is_deleted = row.try_get::<bool, _>("is_deleted").map_postgres_err()?;
        if stored_password_hash.as_deref() != expected_password_hash || !is_active || is_deleted {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        let current_session_exists = sqlx::query_scalar::<_, bool>(
            r#"
SELECT EXISTS (
  SELECT 1 FROM user_sessions
  WHERE user_id = $1 AND id = $2 AND revoked_at IS NULL AND expires_at > $3
)
"#,
        )
        .bind(user_id)
        .bind(current_session_id)
        .bind(changed_at)
        .fetch_one(&mut *tx)
        .await
        .map_postgres_err()?;
        if !current_session_exists {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        sqlx::query(
            "UPDATE users SET password_hash = $2, security_version = security_version + 1, updated_at = $3 WHERE id = $1",
        )
            .bind(user_id)
            .bind(next_password_hash)
            .bind(changed_at)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        let revoked = sqlx::query(
            "UPDATE user_sessions SET revoked_at = $2, revoke_reason = 'password_changed', updated_at = $2 WHERE user_id = $1 AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(changed_at)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        if revoked.rows_affected() == 0 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        tx.commit().await.map_postgres_err()?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_local_auth_user_admin_fields(
        &self,
        user_id: &str,
        role: Option<String>,
        allowed_providers_present: bool,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats_present: bool,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models_present: bool,
        allowed_models: Option<Vec<String>>,
        rate_limit_present: bool,
        rate_limit: Option<i32>,
        is_active: Option<bool>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let active_admin_ids = sqlx::query_scalar::<_, String>(POSTGRES_LOCK_ACTIVE_ADMINS_SQL)
            .fetch_all(&mut *tx)
            .await
            .map_postgres_err()?;
        let current_security_state = sqlx::query(
            "SELECT role::text AS role, is_active, is_deleted FROM users WHERE id = $1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        let Some(current_security_state) = current_security_state else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        };
        let current_role = current_security_state
            .try_get::<String, _>("role")
            .map_postgres_err()?;
        let current_active = current_security_state
            .try_get::<bool, _>("is_active")
            .map_postgres_err()?;
        let current_deleted = current_security_state
            .try_get::<bool, _>("is_deleted")
            .map_postgres_err()?;
        let next_role = role.as_deref().unwrap_or(current_role.as_str());
        let next_active = is_active.unwrap_or(current_active);
        if current_role.eq_ignore_ascii_case("admin")
            && current_active
            && !current_deleted
            && (!next_role.eq_ignore_ascii_case("admin") || !next_active)
            && active_admin_ids.len() <= 1
        {
            tx.rollback().await.map_postgres_err()?;
            return Err(DataLayerError::InvalidInput(
                LAST_ACTIVE_ADMIN_UPDATE_DENIED.to_string(),
            ));
        }
        let security_state_changed =
            !current_role.eq_ignore_ascii_case(next_role) || current_active != next_active;
        let allowed_providers_mode = if allowed_providers
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        {
            "specific"
        } else {
            "unrestricted"
        };
        let allowed_api_formats_mode = if allowed_api_formats
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        {
            "specific"
        } else {
            "unrestricted"
        };
        let allowed_models_mode = if allowed_models
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        {
            "specific"
        } else {
            "unrestricted"
        };
        let rate_limit_mode = if rate_limit.is_some() {
            "custom"
        } else {
            "system"
        };
        let result = sqlx::query(
            r#"
UPDATE users
SET role = CASE
        WHEN $2::BOOLEAN AND $3 IS NOT NULL THEN $3::userrole
        ELSE role
    END,
    allowed_providers = CASE
        WHEN $4::BOOLEAN THEN $5::json
        ELSE allowed_providers
    END,
    allowed_providers_mode = CASE
        WHEN $4::BOOLEAN THEN $6
        ELSE allowed_providers_mode
    END,
    allowed_api_formats = CASE
        WHEN $7::BOOLEAN THEN $8::json
        ELSE allowed_api_formats
    END,
    allowed_api_formats_mode = CASE
        WHEN $7::BOOLEAN THEN $9
        ELSE allowed_api_formats_mode
    END,
    allowed_models = CASE
        WHEN $10::BOOLEAN THEN $11::json
        ELSE allowed_models
    END,
    allowed_models_mode = CASE
        WHEN $10::BOOLEAN THEN $12
        ELSE allowed_models_mode
    END,
    rate_limit = CASE
        WHEN $13::BOOLEAN THEN $14
        ELSE rate_limit
    END,
    rate_limit_mode = CASE
        WHEN $13::BOOLEAN THEN $15
        ELSE rate_limit_mode
    END,
    is_active = CASE
        WHEN $16::BOOLEAN AND $17 IS NOT NULL THEN $17
        ELSE is_active
    END,
    security_version = security_version + CASE WHEN $18::BOOLEAN THEN 1 ELSE 0 END,
    updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(user_id)
        .bind(role.is_some())
        .bind(role)
        .bind(allowed_providers_present)
        .bind(allowed_providers.map(serde_json::Value::from))
        .bind(allowed_providers_mode)
        .bind(allowed_api_formats_present)
        .bind(allowed_api_formats.map(serde_json::Value::from))
        .bind(allowed_api_formats_mode)
        .bind(allowed_models_present)
        .bind(allowed_models.map(serde_json::Value::from))
        .bind(allowed_models_mode)
        .bind(rate_limit_present)
        .bind(rate_limit)
        .bind(rate_limit_mode)
        .bind(is_active.is_some())
        .bind(is_active)
        .bind(security_state_changed)
        .execute(&mut *tx)
        .await
        .map_postgres_err()?;
        if result.rows_affected() == 0 {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        }
        if security_state_changed {
            sqlx::query(
                "UPDATE user_sessions SET revoked_at = NOW(), revoke_reason = 'user_security_state_changed', updated_at = NOW() WHERE user_id = $1 AND revoked_at IS NULL",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
            sqlx::query(
                "UPDATE api_keys SET is_active = FALSE, updated_at = NOW() WHERE user_id = $1 AND is_active IS TRUE",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
            sqlx::query(
                "UPDATE management_tokens SET is_active = FALSE, updated_at = NOW() WHERE user_id = $1 AND is_active IS TRUE",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        }
        tx.commit().await.map_postgres_err()?;
        self.find_user_auth_by_id(user_id).await
    }

    pub async fn update_local_auth_user_policy_modes(
        &self,
        user_id: &str,
        allowed_providers_mode: Option<String>,
        allowed_api_formats_mode: Option<String>,
        allowed_models_mode: Option<String>,
        rate_limit_mode: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE users
SET allowed_providers_mode = CASE
        WHEN $2::BOOLEAN THEN $3
        ELSE allowed_providers_mode
    END,
    allowed_api_formats_mode = CASE
        WHEN $4::BOOLEAN THEN $5
        ELSE allowed_api_formats_mode
    END,
    allowed_models_mode = CASE
        WHEN $6::BOOLEAN THEN $7
        ELSE allowed_models_mode
    END,
    rate_limit_mode = CASE
        WHEN $8::BOOLEAN THEN $9
        ELSE rate_limit_mode
    END,
    updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(user_id)
        .bind(allowed_providers_mode.is_some())
        .bind(allowed_providers_mode)
        .bind(allowed_api_formats_mode.is_some())
        .bind(allowed_api_formats_mode)
        .bind(allowed_models_mode.is_some())
        .bind(allowed_models_mode)
        .bind(rate_limit_mode.is_some())
        .bind(rate_limit_mode)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find_user_auth_by_id(user_id).await
    }

    pub async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let normalized = normalize_optional_json_value(settings);
        let result = sqlx::query(
            r#"
UPDATE users
SET model_capability_settings = $2,
    updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(user_id)
        .bind(normalized.clone())
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(normalized)
    }

    pub async fn update_user_feature_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let normalized = normalize_optional_json_value(settings);
        let result = sqlx::query(
            r#"
UPDATE users
SET feature_settings = $2,
    updated_at = NOW()
WHERE id = $1
"#,
        )
        .bind(user_id)
        .bind(normalized.clone())
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(normalized)
    }

    pub async fn create_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let user_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"
INSERT INTO users (
  id, email, email_verified, username, password_hash, role, auth_source,
  allowed_providers_mode, allowed_api_formats_mode, allowed_models_mode, rate_limit_mode,
  is_active, is_deleted, created_at, updated_at
)
VALUES (
  $1, $2, $3, $4, $5, 'user'::userrole, 'local'::authsource,
  'inherit', 'inherit', 'inherit', 'inherit',
  TRUE, FALSE, NOW(), NOW()
)
"#,
        )
        .bind(&user_id)
        .bind(email)
        .bind(email_verified)
        .bind(username)
        .bind(password_hash)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        self.find_user_auth_by_id(&user_id).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_local_auth_user_with_settings(
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
        let user_id = uuid::Uuid::new_v4().to_string();
        let allowed_providers_mode = if allowed_providers
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        {
            "specific"
        } else {
            "unrestricted"
        };
        let allowed_api_formats_mode = if allowed_api_formats
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        {
            "specific"
        } else {
            "unrestricted"
        };
        let allowed_models_mode = if allowed_models
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        {
            "specific"
        } else {
            "unrestricted"
        };
        let rate_limit_mode = if rate_limit.is_some() {
            "custom"
        } else {
            "system"
        };
        sqlx::query(
            r#"
INSERT INTO users (
  id, email, email_verified, username, password_hash, role, auth_source,
  allowed_providers, allowed_providers_mode,
  allowed_api_formats, allowed_api_formats_mode,
  allowed_models, allowed_models_mode,
  rate_limit, rate_limit_mode,
  is_active, is_deleted, created_at, updated_at
)
VALUES (
  $1, $2, $3, $4, $5, $6::userrole, 'local'::authsource,
  $7::json, $8, $9::json, $10, $11::json, $12, $13, $14,
  TRUE, FALSE, NOW(), NOW()
)
"#,
        )
        .bind(&user_id)
        .bind(email)
        .bind(email_verified)
        .bind(username)
        .bind(password_hash)
        .bind(role)
        .bind(allowed_providers.map(serde_json::Value::from))
        .bind(allowed_providers_mode)
        .bind(allowed_api_formats.map(serde_json::Value::from))
        .bind(allowed_api_formats_mode)
        .bind(allowed_models.map(serde_json::Value::from))
        .bind(allowed_models_mode)
        .bind(rate_limit)
        .bind(rate_limit_mode)
        .execute(&self.pool)
        .await
        .map_postgres_err()?;
        self.find_user_auth_by_id(&user_id).await
    }

    async fn delete_local_auth_user_inner(
        &self,
        user_id: &str,
        require_wallet_absent: bool,
    ) -> Result<bool, DataLayerError> {
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let active_admin_ids = sqlx::query_scalar::<_, String>(POSTGRES_LOCK_ACTIVE_ADMINS_SQL)
            .fetch_all(&mut *tx)
            .await
            .map_postgres_err()?;
        let target_security_state = sqlx::query(
            "SELECT role::text AS role, is_active, is_deleted FROM users WHERE id = $1 FOR UPDATE",
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        let Some(target_security_state) = target_security_state else {
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        };
        let target_role = target_security_state
            .try_get::<String, _>("role")
            .map_postgres_err()?;
        let target_is_active = target_security_state
            .try_get::<bool, _>("is_active")
            .map_postgres_err()?;
        let target_is_deleted = target_security_state
            .try_get::<bool, _>("is_deleted")
            .map_postgres_err()?;
        if target_role.eq_ignore_ascii_case("admin")
            && target_is_active
            && !target_is_deleted
            && active_admin_ids.len() <= 1
        {
            tx.rollback().await.map_postgres_err()?;
            return Err(DataLayerError::InvalidInput(
                LAST_ACTIVE_ADMIN_DELETE_DENIED.to_string(),
            ));
        }
        if require_wallet_absent {
            let wallet_exists: Option<i32> = sqlx::query_scalar(
                r#"
SELECT 1
FROM wallets AS wallet
WHERE wallet.user_id = $1
   OR EXISTS (
     SELECT 1
     FROM api_keys AS api_key
     WHERE api_key.id = wallet.api_key_id
       AND api_key.user_id = $2
   )
LIMIT 1
                "#,
            )
            .bind(user_id)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
            if wallet_exists.is_some() {
                tx.rollback().await.map_postgres_err()?;
                return Ok(false);
            }
        }
        for sql in POSTGRES_PREPARE_USER_FACTS_FOR_DELETION_SQL {
            sqlx::query(sql)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        for sql in POSTGRES_ANONYMIZE_USER_HISTORY_SQL {
            sqlx::query(sql)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        sqlx::query(POSTGRES_ANONYMIZE_USER_API_KEY_HISTORY_SQL)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        for sql in POSTGRES_DELETE_USER_DEPENDENTS_SQL {
            if require_wallet_absent && *sql == POSTGRES_DELETE_USER_API_KEYS_SQL {
                continue;
            }
            sqlx::query(sql)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        let result = if require_wallet_absent {
            sqlx::query(POSTGRES_DELETE_USER_IF_WALLET_ABSENT_SQL)
                .bind(user_id)
                .bind(user_id)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?
        } else {
            sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?
        };
        if require_wallet_absent && result.rows_affected() == 0 {
            // A wallet may have been inserted after the initial check.  Do not
            // commit the history/credential mutations when the guarded delete
            // loses that race.
            tx.rollback().await.map_postgres_err()?;
            return Ok(false);
        }
        if require_wallet_absent {
            sqlx::query(POSTGRES_DELETE_USER_API_KEYS_SQL)
                .bind(user_id)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
        }
        tx.commit().await.map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_local_auth_user(&self, user_id: &str) -> Result<bool, DataLayerError> {
        self.delete_local_auth_user_inner(user_id, false).await
    }

    pub async fn delete_local_auth_user_if_wallet_absent(
        &self,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        self.delete_local_auth_user_inner(user_id, true).await
    }

    pub async fn read_user_preferences(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        let row = sqlx::query(READ_USER_PREFERENCES_SQL)
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_preference_row).transpose()
    }

    pub async fn write_user_preferences(
        &self,
        preferences: &StoredUserPreferenceRecord,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        let row = sqlx::query(UPSERT_USER_PREFERENCES_SQL)
            .bind(uuid::Uuid::new_v4().to_string())
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
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_preference_row).transpose()
    }

    pub async fn find_user_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let row = sqlx::query(FIND_USER_SESSION_SQL)
            .bind(user_id)
            .bind(session_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_user_session_row).transpose()
    }

    pub async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserSessionRecord>, DataLayerError> {
        collect_query_rows(
            sqlx::query(LIST_USER_SESSIONS_SQL)
                .bind(user_id)
                .fetch(&self.pool),
            map_user_session_row,
        )
        .await
    }

    pub async fn create_user_session(
        &self,
        session: &StoredUserSessionRecord,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let now = session
            .created_at
            .or(session.updated_at)
            .or(session.last_seen_at)
            .unwrap_or_else(chrono::Utc::now);
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let user_is_eligible = sqlx::query_scalar::<_, String>(
            r#"
SELECT id FROM users
WHERE id = $1 AND is_active IS TRUE AND is_deleted IS FALSE
  AND security_version = $2
FOR UPDATE
"#,
        )
        .bind(&session.user_id)
        .bind(session.security_version)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if user_is_eligible.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        }
        sqlx::query(REVOKE_ACTIVE_DEVICE_SESSIONS_SQL)
            .bind(&session.user_id)
            .bind(&session.client_device_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        let row = sqlx::query(CREATE_USER_SESSION_SQL)
            .bind(&session.id)
            .bind(&session.user_id)
            .bind(session.security_version)
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
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
        let session = map_user_session_row(&row)?;
        tx.commit().await.map_postgres_err()?;
        Ok(Some(session))
    }

    pub async fn create_user_session_if_password_matches(
        &self,
        session: &StoredUserSessionRecord,
        expected_password_hash: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let now = session
            .created_at
            .or(session.updated_at)
            .or(session.last_seen_at)
            .unwrap_or_else(chrono::Utc::now);
        let mut tx = self.pool.begin().await.map_postgres_err()?;
        let matched = sqlx::query_scalar::<_, String>(
            r#"
SELECT password_hash FROM users
WHERE id = $1 AND password_hash = $2 AND auth_source::text = 'local'
  AND is_active IS TRUE AND is_deleted IS FALSE AND security_version = $3
FOR UPDATE
"#,
        )
        .bind(&session.user_id)
        .bind(expected_password_hash)
        .bind(session.security_version)
        .fetch_optional(&mut *tx)
        .await
        .map_postgres_err()?;
        if matched.is_none() {
            tx.rollback().await.map_postgres_err()?;
            return Ok(None);
        }
        sqlx::query("UPDATE users SET last_login_at = $2 WHERE id = $1")
            .bind(&session.user_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        sqlx::query(REVOKE_ACTIVE_DEVICE_SESSIONS_SQL)
            .bind(&session.user_id)
            .bind(&session.client_device_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_postgres_err()?;
        let row = sqlx::query(CREATE_USER_SESSION_SQL)
            .bind(&session.id)
            .bind(&session.user_id)
            .bind(session.security_version)
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
            .fetch_one(&mut *tx)
            .await
            .map_postgres_err()?;
        tx.commit().await.map_postgres_err()?;
        Ok(Some(map_user_session_row(&row)?))
    }

    pub async fn touch_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        touched_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(TOUCH_USER_SESSION_SQL)
            .bind(user_id)
            .bind(session_id)
            .bind(touched_at)
            .bind(ip_address)
            .bind(user_agent.map(|value| value.chars().take(1000).collect::<String>()))
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_user_session_device_label(
        &self,
        user_id: &str,
        session_id: &str,
        device_label: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(UPDATE_USER_SESSION_DEVICE_LABEL_SQL)
            .bind(user_id)
            .bind(session_id)
            .bind(device_label.chars().take(120).collect::<String>())
            .bind(updated_at)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn rotate_user_session_refresh_token(
        &self,
        user_id: &str,
        session_id: &str,
        expected_refresh_token_hash: &str,
        next_refresh_token_hash: &str,
        rotated_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(ROTATE_USER_SESSION_REFRESH_SQL)
            .bind(user_id)
            .bind(session_id)
            .bind(expected_refresh_token_hash)
            .bind(rotated_at)
            .bind(next_refresh_token_hash)
            .bind(expires_at)
            .bind(ip_address)
            .bind(user_agent.map(|value| value.chars().take(1000).collect::<String>()))
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn revoke_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(REVOKE_USER_SESSION_SQL)
            .bind(user_id)
            .bind(session_id)
            .bind(revoked_at)
            .bind(reason.chars().take(100).collect::<String>())
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn revoke_all_user_sessions(
        &self,
        user_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<u64, DataLayerError> {
        let result = sqlx::query(REVOKE_ALL_USER_SESSIONS_SQL)
            .bind(user_id)
            .bind(revoked_at)
            .bind(reason.chars().take(100).collect::<String>())
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected())
    }

    pub async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, DataLayerError> {
        let hashes = sqlx::query_scalar::<_, String>(LIST_ACTIVE_LOCAL_ADMIN_PASSWORD_HASHES_SQL)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(hashes
            .iter()
            .filter(|hash| is_valid_bcrypt_hash(hash))
            .count() as u64)
    }
}

fn map_user_preference_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUserPreferenceRecord, DataLayerError> {
    let user_id: String = row.try_get("user_id").map_postgres_err()?;
    if user_id.trim().is_empty() {
        return Err(DataLayerError::UnexpectedValue(
            "user_preferences.user_id is empty".to_string(),
        ));
    }

    Ok(StoredUserPreferenceRecord {
        user_id,
        avatar_url: row.try_get("avatar_url").map_postgres_err()?,
        bio: row.try_get("bio").map_postgres_err()?,
        default_provider_id: row.try_get("default_provider_id").map_postgres_err()?,
        default_provider_name: row.try_get("default_provider_name").map_postgres_err()?,
        theme: row.try_get("theme").map_postgres_err()?,
        language: row.try_get("language").map_postgres_err()?,
        timezone: row.try_get("timezone").map_postgres_err()?,
        email_notifications: row.try_get("email_notifications").map_postgres_err()?,
        usage_alerts: row.try_get("usage_alerts").map_postgres_err()?,
        announcement_notifications: row
            .try_get("announcement_notifications")
            .map_postgres_err()?,
    })
}

fn map_user_session_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUserSessionRecord, DataLayerError> {
    StoredUserSessionRecord::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("user_id").map_postgres_err()?,
        row.try_get("client_device_id").map_postgres_err()?,
        row.try_get("device_label").map_postgres_err()?,
        row.try_get("refresh_token_hash").map_postgres_err()?,
        row.try_get("prev_refresh_token_hash").map_postgres_err()?,
        row.try_get("rotated_at").map_postgres_err()?,
        row.try_get("last_seen_at").map_postgres_err()?,
        row.try_get("expires_at").map_postgres_err()?,
        row.try_get("revoked_at").map_postgres_err()?,
        row.try_get("revoke_reason").map_postgres_err()?,
        row.try_get("ip_address").map_postgres_err()?,
        row.try_get("user_agent").map_postgres_err()?,
        row.try_get("created_at").map_postgres_err()?,
        row.try_get("updated_at").map_postgres_err()?,
    )
    .and_then(|record| {
        record.with_security_version(row.try_get("security_version").map_postgres_err()?)
    })
}

fn normalize_optional_json_value(value: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match value {
        Some(serde_json::Value::Null) | None => None,
        Some(value) => Some(value),
    }
}

fn normalized_ids(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

async fn find_postgres_ldap_user_for_update(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ldap_dn: Option<&str>,
    ldap_username: Option<&str>,
    email: &str,
) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
    let select_columns = r#"
SELECT
  id, email, email_verified, username, password_hash, role::text AS role,
  auth_source::text AS auth_source, allowed_providers, allowed_providers_mode,
  allowed_api_formats, allowed_api_formats_mode, allowed_models, allowed_models_mode,
  is_active, is_deleted, created_at, last_login_at
FROM users
"#;
    if let Some(ldap_dn) = ldap_dn.filter(|value| !value.trim().is_empty()) {
        let row = sqlx::query(&format!(
            "{select_columns} WHERE auth_source = 'ldap'::authsource AND ldap_dn = $1 LIMIT 1 FOR UPDATE"
        ))
        .bind(ldap_dn)
        .fetch_optional(&mut **tx)
        .await
        .map_postgres_err()?;
        if let Some(row) = row.as_ref() {
            return map_user_auth_row(row).map(Some);
        }
    }
    if let Some(ldap_username) = ldap_username.filter(|value| !value.trim().is_empty()) {
        let row = sqlx::query(&format!(
            "{select_columns} WHERE auth_source = 'ldap'::authsource AND ldap_username = $1 LIMIT 1 FOR UPDATE"
        ))
        .bind(ldap_username)
        .fetch_optional(&mut **tx)
        .await
        .map_postgres_err()?;
        if let Some(row) = row.as_ref() {
            return map_user_auth_row(row).map(Some);
        }
    }
    let row = sqlx::query(&format!(
        "{select_columns} WHERE email = $1 LIMIT 1 FOR UPDATE"
    ))
    .bind(email)
    .fetch_optional(&mut **tx)
    .await
    .map_postgres_err()?;
    row.as_ref().map(map_user_auth_row).transpose()
}

fn map_user_row(row: &sqlx::postgres::PgRow) -> Result<StoredUserSummary, DataLayerError> {
    StoredUserSummary::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("username").map_postgres_err()?,
        row.try_get("email").map_postgres_err()?,
        row.try_get("role").map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
        row.try_get("is_deleted").map_postgres_err()?,
    )
}

fn map_user_export_row(row: &sqlx::postgres::PgRow) -> Result<StoredUserExportRow, DataLayerError> {
    let feature_settings = row.try_get("feature_settings").map_postgres_err()?;
    StoredUserExportRow::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("email").map_postgres_err()?,
        row.try_get("email_verified").map_postgres_err()?,
        row.try_get("username").map_postgres_err()?,
        row.try_get("password_hash").map_postgres_err()?,
        row.try_get("role").map_postgres_err()?,
        row.try_get("auth_source").map_postgres_err()?,
        row.try_get("allowed_providers").map_postgres_err()?,
        row.try_get("allowed_api_formats").map_postgres_err()?,
        row.try_get("allowed_models").map_postgres_err()?,
        row.try_get("rate_limit").map_postgres_err()?,
        row.try_get("model_capability_settings")
            .map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
    )
    .map(|record| record.with_feature_settings(feature_settings))
    .and_then(|record| {
        record.with_policy_modes(
            row.try_get("allowed_providers_mode").map_postgres_err()?,
            row.try_get("allowed_api_formats_mode").map_postgres_err()?,
            row.try_get("allowed_models_mode").map_postgres_err()?,
            row.try_get("rate_limit_mode").map_postgres_err()?,
        )
    })
}

fn map_user_auth_row(row: &sqlx::postgres::PgRow) -> Result<StoredUserAuthRecord, DataLayerError> {
    StoredUserAuthRecord::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("email").map_postgres_err()?,
        row.try_get("email_verified").map_postgres_err()?,
        row.try_get("username").map_postgres_err()?,
        row.try_get("password_hash").map_postgres_err()?,
        row.try_get("role").map_postgres_err()?,
        row.try_get("auth_source").map_postgres_err()?,
        row.try_get("allowed_providers").map_postgres_err()?,
        row.try_get("allowed_api_formats").map_postgres_err()?,
        row.try_get("allowed_models").map_postgres_err()?,
        row.try_get("is_active").map_postgres_err()?,
        row.try_get("is_deleted").map_postgres_err()?,
        row.try_get("created_at").map_postgres_err()?,
        row.try_get("last_login_at").map_postgres_err()?,
    )
    .and_then(|record| {
        record.with_security_version(row.try_get("security_version").map_postgres_err()?)
    })
    .and_then(|record| {
        record.with_policy_modes(
            row.try_get("allowed_providers_mode").map_postgres_err()?,
            row.try_get("allowed_api_formats_mode").map_postgres_err()?,
            row.try_get("allowed_models_mode").map_postgres_err()?,
        )
    })
}

fn map_user_group_row(row: &sqlx::postgres::PgRow) -> Result<StoredUserGroup, DataLayerError> {
    StoredUserGroup::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("name").map_postgres_err()?,
        row.try_get("normalized_name").map_postgres_err()?,
        row.try_get("description").map_postgres_err()?,
        row.try_get("priority").map_postgres_err()?,
        row.try_get("allowed_providers").map_postgres_err()?,
        row.try_get("allowed_providers_mode").map_postgres_err()?,
        row.try_get("provider_key_policies").map_postgres_err()?,
        row.try_get("allowed_api_formats").map_postgres_err()?,
        row.try_get("allowed_api_formats_mode").map_postgres_err()?,
        row.try_get("allowed_models").map_postgres_err()?,
        row.try_get("allowed_models_mode").map_postgres_err()?,
        row.try_get("rate_limit").map_postgres_err()?,
        row.try_get("rate_limit_mode").map_postgres_err()?,
        row.try_get("created_at").map_postgres_err()?,
        row.try_get("updated_at").map_postgres_err()?,
    )
}

fn map_user_group_member_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUserGroupMember, DataLayerError> {
    Ok(StoredUserGroupMember {
        group_id: row.try_get("group_id").map_postgres_err()?,
        user_id: row.try_get("user_id").map_postgres_err()?,
        username: row.try_get("username").map_postgres_err()?,
        email: row.try_get("email").map_postgres_err()?,
        role: row.try_get("role").map_postgres_err()?,
        is_active: row.try_get("is_active").map_postgres_err()?,
        is_deleted: row.try_get("is_deleted").map_postgres_err()?,
        created_at: row.try_get("created_at").map_postgres_err()?,
    })
}

fn map_user_group_membership_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUserGroupMembership, DataLayerError> {
    Ok(StoredUserGroupMembership {
        user_id: row.try_get("user_id").map_postgres_err()?,
        group_id: row.try_get("group_id").map_postgres_err()?,
        group_name: row.try_get("group_name").map_postgres_err()?,
        group_priority: row.try_get("group_priority").map_postgres_err()?,
        created_at: row.try_get("created_at").map_postgres_err()?,
    })
}

fn map_oauth_link_summary_row(
    row: &sqlx::postgres::PgRow,
) -> Result<StoredUserOAuthLinkSummary, DataLayerError> {
    StoredUserOAuthLinkSummary::new(
        row.try_get("provider_type").map_postgres_err()?,
        row.try_get("display_name").map_postgres_err()?,
        row.try_get("provider_username").map_postgres_err()?,
        row.try_get("provider_email").map_postgres_err()?,
        row.try_get("linked_at").map_postgres_err()?,
        row.try_get("last_login_at").map_postgres_err()?,
        row.try_get("provider_enabled").map_postgres_err()?,
    )
}

async fn collect_query_rows<T, S>(
    mut rows: S,
    mapper: fn(&sqlx::postgres::PgRow) -> Result<T, DataLayerError>,
) -> Result<Vec<T>, DataLayerError>
where
    S: futures_util::TryStream<Ok = sqlx::postgres::PgRow, Error = sqlx::Error> + Unpin,
{
    let mut items = Vec::new();
    while let Some(row) = rows.try_next().await.map_postgres_err()? {
        items.push(mapper(&row)?);
    }
    Ok(items)
}

#[async_trait]
impl UserReadRepository for SqlxUserReadRepository {
    async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        self.list_users_by_ids(user_ids).await
    }

    async fn list_users_by_username_search(
        &self,
        username_search: &str,
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        self.list_users_by_username_search(username_search).await
    }

    async fn list_non_admin_export_users(
        &self,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        self.list_non_admin_export_users().await
    }

    async fn list_export_users(&self) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        self.list_export_users().await
    }

    async fn list_export_users_page(
        &self,
        query: &UserExportListQuery,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        self.list_export_users_page(query).await
    }

    async fn count_export_users(&self, query: &UserExportListQuery) -> Result<u64, DataLayerError> {
        self.count_export_users(query).await
    }

    async fn summarize_export_users(&self) -> Result<UserExportSummary, DataLayerError> {
        self.summarize_export_users().await
    }

    async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserExportRow>, DataLayerError> {
        self.find_export_user_by_id(user_id).await
    }

    async fn list_user_groups(&self) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        self.list_user_groups().await
    }

    async fn find_user_group_by_id(
        &self,
        group_id: &str,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        self.find_user_group_by_id(group_id).await
    }

    async fn list_user_groups_by_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        self.list_user_groups_by_ids(group_ids).await
    }

    async fn create_user_group(
        &self,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        self.create_user_group(record).await
    }

    async fn update_user_group(
        &self,
        group_id: &str,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        self.update_user_group(group_id, record).await
    }

    async fn restore_user_group_if_matches(
        &self,
        expected: &StoredUserGroup,
        restored: &StoredUserGroup,
    ) -> Result<bool, DataLayerError> {
        self.restore_user_group_if_matches(expected, restored).await
    }

    async fn delete_user_group(&self, group_id: &str) -> Result<bool, DataLayerError> {
        self.delete_user_group(group_id).await
    }

    async fn list_user_group_members(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        self.list_user_group_members(group_id).await
    }

    async fn replace_user_group_members(
        &self,
        group_id: &str,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        self.replace_user_group_members(group_id, user_ids).await
    }

    async fn list_user_groups_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        self.list_user_groups_for_user(user_id).await
    }

    async fn list_user_group_memberships_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMembership>, DataLayerError> {
        self.list_user_group_memberships_by_user_ids(user_ids).await
    }

    async fn replace_user_groups_for_user(
        &self,
        user_id: &str,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        self.replace_user_groups_for_user(user_id, group_ids).await
    }

    async fn restore_user_groups_if_matches(
        &self,
        user_id: &str,
        expected_group_ids: &[String],
        restored_group_ids: &[String],
    ) -> Result<bool, DataLayerError> {
        self.restore_user_groups_if_matches(user_id, expected_group_ids, restored_group_ids)
            .await
    }

    async fn add_user_to_group(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        self.add_user_to_group(group_id, user_id).await
    }

    async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.find_user_auth_by_id(user_id).await
    }

    async fn list_user_auth_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserAuthRecord>, DataLayerError> {
        self.list_user_auth_by_ids(user_ids).await
    }

    async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.find_user_auth_by_identifier(identifier).await
    }

    async fn find_user_auth_by_email(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.find_user_auth_by_email(email).await
    }

    async fn find_active_user_auth_by_email_ci(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.find_active_user_auth_by_email_ci(email).await
    }

    async fn find_user_auth_by_username(
        &self,
        username: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.find_user_auth_by_username(username).await
    }

    async fn list_user_oauth_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserOAuthLinkSummary>, DataLayerError> {
        self.list_user_oauth_links(user_id).await
    }

    async fn find_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.find_oauth_linked_user(provider_type, provider_user_id)
            .await
    }

    async fn resolve_enabled_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        verified_email: Option<&str>,
        touched_at: chrono::DateTime<chrono::Utc>,
        _provider_enabled_snapshot: bool,
    ) -> Result<ResolveOAuthLinkedUserOutcome, DataLayerError> {
        self.resolve_enabled_oauth_linked_user(
            provider_type,
            provider_user_id,
            provider_username,
            provider_email,
            extra_data,
            verified_email,
            touched_at,
        )
        .await
    }

    async fn touch_oauth_link(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        touched_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        self.touch_oauth_link(
            provider_type,
            provider_user_id,
            provider_username,
            provider_email,
            extra_data,
            touched_at,
        )
        .await
    }

    async fn create_oauth_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.create_oauth_auth_user(email, email_verified, username, created_at)
            .await
    }

    async fn find_oauth_link_owner(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        self.find_oauth_link_owner(provider_type, provider_user_id)
            .await
    }

    async fn has_user_oauth_provider_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        self.has_user_oauth_provider_link(user_id, provider_type)
            .await
    }

    async fn count_user_oauth_links(&self, user_id: &str) -> Result<u64, DataLayerError> {
        self.count_user_oauth_links(user_id).await
    }

    async fn has_oauth_links_for_provider(
        &self,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let exists: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM user_oauth_links WHERE provider_type = $1 LIMIT 1")
                .bind(provider_type)
                .fetch_optional(&self.pool)
                .await
                .map_postgres_err()?;
        Ok(exists.is_some())
    }

    async fn bind_user_oauth_link_if_provider_enabled(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        linked_at: chrono::DateTime<chrono::Utc>,
        _provider_enabled_snapshot: bool,
        session_expectation: Option<&BindUserOAuthLinkSessionExpectation>,
    ) -> Result<BindUserOAuthLinkOutcome, DataLayerError> {
        if let Some(expectation) = session_expectation {
            let mut tx = self.pool.begin().await.map_postgres_err()?;
            let provider_enabled = sqlx::query_scalar::<_, bool>(
                "SELECT is_enabled FROM oauth_providers WHERE provider_type = $1 FOR UPDATE",
            )
            .bind(provider_type)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
            if provider_enabled != Some(true) {
                tx.rollback().await.map_postgres_err()?;
                return Ok(if provider_enabled.is_none() {
                    BindUserOAuthLinkOutcome::ProviderNotFound
                } else {
                    BindUserOAuthLinkOutcome::ProviderDisabled
                });
            }
            let session_is_current: Option<i32> = sqlx::query_scalar(
                r#"
SELECT 1
FROM users
JOIN user_sessions ON user_sessions.user_id = users.id
WHERE users.id = $1
  AND users.is_active IS TRUE
  AND users.is_deleted IS FALSE
  AND users.security_version = $2
  AND user_sessions.id = $3
  AND user_sessions.security_version = $2
  AND user_sessions.client_device_id = $4
  AND user_sessions.revoked_at IS NULL
  AND user_sessions.expires_at > GREATEST($5, NOW())
FOR UPDATE OF users, user_sessions
"#,
            )
            .bind(user_id)
            .bind(expectation.security_version)
            .bind(&expectation.session_id)
            .bind(&expectation.client_device_id)
            .bind(expectation.checked_at)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?;
            if session_is_current.is_none() {
                tx.rollback().await.map_postgres_err()?;
                return Ok(BindUserOAuthLinkOutcome::SessionUnavailable);
            }
            if let Some(owner) = sqlx::query_scalar::<_, String>(
                "SELECT user_id FROM user_oauth_links WHERE provider_type = $1 AND provider_user_id = $2 LIMIT 1 FOR UPDATE",
            )
            .bind(provider_type)
            .bind(provider_user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()? {
                tx.rollback().await.map_postgres_err()?;
                return Ok(if owner == user_id {
                    BindUserOAuthLinkOutcome::IdentityAlreadyBoundToUser
                } else {
                    BindUserOAuthLinkOutcome::IdentityBoundToAnotherUser
                });
            }
            if sqlx::query_scalar::<_, i32>(
                "SELECT 1 FROM user_oauth_links WHERE user_id = $1 AND provider_type = $2 LIMIT 1 FOR UPDATE",
            )
            .bind(user_id)
            .bind(provider_type)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()?
            .is_some() {
                tx.rollback().await.map_postgres_err()?;
                return Ok(BindUserOAuthLinkOutcome::UserAlreadyLinkedProvider);
            }
            let inserted = sqlx::query(UPSERT_OAUTH_LINK_SQL)
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(user_id)
                .bind(provider_type)
                .bind(provider_user_id)
                .bind(provider_username)
                .bind(provider_email)
                .bind(extra_data)
                .bind(linked_at)
                .execute(&mut *tx)
                .await
                .map_postgres_err()?;
            let outcome = if inserted.rows_affected() == 1 {
                BindUserOAuthLinkOutcome::Bound
            } else if let Some(owner) = sqlx::query_scalar::<_, String>(
                "SELECT user_id FROM user_oauth_links WHERE provider_type = $1 AND provider_user_id = $2 LIMIT 1",
            )
            .bind(provider_type)
            .bind(provider_user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_postgres_err()? {
                if owner == user_id {
                    BindUserOAuthLinkOutcome::IdentityAlreadyBoundToUser
                } else {
                    BindUserOAuthLinkOutcome::IdentityBoundToAnotherUser
                }
            } else {
                BindUserOAuthLinkOutcome::UserAlreadyLinkedProvider
            };
            tx.commit().await.map_postgres_err()?;
            return Ok(outcome);
        }
        self.bind_user_oauth_link(
            user_id,
            provider_type,
            provider_user_id,
            provider_username,
            provider_email,
            extra_data,
            linked_at,
        )
        .await
    }

    async fn upgrade_oauth_email_verification_if_matches(
        &self,
        user_id: &str,
        verified_email: &str,
        verified_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        self.upgrade_oauth_email_verification_if_matches(user_id, verified_email, verified_at)
            .await
    }

    async fn delete_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        local_password_login_allowed: bool,
        _enabled_provider_types_snapshot: &[String],
    ) -> Result<DeleteUserOAuthLinkOutcome, DataLayerError> {
        self.delete_user_oauth_link(user_id, provider_type, local_password_login_allowed)
            .await
    }

    async fn get_or_create_ldap_auth_user(
        &self,
        email: String,
        username: String,
        ldap_dn: Option<String>,
        ldap_username: Option<String>,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<LdapAuthUserProvisioningOutcome>, DataLayerError> {
        self.get_or_create_ldap_auth_user(email, username, ldap_dn, ldap_username, logged_in_at)
            .await
    }

    async fn touch_auth_user_last_login(
        &self,
        user_id: &str,
        logged_in_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        self.touch_auth_user_last_login(user_id, logged_in_at).await
    }

    async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email_present: bool,
        email: Option<String>,
        email_verified: Option<bool>,
        username: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.update_local_auth_user_profile(user_id, email_present, email, email_verified, username)
            .await
    }

    async fn restore_local_auth_user_state_if_matches(
        &self,
        expected_auth: &StoredUserAuthRecord,
        restored_auth: &StoredUserAuthRecord,
        expected_export: &StoredUserExportRow,
        restored_export: &StoredUserExportRow,
        expected_model_capability_settings: Option<&serde_json::Value>,
        restored_model_capability_settings: Option<serde_json::Value>,
        expected_feature_settings: Option<&serde_json::Value>,
        restored_feature_settings: Option<serde_json::Value>,
    ) -> Result<bool, DataLayerError> {
        self.restore_local_auth_user_state_if_matches(
            expected_auth,
            restored_auth,
            expected_export,
            restored_export,
            expected_model_capability_settings,
            restored_model_capability_settings,
            expected_feature_settings,
            restored_feature_settings,
        )
        .await
    }

    async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.update_local_auth_user_password_hash(user_id, password_hash, updated_at)
            .await
    }

    async fn restore_local_auth_user_password_hash_if_matches(
        &self,
        user_id: &str,
        expected_password_hash: Option<&str>,
        password_hash: Option<String>,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        self.restore_local_auth_user_password_hash_if_matches(
            user_id,
            expected_password_hash,
            password_hash,
            updated_at,
        )
        .await
    }

    async fn reset_local_auth_user_password_and_revoke_sessions(
        &self,
        user_id: &str,
        password_hash: String,
        changed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        self.reset_local_auth_user_password_and_revoke_sessions(user_id, password_hash, changed_at)
            .await
    }

    async fn change_local_auth_password_and_revoke_sessions(
        &self,
        user_id: &str,
        current_session_id: &str,
        expected_password_hash: Option<&str>,
        next_password_hash: String,
        changed_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        self.change_local_auth_password_and_revoke_sessions(
            user_id,
            current_session_id,
            expected_password_hash,
            next_password_hash,
            changed_at,
        )
        .await
    }

    async fn update_local_auth_user_admin_fields(
        &self,
        user_id: &str,
        role: Option<String>,
        allowed_providers_present: bool,
        allowed_providers: Option<Vec<String>>,
        allowed_api_formats_present: bool,
        allowed_api_formats: Option<Vec<String>>,
        allowed_models_present: bool,
        allowed_models: Option<Vec<String>>,
        rate_limit_present: bool,
        rate_limit: Option<i32>,
        is_active: Option<bool>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.update_local_auth_user_admin_fields(
            user_id,
            role,
            allowed_providers_present,
            allowed_providers,
            allowed_api_formats_present,
            allowed_api_formats,
            allowed_models_present,
            allowed_models,
            rate_limit_present,
            rate_limit,
            is_active,
        )
        .await
    }

    async fn update_local_auth_user_policy_modes(
        &self,
        user_id: &str,
        allowed_providers_mode: Option<String>,
        allowed_api_formats_mode: Option<String>,
        allowed_models_mode: Option<String>,
        rate_limit_mode: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.update_local_auth_user_policy_modes(
            user_id,
            allowed_providers_mode,
            allowed_api_formats_mode,
            allowed_models_mode,
            rate_limit_mode,
        )
        .await
    }

    async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        self.update_user_model_capability_settings(user_id, settings)
            .await
    }

    async fn update_user_feature_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        self.update_user_feature_settings(user_id, settings).await
    }

    async fn create_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        self.create_local_auth_user(email, email_verified, username, password_hash)
            .await
    }

    async fn create_local_auth_user_with_settings(
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
        self.create_local_auth_user_with_settings(
            email,
            email_verified,
            username,
            password_hash,
            role,
            allowed_providers,
            allowed_api_formats,
            allowed_models,
            rate_limit,
        )
        .await
    }

    async fn delete_local_auth_user(&self, user_id: &str) -> Result<bool, DataLayerError> {
        self.delete_local_auth_user(user_id).await
    }

    async fn delete_local_auth_user_if_wallet_absent(
        &self,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        self.delete_local_auth_user_if_wallet_absent(user_id).await
    }

    async fn read_user_preferences(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        self.read_user_preferences(user_id).await
    }

    async fn write_user_preferences(
        &self,
        preferences: &StoredUserPreferenceRecord,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        self.write_user_preferences(preferences).await
    }

    async fn find_user_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        self.find_user_session(user_id, session_id).await
    }

    async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserSessionRecord>, DataLayerError> {
        self.list_user_sessions(user_id).await
    }

    async fn create_user_session(
        &self,
        session: &StoredUserSessionRecord,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        self.create_user_session(session).await
    }

    async fn create_user_session_if_password_matches(
        &self,
        session: &StoredUserSessionRecord,
        expected_password_hash: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        self.create_user_session_if_password_matches(session, expected_password_hash)
            .await
    }

    async fn touch_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        touched_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        self.touch_user_session(user_id, session_id, touched_at, ip_address, user_agent)
            .await
    }

    async fn update_user_session_device_label(
        &self,
        user_id: &str,
        session_id: &str,
        device_label: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, DataLayerError> {
        self.update_user_session_device_label(user_id, session_id, device_label, updated_at)
            .await
    }

    async fn rotate_user_session_refresh_token(
        &self,
        user_id: &str,
        session_id: &str,
        expected_refresh_token_hash: &str,
        next_refresh_token_hash: &str,
        rotated_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        self.rotate_user_session_refresh_token(
            user_id,
            session_id,
            expected_refresh_token_hash,
            next_refresh_token_hash,
            rotated_at,
            expires_at,
            ip_address,
            user_agent,
        )
        .await
    }

    async fn revoke_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<bool, DataLayerError> {
        self.revoke_user_session(user_id, session_id, revoked_at, reason)
            .await
    }

    async fn revoke_all_user_sessions(
        &self,
        user_id: &str,
        revoked_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
    ) -> Result<u64, DataLayerError> {
        self.revoke_all_user_sessions(user_id, revoked_at, reason)
            .await
    }

    async fn count_active_admin_users(&self) -> Result<u64, DataLayerError> {
        self.count_active_admin_users().await
    }

    async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, DataLayerError> {
        self.count_active_local_admin_users_with_valid_password()
            .await
    }
}

#[cfg(test)]
mod admin_invariant_tests {
    use super::{
        POSTGRES_ANONYMIZE_USER_API_KEY_HISTORY_SQL, POSTGRES_ANONYMIZE_USER_HISTORY_SQL,
        POSTGRES_DELETE_USER_DEPENDENTS_SQL, POSTGRES_LOCK_ACTIVE_ADMINS_SQL,
    };

    #[test]
    fn active_admin_mutations_use_a_deterministic_postgres_row_lock() {
        let normalized = POSTGRES_LOCK_ACTIVE_ADMINS_SQL
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(normalized.contains("role = 'admin'::userrole"));
        assert!(normalized.contains("is_active IS TRUE"));
        assert!(normalized.contains("is_deleted IS FALSE"));
        assert!(normalized.contains("ORDER BY id FOR UPDATE"));
        assert!(POSTGRES_DELETE_USER_DEPENDENTS_SQL
            .iter()
            .any(|sql| sql.starts_with("DELETE FROM management_tokens")));
        assert!(POSTGRES_DELETE_USER_DEPENDENTS_SQL
            .iter()
            .any(|sql| sql.starts_with("DELETE FROM api_keys")));
        assert!(POSTGRES_DELETE_USER_DEPENDENTS_SQL
            .iter()
            .any(|sql| sql.starts_with("DELETE FROM user_sessions")));
        assert_history_anonymization_contract(POSTGRES_ANONYMIZE_USER_HISTORY_SQL);
        assert!(POSTGRES_ANONYMIZE_USER_API_KEY_HISTORY_SQL
            .starts_with("UPDATE stats_daily_api_key SET api_key_name = NULL"));
        assert!(POSTGRES_ANONYMIZE_USER_API_KEY_HISTORY_SQL
            .contains("SELECT id FROM api_keys WHERE user_id = $1"));
    }

    fn assert_history_anonymization_contract(statements: &[&str]) {
        const TABLES: &[&str] = &[
            "request_candidates",
            "video_tasks",
            "usage",
            "stats_user_daily",
            "stats_user_summary",
            "stats_user_daily_model",
            "stats_user_daily_provider",
            "stats_user_daily_api_format",
            "stats_user_daily_model_provider",
            "stats_user_daily_cost_savings",
            "stats_user_daily_cost_savings_provider",
            "stats_user_daily_cost_savings_model",
            "stats_user_daily_cost_savings_model_provider",
        ];

        assert_eq!(statements.len(), TABLES.len());
        for table in TABLES {
            let statement = statements
                .iter()
                .find(|sql| sql.starts_with(&format!("UPDATE {table} ")))
                .unwrap_or_else(|| panic!("missing history anonymization for {table}"));
            assert!(statement.contains("username = NULL"));
            assert!(statement.ends_with("WHERE user_id = $1"));
        }
        for table in ["request_candidates", "video_tasks", "usage"] {
            let statement = statements
                .iter()
                .find(|sql| sql.starts_with(&format!("UPDATE {table} ")))
                .expect("identity snapshot table should be covered");
            assert!(statement.contains("api_key_name = NULL"));
        }
    }
}
