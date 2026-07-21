use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use aether_data_contracts::repository::management_tokens::{
    CreateManagementTokenRecord, ManagementTokenListQuery, ManagementTokenReadRepository,
    ManagementTokenWriteRepository, RegenerateManagementTokenSecret, StoredManagementToken,
    StoredManagementTokenListPage, StoredManagementTokenUserSummary, StoredManagementTokenWithUser,
    UpdateManagementTokenRecord,
};
use aether_data_contracts::DataLayerError;
use aether_data_query::{push_eq, push_limit, push_limit_offset, push_optional_eq, WhereClause};

use crate::error::SqlxResultExt;

const MANAGEMENT_TOKEN_WITH_USER_COLUMNS: &str = r#"
SELECT
  mt.id,
  mt.user_id,
  mt.name,
  mt.description,
  mt.token_prefix,
  mt.allowed_ips,
  mt.permissions,
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
"#;

const DELETE_MANAGEMENT_TOKEN_SQL: &str = r#"
DELETE FROM management_tokens
WHERE id = $1
"#;

const MANAGEMENT_TOKEN_JSON_COLUMN_TYPES_SQL: &str = r#"
SELECT column_name, udt_name
FROM information_schema.columns
WHERE table_schema = 'public'
  AND table_name = 'management_tokens'
  AND column_name IN ('allowed_ips', 'permissions')
"#;

const CREATE_MANAGEMENT_TOKEN_SQL_PREFIX: &str = r#"
INSERT INTO management_tokens (
  id,
  user_id,
  token_hash,
  token_prefix,
  name,
  description,
  allowed_ips,
  permissions,
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
"#;

const CREATE_MANAGEMENT_TOKEN_SQL_SUFFIX: &str = r#",
  CASE
    WHEN $9::bigint IS NULL THEN NULL
    ELSE to_timestamp($9::double precision)
  END,
  $10
)
RETURNING
  id,
  user_id,
  name,
  description,
  token_prefix,
  allowed_ips,
  permissions,
  EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at_unix_secs,
  EXTRACT(EPOCH FROM last_used_at)::bigint AS last_used_at_unix_secs,
  last_used_ip,
  COALESCE(usage_count, 0) AS usage_count,
  is_active,
  EXTRACT(EPOCH FROM created_at)::bigint AS created_at_unix_ms,
  EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_unix_secs
"#;

const UPDATE_MANAGEMENT_TOKEN_SQL_PREFIX: &str = r#"
UPDATE management_tokens
SET name = $2,
    description = $3,
    allowed_ips =
"#;

const UPDATE_MANAGEMENT_TOKEN_SQL_MIDDLE: &str = r#",
    permissions =
"#;

const UPDATE_MANAGEMENT_TOKEN_SQL_SUFFIX: &str = r#",
    expires_at = CASE
      WHEN $6::bigint IS NULL THEN NULL
      ELSE to_timestamp($6::double precision)
    END,
    is_active = $7,
    updated_at = NOW()
WHERE id = $1
RETURNING
  id,
  user_id,
  name,
  description,
  token_prefix,
  allowed_ips,
  permissions,
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
  permissions,
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
  permissions,
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
  permissions,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonColumnType {
    Json,
    Jsonb,
}

impl JsonColumnType {
    fn from_udt_name(value: &str) -> Option<Self> {
        match value {
            "json" => Some(Self::Json),
            "jsonb" => Some(Self::Jsonb),
            _ => None,
        }
    }

