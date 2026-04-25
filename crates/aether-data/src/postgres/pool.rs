use crate::DataLayerError;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;

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

    pub fn connect_options(&self) -> Result<PgConnectOptions, DataLayerError> {
        self.validate()?;

        let ssl_mode = if self.require_ssl {
            PgSslMode::Require
        } else {
            PgSslMode::Prefer
        };

        let options = PgConnectOptions::from_str(self.database_url.trim()).map_err(|err| {
            DataLayerError::InvalidConfiguration(format!("invalid postgres database_url: {err}"))
        })?;

        Ok(options
            .ssl_mode(ssl_mode)
            .statement_cache_capacity(self.statement_cache_capacity))
    }
}

pub type PostgresPool = PgPool;

#[derive(Debug, Clone)]
pub struct PostgresPoolFactory {
    config: PostgresPoolConfig,
}

impl PostgresPoolFactory {
    pub fn new(config: PostgresPoolConfig) -> Result<Self, DataLayerError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &PostgresPoolConfig {
        &self.config
    }

    pub fn connect_lazy(&self) -> Result<PostgresPool, DataLayerError> {
        let options = self.config.connect_options()?;
        Ok(PgPoolOptions::new()
            .min_connections(self.config.min_connections)
            .max_connections(self.config.max_connections)
            .acquire_timeout(Duration::from_millis(self.config.acquire_timeout_ms))
            .idle_timeout(Duration::from_millis(self.config.idle_timeout_ms))
            .max_lifetime(Duration::from_millis(self.config.max_lifetime_ms))
            .connect_lazy_with(options))
    }
}

#[cfg(test)]
mod tests {
    use super::{PostgresPoolConfig, PostgresPoolFactory};

    #[tokio::test]
    async fn factory_builds_lazy_pool_from_valid_config() {
        let config = PostgresPoolConfig {
            database_url: "postgres://localhost/aether".to_string(),
            min_connections: 1,
            max_connections: 4,
            acquire_timeout_ms: 1_000,
            idle_timeout_ms: 5_000,
            max_lifetime_ms: 30_000,
            statement_cache_capacity: 64,
            require_ssl: false,
        };

        let factory = PostgresPoolFactory::new(config).expect("factory should build");
        let _pool = factory.connect_lazy().expect("lazy pool should build");
    }
}
