use async_trait::async_trait;
use futures_util::TryStreamExt;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};

use super::types::{
    StoredUserAuthRecord, StoredUserExportRow, StoredUserSummary, UserExportListQuery,
    UserExportSummary, UserReadRepository,
};
use crate::{error::SqlxResultExt, DataLayerError};

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
  allowed_api_formats,
  allowed_models,
  rate_limit,
  model_capability_settings,
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
  allowed_api_formats,
  allowed_models,
  rate_limit,
  model_capability_settings,
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
  allowed_api_formats,
  allowed_models,
  rate_limit,
  model_capability_settings,
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
  allowed_api_formats,
  allowed_models,
  rate_limit,
  model_capability_settings,
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
  allowed_api_formats,
  allowed_models,
  is_active,
  is_deleted,
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
  allowed_api_formats,
  allowed_models,
  is_active,
  is_deleted,
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
  allowed_api_formats,
  allowed_models,
  is_active,
  is_deleted,
  created_at,
  last_login_at
FROM users
WHERE email = $1 OR username = $1
LIMIT 1
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

        builder
            .push(" ORDER BY id ASC OFFSET ")
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

    async fn summarize_export_users(&self) -> Result<UserExportSummary, DataLayerError> {
        self.summarize_export_users().await
    }

    async fn find_export_user_by_id(
        &self,
        user_id: &str,
    ) -> Result<Option<StoredUserExportRow>, DataLayerError> {
        self.find_export_user_by_id(user_id).await
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
}