    fn sql_type(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Jsonb => "jsonb",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ManagementTokenJsonColumnTypes {
    allowed_ips: JsonColumnType,
    permissions: JsonColumnType,
}

impl SqlxManagementTokenRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn json_column_types(&self) -> Result<ManagementTokenJsonColumnTypes, DataLayerError> {
        let rows = sqlx::query(MANAGEMENT_TOKEN_JSON_COLUMN_TYPES_SQL)
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        let mut allowed_ips = None;
        let mut permissions = None;
        for row in rows {
            let column_name: String = row.try_get("column_name").map_postgres_err()?;
            let udt_name: String = row.try_get("udt_name").map_postgres_err()?;
            let Some(column_type) = JsonColumnType::from_udt_name(udt_name.as_str()) else {
                return Err(DataLayerError::UnexpectedValue(format!(
                    "unsupported management_tokens.{column_name} column type: {udt_name}"
                )));
            };
            match column_name.as_str() {
                "allowed_ips" => allowed_ips = Some(column_type),
                "permissions" => permissions = Some(column_type),
                _ => {}
            }
        }

        match (allowed_ips, permissions) {
            (Some(allowed_ips), Some(permissions)) => Ok(ManagementTokenJsonColumnTypes {
                allowed_ips,
                permissions,
            }),
            _ => Err(DataLayerError::UnexpectedValue(
                "management_tokens JSON column metadata missing".to_string(),
            )),
        }
    }
}

fn create_management_token_sql(types: ManagementTokenJsonColumnTypes) -> String {
    format!(
        "{}  $7::text::{},\n  $8::text::{}{}",
        CREATE_MANAGEMENT_TOKEN_SQL_PREFIX,
        types.allowed_ips.sql_type(),
        types.permissions.sql_type(),
        CREATE_MANAGEMENT_TOKEN_SQL_SUFFIX
    )
}

fn update_management_token_sql(types: ManagementTokenJsonColumnTypes) -> String {
    format!(
        "{} $4::text::{}{} $5::text::{}{}",
        UPDATE_MANAGEMENT_TOKEN_SQL_PREFIX,
        types.allowed_ips.sql_type(),
        UPDATE_MANAGEMENT_TOKEN_SQL_MIDDLE,
        types.permissions.sql_type(),
        UPDATE_MANAGEMENT_TOKEN_SQL_SUFFIX
    )
}

#[async_trait]
impl ManagementTokenReadRepository for SqlxManagementTokenRepository {
    async fn list_management_tokens(
        &self,
        query: &ManagementTokenListQuery,
    ) -> Result<StoredManagementTokenListPage, DataLayerError> {
        let mut count_builder =
            QueryBuilder::<Postgres>::new("SELECT COUNT(mt.id) AS total FROM management_tokens mt");
        let mut count_where = WhereClause::new();
        apply_management_token_filters(&mut count_builder, &mut count_where, query);
        let total = count_builder
            .build_query_scalar::<i64>()
            .fetch_one(&self.pool)
            .await
            .map_postgres_err()?;

        let mut list_builder = QueryBuilder::<Postgres>::new(MANAGEMENT_TOKEN_WITH_USER_COLUMNS);
        let mut list_where = WhereClause::new();
        apply_management_token_filters(&mut list_builder, &mut list_where, query);
        list_builder.push(" ORDER BY mt.created_at DESC, mt.id DESC");
        push_limit_offset(
            &mut list_builder,
            i64::try_from(query.limit).unwrap_or(i64::MAX),
            i64::try_from(query.offset).unwrap_or(i64::MAX),
        );
        let rows = list_builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_postgres_err()?;
        let items = rows
            .iter()
            .map(map_token_with_user_row)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(StoredManagementTokenListPage {
            items,
            total: usize::try_from(total.max(0)).unwrap_or(usize::MAX),
        })
    }

    async fn get_management_token_with_user(
        &self,
        token_id: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(MANAGEMENT_TOKEN_WITH_USER_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "mt.id",
            token_id.to_string(),
        );
        push_limit(&mut builder, 1);
        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_token_with_user_row).transpose()
    }

    async fn get_management_token_with_user_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<StoredManagementTokenWithUser>, DataLayerError> {
        let mut builder = QueryBuilder::<Postgres>::new(MANAGEMENT_TOKEN_WITH_USER_COLUMNS);
        let mut where_clause = WhereClause::new();
        push_eq(
            &mut builder,
            &mut where_clause,
            "mt.token_hash",
            token_hash.to_string(),
        );
        push_limit(&mut builder, 1);
        let row = builder
            .build()
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        row.as_ref().map(map_token_with_user_row).transpose()
    }
}

fn apply_management_token_filters<'a>(
    builder: &mut QueryBuilder<'a, Postgres>,
    where_clause: &mut WhereClause,
    query: &'a ManagementTokenListQuery,
) {
    push_optional_eq(builder, where_clause, "mt.user_id", query.user_id.clone());
    push_optional_eq(builder, where_clause, "mt.is_active", query.is_active);
}

