use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::{postgres::PgRow, PgPool, Row};

use super::types::{
    CreateManagementTokenRecord, ManagementTokenListQuery, ManagementTokenReadRepository,
    ManagementTokenWriteRepository, RegenerateManagementTokenSecret, StoredManagementToken,
    StoredManagementTokenListPage, StoredManagementTokenUserSummary, StoredManagementTokenWithUser,
    UpdateManagementTokenRecord,
};
use crate::{error::SqlxResultExt, DataLayerError};

const LIST_MANAGEMENT_TOKENS_SQL: &str = r#"
SELECT
  mt.id,
  mt.user_id,
  mt.name,
  mt.description,
  mt.token_prefix,
  mt.allowed_ips,
  EXTRACT(EPOCH FROM mt.expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM mt.last_used_at)::bigint AS last_used_at_unix_secs,
  mt.last_used_ip,
  COALESCE(mt.usage_count, 0) AS usage_count,
  mt.is_active,
  EXTRACT(EPOCH FROM mt.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM mt.updated_at)::bigint AS updated_at_unix_secs,
  u.id AS user_row_id,
  u.email AS user_email,
  u.username AS user_username,
  u.role::text AS user_role
FROM management_tokens mt
JOIN users u ON u.id = mt.user_id
WHERE ($1::text IS NULL OR mt.user_id = $1)
  AND ($2::boolean IS NULL OR mt.is_active = $2)
ORDER BY mt.created_at DESC, mt.id DESC
OFFSET $3
LIMIT $4
"#;

const COUNT_MANAGEMENT_TOKENS_SQL: &str = r#"
SELECT COUNT(mt.id) AS total
FROM management_tokens mt
WHERE ($1::text IS NULL OR mt.user_id = $1)
  AND ($2::boolean IS NULL OR mt.is_active = $2)
"#;

const GET_MANAGEMENT_TOKEN_WITH_USER_SQL: &str = r#"
SELECT
  mt.id,
  mt.user_id,
  mt.name,
  mt.description,
  mt.token_prefix,
  mt.allowed_ips,
  EXTRACT(EPOCH FROM mt.expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM mt.last_used_at)::bigint AS last_used_at_unix_secs,
  mt.last_used_ip,
  COALESCE(mt.usage_count, 0) AS usage_count,
  mt.is_active,
  EXTRACT(EPOCH FROM mt.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM mt.updated_at)::bigint AS updated_at_unix_secs,
  u.id AS user_row_id,
  u.email AS user_email,
  u.username AS user_username,
  u.role::text AS user_role
FROM management_tokens mt
JOIN users u ON u.id = mt.user_id
WHERE mt.id = $1
LIMIT 1
"#;

const GET_MANAGEMENT_TOKEN_WITH_USER_BY_HASH_SQL: &str = r#"
SELECT
  mt.id,
  mt.user_id,
  mt.name,
  mt.description,
  mt.token_prefix,
  mt.allowed_ips,
  EXTRACT(EPOCH FROM mt.expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM mt.last_used_at)::bigint AS last_used_at_unix_secs,
  mt.last_used_ip,
  COALESCE(mt.usage_count, 0) AS usage_count,
  mt.is_active,
  EXTRACT(EPOCH FROM mt.created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM mt.updated_at)::bigint AS updated_at_unix_secs,
  u.id AS user_row_id,
  u.email AS user_email,
  u.username AS user_username,
  u.role::text AS user_role
FROM management_tokens mt
JOIN users u ON u.id = mt.user_id
WHERE mt.token_hash = $1
LIMIT 1
"#;

const DELETE_MANAGEMENT_TOKEN_SQL: &str = r#"
DELETE FROM management_tokens
WHERE id = $1
"#;

const CREATE_MANAGEMENT_TOKEN_SQL: &str = r#"
INSERT INTO management_tokens (
  id,
  user_id,
  token_hash,
  token_prefix,
  name,
  description,
  allowed_ips,
  expires_at,
  is_active
)
VALUES (
  $1,
  $2,
  $3,
  $4,
  $5,
  $6,
  $7,
  CASE
    WHEN $8::bigint IS NULL THEN NULL
    ELSE to_timestamp($8::double precision)
  END,
  $9
)
RETURNING
  id,
  user_id,
  name,
  description,
  token_prefix,
  allowed_ips,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  last_used_ip,
  COALESCE(usage_count, 0) AS usage_count,
  is_active,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const UPDATE_MANAGEMENT_TOKEN_SQL: &str = r#"
