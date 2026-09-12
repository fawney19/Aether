use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Postgres, QueryBuilder, Row};

use aether_data_contracts::repository::auth_modules::*;
use aether_data_contracts::DataLayerError;
use aether_data_query::{push_eq, WhereClause};

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

const UPDATE_LDAP_CONFIG_PRESERVE_PASSWORD_SQL: &str = r#"
UPDATE ldap_configs
SET
  server_url = $1,
  bind_dn = $2,
  base_dn = $3,
  user_search_filter = $4,
  username_attr = $5,
  email_attr = $6,
  display_name_attr = $7,
  is_enabled = $8,
  is_exclusive = $9,
  use_starttls = $10,
  connect_timeout = $11,
  updated_at = NOW()
WHERE singleton_key = 1
  AND server_url IS NOT DISTINCT FROM $12
  AND bind_dn IS NOT DISTINCT FROM $13
  AND bind_password_encrypted IS NOT DISTINCT FROM $14
  AND base_dn IS NOT DISTINCT FROM $15
  AND user_search_filter IS NOT DISTINCT FROM $16
  AND username_attr IS NOT DISTINCT FROM $17
  AND email_attr IS NOT DISTINCT FROM $18
  AND display_name_attr IS NOT DISTINCT FROM $19
  AND is_enabled IS NOT DISTINCT FROM $20
  AND is_exclusive IS NOT DISTINCT FROM $21
  AND use_starttls IS NOT DISTINCT FROM $22
  AND connect_timeout IS NOT DISTINCT FROM $23
"#;

const UPDATE_LDAP_CONFIG_REPLACE_PASSWORD_SQL: &str = r#"
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
WHERE singleton_key = 1
  AND server_url IS NOT DISTINCT FROM $13
  AND bind_dn IS NOT DISTINCT FROM $14
  AND bind_password_encrypted IS NOT DISTINCT FROM $15
  AND base_dn IS NOT DISTINCT FROM $16
  AND user_search_filter IS NOT DISTINCT FROM $17
  AND username_attr IS NOT DISTINCT FROM $18
  AND email_attr IS NOT DISTINCT FROM $19
  AND display_name_attr IS NOT DISTINCT FROM $20
  AND is_enabled IS NOT DISTINCT FROM $21
  AND is_exclusive IS NOT DISTINCT FROM $22
  AND use_starttls IS NOT DISTINCT FROM $23
  AND connect_timeout IS NOT DISTINCT FROM $24
"#;

