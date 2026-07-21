//! Backend composition layer.
//!
//! `DataBackends` chooses the configured SQL driver, builds low-level pools,
//! instantiates concrete repositories, and exposes app-facing read/write,
//! lease, transaction, and maintenance handles. Request-path repository SQL
//! belongs in the selected `aether-data-*` adapter; backend-owned maintenance
//! SQL lives in focused modules such as `stats`, `wallet`, and `system`.

mod leases;
mod maintenance;
#[cfg(feature = "mysql")]
mod mysql;
#[cfg(feature = "postgres")]
mod postgres;
mod read;
mod referrals;
#[cfg(feature = "sqlite")]
mod sqlite;
mod stats;
#[cfg(any(feature = "mysql", feature = "sqlite"))]
mod stats_common;
mod system;
mod transactions;
mod wallet;
mod write;

use crate::maintenance::DatabasePoolSummary;
pub use leases::DataLeaseBackends;
#[cfg(feature = "mysql")]
pub use mysql::MysqlBackend;
#[cfg(feature = "postgres")]
pub use postgres::PostgresBackend;
pub use read::DataReadRepositories;
pub use referrals::{
    ReferralAdminStats, ReferralDataState, ReferralMutationStatus, ReferralRelationshipListQuery,
    ReferralRelationshipRecord, ReferralRewardConfig, ReferralRewardListQuery,
    ReferralRewardRecord, ReferralUserDashboard,
};
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteBackend;
pub use transactions::DataTransactionBackends;
pub use write::DataWriteRepositories;

use crate::database::DatabaseDriver;
use crate::{DataLayerConfig, DataLayerError};

