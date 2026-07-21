use std::str::FromStr;
use std::time::Duration;

use crate::{DataLayerError, DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
use sqlx::MySqlPool as SqlxMysqlPool;

pub type MysqlPool = SqlxMysqlPool;
pub type MysqlPoolConfig = SqlDatabaseConfig;

#[derive(Debug, Clone)]
pub struct MysqlPoolFactory {
    config: MysqlPoolConfig,
}

impl MysqlPoolFactory {
    pub fn new(config: MysqlPoolConfig) -> Result<Self, DataLayerError> {
        if config.driver != DatabaseDriver::Mysql {
            return Err(DataLayerError::InvalidConfiguration(format!(
                "mysql pool requires mysql driver, got {}",
                config.driver
            )));
        }
        config.validate()?;
        Ok(Self { config })
    }

    pub fn config(&self) -> &MysqlPoolConfig {
        &self.config
    }

    pub fn connect_options(&self) -> Result<MySqlConnectOptions, DataLayerError> {
        let ssl_mode = if self.config.pool.require_ssl {
            MySqlSslMode::Required
        } else {
            MySqlSslMode::Preferred
        };
        MySqlConnectOptions::from_str(self.config.url.trim())
            .map(|options| {
                options
                    .ssl_mode(ssl_mode)
                    .statement_cache_capacity(self.config.pool.statement_cache_capacity)
            })
            .map_err(|err| {
                DataLayerError::InvalidConfiguration(format!("invalid mysql database url: {err}"))
            })
    }

    pub fn connect_lazy(&self) -> Result<MysqlPool, DataLayerError> {
        let SqlPoolConfig {
            min_connections,
            max_connections,
            acquire_timeout_ms,
            idle_timeout_ms,
            max_lifetime_ms,
            ..
        } = self.config.pool;

        Ok(MySqlPoolOptions::new()
            .min_connections(min_connections)
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_millis(acquire_timeout_ms))
            .idle_timeout(Duration::from_millis(idle_timeout_ms))
            .max_lifetime(Duration::from_millis(max_lifetime_ms))
            .connect_lazy_with(self.connect_options()?))
    }
}

#[cfg(test)]
mod tests {
    use super::MysqlPoolFactory;
    use crate::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};

    #[tokio::test]
    async fn factory_builds_lazy_pool_from_valid_config() {
        let config = SqlDatabaseConfig {
            driver: DatabaseDriver::Mysql,
            url: "mysql://user:pass@localhost:3306/aether".to_string(),
            pool: SqlPoolConfig {
                min_connections: 1,
                max_connections: 4,
                acquire_timeout_ms: 1_000,
                idle_timeout_ms: 5_000,
                max_lifetime_ms: 30_000,
                statement_cache_capacity: 64,
                require_ssl: false,
            },
        };

        let factory = MysqlPoolFactory::new(config).expect("factory should build");
        let _pool = factory.connect_lazy().expect("lazy pool should build");
    }
}
