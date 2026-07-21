use async_trait::async_trait;
use sqlx::{mysql::MySqlRow, MySql, QueryBuilder, Row};

use aether_data_contracts::repository::auth_modules::*;
use aether_data_contracts::DataLayerError;
use aether_data_query::{push_eq, push_limit, WhereClause};

use crate::error::SqlResultExt;
use crate::MysqlPool;

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

#[derive(Debug, Clone)]
pub struct MysqlAuthModuleReadRepository {
    pool: MysqlPool,
}

impl MysqlAuthModuleReadRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone)]
pub struct MysqlAuthModuleRepository {
    pool: MysqlPool,
}

impl MysqlAuthModuleRepository {
    pub fn new(pool: MysqlPool) -> Self {
        Self { pool }
    }
}

async fn list_enabled_oauth_providers(
    pool: &MysqlPool,
) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
    let mut builder = QueryBuilder::<MySql>::new(OAUTH_PROVIDER_COLUMNS);
    let mut where_clause = WhereClause::new();
    push_eq(&mut builder, &mut where_clause, "is_enabled", true);
    builder.push(" ORDER BY provider_type ASC");
    let rows = builder.build().fetch_all(pool).await.map_sql_err()?;
    rows.iter().map(map_oauth_row).collect()
}

async fn get_ldap_config(
    pool: &MysqlPool,
) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
    let mut builder = QueryBuilder::<MySql>::new(LDAP_CONFIG_COLUMNS);
    builder.push(" ORDER BY id ASC");
    push_limit(&mut builder, 1);
    let row = builder.build().fetch_optional(pool).await.map_sql_err()?;
    row.as_ref().map(map_ldap_row).transpose()
}

#[async_trait]
impl AuthModuleReadRepository for MysqlAuthModuleReadRepository {
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
impl AuthModuleReadRepository for MysqlAuthModuleRepository {
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
impl AuthModuleWriteRepository for MysqlAuthModuleRepository {
    async fn upsert_ldap_config(
        &self,
        config: &StoredLdapModuleConfig,
    ) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
        let now = now_unix_secs();
        let updated = sqlx::query(
            r#"
UPDATE ldap_configs
SET
  server_url = ?,
  bind_dn = ?,
  bind_password_encrypted = ?,
  base_dn = ?,
  user_search_filter = ?,
  username_attr = ?,
  email_attr = ?,
  display_name_attr = ?,
  is_enabled = ?,
  is_exclusive = ?,
  use_starttls = ?,
  connect_timeout = ?,
  updated_at = ?
WHERE id = (
  SELECT id FROM (
    SELECT id
    FROM ldap_configs
    ORDER BY id ASC
    LIMIT 1
  ) selected_ldap_config
)
"#,
        )
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
        .bind(now as i64)
        .execute(&self.pool)
        .await
        .map_sql_err()?;

        if updated.rows_affected() == 0 {
            sqlx::query(
                r#"
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
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#,
            )
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
            .bind(now as i64)
            .bind(now as i64)
            .execute(&self.pool)
            .await
            .map_sql_err()?;
        }

        self.get_ldap_config().await
    }
}

fn now_unix_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn map_oauth_row(row: &MySqlRow) -> Result<StoredOAuthProviderModuleConfig, DataLayerError> {
    StoredOAuthProviderModuleConfig::new(
        row.try_get("provider_type").map_sql_err()?,
        row.try_get("display_name").map_sql_err()?,
        row.try_get("client_id").map_sql_err()?,
        row.try_get("client_secret_encrypted").map_sql_err()?,
        row.try_get("redirect_uri").map_sql_err()?,
    )
}

fn map_ldap_row(row: &MySqlRow) -> Result<StoredLdapModuleConfig, DataLayerError> {
    Ok(StoredLdapModuleConfig {
        server_url: row.try_get("server_url").map_sql_err()?,
        bind_dn: row.try_get("bind_dn").map_sql_err()?,
        bind_password_encrypted: row.try_get("bind_password_encrypted").map_sql_err()?,
        base_dn: row.try_get("base_dn").map_sql_err()?,
        user_search_filter: row.try_get("user_search_filter").map_sql_err()?,
        username_attr: row.try_get("username_attr").map_sql_err()?,
        email_attr: row.try_get("email_attr").map_sql_err()?,
        display_name_attr: row.try_get("display_name_attr").map_sql_err()?,
        is_enabled: row.try_get("is_enabled").map_sql_err()?,
        is_exclusive: row.try_get("is_exclusive").map_sql_err()?,
        use_starttls: row.try_get("use_starttls").map_sql_err()?,
        connect_timeout: row.try_get("connect_timeout").map_sql_err()?,
    })
}

#[cfg(test)]
mod tests {
    use super::{MysqlAuthModuleReadRepository, MysqlAuthModuleRepository};

    #[tokio::test]
    async fn repository_builds_from_lazy_pool() {
        let pool = sqlx::mysql::MySqlPoolOptions::new().connect_lazy_with(
            "mysql://user:pass@localhost:3306/aether"
                .parse()
                .expect("mysql options should parse"),
        );

        let _repository = MysqlAuthModuleReadRepository::new(pool.clone());
        let _writable_repository = MysqlAuthModuleRepository::new(pool);
    }
}
