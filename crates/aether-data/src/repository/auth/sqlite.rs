use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use super::types::{
    AuthApiKeyExportSummary, AuthApiKeyLookupKey, AuthApiKeyReadRepository,
    AuthApiKeyWriteRepository, CreateStandaloneApiKeyRecord, CreateUserApiKeyRecord,
    StandaloneApiKeyExportListQuery, StoredAuthApiKeyExportRecord, StoredAuthApiKeySnapshot,
    UpdateStandaloneApiKeyBasicRecord, UpdateUserApiKeyBasicRecord,
};
use crate::driver::sqlite::{sqlite_real, SqlitePool};
use crate::error::SqlResultExt;
use crate::DataLayerError;

const SNAPSHOT_COLUMNS: &str = r#"
SELECT
  users.id AS user_id,
  users.username,
  users.email,
  users.role AS user_role,
  users.auth_source AS user_auth_source,
  users.is_active AS user_is_active,
  users.is_deleted AS user_is_deleted,
  users.rate_limit AS user_rate_limit,
  users.allowed_providers AS user_allowed_providers,
  users.allowed_api_formats AS user_allowed_api_formats,
  users.allowed_models AS user_allowed_models,
  api_keys.id AS api_key_id,
  api_keys.name AS api_key_name,
  api_keys.is_active AS api_key_is_active,
  api_keys.is_locked AS api_key_is_locked,
  api_keys.is_standalone AS api_key_is_standalone,
  api_keys.rate_limit AS api_key_rate_limit,
  api_keys.concurrent_limit AS api_key_concurrent_limit,
  api_keys.expires_at AS api_key_expires_at_unix_secs,
  api_keys.allowed_providers AS api_key_allowed_providers,
  api_keys.allowed_api_formats AS api_key_allowed_api_formats,
  api_keys.allowed_models AS api_key_allowed_models
FROM api_keys
JOIN users ON users.id = api_keys.user_id
"#;

const EXPORT_COLUMNS: &str = r#"
SELECT
  api_keys.user_id,
  api_keys.id AS api_key_id,
  api_keys.key_hash,
  api_keys.key_encrypted,
  api_keys.name,
  api_keys.allowed_providers,
  api_keys.allowed_api_formats,
  api_keys.allowed_models,
  api_keys.rate_limit,
  api_keys.concurrent_limit,
  api_keys.force_capabilities,
  api_keys.feature_settings,
  api_keys.is_active,
  api_keys.expires_at AS expires_at_unix_secs,
  api_keys.auto_delete_on_expiry,
  api_keys.total_requests,
  COALESCE(api_keys.total_tokens, 0) AS total_tokens,
  CAST(COALESCE(api_keys.total_cost_usd, 0) AS REAL) AS total_cost_usd,
  api_keys.last_used_at AS last_used_at_unix_secs,
  api_keys.created_at AS created_at_unix_secs,
  api_keys.updated_at AS updated_at_unix_secs,
  api_keys.is_standalone
FROM api_keys
"#;

#[derive(Debug, Clone)]
pub struct SqliteAuthApiKeyReadRepository {
    pool: SqlitePool,
}

impl SqliteAuthApiKeyReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    async fn fetch_snapshot_rows(
        &self,
        mut builder: QueryBuilder<'_, Sqlite>,
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_auth_api_key_snapshot_row).collect()
    }

    async fn fetch_export_rows(
        &self,
        mut builder: QueryBuilder<'_, Sqlite>,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let rows = builder.build().fetch_all(&self.pool).await.map_sql_err()?;
        rows.iter().map(map_auth_api_key_export_row).collect()
    }

    async fn reload_export_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        Ok(self
            .list_export_api_keys_by_ids(&[api_key_id.to_string()])
            .await?
            .into_iter()
            .next())
    }

    async fn create_api_key(
        &self,
        record: CreateApiKeyInsertRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let now = current_unix_secs();
        sqlx::query(
            r#"
INSERT INTO api_keys (
  id, user_id, key_hash, key_encrypted, name, allowed_providers,
  allowed_api_formats, allowed_models, rate_limit, concurrent_limit,
  force_capabilities, feature_settings, is_active, expires_at, auto_delete_on_expiry,
  total_requests, total_tokens, total_cost_usd, is_standalone,
  created_at, updated_at
)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
        )
        .bind(&record.api_key_id)
        .bind(&record.user_id)
        .bind(&record.key_hash)
        .bind(&record.key_encrypted)
        .bind(&record.name)
        .bind(json_string_from_string_list(
            record.allowed_providers.as_ref(),
            "api_keys.allowed_providers",
        )?)
        .bind(json_string_from_string_list(
            record.allowed_api_formats.as_ref(),
            "api_keys.allowed_api_formats",
        )?)
        .bind(json_string_from_string_list(
            record.allowed_models.as_ref(),
            "api_keys.allowed_models",
        )?)
        .bind(record.rate_limit)
        .bind(record.concurrent_limit)
        .bind(optional_json_to_string(
            &record.force_capabilities,
            "api_keys.force_capabilities",
        )?)
        .bind(record.is_active)
        .bind(optional_i64_from_u64(
            record.expires_at_unix_secs,
            "api_keys.expires_at",
        )?)
        .bind(record.auto_delete_on_expiry)
        .bind(i64_from_u64(
            record.total_requests,
            "api_keys.total_requests",
        )?)
        .bind(i64_from_u64(record.total_tokens, "api_keys.total_tokens")?)
        .bind(record.total_cost_usd)
        .bind(record.is_standalone)
        .bind(now as i64)
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        self.reload_export_by_id(&record.api_key_id).await
    }
}

