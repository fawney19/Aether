use std::fmt;
use std::str::FromStr;

use crate::DataLayerError;

pub const DEFAULT_SQLITE_DATABASE_URL: &str = "sqlite://./data/aether.db";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PostgresPoolConfig {
    pub database_url: String,
    pub min_connections: u32,
    pub max_connections: u32,
    pub acquire_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub max_lifetime_ms: u64,
    pub statement_cache_capacity: usize,
    pub require_ssl: bool,
}

impl Default for PostgresPoolConfig {
    fn default() -> Self {
        Self {
            database_url: String::new(),
            min_connections: 4,
            max_connections: 20,
            acquire_timeout_ms: 10_000,
            idle_timeout_ms: 30_000,
            max_lifetime_ms: 30 * 60_000,
            statement_cache_capacity: 100,
            require_ssl: false,
        }
    }
}

impl PostgresPoolConfig {
    pub fn validate(&self) -> Result<(), DataLayerError> {
        if self.database_url.trim().is_empty() {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres database_url cannot be empty".to_string(),
            ));
        }
        if self.min_connections > self.max_connections {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres min_connections cannot exceed max_connections".to_string(),
            ));
        }
        if self.statement_cache_capacity == 0 {
            return Err(DataLayerError::InvalidConfiguration(
                "postgres statement_cache_capacity must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(
    Debug, Copy, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseDriver {
    Sqlite,
    Mysql,
    Postgres,
}

impl DatabaseDriver {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Mysql => "mysql",
            Self::Postgres => "postgres",
        }
    }

    pub fn from_database_url(url: &str) -> Option<Self> {
        let scheme = url.split_once(':')?.0.to_ascii_lowercase();
        match scheme.as_str() {
            "sqlite" => Some(Self::Sqlite),
            "mysql" | "mariadb" => Some(Self::Mysql),
            "postgres" | "postgresql" => Some(Self::Postgres),
            _ => None,
        }
    }
}

impl fmt::Display for DatabaseDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DatabaseDriver {
    type Err = DataLayerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sqlite" => Ok(Self::Sqlite),
            "mysql" | "mariadb" => Ok(Self::Mysql),
            "postgres" | "postgresql" => Ok(Self::Postgres),
            other => Err(DataLayerError::InvalidConfiguration(format!(
                "unsupported database driver '{other}'; expected sqlite, mysql, or postgres"
            ))),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SqlPoolConfig {
    pub min_connections: u32,
    pub max_connections: u32,
    pub acquire_timeout_ms: u64,
    pub idle_timeout_ms: u64,
    pub max_lifetime_ms: u64,
    pub statement_cache_capacity: usize,
    pub require_ssl: bool,
}

impl Default for SqlPoolConfig {
    fn default() -> Self {
        Self {
            min_connections: 1,
            max_connections: 20,
            acquire_timeout_ms: 10_000,
            idle_timeout_ms: 30_000,
            max_lifetime_ms: 30 * 60_000,
            statement_cache_capacity: 100,
            require_ssl: false,
        }
    }
}

impl SqlPoolConfig {
    pub fn validate(&self, driver: DatabaseDriver) -> Result<(), DataLayerError> {
        if self.min_connections > self.max_connections {
            return Err(DataLayerError::InvalidConfiguration(format!(
                "{driver} min_connections cannot exceed max_connections"
            )));
        }
        if self.statement_cache_capacity == 0 {
            return Err(DataLayerError::InvalidConfiguration(format!(
                "{driver} statement_cache_capacity must be positive"
            )));
        }
        if driver == DatabaseDriver::Sqlite && self.require_ssl {
            return Err(DataLayerError::InvalidConfiguration(
                "sqlite database does not support require_ssl".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SqlDatabaseConfig {
    pub driver: DatabaseDriver,
    pub url: String,
    pub pool: SqlPoolConfig,
}

impl SqlDatabaseConfig {
    pub fn new(
        driver: DatabaseDriver,
        url: impl Into<String>,
        pool: SqlPoolConfig,
    ) -> Result<Self, DataLayerError> {
        let config = Self {
            driver,
            url: url.into(),
            pool,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn sqlite_default() -> Self {
        Self {
            driver: DatabaseDriver::Sqlite,
            url: DEFAULT_SQLITE_DATABASE_URL.to_string(),
            pool: SqlPoolConfig::default(),
        }
    }

    pub fn validate(&self) -> Result<(), DataLayerError> {
        if self.url.trim().is_empty() {
            return Err(DataLayerError::InvalidConfiguration(format!(
                "{} database url cannot be empty",
                self.driver
            )));
        }
        if let Some(url_driver) = DatabaseDriver::from_database_url(&self.url) {
            if url_driver != self.driver {
                return Err(DataLayerError::InvalidConfiguration(format!(
                    "database driver '{}' does not match url scheme '{}'",
                    self.driver, url_driver
                )));
            }
        }
        self.pool.validate(self.driver)
    }

    pub fn from_postgres_config(postgres: PostgresPoolConfig) -> Self {
        Self {
            driver: DatabaseDriver::Postgres,
            url: postgres.database_url,
            pool: SqlPoolConfig {
                min_connections: postgres.min_connections,
                max_connections: postgres.max_connections,
                acquire_timeout_ms: postgres.acquire_timeout_ms,
                idle_timeout_ms: postgres.idle_timeout_ms,
                max_lifetime_ms: postgres.max_lifetime_ms,
                statement_cache_capacity: postgres.statement_cache_capacity,
                require_ssl: postgres.require_ssl,
            },
        }
    }

    pub fn to_postgres_config(&self) -> Result<PostgresPoolConfig, DataLayerError> {
        if self.driver != DatabaseDriver::Postgres {
            return Err(DataLayerError::InvalidConfiguration(format!(
                "cannot build postgres pool config from {} database config",
                self.driver
            )));
        }
        Ok(PostgresPoolConfig {
            database_url: self.url.clone(),
            min_connections: self.pool.min_connections,
            max_connections: self.pool.max_connections,
            acquire_timeout_ms: self.pool.acquire_timeout_ms,
            idle_timeout_ms: self.pool.idle_timeout_ms,
            max_lifetime_ms: self.pool.max_lifetime_ms,
            statement_cache_capacity: self.pool.statement_cache_capacity,
            require_ssl: self.pool.require_ssl,
        })
    }
}

impl From<PostgresPoolConfig> for SqlDatabaseConfig {
    fn from(value: PostgresPoolConfig) -> Self {
        Self::from_postgres_config(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DatabaseDriver, PostgresPoolConfig, SqlDatabaseConfig, SqlPoolConfig,
        DEFAULT_SQLITE_DATABASE_URL,
    };

    #[test]
    fn parses_database_driver_aliases() {
        assert_eq!(
            "sqlite".parse::<DatabaseDriver>().unwrap(),
            DatabaseDriver::Sqlite
        );
        assert_eq!(
            "mariadb".parse::<DatabaseDriver>().unwrap(),
            DatabaseDriver::Mysql
        );
        assert_eq!(
            "postgresql".parse::<DatabaseDriver>().unwrap(),
            DatabaseDriver::Postgres
        );
        assert!("oracle".parse::<DatabaseDriver>().is_err());
    }

    #[test]
    fn infers_driver_from_database_url_scheme() {
        assert_eq!(
            DatabaseDriver::from_database_url("sqlite://./data/aether.db"),
            Some(DatabaseDriver::Sqlite)
        );
        assert_eq!(
            DatabaseDriver::from_database_url("mysql://localhost/aether"),
            Some(DatabaseDriver::Mysql)
        );
        assert_eq!(
            DatabaseDriver::from_database_url("postgres://localhost/aether"),
            Some(DatabaseDriver::Postgres)
        );
    }

    #[test]
    fn validates_driver_url_mismatch() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Mysql,
            url: "postgres://localhost/aether".to_string(),
            pool: SqlPoolConfig::default(),
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn builds_legacy_postgres_config_round_trip() {
        let postgres = PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 2,
            max_connections: 8,
            acquire_timeout_ms: 1_500,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: true,
        };

        let database = SqlDatabaseConfig::from_postgres_config(postgres.clone());

        assert_eq!(database.driver, DatabaseDriver::Postgres);
        assert_eq!(database.to_postgres_config().unwrap(), postgres);
    }

    #[test]
    fn sqlite_default_uses_local_database_path() {
        let database = SqlDatabaseConfig::sqlite_default();

        assert_eq!(database.driver, DatabaseDriver::Sqlite);
        assert_eq!(database.url, DEFAULT_SQLITE_DATABASE_URL);
    }
}