UPDATE management_tokens
SET name = COALESCE($2, name),
    description = CASE
      WHEN $3 THEN NULL
      WHEN $4::text IS NULL THEN description
      ELSE $4
    END,
    allowed_ips = CASE
      WHEN $5 THEN NULL
      WHEN $6::json IS NULL THEN allowed_ips
      ELSE $6
    END,
    expires_at = CASE
      WHEN $7 THEN NULL
      WHEN $8::bigint IS NULL THEN expires_at
      ELSE to_timestamp($8::double precision)
    END,
    is_active = COALESCE($9, is_active),
    updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  name,
  description,
  token_prefix,
  allowed_ips,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  last_used_ip,
  COALESCE(usage_count, 0) AS usage_count,
  is_active,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const SET_MANAGEMENT_TOKEN_ACTIVE_SQL: &str = r#"
UPDATE management_tokens
SET is_active = $2,
    updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  name,
  description,
  token_prefix,
  allowed_ips,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  last_used_ip,
  COALESCE(usage_count, 0) AS usage_count,
  is_active,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const REGENERATE_MANAGEMENT_TOKEN_SECRET_SQL: &str = r#"
UPDATE management_tokens
SET token_hash = $2,
    token_prefix = $3,
    updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  name,
  description,
  token_prefix,
  allowed_ips,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  last_used_ip,
  COALESCE(usage_count, 0) AS usage_count,
  is_active,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const RECORD_MANAGEMENT_TOKEN_USAGE_SQL: &str = r#"
UPDATE management_tokens
SET last_used_at = NOW(),
    last_used_ip = $2,
    usage_count = COALESCE(usage_count, 0) + 1,
    updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  name,
  description,
  token_prefix,
  allowed_ips,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  last_used_ip,
  COALESCE(usage_count, 0) AS usage_count,
  is_active,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

#[derive(Debug, Clone)]
pub struct SqlxManagementTokenRepository {
    pool: PgPool,
}

impl SqlxManagementTokenRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ManagementTokenReadRepository for SqlxManagementTokenRepository {
    async fn list_management_tokens(
        &self,
        query: &ManagementTokenListQuery,
    ) -> Result<StoredManagementTokenListPage, DataLayerError> {
        let count_row = sqlx::query(COUNT_MANAGEMENT_TOKENS_SQL)
            .bind(query.user_id.as_deref())
            .bind(query.is_active)
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;
        let total = count_row.try_get::<i64, _>("total").map_postgres_err()?;

        let mut rows = sqlx::query(LIST_MANAGEMENT_TOKENS_SQL)
            .bind(query.user_id.as_deref())
            .bind(query.is_active)
            .bind(i64::try_from(query.offset).unwrap_or(i64::MAX))
            .bind(i64::try_from(query.limit).unwrap_or(i64::MAX))
            .fetch(&self.pool);
        let mut items = Vec::new();
        while let Some(row) = rows.try_next().await.map_postgres_err()? {
            items.push(map_token_with_user_row(&row)?);
        }

        Ok(StoredManagementTokenListPage {
            items,
            total: usize::try_from(total.max(0)).unwrap_or(usize::MAX),
        })
    }

    async fn get_management_token_with_user(
        &self,
        token_id: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        let row = sqlx::query(GET_MANAGEMENT_TOKEN_WITH_USER_SQL)
            .bind(token_id)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_token_with_user_row).transpose()
    }

    async fn get_management_token_with_user_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        let row = sqlx::query(GET_MANAGEMENT_TOKEN_WITH_USER_BY_HASH_SQL)
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_token_with_user_row).transpose()
    }
}

#[async_trait]
impl ManagementTokenWriteRepository for SqlxManagementTokenRepository {
    async fn create_management_token(
        &self,
        record: &CreateManagementTokenRecord,
    ) -> Result<StoredManagementToken, DataLayerError> {
        record.validate()?;
        let row = sqlx::query(CREATE_MANAGEMENT_TOKEN_SQL)
            .bind(&record.id)
            .bind(&record.user_id)
            .bind(&record.token_hash)
            .bind(record.token_prefix.as_deref())
            .bind(&record.name)
            .bind(record.description.as_deref())
            .bind(record.allowed_ips.as_ref())
            .bind(
                record
                    .expires_at_unix_secs
                    .and_then(|value| i64::try_from(value).ok()),
            )
            .bind(record.is_active)
            .fetch_one(&self.pool)
            .await
            .map_err(|err| map_management_token_write_error(err, Some(record.name.as_str())))?;
        map_token_row(&row)
    }

    async fn update_management_token(
        &self,
        record: &UpdateManagementTokenRecord,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        record.validate()?;
        let row = sqlx::query(UPDATE_MANAGEMENT_TOKEN_SQL)
            .bind(&record.token_id)
            .bind(record.name.as_deref())
            .bind(record.clear_description)
            .bind(record.description.as_deref())
            .bind(record.clear_allowed_ips)
            .bind(record.allowed_ips.as_ref())
            .bind(record.clear_expires_at)
            .bind(
                record
                    .expires_at_unix_secs
                    .and_then(|value| i64::try_from(value).ok()),
            )
            .bind(record.is_active)
            .fetch_optional(&self.pool)
            .await
            .map_err(|err| map_management_token_write_error(err, record.name.as_deref()))?;
        row.as_ref().map(map_token_row).transpose()
    }

