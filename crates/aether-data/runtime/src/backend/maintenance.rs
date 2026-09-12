#[cfg(feature = "postgres")]
mod postgres;

use super::{summarize_pool, DataBackends, SqlBackendRef};
use crate::maintenance::{
    DatabaseMaintenanceSummary, DatabasePoolSummary, DatabasePostgresActivityGroup,
    DatabasePostgresObservabilitySnapshot, StatsDailyAggregationInput,
    StatsDailyAggregationSummary, StatsHourlyAggregationInput, StatsHourlyAggregationSummary,
    WalletDailyUsageAggregationInput, WalletDailyUsageAggregationResult,
};
use crate::repository::system::{
    AdminSystemPurgeSummary, AdminSystemPurgeTarget, AdminSystemStats,
    AdminSystemUsageAggregateImportMode, AdminSystemUsageAggregateImportSummary,
    AdminSystemUsageAggregateSnapshot, StoredSystemConfigEntry,
};
use crate::DataLayerError;
use sqlx::migrate::MigrateError;

async fn warm_pool<DB>(pool: &sqlx::Pool<DB>, min_connections: u32) -> Result<(), DataLayerError>
where
    DB: sqlx::Database,
{
    let mut connections = Vec::with_capacity(min_connections as usize);
    for _ in 0..min_connections {
        connections.push(pool.acquire().await.map_err(DataLayerError::sql)?);
    }
    Ok(())
}

pub(super) fn maintenance_identifier(value: &str) -> Result<&str, DataLayerError> {
    let valid = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if valid {
        Ok(value)
    } else {
        Err(DataLayerError::InvalidInput(format!(
            "invalid maintenance table name: {value}"
        )))
    }
}

impl DataBackends {
    pub fn has_database_maintenance_backend(&self) -> bool {
        self.sql_backend().is_some()
    }

    pub fn has_database_pool_summary(&self) -> bool {
        self.sql_backend().is_some()
    }