#[derive(Clone, Copy)]
enum SqlBackendRef<'a> {
    #[cfg(feature = "postgres")]
    Postgres(&'a PostgresBackend),
    #[cfg(feature = "mysql")]
    Mysql(&'a MysqlBackend),
    #[cfg(feature = "sqlite")]
    Sqlite(&'a SqliteBackend),
}

#[derive(Debug, Clone, Default)]
pub struct DataBackends {
    config: DataLayerConfig,
    #[cfg(feature = "postgres")]
    postgres: Option<PostgresBackend>,
    #[cfg(feature = "mysql")]
    mysql: Option<MysqlBackend>,
    #[cfg(feature = "sqlite")]
    sqlite: Option<SqliteBackend>,
    leases: DataLeaseBackends,
    read: DataReadRepositories,
    transactions: DataTransactionBackends,
    write: DataWriteRepositories,
}

fn summarize_pool(
    driver: DatabaseDriver,
    pool_size: usize,
    idle: usize,
    max_connections: u32,
) -> DatabasePoolSummary {
    let max_connections = max_connections.max(1);
    let checked_out = pool_size.saturating_sub(idle);
    let usage_rate = checked_out as f64 / f64::from(max_connections) * 100.0;

    DatabasePoolSummary {
        driver,
        checked_out,
        pool_size,
        idle,
        max_connections,
        usage_rate,
    }
}

fn ensure_driver_enabled(driver: DatabaseDriver) -> Result<(), DataLayerError> {
    match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => Ok(()),
        #[cfg(not(feature = "postgres"))]
        DatabaseDriver::Postgres => Err(DataLayerError::InvalidInput(
            "PostgreSQL driver is not enabled for this aether-data build".to_string(),
        )),
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => Ok(()),
        #[cfg(not(feature = "mysql"))]
        DatabaseDriver::Mysql => Err(DataLayerError::InvalidInput(
            "MySQL driver is not enabled for this aether-data build".to_string(),
        )),
        #[cfg(feature = "sqlite")]
        DatabaseDriver::Sqlite => Ok(()),
        #[cfg(not(feature = "sqlite"))]
        DatabaseDriver::Sqlite => Err(DataLayerError::InvalidInput(
            "SQLite driver is not enabled for this aether-data build".to_string(),
        )),
    }
}

impl DataBackends {
    fn sql_backend(&self) -> Option<SqlBackendRef<'_>> {
        #[cfg(feature = "postgres")]
        if let Some(postgres) = self.postgres.as_ref() {
            return Some(SqlBackendRef::Postgres(postgres));
        }
        #[cfg(feature = "mysql")]
        if let Some(mysql) = self.mysql.as_ref() {
            return Some(SqlBackendRef::Mysql(mysql));
        }
        #[cfg(feature = "sqlite")]
        if let Some(sqlite) = self.sqlite.as_ref() {
            return Some(SqlBackendRef::Sqlite(sqlite));
        }
        None
    }

    pub fn from_config(config: DataLayerConfig) -> Result<Self, DataLayerError> {
        config.validate()?;

        let database = config.effective_database();
        if let Some(database) = database.as_ref() {
            ensure_driver_enabled(database.driver)?;
        }
        #[cfg(feature = "postgres")]
        let postgres = match database.clone() {
            Some(database) if database.driver == DatabaseDriver::Postgres => Some(
                PostgresBackend::from_config(database.to_postgres_config()?)?,
            ),
            _ => None,
        };
        #[cfg(feature = "mysql")]
        let mysql = match database.clone() {
            Some(database) if database.driver == DatabaseDriver::Mysql => {
                Some(MysqlBackend::from_config(database)?)
            }
            _ => None,
        };
        #[cfg(feature = "sqlite")]
        let sqlite = match database.clone() {
            Some(database) if database.driver == DatabaseDriver::Sqlite => {
                Some(SqliteBackend::from_config(database)?)
            }
            _ => None,
        };
        #[cfg(feature = "postgres")]
        let leases = DataLeaseBackends::from_postgres(postgres.as_ref())?;
        #[cfg(not(feature = "postgres"))]
        let leases = DataLeaseBackends::default();
        let read = DataReadRepositories::from_backends(
            #[cfg(feature = "postgres")]
            postgres.as_ref(),
            #[cfg(feature = "mysql")]
            mysql.as_ref(),
            #[cfg(feature = "sqlite")]
            sqlite.as_ref(),
        );
        #[cfg(feature = "postgres")]
        let transactions = DataTransactionBackends::from_postgres(postgres.as_ref());
        #[cfg(not(feature = "postgres"))]
        let transactions = DataTransactionBackends::default();
        let write = DataWriteRepositories::from_backends(
            #[cfg(feature = "postgres")]
            postgres.as_ref(),
            #[cfg(feature = "mysql")]
            mysql.as_ref(),
            #[cfg(feature = "sqlite")]
            sqlite.as_ref(),
        );

        Ok(Self {
            config,
            #[cfg(feature = "postgres")]
            postgres,
            #[cfg(feature = "mysql")]
            mysql,
            #[cfg(feature = "sqlite")]
            sqlite,
            leases,
            read,
            transactions,
            write,
        })
    }

    pub fn config(&self) -> &DataLayerConfig {
        &self.config
    }

    #[cfg(feature = "postgres")]
    pub fn postgres(&self) -> Option<&PostgresBackend> {
        self.postgres.as_ref()
    }

    pub fn database_driver(&self) -> Option<DatabaseDriver> {
        self.config
            .effective_database()
            .map(|database| database.driver)
    }

    #[cfg(feature = "mysql")]
    pub fn mysql(&self) -> Option<&MysqlBackend> {
        self.mysql.as_ref()
    }

    #[cfg(feature = "sqlite")]
    pub fn sqlite(&self) -> Option<&SqliteBackend> {
        self.sqlite.as_ref()
    }

    pub fn read(&self) -> &DataReadRepositories {
        &self.read
    }

    pub fn leases(&self) -> &DataLeaseBackends {
        &self.leases
    }

    pub fn transactions(&self) -> &DataTransactionBackends {
        &self.transactions
    }

    pub fn write(&self) -> &DataWriteRepositories {
        &self.write
    }

    pub fn has_runtime_backends(&self) -> bool {
        self.leases.has_any()
            || self.read.has_any()
            || self.transactions.has_any()
            || self.write.has_any()
    }
}

#[cfg(test)]
mod tests {
    use super::DataBackends;
    #[cfg(feature = "postgres")]
    use crate::driver::postgres::PostgresPoolConfig;
    use crate::{DataLayerConfig, DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig};

    #[test]
    #[cfg(not(feature = "mysql"))]
    fn rejects_mysql_when_driver_is_not_enabled() {
        let error = DataBackends::from_config(DataLayerConfig::from_database(SqlDatabaseConfig {
            driver: DatabaseDriver::Mysql,
            url: "mysql://user:pass@localhost/aether".to_string(),
            pool: SqlPoolConfig::default(),
        }))
        .expect_err("disabled mysql should fail explicitly");
        assert!(error.to_string().contains("MySQL driver is not enabled"));
    }

    #[test]
    #[cfg(not(feature = "sqlite"))]
    fn rejects_sqlite_when_driver_is_not_enabled() {
        let error = DataBackends::from_config(DataLayerConfig::from_database(SqlDatabaseConfig {
            driver: DatabaseDriver::Sqlite,
            url: "sqlite://./data/aether.db".to_string(),
            pool: SqlPoolConfig::default(),
        }))
        .expect_err("disabled sqlite should fail explicitly");
        assert!(error.to_string().contains("SQLite driver is not enabled"));
    }

