//! Maintenance and aggregation DTOs used by the runtime data layer.
//!
//! These are not cross-crate repository contracts, but they are shared across
//! the backend composition layer and maintenance entrypoints.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DatabaseMaintenanceSummary {
    pub attempted: usize,
    pub succeeded: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DatabasePoolSummary {
    pub driver: crate::database::DatabaseDriver,
    pub checked_out: usize,
    pub pool_size: usize,
    pub idle: usize,
    pub max_connections: u32,
    pub usage_rate: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DatabasePostgresObservabilitySnapshot {
    pub active_connections: u64,
    pub idle_connections: u64,
    pub idle_in_transaction_connections: u64,
    pub waiting_connections: u64,
    pub lock_waiting_connections: u64,
    pub oldest_active_query_age_ms: u64,
    pub oldest_transaction_age_ms: u64,
    pub deadlocks_total: u64,
    pub block_read_total: u64,
    pub block_hit_total: u64,
    pub block_cache_hit_rate_basis_points: u64,
    pub temp_files_total: u64,
    pub temp_bytes_total: u64,
    pub xact_commit_total: u64,
    pub xact_rollback_total: u64,
    pub wal_observability_available: u64,
    pub wal_observability_unavailable: u64,
    pub wal_records_total: u64,
    pub wal_fpi_total: u64,
    pub wal_bytes_total: u64,
    pub wal_buffers_full_total: u64,
    pub wal_write_total: u64,
    pub wal_sync_total: u64,
    pub wal_write_time_ms_total: u64,
    pub wal_sync_time_ms_total: u64,
    pub checkpoint_observability_available: u64,
    pub checkpoint_observability_unavailable: u64,
    pub checkpoints_timed_total: u64,
    pub checkpoints_requested_total: u64,
    pub checkpoint_write_time_ms_total: u64,
    pub checkpoint_sync_time_ms_total: u64,
    pub buffers_checkpoint_total: u64,
    pub buffers_backend_total: u64,
    pub statement_observability_available: u64,
    pub statement_observability_unavailable: u64,
    pub statement_top_calls_total: u64,
    pub statement_top_exec_time_ms_total: u64,
    pub statement_top_max_mean_exec_time_ms: u64,
    pub statement_top_max_exec_time_ms: u64,
    pub statement_top_shared_blks_read_total: u64,
    pub statement_top_shared_blks_hit_total: u64,
    pub statement_top_temp_blks_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DatabasePostgresActivityGroup {
    pub state: String,
    pub wait_event_type: String,
    pub wait_event: String,
    pub query_prefix: String,
    pub connections: u64,
    pub max_query_age_ms: u64,
    pub max_transaction_age_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletDailyUsageAggregationInput {
    pub billing_date: String,
    pub billing_timezone: String,
    pub window_start_unix_secs: u64,
    pub window_end_unix_secs: u64,
    pub aggregated_at_unix_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WalletDailyUsageAggregationResult {
    pub aggregated_wallets: usize,
    pub deleted_stale_ledgers: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsHourlyAggregationInput {
    pub target_hour_utc: DateTime<Utc>,
    pub aggregated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsDailyAggregationInput {
    pub target_day_utc: DateTime<Utc>,
    pub aggregated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsDailyAggregationSummary {
    pub day_start_utc: DateTime<Utc>,
    pub total_requests: i64,
    pub model_rows: usize,
    pub provider_rows: usize,
    pub api_key_rows: usize,
    pub error_rows: usize,
    pub user_rows: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatsHourlyAggregationSummary {
    pub hour_utc: DateTime<Utc>,
    pub total_requests: i64,
    pub user_rows: usize,
    pub user_model_rows: usize,
    pub model_rows: usize,
    pub provider_rows: usize,
}
