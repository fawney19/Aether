use sqlx::Row;

use crate::backend::PostgresBackend;
use crate::error::SqlxResultExt;
use crate::maintenance::{
    DatabaseMaintenanceSummary, DatabasePostgresActivityGroup,
    DatabasePostgresObservabilitySnapshot,
};
use crate::DataLayerError;

use super::maintenance_identifier;

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

    pub async fn postgres_observability_snapshot(
        &self,
    ) -> Result<DatabasePostgresObservabilitySnapshot, DataLayerError> {
        const ACTIVITY_SQL: &str = r#"
SELECT
    COUNT(*) FILTER (WHERE state = 'active')::BIGINT AS active_connections,
    COUNT(*) FILTER (WHERE state = 'idle')::BIGINT AS idle_connections,
    COUNT(*) FILTER (WHERE state = 'idle in transaction')::BIGINT AS idle_in_transaction_connections,
    COUNT(*) FILTER (WHERE state = 'active' AND wait_event_type IS NOT NULL)::BIGINT AS waiting_connections,
    COUNT(*) FILTER (WHERE state = 'active' AND wait_event_type = 'Lock')::BIGINT AS lock_waiting_connections,
    COALESCE(MAX(EXTRACT(EPOCH FROM now() - query_start) * 1000) FILTER (WHERE state = 'active' AND query_start IS NOT NULL), 0)::BIGINT AS oldest_active_query_age_ms,
    COALESCE(MAX(EXTRACT(EPOCH FROM now() - xact_start) * 1000) FILTER (WHERE xact_start IS NOT NULL), 0)::BIGINT AS oldest_transaction_age_ms
FROM pg_stat_activity
WHERE datname = current_database()
  AND pid <> pg_backend_pid()
  AND backend_type = 'client backend'
"#;
        const DEADLOCKS_SQL: &str = r#"
SELECT
    COALESCE(SUM(deadlocks), 0)::BIGINT AS deadlocks_total,
    COALESCE(SUM(blks_read), 0)::BIGINT AS block_read_total,
    COALESCE(SUM(blks_hit), 0)::BIGINT AS block_hit_total,
    COALESCE(SUM(temp_files), 0)::BIGINT AS temp_files_total,
    COALESCE(SUM(temp_bytes), 0)::BIGINT AS temp_bytes_total,
    COALESCE(SUM(xact_commit), 0)::BIGINT AS xact_commit_total,
    COALESCE(SUM(xact_rollback), 0)::BIGINT AS xact_rollback_total
FROM pg_stat_database
WHERE datname = current_database()
"#;

        let activity = sqlx::query(ACTIVITY_SQL)
            .fetch_one(self.pool())
            .await
            .map_postgres_err()?;
        let database = sqlx::query(DEADLOCKS_SQL)
            .fetch_one(self.pool())
            .await
            .map_postgres_err()?;
        let wal = self.postgres_wal_observability_snapshot().await;
        let checkpoint = self.postgres_checkpoint_observability_snapshot().await;
        let statements = self.postgres_statement_observability_snapshot().await;
        let block_read_total = row_u64(&database, "block_read_total")?;
        let block_hit_total = row_u64(&database, "block_hit_total")?;

        Ok(DatabasePostgresObservabilitySnapshot {
            active_connections: row_u64(&activity, "active_connections")?,
            idle_connections: row_u64(&activity, "idle_connections")?,
            idle_in_transaction_connections: row_u64(&activity, "idle_in_transaction_connections")?,
            waiting_connections: row_u64(&activity, "waiting_connections")?,
            lock_waiting_connections: row_u64(&activity, "lock_waiting_connections")?,
            oldest_active_query_age_ms: row_u64(&activity, "oldest_active_query_age_ms")?,
            oldest_transaction_age_ms: row_u64(&activity, "oldest_transaction_age_ms")?,
            deadlocks_total: row_u64(&database, "deadlocks_total")?,
            block_read_total,
            block_hit_total,
            block_cache_hit_rate_basis_points: ratio_to_basis_points(
                block_hit_total,
                block_read_total.saturating_add(block_hit_total),
            ),
            temp_files_total: row_u64(&database, "temp_files_total")?,
            temp_bytes_total: row_u64(&database, "temp_bytes_total")?,
            xact_commit_total: row_u64(&database, "xact_commit_total")?,
            xact_rollback_total: row_u64(&database, "xact_rollback_total")?,
            wal_observability_available: wal.available,
            wal_observability_unavailable: wal.unavailable,
            wal_records_total: wal.records_total,
            wal_fpi_total: wal.fpi_total,
            wal_bytes_total: wal.bytes_total,
            wal_buffers_full_total: wal.buffers_full_total,
            wal_write_total: wal.write_total,
            wal_sync_total: wal.sync_total,
            wal_write_time_ms_total: wal.write_time_ms_total,
            wal_sync_time_ms_total: wal.sync_time_ms_total,
            checkpoint_observability_available: checkpoint.available,
            checkpoint_observability_unavailable: checkpoint.unavailable,
            checkpoints_timed_total: checkpoint.timed_total,
            checkpoints_requested_total: checkpoint.requested_total,
            checkpoint_write_time_ms_total: checkpoint.write_time_ms_total,
            checkpoint_sync_time_ms_total: checkpoint.sync_time_ms_total,
            buffers_checkpoint_total: checkpoint.buffers_checkpoint_total,
            buffers_backend_total: checkpoint.buffers_backend_total,
            statement_observability_available: statements.available,
            statement_observability_unavailable: statements.unavailable,
            statement_top_calls_total: statements.top_calls_total,
            statement_top_exec_time_ms_total: statements.top_exec_time_ms_total,
            statement_top_max_mean_exec_time_ms: statements.top_max_mean_exec_time_ms,
            statement_top_max_exec_time_ms: statements.top_max_exec_time_ms,
            statement_top_shared_blks_read_total: statements.top_shared_blks_read_total,
            statement_top_shared_blks_hit_total: statements.top_shared_blks_hit_total,
            statement_top_temp_blks_total: statements.top_temp_blks_total,
        })
    }

    pub async fn postgres_activity_groups(
        &self,
        limit: i64,
    ) -> Result<Vec<DatabasePostgresActivityGroup>, DataLayerError> {
        const ACTIVITY_GROUP_SQL: &str = r#"
WITH normalized_activity AS (
    SELECT
        COALESCE(NULLIF(state, ''), 'unknown') AS state,
        COALESCE(NULLIF(wait_event_type, ''), 'none') AS wait_event_type,
        COALESCE(NULLIF(wait_event, ''), 'none') AS wait_event,
        LEFT(
            regexp_replace(
                regexp_replace(
                    COALESCE(NULLIF(query, ''), '<empty>'),
                    '\s+',
                    ' ',
                    'g'
                ),
                '([0-9a-fA-F]{8,}|[0-9]+)',
                '?',
                'g'
            ),
            160
        ) AS query_prefix,
        COALESCE(EXTRACT(EPOCH FROM now() - query_start) * 1000, 0)::BIGINT AS query_age_ms,
        COALESCE(EXTRACT(EPOCH FROM now() - xact_start) * 1000, 0)::BIGINT AS transaction_age_ms
    FROM pg_stat_activity
    WHERE datname = current_database()
      AND pid <> pg_backend_pid()
      AND backend_type = 'client backend'
)
SELECT
    state,
    wait_event_type,
    wait_event,
    query_prefix,
    COUNT(*)::BIGINT AS connections,
    COALESCE(MAX(query_age_ms), 0)::BIGINT AS max_query_age_ms,
    COALESCE(MAX(transaction_age_ms), 0)::BIGINT AS max_transaction_age_ms
FROM normalized_activity
GROUP BY state, wait_event_type, wait_event, query_prefix
ORDER BY connections DESC, max_transaction_age_ms DESC, max_query_age_ms DESC
LIMIT $1
"#;
        let rows = sqlx::query(ACTIVITY_GROUP_SQL)
            .bind(limit.clamp(1, 20))
            .fetch_all(self.pool())
            .await
            .map_postgres_err()?;

        rows.into_iter()
            .map(|row| {
                Ok(DatabasePostgresActivityGroup {
                    state: row.try_get::<String, _>("state").map_postgres_err()?,
                    wait_event_type: row
                        .try_get::<String, _>("wait_event_type")
                        .map_postgres_err()?,
                    wait_event: row.try_get::<String, _>("wait_event").map_postgres_err()?,
                    query_prefix: row
                        .try_get::<String, _>("query_prefix")
                        .map_postgres_err()?,
                    connections: row_u64(&row, "connections")?,
                    max_query_age_ms: row_u64(&row, "max_query_age_ms")?,
                    max_transaction_age_ms: row_u64(&row, "max_transaction_age_ms")?,
                })
            })
            .collect()
    }

    async fn postgres_wal_observability_snapshot(&self) -> PostgresWalObservabilitySnapshot {
        if !self
            .postgres_catalog_relation_has_columns(
                "pg_catalog.pg_stat_wal",
                &["wal_records", "wal_fpi", "wal_bytes", "wal_buffers_full"],
            )
            .await
        {
            return PostgresWalObservabilitySnapshot::default();
        }

        const WAL_SQL: &str = r#"
SELECT
    COALESCE(SUM(wal_records), 0)::BIGINT AS records_total,
    COALESCE(SUM(wal_fpi), 0)::BIGINT AS fpi_total,
    COALESCE(SUM(wal_bytes), 0)::BIGINT AS bytes_total,
    COALESCE(SUM(wal_buffers_full), 0)::BIGINT AS buffers_full_total
FROM pg_stat_wal
"#;
        match sqlx::query(WAL_SQL).fetch_one(self.pool()).await {
            Ok(row) => {
                let io = self.postgres_wal_io_observability_snapshot().await;
                PostgresWalObservabilitySnapshot {
                    available: 1,
                    records_total: row_u64(&row, "records_total").unwrap_or_default(),
                    fpi_total: row_u64(&row, "fpi_total").unwrap_or_default(),
                    bytes_total: row_u64(&row, "bytes_total").unwrap_or_default(),
                    buffers_full_total: row_u64(&row, "buffers_full_total").unwrap_or_default(),
                    write_total: io.write_total,
                    sync_total: io.sync_total,
                    write_time_ms_total: io.write_time_ms_total,
                    sync_time_ms_total: io.sync_time_ms_total,
                    ..PostgresWalObservabilitySnapshot::default()
                }
            }
            Err(_) => PostgresWalObservabilitySnapshot {
                unavailable: 1,
                ..PostgresWalObservabilitySnapshot::default()
            },
        }
    }

    async fn postgres_wal_io_observability_snapshot(&self) -> PostgresWalIoObservabilitySnapshot {
        if self
            .postgres_catalog_relation_has_columns(
                "pg_catalog.pg_stat_wal",
                &["wal_write", "wal_sync", "wal_write_time", "wal_sync_time"],
            )
            .await
        {
            return self.postgres_wal_legacy_io_observability_snapshot().await;
        }

        if !self
            .postgres_catalog_relation_has_columns(
                "pg_catalog.pg_stat_io",
                &["object", "writes", "fsyncs", "write_time", "fsync_time"],
            )
            .await
        {
            return PostgresWalIoObservabilitySnapshot::default();
        }

        const WAL_IO_SQL: &str = r#"
SELECT
    COALESCE(SUM(writes), 0)::BIGINT AS write_total,
    COALESCE(SUM(fsyncs), 0)::BIGINT AS sync_total,
    COALESCE(SUM(write_time), 0)::BIGINT AS write_time_ms_total,
    COALESCE(SUM(fsync_time), 0)::BIGINT AS sync_time_ms_total
FROM pg_stat_io
WHERE object = 'wal'
"#;
        match sqlx::query(WAL_IO_SQL).fetch_one(self.pool()).await {
            Ok(row) => PostgresWalIoObservabilitySnapshot {
                write_total: row_u64(&row, "write_total").unwrap_or_default(),
                sync_total: row_u64(&row, "sync_total").unwrap_or_default(),
                write_time_ms_total: row_u64(&row, "write_time_ms_total").unwrap_or_default(),
                sync_time_ms_total: row_u64(&row, "sync_time_ms_total").unwrap_or_default(),
            },
            Err(_) => PostgresWalIoObservabilitySnapshot::default(),
        }
    }

    async fn postgres_wal_legacy_io_observability_snapshot(
        &self,
    ) -> PostgresWalIoObservabilitySnapshot {
        const WAL_IO_SQL: &str = r#"
SELECT
    COALESCE(SUM(wal_write), 0)::BIGINT AS write_total,
    COALESCE(SUM(wal_sync), 0)::BIGINT AS sync_total,
    COALESCE(SUM(wal_write_time), 0)::BIGINT AS write_time_ms_total,
    COALESCE(SUM(wal_sync_time), 0)::BIGINT AS sync_time_ms_total
FROM pg_stat_wal
"#;
        match sqlx::query(WAL_IO_SQL).fetch_one(self.pool()).await {
            Ok(row) => PostgresWalIoObservabilitySnapshot {
                write_total: row_u64(&row, "write_total").unwrap_or_default(),
                sync_total: row_u64(&row, "sync_total").unwrap_or_default(),
                write_time_ms_total: row_u64(&row, "write_time_ms_total").unwrap_or_default(),
                sync_time_ms_total: row_u64(&row, "sync_time_ms_total").unwrap_or_default(),
            },
            Err(_) => PostgresWalIoObservabilitySnapshot::default(),
        }
    }

    async fn postgres_checkpoint_observability_snapshot(
        &self,
    ) -> PostgresCheckpointObservabilitySnapshot {
        if self
            .postgres_catalog_relation_has_columns(
                "pg_catalog.pg_stat_checkpointer",
                &[
                    "num_timed",
                    "num_requested",
                    "write_time",
                    "sync_time",
                    "buffers_written",
                ],
            )
            .await
        {
            return self
                .postgres_checkpoint_observability_snapshot_from_checkpointer()
                .await;
        }

        if !self
            .postgres_catalog_relation_has_columns(
                "pg_catalog.pg_stat_bgwriter",
                &[
                    "checkpoints_timed",
                    "checkpoints_req",
                    "checkpoint_write_time",
                    "checkpoint_sync_time",
                    "buffers_checkpoint",
                    "buffers_backend",
                ],
            )
            .await
        {
            return PostgresCheckpointObservabilitySnapshot::default();
        }

        const CHECKPOINT_SQL: &str = r#"
SELECT
    COALESCE(SUM(checkpoints_timed), 0)::BIGINT AS timed_total,
    COALESCE(SUM(checkpoints_req), 0)::BIGINT AS requested_total,
    COALESCE(SUM(checkpoint_write_time), 0)::BIGINT AS write_time_ms_total,
    COALESCE(SUM(checkpoint_sync_time), 0)::BIGINT AS sync_time_ms_total,
    COALESCE(SUM(buffers_checkpoint), 0)::BIGINT AS buffers_checkpoint_total,
    COALESCE(SUM(buffers_backend), 0)::BIGINT AS buffers_backend_total
FROM pg_stat_bgwriter
"#;
        match sqlx::query(CHECKPOINT_SQL).fetch_one(self.pool()).await {
            Ok(row) => PostgresCheckpointObservabilitySnapshot {
                available: 1,
                timed_total: row_u64(&row, "timed_total").unwrap_or_default(),
                requested_total: row_u64(&row, "requested_total").unwrap_or_default(),
                write_time_ms_total: row_u64(&row, "write_time_ms_total").unwrap_or_default(),
                sync_time_ms_total: row_u64(&row, "sync_time_ms_total").unwrap_or_default(),
                buffers_checkpoint_total: row_u64(&row, "buffers_checkpoint_total")
                    .unwrap_or_default(),
                buffers_backend_total: row_u64(&row, "buffers_backend_total").unwrap_or_default(),
                ..PostgresCheckpointObservabilitySnapshot::default()
            },
            Err(_) => PostgresCheckpointObservabilitySnapshot {
                unavailable: 1,
                ..PostgresCheckpointObservabilitySnapshot::default()
            },
        }
    }

    async fn postgres_checkpoint_observability_snapshot_from_checkpointer(
        &self,
    ) -> PostgresCheckpointObservabilitySnapshot {
        const CHECKPOINT_SQL: &str = r#"
SELECT
    COALESCE(SUM(num_timed), 0)::BIGINT AS timed_total,
    COALESCE(SUM(num_requested), 0)::BIGINT AS requested_total,
    COALESCE(SUM(write_time), 0)::BIGINT AS write_time_ms_total,
    COALESCE(SUM(sync_time), 0)::BIGINT AS sync_time_ms_total,
    COALESCE(SUM(buffers_written), 0)::BIGINT AS buffers_checkpoint_total
FROM pg_stat_checkpointer
"#;
        match sqlx::query(CHECKPOINT_SQL).fetch_one(self.pool()).await {
            Ok(row) => PostgresCheckpointObservabilitySnapshot {
                available: 1,
                timed_total: row_u64(&row, "timed_total").unwrap_or_default(),
                requested_total: row_u64(&row, "requested_total").unwrap_or_default(),
                write_time_ms_total: row_u64(&row, "write_time_ms_total").unwrap_or_default(),
                sync_time_ms_total: row_u64(&row, "sync_time_ms_total").unwrap_or_default(),
                buffers_checkpoint_total: row_u64(&row, "buffers_checkpoint_total")
                    .unwrap_or_default(),
                ..PostgresCheckpointObservabilitySnapshot::default()
            },
            Err(_) => PostgresCheckpointObservabilitySnapshot {
                unavailable: 1,
                ..PostgresCheckpointObservabilitySnapshot::default()
            },
        }
    }

    async fn postgres_statement_observability_snapshot(
        &self,
    ) -> PostgresStatementObservabilitySnapshot {
        let extension_installed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements')",
        )
        .fetch_one(self.pool())
        .await
        .unwrap_or(false);
        if !extension_installed {
            return PostgresStatementObservabilitySnapshot::default();
        }
        if self
            .postgres_catalog_relation_has_columns(
                "pg_stat_statements",
                &[
                    "calls",
                    "total_exec_time",
                    "mean_exec_time",
                    "max_exec_time",
                    "shared_blks_read",
                    "shared_blks_hit",
                    "temp_blks_read",
                    "temp_blks_written",
                    "dbid",
                ],
            )
            .await
        {
            return self
                .postgres_statement_observability_snapshot_with_exec_time()
                .await;
        }
        if self
            .postgres_catalog_relation_has_columns(
                "pg_stat_statements",
                &[
                    "calls",
                    "total_time",
                    "mean_time",
                    "max_time",
                    "shared_blks_read",
                    "shared_blks_hit",
                    "temp_blks_read",
                    "temp_blks_written",
                    "dbid",
                ],
            )
            .await
        {
            return self
                .postgres_statement_observability_snapshot_with_total_time()
                .await;
        }

        PostgresStatementObservabilitySnapshot::default()
    }

    async fn postgres_statement_observability_snapshot_with_exec_time(
        &self,
    ) -> PostgresStatementObservabilitySnapshot {
        const STATEMENTS_SQL: &str = r#"
SELECT
    COALESCE(SUM(calls), 0)::BIGINT AS top_calls_total,
    COALESCE(SUM(total_exec_time), 0)::BIGINT AS top_exec_time_ms_total,
    COALESCE(MAX(mean_exec_time), 0)::BIGINT AS top_max_mean_exec_time_ms,
    COALESCE(MAX(max_exec_time), 0)::BIGINT AS top_max_exec_time_ms,
    COALESCE(SUM(shared_blks_read), 0)::BIGINT AS top_shared_blks_read_total,
    COALESCE(SUM(shared_blks_hit), 0)::BIGINT AS top_shared_blks_hit_total,
    COALESCE(SUM(temp_blks_read + temp_blks_written), 0)::BIGINT AS top_temp_blks_total
FROM (
    SELECT
        calls,
        total_exec_time,
        mean_exec_time,
        max_exec_time,
        shared_blks_read,
        shared_blks_hit,
        temp_blks_read,
        temp_blks_written
    FROM pg_stat_statements
    WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
    ORDER BY total_exec_time DESC
    LIMIT 20
) top_statements
"#;
        match sqlx::query(STATEMENTS_SQL).fetch_one(self.pool()).await {
            Ok(row) => PostgresStatementObservabilitySnapshot {
                available: 1,
                top_calls_total: row_u64(&row, "top_calls_total").unwrap_or_default(),
                top_exec_time_ms_total: row_u64(&row, "top_exec_time_ms_total").unwrap_or_default(),
                top_max_mean_exec_time_ms: row_u64(&row, "top_max_mean_exec_time_ms")
                    .unwrap_or_default(),
                top_max_exec_time_ms: row_u64(&row, "top_max_exec_time_ms").unwrap_or_default(),
                top_shared_blks_read_total: row_u64(&row, "top_shared_blks_read_total")
                    .unwrap_or_default(),
                top_shared_blks_hit_total: row_u64(&row, "top_shared_blks_hit_total")
                    .unwrap_or_default(),
                top_temp_blks_total: row_u64(&row, "top_temp_blks_total").unwrap_or_default(),
                ..PostgresStatementObservabilitySnapshot::default()
            },
            Err(_) => PostgresStatementObservabilitySnapshot {
                unavailable: 1,
                ..PostgresStatementObservabilitySnapshot::default()
            },
        }
    }

    async fn postgres_statement_observability_snapshot_with_total_time(
        &self,
    ) -> PostgresStatementObservabilitySnapshot {
        const STATEMENTS_SQL: &str = r#"
SELECT
    COALESCE(SUM(calls), 0)::BIGINT AS top_calls_total,
    COALESCE(SUM(total_time), 0)::BIGINT AS top_exec_time_ms_total,
    COALESCE(MAX(mean_time), 0)::BIGINT AS top_max_mean_exec_time_ms,
    COALESCE(MAX(max_time), 0)::BIGINT AS top_max_exec_time_ms,
    COALESCE(SUM(shared_blks_read), 0)::BIGINT AS top_shared_blks_read_total,
    COALESCE(SUM(shared_blks_hit), 0)::BIGINT AS top_shared_blks_hit_total,
    COALESCE(SUM(temp_blks_read + temp_blks_written), 0)::BIGINT AS top_temp_blks_total
FROM (
    SELECT
        calls,
        total_time,
        mean_time,
        max_time,
        shared_blks_read,
        shared_blks_hit,
        temp_blks_read,
        temp_blks_written
    FROM pg_stat_statements
    WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
    ORDER BY total_time DESC
    LIMIT 20
) top_statements
"#;
        match sqlx::query(STATEMENTS_SQL).fetch_one(self.pool()).await {
            Ok(row) => PostgresStatementObservabilitySnapshot {
                available: 1,
                top_calls_total: row_u64(&row, "top_calls_total").unwrap_or_default(),
                top_exec_time_ms_total: row_u64(&row, "top_exec_time_ms_total").unwrap_or_default(),
                top_max_mean_exec_time_ms: row_u64(&row, "top_max_mean_exec_time_ms")
                    .unwrap_or_default(),
                top_max_exec_time_ms: row_u64(&row, "top_max_exec_time_ms").unwrap_or_default(),
                top_shared_blks_read_total: row_u64(&row, "top_shared_blks_read_total")
                    .unwrap_or_default(),
                top_shared_blks_hit_total: row_u64(&row, "top_shared_blks_hit_total")
                    .unwrap_or_default(),
                top_temp_blks_total: row_u64(&row, "top_temp_blks_total").unwrap_or_default(),
                ..PostgresStatementObservabilitySnapshot::default()
            },
            Err(_) => PostgresStatementObservabilitySnapshot {
                unavailable: 1,
                ..PostgresStatementObservabilitySnapshot::default()
            },
        }
    }

    async fn postgres_catalog_relation_has_columns(
        &self,
        relation: &str,
        columns: &[&str],
    ) -> bool {
        if !self.postgres_catalog_relation_exists(relation).await {
            return false;
        }

        for column in columns {
            if !self.postgres_catalog_column_exists(relation, column).await {
                return false;
            }
        }
        true
    }

    async fn postgres_catalog_column_exists(&self, relation: &str, column: &str) -> bool {
        sqlx::query_scalar::<_, bool>(
            r#"
SELECT EXISTS (
    SELECT 1
    FROM pg_attribute
    WHERE attrelid = to_regclass($1)
      AND attname = $2
      AND NOT attisdropped
)
"#,
        )
        .bind(relation)
        .bind(column)
        .fetch_one(self.pool())
        .await
        .unwrap_or(false)
    }

    async fn postgres_catalog_relation_exists(&self, relation: &str) -> bool {
        sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass($1)::TEXT")
            .bind(relation)
            .fetch_one(self.pool())
            .await
            .ok()
            .flatten()
            .is_some()
    }
}