struct CreateApiKeyInsertRecord {
    user_id: String,
    api_key_id: String,
    key_hash: String,
    key_encrypted: Option<String>,
    name: Option<String>,
    allowed_providers: Option<Vec<String>>,
    allowed_api_formats: Option<Vec<String>>,
    allowed_models: Option<Vec<String>>,
    rate_limit: Option<i32>,
    concurrent_limit: Option<i32>,
    force_capabilities: Option<serde_json::Value>,
    is_active: bool,
    expires_at_unix_secs: Option<u64>,
    auto_delete_on_expiry: bool,
    total_requests: u64,
    total_tokens: u64,
    total_cost_usd: f64,
    is_standalone: bool,
}

#[async_trait]
impl AuthApiKeyReadRepository for SqliteAuthApiKeyReadRepository {
    async fn find_api_key_snapshot(
        &self,
        key: AuthApiKeyLookupKey<'_>,
    ) -> Result<Option<StoredAuthApiKeySnapshot>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(SNAPSHOT_COLUMNS);
        match key {
            AuthApiKeyLookupKey::KeyHash(key_hash) => {
                builder
                    .push(" WHERE api_keys.key_hash = ")
                    .push_bind(key_hash);
            }
            AuthApiKeyLookupKey::ApiKeyId(api_key_id) => {
                builder.push(" WHERE api_keys.id = ").push_bind(api_key_id);
            }
            AuthApiKeyLookupKey::UserApiKeyIds {
                user_id,
                api_key_id,
            } => {
                builder
                    .push(" WHERE api_keys.id = ")
                    .push_bind(api_key_id)
                    .push(" AND users.id = ")
                    .push_bind(user_id);
            }
        }
        builder.push(" LIMIT 1");
        Ok(self.fetch_snapshot_rows(builder).await?.into_iter().next())
    }

    async fn list_api_key_snapshots_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeySnapshot>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(SNAPSHOT_COLUMNS);
        push_in_clause(&mut builder, " WHERE api_keys.id IN (", api_key_ids);
        builder.push(" ORDER BY api_keys.id ASC");
        self.fetch_snapshot_rows(builder).await
    }

    async fn list_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(EXPORT_COLUMNS);
        push_in_clause(&mut builder, " WHERE api_keys.user_id IN (", user_ids);
        builder
            .push(" AND api_keys.is_standalone = 0 ORDER BY api_keys.user_id ASC, api_keys.id ASC");
        self.fetch_export_rows(builder).await
    }

    async fn list_export_api_keys_by_ids(
        &self,
        api_key_ids: &[String],
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        if api_key_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(EXPORT_COLUMNS);
        push_in_clause(&mut builder, " WHERE api_keys.id IN (", api_key_ids);
        builder.push(" ORDER BY api_keys.id ASC");
        self.fetch_export_rows(builder).await
    }

    async fn list_export_api_keys_by_name_search(
        &self,
        name_search: &str,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let name_search = name_search.trim();
        if name_search.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(EXPORT_COLUMNS);
        builder
            .push(" WHERE LOWER(COALESCE(api_keys.name, '')) LIKE ")
            .push_bind(format!("%{}%", name_search.to_ascii_lowercase()))
            .push(" ORDER BY api_keys.id ASC");
        self.fetch_export_rows(builder).await
    }

    async fn list_export_standalone_api_keys_page(
        &self,
        query: &StandaloneApiKeyExportListQuery,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(EXPORT_COLUMNS);
        builder.push(" WHERE api_keys.is_standalone = 1");
        if let Some(is_active) = query.is_active {
            builder
                .push(" AND api_keys.is_active = ")
                .push_bind(is_active);
        }
        builder
            .push(" ORDER BY api_keys.id ASC LIMIT ")
            .push_bind(i64::try_from(query.limit).map_err(|_| {
                DataLayerError::InvalidInput(format!(
                    "invalid standalone api key export limit: {}",
                    query.limit
                ))
            })?)
            .push(" OFFSET ")
            .push_bind(i64::try_from(query.skip).map_err(|_| {
                DataLayerError::InvalidInput(format!(
                    "invalid standalone api key export skip: {}",
                    query.skip
                ))
            })?);
        self.fetch_export_rows(builder).await
    }

    async fn count_export_standalone_api_keys(
        &self,
        is_active: Option<bool>,
    ) -> Result<u64, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "SELECT COUNT(*) AS total FROM api_keys WHERE is_standalone = 1",
        );
        if let Some(is_active) = is_active {
            builder.push(" AND is_active = ").push_bind(is_active);
        }
        let row = builder.build().fetch_one(&self.pool).await.map_sql_err()?;
        Ok(row.try_get::<i64, _>("total").map_sql_err()?.max(0) as u64)
    }

    async fn summarize_export_api_keys_by_user_ids(
        &self,
        user_ids: &[String],
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        if user_ids.is_empty() {
            return Ok(AuthApiKeyExportSummary::default());
        }

        let mut builder = QueryBuilder::<Sqlite>::new(
            r#"
SELECT
  COUNT(*) AS total,
  SUM(CASE WHEN is_active = 1 AND (expires_at IS NULL OR expires_at >=
"#,
        );
        builder.push_bind(now_unix_secs as i64);
        builder.push(
            r#") THEN 1 ELSE 0 END) AS active
FROM api_keys
"#,
        );
        push_in_clause(&mut builder, " WHERE user_id IN (", user_ids);
        builder.push(" AND is_standalone = 0");
        summarize_row(builder.build().fetch_one(&self.pool).await.map_sql_err()?)
    }

    async fn summarize_export_non_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        summarize_api_keys(&self.pool, false, now_unix_secs).await
    }

    async fn summarize_export_standalone_api_keys(
        &self,
        now_unix_secs: u64,
    ) -> Result<AuthApiKeyExportSummary, DataLayerError> {
        summarize_api_keys(&self.pool, true, now_unix_secs).await
    }

    async fn find_export_standalone_api_key_by_id(
        &self,
        api_key_id: &str,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(EXPORT_COLUMNS);
        builder
            .push(" WHERE api_keys.is_standalone = 1 AND api_keys.id = ")
            .push_bind(api_key_id)
            .push(" LIMIT 1");
        Ok(self.fetch_export_rows(builder).await?.into_iter().next())
    }

    async fn list_export_standalone_api_keys(
        &self,
    ) -> Result<Vec<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new(EXPORT_COLUMNS);
        builder.push(" WHERE api_keys.is_standalone = 1 ORDER BY api_keys.id ASC");
        self.fetch_export_rows(builder).await
    }
}