    async fn delete_management_token(&self, token_id: &str) -> Result<bool, DataLayerError> {
        let result = sqlx::query(DELETE_MANAGEMENT_TOKEN_SQL)
            .bind(token_id)
            .execute(&self.pool)
            .await
            .map_postgres_err()?;
        Ok(result.rows_affected() > 0)
    }

    async fn set_management_token_active(
        &self,
        token_id: &str,
        is_active: bool,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        let row = sqlx::query(SET_MANAGEMENT_TOKEN_ACTIVE_SQL)
            .bind(token_id)
            .bind(is_active)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_token_row).transpose()
    }

    async fn regenerate_management_token_secret(
        &self,
        mutation: &RegenerateManagementTokenSecret,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        mutation.validate()?;
        let row = sqlx::query(REGENERATE_MANAGEMENT_TOKEN_SECRET_SQL)
            .bind(&mutation.token_id)
            .bind(&mutation.token_hash)
            .bind(mutation.token_prefix.as_deref())
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_token_row).transpose()
    }

    async fn record_management_token_usage(
        &self,
        token_id: &str,
        last_used_ip: Option<&str>,
    ) -> Result<Option<StoredManagementToken>, DataLayerError> {
        let row = sqlx::query(RECORD_MANAGEMENT_TOKEN_USAGE_SQL)
            .bind(token_id)
            .bind(last_used_ip)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_token_row).transpose()
    }
}

fn optional_unix_secs(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

fn map_management_token_write_error(
    err: sqlx::Error,
    requested_name: Option<&str>,
) -> DataLayerError {
    let conflict = err.as_database_error().and_then(|db_err| {
        let code = db_err.code().map(|value| value.as_ref().to_string());
        let constraint = db_err.constraint().map(|value| value.to_string());
        match (code.as_deref(), constraint.as_deref()) {
            (Some("23505"), Some("uq_management_tokens_user_name")) => Some(
                requested_name
                    .map(|name| format!("已存在名为 '{}' 的 Token", name))
                    .unwrap_or_else(|| "Management Token 名称已存在".to_string()),
            ),
            (Some("23514"), Some("check_allowed_ips_not_empty")) => {
                Some("IP 白名单不能为空，如需取消限制请不提供此字段".to_string())
            }
            _ => None,
        }
    });

    match conflict {
        Some(detail) => DataLayerError::InvalidInput(detail),
        None => DataLayerError::Postgres(err.to_string()),
    }
}

fn map_token_row(row: &PgRow) -> Result<StoredManagementToken, DataLayerError> {
    Ok(StoredManagementToken::new(
        row.try_get("id").map_postgres_err()?,
        row.try_get("user_id").map_postgres_err()?,
        row.try_get("name").map_postgres_err()?,
    )?
    .with_display_fields(
        row.try_get("description").map_postgres_err()?,
        row.try_get("token_prefix").map_postgres_err()?,
        row.try_get("allowed_ips").map_postgres_err()?,
    )
    .with_runtime_fields(
        optional_unix_secs(row.try_get("expires_at_unix_secs").map_postgres_err()?),
        optional_unix_secs(row.try_get("last_used_at_unix_secs").map_postgres_err()?),
        row.try_get("last_used_ip").map_postgres_err()?,
        u64::try_from(row.try_get::<i32, _>("usage_count").map_postgres_err()?).unwrap_or(0),
        row.try_get("is_active").map_postgres_err()?,
    )
    .with_timestamps(
        optional_unix_secs(row.try_get("created_at_unix_ms").map_postgres_err()?),
        optional_unix_secs(row.try_get("updated_at_unix_secs").map_postgres_err()?),
    ))
}

fn map_user_summary_row(row: &PgRow) -> Result<StoredManagementTokenUserSummary, DataLayerError> {
    StoredManagementTokenUserSummary::new(
        row.try_get("user_row_id").map_postgres_err()?,
        row.try_get("user_email").map_postgres_err()?,
        row.try_get("user_username").map_postgres_err()?,
        row.try_get("user_role").map_postgres_err()?,
    )
}

fn map_token_with_user_row(row: &PgRow) -> Result<StoredManagementTokenWithUser, DataLayerError> {
    Ok(StoredManagementTokenWithUser::new(
        map_token_row(row)?,
        map_user_summary_row(row)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::SqlxManagementTokenRepository;
    use crate::postgres::{PostgresPoolConfig, PostgresPoolFactory};

    #[tokio::test]
    async fn repository_constructs_from_lazy_pool() {
        let factory = PostgresPoolFactory::new(PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        })
        .expect("factory should build");

        let pool = factory.connect_lazy().expect("pool should build");
        let _repository = SqlxManagementTokenRepository::new(pool);
    }
}