    #[test]
    fn builds_empty_backends_from_default_config() {
        let backends = DataBackends::from_config(DataLayerConfig::default())
            .expect("empty config should be accepted");

        assert!(!backends.has_runtime_backends());
        #[cfg(feature = "postgres")]
        assert!(backends.postgres().is_none());
        #[cfg(feature = "mysql")]
        assert!(backends.mysql().is_none());
        #[cfg(feature = "sqlite")]
        assert!(backends.sqlite().is_none());
        #[cfg(feature = "postgres")]
        assert!(backends.leases().postgres().is_none());
        assert!(backends.read().auth_api_keys().is_none());
        assert!(backends.read().auth_modules().is_none());
        assert!(backends.read().billing().is_none());
        assert!(backends.read().gemini_file_mappings().is_none());
        assert!(backends.read().global_models().is_none());
        assert!(backends.read().management_tokens().is_none());
        assert!(backends.read().oauth_providers().is_none());
        assert!(backends.read().proxy_nodes().is_none());
        assert!(backends.read().minimal_candidate_selection().is_none());
        assert!(backends.read().request_candidates().is_none());
        assert!(backends.read().provider_catalog().is_none());
        assert!(backends.read().usage().is_none());
        assert!(backends.read().video_tasks().is_none());
        #[cfg(feature = "postgres")]
        assert!(backends.transactions().postgres().is_none());
        assert!(backends.write().settlement().is_none());
        assert!(backends.write().usage().is_none());
    }

    #[tokio::test]
    #[cfg(feature = "postgres")]
    async fn builds_postgres_backend_from_config() {
        let backends = DataBackends::from_config(DataLayerConfig {
            database: None,
            postgres: Some(PostgresPoolConfig {
                database_url: "postgres://localhost/aether".to_string(),
                min_connections: 1,
                max_connections: 4,
                acquire_timeout_ms: 1_000,
                idle_timeout_ms: 5_000,
                max_lifetime_ms: 30_000,
                statement_cache_capacity: 64,
                require_ssl: false,
            }),
        })
        .expect("postgres backend should build");

        assert!(backends.has_runtime_backends());
        #[cfg(feature = "postgres")]
        assert!(backends.postgres().is_some());
        #[cfg(feature = "mysql")]
        assert!(backends.mysql().is_none());
        #[cfg(feature = "sqlite")]
        assert!(backends.sqlite().is_none());
        #[cfg(feature = "postgres")]
        assert!(backends.leases().postgres().is_some());
        assert!(backends.read().auth_api_keys().is_some());
        assert!(backends.read().auth_modules().is_some());
        assert!(backends.read().billing().is_some());
        assert!(backends.read().gemini_file_mappings().is_some());
        assert!(backends.read().global_models().is_some());
        assert!(backends.read().management_tokens().is_some());
        assert!(backends.read().minimal_candidate_selection().is_some());
        assert!(backends.read().oauth_providers().is_some());
        assert!(backends.read().proxy_nodes().is_some());
        assert!(backends.read().minimal_candidate_selection().is_some());
        assert!(backends.read().request_candidates().is_some());
        assert!(backends.read().provider_catalog().is_some());
        assert!(backends.read().provider_quotas().is_some());
        assert!(backends.read().usage().is_some());
        assert!(backends.read().video_tasks().is_some());
        assert!(backends.read().wallets().is_some());
        assert!(backends.transactions().postgres().is_some());
        assert!(backends.write().auth_modules().is_some());
        assert!(backends.write().gemini_file_mappings().is_some());
        assert!(backends.write().management_tokens().is_some());
        assert!(backends.write().oauth_providers().is_some());
        assert!(backends.write().proxy_nodes().is_some());
        assert!(backends.write().provider_catalog().is_some());
        assert!(backends.write().provider_quotas().is_some());
        assert!(backends.write().settlement().is_some());
        assert!(backends.write().usage().is_some());
        assert!(backends.write().wallets().is_some());
        assert!(backends.config().effective_database().is_some());
    }

