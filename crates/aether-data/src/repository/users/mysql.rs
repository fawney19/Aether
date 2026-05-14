use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use super::types::{
    normalize_user_group_name, LdapAuthUserProvisioningOutcome, StoredUserAuthRecord,
    StoredUserExportRow, StoredUserGroup, StoredUserGroupMember, StoredUserGroupMembership,
    StoredUserOAuthLinkSummary, StoredUserPreferenceRecord, StoredUserSessionRecord,
    StoredUserSummary, UpsertUserGroupRecord, UserExportListQuery, UserExportSummary,
    UserReadRepository,
};
use crate::driver::mysql::MysqlPool;
use crate::error::SqlResultExt;
use crate::DataLayerError;

const USER_SUMMARY_COLUMNS: &str = r#"
SELECT
  id,
  username,
  email,
  role,
  is_active,
  is_deleted
FROM users
"#;

const USER_EXPORT_COLUMNS: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role,
  auth_source,
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
"#;

const USER_AUTH_COLUMNS: &str = r#"
SELECT
  id,
  email,
  email_verified,
  username,
  password_hash,
  role,
  auth_source,
  allowed_providers,
  allowed_providers_mode,
  allowed_api_formats,
  allowed_api_formats_mode,
  allowed_models,
  allowed_models_mode,
  is_active,
  is_deleted,
  created_at,
  last_login_at
FROM users
"#;

const USER_AUTH_COLUMNS_QUALIFIED: &str = r#"
SELECT
  users.id AS id,
  users.email AS email,
  users.email_verified AS email_verified,
  users.username AS username,
  users.password_hash AS password_hash,
  users.role AS role,
  users.auth_source AS auth_source,
  users.allowed_providers AS allowed_providers,
  users.allowed_providers_mode AS allowed_providers_mode,
  users.allowed_api_formats AS allowed_api_formats,
  users.allowed_api_formats_mode AS allowed_api_formats_mode,
  users.allowed_models AS allowed_models,
  users.allowed_models_mode AS allowed_models_mode,
  users.is_active AS is_active,
  users.is_deleted AS is_deleted,
  users.created_at AS created_at,
  users.last_login_at AS last_login_at
FROM users
"#;

const USER_OAUTH_LINK_SUMMARY_COLUMNS: &str = r#"
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
"#;

const USER_PREFERENCES_COLUMNS: &str = r#"
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
"#;

const USER_SESSION_COLUMNS: &str = r#"
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
  users.role,
  users.is_active,
  users.is_deleted,
  user_group_members.created_at
FROM user_group_members
JOIN users ON users.id = user_group_members.user_id
"#;

#[derive(Debug, Clone)]
pub struct MysqlUserReadRepository {
    pool: MysqlPool,
}

impl MysqlUserReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }

    async fn fetch_summary_rows(
        &self,
        mut builder: QueryBuilder<'_, MySql>,
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_user_row).collect()
    }

    async fn fetch_export_rows(
        &self,
        mut builder: QueryBuilder<'_, MySql>,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_user_export_row).collect()
    }

    async fn fetch_auth_rows(
        &self,
        mut builder: QueryBuilder<'_, MySql>,
    ) -> Result<Vec<StoredUserAuthRecord>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_user_auth_row).collect()
    }

    async fn fetch_group_rows(
        &self,
        mut builder: QueryBuilder<'_, MySql>,
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_user_group_row).collect()
    }

    async fn fetch_group_member_rows(
        &self,
        mut builder: QueryBuilder<'_, MySql>,
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_user_group_member_row).collect()
    }
}