#[async_trait]
impl AuthApiKeyWriteRepository for SqliteAuthApiKeyReadRepository {
    async fn touch_last_used_at(&self, api_key_id: &str) -> Result<bool, DataLayerError> {
        let now = current_unix_secs() as i64;
        let rows_affected = sqlx::query(
            r#"
UPDATE api_keys
SET last_used_at = ?, updated_at = ?
WHERE id = ?
"#,
        )
        .bind(now)
        .bind(now)
        .bind(api_key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    async fn create_user_api_key(
        &self,
        record: CreateUserApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.create_api_key(CreateApiKeyInsertRecord {
            user_id: record.user_id,
            api_key_id: record.api_key_id,
            key_hash: record.key_hash,
            key_encrypted: record.key_encrypted,
            name: record.name,
            allowed_providers: record.allowed_providers,
            allowed_api_formats: record.allowed_api_formats,
            allowed_models: record.allowed_models,
            rate_limit: Some(record.rate_limit),
            concurrent_limit: record.concurrent_limit,
            force_capabilities: record.force_capabilities,
            is_active: record.is_active,
            expires_at_unix_secs: record.expires_at_unix_secs,
            auto_delete_on_expiry: record.auto_delete_on_expiry,
            total_requests: record.total_requests,
            total_tokens: record.total_tokens,
            total_cost_usd: record.total_cost_usd,
            is_standalone: false,
        })
        .await
    }

    async fn create_standalone_api_key(
        &self,
        record: CreateStandaloneApiKeyRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.create_api_key(CreateApiKeyInsertRecord {
            user_id: record.user_id,
            api_key_id: record.api_key_id,
            key_hash: record.key_hash,
            key_encrypted: record.key_encrypted,
            name: record.name,
            allowed_providers: record.allowed_providers,
            allowed_api_formats: record.allowed_api_formats,
            allowed_models: record.allowed_models,
            rate_limit: record.rate_limit,
            concurrent_limit: record.concurrent_limit,
            force_capabilities: record.force_capabilities,
            is_active: record.is_active,
            expires_at_unix_secs: record.expires_at_unix_secs,
            auto_delete_on_expiry: record.auto_delete_on_expiry,
            total_requests: record.total_requests,
            total_tokens: record.total_tokens,
            total_cost_usd: record.total_cost_usd,
            is_standalone: true,
        })
        .await
    }

    async fn update_user_api_key_basic(
        &self,
        record: UpdateUserApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let now = current_unix_secs() as i64;
        sqlx::query(
            r#"
UPDATE api_keys
SET name = COALESCE(?, name),
    rate_limit = COALESCE(?, rate_limit),
    concurrent_limit = COALESCE(?, concurrent_limit),
    updated_at = ?
WHERE id = ?
  AND user_id = ?
  AND is_standalone = 0
"#,
        )
        .bind(record.name.as_deref())
        .bind(record.rate_limit)
        .bind(record.concurrent_limit)
        .bind(now)
        .bind(&record.api_key_id)
        .bind(&record.user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_export_by_id(&record.api_key_id).await
    }

    async fn update_standalone_api_key_basic(
        &self,
        record: UpdateStandaloneApiKeyBasicRecord,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let now = current_unix_secs() as i64;
        sqlx::query(
            r#"
UPDATE api_keys
SET name = COALESCE(?, name),
    rate_limit = CASE WHEN ? THEN ? ELSE rate_limit END,
    concurrent_limit = CASE WHEN ? THEN ? ELSE concurrent_limit END,
    allowed_providers = CASE WHEN ? THEN ? ELSE allowed_providers END,
    allowed_api_formats = CASE WHEN ? THEN ? ELSE allowed_api_formats END,
    allowed_models = CASE WHEN ? THEN ? ELSE allowed_models END,
    expires_at = CASE WHEN ? THEN ? ELSE expires_at END,
    auto_delete_on_expiry = CASE WHEN ? THEN ? ELSE auto_delete_on_expiry END,
    updated_at = ?
WHERE id = ?
  AND is_standalone = 1
"#,
        )
        .bind(record.name.as_deref())
        .bind(record.rate_limit_present)
        .bind(record.rate_limit)
        .bind(record.concurrent_limit_present)
        .bind(record.concurrent_limit)
        .bind(record.allowed_providers.is_some())
        .bind(json_string_from_nested_string_list(
            &record.allowed_providers,
            "api_keys.allowed_providers",
        )?)
        .bind(record.allowed_api_formats.is_some())
        .bind(json_string_from_nested_string_list(
            &record.allowed_api_formats,
            "api_keys.allowed_api_formats",
        )?)
        .bind(record.allowed_models.is_some())
        .bind(json_string_from_nested_string_list(
            &record.allowed_models,
            "api_keys.allowed_models",
        )?)
        .bind(record.expires_at_present)
        .bind(optional_i64_from_u64(
            record.expires_at_unix_secs,
            "api_keys.expires_at",
        )?)
        .bind(record.auto_delete_on_expiry_present)
        .bind(record.auto_delete_on_expiry)
        .bind(now)
        .bind(&record.api_key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_export_by_id(&record.api_key_id).await
    }

    async fn set_user_api_key_active(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.set_active(api_key_id, Some(user_id), is_active, false)
            .await
    }

    async fn set_standalone_api_key_active(
        &self,
        api_key_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        self.set_active(api_key_id, None, is_active, true).await
    }

    async fn set_user_api_key_locked(
        &self,
        user_id: &str,
        api_key_id: &str,
        is_locked: bool,
    ) -> Result<bool, DataLayerError> {
        let rows_affected = sqlx::query(
            r#"
UPDATE api_keys
SET is_locked = ?, updated_at = ?
WHERE id = ?
  AND user_id = ?
  AND is_standalone = 0
"#,
        )
        .bind(is_locked)
        .bind(current_unix_secs() as i64)
        .bind(api_key_id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    async fn set_user_api_key_allowed_providers(
        &self,
        user_id: &str,
        api_key_id: &str,
        allowed_providers: Option<Vec<String>>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        sqlx::query(
            r#"
UPDATE api_keys
SET allowed_providers = ?, updated_at = ?
WHERE id = ?
  AND user_id = ?
  AND is_standalone = 0
"#,
        )
        .bind(json_string_from_string_list(
            allowed_providers.as_ref(),
            "api_keys.allowed_providers",
        )?)
        .bind(current_unix_secs() as i64)
        .bind(api_key_id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_export_by_id(api_key_id).await
    }

    async fn set_user_api_key_force_capabilities(
        &self,
        user_id: &str,
        api_key_id: &str,
        force_capabilities: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        sqlx::query(
            r#"
UPDATE api_keys
SET force_capabilities = ?, updated_at = ?
WHERE id = ?
  AND user_id = ?
  AND is_standalone = 0
"#,
        )
        .bind(optional_json_to_string(
            &force_capabilities,
            "api_keys.force_capabilities",
        )?)
        .bind(current_unix_secs() as i64)
        .bind(api_key_id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_export_by_id(api_key_id).await
    }

    async fn set_user_api_key_feature_settings(
        &self,
        user_id: &str,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        sqlx::query(
            r#"
UPDATE api_keys
SET feature_settings = ?, updated_at = ?
WHERE id = ?
  AND user_id = ?
  AND is_standalone = 0
"#,
        )
        .bind(optional_json_to_string(
            &feature_settings,
            "api_keys.feature_settings",
        )?)
        .bind(current_unix_secs() as i64)
        .bind(api_key_id)
        .bind(user_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_export_by_id(api_key_id).await
    }

    async fn delete_user_api_key(
        &self,
        user_id: &str,
        api_key_id: &str,
    ) -> Result<bool, DataLayerError> {
        self.delete_api_key(api_key_id, Some(user_id), false).await
    }

    async fn delete_standalone_api_key(&self, api_key_id: &str) -> Result<bool, DataLayerError> {
        self.delete_api_key(api_key_id, None, true).await
    }

    async fn set_standalone_api_key_feature_settings(
        &self,
        api_key_id: &str,
        feature_settings: Option<serde_json::Value>,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        sqlx::query(
            r#"
UPDATE api_keys
SET feature_settings = ?, updated_at = ?
WHERE id = ?
  AND is_standalone = 1
"#,
        )
        .bind(optional_json_to_string(
            &feature_settings,
            "api_keys.feature_settings",
        )?)
        .bind(current_unix_secs() as i64)
        .bind(api_key_id)
        .execute(&self.pool)
        .await
        .map_sql_err()?;
        self.reload_export_by_id(api_key_id).await
    }
}

impl SqliteAuthApiKeyReadRepository {
    async fn set_active(
        &self,
        api_key_id: &str,
        user_id: Option<&str>,
        is_active: bool,
        is_standalone: bool,
    ) -> Result<Option<StoredAuthApiKeyExportRecord>, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new("UPDATE api_keys SET is_active = ");
        builder
            .push_bind(is_active)
            .push(", updated_at = ")
            .push_bind(current_unix_secs() as i64)
            .push(" WHERE id = ")
            .push_bind(api_key_id)
            .push(" AND is_standalone = ")
            .push_bind(is_standalone);
        if let Some(user_id) = user_id {
            builder.push(" AND user_id = ").push_bind(user_id);
        }
        builder.build().execute(&self.pool).await.map_sql_err()?;
        self.reload_export_by_id(api_key_id).await
    }

    async fn delete_api_key(
        &self,
        api_key_id: &str,
        user_id: Option<&str>,
        is_standalone: bool,
    ) -> Result<bool, DataLayerError> {
        let mut builder = QueryBuilder::<Sqlite>::new("DELETE FROM api_keys WHERE id = ");
        builder
            .push_bind(api_key_id)
            .push(" AND is_standalone = ")
            .push_bind(is_standalone);
        if let Some(user_id) = user_id {
            builder.push(" AND user_id = ").push_bind(user_id);
        }
        let rows_affected = builder
            .build()
            .execute(&self.pool)
            .await
            .map_sql_err()?
            .rows_affected();
        Ok(rows_affected > 0)
    }
}

fn push_in_clause<'args>(
    builder: &mut QueryBuilder<'args, Sqlite>,
    prefix: &str,
    values: &'args [String],
) {
    builder.push(prefix);
    {
        let mut separated = builder.separated(", ");
        for value in values {
            separated.push_bind(value);
        }
    }
    builder.push(")");
}

async fn summarize_api_keys(
    pool: &SqlitePool,
    is_standalone: bool,
    now_unix_secs: u64,
) -> Result<AuthApiKeyExportSummary, DataLayerError> {
    let row = sqlx::query(
        r#"
SELECT
  COUNT(*) AS total,
  SUM(CASE WHEN is_active = 1 AND (expires_at IS NULL OR expires_at >= ?) THEN 1 ELSE 0 END) AS active
FROM api_keys
WHERE is_standalone = ?
"#,
    )
    .bind(now_unix_secs as i64)
    .bind(is_standalone)
    .fetch_one(pool)
    .await
    .map_sql_err()?;
    summarize_row(row)
}

fn summarize_row(row: SqliteRow) -> Result<AuthApiKeyExportSummary, DataLayerError> {
    Ok(AuthApiKeyExportSummary {
        total: row.try_get::<i64, _>("total").map_sql_err()?.max(0) as u64,
        active: row
            .try_get::<Option<i64>, _>("active")
            .map_sql_err()?
            .unwrap_or(0)
            .max(0) as u64,
    })
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

fn current_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn i64_from_u64(value: u64, field_name: &str) -> Result<i64, DataLayerError> {
    i64::try_from(value)
        .map_err(|_| DataLayerError::InvalidInput(format!("{field_name} exceeds i64: {value}")))
}

fn optional_i64_from_u64(
    value: Option<u64>,
    field_name: &str,
) -> Result<Option<i64>, DataLayerError> {
    value
        .map(|value| i64_from_u64(value, field_name))
        .transpose()
}

fn optional_json_to_string(
    value: &Option<serde_json::Value>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    value
        .as_ref()
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains unserializable JSON: {err}"
                ))
            })
        })
        .transpose()
}

fn json_string_from_string_list(
    value: Option<&Vec<String>>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "{field_name} contains unserializable string list: {err}"
                ))
            })
        })
        .transpose()
}