fn row_u64(row: &sqlx::postgres::PgRow, name: &str) -> Result<u64, DataLayerError> {
    row.try_get::<i64, _>(name)
        .map(u64_from_i64)
        .map_postgres_err()
}

fn u64_from_i64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn ratio_to_basis_points(value: u64, total: u64) -> u64 {
    value.saturating_mul(10_000).checked_div(total).unwrap_or(0)
}

#[derive(Debug, Clone, Copy, Default)]
struct PostgresWalObservabilitySnapshot {
    available: u64,
    unavailable: u64,
    records_total: u64,
    fpi_total: u64,
    bytes_total: u64,
    buffers_full_total: u64,
    write_total: u64,
    sync_total: u64,
    write_time_ms_total: u64,
    sync_time_ms_total: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct PostgresWalIoObservabilitySnapshot {
    write_total: u64,
    sync_total: u64,
    write_time_ms_total: u64,
    sync_time_ms_total: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct PostgresCheckpointObservabilitySnapshot {
    available: u64,
    unavailable: u64,
    timed_total: u64,
    requested_total: u64,
    write_time_ms_total: u64,
    sync_time_ms_total: u64,
    buffers_checkpoint_total: u64,
    buffers_backend_total: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct PostgresStatementObservabilitySnapshot {
    available: u64,
    unavailable: u64,
    top_calls_total: u64,
    top_exec_time_ms_total: u64,
    top_max_mean_exec_time_ms: u64,
    top_max_exec_time_ms: u64,
    top_shared_blks_read_total: u64,
    top_shared_blks_hit_total: u64,
    top_temp_blks_total: u64,
}
