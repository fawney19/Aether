use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use aether_data_contracts::repository::auth_modules::*;
use aether_data_contracts::DataLayerError;
use aether_data_query::{push_eq, push_limit, WhereClause};

use crate::error::SqlxResultExt;

const OAUTH_PROVIDER_COLUMNS: &str = r#"
SELECT
  provider_type,
  display_name,
  client_id,
  client_secret_encrypted,
  redirect_uri
FROM oauth_providers
"#;

const LDAP_CONFIG_COLUMNS: &str = r#"
SELECT
  server_url,
  bind_dn,
  bind_password_encrypted,
  base_dn,
  user_search_filter,
  username_attr,
  email_attr,
  display_name_attr,
  is_enabled,
  is_exclusive,
  use_starttls,
  connect_timeout
FROM ldap_configs
"#;

const UPDATE_LDAP_CONFIG_SQL: &str = r#"
UPDATE ldap_configs
SET
  server_url = $1,
  bind_dn = $2,
  bind_password_encrypted = $3,
  base_dn = $4,
  user_search_filter = $5,
  username_attr = $6,
  email_attr = $7,
  display_name_attr = $8,
  is_enabled = $9,
  is_exclusive = $10,
  use_starttls = $11,
  connect_timeout = $12,
  updated_at = NOW()
WHERE id = (
  SELECT id
  FROM ldap_configs
  ORDER BY id ASC
  LIMIT 1
)
RETURNING
  server_url,
  bind_dn,
  bind_password_encrypted,
  base_dn,
  user_search_filter,
  username_attr,
  email_attr,
  display_name_attr,
  is_enabled,
  is_exclusive,
  use_starttls,
  connect_timeout
"#;

const INSERT_LDAP_CONFIG_SQL: &str = r#"
INSERT INTO ldap_configs (
  server_url,
  bind_dn,
  bind_password_encrypted,
  base_dn,
  user_search_filter,
  username_attr,
  email_attr,
  display_name_attr,
  is_enabled,
  is_exclusive,
  use_starttls,
  connect_timeout,
  created_at,
  updated_at
)
VALUES (
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
  $12,
  NOW(),
  NOW()
)
RETURNING
  server_url,
  bind_dn,
  bind_password_encrypted,
  base_dn,
  user_search_filter,
  username_attr,
  email_attr,
  display_name_attr,
  is_enabled,
  is_exclusive,
  use_starttls,
  connect_timeout
"#;

#[derive(Debug, Clone)]
pub struct SqlxAuthModuleReadRepository {
    pool: PgPool,
}

impl SqlxAuthModuleReadRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone)]
pub struct SqlxAuthModuleRepository {
    pool: PgPool,
}

impl SqlxAuthModuleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn list_enabled_oauth_providers(
    pool: &PgPool,
) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
    let mut builder = QueryBuilder::<Postgres>::new(OAUTH_PROVIDER_COLUMNS);
    let mut where_clause = WhereClause::new();
    push_eq(&mut builder, &mut where_clause, "is_enabled", true);
    builder.push(" ORDER BY provider_type ASC");
    let rows = builder.build().fetch_all(pool).await.map_postgres_err()?;
    rows.iter().map(map_oauth_row).collect()
}

async fn get_ldap_config(pool: &PgPool) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
    let mut builder = QueryBuilder::<Postgres>::new(LDAP_CONFIG_COLUMNS);
    builder.push(" ORDER BY id ASC");
    push_limit(&mut builder, 1);
    let row = builder
        .build()
        .fetch_optional(pool)
        .await
        .map_postgres_err()?;
    row.as_ref().map(map_ldap_row).transpose()
}

#[async_trait]
impl AuthModuleReadRepository for SqlxAuthModuleReadRepository {
    async fn list_enabled_oauth_providers(
        &self,
    ) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
        list_enabled_oauth_providers(&self.pool).await
    }

    async fn get_ldap_config(&self) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        get_ldap_config(&self.pool).await
    }
}