    /// Establishes the configured minimum number of SQL connections before the service reports
    /// ready. Driver pools are built lazily, so relying on request traffic to grow them can make
    /// the first concurrency ramp consume nearly every connection in the small cold pool.
    pub async fn warm_database_pool(&self) -> Result<(), DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.warm_database_pool().await,
            None => Ok(()),
        }
    }

    pub fn has_system_config_backend(&self) -> bool {
        self.sql_backend().is_some()
    }

    pub fn has_wallet_daily_usage_aggregation_backend(&self) -> bool {
        self.sql_backend().is_some()
    }

    pub fn has_stats_hourly_aggregation_backend(&self) -> bool {
        self.sql_backend().is_some()
    }

    pub fn has_stats_daily_aggregation_backend(&self) -> bool {
        self.sql_backend().is_some()
    }

    pub async fn run_database_maintenance(
        &self,
        table_names: &[&str],
    ) -> Result<DatabaseMaintenanceSummary, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.run_database_maintenance(table_names).await,
            None => Ok(DatabaseMaintenanceSummary::default()),
        }
    }

    pub async fn run_database_migrations(&self) -> Result<bool, MigrateError> {
        match self.sql_backend() {
            Some(backend) => backend.run_database_migrations().await,
            None => Ok(false),
        }
    }

    pub async fn run_database_backfills(&self) -> Result<bool, MigrateError> {
        match self.sql_backend() {
            Some(backend) => backend.run_database_backfills().await,
            None => Ok(false),
        }
    }

    pub async fn pending_database_migrations(
        &self,
    ) -> Result<Option<Vec<crate::lifecycle::migrate::PendingMigrationInfo>>, MigrateError> {
        match self.sql_backend() {
            Some(backend) => backend.pending_database_migrations().await,
            None => Ok(None),
        }
    }

    pub async fn prepare_database_for_startup(
        &self,
    ) -> Result<Option<Vec<crate::lifecycle::migrate::PendingMigrationInfo>>, MigrateError> {
        match self.sql_backend() {
            Some(backend) => backend.prepare_database_for_startup().await,
            None => Ok(None),
        }
    }

    pub async fn pending_database_backfills(
        &self,
    ) -> Result<Option<Vec<crate::lifecycle::backfill::PendingBackfillInfo>>, MigrateError> {
        match self.sql_backend() {
            Some(backend) => backend.pending_database_backfills().await,
            None => Ok(None),
        }
    }

    pub fn database_pool_summary(&self) -> Option<DatabasePoolSummary> {
        self.sql_backend().map(SqlBackendRef::database_pool_summary)
    }

    pub async fn postgres_observability_snapshot(
        &self,
    ) -> Result<Option<DatabasePostgresObservabilitySnapshot>, DataLayerError> {
        #[cfg(not(feature = "postgres"))]
        return Ok(None);
        #[cfg(feature = "postgres")]
        match self.postgres() {
            Some(postgres) => postgres.postgres_observability_snapshot().await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn postgres_activity_groups(
        &self,
        limit: i64,
    ) -> Result<Vec<DatabasePostgresActivityGroup>, DataLayerError> {
        #[cfg(not(feature = "postgres"))]
        let _ = limit;
        #[cfg(not(feature = "postgres"))]
        return Ok(Vec::new());
        #[cfg(feature = "postgres")]
        match self.postgres() {
            Some(postgres) => postgres.postgres_activity_groups(limit).await,
            None => Ok(Vec::new()),
        }
    }

    pub async fn aggregate_wallet_daily_usage(
        &self,
        input: &WalletDailyUsageAggregationInput,
    ) -> Result<WalletDailyUsageAggregationResult, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.aggregate_wallet_daily_usage(input).await,
            None => Ok(WalletDailyUsageAggregationResult::default()),
        }
    }

    pub async fn aggregate_stats_hourly(
        &self,
        input: &StatsHourlyAggregationInput,
    ) -> Result<Option<StatsHourlyAggregationSummary>, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.aggregate_stats_hourly(input).await,
            None => Ok(None),
        }
    }

    pub async fn aggregate_stats_daily(
        &self,
        input: &StatsDailyAggregationInput,
    ) -> Result<Option<StatsDailyAggregationSummary>, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.aggregate_stats_daily(input).await,
            None => Ok(None),
        }
    }

    pub async fn find_system_config_value(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.find_system_config_value(key).await,
            None => Ok(None),
        }
    }

    pub async fn compare_and_set_system_config_string_value(
        &self,
        key: &str,
        expected: &str,
        replacement: &str,
    ) -> Result<bool, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => {
                backend
                    .compare_and_set_system_config_string_value(key, expected, replacement)
                    .await
            }
            None => Ok(false),
        }
    }

    pub async fn list_system_config_entries(
        &self,
    ) -> Result<Vec<StoredSystemConfigEntry>, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.list_system_config_entries().await,
            None => Ok(Vec::new()),
        }
    }

    pub async fn upsert_system_config_entry(
        &self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<Option<StoredSystemConfigEntry>, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend
                .upsert_system_config_entry(key, value, description)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    pub async fn delete_system_config_value(&self, key: &str) -> Result<bool, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.delete_system_config_value(key).await,
            None => Ok(false),
        }
    }

    pub async fn read_admin_system_stats(&self) -> Result<AdminSystemStats, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.read_admin_system_stats().await,
            None => Ok(AdminSystemStats::default()),
        }
    }

    pub async fn purge_admin_system_data(
        &self,
        target: AdminSystemPurgeTarget,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.purge_admin_system_data(target).await,
            None => Ok(AdminSystemPurgeSummary::default()),
        }
    }

    pub async fn export_admin_system_usage_aggregates(
        &self,
    ) -> Result<AdminSystemUsageAggregateSnapshot, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.export_admin_system_usage_aggregates().await,
            None => Ok(AdminSystemUsageAggregateSnapshot::default()),
        }
    }

    pub async fn import_admin_system_usage_aggregates(
        &self,
        snapshot: &AdminSystemUsageAggregateSnapshot,
        user_id_map: &std::collections::BTreeMap<String, String>,
        api_key_id_map: &std::collections::BTreeMap<String, String>,
        mode: AdminSystemUsageAggregateImportMode,
    ) -> Result<AdminSystemUsageAggregateImportSummary, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => {
                backend
                    .import_admin_system_usage_aggregates(
                        snapshot,
                        user_id_map,
                        api_key_id_map,
                        mode,
                    )
                    .await
            }
            None => Ok(AdminSystemUsageAggregateImportSummary::default()),
        }
    }

    pub async fn purge_admin_request_bodies_batch(
        &self,
        batch_size: usize,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        match self.sql_backend() {
            Some(backend) => backend.purge_admin_request_bodies_batch(batch_size).await,
            None => Ok(AdminSystemPurgeSummary::default()),
        }
    }
}