    #[tokio::test]
    #[cfg(feature = "mysql")]
    async fn builds_mysql_backend_from_database_config_with_first_core_repository() {
        let backends = DataBackends::from_config(DataLayerConfig {
            database: Some(SqlDatabaseConfig {
                driver: DatabaseDriver::Mysql,
                url: "mysql://user:pass@localhost:3306/aether".to_string(),
                pool: SqlPoolConfig::default(),
            }),
            postgres: None,
        })
        .expect("mysql backend should build");

        assert!(backends.has_runtime_backends());
        #[cfg(feature = "postgres")]
        assert!(backends.postgres().is_none());
        #[cfg(feature = "mysql")]
        assert!(backends.mysql().is_some());
        #[cfg(feature = "sqlite")]
        assert!(backends.sqlite().is_none());
        assert!(backends.read().has_any());
        assert!(backends.read().announcements().is_some());
        assert!(backends.read().auth_api_keys().is_some());
        assert!(backends.read().auth_modules().is_some());
        assert!(backends.read().billing().is_some());
        assert!(backends.read().gemini_file_mappings().is_some());
        assert!(backends.read().global_models().is_some());
        assert!(backends.read().management_tokens().is_some());
        assert!(backends.read().minimal_candidate_selection().is_some());
        assert!(backends.read().oauth_providers().is_some());
        assert!(backends.read().provider_catalog().is_some());
        assert!(backends.read().provider_quotas().is_some());
        assert!(backends.read().proxy_nodes().is_some());
        assert!(backends.read().request_candidates().is_some());
        assert!(backends.read().users().is_some());
        assert!(backends.read().video_tasks().is_some());
        assert!(backends.has_stats_hourly_aggregation_backend());
        assert!(backends.has_stats_daily_aggregation_backend());
        assert!(backends.write().has_any());
        assert!(backends.write().announcements().is_some());
        assert!(backends.write().auth_api_keys().is_some());
        assert!(backends.write().auth_modules().is_some());
        assert!(backends.write().gemini_file_mappings().is_some());
        assert!(backends.write().global_models().is_some());
        assert!(backends.write().management_tokens().is_some());
        assert!(backends.write().oauth_providers().is_some());
        assert!(backends.write().proxy_nodes().is_some());
        assert!(backends.write().provider_catalog().is_some());
        assert!(backends.write().provider_quotas().is_some());
        assert!(backends.write().request_candidates().is_some());
        assert!(backends.write().video_tasks().is_some());
        assert!(backends.write().wallets().is_some());
        assert!(backends.config().effective_database().is_some());
    }

    #[tokio::test]
    #[cfg(feature = "sqlite")]
    async fn builds_sqlite_backend_from_database_config_with_first_core_repository() {
        let backends = DataBackends::from_config(DataLayerConfig {
            database: Some(SqlDatabaseConfig {
                driver: DatabaseDriver::Sqlite,
                url: "sqlite://./data/aether.db".to_string(),
                pool: SqlPoolConfig::default(),
            }),
            postgres: None,
        })
        .expect("sqlite backend should build");

        assert!(backends.has_runtime_backends());
        #[cfg(feature = "postgres")]
        assert!(backends.postgres().is_none());
        #[cfg(feature = "mysql")]
        assert!(backends.mysql().is_none());
        #[cfg(feature = "sqlite")]
        assert!(backends.sqlite().is_some());
        assert!(backends.read().has_any());
        assert!(backends.read().announcements().is_some());
        assert!(backends.read().auth_api_keys().is_some());
        assert!(backends.read().auth_modules().is_some());
        assert!(backends.read().billing().is_some());
        assert!(backends.read().gemini_file_mappings().is_some());
        assert!(backends.read().global_models().is_some());
        assert!(backends.read().management_tokens().is_some());
        assert!(backends.read().oauth_providers().is_some());
        assert!(backends.read().provider_catalog().is_some());
        assert!(backends.read().provider_quotas().is_some());
        assert!(backends.read().proxy_nodes().is_some());
        assert!(backends.read().request_candidates().is_some());
        assert!(backends.read().users().is_some());
        assert!(backends.read().video_tasks().is_some());
        assert!(backends.has_stats_hourly_aggregation_backend());
        assert!(backends.has_stats_daily_aggregation_backend());
        assert!(backends.write().has_any());
        assert!(backends.write().announcements().is_some());
        assert!(backends.write().auth_api_keys().is_some());
        assert!(backends.write().auth_modules().is_some());
        assert!(backends.write().gemini_file_mappings().is_some());
        assert!(backends.write().global_models().is_some());
        assert!(backends.write().management_tokens().is_some());
        assert!(backends.write().oauth_providers().is_some());
        assert!(backends.write().proxy_nodes().is_some());
        assert!(backends.write().provider_catalog().is_some());
        assert!(backends.write().provider_quotas().is_some());
        assert!(backends.write().request_candidates().is_some());
        assert!(backends.write().video_tasks().is_some());
        assert!(backends.write().wallets().is_some());
        assert!(backends.config().effective_database().is_some());
    }
}
