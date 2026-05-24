use super::{
    summarize_pool, DataBackends, MysqlBackend, PostgresBackend, SqlBackendRef, SqliteBackend,
};
use crate::error::{SqlResultExt, SqlxResultExt};
use crate::maintenance::{
    DatabaseMaintenanceSummary, DatabasePoolSummary, StatsDailyAggregationInput,
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

fn maintenance_identifier(value: &str) -> Result<&str, DataLayerError> {
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

impl PostgresBackend {
    pub async fn run_table_maintenance(
        &self,
        table_names: &[&str],
    ) -> Result<DatabaseMaintenanceSummary, DataLayerError> {
        let mut summary = DatabaseMaintenanceSummary::default();
        for table_name in table_names {
            let table_name = maintenance_identifier(table_name)?;
            summary.attempted += 1;
            let statement = format!("VACUUM ANALYZE \"{table_name}\"");
            if sqlx::raw_sql(&statement)
                .execute(self.pool())
                .await
                .map_postgres_err()
                .is_ok()
            {
                summary.succeeded += 1;
            }
        }
        Ok(summary)
    }
}

impl MysqlBackend {
    pub async fn run_table_maintenance(
        &self,
        table_names: &[&str],
    ) -> Result<DatabaseMaintenanceSummary, DataLayerError> {
        let mut summary = DatabaseMaintenanceSummary::default();
        for table_name in table_names {
            let table_name = maintenance_identifier(table_name)?;
            summary.attempted += 1;
            let statement = format!("ANALYZE TABLE `{table_name}`");
            if sqlx::raw_sql(&statement)
                .execute(self.pool())
                .await
                .map_sql_err()
                .is_ok()
            {
                summary.succeeded += 1;
            }
        }
        Ok(summary)
    }
}

impl SqliteBackend {
    pub async fn run_table_maintenance(
        &self,
        table_names: &[&str],
    ) -> Result<DatabaseMaintenanceSummary, DataLayerError> {
        let mut summary = DatabaseMaintenanceSummary::default();
        for table_name in table_names {
            let table_name = maintenance_identifier(table_name)?;
            summary.attempted += 1;
            let statement = format!("ANALYZE \"{table_name}\"");
            if sqlx::raw_sql(&statement)
                .execute(self.pool())
                .await
                .map_sql_err()
                .is_ok()
            {
                summary.succeeded += 1;
            }
        }
        if summary.succeeded > 0 {
            sqlx::raw_sql("PRAGMA optimize")
                .execute(self.pool())
                .await
                .map_sql_err()?;
        }
        Ok(summary)
    }
}

impl<'a> SqlBackendRef<'a> {
    async fn run_database_maintenance(
        self,
        table_names: &[&str],
    ) -> Result<DatabaseMaintenanceSummary, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.run_table_maintenance(table_names).await,
            Self::Mysql(mysql) => mysql.run_table_maintenance(table_names).await,
            Self::Sqlite(sqlite) => sqlite.run_table_maintenance(table_names).await,
        }
    }

    async fn run_database_migrations(self) -> Result<bool, MigrateError> {
        match self {
            Self::Postgres(postgres) => {
                crate::lifecycle::migrate::run_migrations(postgres.pool()).await?;
                Ok(true)
            }
            Self::Mysql(mysql) => {
                crate::lifecycle::migrate::run_mysql_migrations(mysql.pool()).await?;
                Ok(true)
            }
            Self::Sqlite(sqlite) => {
                crate::lifecycle::migrate::run_sqlite_migrations(sqlite.pool()).await?;
                Ok(true)
            }
        }
    }

    async fn run_database_backfills(self) -> Result<bool, MigrateError> {
        match self {
            Self::Postgres(postgres) => {
                crate::lifecycle::backfill::run_backfills(postgres.pool()).await?;
                Ok(true)
            }
            Self::Mysql(mysql) => {
                crate::lifecycle::backfill::run_mysql_backfills(mysql.pool()).await?;
                Ok(true)
            }
            Self::Sqlite(sqlite) => {
                crate::lifecycle::backfill::run_sqlite_backfills(sqlite.pool()).await?;
                Ok(true)
            }
        }
    }

    async fn pending_database_migrations(
        self,
    ) -> Result<Option<Vec<crate::lifecycle::migrate::PendingMigrationInfo>>, MigrateError> {
        match self {
            Self::Postgres(postgres) => Ok(Some(
                crate::lifecycle::migrate::pending_migrations(postgres.pool()).await?,
            )),
            Self::Mysql(mysql) => Ok(Some(
                crate::lifecycle::migrate::pending_mysql_migrations(mysql.pool()).await?,
            )),
            Self::Sqlite(sqlite) => Ok(Some(
                crate::lifecycle::migrate::pending_sqlite_migrations(sqlite.pool()).await?,
            )),
        }
    }

    async fn prepare_database_for_startup(
        self,
    ) -> Result<Option<Vec<crate::lifecycle::migrate::PendingMigrationInfo>>, MigrateError> {
        match self {
            Self::Postgres(postgres) => Ok(Some(
                crate::lifecycle::migrate::prepare_database_for_startup(postgres.pool()).await?,
            )),
            Self::Mysql(mysql) => Ok(Some(
                crate::lifecycle::migrate::prepare_mysql_database_for_startup(mysql.pool()).await?,
            )),
            Self::Sqlite(sqlite) => Ok(Some(
                crate::lifecycle::migrate::prepare_sqlite_database_for_startup(sqlite.pool())
                    .await?,
            )),
        }
    }

    async fn pending_database_backfills(
        self,
    ) -> Result<Option<Vec<crate::lifecycle::backfill::PendingBackfillInfo>>, MigrateError> {
        match self {
            Self::Postgres(postgres) => Ok(Some(
                crate::lifecycle::backfill::pending_backfills(postgres.pool()).await?,
            )),
            Self::Mysql(mysql) => Ok(Some(
                crate::lifecycle::backfill::pending_mysql_backfills(mysql.pool()).await?,
            )),
            Self::Sqlite(sqlite) => Ok(Some(
                crate::lifecycle::backfill::pending_sqlite_backfills(sqlite.pool()).await?,
            )),
        }
    }

    fn database_pool_summary(self) -> DatabasePoolSummary {
        match self {
            Self::Postgres(postgres) => summarize_pool(
                crate::database::DatabaseDriver::Postgres,
                usize::try_from(postgres.pool().size()).unwrap_or(usize::MAX),
                postgres.pool().num_idle(),
                postgres.config().max_connections,
            ),
            Self::Mysql(mysql) => summarize_pool(
                crate::database::DatabaseDriver::Mysql,
                usize::try_from(mysql.pool().size()).unwrap_or(usize::MAX),
                mysql.pool().num_idle(),
                mysql.config().pool.max_connections,
            ),
            Self::Sqlite(sqlite) => summarize_pool(
                crate::database::DatabaseDriver::Sqlite,
                usize::try_from(sqlite.pool().size()).unwrap_or(usize::MAX),
                sqlite.pool().num_idle(),
                sqlite.config().pool.max_connections,
            ),
        }
    }

    async fn aggregate_wallet_daily_usage(
        self,
        input: &WalletDailyUsageAggregationInput,
    ) -> Result<WalletDailyUsageAggregationResult, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.aggregate_wallet_daily_usage(input).await,
            Self::Mysql(mysql) => mysql.aggregate_wallet_daily_usage(input).await,
            Self::Sqlite(sqlite) => sqlite.aggregate_wallet_daily_usage(input).await,
        }
    }

    async fn aggregate_stats_hourly(
        self,
        input: &StatsHourlyAggregationInput,
    ) -> Result<Option<StatsHourlyAggregationSummary>, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.aggregate_stats_hourly(input).await,
            Self::Mysql(mysql) => mysql.aggregate_stats_hourly(input).await,
            Self::Sqlite(sqlite) => sqlite.aggregate_stats_hourly(input).await,
        }
    }

    async fn aggregate_stats_daily(
        self,
        input: &StatsDailyAggregationInput,
    ) -> Result<Option<StatsDailyAggregationSummary>, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.aggregate_stats_daily(input).await,
            Self::Mysql(mysql) => mysql.aggregate_stats_daily(input).await,
            Self::Sqlite(sqlite) => sqlite.aggregate_stats_daily(input).await,
        }
    }

    async fn find_system_config_value(
        self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.find_system_config_value(key).await,
            Self::Mysql(mysql) => mysql.find_system_config_value(key).await,
            Self::Sqlite(sqlite) => sqlite.find_system_config_value(key).await,
        }
    }

    async fn list_system_config_entries(
        self,
    ) -> Result<Vec<StoredSystemConfigEntry>, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.list_system_config_entries().await,
            Self::Mysql(mysql) => mysql.list_system_config_entries().await,
            Self::Sqlite(sqlite) => sqlite.list_system_config_entries().await,
        }
    }

    async fn upsert_system_config_entry(
        self,
        key: &str,
        value: &serde_json::Value,
        description: Option<&str>,
    ) -> Result<StoredSystemConfigEntry, DataLayerError> {
        match self {
            Self::Postgres(postgres) => {
                postgres
                    .upsert_system_config_entry(key, value, description)
                    .await
            }
            Self::Mysql(mysql) => {
                mysql
                    .upsert_system_config_entry(key, value, description)
                    .await
            }
            Self::Sqlite(sqlite) => {
                sqlite
                    .upsert_system_config_entry(key, value, description)
                    .await
            }
        }
    }

    async fn delete_system_config_value(self, key: &str) -> Result<bool, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.delete_system_config_value(key).await,
            Self::Mysql(mysql) => mysql.delete_system_config_value(key).await,
            Self::Sqlite(sqlite) => sqlite.delete_system_config_value(key).await,
        }
    }

    async fn read_admin_system_stats(self) -> Result<AdminSystemStats, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.read_admin_system_stats().await,
            Self::Mysql(mysql) => mysql.read_admin_system_stats().await,
            Self::Sqlite(sqlite) => sqlite.read_admin_system_stats().await,
        }
    }

    async fn purge_admin_system_data(
        self,
        target: AdminSystemPurgeTarget,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.purge_admin_system_data(target).await,
            Self::Mysql(mysql) => mysql.purge_admin_system_data(target).await,
            Self::Sqlite(sqlite) => sqlite.purge_admin_system_data(target).await,
        }
    }

    async fn export_admin_system_usage_aggregates(
        self,
    ) -> Result<AdminSystemUsageAggregateSnapshot, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.export_admin_system_usage_aggregates().await,
            Self::Mysql(mysql) => mysql.export_admin_system_usage_aggregates().await,
            Self::Sqlite(sqlite) => sqlite.export_admin_system_usage_aggregates().await,
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
            Self::Mysql(mysql) => {
                mysql
                    .import_admin_system_usage_aggregates(
                        snapshot,
                        user_id_map,
                        api_key_id_map,
                        mode,
                    )
                    .await
            }
            Self::Sqlite(sqlite) => {
                sqlite
                    .import_admin_system_usage_aggregates(
                        snapshot,
                        user_id_map,
                        api_key_id_map,
                        mode,
                    )
                    .await
            }
        }
    }

    async fn purge_admin_request_bodies_batch(
        self,
        batch_size: usize,
    ) -> Result<AdminSystemPurgeSummary, DataLayerError> {
        match self {
            Self::Postgres(postgres) => postgres.purge_admin_request_bodies_batch(batch_size).await,
            Self::Mysql(mysql) => mysql.purge_admin_request_bodies_batch(batch_size).await,
            Self::Sqlite(sqlite) => sqlite.purge_admin_request_bodies_batch(batch_size).await,
        }
    }
}