#[async_trait]
impl AuthModuleReadRepository for SqlxAuthModuleRepository {
    async fn list_enabled_oauth_providers(
        &self,
    ) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
        list_enabled_oauth_providers(&self.pool).await
    }

    async fn get_ldap_config(&self) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        get_ldap_config(&self.pool).await
    }
}

#[async_trait]
impl AuthModuleWriteRepository for SqlxAuthModuleRepository {
    async fn upsert_ldap_config(
        &self,
        config: &StoredLdapModuleConfig,
    ) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        let updated = sqlx::query(UPDATE_LDAP_CONFIG_SQL)
            .bind(&config.server_url)
            .bind(&config.bind_dn)
            .bind(config.bind_password_encrypted.as_deref())
            .bind(&config.base_dn)
            .bind(config.user_search_filter.as_deref())
            .bind(config.username_attr.as_deref())
            .bind(config.email_attr.as_deref())
            .bind(config.display_name_attr.as_deref())
            .bind(config.is_enabled)
            .bind(config.is_exclusive)
            .bind(config.use_starttls)
            .bind(config.connect_timeout)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        if let Some(row) = updated.as_ref() {
            return map_ldap_row(row).map(Some);
        }

        let inserted = sqlx::query(INSERT_LDAP_CONFIG_SQL)
            .bind(&config.server_url)
            .bind(&config.bind_dn)
            .bind(config.bind_password_encrypted.as_deref())
            .bind(&config.base_dn)
            .bind(config.user_search_filter.as_deref())
            .bind(config.username_attr.as_deref())
            .bind(config.email_attr.as_deref())
            .bind(config.display_name_attr.as_deref())
            .bind(config.is_enabled)
            .bind(config.is_exclusive)
            .bind(config.use_starttls)
            .bind(config.connect_timeout)
            .fetch_optional(&self.pool)
            .await
            .map_postgres_err()?;
        inserted.as_ref().map(map_ldap_row).transpose()
    }
}

fn map_oauth_row(row: &PgRow) -> Result<StoredOAuthProviderModuleConfig, DataLayerError> {
    StoredOAuthProviderModuleConfig::new(
        row.try_get("provider_type").map_postgres_err()?,
        row.try_get("display_name").map_postgres_err()?,
        row.try_get("client_id").map_postgres_err()?,
        row.try_get("client_secret_encrypted").map_postgres_err()?,
        row.try_get("redirect_uri").map_postgres_err()?,
    )
}

fn map_ldap_row(row: &PgRow) -> Result<StoredLdapModuleConfig, DataLayerError> {
    Ok(StoredLdapModuleConfig {
        server_url: row.try_get("server_url").map_postgres_err()?,
        bind_dn: row.try_get("bind_dn").map_postgres_err()?,
        bind_password_encrypted: row.try_get("bind_password_encrypted").map_postgres_err()?,
        base_dn: row.try_get("base_dn").map_postgres_err()?,
        user_search_filter: row.try_get("user_search_filter").map_postgres_err()?,
        username_attr: row.try_get("username_attr").map_postgres_err()?,
        email_attr: row.try_get("email_attr").map_postgres_err()?,
        display_name_attr: row.try_get("display_name_attr").map_postgres_err()?,
        is_enabled: row.try_get("is_enabled").map_postgres_err()?,
        is_exclusive: row.try_get("is_exclusive").map_postgres_err()?,
        use_starttls: row.try_get("use_starttls").map_postgres_err()?,
        connect_timeout: row.try_get("connect_timeout").map_postgres_err()?,
    })
}

#[cfg(test)]
mod tests {
    use super::{SqlxAuthModuleReadRepository, SqlxAuthModuleRepository};
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
        let _repository = SqlxAuthModuleReadRepository::new(pool);
    }

    #[tokio::test]
    async fn writable_repository_constructs_from_lazy_pool() {
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
        let _repository = SqlxAuthModuleRepository::new(pool);
    }
}