impl<'a> SqlBackendRef<'a> {
    async fn warm_database_pool(self) -> Result<(), DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => {
                warm_pool(postgres.pool(), postgres.config().min_connections).await
            }
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn run_database_maintenance(
        self,
        table_names: &[&str],
    ) -> Result<DatabaseMaintenanceSummary, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.run_table_maintenance(table_names).await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn run_database_migrations(self) -> Result<bool, MigrateError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => {
                crate::lifecycle::migrate::run_migrations(postgres.pool()).await?;
                Ok(true)
            }
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn run_database_backfills(self) -> Result<bool, MigrateError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => {
                crate::lifecycle::backfill::run_backfills(postgres.pool()).await?;
                Ok(true)
            }
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn pending_database_migrations(
        self,
    ) -> Result<Option<Vec<crate::lifecycle::migrate::PendingMigrationInfo>>, MigrateError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => Ok(Some(
                crate::lifecycle::migrate::pending_migrations(postgres.pool()).await?,
            )),
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn prepare_database_for_startup(
        self,
    ) -> Result<Option<Vec<crate::lifecycle::migrate::PendingMigrationInfo>>, MigrateError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => Ok(Some(
                crate::lifecycle::migrate::prepare_database_for_startup(postgres.pool()).await?,
            )),
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn pending_database_backfills(
        self,
    ) -> Result<Option<Vec<crate::lifecycle::backfill::PendingBackfillInfo>>, MigrateError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => Ok(Some(
                crate::lifecycle::backfill::pending_backfills(postgres.pool()).await?,
            )),
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    fn database_pool_summary(self) -> DatabasePoolSummary {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => summarize_pool(
                crate::database::DatabaseDriver::Postgres,
                usize::try_from(postgres.pool().size()).unwrap_or(usize::MAX),
                postgres.pool().num_idle(),
                postgres.config().max_connections,
            ),
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn aggregate_wallet_daily_usage(
        self,
        input: &WalletDailyUsageAggregationInput,
    ) -> Result<WalletDailyUsageAggregationResult, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.aggregate_wallet_daily_usage(input).await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn aggregate_stats_hourly(
        self,
        input: &StatsHourlyAggregationInput,
    ) -> Result<Option<StatsHourlyAggregationSummary>, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.aggregate_stats_hourly(input).await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn aggregate_stats_daily(
        self,
        input: &StatsDailyAggregationInput,
    ) -> Result<Option<StatsDailyAggregationSummary>, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.aggregate_stats_daily(input).await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn find_system_config_value(
        self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.find_system_config_value(key).await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn compare_and_set_system_config_string_value(
        self,
        key: &str,
        expected: &str,
        replacement: &str,
    ) -> Result<bool, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => {
                postgres
                    .compare_and_set_system_config_string_value(key, expected, replacement)
                    .await
            }
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn list_system_config_entries(
        self,
    ) -> Result<Vec<StoredSystemConfigEntry>, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.list_system_config_entries().await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn upsert_system_config_entry(
        self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<StoredSystemConfigEntry, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => {
                postgres
                    .upsert_system_config_entry(key, value, description)
                    .await
            }
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn delete_system_config_value(self, key: &str) -> Result<bool, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.delete_system_config_value(key).await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn read_admin_system_stats(self) -> Result<AdminSystemStats, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.read_admin_system_stats().await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn purge_admin_system_data(
        self,
        target: AdminSystemPurgeTarget,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.purge_admin_system_data(target).await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn export_admin_system_usage_aggregates(
        self,
    ) -> Result<AdminSystemUsageAggregateSnapshot, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.export_admin_system_usage_aggregates().await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn import_admin_system_usage_aggregates(
        self,
        snapshot: &AdminSystemUsageAggregateSnapshot,
        user_id_map: &std::collections::BTreeMap<String, String>,
        api_key_id_map: &std::collections::BTreeMap<String, String>,
        mode: AdminSystemUsageAggregateImportMode,
    ) -> Result<AdminSystemUsageAggregateImportSummary, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => {
                postgres
                    .import_admin_system_usage_aggregates(
                        snapshot,
                        user_id_map,
                        api_key_id_map,
                        mode,
                    )
                    .await
            }
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }

    async fn purge_admin_request_bodies_batch(
        self,
        batch_size: usize,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(postgres) => postgres.purge_admin_request_bodies_batch(batch_size).await,
            #[cfg(not(feature = "postgres"))]
            Self::Disabled(_) => unreachable!("a SQL backend cannot exist without a driver"),
        }
    }
}