const INSERT_LDAP_CONFIG_SQL: &str = r#"
INSERT INTO ldap_configs (
  singleton_key,
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
  1,
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
    builder.push(" WHERE singleton_key = 1");
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
    async fn compare_and_swap_ldap_config(
        &self,
        expected: Option<&StoredLdapModuleConfig>,
        replacement: &StoredLdapModuleConfig,
        bind_password_update: &LdapBindPasswordUpdate,
    ) -> Result<CompareAndSwapLdapConfigResult, DataLayerError> {
        let persisted =
            ldap_config_after_password_update(expected, replacement, bind_password_update)?;

        let Some(expected) = expected else {
            let insert = sqlx::query(INSERT_LDAP_CONFIG_SQL)
                .bind(&persisted.server_url)
                .bind(&persisted.bind_dn)
                .bind(persisted.bind_password_encrypted.as_deref())
                .bind(&persisted.base_dn)
                .bind(persisted.user_search_filter.as_deref())
                .bind(persisted.username_attr.as_deref())
                .bind(persisted.email_attr.as_deref())
                .bind(persisted.display_name_attr.as_deref())
                .bind(persisted.is_enabled)
                .bind(persisted.is_exclusive)
                .bind(persisted.use_starttls)
                .bind(persisted.connect_timeout)
                .execute(&self.pool)
                .await;
            return match insert {
                Ok(result) if result.rows_affected() == 1 => {
                    Ok(CompareAndSwapLdapConfigResult::Applied(persisted))
                }
                Ok(_) => Ok(CompareAndSwapLdapConfigResult::Conflict),
                Err(error)
                    if error
                        .as_database_error()
                        .is_some_and(|error| error.is_unique_violation()) =>
                {
                    Ok(CompareAndSwapLdapConfigResult::Conflict)
                }
                Err(error) => Err(crate::error::postgres_error(error)),
            };
        };

        let rows_affected = match bind_password_update {
            LdapBindPasswordUpdate::Preserve => {
                sqlx::query(UPDATE_LDAP_CONFIG_PRESERVE_PASSWORD_SQL)
                    .bind(&replacement.server_url)
                    .bind(&replacement.bind_dn)
                    .bind(&replacement.base_dn)
                    .bind(replacement.user_search_filter.as_deref())
                    .bind(replacement.username_attr.as_deref())
                    .bind(replacement.email_attr.as_deref())
                    .bind(replacement.display_name_attr.as_deref())
                    .bind(replacement.is_enabled)
                    .bind(replacement.is_exclusive)
                    .bind(replacement.use_starttls)
                    .bind(replacement.connect_timeout)
                    .bind(&expected.server_url)
                    .bind(&expected.bind_dn)
                    .bind(expected.bind_password_encrypted.as_deref())
                    .bind(&expected.base_dn)
                    .bind(expected.user_search_filter.as_deref())
                    .bind(expected.username_attr.as_deref())
                    .bind(expected.email_attr.as_deref())
                    .bind(expected.display_name_attr.as_deref())
                    .bind(expected.is_enabled)
                    .bind(expected.is_exclusive)
                    .bind(expected.use_starttls)
                    .bind(expected.connect_timeout)
                    .execute(&self.pool)
                    .await
                    .map_postgres_err()?
                    .rows_affected()
            }
            LdapBindPasswordUpdate::Set(_) | LdapBindPasswordUpdate::Clear => {
                sqlx::query(UPDATE_LDAP_CONFIG_REPLACE_PASSWORD_SQL)
                    .bind(&replacement.server_url)
                    .bind(&replacement.bind_dn)
                    .bind(persisted.bind_password_encrypted.as_deref())
                    .bind(&replacement.base_dn)
                    .bind(replacement.user_search_filter.as_deref())
                    .bind(replacement.username_attr.as_deref())
                    .bind(replacement.email_attr.as_deref())
                    .bind(replacement.display_name_attr.as_deref())
                    .bind(replacement.is_enabled)
                    .bind(replacement.is_exclusive)
                    .bind(replacement.use_starttls)
                    .bind(replacement.connect_timeout)
                    .bind(&expected.server_url)
                    .bind(&expected.bind_dn)
                    .bind(expected.bind_password_encrypted.as_deref())
                    .bind(&expected.base_dn)
                    .bind(expected.user_search_filter.as_deref())
                    .bind(expected.username_attr.as_deref())
                    .bind(expected.email_attr.as_deref())
                    .bind(expected.display_name_attr.as_deref())
                    .bind(expected.is_enabled)
                    .bind(expected.is_exclusive)
                    .bind(expected.use_starttls)
                    .bind(expected.connect_timeout)
                    .execute(&self.pool)
                    .await
                    .map_postgres_err()?
                    .rows_affected()
            }
        };
        if rows_affected == 1 {
            Ok(CompareAndSwapLdapConfigResult::Applied(persisted))
        } else {
            Ok(CompareAndSwapLdapConfigResult::Conflict)
        }
    }

    async fn delete_ldap_config_if_matches(
        &self,
        expected: &StoredLdapModuleConfig,
    ) -> Result<bool, DataLayerError> {
        let rows_affected = sqlx::query(
            r#"
DELETE FROM ldap_configs
WHERE singleton_key = 1
  AND server_url IS NOT DISTINCT FROM $1
  AND bind_dn IS NOT DISTINCT FROM $2
  AND bind_password_encrypted IS NOT DISTINCT FROM $3
  AND base_dn IS NOT DISTINCT FROM $4
  AND user_search_filter IS NOT DISTINCT FROM $5
  AND username_attr IS NOT DISTINCT FROM $6
  AND email_attr IS NOT DISTINCT FROM $7
  AND display_name_attr IS NOT DISTINCT FROM $8
  AND is_enabled IS NOT DISTINCT FROM $9
  AND is_exclusive IS NOT DISTINCT FROM $10
  AND use_starttls IS NOT DISTINCT FROM $11
  AND connect_timeout IS NOT DISTINCT FROM $12
"#,
        )
        .bind(&expected.server_url)
        .bind(&expected.bind_dn)
        .bind(expected.bind_password_encrypted.as_deref())
        .bind(&expected.base_dn)
        .bind(expected.user_search_filter.as_deref())
        .bind(expected.username_attr.as_deref())
        .bind(expected.email_attr.as_deref())
        .bind(expected.display_name_attr.as_deref())
        .bind(expected.is_enabled)
        .bind(expected.is_exclusive)
        .bind(expected.use_starttls)
        .bind(expected.connect_timeout)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();
        Ok(rows_affected == 1)
    }

    async fn compare_and_swap_ldap_bind_password(
        &self,
        expected: &str,
        replacement: &str,
    ) -> Result<bool, DataLayerError> {
        let rows_affected = sqlx::query(
            r#"
UPDATE ldap_configs
SET bind_password_encrypted = $1, updated_at = NOW()
WHERE singleton_key = 1
  AND bind_password_encrypted = $2
"#,
        )
        .bind(replacement)
        .bind(expected)
        .execute(&self.pool)
        .await
        .map_postgres_err()?
        .rows_affected();
        Ok(rows_affected == 1)
    }
}

fn ldap_config_after_password_update(
    expected: Option<&StoredLdapModuleConfig>,
    replacement: &StoredLdapModuleConfig,
    bind_password_update: &LdapBindPasswordUpdate,
) -> Result<StoredLdapModuleConfig, DataLayerError> {
    let bind_password_encrypted = match bind_password_update {
        LdapBindPasswordUpdate::Preserve => expected
            .ok_or_else(|| {
                DataLayerError::InvalidConfiguration(
                    "LDAP bind password cannot be preserved while creating the singleton"
                        .to_string(),
                )
            })?
            .bind_password_encrypted
            .clone(),
        LdapBindPasswordUpdate::Set(ciphertext) => Some(ciphertext.clone()),
        LdapBindPasswordUpdate::Clear => None,
    };
    Ok(StoredLdapModuleConfig {
        bind_password_encrypted,
        ..replacement.clone()
    })
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