fn json_string_from_nested_string_list(
    value: &Option<Option<Vec<String>>>,
    field_name: &str,
) -> Result<Option<String>, DataLayerError> {
    match value {
        Some(Some(values)) => json_string_from_string_list(Some(values), field_name),
        Some(None) | None => Ok(None),
    }
}

fn map_auth_api_key_snapshot_row(
    row: &SqliteRow,
) -> Result<StoredAuthApiKeySnapshot, DataLayerError> {
    let snapshot = StoredAuthApiKeySnapshot::new(
        row.try_get("user_id").map_sql_err()?,
        row.try_get("username").map_sql_err()?,
        row.try_get("email").map_sql_err()?,
        row.try_get("user_role").map_sql_err()?,
        row.try_get("user_auth_source").map_sql_err()?,
        row.try_get("user_is_active").map_sql_err()?,
        row.try_get("user_is_deleted").map_sql_err()?,
        optional_json_from_string(
            row.try_get("user_allowed_providers").map_sql_err()?,
            "users.allowed_providers",
        )?,
        optional_json_from_string(
            row.try_get("user_allowed_api_formats").map_sql_err()?,
            "users.allowed_api_formats",
        )?,
        optional_json_from_string(
            row.try_get("user_allowed_models").map_sql_err()?,
            "users.allowed_models",
        )?,
        row.try_get("api_key_id").map_sql_err()?,
        row.try_get("api_key_name").map_sql_err()?,
        row.try_get("api_key_is_active").map_sql_err()?,
        row.try_get("api_key_is_locked").map_sql_err()?,
        row.try_get("api_key_is_standalone").map_sql_err()?,
        row.try_get("api_key_rate_limit").map_sql_err()?,
        row.try_get("api_key_concurrent_limit").map_sql_err()?,
        row.try_get("api_key_expires_at_unix_secs").map_sql_err()?,
        optional_json_from_string(
            row.try_get("api_key_allowed_providers").map_sql_err()?,
            "api_keys.allowed_providers",
        )?,
        optional_json_from_string(
            row.try_get("api_key_allowed_api_formats").map_sql_err()?,
            "api_keys.allowed_api_formats",
        )?,
        optional_json_from_string(
            row.try_get("api_key_allowed_models").map_sql_err()?,
            "api_keys.allowed_models",
        )?,
    )?;
    Ok(snapshot.with_user_rate_limit(row.try_get("user_rate_limit").map_sql_err()?))
}