#[async_trait]
impl ManagementTokenWriteRepository for SqlxManagementTokenRepository {
    async fn create_management_token(
        &self,
        record: &CreateManagementTokenRecord,
    ) -> Result<StoredManagementToken, DataLayerError> {
        record.validate()?;
        let json_column_types = self.json_column_types().await?;
        let sql = create_management_token_sql(json_column_types);
        let allowed_ips = json_to_string(record.allowed_ips.as_ref())?;
        let permissions = json_to_string(record.permissions.as_ref())?;
        let row = sqlx::query(sql.as_str())
            .bind(&record.id)
            .bind(&record.user_id)
            .bind(&record.token_hash)
            .bind(record.token_prefix.as_deref())
            .bind(&record.name)
            .bind(record.description.as_deref())
            .bind(allowed_ips)
            .bind(permissions)
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
        let Some(current) = self
            .get_management_token_with_user(&record.token_id)
            .await?
        else {
            return Ok(None);
        };
        let json_column_types = self.json_column_types().await?;
        let sql = update_management_token_sql(json_column_types);
        let name = record
            .name
            .as_deref()
            .unwrap_or(current.token.name.as_str());
        let description = if record.clear_description {
            None
        } else {
            record
                .description
                .as_deref()
                .or(current.token.description.as_deref())
        };
        let allowed_ips = if record.clear_allowed_ips {
            None
        } else {
            record
                .allowed_ips
                .as_ref()
                .or(current.token.allowed_ips.as_ref())
        };
        let permissions = record
            .permissions
            .as_ref()
            .or(current.token.permissions.as_ref());
        let expires_at_unix_secs = if record.clear_expires_at {
            None
        } else {
            record
                .expires_at_unix_secs
                .or(current.token.expires_at_unix_secs)
        };
        let is_active = record.is_active.unwrap_or(current.token.is_active);
        let allowed_ips = json_to_string(allowed_ips)?;
        let permissions = json_to_string(permissions)?;
        let row = sqlx::query(sql.as_str())
            .bind(&record.token_id)
            .bind(name)
            .bind(description)
            .bind(allowed_ips)
            .bind(permissions)
            .bind(expires_at_unix_secs.and_then(|value| i64::try_from(value).ok()))
            .bind(is_active)
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

fn json_to_string(value: Option<&serde_json::Value>) -> Result<Option<String>, DataLayerError> {
    value
        .map(|value| {
            serde_json::to_string(value).map_err(|err| {
                DataLayerError::UnexpectedValue(format!(
                    "invalid management token JSON field: {err}"
                ))
            })
        })
        .transpose()
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
                Some("IP 限制规则不能为空，如需取消限制请不提供此字段".to_string())
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
    .with_permissions(row.try_get("permissions").map_postgres_err()?)
    .with_runtime_fields(
        optional_unix_secs(row.try_get("expires_at_unix_secs").map_postgres_err()?),
        optional_unix_secs(row.try_get("last_used_at_unix_secs").map_postgres_err()?),
        row.try_get("last_used_ip").map_postgres_err()?,
        u64::try_from(row.try_get::<i64, _>("usage_count").map_postgres_err()?).unwrap_or(0),
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
    use super::{
        create_management_token_sql, update_management_token_sql, JsonColumnType,
        ManagementTokenJsonColumnTypes, SqlxManagementTokenRepository,
    };
    use crate::{PostgresPoolConfig, PostgresPoolFactory};

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

    #[test]
    fn repository_sql_casts_json_fields_to_detected_column_types_without_json_case() {
        let jsonb_types = ManagementTokenJsonColumnTypes {
            allowed_ips: JsonColumnType::Jsonb,
            permissions: JsonColumnType::Jsonb,
        };
        let json_types = ManagementTokenJsonColumnTypes {
            allowed_ips: JsonColumnType::Json,
            permissions: JsonColumnType::Json,
        };
        let jsonb_create_sql = create_management_token_sql(jsonb_types);
        let jsonb_update_sql = update_management_token_sql(jsonb_types);
        let json_create_sql = create_management_token_sql(json_types);
        let json_update_sql = update_management_token_sql(json_types);

        assert!(jsonb_create_sql.contains("$7::text::jsonb"));
        assert!(jsonb_create_sql.contains("$8::text::jsonb"));
        assert!(jsonb_update_sql.contains("allowed_ips =\n $4::text::jsonb"));
        assert!(jsonb_update_sql.contains("permissions =\n $5::text::jsonb"));

        assert!(json_create_sql.contains("$7::text::json"));
        assert!(json_create_sql.contains("$8::text::json"));
        assert!(json_update_sql.contains("allowed_ips =\n $4::text::json"));
        assert!(json_update_sql.contains("permissions =\n $5::text::json"));

        for sql in [
            jsonb_create_sql.as_str(),
            jsonb_update_sql.as_str(),
            json_create_sql.as_str(),
            json_update_sql.as_str(),
        ] {
            assert!(!sql.contains("allowed_ips = CASE"));
            assert!(!sql.contains("permissions = CASE"));
            assert!(!sql.contains("$6::json IS NULL"));
            assert!(!sql.contains("COALESCE($7::json"));
        }
    }
}
