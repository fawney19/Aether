//! Runtime data access for Aether.
//!
//! This crate is the runtime composition facade. It selects concrete SQL
//! adapters, owns in-memory repositories and maintenance/backfill/export
//! workflows, and exposes application-facing backend handles. Shared repository
//! contracts live in `aether-data-contracts`; request-path SQL and executable
//! migrations live in the driver adapter crates.
//!
//! See `crates/aether-data/runtime/README.md` for the layer map and SQL driver policy.

pub mod backend;
mod config;
mod database;
pub mod driver;
mod error;
pub mod lifecycle;
pub mod maintenance;
pub mod repository;

#[cfg(feature = "mysql")]
pub use backend::MysqlBackend;
#[cfg(feature = "postgres")]
pub use backend::PostgresBackend;
#[cfg(feature = "sqlite")]
pub use backend::SqliteBackend;
pub use backend::{
    DataBackends, DataLeaseBackends, DataReadRepositories, DataTransactionBackends,
    DataWriteRepositories,
};
pub use config::DataLayerConfig;
pub use database::{DatabaseDriver, SqlDatabaseConfig, SqlPoolConfig, DEFAULT_SQLITE_DATABASE_URL};
pub use error::DataLayerError;
pub use maintenance::{
    DatabaseMaintenanceSummary, DatabasePoolSummary, DatabasePostgresActivityGroup,
    DatabasePostgresObservabilitySnapshot, StatsDailyAggregationInput,
    StatsDailyAggregationSummary, StatsHourlyAggregationInput, StatsHourlyAggregationSummary,
    WalletDailyUsageAggregationInput, WalletDailyUsageAggregationResult,
};
