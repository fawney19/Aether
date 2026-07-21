use crate::{DataLayerError, PostgresPoolConfig};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;

fn connect_options(config: &PostgresPoolConfig) -> Result<PgConnectOptions, DataLayerError> {
    config.validate()?;

    let ssl_mode = if config.require_ssl {
        PgSslMode::Require
    } else {
        PgSslMode::Prefer
    };

    let options = PgConnectOptions::from_str(config.database_url.trim()).map_err(|err| {
        DataLayerError::InvalidConfiguration(format!("invalid postgres database_url: {err}"))
    })?;

    Ok(options
        .ssl_mode(ssl_mode)
        .statement_cache_capacity(config.statement_cache_capacity))
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
        let options = connect_options(&self.config)?;
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
    use super::PostgresPoolFactory;
    use crate::PostgresPoolConfig;

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