#[async_trait]
impl UserReadRepository for MysqlUserReadRepository {
    async fn list_users_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<MySql>::new(USER_SUMMARY_COLUMNS);
        builder.push(" WHERE id IN (");
        {
            let mut separated = builder.separated(", ");
            for user_id in user_ids {
                separated.push_bind(user_id);
            }
        }
        builder.push(") ORDER BY id ASC");
        self.fetch_summary_rows(builder).await
    }

    async fn list_users_by_username_search(
        &self,
        username_search: &str,
    ) -> Result<Vec<StoredUserSummary>, DataLayerError> {
        let username_search = username_search.trim();
        if username_search.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<MySql>::new(USER_SUMMARY_COLUMNS);
        builder
            .push(" WHERE is_deleted = 0 AND LOWER(username) LIKE ")
            .push_bind(format!("%{}%", username_search.to_ascii_lowercase()))
            .push(" ORDER BY id ASC");
        self.fetch_summary_rows(builder).await
    }

    async fn list_export_users(&self) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_EXPORT_COLUMNS);
        builder.push(" WHERE is_deleted = 0 ORDER BY id ASC");
        self.fetch_export_rows(builder).await
    }

    async fn list_export_users_page(
        &self,
        query: &UserExportListQuery,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_EXPORT_COLUMNS);
        builder.push(" WHERE is_deleted = 0");
        if let Some(role) = query.role.as_deref() {
            builder
                .push(" AND LOWER(role) = ")
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
        builder
            .push(" ORDER BY id ASC LIMIT ")
            .push_bind(i64::try_from(query.limit).map_err(|_| {
                DataLayerError::InvalidInput(format!("invalid user export limit: {}", query.limit))
            })?)
            .push(" OFFSET ")
            .push_bind(i64::try_from(query.skip).map_err(|_| {
                DataLayerError::InvalidInput(format!("invalid user export skip: {}", query.skip))
            })?);
        self.fetch_export_rows(builder).await
    }

    async fn summarize_export_users(&self) -> Result<UserExportSummary, DataLayerError> {
        let row = sqlx::query(
            r#"
SELECT
  COUNT(*) AS total,
  SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END) AS active
FROM users
WHERE is_deleted = 0
"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;

        Ok(UserExportSummary {
            total: row.try_get::<i64, _>("total").map_sql_err()?.max(0) as u64,
            active: row
                .try_get::<Option<i64>, _>("active")
                .map_sql_err()?
                .unwrap_or(0)
                .max(0) as u64,
        })
    }

    async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserExportRow>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_EXPORT_COLUMNS);
        builder
            .push(" WHERE is_deleted = 0 AND id = ")
            .push_bind(user_id)
            .push(" LIMIT 1");
        Ok(self.fetch_export_rows(builder).await?.into_iter().next())
    }

    async fn list_non_admin_export_users(
        &self,
    ) -> Result<Vec<StoredUserExportRow>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_EXPORT_COLUMNS);
        builder.push(" WHERE is_deleted = 0 AND LOWER(role) != 'admin' ORDER BY id ASC");
        self.fetch_export_rows(builder).await
    }

    async fn list_user_groups(&self) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_GROUP_COLUMNS);
        builder.push(" ORDER BY name ASC, id ASC");
        self.fetch_group_rows(builder).await
    }

    async fn find_user_group_by_id(
        &self,
        group_id: &str,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_GROUP_COLUMNS);
        builder
            .push(" WHERE id = ")
            .push_bind(group_id)
            .push(" LIMIT 1");
        Ok(self.fetch_group_rows(builder).await?.into_iter().next())
    }

    async fn list_user_groups_by_ids(
        &self,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        if group_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(USER_GROUP_COLUMNS);
        builder.push(" WHERE id IN (");
        {
            let mut separated = builder.separated(", ");
            for group_id in group_ids {
                separated.push_bind(group_id);
            }
        }
        builder.push(") ORDER BY name ASC, id ASC");
        self.fetch_group_rows(builder).await
    }

    async fn create_user_group(
        &self,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        let now = current_unix_secs();
        let id = uuid::Uuid::new_v4().to_string();
        let name = normalize_user_group_name(&record.name);
        let normalized_name = name.to_ascii_lowercase();
        let result = sqlx::query(
            r#"
INSERT INTO user_groups (
  id, name, normalized_name, description, priority,
  allowed_providers, allowed_providers_mode,
  allowed_api_formats, allowed_api_formats_mode,
  allowed_models, allowed_models_mode,
  rate_limit, rate_limit_mode, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&id)
        .bind(name)
        .bind(normalized_name)
        .bind(record.description)
        .bind(record.priority)
        .bind(json_string_from_option_vec(
            record.allowed_providers.as_ref(),
        ))
        .bind(record.allowed_providers_mode)
        .bind(json_string_from_option_vec(
            record.allowed_api_formats.as_ref(),
        ))
        .bind(record.allowed_api_formats_mode)
        .bind(json_string_from_option_vec(record.allowed_models.as_ref()))
        .bind(record.allowed_models_mode)
        .bind(record.rate_limit)
        .bind(record.rate_limit_mode)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await;
        match result {
            Ok(_) => self.find_user_group_by_id(&id).await,
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Err(
                DataLayerError::InvalidInput("duplicate user group name".to_string()),
            ),
            Err(err) => Err(err).map_sql_err(),
        }
    }

    async fn update_user_group(
        &self,
        group_id: &str,
        record: UpsertUserGroupRecord,
    ) -> Result<Option<StoredUserGroup>, DataLayerError> {
        let now = current_unix_secs();
        let name = normalize_user_group_name(&record.name);
        let normalized_name = name.to_ascii_lowercase();
        let result = sqlx::query(
            r#"
UPDATE user_groups
SET name = ?,
    normalized_name = ?,
    description = ?,
    priority = ?,
    allowed_providers = ?,
    allowed_providers_mode = ?,
    allowed_api_formats = ?,
    allowed_api_formats_mode = ?,
    allowed_models = ?,
    allowed_models_mode = ?,
    rate_limit = ?,
    rate_limit_mode = ?,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(name)
        .bind(normalized_name)
        .bind(record.description)
        .bind(record.priority)
        .bind(json_string_from_option_vec(
            record.allowed_providers.as_ref(),
        ))
        .bind(record.allowed_providers_mode)
        .bind(json_string_from_option_vec(
            record.allowed_api_formats.as_ref(),
        ))
        .bind(record.allowed_api_formats_mode)
        .bind(json_string_from_option_vec(record.allowed_models.as_ref()))
        .bind(record.allowed_models_mode)
        .bind(record.rate_limit)
        .bind(record.rate_limit_mode)
        .bind(now)
        .bind(group_id)
        .execute(&self.pool)
        .await;
        match result {
            Ok(result) if result.rows_affected() == 0 => Ok(None),
            Ok(_) => self.find_user_group_by_id(group_id).await,
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Err(
                DataLayerError::InvalidInput("duplicate user group name".to_string()),
            ),
            Err(err) => Err(err).map_sql_err(),
        }
    }

    async fn delete_user_group(&self, group_id: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query("DELETE FROM user_groups WHERE id = ?")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_user_group_members(
        &self,
        group_id: &str,
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_GROUP_MEMBER_COLUMNS);
        builder
            .push(" WHERE user_group_members.group_id = ")
            .push_bind(group_id)
            .push(" ORDER BY users.username ASC, users.id ASC");
        self.fetch_group_member_rows(builder).await
    }

    async fn replace_user_group_members(
        &self,
        group_id: &str,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMember>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        sqlx::query("DELETE FROM user_group_members WHERE group_id = ?")
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        let now = current_unix_secs();
        for user_id in normalized_ids(user_ids) {
            sqlx::query(
                "INSERT IGNORE INTO user_group_members (group_id, user_id, created_at) VALUES (?, ?, ?)",
            )
            .bind(group_id)
            .bind(user_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }
        tx.commit().await.map_sql_err()?;
        self.list_user_group_members(group_id).await
    }

    async fn list_user_groups_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_GROUP_COLUMNS);
        builder
            .push(" WHERE id IN (SELECT group_id FROM user_group_members WHERE user_id = ")
            .push_bind(user_id)
            .push(") ORDER BY name ASC, id ASC");
        self.fetch_group_rows(builder).await
    }

    async fn list_user_group_memberships_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserGroupMembership>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = QueryBuilder::<MySql>::new(
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
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_user_group_membership_row).collect()
    }

    async fn replace_user_groups_for_user(
        &self,
        user_id: &str,
        group_ids: &[String],
    ) -> Result<Vec<StoredUserGroup>, DataLayerError> {
        let mut tx = self.pool.begin().await.map_sql_err()?;
        sqlx::query("DELETE FROM user_group_members WHERE user_id = ?")
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        let now = current_unix_secs();
        for group_id in normalized_ids(group_ids) {
            sqlx::query(
                "INSERT IGNORE INTO user_group_members (group_id, user_id, created_at) VALUES (?, ?, ?)",
            )
            .bind(group_id)
            .bind(user_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_sql_err()?;
        }
        tx.commit().await.map_sql_err()?;
        self.list_user_groups_for_user(user_id).await
    }

    async fn add_user_to_group(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            "INSERT IGNORE INTO user_group_members (group_id, user_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(group_id)
        .bind(user_id)
        .bind(current_unix_secs())
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn find_user_auth_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_AUTH_COLUMNS);
        builder
            .push(" WHERE id = ")
            .push_bind(user_id)
            .push(" LIMIT 1");
        Ok(self.fetch_auth_rows(builder).await?.into_iter().next())
    }

    async fn list_user_auth_by_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredUserAuthRecord>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<MySql>::new(USER_AUTH_COLUMNS);
        builder.push(" WHERE id IN (");
        {
            let mut separated = builder.separated(", ");
            for user_id in user_ids {
                separated.push_bind(user_id);
            }
        }
        builder.push(") ORDER BY id ASC");
        self.fetch_auth_rows(builder).await
    }

    async fn find_user_auth_by_identifier(
        &self,
        identifier: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_AUTH_COLUMNS);
        builder
            .push(" WHERE email = ")
            .push_bind(identifier)
            .push(" OR username = ")
            .push_bind(identifier)
            .push(" LIMIT 1");
        Ok(self.fetch_auth_rows(builder).await?.into_iter().next())
    }

    async fn find_user_auth_by_email(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_AUTH_COLUMNS);
        builder
            .push(" WHERE email = ")
            .push_bind(email)
            .push(" LIMIT 1");
        Ok(self.fetch_auth_rows(builder).await?.into_iter().next())
    }

    async fn find_active_user_auth_by_email_ci(
        &self,
        email: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_AUTH_COLUMNS);
        builder
            .push(" WHERE LOWER(email) = LOWER(")
            .push_bind(email)
            .push(") AND is_deleted = 0 LIMIT 1");
        Ok(self.fetch_auth_rows(builder).await?.into_iter().next())
    }

    async fn find_user_auth_by_username(
        &self,
        username: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_AUTH_COLUMNS);
        builder
            .push(" WHERE username = ")
            .push_bind(username)
            .push(" LIMIT 1");
        Ok(self.fetch_auth_rows(builder).await?.into_iter().next())
    }

    async fn list_user_oauth_links(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserOAuthLinkSummary>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_OAUTH_LINK_SUMMARY_COLUMNS);
        builder
            .push(" WHERE user_oauth_links.user_id = ")
            .push_bind(user_id)
            .push(" ORDER BY user_oauth_links.linked_at ASC");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_oauth_link_summary_row).collect()
    }

    async fn find_oauth_linked_user(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_AUTH_COLUMNS_QUALIFIED);
        builder
            .push(" JOIN user_oauth_links ON users.id = user_oauth_links.user_id")
            .push(" WHERE user_oauth_links.provider_type = ")
            .push_bind(provider_type)
            .push(" AND user_oauth_links.provider_user_id = ")
            .push_bind(provider_user_id)
            .push(" LIMIT 1");
        Ok(self.fetch_auth_rows(builder).await?.into_iter().next())
    }

    async fn touch_oauth_link(
        &self,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        touched_at: DateTime<Utc>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE user_oauth_links
SET provider_username = COALESCE(?, provider_username),
    provider_email = COALESCE(?, provider_email),
    extra_data = COALESCE(?, extra_data),
    last_login_at = ?
WHERE provider_type = ?
  AND provider_user_id = ?
"#,
        )
        .bind(provider_username)
        .bind(provider_email)
        .bind(optional_json_string(
            extra_data,
            "user_oauth_links.extra_data",
        )?)
        .bind(touched_at.timestamp())
        .bind(provider_type)
        .bind(provider_user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn create_oauth_auth_user(
        &self,
        email: Option<String>,
        username: String,
        created_at: DateTime<Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let user_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            r#"
INSERT INTO users (
  id, email, email_verified, username, password_hash, role, auth_source,
  allowed_providers_mode, allowed_api_formats_mode, allowed_models_mode, rate_limit_mode,
  is_active, is_deleted, created_at, updated_at, last_login_at
)
VALUES (?, ?, 1, ?, NULL, 'user', 'oauth', 'inherit', 'inherit', 'inherit', 'inherit', 1, 0, ?, ?, ?)
"#,
        )
        .bind(&user_id)
        .bind(email)
        .bind(username)
        .bind(created_at.timestamp())
        .bind(created_at.timestamp())
        .bind(created_at.timestamp())
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.find_user_auth_by_id(&user_id).await
    }

    async fn find_oauth_link_owner(
        &self,
        provider_type: &str,
        provider_user_id: &str,
    ) -> Result<Option<String>, DataLayerError> {
        sqlx::query_scalar(
            "SELECT user_id FROM user_oauth_links WHERE provider_type = ? AND provider_user_id = ? LIMIT 1",
        )
        .bind(provider_type)
        .bind(provider_user_id)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()
    }

    async fn has_user_oauth_provider_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let owner: Option<String> = sqlx::query_scalar(
            "SELECT user_id FROM user_oauth_links WHERE user_id = ? AND provider_type = ? LIMIT 1",
        )
        .bind(user_id)
        .bind(provider_type)
        .fetch_optional(&self.pool)
        .await
        .map_sql_err()?;
        Ok(owner.is_some())
    }

    async fn count_user_oauth_links(&self, user_id: &str) -> Result<u64, DataLayerError> {
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM user_oauth_links WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&self.pool)
                .await
                .map_sql_err()?;
        Ok(total.max(0) as u64)
    }

    async fn upsert_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
        provider_user_id: &str,
        provider_username: Option<&str>,
        provider_email: Option<&str>,
        extra_data: Option<serde_json::Value>,
        linked_at: DateTime<Utc>,
    ) -> Result<(), DataLayerError> {
        let extra_data = optional_json_string(extra_data, "user_oauth_links.extra_data")?;
        let updated = sqlx::query(
            r#"
UPDATE user_oauth_links
SET provider_user_id = ?,
    provider_username = ?,
    provider_email = ?,
    extra_data = ?,
    last_login_at = ?
WHERE user_id = ?
  AND provider_type = ?
"#,
        )
        .bind(provider_user_id)
        .bind(provider_username)
        .bind(provider_email)
        .bind(extra_data.as_deref())
        .bind(linked_at.timestamp())
        .bind(user_id)
        .bind(provider_type)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        if updated.rows_affected() == 0 {
            sqlx::query(
                r#"
INSERT INTO user_oauth_links (
  id, user_id, provider_type, provider_user_id, provider_username, provider_email,
  extra_data, linked_at, last_login_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
            )
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(user_id)
            .bind(provider_type)
            .bind(provider_user_id)
            .bind(provider_username)
            .bind(provider_email)
            .bind(extra_data.as_deref())
            .bind(linked_at.timestamp())
            .bind(linked_at.timestamp())
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        }
        Ok(())
    }

    async fn delete_user_oauth_link(
        &self,
        user_id: &str,
        provider_type: &str,
    ) -> Result<bool, DataLayerError> {
        let result =
            sqlx::query("DELETE FROM user_oauth_links WHERE user_id = ? AND provider_type = ?")
                .bind(user_id)
                .bind(provider_type)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn get_or_create_ldap_auth_user(
        &self,
        email: String,
        username: String,
        ldap_dn: Option<String>,
        ldap_username: Option<String>,
        logged_in_at: DateTime<Utc>,
    ) -> Result<Option<LdapAuthUserProvisioningOutcome>, DataLayerError> {
        get_or_create_mysql_ldap_auth_user(
            &self.pool,
            email,
            username,
            ldap_dn,
            ldap_username,
            logged_in_at,
        )
        .await
    }

    async fn touch_auth_user_last_login(
        &self,
        user_id: &str,
        logged_in_at: DateTime<Utc>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query("UPDATE users SET last_login_at = ?, updated_at = ? WHERE id = ?")
            .bind(logged_in_at.timestamp())
            .bind(logged_in_at.timestamp())
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_local_auth_user_profile(
        &self,
        user_id: &str,
        email: Option<String>,
        username: Option<String>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let now = chrono::Utc::now().timestamp();
        let result = sqlx::query(
            "UPDATE users SET email = COALESCE(?, email), username = COALESCE(?, username), updated_at = ? WHERE id = ?",
        )
        .bind(email)
        .bind(username)
        .bind(now)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find_user_auth_by_id(user_id).await
    }

    async fn update_local_auth_user_password_hash(
        &self,
        user_id: &str,
        password_hash: String,
        updated_at: DateTime<Utc>,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let result = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(password_hash)
            .bind(updated_at.timestamp())
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find_user_auth_by_id(user_id).await
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
SET role = CASE WHEN ? THEN COALESCE(?, role) ELSE role END,
    allowed_providers = CASE WHEN ? THEN ? ELSE allowed_providers END,
    allowed_providers_mode = CASE WHEN ? THEN ? ELSE allowed_providers_mode END,
    allowed_api_formats = CASE WHEN ? THEN ? ELSE allowed_api_formats END,
    allowed_api_formats_mode = CASE WHEN ? THEN ? ELSE allowed_api_formats_mode END,
    allowed_models = CASE WHEN ? THEN ? ELSE allowed_models END,
    allowed_models_mode = CASE WHEN ? THEN ? ELSE allowed_models_mode END,
    rate_limit = CASE WHEN ? THEN ? ELSE rate_limit END,
    rate_limit_mode = CASE WHEN ? THEN ? ELSE rate_limit_mode END,
    is_active = CASE WHEN ? THEN ? ELSE is_active END,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(role.is_some())
        .bind(role)
        .bind(allowed_providers_present)
        .bind(optional_string_list_json(
            allowed_providers,
            "users.allowed_providers",
        )?)
        .bind(allowed_providers_present)
        .bind(allowed_providers_mode)
        .bind(allowed_api_formats_present)
        .bind(optional_string_list_json(
            allowed_api_formats,
            "users.allowed_api_formats",
        )?)
        .bind(allowed_api_formats_present)
        .bind(allowed_api_formats_mode)
        .bind(allowed_models_present)
        .bind(optional_string_list_json(
            allowed_models,
            "users.allowed_models",
        )?)
        .bind(allowed_models_present)
        .bind(allowed_models_mode)
        .bind(rate_limit_present)
        .bind(rate_limit)
        .bind(rate_limit_present)
        .bind(rate_limit_mode)
        .bind(is_active.is_some())
        .bind(is_active)
        .bind(chrono::Utc::now().timestamp())
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find_user_auth_by_id(user_id).await
    }

    async fn update_local_auth_user_policy_modes(
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
SET allowed_providers_mode = CASE WHEN ? THEN ? ELSE allowed_providers_mode END,
    allowed_api_formats_mode = CASE WHEN ? THEN ? ELSE allowed_api_formats_mode END,
    allowed_models_mode = CASE WHEN ? THEN ? ELSE allowed_models_mode END,
    rate_limit_mode = CASE WHEN ? THEN ? ELSE rate_limit_mode END,
    updated_at = ?
WHERE id = ?
"#,
        )
        .bind(allowed_providers_mode.is_some())
        .bind(allowed_providers_mode)
        .bind(allowed_api_formats_mode.is_some())
        .bind(allowed_api_formats_mode)
        .bind(allowed_models_mode.is_some())
        .bind(allowed_models_mode)
        .bind(rate_limit_mode.is_some())
        .bind(rate_limit_mode)
        .bind(chrono::Utc::now().timestamp())
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        self.find_user_auth_by_id(user_id).await
    }

    async fn update_user_model_capability_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let normalized = normalize_optional_json_value(settings);
        let result = sqlx::query(
            "UPDATE users SET model_capability_settings = ?, updated_at = ? WHERE id = ?",
        )
        .bind(optional_json_string(
            normalized.clone(),
            "users.model_capability_settings",
        )?)
        .bind(chrono::Utc::now().timestamp())
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(normalized)
    }

    async fn update_user_feature_settings(
        &self,
        user_id: &str,
        settings: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        let normalized = normalize_optional_json_value(settings);
        let result =
            sqlx::query("UPDATE users SET feature_settings = ?, updated_at = ? WHERE id = ?")
                .bind(optional_json_string(
                    normalized.clone(),
                    "users.feature_settings",
                )?)
                .bind(chrono::Utc::now().timestamp())
                .bind(user_id)
                .execute(&self.pool)
                .await
                .map_sql_err()?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(normalized)
    }

    async fn create_local_auth_user(
        &self,
        email: Option<String>,
        email_verified: bool,
        username: String,
        password_hash: String,
    ) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
        let user_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            r#"
INSERT INTO users (
  id, email, email_verified, username, password_hash, role, auth_source,
  allowed_providers_mode, allowed_api_formats_mode, allowed_models_mode, rate_limit_mode,
  is_active, is_deleted, created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, 'user', 'local', 'inherit', 'inherit', 'inherit', 'inherit', 1, 0, ?, ?)
"#,
        )
        .bind(&user_id)
        .bind(email)
        .bind(email_verified)
        .bind(username)
        .bind(password_hash)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.find_user_auth_by_id(&user_id).await
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
        let user_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp();
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
VALUES (?, ?, ?, ?, ?, ?, 'local', ?, ?, ?, ?, ?, ?, ?, ?, 1, 0, ?, ?)
"#,
        )
        .bind(&user_id)
        .bind(email)
        .bind(email_verified)
        .bind(username)
        .bind(password_hash)
        .bind(role)
        .bind(optional_string_list_json(
            allowed_providers,
            "users.allowed_providers",
        )?)
        .bind(allowed_providers_mode)
        .bind(optional_string_list_json(
            allowed_api_formats,
            "users.allowed_api_formats",
        )?)
        .bind(allowed_api_formats_mode)
        .bind(optional_string_list_json(
            allowed_models,
            "users.allowed_models",
        )?)
        .bind(allowed_models_mode)
        .bind(rate_limit)
        .bind(rate_limit_mode)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.find_user_auth_by_id(&user_id).await
    }

    async fn delete_local_auth_user(&self, user_id: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn count_active_admin_users(&self) -> Result<u64, DataLayerError> {
        let total: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM users
WHERE LOWER(role) = 'admin'
  AND is_deleted = 0
  AND is_active = 1
"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        Ok(total.max(0) as u64)
    }

    async fn read_user_preferences(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_PREFERENCES_COLUMNS);
        builder.push(" WHERE up.user_id = ").push_bind(user_id);
        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_user_preference_row).transpose()
    }

    async fn write_user_preferences(
        &self,
        preferences: &StoredUserPreferenceRecord,
    ) -> Result<Option<StoredUserPreferenceRecord>, DataLayerError> {
        let now = Utc::now().timestamp();
        sqlx::query(
            r#"
INSERT INTO user_preferences (
  id, user_id, avatar_url, bio, default_provider_id, theme, language, timezone,
  email_notifications, usage_alerts, announcement_notifications, created_at, updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON DUPLICATE KEY UPDATE
  avatar_url = VALUES(avatar_url),
  bio = VALUES(bio),
  default_provider_id = VALUES(default_provider_id),
  theme = VALUES(theme),
  language = VALUES(language),
  timezone = VALUES(timezone),
  email_notifications = VALUES(email_notifications),
  usage_alerts = VALUES(usage_alerts),
  announcement_notifications = VALUES(announcement_notifications),
  updated_at = VALUES(updated_at)
"#,
        )
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
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.read_user_preferences(&preferences.user_id).await
    }

    async fn find_user_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_SESSION_COLUMNS);
        builder
            .push(" WHERE user_id = ")
            .push_bind(user_id)
            .push(" AND id = ")
            .push_bind(session_id)
            .push(" LIMIT 1");
        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_sql_err()?;
        row.as_ref().map(map_user_session_row).transpose()
    }

    async fn list_user_sessions(
        &self,
        user_id: &str,
    ) -> Result<Vec<StoredUserSessionRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<MySql>::new(USER_SESSION_COLUMNS);
        builder
            .push(" WHERE user_id = ")
            .push_bind(user_id)
            .push(" AND revoked_at IS NULL AND expires_at > ")
            .push_bind(Utc::now().timestamp())
            .push(" ORDER BY last_seen_at DESC, created_at DESC");
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_user_session_row).collect()
    }

    async fn create_user_session(
        &self,
        session: &StoredUserSessionRecord,
    ) -> Result<Option<StoredUserSessionRecord>, DataLayerError> {
        let now = session
            .created_at
            .or(session.updated_at)
            .or(session.last_seen_at)
            .unwrap_or_else(Utc::now);
        sqlx::query(
            r#"
UPDATE user_sessions
SET revoked_at = ?, revoke_reason = 'replaced_by_new_login', updated_at = ?
WHERE user_id = ? AND client_device_id = ? AND revoked_at IS NULL AND expires_at > ?
"#,
        )
        .bind(now.timestamp())
        .bind(now.timestamp())
        .bind(&session.user_id)
        .bind(&session.client_device_id)
        .bind(now.timestamp())
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        sqlx::query(
            r#"
INSERT INTO user_sessions (
  id, user_id, client_device_id, device_label, device_type, ip_address, user_agent,
  refresh_token_hash, last_seen_at, expires_at, created_at, updated_at
) VALUES (?, ?, ?, ?, 'unknown', ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&session.id)
        .bind(&session.user_id)
        .bind(&session.client_device_id)
        .bind(session.device_label.as_deref())
        .bind(session.ip_address.as_deref())
        .bind(session.user_agent.as_deref())
        .bind(&session.refresh_token_hash)
        .bind(session.last_seen_at.unwrap_or(now).timestamp())
        .bind(session.expires_at.unwrap_or(now).timestamp())
        .bind(session.created_at.unwrap_or(now).timestamp())
        .bind(session.updated_at.unwrap_or(now).timestamp())
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.find_user_session(&session.user_id, &session.id).await
    }

    async fn touch_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        touched_at: DateTime<Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE user_sessions
SET last_seen_at = ?, ip_address = COALESCE(?, ip_address),
    user_agent = COALESCE(?, user_agent), updated_at = ?
WHERE user_id = ? AND id = ?
"#,
        )
        .bind(touched_at.timestamp())
        .bind(ip_address)
        .bind(user_agent.map(|value| value.chars().take(1000).collect::<String>()))
        .bind(touched_at.timestamp())
        .bind(user_id)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_user_session_device_label(
        &self,
        user_id: &str,
        session_id: &str,
        device_label: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            "UPDATE user_sessions SET device_label = ?, updated_at = ? WHERE user_id = ? AND id = ?",
        )
        .bind(device_label.chars().take(120).collect::<String>())
        .bind(updated_at.timestamp())
        .bind(user_id)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn rotate_user_session_refresh_token(
        &self,
        user_id: &str,
        session_id: &str,
        previous_refresh_token_hash: &str,
        next_refresh_token_hash: &str,
        rotated_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            r#"
UPDATE user_sessions
SET prev_refresh_token_hash = ?, rotated_at = ?, refresh_token_hash = ?,
    expires_at = ?, last_seen_at = ?, ip_address = COALESCE(?, ip_address),
    user_agent = COALESCE(?, user_agent), updated_at = ?
WHERE user_id = ? AND id = ?
"#,
        )
        .bind(previous_refresh_token_hash)
        .bind(rotated_at.timestamp())
        .bind(next_refresh_token_hash)
        .bind(expires_at.timestamp())
        .bind(rotated_at.timestamp())
        .bind(ip_address)
        .bind(user_agent.map(|value| value.chars().take(1000).collect::<String>()))
        .bind(rotated_at.timestamp())
        .bind(user_id)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn revoke_user_session(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: DateTime<Utc>,
        reason: &str,
    ) -> Result<bool, DataLayerError> {
        let result = sqlx::query(
            "UPDATE user_sessions SET revoked_at = ?, revoke_reason = ?, updated_at = ? WHERE user_id = ? AND id = ?",
        )
        .bind(revoked_at.timestamp())
        .bind(reason.chars().take(100).collect::<String>())
        .bind(revoked_at.timestamp())
        .bind(user_id)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn revoke_all_user_sessions(
        &self,
        user_id: &str,
        revoked_at: DateTime<Utc>,
        reason: &str,
    ) -> Result<u64, DataLayerError> {
        let result = sqlx::query(
            "UPDATE user_sessions SET revoked_at = ?, revoke_reason = ?, updated_at = ? WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(revoked_at.timestamp())
        .bind(reason.chars().take(100).collect::<String>())
        .bind(revoked_at.timestamp())
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        Ok(result.rows_affected())
    }

    async fn count_active_local_admin_users_with_valid_password(
        &self,
    ) -> Result<u64, DataLayerError> {
        let total: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM users
WHERE LOWER(role) = 'admin'
  AND LOWER(auth_source) = 'local'
  AND is_deleted = 0
  AND is_active = 1
  AND CHAR_LENGTH(password_hash) = 60
  AND (
    password_hash LIKE '$2a$%'
    OR password_hash LIKE '$2b$%'
    OR password_hash LIKE '$2y$%'
  )
"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_sql_err()?;
        Ok(total.max(0) as u64)
    }
}

fn optional_json_from_string(
    value: Option<String>,
    field_name: &str,
) -> Result<Option<serde_json::Value>, DataLayerError> {
    value
        .map(|value| {
            serde_json::from_str(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains invalid JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn optional_string_list_json(
    value: Option<Vec<String>>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| {
            serde_json::to_string(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} could not be serialized as JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn json_string_from_option_vec(value: Option<&Vec<String>>) -> Option<String> {
    value.and_then(|items| serde_json::to_string(items).ok())
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

fn current_unix_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn optional_json_string(
    value: Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| {
            serde_json::to_string(&value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} could not be serialized as JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn normalize_optional_json_value(value: Option<serde_json::Value>) -> Option<serde_json::Value> {
    match value {
        Some(serde_json::Value::Null) | None => None,
        Some(value) => Some(value),
    }
}

async fn get_or_create_mysql_ldap_auth_user(
    pool: &MysqlPool,
    email: String,
    username: String,
    ldap_dn: Option<String>,
    ldap_username: Option<String>,
    logged_in_at: DateTime<Utc>,
) -> Result<Option<LdapAuthUserProvisioningOutcome>, DataLayerError> {
    let existing =
        find_mysql_ldap_auth_user(pool, ldap_dn.as_deref(), ldap_username.as_deref(), &email)
            .await?;
    if let Some(existing) = existing {
        if existing.is_deleted
            || !existing.is_active
            || !existing.auth_source.eq_ignore_ascii_case("ldap")
        {
            return Ok(None);
        }
        if existing.email.as_deref() != Some(email.as_str()) {
            let taken: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM users WHERE email = ? AND id <> ? LIMIT 1")
                    .bind(&email)
                    .bind(&existing.id)
                    .fetch_optional(pool)
                    .await
                    .map_sql_err()?;
            if taken.is_some() {
                return Ok(None);
            }
        }
        sqlx::query("UPDATE users SET email = ?, email_verified = 1, ldap_dn = COALESCE(?, ldap_dn), ldap_username = COALESCE(?, ldap_username), last_login_at = ?, updated_at = ? WHERE id = ?")
            .bind(&email)
            .bind(ldap_dn.as_deref())
            .bind(ldap_username.as_deref())
            .bind(logged_in_at.timestamp())
            .bind(logged_in_at.timestamp())
            .bind(&existing.id)
            .execute(pool)
            .await
            .map_sql_err()?;
        let user = find_mysql_auth_by_id(pool, &existing.id)
            .await?
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue("updated LDAP user disappeared".to_string())
            })?;
        return Ok(Some(LdapAuthUserProvisioningOutcome {
            user,
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
        let taken: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM users WHERE username = ? LIMIT 1")
                .bind(&candidate_username)
                .fetch_optional(pool)
                .await
                .map_sql_err()?;
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
        let user_id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO users (id, email, email_verified, username, password_hash, role, auth_source, ldap_dn, ldap_username, is_active, is_deleted, created_at, updated_at, last_login_at) VALUES (?, ?, 1, ?, NULL, 'user', 'ldap', ?, ?, 1, 0, ?, ?, ?)")
            .bind(&user_id)
            .bind(&email)
            .bind(&candidate_username)
            .bind(ldap_dn.as_deref())
            .bind(ldap_username.as_deref())
            .bind(logged_in_at.timestamp())
            .bind(logged_in_at.timestamp())
            .bind(logged_in_at.timestamp())
            .execute(pool)
            .await
            .map_sql_err()?;
        let user = find_mysql_auth_by_id(pool, &user_id)
            .await?
            .ok_or_else(|| {
                DataLayerError::UnexpectedValue("created LDAP user disappeared".to_string())
            })?;
        return Ok(Some(LdapAuthUserProvisioningOutcome {
            user,
            created: true,
        }));
    }
    Ok(None)
}

async fn find_mysql_ldap_auth_user(
    pool: &MysqlPool,
    ldap_dn: Option<&str>,
    ldap_username: Option<&str>,
    email: &str,
) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
    if let Some(ldap_dn) = ldap_dn.filter(|value| !value.trim().is_empty()) {
        let row = sqlx::query(&format!(
            "{USER_AUTH_COLUMNS} WHERE auth_source = 'ldap' AND ldap_dn = ? LIMIT 1"
        ))
        .bind(ldap_dn)
        .fetch_optional(pool)
        .await
        .map_sql_err()?;
        if let Some(row) = row.as_ref() {
            return map_user_auth_row(row).map(Some);
        }
    }
    if let Some(ldap_username) = ldap_username.filter(|value| !value.trim().is_empty()) {
        let row = sqlx::query(&format!(
            "{USER_AUTH_COLUMNS} WHERE auth_source = 'ldap' AND ldap_username = ? LIMIT 1"
        ))
        .bind(ldap_username)
        .fetch_optional(pool)
        .await
        .map_sql_err()?;
        if let Some(row) = row.as_ref() {
            return map_user_auth_row(row).map(Some);
        }
    }
    let row = sqlx::query(&format!("{USER_AUTH_COLUMNS} WHERE email = ? LIMIT 1"))
        .bind(email)
        .fetch_optional(pool)
        .await
        .map_sql_err()?;
    row.as_ref().map(map_user_auth_row).transpose()
}

async fn find_mysql_auth_by_id(
    pool: &MysqlPool,
    user_id: &str,
) -> Result<Option<StoredUserAuthRecord>, DataLayerError> {
    let row = sqlx::query(&format!("{USER_AUTH_COLUMNS} WHERE id = ? LIMIT 1"))
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_sql_err()?;
    row.as_ref().map(map_user_auth_row).transpose()
}

fn optional_datetime_from_unix_secs(value: Option<i64>) -> Option<DateTime<Utc>> {
    value.and_then(|value| Utc.timestamp_opt(value, 0).single())
}

fn map_user_row(row: &MySqlRow) -> Result<StoredUserSummary, DataLayerError> {
    StoredUserSummary::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("username").map_sql_err()?,
        row.try_get("email").map_sql_err()?,
        row.try_get("role").map_sql_err()?,
        row.try_get("is_active").map_sql_err()?,
        row.try_get("is_deleted").map_sql_err()?,
    )
}

fn map_user_export_row(row: &MySqlRow) -> Result<StoredUserExportRow, DataLayerError> {
    let feature_settings = optional_json_from_string(
        row.try_get("feature_settings").map_sql_err()?,
        "users.feature_settings",
    )?;
    StoredUserExportRow::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("email").map_sql_err()?,
        row.try_get("email_verified").map_sql_err()?,
        row.try_get("username").map_sql_err()?,
        row.try_get("password_hash").map_sql_err()?,
        row.try_get("role").map_sql_err()?,
        row.try_get("auth_source").map_sql_err()?,
        optional_json_from_string(
            row.try_get("allowed_providers").map_sql_err()?,
            "users.allowed_providers",
        )?,
        optional_json_from_string(
            row.try_get("allowed_api_formats").map_sql_err()?,
            "users.allowed_api_formats",
        )?,
        optional_json_from_string(
            row.try_get("allowed_models").map_sql_err()?,
            "users.allowed_models",
        )?,
        row.try_get("rate_limit").map_sql_err()?,
        optional_json_from_string(
            row.try_get("model_capability_settings").map_sql_err()?,
            "users.model_capability_settings",
        )?,
        row.try_get("is_active").map_sql_err()?,
    )
    .map(|record| record.with_feature_settings(feature_settings))
    .and_then(|record| {
        record.with_policy_modes(
            row.try_get("allowed_providers_mode").map_sql_err()?,
            row.try_get("allowed_api_formats_mode").map_sql_err()?,
            row.try_get("allowed_models_mode").map_sql_err()?,
            row.try_get("rate_limit_mode").map_sql_err()?,
        )
    })
}

fn map_user_auth_row(row: &MySqlRow) -> Result<StoredUserAuthRecord, DataLayerError> {
    StoredUserAuthRecord::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("email").map_sql_err()?,
        row.try_get("email_verified").map_sql_err()?,
        row.try_get("username").map_sql_err()?,
        row.try_get("password_hash").map_sql_err()?,
        row.try_get("role").map_sql_err()?,
        row.try_get("auth_source").map_sql_err()?,
        optional_json_from_string(
            row.try_get("allowed_providers").map_sql_err()?,
            "users.allowed_providers",
        )?,
        optional_json_from_string(
            row.try_get("allowed_api_formats").map_sql_err()?,
            "users.allowed_api_formats",
        )?,
        optional_json_from_string(
            row.try_get("allowed_models").map_sql_err()?,
            "users.allowed_models",
        )?,
        row.try_get("is_active").map_sql_err()?,
        row.try_get("is_deleted").map_sql_err()?,
        optional_datetime_from_unix_secs(row.try_get("created_at").map_sql_err()?),
        optional_datetime_from_unix_secs(row.try_get("last_login_at").map_sql_err()?),
    )
    .and_then(|record| {
        record.with_policy_modes(
            row.try_get("allowed_providers_mode").map_sql_err()?,
            row.try_get("allowed_api_formats_mode").map_sql_err()?,
            row.try_get("allowed_models_mode").map_sql_err()?,
        )
    })
}

fn map_user_group_row(row: &MySqlRow) -> Result<StoredUserGroup, DataLayerError> {
    StoredUserGroup::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        row.try_get("normalized_name").map_sql_err()?,
        row.try_get("description").map_sql_err()?,
        row.try_get("priority").map_sql_err()?,
        optional_json_from_string(
            row.try_get("allowed_providers").map_sql_err()?,
            "user_groups.allowed_providers",
        )?,
        row.try_get("allowed_providers_mode").map_sql_err()?,
        optional_json_from_string(
            row.try_get("allowed_api_formats").map_sql_err()?,
            "user_groups.allowed_api_formats",
        )?,
        row.try_get("allowed_api_formats_mode").map_sql_err()?,
        optional_json_from_string(
            row.try_get("allowed_models").map_sql_err()?,
            "user_groups.allowed_models",
        )?,
        row.try_get("allowed_models_mode").map_sql_err()?,
        row.try_get("rate_limit").map_sql_err()?,
        row.try_get("rate_limit_mode").map_sql_err()?,
        optional_datetime_from_unix_secs(row.try_get("created_at").map_sql_err()?),
        optional_datetime_from_unix_secs(row.try_get("updated_at").map_sql_err()?),
    )
}

fn map_user_group_member_row(row: &MySqlRow) -> Result<StoredUserGroupMember, DataLayerError> {
    Ok(StoredUserGroupMember {
        group_id: row.try_get("group_id").map_sql_err()?,
        user_id: row.try_get("user_id").map_sql_err()?,
        username: row.try_get("username").map_sql_err()?,
        email: row.try_get("email").map_sql_err()?,
        role: row.try_get("role").map_sql_err()?,
        is_active: row.try_get("is_active").map_sql_err()?,
        is_deleted: row.try_get("is_deleted").map_sql_err()?,
        created_at: optional_datetime_from_unix_secs(row.try_get("created_at").map_sql_err()?),
    })
}

fn map_user_group_membership_row(
    row: &MySqlRow,
) -> Result<StoredUserGroupMembership, DataLayerError> {
    Ok(StoredUserGroupMembership {
        user_id: row.try_get("user_id").map_sql_err()?,
        group_id: row.try_get("group_id").map_sql_err()?,
        group_name: row.try_get("group_name").map_sql_err()?,
        group_priority: row.try_get("group_priority").map_sql_err()?,
        created_at: optional_datetime_from_unix_secs(row.try_get("created_at").map_sql_err()?),
    })
}

fn map_oauth_link_summary_row(
    row: &MySqlRow,
) -> Result<StoredUserOAuthLinkSummary, DataLayerError> {
    StoredUserOAuthLinkSummary::new(
        row.try_get("provider_type").map_sql_err()?,
        row.try_get("display_name").map_sql_err()?,
        row.try_get("provider_username").map_sql_err()?,
        row.try_get("provider_email").map_sql_err()?,
        optional_datetime_from_unix_secs(row.try_get("linked_at").map_sql_err()?),
        optional_datetime_from_unix_secs(row.try_get("last_login_at").map_sql_err()?),
        row.try_get("provider_enabled").map_sql_err()?,
    )
}

fn map_user_preference_row(row: &MySqlRow) -> Result<StoredUserPreferenceRecord, DataLayerError> {
    let user_id: String = row.try_get("user_id").map_sql_err()?;
    if user_id.trim().is_empty() {
        return Err(DataLayerError::UnexpectedValue(
            "user_preferences.user_id is empty".to_string(),
        ));
    }

    Ok(StoredUserPreferenceRecord {
        user_id,
        avatar_url: row.try_get("avatar_url").map_sql_err()?,
        bio: row.try_get("bio").map_sql_err()?,
        default_provider_id: row.try_get("default_provider_id").map_sql_err()?,
        default_provider_name: row.try_get("default_provider_name").map_sql_err()?,
        theme: row.try_get("theme").map_sql_err()?,
        language: row.try_get("language").map_sql_err()?,
        timezone: row.try_get("timezone").map_sql_err()?,
        email_notifications: row.try_get("email_notifications").map_sql_err()?,
        usage_alerts: row.try_get("usage_alerts").map_sql_err()?,
        announcement_notifications: row.try_get("announcement_notifications").map_sql_err()?,
    })
}

fn map_user_session_row(row: &MySqlRow) -> Result<StoredUserSessionRecord, DataLayerError> {
    StoredUserSessionRecord::new(
        row.try_get("id").map_sql_err()?,
        row.try_get("user_id").map_sql_err()?,
        row.try_get("client_device_id").map_sql_err()?,
        row.try_get("device_label").map_sql_err()?,
        row.try_get("refresh_token_hash").map_sql_err()?,
        row.try_get("prev_refresh_token_hash").map_sql_err()?,
        optional_datetime_from_unix_secs(row.try_get("rotated_at").map_sql_err()?),
        optional_datetime_from_unix_secs(row.try_get("last_seen_at").map_sql_err()?),
        optional_datetime_from_unix_secs(row.try_get("expires_at").map_sql_err()?),
        optional_datetime_from_unix_secs(row.try_get("revoked_at").map_sql_err()?),
        row.try_get("revoke_reason").map_sql_err()?,
        row.try_get("ip_address").map_sql_err()?,
        row.try_get("user_agent").map_sql_err()?,
        optional_datetime_from_unix_secs(row.try_get("created_at").map_sql_err()?),
        optional_datetime_from_unix_secs(row.try_get("updated_at").map_sql_err()?),
    )
}

#[cfg(test)]
mod tests {
    use super::MysqlUserReadRepository;

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlUserReadRepository::new(pool);
    }
}