fn map_auth_api_key_export_row(
    row: &SqliteRow,
) -> Result<StoredAuthApiKeyExportRecord, DataLayerError> {
    let feature_settings = optional_json_from_string(
        row.try_get("feature_settings").map_sql_err()?,
        "api_keys.feature_settings",
    )?;
    StoredAuthApiKeyExportRecord::new(
        row.try_get("user_id").map_sql_err()?,
        row.try_get("api_key_id").map_sql_err()?,
        row.try_get("key_hash").map_sql_err()?,
        row.try_get("key_encrypted").map_sql_err()?,
        row.try_get("name").map_sql_err()?,
        optional_json_from_string(
            row.try_get("allowed_providers").map_sql_err()?,
            "api_keys.allowed_providers",
        )?,
        optional_json_from_string(
            row.try_get("allowed_api_formats").map_sql_err()?,
            "api_keys.allowed_api_formats",
        )?,
        optional_json_from_string(
            row.try_get("allowed_models").map_sql_err()?,
            "api_keys.allowed_models",
        )?,
        row.try_get("rate_limit").map_sql_err()?,
        row.try_get("concurrent_limit").map_sql_err()?,
        optional_json_from_string(
            row.try_get("force_capabilities").map_sql_err()?,
            "api_keys.force_capabilities",
        )?,
        row.try_get("is_active").map_sql_err()?,
        row.try_get("expires_at_unix_secs").map_sql_err()?,
        row.try_get("auto_delete_on_expiry").map_sql_err()?,
        row.try_get("total_requests").map_sql_err()?,
        row.try_get("total_tokens").map_sql_err()?,
        sqlite_real(row, "total_cost_usd")?,
        row.try_get("is_standalone").map_sql_err()?,
    )
    .map(|record| record.with_feature_settings(feature_settings))
    .and_then(|record| {
        record.with_activity_timestamps(
            row.try_get("last_used_at_unix_secs").map_sql_err()?,
            row.try_get("created_at_unix_secs").map_sql_err()?,
            row.try_get("updated_at_unix_secs").map_sql_err()?,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::SqliteAuthApiKeyReadRepository;
    use crate::lifecycle::migrate::run_sqlite_migrations;
    use crate::repository::auth::{
        AuthApiKeyLookupKey, AuthApiKeyReadRepository, AuthApiKeyWriteRepository,
        CreateStandaloneApiKeyRecord, CreateUserApiKeyRecord, StandaloneApiKeyExportListQuery,
        UpdateStandaloneApiKeyBasicRecord, UpdateUserApiKeyBasicRecord,
    };
    use serde_json::json;

    #[tokio::test]
    async fn sqlite_repository_reads_auth_api_key_contract_views() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_auth_api_key_rows(&pool).await;

        let repository = SqliteAuthApiKeyReadRepository::new(pool);
        let snapshot = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::KeyHash("hash-user"))
            .await
            .expect("snapshot lookup should run")
            .expect("snapshot should exist");
        assert_eq!(snapshot.user_id, "user-1");
        assert_eq!(
            snapshot.api_key_allowed_models,
            Some(vec!["gpt-4.1".to_string()])
        );

        let by_ids = repository
            .list_api_key_snapshots_by_ids(&["key-user".to_string(), "key-standalone".to_string()])
            .await
            .expect("snapshot list should run");
        assert_eq!(by_ids.len(), 2);

        let user_exports = repository
            .list_export_api_keys_by_user_ids(&["user-1".to_string()])
            .await
            .expect("user exports should load");
        assert_eq!(user_exports.len(), 1);
        assert_eq!(user_exports[0].total_tokens, 456);

        let page = repository
            .list_export_standalone_api_keys_page(&StandaloneApiKeyExportListQuery {
                skip: 0,
                limit: 10,
                is_active: Some(true),
            })
            .await
            .expect("standalone page should load");
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].api_key_id, "key-standalone");

        let count = repository
            .count_export_standalone_api_keys(Some(true))
            .await
            .expect("standalone count should load");
        assert_eq!(count, 1);

        let summary = repository
            .summarize_export_api_keys_by_user_ids(&["user-1".to_string()], 100)
            .await
            .expect("summary should load");
        assert_eq!(summary.total, 1);
        assert_eq!(summary.active, 1);

        assert_eq!(
            repository
                .find_export_standalone_api_key_by_id("key-standalone")
                .await
                .expect("standalone find should run")
                .expect("standalone should exist")
                .name,
            Some("Standalone".to_string())
        );
    }

    #[tokio::test]
    async fn sqlite_repository_writes_auth_api_key_contract_views() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_sqlite_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        seed_auth_user(&pool).await;

        let repository = SqliteAuthApiKeyReadRepository::new(pool);
        let user_key = repository
            .create_user_api_key(CreateUserApiKeyRecord {
                user_id: "user-1".to_string(),
                api_key_id: "key-created-user".to_string(),
                key_hash: "hash-created-user".to_string(),
                key_encrypted: Some("enc-user".to_string()),
                name: Some("Created User".to_string()),
                allowed_providers: Some(vec!["openai".to_string()]),
                allowed_api_formats: Some(vec!["openai:chat".to_string()]),
                allowed_models: Some(vec!["gpt-4.1".to_string()]),
                rate_limit: 100,
                concurrent_limit: Some(5),
                force_capabilities: Some(json!({"cache": true})),
                is_active: true,
                expires_at_unix_secs: Some(2_000_000_000),
                auto_delete_on_expiry: false,
                total_requests: 3,
                total_tokens: 42,
                total_cost_usd: 0.25,
            })
            .await
            .expect("user key should create")
            .expect("user key should reload");
        assert_eq!(user_key.allowed_models, Some(vec!["gpt-4.1".to_string()]));
        assert_eq!(user_key.total_tokens, 42);

        let updated_user_key = repository
            .update_user_api_key_basic(UpdateUserApiKeyBasicRecord {
                user_id: "user-1".to_string(),
                api_key_id: "key-created-user".to_string(),
                name: Some("Updated User".to_string()),
                rate_limit: Some(150),
                concurrent_limit: Some(6),
            })
            .await
            .expect("user key should update")
            .expect("user key should reload");
        assert_eq!(updated_user_key.name, Some("Updated User".to_string()));
        assert_eq!(updated_user_key.concurrent_limit, Some(6));

        assert!(repository
            .set_user_api_key_locked("user-1", "key-created-user", true)
            .await
            .expect("lock should update"));
        let snapshot = repository
            .find_api_key_snapshot(AuthApiKeyLookupKey::ApiKeyId("key-created-user"))
            .await
            .expect("snapshot should load")
            .expect("snapshot should exist");
        assert!(snapshot.api_key_is_locked);

        let active_user_key = repository
            .set_user_api_key_active("user-1", "key-created-user", false)
            .await
            .expect("active flag should update")
            .expect("user key should reload");
        assert!(!active_user_key.is_active);

        let provider_updated = repository
            .set_user_api_key_allowed_providers(
                "user-1",
                "key-created-user",
                Some(vec!["anthropic".to_string()]),
            )
            .await
            .expect("allowed providers should update")
            .expect("user key should reload");
        assert_eq!(
            provider_updated.allowed_providers,
            Some(vec!["anthropic".to_string()])
        );

        let capabilities_updated = repository
            .set_user_api_key_force_capabilities(
                "user-1",
                "key-created-user",
                Some(json!({"vision": true})),
            )
            .await
            .expect("force capabilities should update")
            .expect("user key should reload");
        assert_eq!(
            capabilities_updated.force_capabilities,
            Some(json!({"vision": true}))
        );

        assert!(repository
            .touch_last_used_at("key-created-user")
            .await
            .expect("touch should update"));
        assert!(repository
            .reload_export_by_id("key-created-user")
            .await
            .expect("touched key should reload")
            .expect("touched key should exist")
            .last_used_at_unix_secs
            .is_some());

        let standalone = repository
            .create_standalone_api_key(CreateStandaloneApiKeyRecord {
                user_id: "user-1".to_string(),
                api_key_id: "key-created-standalone".to_string(),
                key_hash: "hash-created-standalone".to_string(),
                key_encrypted: Some("enc-standalone".to_string()),
                name: Some("Created Standalone".to_string()),
                allowed_providers: Some(vec!["openai".to_string()]),
                allowed_api_formats: None,
                allowed_models: None,
                rate_limit: None,
                concurrent_limit: Some(2),
                force_capabilities: None,
                is_active: true,
                expires_at_unix_secs: None,
                auto_delete_on_expiry: false,
                total_requests: 0,
                total_tokens: 0,
                total_cost_usd: 0.0,
            })
            .await
            .expect("standalone key should create")
            .expect("standalone key should reload");
        assert!(standalone.is_standalone);

        let standalone = repository
            .update_standalone_api_key_basic(UpdateStandaloneApiKeyBasicRecord {
                api_key_id: "key-created-standalone".to_string(),
                name: Some("Updated Standalone".to_string()),
                rate_limit_present: true,
                rate_limit: Some(20),
                concurrent_limit_present: true,
                concurrent_limit: None,
                allowed_providers: Some(None),
                allowed_api_formats: Some(Some(vec!["openai:responses".to_string()])),
                allowed_models: Some(Some(vec!["gpt-4.1-mini".to_string()])),
                expires_at_present: true,
                expires_at_unix_secs: Some(2_100_000_000),
                auto_delete_on_expiry_present: true,
                auto_delete_on_expiry: true,
            })
            .await
            .expect("standalone key should update")
            .expect("standalone key should reload");
        assert_eq!(standalone.name, Some("Updated Standalone".to_string()));
        assert_eq!(standalone.allowed_providers, None);
        assert_eq!(
            standalone.allowed_api_formats,
            Some(vec!["openai:responses".to_string()])
        );
        assert_eq!(standalone.concurrent_limit, None);
        assert!(standalone.auto_delete_on_expiry);

        let standalone = repository
            .set_standalone_api_key_active("key-created-standalone", false)
            .await
            .expect("standalone active flag should update")
            .expect("standalone key should reload");
        assert!(!standalone.is_active);

        assert!(repository
            .delete_standalone_api_key("key-created-standalone")
            .await
            .expect("standalone key should delete"));
        assert!(repository
            .delete_user_api_key("user-1", "key-created-user")
            .await
            .expect("user key should delete"));
    }

    async fn seed_auth_api_key_rows(pool: &sqlx::SqlitePool) {
        seed_auth_user(pool).await;
        sqlx::query(
            r#"
INSERT INTO api_keys (
  id, user_id, key_hash, key_encrypted, name, allowed_providers,
  allowed_api_formats, allowed_models, rate_limit, concurrent_limit,
  force_capabilities, is_active, expires_at, auto_delete_on_expiry,
  total_requests, total_tokens, total_cost_usd, last_used_at, created_at,
  updated_at, is_standalone
) VALUES
  (
    'key-user', 'user-1', 'hash-user', 'enc-user', 'User Key', '["openai"]',
    '["openai:chat"]', '["gpt-4.1"]', 30, 2, '{"cache":true}', 1, 200, 0,
    123, 456, 1.25, 10, 1, 2, 0
  ),
  (
    'key-standalone', 'user-1', 'hash-standalone', 'enc-standalone', 'Standalone', NULL,
    NULL, NULL, NULL, NULL, NULL, 1, NULL, 0, 0, 0, 0, NULL, 3, 4, 1
  )
"#,
        )
        .execute(pool)
        .await
        .expect("api keys should seed");
    }

    async fn seed_auth_user(pool: &sqlx::SqlitePool) {
        sqlx::query(
            r#"
INSERT INTO users (
  id, email, email_verified, username, password_hash, role, auth_source,
  allowed_providers, allowed_api_formats, allowed_models, rate_limit,
  is_active, is_deleted, created_at, updated_at
) VALUES (
  'user-1', 'user@example.com', 1, 'alice', NULL, 'user', 'local',
  '["openai"]', '["openai:chat"]', '["gpt-4.1"]', 60, 1, 0, 1, 1
)
"#,
        )
        .execute(pool)
        .await
        .expect("user should seed");
    }
}
