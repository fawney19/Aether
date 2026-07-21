use aether_data::driver::postgres::PostgresPoolConfig;
use aether_data::{DataLayerConfig, SqlDatabaseConfig};
use std::fmt;

#[derive(Clone, Default)]
pub struct GatewayDataConfig {
    database: Option<SqlDatabaseConfig>,
    postgres: Option<PostgresPoolConfig>,
    encryption_key: Option<String>,
}

impl fmt::Debug for GatewayDataConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayDataConfig")
            .field("database", &self.database)
            .field("postgres", &self.postgres)
            .field("has_encryption_key", &self.encryption_key.is_some())
            .finish()
    }
}

impl GatewayDataConfig {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn from_postgres_config(postgres: PostgresPoolConfig) -> Self {
        Self {
            database: Some(SqlDatabaseConfig::from_postgres_config(postgres.clone())),
            postgres: Some(postgres),
            encryption_key: None,
        }
    }

    pub fn from_database_config(database: SqlDatabaseConfig) -> Self {
        let postgres = database.to_postgres_config().ok();
        Self {
            database: Some(database),
            postgres,
            encryption_key: None,
        }
    }

    pub fn from_postgres_url(database_url: impl Into<String>, require_ssl: bool) -> Self {
        let mut postgres = PostgresPoolConfig::default();
        postgres.database_url = database_url.into();
        postgres.require_ssl = require_ssl;
        Self::from_postgres_config(postgres)
    }

    pub fn postgres(&self) -> Option<&PostgresPoolConfig> {
        self.postgres.as_ref()
    }

    pub fn database(&self) -> Option<&SqlDatabaseConfig> {
        self.database.as_ref()
    }

    pub fn with_encryption_key(mut self, encryption_key: impl Into<String>) -> Self {
        let encryption_key = encryption_key.into();
        let encryption_key = encryption_key.trim();
        self.encryption_key = if encryption_key.is_empty() {
            None
        } else {
            Some(encryption_key.to_string())
        };
        self
    }

    pub fn encryption_key(&self) -> Option<&str> {
        self.encryption_key.as_deref()
    }

    pub fn with_redis_url(
        self,
        _url: impl Into<String>,
        _key_prefix: Option<impl Into<String>>,
    ) -> Self {
        self
    }

    pub fn is_enabled(&self) -> bool {
        self.database.is_some() || self.postgres.is_some()
    }

    pub fn to_data_layer_config(&self) -> DataLayerConfig {
        DataLayerConfig {
            database: self.database.clone(),
            postgres: self.postgres.clone(),
        }
    }

    pub(crate) fn split_runtime_pools(&self) -> (Self, Option<Self>) {
        let configured_background_max =
            std::env::var("AETHER_GATEWAY_BACKGROUND_DB_MAX_CONNECTIONS")
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok());
        self.split_runtime_pools_with_background_max(configured_background_max)
    }

    /// Returns the database capacity reserved for isolated background work, when enabled.
    pub fn background_database_config(&self) -> Option<SqlDatabaseConfig> {
        self.split_runtime_pools()
            .1
            .and_then(|config| config.database)
    }

    fn split_runtime_pools_with_background_max(
        &self,
        configured_background_max: Option<u32>,
    ) -> (Self, Option<Self>) {
        let Some(database) = self.database.as_ref() else {
            return (self.clone(), None);
        };
        let total_max = database.pool.max_connections;
        if total_max < 2
            || configured_background_max == Some(0)
            || is_private_sqlite_memory_database(database)
        {
            return (self.clone(), None);
        }

        let default_background_max = (total_max / 5).clamp(1, 8);
        let background_max = configured_background_max
            .unwrap_or(default_background_max)
            .clamp(1, total_max.saturating_sub(1));
        let foreground_max = total_max.saturating_sub(background_max).max(1);
        let total_min = database.pool.min_connections.min(total_max);
        let background_min = u32::from(total_min > 1).min(background_max);
        // The configured minimum protects foreground readiness. Isolating background work must
        // not take one of those warm foreground connections away; the background pool receives
        // its own single warm connection while the combined hard maximum remains unchanged.
        let foreground_min = total_min.min(foreground_max);

        (
            self.with_pool_limits(foreground_min, foreground_max),
            Some(self.with_pool_limits(background_min, background_max)),
        )
    }

    fn with_pool_limits(&self, min_connections: u32, max_connections: u32) -> Self {
        let mut config = self.clone();
        if let Some(database) = config.database.as_mut() {
            database.pool.min_connections = min_connections.min(max_connections);
            database.pool.max_connections = max_connections;
        }
        if let Some(postgres) = config.postgres.as_mut() {
            postgres.min_connections = min_connections.min(max_connections);
            postgres.max_connections = max_connections;
        }
        config
    }
}

fn is_private_sqlite_memory_database(database: &aether_data::SqlDatabaseConfig) -> bool {
    database.driver == aether_data::DatabaseDriver::Sqlite
        && matches!(database.url.trim(), "sqlite::memory:" | "sqlite://:memory:")
}

#[cfg(test)]
mod tests {
    use super::GatewayDataConfig;
    use aether_data::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};

    #[test]
    fn runtime_pool_split_preserves_total_connection_budget() {
        let config = GatewayDataConfig::from_database_config(
            SqlDatabaseConfig::new(
                DatabaseDriver::Postgres,
                "postgres://localhost/aether",
                SqlPoolConfig {
                    min_connections: 4,
                    max_connections: 20,
                    ..SqlPoolConfig::default()
                },
            )
            .expect("database config should be valid"),
        );

        let (foreground, background) = config.split_runtime_pools_with_background_max(Some(4));
        let foreground = foreground.database().expect("foreground database");
        let background = background
            .expect("background database config")
            .database()
            .expect("background database")
            .clone();

        assert_eq!(foreground.pool.max_connections, 16);
        assert_eq!(background.pool.max_connections, 4);
        assert_eq!(
            foreground.pool.max_connections + background.pool.max_connections,
            20
        );
        assert_eq!(foreground.pool.min_connections, 4);
        assert_eq!(background.pool.min_connections, 1);
    }

    #[test]
    fn runtime_pool_split_can_be_disabled_or_degrade_for_single_connection() {
        let mut database = SqlDatabaseConfig::sqlite_default();
        database.pool.max_connections = 1;
        let config = GatewayDataConfig::from_database_config(database);
        assert!(config
            .split_runtime_pools_with_background_max(Some(1))
            .1
            .is_none());

        let mut database = SqlDatabaseConfig::sqlite_default();
        database.pool.max_connections = 8;
        let config = GatewayDataConfig::from_database_config(database);
        assert!(config
            .split_runtime_pools_with_background_max(Some(0))
            .1
            .is_none());
    }

    #[test]
    fn runtime_pool_split_keeps_private_sqlite_memory_database_in_one_pool() {
        for url in ["sqlite::memory:", "sqlite://:memory:"] {
            let config = GatewayDataConfig::from_database_config(
                SqlDatabaseConfig::new(
                    DatabaseDriver::Sqlite,
                    url,
                    SqlPoolConfig {
                        min_connections: 1,
                        max_connections: 8,
                        ..SqlPoolConfig::default()
                    },
                )
                .expect("sqlite memory database config should be valid"),
            );

            let (foreground, background) = config.split_runtime_pools_with_background_max(Some(2));

            assert!(background.is_none(), "private SQLite URL {url} was split");
            assert_eq!(
                foreground
                    .database()
                    .expect("foreground database")
                    .pool
                    .max_connections,
                8
            );
        }
    }
}
