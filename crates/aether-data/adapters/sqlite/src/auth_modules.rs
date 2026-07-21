use async_trait::async_trait;
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite};

use aether_data_contracts::repository::auth_modules::*;
use aether_data_contracts::DataLayerError;
use aether_data_query::{push_eq, push_limit, WhereClause};

use crate::error::SqlResultExt;
use crate::SqlitePool;

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
pub struct SqliteAuthModuleReadRepository {
    pool: SqlitePool,
}

impl SqliteAuthModuleReadRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteAuthModuleRepository {
    pool: SqlitePool,
}

impl SqliteAuthModuleRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

async fn list_enabled_oauth_providers(
    pool: &SqlitePool,
) -> Result<Vec<StoredOAuthProviderModuleConfig>, DataLayerError> {
    let mut builder = QueryBuilder::<Sqlite>::new(OAUTH_PROVIDER_COLUMNS);
    let mut where_clause = WhereClause::new();
    push_eq(&mut builder, &mut where_clause, "is_enabled", true);
    builder.push(" ORDER BY provider_type ASC");
    let rows = builder.build().fetch_all(pool).await.map_sql_err()?;
    rows.iter().map(map_oauth_row).collect()
}

async fn get_ldap_config(
    pool: &SqlitePool,
) -> Result<Option<StoredLdapModuleConfig>, DataLayerError> {
    let mut builder = QueryBuilder::<Sqlite>::new(LDAP_CONFIG_COLUMNS);
    builder.push(" ORDER BY id ASC");
    push_limit(&mut builder, 1);
    let row = builder.build().fetch_optional(pool).await.map_sql_err()?;
    row.as_ref().map(map_ldap_row).transpose()
}

#[async_trait]
impl AuthModuleReadRepository for SqliteAuthModuleReadRepository {
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
impl AuthModuleReadRepository for SqliteAuthModuleRepository {
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
impl AuthModuleWriteRepository for SqliteAuthModuleRepository {
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
  SELECT id
  FROM ldap_configs
  ORDER BY id ASC
  LIMIT 1
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

fn map_oauth_row(row: &SqliteRow) -> Result<StoredOAuthProviderModuleConfig, DataLayerError> {
    StoredOAuthProviderModuleConfig::new(
        row.try_get("provider_type").map_sql_err()?,
        row.try_get("display_name").map_sql_err()?,
        row.try_get("client_id").map_sql_err()?,
        row.try_get("client_secret_encrypted").map_sql_err()?,
        row.try_get("redirect_uri").map_sql_err()?,
    )
}

fn map_ldap_row(row: &SqliteRow) -> Result<StoredLdapModuleConfig, DataLayerError> {
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
    use super::SqliteAuthModuleRepository;
    use aether_data_contracts::repository::auth_modules::{
        AuthModuleReadRepository, AuthModuleWriteRepository, StoredLdapModuleConfig,
    };

    use crate::run_migrations;

    #[tokio::test]
    async fn sqlite_repository_reads_and_writes_auth_module_configs() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("sqlite pool should connect");
        run_migrations(&pool)
            .await
            .expect("sqlite migrations should run");
        sqlx::query(
            r#"
INSERT INTO oauth_providers (
  provider_type, display_name, client_id, redirect_uri, frontend_callback_url,
  is_enabled, created_at, updated_at
) VALUES
  ('github', 'GitHub', 'github-client', 'https://github.example.com/callback',
   'https://frontend.example.com/callback', 1, 1, 1),
  ('disabled', 'Disabled', 'disabled-client', 'https://disabled.example.com/callback',
   'https://frontend.example.com/callback', 0, 1, 1)
"#,
        )
        .execute(&pool)
        .await
        .expect("oauth providers should seed");

        let repository = SqliteAuthModuleRepository::new(pool);
        let oauth = repository
            .list_enabled_oauth_providers()
            .await
            .expect("oauth providers should load");
        assert_eq!(oauth.len(), 1);
        assert_eq!(oauth[0].provider_type, "github");

        let ldap = StoredLdapModuleConfig {
            server_url: "ldaps://ldap.example.com".to_string(),
            bind_dn: "cn=admin,dc=example,dc=com".to_string(),
            bind_password_encrypted: Some("encrypted-password".to_string()),
            base_dn: "dc=example,dc=com".to_string(),
            user_search_filter: Some("(uid={username})".to_string()),
            username_attr: Some("uid".to_string()),
            email_attr: Some("mail".to_string()),
            display_name_attr: Some("displayName".to_string()),
            is_enabled: true,
            is_exclusive: false,
            use_starttls: true,
            connect_timeout: Some(10),
        };
        let stored = repository
            .upsert_ldap_config(&ldap)
            .await
            .expect("ldap should upsert")
            .expect("ldap should be returned");
        assert_eq!(stored.server_url, "ldaps://ldap.example.com");

        let updated = repository
            .upsert_ldap_config(&StoredLdapModuleConfig {
                server_url: "ldap://ldap.example.com".to_string(),
                ..ldap
            })
            .await
            .expect("ldap should update")
            .expect("ldap should be returned");
        assert_eq!(updated.server_url, "ldap://ldap.example.com");
    }
}
